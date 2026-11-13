//! The game's Scaleform GFx interface, played by Ruffle on the renderer's wgpu device.
//!
//! Each [`UiMovie`] owns one Ruffle player rendering into an offscreen RGBA texture that the
//! application composites over the scene. Movies are rewritten to plain SWF first
//! ([`gfx::rewrite`]) so images the exporter moved into texture atlases are inlined.
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

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use ruffle_core::backend::navigator::NullExecutor;
use ruffle_core::context::UpdateContext;
use ruffle_core::events::{
    KeyDescriptor, KeyLocation, LogicalKey, MouseButton, NamedKey, PhysicalKey, PlayerEvent,
    TextControlCode,
};
use ruffle_core::external::{ExternalInterfaceProvider, FsCommandProvider, Value as RValue};
use ruffle_core::tag_utils::SwfMovieData;
use ruffle_core::{FloatDuration, Player, PlayerBuilder, StageScaleMode};
use ruffle_render_wgpu::backend::WgpuRenderBackend;
use ruffle_render_wgpu::descriptors::Descriptors;
use ruffle_render_wgpu::target::TextureTarget;

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

impl UiKey {
    fn descriptor(self) -> (PhysicalKey, LogicalKey) {
        let named = |p, n| (p, LogicalKey::Named(n));
        match self {
            UiKey::Char(c) => (char_physical(c), LogicalKey::Character(c)),
            UiKey::Backspace => named(PhysicalKey::Backspace, NamedKey::Backspace),
            UiKey::Delete => named(PhysicalKey::Delete, NamedKey::Delete),
            UiKey::Enter => named(PhysicalKey::Enter, NamedKey::Enter),
            UiKey::Escape => named(PhysicalKey::Escape, NamedKey::Escape),
            UiKey::Tab => named(PhysicalKey::Tab, NamedKey::Tab),
            UiKey::Left => named(PhysicalKey::ArrowLeft, NamedKey::ArrowLeft),
            UiKey::Right => named(PhysicalKey::ArrowRight, NamedKey::ArrowRight),
            UiKey::Home => named(PhysicalKey::Home, NamedKey::Home),
            UiKey::End => named(PhysicalKey::End, NamedKey::End),
            UiKey::SelectAll => (PhysicalKey::KeyA, LogicalKey::Character('a')),
        }
    }

    /// The edit a text field makes for the key.
    fn text_control(self) -> Option<TextControlCode> {
        Some(match self {
            UiKey::Backspace => TextControlCode::Backspace,
            UiKey::Delete => TextControlCode::Delete,
            UiKey::Enter => TextControlCode::Enter,
            UiKey::Left => TextControlCode::MoveLeft,
            UiKey::Right => TextControlCode::MoveRight,
            UiKey::Home => TextControlCode::MoveLeftDocument,
            UiKey::End => TextControlCode::MoveRightDocument,
            UiKey::SelectAll => TextControlCode::SelectAll,
            _ => return None,
        })
    }
}

/// The physical key that types a character on a US layout (Unknown for others).
fn char_physical(c: char) -> PhysicalKey {
    use PhysicalKey as P;
    const LETTERS: [PhysicalKey; 26] = [
        P::KeyA,
        P::KeyB,
        P::KeyC,
        P::KeyD,
        P::KeyE,
        P::KeyF,
        P::KeyG,
        P::KeyH,
        P::KeyI,
        P::KeyJ,
        P::KeyK,
        P::KeyL,
        P::KeyM,
        P::KeyN,
        P::KeyO,
        P::KeyP,
        P::KeyQ,
        P::KeyR,
        P::KeyS,
        P::KeyT,
        P::KeyU,
        P::KeyV,
        P::KeyW,
        P::KeyX,
        P::KeyY,
        P::KeyZ,
    ];
    const DIGITS: [PhysicalKey; 10] = [
        P::Digit0,
        P::Digit1,
        P::Digit2,
        P::Digit3,
        P::Digit4,
        P::Digit5,
        P::Digit6,
        P::Digit7,
        P::Digit8,
        P::Digit9,
    ];
    match c {
        'a'..='z' => LETTERS[c as usize - 'a' as usize],
        'A'..='Z' => LETTERS[c as usize - 'A' as usize],
        '0'..='9' => DIGITS[c as usize - '0' as usize],
        ' ' => P::Space,
        _ => P::Unknown,
    }
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

impl From<&RValue> for UiValue {
    fn from(v: &RValue) -> Self {
        match v {
            RValue::Undefined => Self::Undefined,
            RValue::Null => Self::Null,
            RValue::Bool(b) => Self::Bool(*b),
            RValue::Number(n) => Self::Number(*n),
            RValue::String(s) if s == avm1::EMPTY => Self::String(String::new()),
            RValue::String(s) => Self::String(s.clone()),
            RValue::List(l) => Self::List(l.iter().map(Into::into).collect()),
            RValue::Object(o) => {
                Self::Object(o.iter().map(|(k, v)| (k.clone(), v.into())).collect())
            }
        }
    }
}

impl From<&UiValue> for RValue {
    fn from(v: &UiValue) -> Self {
        match v {
            UiValue::Undefined => Self::Undefined,
            UiValue::Null => Self::Null,
            UiValue::Bool(b) => Self::Bool(*b),
            UiValue::Number(n) => Self::Number(*n),
            // See `avm1::EMPTY`.
            UiValue::String(s) if s.is_empty() => Self::String(avm1::EMPTY.to_string()),
            UiValue::String(s) => Self::String(s.clone()),
            UiValue::List(l) => Self::List(l.iter().map(Into::into).collect()),
            UiValue::Object(o) => {
                Self::Object(o.iter().map(|(k, v)| (k.clone(), v.into())).collect())
            }
        }
    }
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
    callbacks: Mutex<Vec<String>>,
    log: Mutex<Vec<String>>,
    /// Observers registered through `ExternalInterface.call("AddTokenObserver", ...)`
    /// (callback id, token), for [`UiMovie::sync_tokens`] to take up.
    called_observers: Mutex<Vec<(String, String)>>,
    /// The last id handed out for those.
    observer_ids: std::sync::atomic::AtomicU64,
}

/// The first id handed to `AddTokenObserver` observers: past the ids the movies number their
/// own observers with (`_OBSERVER_ID` constants and counters). Provisional.
const CALLED_OBSERVER_IDS: u64 = 1_000_000;

struct Bridge {
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

impl FsCommandProvider for Bridge {
    fn on_fs_command(&self, command: &str, args: &str) -> bool {
        let r = self.push(UiCall {
            movie: self.movie.clone(),
            kind: CallKind::FsCommand,
            name: command.to_string(),
            args: vec![UiValue::String(args.to_string())],
        });
        if let Ok(mut v) = self.shared.retval.lock() {
            *v = r;
        }
        true
    }
}

impl ExternalInterfaceProvider for Bridge {
    fn call_method(&self, _: &mut UpdateContext<'_>, name: &str, args: &[RValue]) -> RValue {
        // `TokenManager.AddTokenObserverFunc(label, token, callback)` asks the game for the
        // observer's id (`AddTokenObserver(label, "", token)`) and files the callback under
        // it: the observer is the game's to feed from then on, as for
        // `AddTokenObserverWithID` (optionsmenu.gfx's checkboxes and sliders).
        if name == "AddTokenObserver" {
            let token: UiValue = args.get(2).map(Into::into).unwrap_or(UiValue::Undefined);
            let UiValue::String(token) = token else {
                return RValue::Undefined;
            };
            let id = CALLED_OBSERVER_IDS
                + self
                    .shared
                    .observer_ids
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if let Ok(mut o) = self.shared.called_observers.lock() {
                o.push((id.to_string(), token));
            }
            return RValue::Number(id as f64);
        }
        if name == avm1::RETVAL_CALL {
            let v = self
                .shared
                .retval
                .lock()
                .map(|mut v| std::mem::replace(&mut *v, UiValue::Undefined))
                .unwrap_or(UiValue::Undefined);
            return (&v).into();
        }
        let r = self.push(UiCall {
            movie: self.movie.clone(),
            kind: CallKind::ExternalInterface,
            name: name.to_string(),
            args: args.iter().map(Into::into).collect(),
        });
        (&r).into()
    }

    fn on_callback_available(&self, name: &str) {
        if let Ok(mut c) = self.shared.callbacks.lock() {
            c.push(name.to_string());
        }
    }

    fn get_id(&self) -> Option<String> {
        None
    }
}

struct Log(Arc<Shared>);

impl ruffle_core::backend::log::LogBackend for Log {
    fn avm_trace(&self, message: &str) {
        log::debug!("ui trace: {message}");
        if let Ok(mut l) = self.0.log.lock() {
            l.push(format!("trace: {message}"));
        }
    }

    fn avm_warning(&self, message: &str) {
        log::debug!("ui warning: {message}");
        if let Ok(mut l) = self.0.log.lock() {
            l.push(format!("warning: {message}"));
        }
    }
}

/// The wgpu handles the movie renders with; clones of the renderer's.
#[derive(Clone)]
pub struct Gpu {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

/// Loads `name` (e.g. `mainmenu.gfx`) and rewrites it to a plain SWF with atlas images inlined.
pub fn load_movie(
    name: &str,
    lookup: &ResourceLookup,
    images: &mut ImageSource,
) -> Result<(Vec<u8>, RewriteReport), UiError> {
    let data = lookup(name).ok_or_else(|| UiError::NotFound(name.to_string()))?;
    let inject = avm1::register_invoke();
    let fonts = font_library(lookup);
    let (mut swf, report) = gfx::rewrite_with_slots(
        &data,
        &mut |e| images.image(&e.file),
        fonts.as_ref(),
        Some(&inject),
        ENGINE_IMAGES,
    )?;
    if name.eq_ignore_ascii_case(stage::POPUP_LAYER) {
        allow_spaces_in_typed_names(&mut swf);
    }
    Ok((swf, report))
}

/// The restrict popuplayer.gfx's text input sets (`TextInput_mc_init`: `restrict = "^ /t"`,
/// in its constant pool), as it appears there.
const POPUP_RESTRICT: &[u8] = b"restrict\0^ /t\0";
/// What it is replaced with: `/` alone excluded (the same length, so the pool is unchanged).
const POPUP_RESTRICT_ALLOWED: &[u8] = b"restrict\0^///\0";

/// Lets the save dialog's name field take spaces and a lowercase `t`.
///
/// A deliberate deviation, asked for by the project owner, not traced in DAOrigins.exe. The
/// movie's own restrict excludes space, `/` and `t` in Flash's syntax, so Ruffle drops spaces
/// and, as Flash does, types `t` as `T` ("Before the Tower" became "BeforeTheTower"). The
/// engine adds no character filter of its own: `FilterKeyboardInput` (handler 0x51c760 →
/// 0x5110f0) only enters or leaves the "GUITextInput" input context. How Scaleform read this
/// restrict is not verified; the owner believes the original allows spaces, and wants them
/// either way. `/` stays excluded, as the movie asks.
fn allow_spaces_in_typed_names(swf: &mut [u8]) {
    if let Some(i) = swf
        .windows(POPUP_RESTRICT.len())
        .position(|w| w == POPUP_RESTRICT)
    {
        swf[i..i + POPUP_RESTRICT.len()].copy_from_slice(POPUP_RESTRICT_ALLOWED);
    }
}

/// Images the engine draws into the movies while they run (Scaleform render targets): the
/// paperdoll (DAOrigins.exe's `GUIPaperdoll` names its target `paperdoll`, 0x7bc659).
pub const ENGINE_IMAGES: &[&str] = &["paperdoll"];

/// The font library the game installs for Scaleform (see [`gfx::FontLibrary`]).
pub const FONT_LIBRARY: &str = "fonts_en-us.gfx";

/// The font library's fonts, if the resources have it.
pub fn font_library(lookup: &ResourceLookup) -> Option<gfx::FontLibrary> {
    let lib = gfx::FontLibrary::from_movie(&lookup(FONT_LIBRARY)?).ok()?;
    (!lib.is_empty()).then_some(lib)
}

/// Pre-rewrites imported libraries (`gfxfontlib.swf`) the same way when they are fetched.
fn rewriting_lookup(lookup: ResourceLookup) -> ResourceLookup {
    let fonts = std::sync::OnceLock::new();
    Arc::new(move |name: &str| {
        let data = lookup(name)?;
        if name.ends_with(".gfx") {
            let fonts = fonts.get_or_init(|| font_library(&lookup));
            if let Ok((swf, _)) = gfx::rewrite(&data, &mut |_| None, fonts.as_ref(), None) {
                return Some(swf);
            }
        }
        Some(data)
    })
}

/// How many window pixels one stage pixel takes: the movie's authored size fitted inside the
/// window (`size`), whole, centred.
///
/// DAOrigins.exe leaves its movies in Scaleform's show-all scale mode (the movie root's
/// constructor sets it, 0x95af2a, and the engine never calls `SetViewScaleMode`) and centres
/// them (`SetViewAlignment(Align_Center)` on each new movie view, 0x514aaa). Scaleform's
/// `Stage.width`/`height` then report the whole visible area in stage units, wider than the
/// authored stage on a wide window; the movies anchor to that rectangle
/// (`SceneManager.SetSafeZone`: `sm_rScreenRect` is `Stage.width` x `Stage.height` at
/// `((OriginalStageWidth - Stage.width) / 2, ...)`). Flash (and Ruffle) report the authored
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
    player: Arc<Mutex<Player>>,
    executor: NullExecutor,
    shared: Arc<Shared>,
    fetches: navigator::FetchLog,
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
    /// The movie's scaling-grid sprites (see `gfx::Scale9`), by sprite id, and the scale each
    /// registered one was last laid out for.
    scale9: BTreeMap<u16, gfx::Scale9>,
    scale9_laid: Vec<(f64, f64)>,
    /// The movie's engine-image slots (see `gfx::Slot`), by image name; the engine's current
    /// drawing of each image (its URL); the URL each registered slot last loaded.
    slots: BTreeMap<String, gfx::Slot>,
    engine_images: BTreeMap<String, String>,
    slots_loaded: Vec<String>,
    /// How many inline images each text field (by its index, see `avm1::IMG_TAKE`) shows.
    inline_images: std::collections::HashMap<u32, usize>,
    /// Clips the movie blends with `multiply` onto the game's picture, drawn in a pass of
    /// their own (see [`MultiplyPass`]), and the texture that pass is copied to.
    multiply_pass: Option<MultiplyPass>,
    multiply_texture: Option<wgpu::Texture>,
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
    /// Starts a movie from SWF bytes (see [`load_movie`]). `lookup` serves the movie's own
    /// loads and imports.
    pub fn new(
        gpu: &Gpu,
        name: &str,
        swf: &[u8],
        size: (u32, u32),
        lookup: ResourceLookup,
        responder: Option<Responder>,
    ) -> Result<Self, UiError> {
        let descriptors = Arc::new(Descriptors::new(
            gpu.instance.clone(),
            gpu.adapter.clone(),
            gpu.device.clone(),
            gpu.queue.clone(),
        ));
        let target =
            TextureTarget::new(&gpu.device, size).map_err(|e| UiError::Render(e.to_string()))?;
        let backend = WgpuRenderBackend::new(descriptors, target)
            .map_err(|e| UiError::Render(e.to_string()))?;
        let url = format!("{}{name}", navigator::BASE_URL);
        let movie = SwfMovieData::from_data(swf, url, None, None)
            .map_err(|e| UiError::Swf(e.to_string()))?;
        let authored = (movie.width().to_pixels(), movie.height().to_pixels());
        let shared = Arc::new(Shared::default());
        let executor = NullExecutor::new();
        let fetches = navigator::FetchLog::default();
        let nav =
            navigator::ResourceNavigator::new(rewriting_lookup(lookup), &executor, fetches.clone());
        let bridge = || Bridge {
            movie: name.to_string(),
            shared: shared.clone(),
            responder: responder.clone(),
        };
        let player = PlayerBuilder::new()
            .with_renderer(backend)
            .with_navigator(nav)
            .with_log(Log(shared.clone()))
            .with_fs_commands(Box::new(bridge()))
            .with_external_interface(Box::new(bridge()))
            .with_viewport_dimensions(size.0, size.1, stage_scale(size, authored))
            .with_scale_mode(StageScaleMode::NoScale, true)
            .with_autoplay(true)
            // The whole movie is loaded before its first frame runs. Ruffle otherwise loads
            // it a frame's worth of time at a go; on a slow start (a debug build, a busy
            // machine) frame one was not in by the time the game starts the scene, the
            // start call found nothing to call, and the movie then started itself as in a
            // standalone Flash player: label names for text, test content (popuplayer.gfx's
            // "Sell how many items?" box) and art left where it was authored.
            .with_load_behavior(ruffle_core::LoadBehavior::Blocking)
            .with_movie(movie)
            .build();
        if let Ok(mut p) = player.lock() {
            p.set_window_mode("transparent");
        }
        Ok(Self {
            name: name.to_string(),
            player,
            executor,
            shared,
            fetches,
            size,
            authored,
            observers: Default::default(),
            sent: Default::default(),
            fresh: Vec::new(),
            scale9: BTreeMap::new(),
            scale9_laid: Vec::new(),
            slots: BTreeMap::new(),
            engine_images: BTreeMap::new(),
            slots_loaded: Vec::new(),
            inline_images: Default::default(),
            multiply_pass: None,
            multiply_texture: None,
        })
    }

    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// Resizes the stage and its texture. Sizes are clamped to 1..=8192.
    pub fn resize(&mut self, (w, h): (u32, u32)) {
        let size = (w.clamp(1, 8192), h.clamp(1, 8192));
        self.size = size;
        let scale_factor = stage_scale(size, self.authored);
        self.with_player(|p| {
            p.set_viewport_dimensions(ruffle_core::ViewportDimensions {
                width: size.0,
                height: size.1,
                scale_factor,
            })
        });
    }

    fn with_player<R>(&self, f: impl FnOnce(&mut Player) -> R) -> Option<R> {
        self.player.lock().ok().map(|mut p| f(&mut p))
    }

    /// The movie's scaling-grid sprites, from its rewrite (`RewriteReport::scale9`).
    pub fn set_scale9(&mut self, scale9: BTreeMap<u16, gfx::Scale9>) {
        self.scale9 = scale9;
    }

    /// Lays out the pieces of the scaling-grid sprites whose scale changed since they were
    /// last laid out (see `gfx::scale9_layout`): Ruffle draws a grid sprite scaled as a whole.
    fn layout_scale9(&mut self) {
        if self.scale9.is_empty() {
            return;
        }
        let UiValue::List(v) = self.call(avm1::SCALE9_SCALES, &[]) else {
            return;
        };
        let num = |x: Option<&UiValue>| match x {
            Some(UiValue::Number(n)) => Some(*n),
            _ => None,
        };
        for (i, t) in v.chunks(3).enumerate() {
            let (Some(id), Some(xs), Some(ys)) = (num(t.first()), num(t.get(1)), num(t.get(2)))
            else {
                continue;
            };
            if self.scale9_laid.len() <= i {
                self.scale9_laid.resize(i + 1, (f64::NAN, f64::NAN));
            }
            if self.scale9_laid[i] == (xs, ys) {
                continue;
            }
            self.scale9_laid[i] = (xs, ys);
            let Some(grid) = self.scale9.get(&(id as u16)).copied() else {
                continue;
            };
            for (k, [x, y, a, b]) in gfx::scale9_layout(&grid, xs / 100.0, ys / 100.0)
                .into_iter()
                .enumerate()
            {
                let args = [i as f64, k as f64, x, y, a, b].map(UiValue::Number);
                self.call(avm1::SCALE9_SET, &args);
            }
        }
    }

    /// The movie's engine-image slots, from its rewrite (`RewriteReport::slots`).
    pub fn set_slots(&mut self, slots: BTreeMap<String, gfx::Slot>) {
        self.slots = slots;
    }

    /// The engine's drawing of image `name` (`paperdoll`) is now at `url`
    /// (`img://thedas_paperdoll_3.png`), or there is none.
    pub fn set_engine_image(&mut self, name: &str, url: Option<&str>) {
        match url {
            Some(u) => self.engine_images.insert(name.to_string(), u.to_string()),
            None => self.engine_images.remove(name),
        };
    }

    /// Loads the engine's drawings into the slots showing an older one (or none).
    fn fill_slots(&mut self) {
        if self.slots.is_empty() {
            return;
        }
        let UiValue::List(names) = self.call(avm1::SLOT_NAMES, &[]) else {
            return;
        };
        for (i, n) in names.iter().enumerate() {
            let UiValue::String(name) = n else { continue };
            let (Some(url), Some(slot)) = (self.engine_images.get(name), self.slots.get(name))
            else {
                continue;
            };
            if self.slots_loaded.len() <= i {
                self.slots_loaded.resize(i + 1, String::new());
            }
            if self.slots_loaded[i] == *url {
                continue;
            }
            self.slots_loaded[i] = url.clone();
            let [x, y, _, _] = slot.rect;
            let args = [
                UiValue::Number(i as f64),
                UiValue::String(url.clone()),
                UiValue::Number(x),
                UiValue::Number(y),
            ];
            self.call(avm1::SLOT_LOAD, &args);
        }
    }

    /// Runs what the movies put off past the host's last step: post-load functions, then
    /// the scaling-grid layout and the engine's images.
    fn after_step(&mut self) {
        self.call(avm1::POST_LOAD_RUN, &[]);
        self.layout_scale9();
        self.fill_slots();
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
        self.executor.run();
        self.with_player(|p| p.tick(FloatDuration::from_millis(dt_ms)));
        self.after_step();
        self.executor.run();
    }

    /// Runs exactly one movie frame, independent of time.
    pub fn run_frame(&mut self) {
        self.executor.run();
        self.with_player(|p| p.run_frame());
        self.after_step();
        self.executor.run();
    }

    /// Renders the current frame into the texture.
    pub fn render(&mut self) {
        self.with_player(|p| p.render());
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
        Some(r)
    }

    /// Renders the movie's multiply pass (see [`MultiplyPass`]) and returns a copy of it
    /// (RGBA8, premultiplied), to multiply onto the frame under the movie's own picture.
    /// `None` without a pass.
    pub fn render_multiply(&mut self) -> Option<wgpu::Texture> {
        self.in_multiply_pass(|m| {
            m.render();
            let (texture, device, queue) = m
                .with_player(|p| {
                    let r: &mut dyn std::any::Any = p.renderer_mut();
                    r.downcast_mut::<WgpuRenderBackend<TextureTarget>>()
                        .map(|b| {
                            (
                                b.target().texture.clone(),
                                b.descriptors().device.clone(),
                                b.descriptors().queue.clone(),
                            )
                        })
                })
                .flatten()?;
            let same = m
                .multiply_texture
                .as_ref()
                .is_some_and(|t| t.size() == texture.size() && t.format() == texture.format());
            if !same {
                m.multiply_texture = Some(device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("ui multiply pass"),
                    size: texture.size(),
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: texture.format(),
                    usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                }));
            }
            let copy = m.multiply_texture.clone()?;
            let mut encoder = device.create_command_encoder(&Default::default());
            encoder.copy_texture_to_texture(
                texture.as_image_copy(),
                copy.as_image_copy(),
                texture.size(),
            );
            queue.submit([encoder.finish()]);
            Some(copy)
        })
        .flatten()
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

    /// The texture the movie renders into (RGBA8, premultiplied alpha).
    pub fn texture(&self) -> Option<wgpu::Texture> {
        self.with_player(|p| {
            let r: &mut dyn std::any::Any = p.renderer_mut();
            r.downcast_mut::<WgpuRenderBackend<TextureTarget>>()
                .map(|b| b.target().texture.clone())
        })
        .flatten()
    }

    /// Reads the last rendered frame back to the CPU (RGBA8, premultiplied alpha).
    pub fn capture(&mut self) -> Option<Rgba> {
        self.with_player(|p| {
            let r: &mut dyn std::any::Any = p.renderer_mut();
            r.downcast_mut::<WgpuRenderBackend<TextureTarget>>()
                .and_then(|b| b.capture_frame())
                .map(|img| {
                    let (width, height) = (img.width(), img.height());
                    // Ruffle hands the frame back unmultiplied (it divides by alpha for
                    // export); blending wants it premultiplied, as the GPU texture is.
                    // Blended unmultiplied, a nearly clear pixel's full colour was added:
                    // the minimap lens's orange rim and the headers' smoke glowed in
                    // captures (the window's GPU composite was right).
                    let mut pixels = img.into_raw();
                    premultiply(&mut pixels);
                    Rgba {
                        width,
                        height,
                        pixels,
                    }
                })
        })
        .flatten()
    }

    pub fn mouse_move(&mut self, x: f64, y: f64) {
        self.with_player(|p| p.handle_event(PlayerEvent::MouseMove { x, y }));
    }

    /// The mouse wheel turned `lines` (up positive) with the mouse at (`x`, `y`): the movie's
    /// `onMouseWheel` listeners and the scrollable under the mouse hear it.
    pub fn mouse_wheel(&mut self, x: f64, y: f64, lines: f64) {
        self.with_player(|p| {
            p.handle_event(PlayerEvent::MouseMove { x, y });
            p.handle_event(PlayerEvent::MouseWheel {
                delta: ruffle_core::events::MouseWheelDelta::Lines(lines),
            });
        });
    }

    /// A key pressed and released: the movie's key listeners hear it (`Key.getCode`), and a
    /// focused text field takes the character or the edit.
    pub fn key(&mut self, key: UiKey) {
        let (physical, logical) = key.descriptor();
        let descriptor = || KeyDescriptor {
            physical_key: physical,
            logical_key: logical,
            key_location: KeyLocation::Standard,
        };
        self.with_player(|p| {
            p.handle_event(PlayerEvent::KeyDown { key: descriptor() });
            match key {
                UiKey::Char(c) => {
                    p.handle_event(PlayerEvent::TextInput { codepoint: c });
                }
                _ => {
                    if let Some(code) = key.text_control() {
                        p.handle_event(PlayerEvent::TextControl { code });
                    }
                }
            }
            p.handle_event(PlayerEvent::KeyUp { key: descriptor() });
        });
    }

    pub fn mouse_button(&mut self, x: f64, y: f64, down: bool) {
        let button = MouseButton::Left;
        if down {
            let ev = PlayerEvent::MouseDown {
                x,
                y,
                button,
                index: None,
            };
            self.with_player(|p| p.handle_event(ev));
            self.after_step();
            return;
        }
        // The button under the mouse is released before the movies' mouse-up handlers hide
        // anything (see `avm1::HIDE_LATER`).
        self.set(avm1::HIDE_LATER, UiValue::List(Vec::new()));
        self.with_player(|p| p.handle_event(PlayerEvent::MouseUp { x, y, button }));
        self.call(avm1::HIDE_FLUSH, &[]);
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
    /// (`_global.GUISystem.StartSceneFromGame`). Needs a movie from [`load_movie`].
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
    /// `SetVariable` does. Needs a movie from [`load_movie`].
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
        // its login panel are drawn, not from Ruffle's view matrix (which is not exposed).
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
        self.with_player(|p| {
            let r = p.call_internal_interface(name, args.iter().map(Into::into));
            UiValue::from(&r)
        })
        .unwrap_or(UiValue::Undefined)
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
        self.shared
            .callbacks
            .lock()
            .map(|c| c.clone())
            .unwrap_or_default()
    }

    /// AVM traces and warnings since the last drain.
    pub fn drain_log(&self) -> Vec<String> {
        self.shared
            .log
            .lock()
            .map(|mut c| std::mem::take(&mut *c))
            .unwrap_or_default()
    }

    /// Every URL the movie fetched, with whether a resource served it.
    pub fn fetches(&self) -> Vec<(String, bool)> {
        self.fetches.lock().map(|f| f.clone()).unwrap_or_default()
    }
}

#[cfg(test)]
mod key_tests {
    use super::*;

    #[test]
    fn keys_reach_the_player_as_its_key_and_text_events() {
        assert_eq!(
            UiKey::Char('a').descriptor(),
            (PhysicalKey::KeyA, LogicalKey::Character('a'))
        );
        assert_eq!(
            UiKey::Char('R').descriptor(),
            (PhysicalKey::KeyR, LogicalKey::Character('R'))
        );
        assert_eq!(UiKey::Char('é').descriptor().0, PhysicalKey::Unknown);
        assert_eq!(
            UiKey::Enter.descriptor(),
            (PhysicalKey::Enter, LogicalKey::Named(NamedKey::Enter))
        );
        assert_eq!(UiKey::Enter.text_control(), Some(TextControlCode::Enter));
        assert_eq!(
            UiKey::SelectAll.text_control(),
            Some(TextControlCode::SelectAll)
        );
        assert_eq!(UiKey::Char('x').text_control(), None);
        assert_eq!(UiKey::Escape.text_control(), None);
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
    fn the_save_name_field_lets_spaces_through_and_keeps_its_length() {
        let mut swf =
            b"FWS\x08....\x88pool\0TextInput_mc_init\0restrict\0^ /t\0maxChars\0".to_vec();
        let len = swf.len();
        allow_spaces_in_typed_names(&mut swf);
        assert_eq!(swf.len(), len, "the constant pool keeps its size");
        let s = String::from_utf8_lossy(&swf);
        assert!(s.contains("restrict\0^///\0maxChars"), "{s:?}");
        // Anything else is left alone.
        let mut other = b"FWS\x08restrict\0abc\0".to_vec();
        let before = other.clone();
        allow_spaces_in_typed_names(&mut other);
        assert_eq!(other, before);
    }

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
