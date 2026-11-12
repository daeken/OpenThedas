//! The movies the game plays as it boots, before the start menu: the exe's boot list
//! (0x5ec247, filled by 0x5dd870) is `dragon_age_ea_logo.bik` then `dragon_age_main.bik`,
//! from `packages/core/data/movies`.
//!
//! Provisional: the Bink frames are decoded by an `ffmpeg` on the PATH (Homebrew's, on the
//! Mac) until there is a Bink decoder of our own; without one the movies are skipped. They
//! play silent (there is no audio yet). A key, a click or the pad's A, B or Start skips the
//! movie playing (the exe's skip input is not traced). The options' `DisableIntroMovies`
//! (and the exe's `FirstRun` gate) are not read; `--no-intro` skips them.

use std::collections::VecDeque;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, TryRecvError, sync_channel};

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

    fn frame_bytes(&self) -> usize {
        self.width as usize * self.height as usize * 4
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

/// The `ffmpeg` that decodes the movies, if there is one.
pub fn find_ffmpeg() -> Option<PathBuf> {
    let candidates = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .chain(["/opt/homebrew/bin", "/usr/local/bin"].map(PathBuf::from));
    candidates.map(|d| d.join("ffmpeg")).find(|p| p.is_file())
}

/// A movie decoding as it plays.
struct Playing {
    name: String,
    header: BinkHeader,
    child: Child,
    frames: Receiver<Vec<u8>>,
    /// Milliseconds played.
    ms: f64,
    /// Frames taken from the decoder so far.
    taken: u64,
    /// The frame on screen, until it is uploaded.
    pending: Option<Vec<u8>>,
    texture: Option<(wgpu::Texture, wgpu::BindGroup)>,
}

impl Playing {
    fn start(ffmpeg: &Path, path: &Path) -> Option<Self> {
        let name = path.file_name()?.to_string_lossy().into_owned();
        let mut head = [0u8; 64];
        let header = std::fs::File::open(path)
            .and_then(|mut f| f.read_exact(&mut head))
            .ok()
            .and_then(|()| BinkHeader::parse(&head));
        let Some(header) = header else {
            log::warn!("intro: {} is missing or not a Bink movie", path.display());
            return None;
        };
        let mut child = Command::new(ffmpeg)
            .args(["-nostdin", "-v", "error", "-i"])
            .arg(path)
            .args(["-an", "-f", "rawvideo", "-pix_fmt", "rgba", "-"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| log::warn!("intro: {} could not start: {e}", ffmpeg.display()))
            .ok()?;
        let mut out = child.stdout.take()?;
        let bytes = header.frame_bytes();
        // A few frames ahead; the decoder waits when the player falls behind.
        let (tx, frames) = sync_channel(6);
        std::thread::Builder::new()
            .name(format!("intro {name}"))
            .spawn(move || {
                loop {
                    let mut f = vec![0u8; bytes];
                    if out.read_exact(&mut f).is_err() || tx.send(f).is_err() {
                        break;
                    }
                }
            })
            .ok()?;
        log::info!(
            "intro: {name} {}x{} {} frames at {:.2} fps",
            header.width,
            header.height,
            header.frames,
            header.fps()
        );
        Some(Self {
            name,
            header,
            child,
            frames,
            ms: 0.0,
            taken: 0,
            pending: None,
            texture: None,
        })
    }

    /// Runs `dt` milliseconds on; false once the movie has ended.
    fn advance(&mut self, dt: f64) -> bool {
        let due = self.header.frame_at(self.ms);
        // Every frame due is taken; the latest is the one shown.
        while self.taken <= due {
            match self.frames.try_recv() {
                Ok(f) => {
                    self.pending = Some(f);
                    self.taken += 1;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return false,
            }
        }
        // The clock waits for a decoder that has fallen behind (and for the first frame).
        if self.taken > due {
            self.ms += dt;
        }
        true
    }
}

impl Drop for Playing {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The boot movies, played in turn.
pub struct Intro {
    ffmpeg: PathBuf,
    queue: VecDeque<PathBuf>,
    playing: Option<Playing>,
    composite: Option<Composite>,
}

impl Intro {
    /// The boot movies of the install at `install`; None when they cannot play (no
    /// `ffmpeg`, or none of them there).
    pub fn boot(install: &Path) -> Option<Self> {
        let Some(ffmpeg) = find_ffmpeg() else {
            log::warn!("intro: no ffmpeg on the PATH, so the intro movies are skipped");
            return None;
        };
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
            ffmpeg,
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
            self.playing = Playing::start(&self.ffmpeg, &next);
        }
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
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("intro"),
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
        });
        let Some(p) = self.playing.as_mut() else {
            return;
        };
        let (w, h) = (p.header.width, p.header.height);
        let (tex, group) = p.texture.get_or_insert_with(|| {
            let tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("intro frame"),
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
                label: Some("intro frame"),
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
        if let Some(f) = p.pending.take() {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: tex,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &f,
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
        // Nothing decoded yet: black.
        if p.taken == 0 {
            return;
        }
        let (x, y, dw, dh) = letterbox(w, h, size);
        pass.set_viewport(x, y, dw, dh, 0.0, 1.0);
        pass.set_pipeline(&comp.pipeline);
        pass.set_bind_group(0, &*group, &[]);
        pass.draw(0..3, 0..1);
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

    /// Against a real install (`THEDAS_INSTALL`) and `ffmpeg`: both movies decode whole and
    /// play to the end at their frame rate.
    #[test]
    #[ignore]
    fn the_installs_boot_movies_play_through() {
        let install = PathBuf::from(std::env::var_os("THEDAS_INSTALL").expect("THEDAS_INSTALL"));
        let mut intro = Intro::boot(&install).expect("ffmpeg and the movies");
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
    fn the_boot_list_is_the_exes() {
        assert_eq!(
            BOOT_MOVIES,
            ["dragon_age_ea_logo.bik", "dragon_age_main.bik"]
        );
    }
}
