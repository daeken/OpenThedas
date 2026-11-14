//! A set of movies sharing one engine bridge: loading from the game's resources, starting
//! them the way the game does, input, ticking and CPU captures. The application adds the GPU
//! composite on top.

use std::sync::Arc;

use thedas_vfs::Vfs;

use crate::protocol::{FINISH_SCENE, GameBridge, GameMessage, LAYOUT, START_SCENE};
use crate::{Gpu, ImageSource, ResourceLookup, Rgba, UiError, UiMovie, UiValue};

/// Movie sets by name (`menu`, `hud`); anything else is a comma-separated list of movie names.
pub fn movie_names(spec: &str) -> Vec<String> {
    let list: &[&str] = match spec {
        "menu" => &["mainmenu"],
        "hud" => &["portraits", "quickbar", "minimap"],
        _ => &[],
    };
    let names: Vec<String> = if list.is_empty() {
        spec.split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect()
    } else {
        list.iter().map(|s| s.to_string()).collect()
    };
    names
        .into_iter()
        .map(|n| {
            if n.contains('.') {
                n.to_ascii_lowercase()
            } else {
                format!("{}.gfx", n.to_ascii_lowercase())
            }
        })
        .collect()
}

pub fn vfs_lookup(vfs: &Arc<Vfs>) -> ResourceLookup {
    let vfs = vfs.clone();
    Arc::new(move |name: &str| vfs.read(name).ok().map(|x| x.0))
}

/// The game's 2DA tables, for `Get2DAValue` (by M2DA table id, row id and column hash).
pub fn table_lookup(vfs: &Arc<Vfs>) -> crate::protocol::TableLookup {
    use crate::protocol::TableQuery;
    let tables = thedas_sim::m2da::Tables::new(vfs.clone());
    Arc::new(move |q| {
        let value = |c: &thedas_formats::gda::Cell| match c {
            thedas_formats::gda::Cell::Int(v) => Some(UiValue::Number(*v as f64)),
            thedas_formats::gda::Cell::Float(v) => Some(UiValue::Number(*v)),
            thedas_formats::gda::Cell::String(s) => {
                Some(UiValue::String(s.trim_end_matches('\0').to_string()))
            }
            _ => None,
        };
        match q {
            TableQuery::Cell { table, row, column } => {
                value(tables.get(table, "")?.cell_by_hash(column, row)?)
            }
            TableQuery::CellAt {
                table,
                index,
                column,
            } => {
                let t = tables.get(table, "")?;
                value(t.cell_by_hash(column, *t.order.get(index)?)?)
            }
            TableQuery::Rows { table } => {
                Some(UiValue::Number(tables.get(table, "")?.order.len() as f64))
            }
        }
    })
}

/// English talk-table strings from every `*en-us.tlk` in the stack, then Dragon Age II's
/// `*en-us_p.tlk` patch tables over them.
pub fn talk_table(vfs: &Vfs) -> crate::protocol::StringLookup {
    let mut map = std::collections::HashMap::new();
    for name in vfs.talk_tables("en-us") {
        if let Ok((data, _)) = vfs.read(&name) {
            if let Ok(t) = thedas_formats::Tlk::parse(&data) {
                map.extend(
                    t.strings
                        .into_iter()
                        .map(|(k, v)| (k, v.trim_end_matches('\0').to_string())),
                );
            }
        }
    }
    Arc::new(move |id| map.get(&id).cloned())
}

/// Serves the movies' loads: images they ask for by `.dds` name (`img://name.dds`, how
/// Scaleform games hand textures to `loadMovie`) come from the GUI atlases or the resource
/// stack, converted to PNG, which the player can decode; everything else is passed through.
pub fn image_lookup(
    lookup: ResourceLookup,
    images: Arc<std::sync::Mutex<ImageSource>>,
) -> ResourceLookup {
    Arc::new(move |name: &str| {
        if name.ends_with(".dds") || name.ends_with(".tga") {
            if let Some(img) = images.lock().ok()?.image(name) {
                return encode_png(&img);
            }
            // A picture the engine made under a texture's name (map.gfx's `mapmask`), served
            // as made (see [`Stage::set_image`]).
            return lookup(name).filter(|b| b.starts_with(b"\x89PNG"));
        }
        lookup(name)
    })
}

/// RGBA8 to PNG bytes.
pub fn encode_png(img: &Rgba) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(std::io::Cursor::new(&mut out), img.width, img.height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().ok()?;
        w.write_image_data(&img.pixels).ok()?;
    }
    Some(out)
}

/// `base = src + base * (1 - src.a)` for a premultiplied `src` of the same size.
pub fn blend_premultiplied(base: &mut Rgba, src: &Rgba) {
    if (base.width, base.height) != (src.width, src.height) {
        return;
    }
    for (b, s) in base
        .pixels
        .chunks_exact_mut(4)
        .zip(src.pixels.chunks_exact(4))
    {
        let inv = 255 - s[3] as u32;
        for c in 0..3 {
            b[c] = (s[c] as u32 + (b[c] as u32 * inv + 127) / 255).min(255) as u8;
        }
        b[3] = (s[3] as u32 + (b[3] as u32 * inv + 127) / 255).min(255) as u8;
    }
}

/// Multiplies `src` (premultiplied) onto `base`, as Flash's `multiply` blend draws over an
/// opaque picture: each channel becomes base x (src + 1 - src alpha), so a clear pixel
/// leaves the base as it was and an opaque one multiplies it by its colour.
pub fn blend_multiply(base: &mut Rgba, src: &Rgba) {
    if (base.width, base.height) != (src.width, src.height) {
        return;
    }
    for (b, s) in base
        .pixels
        .chunks_exact_mut(4)
        .zip(src.pixels.chunks_exact(4))
    {
        let inv = 255 - u32::from(s[3]);
        for c in 0..3 {
            let f = (u32::from(s[c]) + inv).min(255);
            b[c] = ((u32::from(b[c]) * f + 127) / 255) as u8;
        }
    }
}

/// The path of the movie's focused text field when it is an input field
/// (`Selection.getFocus`, then the field's `type`).
fn focused_input_field(m: &mut UiMovie) -> Option<String> {
    let UiValue::String(path) = m.call("_global.Selection.getFocus", &[]) else {
        return None;
    };
    let kind = m.call(&format!("{path}.type.toString"), &[]);
    (kind == UiValue::String("input".into())).then_some(path)
}

/// A press lets go of the text field's focus first; one on the field takes it back. In the
/// player the focus moves during the press, and the field losing it there cancels a dialog
/// button's press (popuplayer.gfx's push buttons: the first click on Save only moved the
/// focus, the second saved). Provisional: Scaleform's focus handling is not traced; this keeps
/// one click enough.
fn release_text_focus(m: &mut UiMovie) -> Option<String> {
    let path = focused_input_field(m)?;
    m.call("_global.Selection.setFocus", &[UiValue::Null]);
    Some(path)
}

/// The movie that shows the game's dialog boxes (`PopupLayer.ShowDialogBoxFromGame`).
pub const POPUP_LAYER: &str = "popuplayer.gfx";

/// The full-screen screens the game draws on its open book: they set up their contents for it
/// (`SharedLibrary.BookContents.SetupShowContents`) and their paper and stains are placed with
/// the multiply blend, meant to darken the book's pages.
pub const BOOK_SCREENS: &[&str] = &[
    "inventory.gfx",
    "characterrecord.gfx",
    "journal.gfx",
    "map.gfx",
    "tactics.gfx",
    "store.gfx",
    "levelup.gfx",
    "abilities.gfx",
];

/// The book bookback.gfx draws under the book screens (`GUIBookback`, the exe's `BookBack`
/// movie): `BookBack_I3.dds` over a black screen (`BlackBack`, shown by
/// `GUIBookback.ShowBlack`).
pub const BOOK_IMAGE: &str = "BookBack_I3.dds";

/// Puts the book under a book screen's own clips, inside its movie, so that its multiply
/// images blend with the pages as they did over the game's book movie (Ruffle's multiply over
/// a transparent target leaves them white). Both movies have a 1024x768 stage, so the image
/// sits at the stage origin at its own size. Provisional: the black is drawn whenever the
/// book is (the exe's `GUIBookback.ShowBlack` setter, 0x716a00, is not traced to its callers),
/// and the book's opening and closing animations are not played.
fn add_book_backdrop(m: &mut UiMovie) {
    const ROOT: &str = "_root.thedas_book";
    let n = UiValue::Number;
    let s = |v: &str| UiValue::String(v.to_string());
    // Timeline depth 0 (-16384) is below every placed clip.
    m.call(
        "_root.createEmptyMovieClip",
        &[s("thedas_book"), n(-16384.0)],
    );
    m.call(
        &format!("{ROOT}.createEmptyMovieClip"),
        &[s("black"), n(0.0)],
    );
    let black = format!("{ROOT}.black");
    m.call(&format!("{black}.beginFill"), &[n(0.0), n(100.0)]);
    m.call(&format!("{black}.moveTo"), &[n(-4096.0), n(-4096.0)]);
    for (x, y) in [(5120.0, -4096.0), (5120.0, 5120.0), (-4096.0, 5120.0)] {
        m.call(&format!("{black}.lineTo"), &[n(x), n(y)]);
    }
    m.call(&format!("{black}.endFill"), &[]);
    m.call(
        &format!("{ROOT}.createEmptyMovieClip"),
        &[s("page"), n(1.0)],
    );
    m.call(
        &format!("{ROOT}.page.loadMovie"),
        &[s(&format!("img://{BOOK_IMAGE}"))],
    );
}
/// The quickbar's movie.
const QUICKBAR: &str = "quickbar.gfx";
/// How long (the movie's time) quickbar.gfx is given to lay itself out, its width put back
/// whenever it changes it.
const QUICKBAR_SETTLE_MS: f64 = 500.0;

/// The engine value holding how far the quickbar is dragged open (see [`Stage::new`]).
pub const QUICKBAR_EXPAND_VALUE: &str = "Party.QuickbarExpandValue";

/// Where the mouse is taken to be before it first moves: nowhere near any clip.
const OFF_STAGE: (f64, f64) = (-10_000.0, -10_000.0);

/// The context the popup layer pushes while a dialog box is up.
const POPUP_CONTEXT: &str = "GUIPopup";

/// Time each movie's tick takes, logged every few hundred ticks when `THEDAS_UI_PROFILE` is
/// set: its token sync (the engine's values pushed in) and its player tick (scripts, frames
/// and Ruffle's mouse picking).
mod profile {
    use std::collections::BTreeMap;
    use std::sync::Mutex;
    use std::time::Duration;

    /// Ticks between reports.
    const EVERY: u32 = 300;

    /// Each movie's summed sync and tick time.
    type ByMovie = BTreeMap<String, (Duration, Duration)>;

    /// The ticks counted and the times summed since the last report.
    static TIMES: Mutex<Option<(u32, ByMovie)>> = Mutex::new(None);

    fn on() -> bool {
        static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *ON.get_or_init(|| std::env::var_os("THEDAS_UI_PROFILE").is_some())
    }

    pub fn note(movie: &str, sync: Duration, tick: Duration) {
        if !on() {
            return;
        }
        let Ok(mut t) = TIMES.lock() else { return };
        let (_, by) = t.get_or_insert_with(Default::default);
        let e = by.entry(movie.to_string()).or_default();
        e.0 += sync;
        e.1 += tick;
    }

    /// One stage tick done: every [`EVERY`], the averages are logged and start again.
    pub fn tick_done() {
        if !on() {
            return;
        }
        let Ok(mut t) = TIMES.lock() else { return };
        let (n, by) = t.get_or_insert_with(Default::default);
        *n += 1;
        if *n < EVERY {
            return;
        }
        let per = |d: Duration| d.as_secs_f64() * 1000.0 / f64::from(*n);
        let mut rows: Vec<_> = by
            .iter()
            .map(|(m, (s, k))| (per(*s) + per(*k), m, per(*s), per(*k)))
            .collect();
        rows.sort_by(|a, b| b.0.total_cmp(&a.0));
        let total: f64 = rows.iter().map(|r| r.0).sum();
        let line: Vec<String> = rows
            .iter()
            .map(|(_, m, s, k)| format!("{m} {s:.2}+{k:.2}"))
            .collect();
        log::info!(
            "ui profile: {total:.2} ms a tick (sync+tick): {}",
            line.join(", ")
        );
        *t = None;
    }
}

pub struct Stage {
    pub movies: Vec<UiMovie>,
    /// Where the mouse is (stage pixels), which a movie is told as it opens: a player takes
    /// the mouse to be at its origin until it moves, and a clip there (the portraits'
    /// level-up mark, shown for its first frames) took that for a roll-over and left its
    /// tooltip up. Far off the stage until the mouse first moves.
    mouse: (f64, f64),
    /// The dialog box's text field a press let go of (see `release_text_focus`), given
    /// back when the click leaves the box up with nothing focused.
    dialog_focus: Option<String>,
    /// The quickbar's width as the engine held it when quickbar.gfx opened, put back while
    /// the movie settles (see [`QUICKBAR_EXPAND_VALUE`]).
    quickbar_width: Option<(UiValue, f64)>,
    /// Recent ActionScript traces and warnings, newest last (at most 500).
    pub log: Vec<String>,
    pub bridge: GameBridge,
    gpu: Gpu,
    lookup: ResourceLookup,
    /// The current level's resources (its minimap picture), searched after the stage's own.
    level: Arc<std::sync::Mutex<Option<Arc<Vfs>>>>,
    /// Images the engine makes (party portraits), by name, served before anything else.
    made: Arc<std::sync::Mutex<std::collections::HashMap<String, Vec<u8>>>>,
    images: Arc<std::sync::Mutex<ImageSource>>,
    size: (u32, u32),
    /// The engine's current drawing of each engine image (see `UiMovie::set_engine_image`).
    engine_images: std::collections::BTreeMap<String, String>,
}

impl Stage {
    pub fn new(gpu: Gpu, vfs: &Arc<Vfs>, size: (u32, u32)) -> Self {
        let level: Arc<std::sync::Mutex<Option<Arc<Vfs>>>> = Default::default();
        let made: Arc<std::sync::Mutex<std::collections::HashMap<String, Vec<u8>>>> =
            Default::default();
        let lookup = {
            let (global, level, made) = (vfs_lookup(vfs), level.clone(), made.clone());
            let lookup: ResourceLookup = Arc::new(move |name: &str| {
                if let Some(bytes) = made.lock().ok().and_then(|m| m.get(name).cloned()) {
                    return Some(bytes);
                }
                global(name).or_else(|| {
                    let l = level.lock().ok()?.clone()?;
                    l.read(name).ok().map(|x| x.0)
                })
            });
            lookup
        };
        let atlases = vfs
            .names()
            .into_iter()
            .filter(|n| n.starts_with("atl_") && n.ends_with("_dat.xml"));
        let images = ImageSource::new(lookup.clone(), atlases);
        log::info!("ui: {} atlas images indexed", images.len());
        let images = Arc::new(std::sync::Mutex::new(images));
        Self {
            movies: Vec::new(),
            mouse: OFF_STAGE,
            dialog_focus: None,
            quickbar_width: None,
            log: Vec::new(),
            bridge: {
                let b = GameBridge::new(talk_table(vfs));
                b.set_table_lookup(Some(table_lookup(vfs)));
                // How far the player has dragged the quickbar open (quickbar.gfx reads it in
                // `UpdateLayout` and writes it back as the handle moves): the engine keeps
                // it, so it is seeded once here and left to the movie, not bound every frame
                // (a per-frame 0 raced the movie's own value and the bar's width changed from
                // run to run). Provisional: closed (0) to start, as no profile is read.
                b.set_value(QUICKBAR_EXPAND_VALUE, UiValue::Number(0.0));
                // The area map's zoom (0 the whole map to 1 the closest) and whether its
                // legend shows: the player's, kept by the engine (map.gfx writes them back as
                // they change), seeded once likewise. Unset, the zoom read as NaN and the map
                // was neither scaled nor centred. Provisional: the whole map and the legend
                // shown to start.
                b.set_value("Player.MapZoom", UiValue::Number(0.0));
                b.set_value("Player.LegendVisible", UiValue::Number(1.0));
                b
            },
            gpu,
            lookup,
            level,
            made,
            images,
            size,
            engine_images: Default::default(),
        }
    }

    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// Serves `rgba` (PNG-encoded) to the movies as image `name` (a `.png`); `None` stops
    /// serving it.
    /// The engine's drawing of image `name` (see `crate::ENGINE_IMAGES`) is now the made
    /// image `file` (see [`Stage::set_image`]): the movies' slots for it load it.
    pub fn set_engine_image(&mut self, name: &str, file: Option<&str>) {
        let url = file.map(|f| format!("img://{f}"));
        match &url {
            Some(u) => self.engine_images.insert(name.to_string(), u.clone()),
            None => self.engine_images.remove(name),
        };
        for m in &mut self.movies {
            m.set_engine_image(name, url.as_deref());
        }
    }

    /// A picture the engine holds as a PNG already (a save's screenshot), served to the
    /// movies under `name` as [`Stage::set_image`]'s are; `None` takes it away.
    pub fn set_image_png(&self, name: &str, png: Option<Vec<u8>>) {
        if let Ok(mut m) = self.made.lock() {
            match png {
                Some(b) => m.insert(name.to_string(), b),
                None => m.remove(name),
            };
        }
    }

    pub fn set_image(&self, name: &str, rgba: Option<&Rgba>) {
        let bytes = rgba.and_then(encode_png);
        if let Ok(mut m) = self.made.lock() {
            match bytes {
                Some(b) => m.insert(name.to_string(), b),
                None => m.remove(name),
            };
        }
    }

    /// The level's resources the movies may load from too (the minimap picture), or none.
    pub fn set_level_resources(&self, vfs: Option<Arc<Vfs>>) {
        if let Ok(mut l) = self.level.lock() {
            *l = vfs;
        }
    }

    /// A movie on Ruffle: rewritten to a plain SWF with its images inlined.
    fn open_ruffle(&mut self, name: &str) -> Result<UiMovie, UiError> {
        let (swf, rep) = {
            let mut images = self
                .images
                .lock()
                .map_err(|_| UiError::Render("image source poisoned".into()))?;
            crate::load_movie(name, &self.lookup, &mut images)?
        };
        log::info!(
            "ui: {name}: {} images inlined, {} missing, Scaleform tags dropped {:?}",
            rep.images_inlined.len(),
            rep.images_missing.len(),
            rep.dropped
        );
        for m in &rep.images_missing {
            log::warn!("ui: {name}: image {m} not found");
        }
        let mut m = UiMovie::new(
            &self.gpu,
            name,
            &swf,
            self.size,
            image_lookup(self.lookup.clone(), self.images.clone()),
            Some(self.bridge.responder()),
        )?;
        m.set_scale9(rep.scale9.clone());
        m.set_slots(rep.slots.clone());
        Ok(m)
    }

    /// Loads a movie (`mainmenu.gfx`) on top of the others and starts it, on Ruffle or on
    /// our own player (see [`crate::own::wanted`]).
    pub fn open(&mut self, name: &str) -> Result<(), UiError> {
        // A popup layer loaded anew has no dialog box up, whatever the one before it left
        // on the context stack.
        if name == POPUP_LAYER {
            self.bridge.drop_context(POPUP_CONTEXT);
        }
        if name == QUICKBAR {
            self.quickbar_width = self
                .bridge
                .value(QUICKBAR_EXPAND_VALUE)
                .map(|v| (v, QUICKBAR_SETTLE_MS));
        }
        let mut m = if crate::own::wanted(name) {
            let gfx = (self.lookup)(name).ok_or_else(|| UiError::NotFound(name.to_string()))?;
            log::info!("ui: {name}: on our own player");
            UiMovie::new_own(
                &self.gpu,
                name,
                &gfx,
                self.size,
                self.lookup.clone(),
                self.images.clone(),
                Some(self.bridge.responder()),
            )?
        } else {
            self.open_ruffle(name)?
        };
        let swf = (self.lookup)(name).unwrap_or_default();
        if self.mouse != OFF_STAGE {
            m.mouse_move(self.mouse.0, self.mouse.1);
        }
        for (name, url) in &self.engine_images {
            m.set_engine_image(name, Some(url));
        }
        // Run until the injected frame-one action has registered its callback (class
        // definitions and InitMovie run first), then start the movie as the game does.
        for _ in 0..5 {
            if !m.callbacks().is_empty() {
                break;
            }
            m.run_frame();
        }
        // StartSceneFromFlash records the authored stage size; when the game starts the movie
        // it sets the size itself, and layout depends on it.
        if let Some((w, h)) = crate::gfx::Movie::parse(&swf)
            .ok()
            .and_then(|x| x.stage_size())
        {
            m.set(
                "_global.GUISystem.OriginalStageWidth",
                UiValue::Number(w as f64),
            );
            m.set(
                "_global.GUISystem.OriginalStageHeight",
                UiValue::Number(h as f64),
            );
            // The engine also sets them on `_root` when it starts a movie (the executable
            // pushes `_root.OriginalStageWidth` at 0x51a289, beside `_root.Filename`), as
            // StartSceneFromFlash does; the HUD's layout and popuplayer.gfx's dialog boxes
            // read those.
            m.set("_root.OriginalStageWidth", UiValue::Number(w as f64));
            m.set("_root.OriginalStageHeight", UiValue::Number(h as f64));
        }
        // Scaleform's mouse button constants, which the movies test their clicks against.
        for (k, b) in [
            ("LEFT", crate::avm1::MOUSE_LEFT),
            ("RIGHT", crate::avm1::MOUSE_RIGHT),
            ("MIDDLE", crate::avm1::MOUSE_MIDDLE),
        ] {
            m.set(&format!("_global.Mouse.{k}"), UiValue::Number(b));
        }
        // The engine names each movie's file in `_root.Filename` (the executable pushes the
        // name at 0x51a242, in the routine that sets the stage size); the popup layer's
        // buttons name the movie to call back with it (`GUI.InvokeCallback <file>
        // _root.DialogBoxCallbackN`). Provisional: the value's form (the file name with its
        // extension) is inferred from the movies.
        m.set("_root.Filename", UiValue::String(name.to_string()));
        m.call(START_SCENE, &[]);
        m.call(crate::avm1::INSTALL, &[]);
        // Movies anchor their elements to the screen when the engine asks them to lay out.
        m.call(LAYOUT, &[]);
        if BOOK_SCREENS.iter().any(|b| name.eq_ignore_ascii_case(b)) {
            add_book_backdrop(&mut m);
        }
        self.movies.push(m);
        Ok(())
    }

    /// Opens several movies, logging the ones that fail.
    pub fn open_all(&mut self, names: &[String]) {
        for n in names {
            if let Err(e) = self.open(n) {
                log::warn!("ui: {n}: {e}");
            }
        }
    }

    pub fn close(&mut self, name: &str) -> bool {
        let before = self.movies.len();
        self.movies.retain(|m| m.name != name);
        before != self.movies.len()
    }

    /// Advances every movie; returns the messages meant for the rest of the engine.
    ///
    /// A movie that asks for its full-screen interface to be hidden (`GUI.HideFullScreenGUI`,
    /// e.g. the world map's close button) is finished as the game finishes it, with
    /// `GUISystem.FinishSceneFromGame` (the movie's own closing messages, such as
    /// `WorldMap.WorldMapClosed`, are returned too), then closed. Provisional: inferred from
    /// the movies, which send the message and close themselves only in that function.
    pub fn tick(&mut self, dt_ms: f64) -> Vec<GameMessage> {
        let mut out = Vec::new();
        let bridge = self.bridge.clone();
        let mut finished = Vec::new();
        for m in &mut self.movies {
            let started = std::time::Instant::now();
            let missing = m.sync_tokens(&|t| bridge.value(t));
            bridge.note_unanswered(missing);
            let synced = started.elapsed();
            m.tick(dt_ms);
            profile::note(&m.name, synced, started.elapsed() - synced);
            // quickbar.gfx's first frame places its expansion handle before the handle's
            // own init (Ruffle runs the clips' init in another order than Scaleform): the
            // handle sits at its authored place, the bar's full width, and the movie writes
            // that back as the player's choice. While the movie settles, the engine's value
            // is put back and the bar laid out again whenever the movie has changed it. The
            // window is the movie's own time: a tick does not always play a frame (a short
            // one plays none), so counting ticks could end before that first frame, and the
            // bar then stayed at its full width with six slots never filled in ("00" over
            // "??"). Provisional: inferred from the movie's code
            // (`UpdateExpansionMaskPosition` before `ExpansionHandle_mc_init`).
            if m.name == QUICKBAR {
                if let Some((width, left)) = self.quickbar_width.take() {
                    if self.bridge.value(QUICKBAR_EXPAND_VALUE).as_ref() != Some(&width) {
                        self.bridge.set_value(QUICKBAR_EXPAND_VALUE, width.clone());
                        m.call(
                            "_level0.QuickbarAndEffectBar_mc.Quickbar_mc.ExpandQuickbarFromGameData",
                            std::slice::from_ref(&width),
                        );
                    }
                    if left > dt_ms {
                        self.quickbar_width = Some((width, left - dt_ms));
                    }
                }
            }
            let messages: Vec<GameMessage> = m
                .drain_calls()
                .into_iter()
                .filter_map(|c| self.bridge.handle(c))
                .collect();
            // A screen asking for its ability lists again (it does after each point spent) is
            // answered with every list entry pushed again, as the game rebuilds the lists:
            // an entry's value (the ability) is unchanged, but its state (learned, available)
            // is not, and the screen reloads an entry only when it hears it.
            if messages.iter().any(|g| {
                matches!(g, GameMessage::Send(n, _) | GameMessage::Handled(n, _)
                    if n == "GameModeController.FindAbilityChains")
            }) {
                m.resend_observers(|t| t.contains("Chains."));
            }
            if messages.iter().any(is_hide_request) {
                m.call(FINISH_SCENE, &[]);
                out.extend(messages);
                out.extend(
                    m.drain_calls()
                        .into_iter()
                        .filter_map(|c| self.bridge.handle(c)),
                );
                finished.push(m.name.clone());
            } else {
                out.extend(messages);
            }
            for line in m.drain_log() {
                log::debug!("ui: {}: {line}", m.name);
                self.log.push(format!("{}: {line}", m.name));
            }
            if self.log.len() > 500 {
                self.log.drain(..self.log.len() - 500);
            }
        }
        self.movies.retain(|m| !finished.contains(&m.name));
        profile::tick_done();
        out
    }

    pub fn resize(&mut self, size: (u32, u32)) {
        self.size = size;
        for m in &mut self.movies {
            m.resize(size);
            m.call(LAYOUT, &[]);
        }
    }

    /// Calls a function in the movie named `movie` (`None` when it is not open).
    pub fn call_in(&mut self, movie: &str, path: &str, args: &[UiValue]) -> Option<UiValue> {
        let m = self.movies.iter_mut().find(|m| m.name == movie)?;
        Some(m.call(path, args))
    }

    /// Whether one of the popup layer's dialog boxes is up.
    pub fn dialog_up(&self) -> bool {
        // Anywhere on the stack: a movie that starts after the box showed (the HUD's, as a
        // game loads) pushes its own context over the popup's, and the box still has the
        // input. The popup layer pops its context when the box closes; a popup layer loaded
        // anew starts without one (see `Stage::open`).
        self.bridge.contexts().iter().any(|c| c == POPUP_CONTEXT)
            && self.movies.iter().any(|m| m.name == POPUP_LAYER)
    }

    /// The movies that take the mouse: all of them, or only the popup layer while one of its
    /// dialogs is up (it pushes the `GUIPopup` context then and pops it when the dialog
    /// closes). Provisional: the game's context system decides which input reaches which
    /// movie; this follows only the popup's context.
    fn input_movies(&mut self) -> impl Iterator<Item = &mut UiMovie> {
        let popup = self.dialog_up();
        self.movies
            .iter_mut()
            .filter(move |m| !popup || m.name == POPUP_LAYER)
    }

    /// What a pad's focus can land on: the clips taking a click in the movies that take the
    /// mouse, in their drawing order.
    pub fn focus_targets(&mut self) -> Vec<crate::FocusTarget> {
        self.input_movies()
            .flat_map(|m| m.focus_targets())
            .collect()
    }

    /// The mouse left the window (see `UiMovie::mouse_leave`).
    pub fn mouse_leave(&mut self) {
        self.mouse = OFF_STAGE;
        for m in &mut self.movies {
            m.mouse_leave();
        }
    }

    pub fn mouse_move(&mut self, x: f64, y: f64) {
        if self.mouse == OFF_STAGE {
            log::debug!("ui: the mouse is in the window at {x:.0},{y:.0}");
        }
        self.mouse = (x, y);
        for m in self.input_movies() {
            m.mouse_move(x, y);
        }
    }

    /// The mouse wheel turned `lines` (up positive) at (`x`, `y`).
    pub fn mouse_wheel(&mut self, x: f64, y: f64, lines: f64) {
        for m in self.input_movies() {
            m.mouse_wheel(x, y, lines);
        }
    }

    pub fn mouse_button(&mut self, x: f64, y: f64, down: bool) {
        let mut released = None;
        for m in self.input_movies() {
            if down {
                released = released.or(release_text_focus(m));
            }
            m.mouse_button(x, y, down);
        }
        self.after_click(released, !down);
    }

    /// A dialog box with a text field (a script's name prompt, the engine's own text box)
    /// gives the field the keyboard focus when nothing has it, so typing reaches it at once.
    /// Returns whether a field has the focus. Provisional: when Scaleform focused the field
    /// is not traced; popuplayer.gfx sometimes focuses it itself.
    pub fn focus_dialog_field(&mut self) -> bool {
        if !self.dialog_up() {
            return false;
        }
        let Some(m) = self.movies.iter_mut().find(|m| m.name == POPUP_LAYER) else {
            return false;
        };
        if focused_input_field(m).is_some() {
            return true;
        }
        let UiValue::String(tree) = m.call("_global.thedas_tree_list", &[]) else {
            return false;
        };
        let field = tree
            .split(';')
            .filter_map(|e| e.split('|').next())
            .filter(|p| p.ends_with(".TextInput_mc"))
            .map(|p| format!("{p}.TextField"))
            .find(|f| {
                m.call(&format!("{f}.type.toString"), &[]) == UiValue::String("input".into())
            });
        match field {
            Some(f) => {
                m.call("_global.Selection.setFocus", &[UiValue::String(f)]);
                focused_input_field(m).is_some()
            }
            None => false,
        }
    }

    /// A dialog box's text field keeps the focus through a click that leaves the box up
    /// and gives it to nothing else (a click beside the field, on the box or off it): the
    /// field a press let go of (`released`) is focused again once the click is done
    /// (`done`), so typing goes on reaching it. Provisional: Scaleform's focus handling is
    /// not traced; a modal box with a text field keeps it here.
    fn after_click(&mut self, released: Option<String>, done: bool) {
        if self.dialog_up() {
            if let Some(p) = released {
                self.dialog_focus = Some(p);
            }
        } else {
            self.dialog_focus = None;
        }
        if !done {
            return;
        }
        let Some(path) = self.dialog_focus.take() else {
            return;
        };
        if !self.dialog_up() {
            return;
        }
        if let Some(m) = self.movies.iter_mut().find(|m| m.name == POPUP_LAYER) {
            if focused_input_field(m).is_none() {
                // Focusing selects the whole text, which the next key would replace: the
                // caret goes back to its end.
                m.call(
                    "_global.Selection.setFocus",
                    &[UiValue::String(path.clone())],
                );
                let len = match m.call(&format!("{path}.length.toString"), &[]) {
                    UiValue::String(n) => n.parse::<f64>().unwrap_or(0.0),
                    _ => 0.0,
                };
                m.call(
                    "_global.Selection.setSelection",
                    &[UiValue::Number(len), UiValue::Number(len)],
                );
            }
        }
    }

    /// The movie whose focused text field takes typing (an input field: a save's name in a
    /// dialog box, the hero's name at character creation), if any. Keys go there instead of
    /// to the game while it has one.
    pub fn text_focus(&mut self) -> Option<String> {
        for m in self.input_movies() {
            if focused_input_field(m).is_some() {
                return Some(m.name.clone());
            }
        }
        None
    }

    /// A key for the focused text field (see [`Stage::text_focus`]); false when no field
    /// has focus (the key is the game's).
    pub fn key(&mut self, key: crate::UiKey) -> bool {
        for m in self.input_movies() {
            if focused_input_field(m).is_some() {
                m.key(key);
                return true;
            }
        }
        false
    }

    /// Whether a text field has the keyboard focus.
    pub fn text_focused(&mut self) -> bool {
        self.input_movies()
            .any(|m| focused_input_field(m).is_some())
    }

    /// Types each character of `text` into the focused text field; false when none has
    /// focus.
    pub fn type_text(&mut self, text: &str) -> bool {
        text.chars().all(|c| self.key(crate::UiKey::Char(c)))
    }

    /// A whole click (press and release) at (`x`, `y`) that the movies see as Scaleform
    /// `button` ([`crate::avm1::MOUSE_RIGHT`], or a button + [`crate::avm1::DOUBLE_CLICK`]
    /// for a double click), delivered as an ordinary click with the button set aside (see
    /// [`crate::avm1::BUTTON_VAR`]).
    pub fn click_as(&mut self, button: f64, x: f64, y: f64) {
        let mut released = None;
        for m in self.input_movies() {
            m.set(crate::avm1::BUTTON_VAR, UiValue::Number(button));
            m.mouse_move(x, y);
            released = released.or(release_text_focus(m));
            m.mouse_button(x, y, true);
            m.mouse_button(x, y, false);
            m.set(crate::avm1::BUTTON_VAR, UiValue::Undefined);
        }
        self.after_click(released, true);
    }

    /// A press (`down`) or release at (`x`, `y`) that the movies see as Scaleform `button`
    /// ([`crate::avm1::MOUSE_RIGHT`]), delivered as the left button with the button set
    /// aside (see [`crate::avm1::BUTTON_VAR`]): a right press opens an item's radial menu,
    /// and its release picks the action under the mouse.
    pub fn button_as(&mut self, button: f64, x: f64, y: f64, down: bool) {
        let mut released = None;
        for m in self.input_movies() {
            m.set(crate::avm1::BUTTON_VAR, UiValue::Number(button));
            m.mouse_move(x, y);
            if down {
                released = released.or(release_text_focus(m));
            }
            m.mouse_button(x, y, down);
            m.set(crate::avm1::BUTTON_VAR, UiValue::Undefined);
        }
        self.after_click(released, !down);
    }

    /// Loads that failed, by movie.
    pub fn failed_loads(&self) -> Vec<(String, String)> {
        self.movies
            .iter()
            .flat_map(|m| {
                m.fetches()
                    .into_iter()
                    .filter(|(_, ok)| !ok)
                    .map(|(u, _)| (m.name.clone(), u))
            })
            .collect()
    }

    /// Renders every movie and blends the frames over `base` (straight RGBA) on the CPU.
    pub fn capture_over(&mut self, base: &mut Rgba) -> Result<(), UiError> {
        // The movies' multiplied clips first, onto the picture under the whole interface
        // (see `UiMovie::render_multiply`).
        for m in self.movies.iter_mut().filter(|m| !m.multiplies_over()) {
            if let Some(pass) = m.capture_multiply() {
                blend_multiply(base, &pass);
            }
        }
        for m in &mut self.movies {
            m.render();
            let frame = m
                .capture()
                .ok_or_else(|| UiError::Render(format!("capturing {}", m.name)))?;
            blend_premultiplied(base, &frame);
            // Clips multiplied onto the movie's own art, and those above them.
            if m.multiplies_over() {
                if let Some(pass) = m.capture_multiply() {
                    blend_multiply(base, &pass);
                }
                if let Some(above) = m.capture_above() {
                    blend_premultiplied(base, &above);
                }
            }
        }
        Ok(())
    }
}

/// `GUI.HideFullScreenGUI`: the movie asks the game to take its screen away.
fn is_hide_request(m: &GameMessage) -> bool {
    matches!(m, GameMessage::Send(n, _) if n == "GUI.HideFullScreenGUI")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multiply_darkens_by_the_colour_and_leaves_clear_pixels_alone() {
        let mut base = Rgba {
            width: 3,
            height: 1,
            pixels: vec![200, 100, 50, 255, 200, 100, 50, 255, 200, 100, 50, 255],
        };
        // Opaque half grey; clear; half-covered black (premultiplied).
        let src = Rgba {
            width: 3,
            height: 1,
            pixels: vec![128, 128, 128, 255, 0, 0, 0, 0, 0, 0, 0, 128],
        };
        blend_multiply(&mut base, &src);
        assert_eq!(&base.pixels[0..4], &[100, 50, 25, 255]);
        assert_eq!(&base.pixels[4..8], &[200, 100, 50, 255]);
        assert_eq!(&base.pixels[8..12], &[100, 50, 25, 255]);
    }

    #[test]
    fn names_movies() {
        assert_eq!(movie_names("menu"), vec!["mainmenu.gfx"]);
        assert_eq!(
            movie_names("Quickbar, x.swf,"),
            vec!["quickbar.gfx", "x.swf"]
        );
        assert_eq!(movie_names("hud").len(), 3);
    }

    #[test]
    fn images_become_png_and_other_loads_pass_through() {
        let lookup: ResourceLookup = Arc::new(|n: &str| (n == "lib.gfx").then(|| b"GFX".to_vec()));
        let images = Arc::new(std::sync::Mutex::new(ImageSource::new(
            lookup.clone(),
            Vec::<String>::new(),
        )));
        let l = image_lookup(lookup, images);
        assert_eq!(l("lib.gfx"), Some(b"GFX".to_vec()));
        assert_eq!(l("missing.dds"), None);
        let png = encode_png(&Rgba {
            width: 1,
            height: 1,
            pixels: vec![1, 2, 3, 4],
        })
        .unwrap();
        assert_eq!(&png[1..4], b"PNG");
    }

    #[test]
    fn blends_premultiplied() {
        let mut base = Rgba {
            width: 1,
            height: 1,
            pixels: vec![200, 100, 0, 255],
        };
        let src = Rgba {
            width: 1,
            height: 1,
            pixels: vec![50, 0, 0, 128],
        };
        blend_premultiplied(&mut base, &src);
        assert_eq!(base.pixels, vec![50 + 100, 50, 0, 255]);
    }
}
