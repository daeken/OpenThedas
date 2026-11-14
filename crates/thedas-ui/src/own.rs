//! The movies on our own Scaleform player (`thedas-gfx`) instead of Ruffle.
//!
//! Chosen at run time with `THEDAS_UI_PLAYER`: `ruffle` (the default), `own` for every movie,
//! or `own:mainmenu,quickbar` for the named movies only. The rest of [`crate::UiMovie`] (token
//! observers, calls by path through the injected glue) is shared by both players.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use thedas_gfx::render::{Images, Renderer};
use thedas_gfx::swf::{Character, Font};
use thedas_gfx::{Host, HostValue, Player};

use crate::atlas::ImageSource;
use crate::navigator::ResourceLookup;
use crate::{Gpu, Rgba, UiKey, UiValue};

/// Whether the movie `name` (`mainmenu.gfx`) plays on our player.
pub fn wanted(name: &str) -> bool {
    let Ok(v) = std::env::var("THEDAS_UI_PLAYER") else {
        return false;
    };
    let v = v.trim().to_ascii_lowercase();
    if v == "own" {
        return true;
    }
    let Some(list) = v.strip_prefix("own:") else {
        return false;
    };
    let stem = name
        .to_ascii_lowercase()
        .trim_end_matches(".gfx")
        .to_string();
    list.split(',')
        .any(|m| m.trim().trim_end_matches(".gfx") == stem)
}

pub fn to_host(v: &UiValue) -> HostValue {
    match v {
        UiValue::Undefined => HostValue::Undefined,
        UiValue::Null => HostValue::Null,
        UiValue::Bool(b) => HostValue::Bool(*b),
        UiValue::Number(n) => HostValue::Number(*n),
        UiValue::String(s) => HostValue::String(s.clone()),
        UiValue::List(l) => HostValue::List(l.iter().map(to_host).collect()),
        UiValue::Object(o) => {
            HostValue::Object(o.iter().map(|(k, v)| (k.clone(), to_host(v))).collect())
        }
    }
}

pub fn from_host(v: &HostValue) -> UiValue {
    match v {
        HostValue::Undefined => UiValue::Undefined,
        HostValue::Null => UiValue::Null,
        HostValue::Bool(b) => UiValue::Bool(*b),
        HostValue::Number(n) => UiValue::Number(*n),
        HostValue::String(s) => UiValue::String(s.clone()),
        HostValue::List(l) => UiValue::List(l.iter().map(from_host).collect()),
        HostValue::Object(o) => {
            UiValue::Object(o.iter().map(|(k, v)| (k.clone(), from_host(v))).collect())
        }
    }
}

/// Image sizes by URL (`None`: not found).
type SizeCache = HashMap<String, Option<(u32, u32)>>;

/// Images by name for the renderer and the player: atlas images (`*.dds`), then pictures the
/// engine made (PNG) from the resources.
#[derive(Clone)]
pub struct ImageFetch {
    pub lookup: ResourceLookup,
    pub atlas: Arc<Mutex<ImageSource>>,
    sizes: Arc<Mutex<SizeCache>>,
}

impl ImageFetch {
    pub fn new(lookup: ResourceLookup, atlas: Arc<Mutex<ImageSource>>) -> Self {
        Self {
            lookup,
            atlas,
            sizes: Default::default(),
        }
    }

    pub fn fetch(&self, url: &str) -> Option<Rgba> {
        let name = crate::navigator::candidates(url).into_iter().next()?;
        let img = if name.ends_with(".dds") || name.ends_with(".tga") {
            self.atlas.lock().ok()?.image(&name).or_else(|| {
                (self.lookup)(&name)
                    .filter(|b| b.starts_with(b"\x89PNG"))
                    .and_then(|b| decode_png(&b))
            })
        } else {
            (self.lookup)(&name).and_then(|b| decode_png(&b))
        }?;
        Some(match crate::navigator::requested_size(url) {
            Some((w, h)) if (w, h) != (img.width, img.height) => crate::gfx::resize(&img, w, h),
            _ => img,
        })
    }

    pub fn size(&self, url: &str) -> Option<(u32, u32)> {
        if let Some(s) = self.sizes.lock().ok().and_then(|m| m.get(url).copied()) {
            return s;
        }
        let s = crate::navigator::requested_size(url)
            .or_else(|| self.fetch(url).map(|i| (i.width, i.height)));
        if let Ok(mut m) = self.sizes.lock() {
            m.insert(url.to_string(), s);
        }
        s
    }
}

fn decode_png(bytes: &[u8]) -> Option<Rgba> {
    let mut dec = png::Decoder::new(std::io::Cursor::new(bytes));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut r = dec.read_info().ok()?;
    let mut buf = vec![0; r.output_buffer_size()?];
    let info = r.next_frame(&mut buf).ok()?;
    let px = &buf[..info.buffer_size()];
    let pixels = match info.color_type {
        png::ColorType::Rgba => px.to_vec(),
        png::ColorType::Rgb => px
            .chunks_exact(3)
            .flat_map(|c| [c[0], c[1], c[2], 255])
            .collect(),
        png::ColorType::GrayscaleAlpha => px
            .chunks_exact(2)
            .flat_map(|c| [c[0], c[0], c[0], c[1]])
            .collect(),
        png::ColorType::Grayscale => px.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        _ => return None,
    };
    Some(Rgba {
        width: info.width,
        height: info.height,
        pixels,
    })
}

impl Images for ImageFetch {
    fn image(&mut self, name: &str) -> Option<(u32, u32, Vec<u8>)> {
        let img = self.fetch(name)?;
        Some((img.width, img.height, img.pixels))
    }
}

/// The game's font library (`fonts_en-us.gfx`), with the movies' imported names
/// (`$BodyFont`, `$TitleFont`, through `gfxfontlib`) mapped to its fonts.
pub struct FontSet {
    fonts: Vec<Arc<Font>>,
    aliases: HashMap<String, String>,
}

impl FontSet {
    pub fn load(lookup: &ResourceLookup) -> Self {
        let mut fonts = Vec::new();
        let mut aliases = HashMap::new();
        let parse = |n: &str| lookup(n).and_then(|d| thedas_gfx::swf::Movie::parse(&d).ok());
        if let Some(m) = parse(crate::FONT_LIBRARY) {
            fonts.extend(m.characters.values().filter_map(|c| match c {
                Character::Font(f) if !f.glyphs.is_empty() => Some(f.clone()),
                _ => None,
            }));
        }
        if let Some(m) = parse("gfxfontlib.gfx") {
            for (name, id) in &m.exports {
                if let Some(Character::Font(f)) = m.characters.get(id) {
                    aliases.insert(name.to_ascii_lowercase(), f.name.clone());
                    if !fonts.iter().any(|x| x.name.eq_ignore_ascii_case(&f.name)) {
                        fonts.push(f.clone());
                    }
                }
            }
        }
        Self { fonts, aliases }
    }
}

impl thedas_gfx::text::Fonts for FontSet {
    fn font(&self, name: &str, bold: bool, italic: bool) -> Option<Arc<Font>> {
        let name = self
            .aliases
            .get(&name.to_ascii_lowercase())
            .map(String::as_str)
            .unwrap_or(name);
        let named = |f: &&Arc<Font>| f.name.eq_ignore_ascii_case(name);
        self.fonts
            .iter()
            .filter(named)
            .find(|f| f.bold == bold && f.italic == italic)
            .or_else(|| self.fonts.iter().find(named))
            .or_else(|| {
                self.fonts
                    .iter()
                    .find(|f| f.name.eq_ignore_ascii_case("Dragon Text"))
            })
            .cloned()
    }
}

/// The engine side of a movie's calls, as for Ruffle (see `crate::Bridge`).
pub(crate) struct OwnHost {
    pub(crate) bridge: crate::Bridge,
    pub images: ImageFetch,
}

impl Host for OwnHost {
    fn fscommand(&mut self, command: &str, args: &str) {
        self.bridge.fs_command(command, args);
    }

    fn external_call(&mut self, name: &str, args: &[HostValue]) -> HostValue {
        let args: Vec<UiValue> = args.iter().map(from_host).collect();
        to_host(&self.bridge.external_call(name, args))
    }

    fn image_size(&mut self, url: &str) -> Option<(u32, u32)> {
        self.images.size(url)
    }
}

/// A movie on our player, with its renderer.
pub struct OwnMovie {
    pub player: Player,
    renderer: Renderer,
    images: ImageFetch,
    size: (u32, u32),
    gpu: Gpu,
}

impl OwnMovie {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        gpu: &Gpu,
        name: &str,
        data: &[u8],
        size: (u32, u32),
        lookup: ResourceLookup,
        atlas: Arc<Mutex<ImageSource>>,
        bridge: crate::Bridge,
        glue: Vec<u8>,
    ) -> Result<Self, crate::UiError> {
        let images = ImageFetch::new(lookup.clone(), atlas);
        let fonts = Arc::new(FontSet::load(&lookup));
        let host = OwnHost {
            bridge,
            images: images.clone(),
        };
        let mut player = Player::new(data, name, Box::new(host), fonts, Some(glue))
            .map_err(|e| crate::UiError::Swf(e.to_string()))?;
        // The Ruffle-era glue takes the button from a global the host sets; until it goes,
        // press handlers get no button from the player.
        player.pass_buttons = false;
        let renderer = Renderer::new(&gpu.device, &gpu.queue, wgpu::TextureFormat::Rgba8Unorm);
        let mut m = Self {
            player,
            renderer,
            images,
            size,
            gpu: gpu.clone(),
        };
        m.resize(size);
        Ok(m)
    }

    fn scale(&self) -> (f64, f64, f64) {
        let (aw, ah) = self.player.vm.movie.stage_size();
        let (w, h) = (self.size.0 as f64, self.size.1 as f64);
        let s = (w / aw as f64).min(h / ah as f64);
        (s, (w - aw as f64 * s) / 2.0, (h - ah as f64 * s) / 2.0)
    }

    /// Window pixels to stage pixels.
    fn to_stage(&self, x: f64, y: f64) -> (f64, f64) {
        let (s, ox, oy) = self.scale();
        ((x - ox) / s, (y - oy) / s)
    }

    pub fn resize(&mut self, size: (u32, u32)) {
        self.size = size;
        let (s, _, _) = self.scale();
        self.player
            .set_stage_size(size.0 as f64 / s, size.1 as f64 / s);
    }

    pub fn tick(&mut self, dt_ms: f64) {
        self.player.tick(dt_ms);
    }

    pub fn run_frame(&mut self) {
        self.player.run_frame();
        self.player.run_queue();
    }

    pub fn render(&mut self) {
        let mut images = self.images.clone();
        self.renderer
            .render_if_dirty(&mut self.player, &mut images, self.size);
    }

    pub fn texture(&self) -> Option<wgpu::Texture> {
        self.renderer.output().cloned()
    }

    pub fn device(&self) -> (wgpu::Device, wgpu::Queue) {
        (self.gpu.device.clone(), self.gpu.queue.clone())
    }

    /// The last frame (RGBA8, premultiplied).
    pub fn capture(&mut self) -> Option<Rgba> {
        let (width, height, mut pixels) = self.renderer.capture()?;
        crate::premultiply(&mut pixels);
        Some(Rgba {
            width,
            height,
            pixels,
        })
    }

    pub fn mouse_move(&mut self, x: f64, y: f64) {
        let (x, y) = self.to_stage(x, y);
        self.player.mouse_move(x, y);
    }

    pub fn mouse_wheel(&mut self, x: f64, y: f64, lines: f64) {
        let (x, y) = self.to_stage(x, y);
        self.player.mouse_move(x, y);
        self.player.mouse_wheel(x, y, lines);
    }

    pub fn mouse_button(&mut self, x: f64, y: f64, down: bool) {
        let (x, y) = self.to_stage(x, y);
        self.player
            .mouse_button(x, y, thedas_gfx::player::MOUSE_LEFT, down, false);
    }

    pub fn key(&mut self, key: UiKey) {
        let (code, ch) = match key {
            UiKey::Char(c) => (c.to_ascii_uppercase() as u32, Some(c)),
            UiKey::Backspace => (8, None),
            UiKey::Delete => (46, None),
            UiKey::Enter => (13, None),
            UiKey::Escape => (27, None),
            UiKey::Tab => (9, None),
            UiKey::Left => (37, None),
            UiKey::Right => (39, None),
            UiKey::Home => (36, None),
            UiKey::End => (35, None),
            UiKey::SelectAll => (65, None),
        };
        self.player.key_down(code, ch);
        self.player.key_up(code);
    }

    pub fn invoke(&mut self, name: &str, args: &[UiValue]) -> UiValue {
        let args: Vec<HostValue> = args.iter().map(to_host).collect();
        from_host(&self.player.call_callback(name, &args))
    }

    pub fn callbacks(&self) -> Vec<String> {
        self.player.callbacks()
    }

    pub fn drain_log(&mut self) -> Vec<String> {
        self.player.drain_log()
    }

    /// Draw calls in the last render (for profiling).
    pub fn draws(&self) -> usize {
        self.renderer.draws
    }
}
