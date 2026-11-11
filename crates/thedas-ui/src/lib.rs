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

pub mod atlas;
pub mod avm1;
pub mod binding;
pub mod chains;
pub mod input;
pub mod minimap;
pub mod models;
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
}

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
    Ok(gfx::rewrite(
        &data,
        &mut |e| images.image(&e.file),
        fonts.as_ref(),
        Some(&inject),
    )?)
}

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

    /// Advances the movie by `dt_ms` of wall time and runs pending loads.
    pub fn tick(&mut self, dt_ms: f64) {
        self.executor.run();
        self.with_player(|p| p.tick(FloatDuration::from_millis(dt_ms)));
        self.executor.run();
    }

    /// Runs exactly one movie frame, independent of time.
    pub fn run_frame(&mut self) {
        self.executor.run();
        self.with_player(|p| p.run_frame());
        self.executor.run();
    }

    /// Renders the current frame into the texture.
    pub fn render(&mut self) {
        self.with_player(|p| p.render());
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

    /// Reads the last rendered frame back to the CPU.
    pub fn capture(&mut self) -> Option<Rgba> {
        self.with_player(|p| {
            let r: &mut dyn std::any::Any = p.renderer_mut();
            r.downcast_mut::<WgpuRenderBackend<TextureTarget>>()
                .and_then(|b| b.capture_frame())
                .map(|img| Rgba {
                    width: img.width(),
                    height: img.height(),
                    pixels: img.into_raw(),
                })
        })
        .flatten()
    }

    pub fn mouse_move(&mut self, x: f64, y: f64) {
        self.with_player(|p| p.handle_event(PlayerEvent::MouseMove { x, y }));
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
        let ev = if down {
            PlayerEvent::MouseDown {
                x,
                y,
                button,
                index: None,
            }
        } else {
            PlayerEvent::MouseUp { x, y, button }
        };
        self.with_player(|p| p.handle_event(ev));
    }

    /// The engine side of the movies' `TokenManager`: collects observers registered with
    /// `AddTokenObserverWithID` (batched as `[id, token, flags]` triples in
    /// `TokenManager.AddTokenObserverBatchData`) and removed ones, then pushes each observed
    /// value that changed to `TokenManager.cm_mapCallbacks[id](value)`. `value` answers a
    /// token or returns `None` (the observer is then left alone). Returns the tokens nothing
    /// answered. Provisional, from the movies' bytecode.
    pub fn sync_tokens(&mut self, value: &dyn Fn(&str) -> Option<UiValue>) -> Vec<String> {
        const TM: &str = "_global.TokenManager";
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
                self.observers.retain(|(i, _)| *i != id);
                self.observers.push((id, token.clone()));
            }
        }
        for r in take(self, "RemoveTokenObserverBatchData") {
            if let Some(id) = callback_key(&r) {
                self.observers.retain(|(i, _)| *i != id);
                self.sent.remove(&id);
            }
        }
        let mut missing = Vec::new();
        for (id, token) in self.observers.clone() {
            match value(&token) {
                Some(v) => {
                    if self.sent.get(&id) != Some(&v) {
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
