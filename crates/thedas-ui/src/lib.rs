//! The game's Scaleform GFx interface, played by [`thedas_gfx`] on the renderer's wgpu device.
//!
//! Each [`UiMovie`] owns one player rendering into an offscreen RGBA texture that the
//! application composites over the scene. Movies load directly from the game's GFx files.
//!
//! The interface reads simulation state and never writes it: calls the movie makes through
//! `fscommand` and `ExternalInterface.call` are queued as [`UiCall`]s for the application to
//! act on, and an optional responder answers `ExternalInterface.call` return values. The
//! messages a movie sends reach the simulation only as player input ([`input`]).

pub mod appearance;
pub mod atlas;
pub mod avm1;
pub mod binding;
pub mod chains;
pub mod input;
pub mod keys;
pub mod minimap;
pub mod models;
pub mod options;
pub mod protocol;
pub mod stage;

pub use thedas_formats::gfx;
pub mod navigator;
pub mod own;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

pub use atlas::ImageSource;
pub use gfx::{Inventory, RewriteReport, Rgba};
pub use navigator::ResourceLookup;

#[derive(Debug, thiserror::Error)]
pub enum UiError {
    #[error("movie {0} not found")]
    NotFound(String),
    #[error(transparent)]
    Gfx(#[from] gfx::GfxError),
    #[error("SWF: {0}")]
    Swf(String),
    #[error("renderer: {0}")]
    Render(String),
}

/// A key for the movies: a character typed, or a key that edits text or moves its cursor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiKey {
    Char(char),
    Backspace,
    Delete,
    Enter,
    Escape,
    Tab,
    Left,
    Right,
    Home,
    End,
    /// Ctrl+A: select the field's text (typing then replaces it).
    SelectAll,
}

/// A value passed between a movie and the application.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum UiValue {
    #[default]
    Undefined,
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    List(Vec<UiValue>),
    Object(BTreeMap<String, UiValue>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallKind {
    FsCommand,
    ExternalInterface,
}

/// A call a movie made to the engine.
#[derive(Clone, Debug, PartialEq)]
pub struct UiCall {
    pub movie: String,
    pub kind: CallKind,
    pub name: String,
    pub args: Vec<UiValue>,
}

/// Answers `ExternalInterface.call` with a return value.
pub type Responder = Arc<dyn Fn(&UiCall) -> UiValue + Send + Sync>;

#[derive(Default)]
struct Shared {
    calls: Mutex<Vec<UiCall>>,
    /// The answer to the last fscommand, fetched by the wrapped `ExternalCommands` functions.
    retval: Mutex<UiValue>,
    /// Observers registered through `ExternalInterface.call("AddTokenObserver", ...)`
    /// (callback id, token), for [`UiMovie::sync_tokens`] to take up.
    called_observers: Mutex<Vec<(String, String)>>,
    /// The last id handed out for those.
    observer_ids: std::sync::atomic::AtomicU64,
}

/// The first id handed to `AddTokenObserver` observers: past the ids the movies number their
/// own observers with (`_OBSERVER_ID` constants and counters). Provisional.
const CALLED_OBSERVER_IDS: u64 = 1_000_000;

pub(crate) struct Bridge {
    movie: String,
    shared: Arc<Shared>,
    responder: Option<Responder>,
}

impl Bridge {
    fn push(&self, call: UiCall) -> UiValue {
        let r = match &self.responder {
            Some(f) => f(&call),
            None => UiValue::Undefined,
        };
        if let Ok(mut c) = self.shared.calls.lock() {
            c.push(call);
        }
        r
    }
}

impl Bridge {
    /// An `fscommand` from the movie: queued for the engine, its answer kept for the
    /// wrapped getters.
    pub(crate) fn fs_command(&self, command: &str, args: &str) {
        let r = self.push(UiCall {
            movie: self.movie.clone(),
            kind: CallKind::FsCommand,
            name: command.to_string(),
            args: vec![UiValue::String(args.to_string())],
        });
        if let Ok(mut v) = self.shared.retval.lock() {
            *v = r;
        }
    }

    /// An `ExternalInterface.call` from the movie.
    pub(crate) fn external_call(&self, name: &str, args: Vec<UiValue>) -> UiValue {
        // `TokenManager.AddTokenObserverFunc(label, token, callback)` asks the game for the
        // observer's id (`AddTokenObserver(label, "", token)`) and files the callback under
        // it: the observer is the game's to feed from then on, as for
        // `AddTokenObserverWithID` (optionsmenu.gfx's checkboxes and sliders).
        if name == "AddTokenObserver" {
            let UiValue::String(token) = args.get(2).cloned().unwrap_or(UiValue::Undefined) else {
                return UiValue::Undefined;
            };
            let id = CALLED_OBSERVER_IDS
                + self
                    .shared
                    .observer_ids
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if let Ok(mut o) = self.shared.called_observers.lock() {
                o.push((id.to_string(), token));
            }
            return UiValue::Number(id as f64);
        }
        if name == avm1::RETVAL_CALL {
            return self
                .shared
                .retval
                .lock()
                .map(|mut v| std::mem::replace(&mut *v, UiValue::Undefined))
                .unwrap_or(UiValue::Undefined);
        }
        self.push(UiCall {
            movie: self.movie.clone(),
            kind: CallKind::ExternalInterface,
            name: name.to_string(),
            args,
        })
    }
}

/// The wgpu handles the movie renders with; clones of the renderer's.
#[derive(Clone)]
pub struct Gpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

/// Images the engine renders into the movies.
pub const ENGINE_IMAGES: &[&str] = &["paperdoll"];

/// The font library the game installs for Scaleform (see [`gfx::FontLibrary`]).
pub const FONT_LIBRARY: &str = "fonts_en-us.gfx";

/// How many window pixels one stage pixel takes: the movie's authored size fitted inside the
/// window (`size`), whole, centred.
///
/// DAOrigins.exe leaves its movies in Scaleform's show-all scale mode (the movie root's
/// constructor sets it, 0x95af2a, and the engine never calls `SetViewScaleMode`) and centres
/// them (`SetViewAlignment(Align_Center)` on each new movie view, 0x514aaa). Scaleform's
/// `Stage.width`/`height` then report the whole visible area in stage units, wider than the
/// authored stage on a wide window; the movies anchor to that rectangle
/// (`SceneManager.SetSafeZone`: `sm_rScreenRect` is `Stage.width` x `Stage.height` at
/// `((OriginalStageWidth - Stage.width) / 2, ...)`). Flash reports the authored
/// size under show-all, so the stage runs in no-scale mode with this scale factor instead:
/// the same picture, with `Stage.width`/`height` the visible area. Provisional: the
/// visible-area reading of `Stage.width` is inferred from the movies' code, not checked
/// against Scaleform's own.
pub fn stage_scale(size: (u32, u32), authored: (f64, f64)) -> f64 {
    let (w, h) = (f64::from(size.0.max(1)), f64::from(size.1.max(1)));
    if authored.0 <= 0.0 || authored.1 <= 0.0 {
        return 1.0;
    }
    (w / authored.0).min(h / authored.1)
}

/// RGBA8 `pixels` with each colour multiplied by its alpha.
pub fn premultiply(pixels: &mut [u8]) {
    for p in pixels.chunks_exact_mut(4) {
        let a = u32::from(p[3]);
        for c in &mut p[..3] {
            *c = ((u32::from(*c) * a + 127) / 255) as u8;
        }
    }
}

/// A clip a pad's focus can land on: the movie, its path, and its bounds in window pixels
/// (`x0, y0, x1, y1`).
#[derive(Clone, Debug, PartialEq)]
pub struct FocusTarget {
    pub movie: String,
    pub path: String,
    pub rect: [f64; 4],
}

impl FocusTarget {
    pub fn center(&self) -> (f64, f64) {
        (
            (self.rect[0] + self.rect[2]) / 2.0,
            (self.rect[1] + self.rect[3]) / 2.0,
        )
    }
}

/// Reads `thedas_focus_list`'s `path|xMin|yMin|xMax|yMax;...`, taking the bounds to window
/// pixels (scaled by `scale`, then moved by `offset`); entries that do not parse, or have
/// no area, are left out.
pub fn parse_focus_list(list: &str, scale: f64, offset: (f64, f64)) -> Vec<FocusTarget> {
    list.split(';')
        .filter_map(|e| {
            let mut f = e.split('|');
            let path = f.next()?.to_string();
            let mut r = [0.0; 4];
            for (i, v) in r.iter_mut().enumerate() {
                let o = if i % 2 == 0 { offset.0 } else { offset.1 };
                *v = o + f.next()?.parse::<f64>().ok()? * scale;
            }
            (r.iter().all(|v| v.is_finite()) && r[2] > r[0] && r[3] > r[1]).then_some(FocusTarget {
                movie: String::new(),
                path,
                rect: r,
            })
        })
        .collect()
}

/// One Scaleform movie playing into an offscreen texture.
pub struct UiMovie {
    pub name: String,
    player: own::OwnMovie,
    shared: Arc<Shared>,
    size: (u32, u32),
    /// The movie's authored stage size (pixels), which [`stage_scale`] fits to the window.
    authored: (f64, f64),
    /// Token observers the movie registered (callback id, engine value name), in
    /// registration order: values are pushed in that order, so a movie that registers a
    /// "ready" token after the data it guards sees the data first.
    observers: Vec<(String, String)>,
    /// The value last pushed to each observer.
    sent: std::collections::HashMap<String, UiValue>,
    /// Observers registered since the last push, heard from on the next one.
    fresh: Vec<String>,
    /// How many inline images each text field (by its index, see `avm1::IMG_TAKE`) shows.
    inline_images: std::collections::HashMap<u32, usize>,
    /// Clips the movie blends with `multiply` onto the game's picture, drawn in a pass of
    /// their own (see [`MultiplyPass`]), and the texture that pass is copied to.
    multiply_pass: Option<MultiplyPass>,
    multiply_texture: Option<wgpu::Texture>,
    above_texture: Option<wgpu::Texture>,
    own_texture: Option<wgpu::Texture>,
}

/// Clips a movie draws with the `multiply` blend onto what is under the interface (the
/// "New Quest" banner's blood splash darkens the game's picture). The movie renders into
/// its own transparent texture, where a multiply has nothing under it, so these clips are
/// drawn alone in a pass of their own, with the normal blend, and that picture is multiplied
/// onto the frame before the movie's own (where they are hidden).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MultiplyPass {
    /// Clips the pass hides: everything outside the multiplied clips' branch.
    pub hide: Vec<String>,
    /// Clips the pass shows: the multiplied clips' parent, hidden in the movie's own frame.
    pub show: Vec<String>,
    /// The `multiply` clips, drawn with the normal blend in the pass.
    pub multiply: Vec<String>,
    /// For clips that multiply onto the movie's own art (the loading screen's stains on its
    /// parchment): the pass is multiplied onto the frame after the movie's own picture, not
    /// before, and these clips, which the movie draws above the multiplied ones, follow it
    /// in a pass of their own (normal blend; hidden in the movie's own frame).
    pub above: Option<ClipPass>,
}

/// Which of a movie's pass textures a rendered frame is copied into.
#[derive(Clone, Copy)]
enum PassCopy {
    Multiply,
    Above,
    Own,
}

/// A pass of some of a movie's clips: those it hides, and those it shows (hidden in the
/// movie's own frame).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClipPass {
    pub hide: Vec<String>,
    pub show: Vec<String>,
}

/// An `<img>` tag in HTML text: where it is, its linkage name and its horizontal space.
#[derive(Clone, Debug, PartialEq)]
struct InlineImage {
    start: usize,
    end: usize,
    src: String,
    hspace: f64,
}

/// The `<img .../>` tags of `html`, in order.
fn inline_images(html: &str) -> Vec<InlineImage> {
    let lower = html.to_ascii_lowercase();
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(i) = lower[from..].find("<img") {
        let start = from + i;
        let Some(j) = lower[start..].find('>') else {
            break;
        };
        let end = start + j + 1;
        let tag = &html[start..end];
        let attr = |name: &str| -> Option<String> {
            let l = tag.to_ascii_lowercase();
            let p = l.find(&format!("{name}="))? + name.len() + 1;
            let rest = &tag[p..];
            let q = rest.chars().next()?;
            if q == '\'' || q == '"' {
                let e = rest[1..].find(q)?;
                Some(rest[1..1 + e].to_string())
            } else {
                Some(
                    rest.split(|c: char| c.is_whitespace() || c == '/' || c == '>')
                        .next()?
                        .to_string(),
                )
            }
        };
        if let Some(src) = attr("src") {
            out.push(InlineImage {
                start,
                end,
                src,
                hspace: attr("hspace").and_then(|v| v.parse().ok()).unwrap_or(0.0),
            });
        }
        from = end;
    }
    out
}

/// Where the last line of `html` starts (after its last line break).
/// Where the line of `html` that `from` is on ends: the first line break after it, or the
/// end.
fn line_end(html: &str, from: usize) -> usize {
    let lower = html.to_ascii_lowercase();
    let rest = lower.get(from..).unwrap_or("");
    ["<br>", "<br/>", "<br />", "</p>", "\n", "\r"]
        .iter()
        .filter_map(|b| rest.find(b).map(|i| from + i))
        .min()
        .unwrap_or(html.len())
}

fn last_line_start(html: &str) -> usize {
    let lower = html.to_ascii_lowercase();
    ["<br>", "<br/>", "<br />", "</p>", "\n", "\r"]
        .iter()
        .filter_map(|b| lower.rfind(b).map(|i| i + b.len()))
        .max()
        .unwrap_or(0)
}

/// Turns a callback id from the movie (a number) into its `cm_mapCallbacks` key.
fn callback_key(v: &UiValue) -> Option<String> {
    match v {
        UiValue::Number(n) if n.fract() == 0.0 => Some(format!("{}", *n as i64)),
        UiValue::String(s) if !s.is_empty() => Some(s.clone()),
        _ => None,
    }
}

impl UiMovie {
    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// Resizes the stage and its texture. Sizes are clamped to 1..=8192.
    pub fn resize(&mut self, (w, h): (u32, u32)) {
        let size = (w.clamp(1, 8192), h.clamp(1, 8192));
        self.size = size;
        self.player.resize(size);
    }

    /// Starts a movie from its GFx file as the game ships it.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        gpu: &Gpu,
        name: &str,
        gfx: &[u8],
        size: (u32, u32),
        lookup: ResourceLookup,
        atlas: Arc<Mutex<atlas::ImageSource>>,
        responder: Option<Responder>,
    ) -> Result<Self, UiError> {
        let shared = Arc::new(Shared::default());
        let bridge = Bridge {
            movie: name.to_string(),
            shared: shared.clone(),
            responder,
        };
        let m = own::OwnMovie::new(
            gpu,
            name,
            gfx,
            size,
            lookup,
            atlas,
            bridge,
            avm1::register_invoke(),
        )?;
        let authored = {
            let (w, h) = m.player.vm.movie.stage_size();
            (w as f64, h as f64)
        };
        Ok(Self {
            name: name.to_string(),
            player: m,
            shared,
            size,
            authored,
            observers: Default::default(),
            sent: Default::default(),
            fresh: Vec::new(),
            inline_images: Default::default(),
            multiply_pass: None,
            multiply_texture: None,
            above_texture: None,
            own_texture: None,
        })
    }

    /// The engine's drawing of image `name` (`paperdoll`) is now at `url`
    /// (`img://thedas_paperdoll_3.png`), or there is none.
    pub fn set_engine_image(&mut self, name: &str, url: Option<&str>) {
        self.player.set_engine_image(name, url);
    }

    /// Runs what the movies put off past the host's last step: post-load functions, then
    /// inline-image layout.
    fn after_step(&mut self) {
        self.call(avm1::POST_LOAD_RUN, &[]);
        self.layout_inline_images();
    }

    /// Draws the inline images of the text fields that took new text (see
    /// [`avm1::IMG_TAKE`]): each `<img>` is replaced in the text by enough non-breaking
    /// spaces to hold it, and the image, by its linkage name, is placed over them. An image
    /// is centred on its line, and a right-aligned or centred field's line (the stores'
    /// price columns) starts where its alignment puts it. Provisional: Scaleform's own image layout (`align`, `vspace`,
    /// `hspace`) is not traced, and a line the field wraps by itself is not followed.
    fn layout_inline_images(&mut self) {
        let UiValue::List(v) = self.call(avm1::IMG_TAKE, &[]) else {
            return;
        };
        let num = |x: &UiValue| match x {
            UiValue::Number(n) => *n,
            _ => 0.0,
        };
        for pair in v.chunks(2) {
            let (Some(UiValue::Number(k)), Some(UiValue::String(html))) =
                (pair.first(), pair.get(1))
            else {
                continue;
            };
            let k = *k;
            let images = inline_images(html);
            let old = self
                .inline_images
                .insert(k as u32, images.len())
                .unwrap_or(0);
            // Clear what an earlier text placed beyond this one's images.
            for n in images.len()..old {
                let args = [k, n as f64].map(UiValue::Number);
                self.call(
                    avm1::IMG_PUT,
                    &[
                        args[0].clone(),
                        args[1].clone(),
                        UiValue::String(String::new()),
                        UiValue::Number(0.0),
                        UiValue::Number(0.0),
                    ],
                );
            }
            if images.is_empty() {
                continue;
            }
            let put = |m: &mut Self, n: usize, src: &str, x: f64, y: f64| -> (f64, f64) {
                match m.call(
                    avm1::IMG_PUT,
                    &[
                        UiValue::Number(k),
                        UiValue::Number(n as f64),
                        UiValue::String(src.to_string()),
                        UiValue::Number(x),
                        UiValue::Number(y),
                    ],
                ) {
                    UiValue::List(l) if l.len() == 2 => (num(&l[0]), num(&l[1])),
                    _ => (0.0, 0.0),
                }
            };
            let measure = |m: &mut Self, h: &str, wrap: bool| -> (f64, f64) {
                match m.call(
                    avm1::IMG_MEASURE,
                    &[
                        UiValue::Number(k),
                        UiValue::String(h.to_string()),
                        UiValue::Bool(wrap),
                    ],
                ) {
                    UiValue::List(l) if l.len() == 2 => (num(&l[0]), num(&l[1])),
                    _ => (0.0, 0.0),
                }
            };
            // The images' sizes, then room for them in the text.
            let sizes: Vec<(f64, f64)> = images
                .iter()
                .enumerate()
                .map(|(n, im)| put(self, n, &im.src, -10000.0, -10000.0))
                .collect();
            let space = (measure(self, "&nbsp;&nbsp;&nbsp;&nbsp;", false).0 / 4.0).max(1.0);
            let mut text = String::new();
            let mut at = Vec::new();
            let mut last = 0;
            for (im, (w, _)) in images.iter().zip(&sizes) {
                text.push_str(&html[last..im.start]);
                at.push(text.len());
                let n = ((w + 2.0 * im.hspace) / space).ceil().max(1.0) as usize;
                text.push_str(&"&nbsp;".repeat(n));
                last = im.end;
            }
            text.push_str(&html[last..]);
            self.call(
                avm1::IMG_SET,
                &[UiValue::Number(k), UiValue::String(text.clone())],
            );
            let line = measure(self, "&nbsp;", true).1.max(1.0);
            // A right-aligned or centred field (a price column) starts each line that much
            // further in: the room the line leaves, all or half.
            let (field_width, align) = match self.call(avm1::IMG_FIELD, &[UiValue::Number(k)]) {
                UiValue::List(l) if l.len() == 2 => (
                    num(&l[0]),
                    match &l[1] {
                        UiValue::String(a) => a.to_ascii_lowercase(),
                        _ => String::new(),
                    },
                ),
                _ => (0.0, String::new()),
            };
            for (n, ((im, &(w, h)), &pos)) in images.iter().zip(&sizes).zip(&at).enumerate() {
                let before = &text[..pos];
                let lines = measure(self, &format!("{before}&nbsp;"), true).1;
                let line_start = last_line_start(before);
                let room = match align.as_str() {
                    "right" | "center" => {
                        let line_text = &text[line_start..line_end(&text, pos)];
                        // The field's gutters, 2 pixels each side.
                        let room = (field_width - 4.0 - measure(self, line_text, false).0).max(0.0);
                        if align == "right" { room } else { room / 2.0 }
                    }
                    _ => 0.0,
                };
                let x = room + measure(self, &before[line_start..], false).0 + im.hspace;
                let y = (lines - line) + (line - h) / 2.0;
                // The text field's gutter (2 pixels each way), and a pixel's gap from the
                // text before (the measuring field came out a little narrower than the
                // field's own text). Provisional.
                let _ = w;
                let gap = if pos > 0 { 1.0 } else { 0.0 };
                put(self, n, &im.src, x + 2.0 + gap, y + 2.0);
            }
        }
    }

    /// Advances the movie by `dt_ms` of wall time and runs pending loads.
    pub fn tick(&mut self, dt_ms: f64) {
        self.player.tick(dt_ms);
        self.after_step();
    }

    /// Runs exactly one movie frame, independent of time.
    pub fn run_frame(&mut self) {
        self.player.run_frame();
        self.after_step();
    }

    /// Renders the current frame into the texture.
    pub fn render(&mut self) {
        self.player.render();
    }

    /// Whether the movie has a multiply pass (see [`MultiplyPass`]).
    pub fn has_multiply_pass(&self) -> bool {
        self.multiply_pass.is_some()
    }

    /// The clips to draw in a multiply pass of their own, or none (see [`MultiplyPass`]).
    pub fn set_multiply_pass(&mut self, pass: Option<MultiplyPass>) {
        self.multiply_pass = pass;
    }

    /// Sets the movie up for its multiply pass, runs `f`, and puts the clips back as they
    /// were (their visibility read first: the movie may have changed it since the pass was
    /// made). `None` without a pass.
    fn in_multiply_pass<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> Option<R> {
        let pass = self.multiply_pass.clone()?;
        Some(self.in_pass(&pass.hide, &pass.show, &pass.multiply, f))
    }

    /// Sets the movie up for its above pass (see [`MultiplyPass::above`]), as
    /// [`UiMovie::in_multiply_pass`] does. `None` without one.
    fn in_above_pass<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> Option<R> {
        let above = self.multiply_pass.as_ref()?.above.clone()?;
        Some(self.in_pass(&above.hide, &above.show, &[], f))
    }

    /// Hides `hide`, shows `show` and draws `multiply` with the normal blend while `f` runs,
    /// then puts them back as they were.
    fn in_pass<R>(
        &mut self,
        hide: &[String],
        show: &[String],
        multiply: &[String],
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        let pass = MultiplyPass {
            hide: hide.to_vec(),
            show: show.to_vec(),
            multiply: multiply.to_vec(),
            above: None,
        };
        let visible = |m: &mut Self, path: &str| {
            m.call(&format!("{path}._visible.toString"), &[]) == UiValue::String("true".into())
        };
        let was: Vec<(String, bool)> = pass
            .hide
            .iter()
            .chain(&pass.show)
            .map(|p| (p.clone(), visible(self, p)))
            .collect();
        for p in &pass.hide {
            self.set(&format!("{p}._visible"), UiValue::Bool(false));
        }
        for p in &pass.show {
            self.set(&format!("{p}._visible"), UiValue::Bool(true));
        }
        for p in &pass.multiply {
            self.set(&format!("{p}.blendMode"), UiValue::String("normal".into()));
        }
        let r = f(self);
        for p in &pass.multiply {
            self.set(
                &format!("{p}.blendMode"),
                UiValue::String("multiply".into()),
            );
        }
        for (p, v) in was {
            self.set(&format!("{p}._visible"), UiValue::Bool(v));
        }
        r
    }

    /// Renders the movie's multiply pass (see [`MultiplyPass`]) and returns a copy of it
    /// (RGBA8, premultiplied), to multiply onto the frame under the movie's own picture.
    /// `None` without a pass.
    pub fn render_multiply(&mut self) -> Option<wgpu::Texture> {
        self.in_multiply_pass(|m| {
            m.render();
            m.copy_rendered(PassCopy::Multiply)
        })
        .flatten()
    }

    /// Renders the movie's above pass (see [`MultiplyPass::above`]) and returns a copy of
    /// it, drawn normally after the multiply pass. `None` without one.
    pub fn render_above(&mut self) -> Option<wgpu::Texture> {
        self.in_above_pass(|m| {
            m.render();
            m.copy_rendered(PassCopy::Above)
        })
        .flatten()
    }

    /// Whether the movie's multiply pass goes onto its own picture, followed by its above
    /// pass (see [`MultiplyPass::above`]), rather than under it.
    pub fn multiplies_over(&self) -> bool {
        self.multiply_pass
            .as_ref()
            .is_some_and(|p| p.above.is_some())
    }

    /// A copy of the movie's own frame as last rendered, for a movie that renders more
    /// passes after it (see [`UiMovie::multiplies_over`]).
    pub fn render_own_copy(&mut self) -> Option<wgpu::Texture> {
        self.copy_rendered(PassCopy::Own)
    }

    /// A copy of the frame just rendered, kept in one of the movie's pass textures: the
    /// movie renders the next pass into the same target.
    fn copy_rendered(&mut self, which: PassCopy) -> Option<wgpu::Texture> {
        let texture = self.player.texture()?;
        let (device, queue) = self.player.device();
        let slot = match which {
            PassCopy::Multiply => &mut self.multiply_texture,
            PassCopy::Above => &mut self.above_texture,
            PassCopy::Own => &mut self.own_texture,
        };
        let same = slot
            .as_ref()
            .is_some_and(|t| t.size() == texture.size() && t.format() == texture.format());
        if !same {
            *slot = Some(device.create_texture(&wgpu::TextureDescriptor {
                label: Some("ui pass"),
                size: texture.size(),
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: texture.format(),
                usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            }));
        }
        let copy = slot.clone()?;
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_texture(
            texture.as_image_copy(),
            copy.as_image_copy(),
            texture.size(),
        );
        queue.submit([encoder.finish()]);
        Some(copy)
    }

    /// The movie's multiply pass read back to the CPU (RGBA8, premultiplied), as
    /// [`UiMovie::render_multiply`] draws it. `None` without a pass.
    pub fn capture_multiply(&mut self) -> Option<Rgba> {
        self.in_multiply_pass(|m| {
            m.render();
            m.capture()
        })
        .flatten()
    }

    /// The movie's above pass read back to the CPU, as [`UiMovie::render_above`] draws it.
    pub fn capture_above(&mut self) -> Option<Rgba> {
        self.in_above_pass(|m| {
            m.render();
            m.capture()
        })
        .flatten()
    }

    /// The texture the movie renders into (RGBA8, premultiplied alpha).
    pub fn texture(&self) -> Option<wgpu::Texture> {
        self.player.texture()
    }

    /// Reads the last rendered frame back to the CPU (RGBA8, premultiplied alpha).
    pub fn capture(&mut self) -> Option<Rgba> {
        self.player.capture()
    }

    pub fn mouse_move(&mut self, x: f64, y: f64) {
        self.player.mouse_move(x, y);
    }

    /// The mouse left the window. The current player keeps its last pointer position.
    pub fn mouse_leave(&mut self) {
        // Window leave has no movie event in the current player.
    }

    /// The mouse wheel turned `lines` (up positive) with the mouse at (`x`, `y`): the movie's
    /// `onMouseWheel` listeners and the scrollable under the mouse hear it.
    pub fn mouse_wheel(&mut self, x: f64, y: f64, lines: f64) {
        self.player.mouse_wheel(x, y, lines);
    }

    /// A key pressed and released: the movie's key listeners hear it (`Key.getCode`), and a
    /// focused text field takes the character or the edit.
    pub fn key(&mut self, key: UiKey) {
        self.player.key(key);
    }

    pub fn mouse_button(&mut self, x: f64, y: f64, down: bool) {
        self.button_as(avm1::MOUSE_LEFT, x, y, down);
    }

    /// A press or release with Scaleform's button code (button + 4 for a double click).
    pub fn button_as(&mut self, button: f64, x: f64, y: f64, down: bool) {
        self.player.mouse_button(x, y, button, down);
        self.after_step();
    }

    /// The engine side of the movies' `TokenManager`: collects observers registered with
    /// `AddTokenObserverWithID` (batched as `[id, token, flags]` triples in
    /// `TokenManager.AddTokenObserverBatchData`) or `AddTokenObserverFunc` (through
    /// `ExternalInterface`) and removed ones, then pushes each observed
    /// value that changed to `TokenManager.cm_mapCallbacks[id](value)`. `value` answers a
    /// token or returns `None` (the observer is then left alone). Returns the tokens nothing
    /// answered. Provisional, from the movies' bytecode.
    pub fn sync_tokens(&mut self, value: &dyn Fn(&str) -> Option<UiValue>) -> Vec<String> {
        const TM: &str = "_global.TokenManager";
        self.fresh.clear();
        let take = |m: &mut Self, list: &str| match m
            .call(&format!("{TM}.{list}.splice"), &[UiValue::Number(0.0)])
        {
            UiValue::List(v) => v,
            _ => Vec::new(),
        };
        let added = take(self, "AddTokenObserverBatchData");
        for t in added.chunks(3) {
            if let (Some(id), Some(UiValue::String(token))) =
                (t.first().and_then(callback_key), t.get(1))
            {
                // A new observer hears the value at once, as from the game, even when the id
                // was fed before (a screen's clips placed again register their ids anew).
                self.observers.retain(|(i, _)| *i != id);
                self.sent.remove(&id);
                self.fresh.push(id.clone());
                self.observers.push((id, token.clone()));
            }
        }
        let called = self
            .shared
            .called_observers
            .lock()
            .map(|mut o| std::mem::take(&mut *o))
            .unwrap_or_default();
        for (id, token) in called {
            self.observers.retain(|(i, _)| *i != id);
            self.sent.remove(&id);
            self.fresh.push(id.clone());
            self.observers.push((id, token));
        }
        for r in take(self, "RemoveTokenObserverBatchData") {
            if let Some(id) = callback_key(&r) {
                self.observers.retain(|(i, _)| *i != id);
                self.sent.remove(&id);
            }
        }
        let mut missing = Vec::new();
        let trace = std::env::var_os("THEDAS_TOKEN_LOG").is_some();
        for (id, token) in self.observers.clone() {
            // Registered just now: heard from on the next push, once the clips placed with
            // the registering one exist (inventory.gfx's equipment layout hid nothing when
            // pushed at once). Pushing at once as well (#182) made conversation.gfx loop: its
            // reply and subtitle callbacks place their clips anew, which register anew, so
            // every push re-placed them and the replies never showed.
            if self.fresh.contains(&id) {
                continue;
            }
            match value(&token) {
                Some(v) => {
                    if self.sent.get(&id) != Some(&v) {
                        if trace {
                            log::info!("{}: token {token} ({id}) = {v:?}", self.name);
                        }
                        self.call(
                            &format!("{TM}.cm_mapCallbacks.{id}"),
                            std::slice::from_ref(&v),
                        );
                        self.sent.insert(id, v);
                    }
                }
                None => missing.push(token),
            }
        }
        missing
    }

    /// Has the token observers whose engine value name `pick` accepts hear their value
    /// again at the next push, changed or not.
    pub fn resend_observers(&mut self, pick: impl Fn(&str) -> bool) {
        for (id, token) in &self.observers {
            if pick(token) {
                self.sent.remove(id);
            }
        }
    }

    /// Calls a movie function by path, as Scaleform's `Invoke` does
    /// (`_global.GUISystem.StartSceneFromGame`).
    pub fn call(&mut self, path: &str, args: &[UiValue]) -> UiValue {
        let Some((object, method)) = path.rsplit_once('.') else {
            return UiValue::Undefined;
        };
        self.invoke(
            avm1::INVOKE_CALLBACK,
            &[
                UiValue::String(object.to_string()),
                UiValue::String(method.to_string()),
                UiValue::List(args.to_vec()),
            ],
        )
    }

    /// Sets a movie variable by path (`_global.GUISystem.OriginalStageWidth`), as Scaleform's
    /// `SetVariable` does.
    pub fn set(&mut self, path: &str, value: UiValue) {
        if let Some((object, member)) = path.rsplit_once('.') {
            self.invoke(
                avm1::SET_CALLBACK,
                &[
                    UiValue::String(object.to_string()),
                    UiValue::String(member.to_string()),
                    value,
                ],
            );
        }
    }

    /// What a pad's focus can land on in this movie: each visible, enabled clip that takes a
    /// click, with its bounds in window pixels (see `avm1::focus_function`).
    pub fn focus_targets(&mut self) -> Vec<FocusTarget> {
        let UiValue::String(list) = self.call(avm1::FOCUS_LIST, &[]) else {
            return Vec::new();
        };
        // The stage plays unscaled (`NoScale`) at `stage_scale`, its authored rectangle
        // centred in the window. Provisional: read from where the start menu's buttons and
        // its login panel are drawn, in the authored movie.
        let scale = stage_scale(self.size, self.authored);
        let (w, h) = (f64::from(self.size.0), f64::from(self.size.1));
        let offset = (
            (w - self.authored.0 * scale) / 2.0,
            (h - self.authored.1 * scale) / 2.0,
        );
        let mut seen = std::collections::HashSet::new();
        parse_focus_list(&list, scale, offset)
            .into_iter()
            .filter_map(|mut t| {
                // Only what shows in the window, clipped to it.
                t.rect = [
                    t.rect[0].max(0.0),
                    t.rect[1].max(0.0),
                    t.rect[2].min(w),
                    t.rect[3].min(h),
                ];
                let (tw, th) = (t.rect[2] - t.rect[0], t.rect[3] - t.rect[1]);
                // A clip as big as most of the window catches clicks behind a dialog box (a
                // popup's background), not a button.
                let shows = tw >= 2.0 && th >= 2.0 && tw * th < w * h * 0.25;
                (shows && seen.insert(t.path.clone())).then(|| {
                    t.movie = self.name.clone();
                    t
                })
            })
            .collect()
    }

    /// Calls a function the movie registered with `ExternalInterface.addCallback`.
    pub fn invoke(&mut self, name: &str, args: &[UiValue]) -> UiValue {
        self.player.invoke(name, args)
    }

    /// Calls the movie made since the last drain.
    pub fn drain_calls(&self) -> Vec<UiCall> {
        self.shared
            .calls
            .lock()
            .map(|mut c| std::mem::take(&mut *c))
            .unwrap_or_default()
    }

    /// Names the movie registered with `ExternalInterface.addCallback`.
    pub fn callbacks(&self) -> Vec<String> {
        self.player.callbacks()
    }

    /// AVM traces and warnings since the last drain.
    pub fn drain_log(&mut self) -> Vec<String> {
        self.player.drain_log()
    }
}

#[cfg(test)]
mod inline_image_tests {
    use super::*;

    #[test]
    fn img_tags_are_found_with_their_names_and_spacing() {
        let html = "Value: 1<img src='ico_silver' align='baseline' vspace='-10'/> \
                    0<IMG SRC=\"ico_copper\" hspace=3>";
        let im = inline_images(html);
        assert_eq!(im.len(), 2);
        assert_eq!(&html[im[0].start..im[0].start + 4], "<img");
        assert_eq!(im[0].src, "ico_silver");
        assert_eq!((im[1].src.as_str(), im[1].hspace), ("ico_copper", 3.0));
        assert_eq!(im[1].end, html.len());
        assert!(inline_images("no images").is_empty());
    }

    #[test]
    fn the_last_line_starts_after_the_last_break() {
        assert_eq!(last_line_start("abc"), 0);
        assert_eq!(last_line_start("a<br>bc"), 5);
        assert_eq!(last_line_start("a<BR/>b\nc"), 8);
    }

    #[test]
    fn a_line_ends_at_the_next_break() {
        assert_eq!(line_end("abc", 0), 3);
        assert_eq!(line_end("a<br>bc<BR/>d", 1), 1);
        assert_eq!(line_end("a<br>bc<BR/>d", 5), 7);
        assert_eq!(line_end("ab", 9), 2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stage_fits_the_authored_movie_whole_and_reports_the_visible_area() {
        let authored = (1024.0, 768.0);
        // 16:9: height-limited, so the stage is wider than authored (1365 x 768 stage units).
        let k = stage_scale((1920, 1080), authored);
        assert!((k - 1.40625).abs() < 1e-9);
        assert_eq!((1920.0 / k).round(), 1365.0);
        assert_eq!((1080.0 / k).round(), 768.0);
        // 4:3 at the authored size: one to one.
        assert_eq!(stage_scale((1024, 768), authored), 1.0);
        // Taller than 4:3: width-limited, the stage grows downwards instead.
        let k = stage_scale((1000, 1000), authored);
        assert!((1000.0 / k - 1024.0).abs() < 1e-9 && 1000.0 / k > 768.0);
        // A movie with no size of its own is not scaled.
        assert_eq!(stage_scale((800, 600), (0.0, 0.0)), 1.0);
    }
}
