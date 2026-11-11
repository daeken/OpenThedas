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

/// English talk-table strings from every `*en-us.tlk` in the stack.
pub fn talk_table(vfs: &Vfs) -> crate::protocol::StringLookup {
    let mut map = std::collections::HashMap::new();
    for name in vfs.names().into_iter().filter(|n| n.ends_with("en-us.tlk")) {
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
            let img = images.lock().ok()?.image(name)?;
            return encode_png(&img);
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

/// The movie that shows the game's dialog boxes (`PopupLayer.ShowDialogBoxFromGame`).
pub const POPUP_LAYER: &str = "popuplayer.gfx";
/// The engine value holding how far the quickbar is dragged open (see [`Stage::new`]).
pub const QUICKBAR_EXPAND_VALUE: &str = "Party.QuickbarExpandValue";

/// The context the popup layer pushes while a dialog box is up.
const POPUP_CONTEXT: &str = "GUIPopup";

pub struct Stage {
    pub movies: Vec<UiMovie>,
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
                b
            },
            gpu,
            lookup,
            level,
            made,
            images,
            size,
        }
    }

    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// Serves `rgba` (PNG-encoded) to the movies as image `name` (a `.png`); `None` stops
    /// serving it.
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

    /// Loads a movie (`mainmenu.gfx`) on top of the others and starts it.
    pub fn open(&mut self, name: &str) -> Result<(), UiError> {
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
            let missing = m.sync_tokens(&|t| bridge.value(t));
            bridge.note_unanswered(missing);
            m.tick(dt_ms);
            let messages: Vec<GameMessage> = m
                .drain_calls()
                .into_iter()
                .filter_map(|c| self.bridge.handle(c))
                .collect();
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
        self.bridge.contexts().last().map(String::as_str) == Some(POPUP_CONTEXT)
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

    pub fn mouse_move(&mut self, x: f64, y: f64) {
        for m in self.input_movies() {
            m.mouse_move(x, y);
        }
    }

    pub fn mouse_button(&mut self, x: f64, y: f64, down: bool) {
        for m in self.input_movies() {
            m.mouse_button(x, y, down);
        }
    }

    /// A whole click (press and release) at (`x`, `y`) that the movies see as Scaleform
    /// `button` ([`crate::avm1::MOUSE_RIGHT`], or a button + [`crate::avm1::DOUBLE_CLICK`]
    /// for a double click), delivered as an ordinary click with the button set aside (see
    /// [`crate::avm1::BUTTON_VAR`]).
    pub fn click_as(&mut self, button: f64, x: f64, y: f64) {
        for m in self.input_movies() {
            m.set(crate::avm1::BUTTON_VAR, UiValue::Number(button));
            m.mouse_move(x, y);
            m.mouse_button(x, y, true);
            m.mouse_button(x, y, false);
            m.set(crate::avm1::BUTTON_VAR, UiValue::Undefined);
        }
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
        for m in &mut self.movies {
            m.render();
            let frame = m
                .capture()
                .ok_or_else(|| UiError::Render(format!("capturing {}", m.name)))?;
            blend_premultiplied(base, &frame);
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
