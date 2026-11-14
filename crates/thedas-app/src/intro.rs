//! The movies the game plays as it boots, before the start menu: the exe's boot list
//! (0x5ec247, filled by 0x5dd870) is `dragon_age_ea_logo.bik` then `dragon_age_main.bik`,
//! from `packages/core/data/movies`.
//!
//! The Bink movies are decoded by our own decoder ([`thedas_video::bink`]); their sound
//! plays from [`crate::audio`]. A key, a click or the pad's A, B or
//! Start skips the movie playing (the exe's skip input is not traced). The options'
//! `DisableIntroMovies` (and the exe's `FirstRun` gate) are not read; `--no-intro` skips
//! them.
//!
//! The same player shows the movies cutscenes play ([`CutsceneMovies`]).

use std::collections::VecDeque;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, TryRecvError, sync_channel};
use thedas_video::bink::{Movie, VideoDecoder};

use crate::ui::Composite;

/// The exe's boot movies, in the order it plays them.
pub const BOOT_MOVIES: [&str; 2] = ["dragon_age_ea_logo.bik", "dragon_age_main.bik"];

/// Where the movies are, under the install.
pub const MOVIES_DIR: &str = "packages/core/data/movies";

/// What a Bink file's header says about its video.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BinkHeader {
    pub frames: u32,
    pub width: u32,
    pub height: u32,
    pub fps_num: u32,
    pub fps_den: u32,
}

impl BinkHeader {
    /// Reads the header at the start of a `.bik` (`BIK` and a revision letter, then little-
    /// endian u32s: the size, the frame count, the largest frame, one more, the width, the
    /// height and the frame rate as a fraction).
    pub fn parse(b: &[u8]) -> Option<Self> {
        if b.len() < 36 || &b[..3] != b"BIK" {
            return None;
        }
        let u = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
        let h = Self {
            frames: u(8),
            width: u(20),
            height: u(24),
            fps_num: u(28),
            fps_den: u(32),
        };
        (h.width > 0 && h.height > 0 && h.width <= 8192 && h.height <= 8192 && h.fps_num > 0)
            .then_some(h)
            .map(|h| Self {
                fps_den: h.fps_den.max(1),
                ..h
            })
    }

    pub fn fps(&self) -> f64 {
        self.fps_num as f64 / self.fps_den as f64
    }

    /// The frame due `ms` into the movie.
    pub fn frame_at(&self, ms: f64) -> u64 {
        (ms.max(0.0) / 1000.0 * self.fps()).floor() as u64
    }
}

/// Where a `w`×`h` frame sits in a `size` window: as large as fits, its shape kept, centred
/// (the rest is black). `(x, y, w, h)` in pixels.
pub fn letterbox(w: u32, h: u32, size: (u32, u32)) -> (f32, f32, f32, f32) {
    let (sw, sh) = (size.0 as f32, size.1 as f32);
    let scale = (sw / w as f32).min(sh / h as f32);
    let (dw, dh) = (w as f32 * scale, h as f32 * scale);
    ((sw - dw) / 2.0, (sh - dh) / 2.0, dw, dh)
}

/// Where movie `name` (`bhmcs_intro.bik`) is: the loose file the resource stack resolves it
/// to (modules, packages, addins and overrides, highest layer first, as every other resource),
/// else `packages/core/data/movies`. Origin openings keep their movies in the module
/// (`modules/single player/data/movies`), so a fixed core folder missed them and their
/// cutscenes ended at once. Provisional: the exe's own movie path resolution is not traced; the
/// resource stack is the order the rest of the game's data follows.
pub fn movie_path(vfs: &thedas_vfs::Vfs, install: &Path, name: &str) -> PathBuf {
    match vfs.lookup(name) {
        Some(p) if p.entry.is_none() => p.file,
        _ => thedas_vfs::join_ci(install, &format!("{MOVIES_DIR}/{name}")),
    }
}

/// A movie decoding as it plays.
pub(crate) struct Playing {
    name: String,
    /// The movie file (its sound is decoded separately; see [`crate::audio`]).
    path: PathBuf,
    header: BinkHeader,
    /// Decoded frames (RGBA), from the decoding thread; it stops once this is dropped.
    frames: Receiver<Vec<u8>>,
    /// Milliseconds played.
    ms: f64,
    /// Frames taken from the decoder so far.
    taken: u64,
    /// The frame on screen, and whether it has changed since it was last uploaded.
    frame: Option<Vec<u8>>,
    fresh: bool,
    texture: Option<(wgpu::Texture, wgpu::BindGroup)>,
}

/// How long a decoder that has to keep up (a capture) is waited for, for one frame.
const DECODE_WAIT: std::time::Duration = std::time::Duration::from_secs(10);

impl Playing {
    pub(crate) fn start(path: &Path) -> Option<Self> {
        let name = path.file_name()?.to_string_lossy().into_owned();
        let mut head = [0u8; 64];
        let header = std::fs::File::open(path)
            .and_then(|mut f| f.read_exact(&mut head))
            .ok()
            .and_then(|()| BinkHeader::parse(&head));
        let Some(header) = header else {
            log::warn!("movie: {} is missing or not a Bink movie", path.display());
            return None;
        };
        let opened = Movie::open(path).and_then(|m| {
            let d = VideoDecoder::new(&m.header)?;
            Ok((m, d))
        });
        let (mut movie, mut video) = match opened {
            Ok(v) => v,
            Err(e) => {
                log::warn!("movie: {} cannot play: {e}", path.display());
                return None;
            }
        };
        // A few frames ahead; the decoder waits when the player falls behind.
        let (tx, frames) = sync_channel(6);
        let thread_name = name.clone();
        std::thread::Builder::new()
            .name(format!("movie {name}"))
            .spawn(move || {
                let mut warned = false;
                for i in 0..movie.frames() {
                    let mut rgba = Vec::new();
                    let p = match movie.packets(i) {
                        Ok(p) => p,
                        Err(e) => {
                            log::warn!("movie: {thread_name} frame {i}: {e}");
                            break;
                        }
                    };
                    match video.decode(&p.video) {
                        Ok(f) => f.write_rgba(&mut rgba),
                        Err(e) => {
                            // Shown as far as it decoded; the next frame builds on it.
                            if !std::mem::replace(&mut warned, true) {
                                log::warn!("movie: {thread_name} frame {i}: {e}");
                            }
                            video.last().write_rgba(&mut rgba);
                        }
                    }
                    if tx.send(rgba).is_err() {
                        break;
                    }
                }
            })
            .ok()?;
        log::info!(
            "movie: {name} {}x{} {} frames at {:.2} fps",
            header.width,
            header.height,
            header.frames,
            header.fps()
        );
        Some(Self {
            name,
            path: path.to_path_buf(),
            header,
            frames,
            ms: 0.0,
            taken: 0,
            frame: None,
            fresh: false,
            texture: None,
        })
    }

    /// Takes every frame due by now (the latest is the one shown); `wait` waits for a
    /// decoder that has fallen behind. False once the decoder has ended.
    fn take_due(&mut self, wait: bool) -> bool {
        let due = self.header.frame_at(self.ms);
        while self.taken <= due {
            let got = match wait {
                true => self.frames.recv_timeout(DECODE_WAIT).map_err(|e| match e {
                    std::sync::mpsc::RecvTimeoutError::Timeout => TryRecvError::Empty,
                    std::sync::mpsc::RecvTimeoutError::Disconnected => TryRecvError::Disconnected,
                }),
                false => self.frames.try_recv(),
            };
            match got {
                Ok(f) => {
                    self.frame = Some(f);
                    self.fresh = true;
                    self.taken += 1;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return false,
            }
        }
        true
    }

    /// Runs `dt` milliseconds on; false once the movie has ended.
    fn advance(&mut self, dt: f64) -> bool {
        if !self.take_due(false) {
            return false;
        }
        // The clock waits for a decoder that has fallen behind (and for the first frame).
        if self.taken > self.header.frame_at(self.ms) {
            self.ms += dt;
        }
        true
    }

    /// Plays on to `ms` into the movie, a clock kept by someone else (a cutscene's); false
    /// once the movie has ended.
    pub(crate) fn advance_to(&mut self, ms: f64, wait: bool) -> bool {
        self.ms = self.ms.max(ms);
        self.take_due(wait)
    }

    /// Frames shown so far.
    pub(crate) fn taken(&self) -> u64 {
        self.taken
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    /// The movie file and how far it has played (milliseconds).
    pub(crate) fn position(&self) -> (&Path, f64) {
        (&self.path, self.ms)
    }

    /// Draws the frame showing into `pass`, letterboxed in `size` (nothing before the first).
    fn draw(
        &mut self,
        comp: &Composite,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pass: &mut wgpu::RenderPass<'_>,
        size: (u32, u32),
    ) {
        let (w, h) = (self.header.width, self.header.height);
        let (tex, group) = self.texture.get_or_insert_with(|| {
            let tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("movie frame"),
                size: wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                // The frames are sRGB-encoded, as the (non-sRGB) surface is: they pass through.
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = tex.create_view(&Default::default());
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("movie frame"),
                layout: &comp.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&comp.sampler),
                    },
                ],
            });
            (tex, group)
        });
        let Some(f) = self.frame.as_ref() else {
            // Nothing decoded yet: black.
            return;
        };
        if std::mem::take(&mut self.fresh) {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: tex,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                f,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(w * 4),
                    rows_per_image: Some(h),
                },
                wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
            );
        }
        let (x, y, dw, dh) = letterbox(w, h, size);
        pass.set_viewport(x, y, dw, dh, 0.0, 1.0);
        pass.set_pipeline(&comp.pipeline);
        pass.set_bind_group(0, &*group, &[]);
        pass.draw(0..3, 0..1);
    }

    /// The frame showing letterboxed into a black `size` RGBA8 image (black before the
    /// first frame).
    pub(crate) fn rgba(&self, size: (u32, u32)) -> Vec<u8> {
        let mut out = [0, 0, 0, 255].repeat(size.0 as usize * size.1 as usize);
        if let Some(f) = self.frame.as_ref() {
            blit_letterboxed(f, (self.header.width, self.header.height), &mut out, size);
        }
        out
    }
}

/// Scales an RGBA8 `frame` of `fsize` into `out` (`size`), letterboxed (nearest texel).
fn blit_letterboxed(frame: &[u8], fsize: (u32, u32), out: &mut [u8], size: (u32, u32)) {
    let (fw, fh) = (fsize.0 as usize, fsize.1 as usize);
    if fw == 0 || fh == 0 || frame.len() < fw * fh * 4 || out.len() < (size.0 * size.1 * 4) as usize
    {
        return;
    }
    let (x0, y0, dw, dh) = letterbox(fsize.0, fsize.1, size);
    let (x0, y0) = (x0.round() as usize, y0.round() as usize);
    let (dw, dh) = (dw.round() as usize, dh.round() as usize);
    let sw = size.0 as usize;
    for y in 0..dh.min(size.1 as usize - y0.min(size.1 as usize)) {
        let sy = (y * fh / dh.max(1)).min(fh - 1);
        for x in 0..dw.min(sw - x0.min(sw)) {
            let sx = (x * fw / dw.max(1)).min(fw - 1);
            let s = (sy * fw + sx) * 4;
            let o = ((y0 + y) * sw + x0 + x) * 4;
            out[o..o + 3].copy_from_slice(&frame[s..s + 3]);
            out[o + 3] = 255;
        }
    }
}

/// A render pass over `target` cleared to black.
fn black_pass<'e>(
    encoder: &'e mut wgpu::CommandEncoder,
    target: &wgpu::TextureView,
    label: &'static str,
) -> wgpu::RenderPass<'e> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target,
            resolve_target: None,
            depth_slice: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                store: wgpu::StoreOp::Store,
            },
        })],
        ..Default::default()
    })
}

/// The boot movies, played in turn.
pub struct Intro {
    queue: VecDeque<PathBuf>,
    playing: Option<Playing>,
    composite: Option<Composite>,
}

impl Intro {
    /// The boot movies of the install at `install`; None when none of them are there.
    pub fn boot(install: &Path) -> Option<Self> {
        let queue: VecDeque<PathBuf> = BOOT_MOVIES
            .iter()
            .map(|m| install.join(MOVIES_DIR).join(m))
            .filter(|p| p.is_file())
            .collect();
        if queue.is_empty() {
            log::warn!(
                "intro: no boot movies under {}",
                install.join(MOVIES_DIR).display()
            );
            return None;
        }
        Some(Self {
            queue,
            playing: None,
            composite: None,
        })
    }

    /// Runs `dt` milliseconds on, starting the next movie as one ends; false once they all
    /// have.
    pub fn advance(&mut self, dt: f64) -> bool {
        loop {
            if let Some(p) = self.playing.as_mut() {
                if p.advance(dt) {
                    return true;
                }
                log::info!("intro: {} ended", p.name);
                self.playing = None;
            }
            let Some(next) = self.queue.pop_front() else {
                return false;
            };
            self.playing = Playing::start(&next);
        }
    }

    /// The boot movie playing and how far it has played (milliseconds).
    pub fn now_playing(&self) -> Option<(&Path, f64)> {
        self.playing.as_ref().map(Playing::position)
    }

    /// Skips the movie playing (the next one starts at the next frame).
    pub fn skip(&mut self) {
        if let Some(p) = self.playing.take() {
            log::info!("intro: {} skipped", p.name);
        }
    }

    /// Draws the movie's frame over `target`, black around it.
    pub fn draw(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        format: wgpu::TextureFormat,
        size: (u32, u32),
    ) {
        let comp = self
            .composite
            .get_or_insert_with(|| Composite::new(device, format));
        let mut pass = black_pass(encoder, target, "intro");
        if let Some(p) = self.playing.as_mut() {
            p.draw(comp, device, queue, &mut pass, size);
        }
    }
}

/// The movies a cutscene plays full screen (its `APBM` tracks; see
/// [`thedas_sim::cutscene::movies`]), kept in step with the cutscene's clock as the
/// executable's `ActionPlayMovie` does (0x7ce740 hands the movie player the cutscene's time
/// every tick).
///
/// While one of its movies is due the movie is all that is drawn of the world (the
/// cutscene's own interface, its subtitles, goes over it). Provisional, not traced: what
/// shows after a movie ends and before the cutscene does (black here), and how the movie
/// player catches up with a clock that ran ahead (frames are dropped). The movies play
/// silent: there is no audio yet (nor the cutscene's own sound track).
#[derive(Default)]
pub struct CutsceneMovies {
    /// The cutscene followed (its resource name), and its movies.
    cutscene: Option<String>,
    tracks: Vec<thedas_sim::cutscene::CutsceneMovie>,
    /// The track playing, and its movie (None once it has ended).
    playing: Option<(usize, Option<Playing>)>,
    /// Whether the last [`CutsceneMovies::follow`] found a movie due.
    shown: bool,
    composite: Option<Composite>,
}

/// What [`CutsceneMovies::follow`] found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MovieState {
    /// No movie is due: the world is drawn.
    None,
    /// A movie is due: it is drawn instead of the world.
    Showing,
    /// A movie is due that cannot play (the movie missing or undecodable):
    /// the cutscene should end now rather than show the world standing still.
    CannotPlay,
}

impl CutsceneMovies {
    /// Follows the cutscene playing (`(resource, ms played)`), reading its movies from
    /// `vfs` when it starts; `wait` waits for the decoder (a capture, which must show the
    /// frame due).
    pub fn follow(
        &mut self,
        cutscene: Option<(&str, u64)>,
        vfs: &thedas_vfs::Vfs,
        install: &Path,
        wait: bool,
    ) -> MovieState {
        let state = self.follow_inner(cutscene, vfs, install, wait);
        self.shown = state == MovieState::Showing;
        state
    }

    fn follow_inner(
        &mut self,
        cutscene: Option<(&str, u64)>,
        vfs: &thedas_vfs::Vfs,
        install: &Path,
        wait: bool,
    ) -> MovieState {
        let Some((name, elapsed_ms)) = cutscene else {
            if let Some(c) = self.cutscene.take() {
                log::debug!("cutscene {c}: its movies stop");
            }
            self.tracks.clear();
            self.playing = None;
            return MovieState::None;
        };
        if self.cutscene.as_deref() != Some(name) {
            self.tracks = vfs
                .read(name)
                .map(|(data, _)| thedas_sim::cutscene::movies(&data))
                .unwrap_or_default();
            self.playing = None;
            self.cutscene = Some(name.to_string());
            if !self.tracks.is_empty() {
                log::info!(
                    "cutscene {name}: movies {:?}",
                    self.tracks
                        .iter()
                        .map(|t| format!("{} at {:.2}s", t.name, t.start))
                        .collect::<Vec<_>>()
                );
            }
        }
        let t = elapsed_ms as f32 / 1000.0;
        let Some(i) = self.tracks.iter().rposition(|m| m.start <= t && t < m.end) else {
            // Between movies (or after the last): black once the first has started.
            return match self.tracks.first() {
                Some(m) if m.start <= t => {
                    self.playing = None;
                    MovieState::Showing
                }
                _ => MovieState::None,
            };
        };
        if self.playing.as_ref().map(|p| p.0) != Some(i) {
            let track = &self.tracks[i];
            let path = movie_path(vfs, install, &format!("{}.bik", track.name));
            let Some(p) = Playing::start(&path) else {
                log::warn!(
                    "cutscene {name}: movie {} cannot play (missing or undecodable), so the \
                     cutscene ends",
                    path.display()
                );
                self.playing = None;
                return MovieState::CannotPlay;
            };
            log::info!("cutscene {name}: movie {} plays", p.name());
            self.playing = Some((i, Some(p)));
        }
        let start = self.tracks[i].start as f64;
        if let Some((_, slot)) = self.playing.as_mut()
            && let Some(p) = slot.as_mut()
            && !p.advance_to(elapsed_ms as f64 - start * 1000.0, wait)
        {
            if p.taken() == 0 {
                log::warn!(
                    "cutscene {name}: movie {} gave no frames, so the cutscene ends",
                    p.name()
                );
                *slot = None;
                return MovieState::CannotPlay;
            }
            log::info!("cutscene {name}: movie {} ended", p.name());
            *slot = None;
        }
        // The last movie has ended before its track does (one played "to the end" of a
        // cutscene that goes on, `bhn100cs_intro`): the world shows again, as the
        // cutscene's cameras cut to it. Provisional: ActionPlayMovie's update (0x7ce740)
        // is not traced for a movie shorter than its track.
        if i + 1 == self.tracks.len() && matches!(self.playing, Some((j, None)) if j == i) {
            return MovieState::None;
        }
        MovieState::Showing
    }

    /// Whether a movie is due (drawn instead of the world), as the last
    /// [`CutsceneMovies::follow`] found.
    pub fn showing(&self) -> bool {
        self.shown
    }

    /// Draws the movie due over `target`, black around it (all black between movies).
    pub fn draw(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        format: wgpu::TextureFormat,
        size: (u32, u32),
    ) {
        let comp = self
            .composite
            .get_or_insert_with(|| Composite::new(device, format));
        let mut pass = black_pass(encoder, target, "cutscene movie");
        if let Some((_, Some(p))) = self.playing.as_mut() {
            p.draw(comp, device, queue, &mut pass, size);
        }
    }

    /// The movie due as RGBA8 pixels of `size`, letterboxed (black between movies).
    pub fn rgba(&self, size: (u32, u32)) -> Vec<u8> {
        match &self.playing {
            Some((_, Some(p))) => p.rgba(size),
            _ => [0, 0, 0, 255].repeat(size.0 as usize * size.1 as usize),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(frames: u32, w: u32, h: u32, num: u32, den: u32) -> Vec<u8> {
        let mut b = b"BIKi".to_vec();
        for v in [1000, frames, 5000, frames, w, h, num, den, 0] {
            b.extend_from_slice(&u32::to_le_bytes(v));
        }
        b
    }

    #[test]
    fn the_bink_header_gives_the_size_and_rate() {
        let h = BinkHeader::parse(&header(260, 1280, 720, 5_000_000, 166_833)).unwrap();
        assert_eq!((h.width, h.height, h.frames), (1280, 720, 260));
        assert!((h.fps() - 29.97).abs() < 0.01);
        assert_eq!(h.frame_at(0.0), 0);
        assert_eq!(h.frame_at(1000.0), 29);
        assert_eq!(h.frame_at(-5.0), 0);
        assert!(BinkHeader::parse(b"RIFF....").is_none());
        assert!(BinkHeader::parse(&header(1, 0, 720, 30, 1)).is_none());
        // A zero denominator reads as one rather than dividing by zero.
        assert_eq!(
            BinkHeader::parse(&header(1, 64, 64, 30, 0)).unwrap().fps(),
            30.0
        );
    }

    #[test]
    fn a_movie_fits_the_window_with_its_shape_kept() {
        assert_eq!(letterbox(1280, 720, (1280, 720)), (0.0, 0.0, 1280.0, 720.0));
        // Taller window: bars above and below.
        assert_eq!(
            letterbox(1280, 720, (1280, 1024)),
            (0.0, 152.0, 1280.0, 720.0)
        );
        // Wider window: bars at the sides.
        assert_eq!(
            letterbox(1280, 720, (2560, 720)),
            (640.0, 0.0, 1280.0, 720.0)
        );
    }

    /// Against a real install (`THEDAS_INSTALL`): both movies decode whole and play to the
    /// end at their frame rate.
    #[test]
    #[ignore]
    fn the_installs_boot_movies_play_through() {
        let install = PathBuf::from(std::env::var_os("THEDAS_INSTALL").expect("THEDAS_INSTALL"));
        let mut intro = Intro::boot(&install).expect("the movies");
        let mut ms = 0.0;
        let mut seen = Vec::new();
        while intro.advance(1000.0 / 60.0) {
            ms += 1000.0 / 60.0;
            let p = intro.playing.as_ref().expect("a movie playing");
            match seen.last() {
                Some((name, _, _)) if *name == p.name => {}
                _ => seen.push((p.name.clone(), p.header.frames, 0)),
            }
            seen.last_mut().unwrap().2 = p.taken;
            assert!(ms < 120_000.0, "still playing after two minutes");
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let names: Vec<_> = seen.iter().map(|s| s.0.as_str()).collect();
        assert_eq!(names, BOOT_MOVIES);
        for (name, frames, taken) in &seen {
            assert_eq!(u64::from(*frames), *taken, "{name}");
        }
    }

    #[test]
    fn a_frame_is_scaled_into_the_letterbox() {
        // A 2x1 frame (red, green) into a 4x4 image: two rows of bars, red then green.
        let frame = [255, 0, 0, 255, 0, 255, 0, 255];
        let mut out = [0, 0, 0, 255].repeat(16);
        blit_letterboxed(&frame, (2, 1), &mut out, (4, 4));
        let px = |x: usize, y: usize| &out[(y * 4 + x) * 4..(y * 4 + x) * 4 + 3];
        assert_eq!(px(0, 0), [0, 0, 0]);
        assert_eq!(px(0, 1), [255, 0, 0]);
        assert_eq!(px(1, 2), [255, 0, 0]);
        assert_eq!(px(3, 2), [0, 255, 0]);
        assert_eq!(px(3, 3), [0, 0, 0]);
        // A short frame buffer draws nothing.
        let mut out2 = [0, 0, 0, 255].repeat(16);
        blit_letterboxed(&frame[..4], (2, 1), &mut out2, (4, 4));
        assert!(out2.chunks(4).all(|p| p == [0, 0, 0, 255]));
    }

    #[test]
    fn the_boot_list_is_the_exes() {
        assert_eq!(
            BOOT_MOVIES,
            ["dragon_age_ea_logo.bik", "dragon_age_main.bik"]
        );
    }
}
