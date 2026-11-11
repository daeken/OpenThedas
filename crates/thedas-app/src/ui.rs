//! The Scaleform interface layer: movies played by `thedas-ui` into offscreen textures,
//! composited over the scene with premultiplied alpha.

use std::sync::{Arc, Mutex};

use anyhow::Result;
use thedas_render::GpuContext;
use thedas_sim::Sim;
use thedas_ui::protocol::GameMessage;
use thedas_ui::stage::Stage;
use thedas_ui::{Gpu, Rgba};
use thedas_vfs::Vfs;

use crate::screens::{Screen, Screens};

/// A movie pixel at least this opaque (of 255) takes a click from the world.
pub const COVER_ALPHA: u8 = 16;

const COMPOSITE_WGSL: &str = r#"
@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;

struct Out {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs(@builtin(vertex_index) i: u32) -> Out {
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var o: Out;
    o.pos = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.0, 1.0);
    o.uv = uv;
    return o;
}

@fragment
fn fs(i: Out) -> @location(0) vec4<f32> {
    return textureSample(tex, samp, i.uv);
}
"#;

struct Composite {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
}

impl Composite {
    fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ui composite"),
            source: wgpu::ShaderSource::Wgsl(COMPOSITE_WGSL.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ui composite"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ui composite"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("ui composite"),
            layout: Some(&pl),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self {
            pipeline,
            layout,
            sampler,
        }
    }
}

pub struct UiLayer {
    pub stage: Stage,
    composite: Option<Composite>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    /// Messages the simulation did not take, already logged.
    ignored: std::collections::HashSet<String>,
    /// The screens' own choices (the inventory's category tab), shared with the message
    /// handler.
    shown: Arc<Mutex<thedas_ui::binding::Presentation>>,
    /// The conversation line on screen and how far it has played.
    line: Option<Line>,
    /// The level whose resources and minimap the interface has.
    level: Option<String>,
    /// Frames since the minimap's creatures were last sent.
    minimap_frame: u32,
    portraits: PortraitClock,
    /// Where the player's games are saved (none: the save and load screens list nothing).
    saves: Option<crate::saves::SaveStore>,
}

/// When portraits are drawn: once, then again only when what they show changes (see
/// [`portrait_key`]), as the original's are updated by message (`UpdatePortraitMessage`,
/// built at 0x607b70, vtable 0xb02298, handled by `GUIPaperdoll`) rather than continuously.
/// A member left out of a drawing (its model not drawn yet) is retried every
/// [`PORTRAIT_RETRY_MS`]. Each drawing gets new image names, so the movies load it afresh.
/// Provisional: which changes the original posts the message for is not traced.
#[derive(Default)]
struct PortraitClock {
    key: Vec<(u32, u64)>,
    missing: bool,
    since_ms: f64,
    drawn: u32,
}

impl PortraitClock {
    /// Whether they are due: what they show (`key`) changed, or a member was left out of the
    /// last drawing [`PORTRAIT_RETRY_MS`] ago. `dt_ms` is the time since the last call.
    fn due(&mut self, key: Vec<(u32, u64)>, dt_ms: f64) -> bool {
        self.since_ms += dt_ms;
        if key != self.key || (self.missing && self.since_ms >= PORTRAIT_RETRY_MS) {
            self.key = key;
            self.since_ms = 0.0;
            return true;
        }
        false
    }

    /// Notes which members the last drawing has.
    fn drew(&mut self, ids: &[u32]) {
        self.missing = self.key.iter().any(|(id, _)| !ids.contains(id));
    }

    /// The image name of member `id`'s portrait in drawing `drawn`.
    fn name(id: u32, drawn: u32) -> String {
        format!("thedas_portrait_{id}_{drawn}.png")
    }
}

/// How soon a member missing from the last portraits is tried again (milliseconds).
const PORTRAIT_RETRY_MS: f64 = 1000.0;

/// What the portraits show, by member (the party, and the followers picked in an open party
/// picker: see [`crate::live::portrait_subjects`]): who, in which slot, with which
/// appearance, head morph and equipment. They are drawn again when it changes.
pub fn portrait_key(sim: &thedas_sim::Sim) -> Vec<(u32, u64)> {
    use std::hash::{Hash, Hasher};
    crate::live::portrait_subjects(sim)
        .into_iter()
        .map(|id| {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            if let Some(o) = sim.world.get(id) {
                o.appearance.hash(&mut h);
                o.head_morph.hash(&mut h);
                o.equipment.hash(&mut h);
                o.area.hash(&mut h);
            }
            (id, h.finish())
        })
        .collect()
}
/// Portraits' size (pixels, square).
pub const PORTRAIT_SIZE: u32 = 128;

/// minimap.gfx's creature flags (`SharedLibrary.CreatureMapIcon.FLAG_*`).
const MAP_HOSTILE: u32 = 1;
const MAP_IN_PARTY: u32 = 2;
const MAP_PLAYER: u32 = 8;
const MAP_WITHIN_RADIUS: u32 = 16;
/// The minimap's creatures are sent every this many frames.
const MINIMAP_EVERY: u32 = 3;

/// A conversation line: the dialog, the line's text and its replies' nodes.
type LineKey = (String, String, Vec<usize>);

/// The conversation line on screen: while it plays its replies are held back
/// (`Conversation.ResponsesReady` 0, which conversation.gfx watches to show them); once it
/// has played they show, or, with nothing to choose, the conversation goes on.
struct Line {
    key: LineKey,
    /// Milliseconds it has been up.
    t: f64,
    /// Nothing to choose: it continues by itself once played.
    waits: bool,
    continued: bool,
}

impl Line {
    fn played(&self) -> bool {
        self.t >= line_ms(&self.key.1)
    }
}

/// How long a line plays before the conversation goes on by itself (milliseconds), standing
/// in for its voice-over: a reading time for its text, a moment for a line without text (a
/// node that only moves the conversation on: across the game's conversations none of them
/// carries staging of its own). Provisional.
pub fn line_ms(text: &str) -> f64 {
    let chars = text.trim().chars().count() as f64;
    if chars == 0.0 {
        100.0
    } else {
        (1500.0 + 55.0 * chars).min(12_000.0)
    }
}

impl UiLayer {
    /// Loads the movies at `size`; failures are logged and the movie is skipped.
    pub fn new(ctx: &GpuContext, vfs: &Arc<Vfs>, names: &[String], size: (u32, u32)) -> Self {
        let gpu = Gpu {
            instance: ctx.instance.clone(),
            adapter: ctx.adapter.clone(),
            device: ctx.device.clone(),
            queue: ctx.queue.clone(),
        };
        let mut stage = Stage::new(gpu, vfs, size);
        stage.open_all(names);
        Self {
            stage,
            composite: None,
            device: ctx.device.clone(),
            queue: ctx.queue.clone(),
            ignored: Default::default(),
            shown: Default::default(),
            line: None,
            level: None,
            minimap_frame: 0,
            portraits: PortraitClock::default(),
            saves: None,
        }
    }

    /// Where the player's games are saved and loaded from.
    pub fn set_saves(&mut self, store: crate::saves::SaveStore) {
        self.saves = Some(store);
        self.refresh_saves();
    }

    /// Lists the saves again for the screens.
    fn refresh_saves(&self) {
        let (list, open) = match &self.saves {
            Some(s) => (s.list(), s.open_slot()),
            None => (Vec::new(), 1),
        };
        if let Ok(mut shown) = self.shown.lock() {
            shown.saves = list;
            shown.open_slot = open;
        }
    }

    /// Opens the save and load screen in `mode` (1 save, 2 load), as F5 and F9 do.
    pub fn open_saves(&mut self, screens: &mut Screens, mode: i32) {
        if let Ok(mut shown) = self.shown.lock() {
            shown.save_mode = mode;
        }
        self.refresh_saves();
        screens.menu = Some(Screen::SaveLoad);
    }

    /// The popup layer's messages: a movie's `GUI.ShowDialogBox` (after it set the dialog's
    /// text, buttons and callbacks in `GUIPopupLayer.*`) shows it with popuplayer.gfx's
    /// `PopupLayer.ShowDialogBoxFromGame` (the executable invokes it at 0x60bba4), and the
    /// dialog's buttons send `GUI.InvokeCallback <file> <function>`, which calls the function
    /// in that movie. Provisional: only screens that list the popup layer show dialogs.
    /// Returns whether the message was one of these.
    fn popup_message(&mut self, name: &str, args: &[String]) -> bool {
        match name {
            "GUI.ShowDialogBox" => {
                let shown = self.stage.call_in(
                    thedas_ui::stage::POPUP_LAYER,
                    "_global.PopupLayer.ShowDialogBoxFromGame",
                    &[],
                );
                if shown.is_none() {
                    log::warn!("ui: a dialog box was asked for with no popup layer up");
                }
            }
            "GUI.InvokeCallback" => {
                let (Some(movie), Some(function)) = (args.first(), args.get(1)) else {
                    return true;
                };
                if self.stage.call_in(movie, function, &[]).is_none() {
                    log::warn!("ui: no movie {movie} to call back ({function})");
                }
            }
            _ => return false,
        }
        true
    }

    /// The save, load and death screens' messages, which the app acts on (the saves are
    /// files): saveload.gfx's `SaveLoad.SaveGame` (slot, and the name typed),
    /// `LoadGame`/`DeleteSaveGame` (slot, kind), deathscreen.gfx's `ResumeGame` (the newest
    /// save) and `SetSaveLoadMode` + `GameModeController.PushGUIScreen SaveLoad` (its Load
    /// Game button), and `GameModeController.HideGUIScreen`. Loading replaces the
    /// simulation's state with the save's (`Sim::load_json`); saving writes `Sim::save_json`.
    /// Returns whether the message was one of these.
    fn save_message(
        &mut self,
        screens: &mut Screens,
        sim: &Arc<Mutex<Sim>>,
        name: &str,
        args: &[String],
    ) -> bool {
        let int = |i: usize| {
            args.get(i)
                .and_then(|a| a.trim().parse::<f64>().ok())
                .map(|v| v as i32)
        };
        match name {
            "SaveLoad.SetSaveLoadMode" => {
                if let (Ok(mut shown), Some(m)) = (self.shown.lock(), int(0)) {
                    shown.save_mode = m;
                }
            }
            "GameModeController.PushGUIScreen"
                if args.first().map(String::as_str) == Some("SaveLoad") =>
            {
                self.refresh_saves();
                screens.menu = Some(Screen::SaveLoad);
            }
            "GameModeController.HideGUIScreen" => {
                if screens.menu == Some(Screen::SaveLoad) {
                    screens.menu = None;
                }
            }
            "SaveLoad.LoadGame" | "SaveLoad.ResumeGame" => {
                let slot = match name {
                    "SaveLoad.LoadGame" => int(0),
                    _ => self.saves.as_ref().and_then(|s| s.latest()),
                };
                let bytes = slot.and_then(|slot| self.saves.as_ref().map(|s| s.read(slot)));
                match bytes {
                    Some(Ok(b)) => {
                        let mut sim = sim.lock().unwrap_or_else(|p| p.into_inner());
                        match sim.load_json(&b) {
                            Ok(()) => log::info!("ui: loaded save {slot:?}"),
                            Err(e) => log::warn!("ui: save {slot:?} did not load: {e}"),
                        }
                    }
                    Some(Err(e)) => log::warn!("ui: save {slot:?}: {e}"),
                    None => log::warn!("ui: no save to load"),
                }
                screens.menu = None;
            }
            "SaveLoad.SaveGame" => {
                let (Some(store), Some(slot)) = (self.saves.as_ref(), int(0)) else {
                    return true;
                };
                // The list's new save (slot -1) goes in the open slot.
                let slot = if slot > 0 { slot } else { store.open_slot() };
                let sim = sim.lock().unwrap_or_else(|p| p.into_inner());
                let area = sim
                    .world
                    .area
                    .map(|a| sim.display_name(a))
                    .unwrap_or_default();
                let typed = args
                    .get(1)
                    .map(|n| n.trim().to_string())
                    .unwrap_or_default();
                let meta = crate::saves::SaveMeta {
                    name: if typed.is_empty() {
                        area.clone()
                    } else {
                        typed
                    },
                    area,
                    time_ms: sim.world.time_ms,
                    written: crate::saves::now_secs(),
                };
                match sim.save_json() {
                    Ok(b) => match store.write(slot, &b, &meta) {
                        Ok(()) => log::info!("ui: saved to slot {slot}"),
                        Err(e) => log::warn!("ui: saving to slot {slot}: {e}"),
                    },
                    Err(e) => log::warn!("ui: the game could not be saved: {e}"),
                }
                drop(sim);
                self.refresh_saves();
                screens.menu = None;
            }
            "SaveLoad.DeleteSaveGame" => {
                if let (Some(store), Some(slot)) = (self.saves.as_ref(), int(0)) {
                    if let Err(e) = store.delete(slot) {
                        log::warn!("ui: deleting save {slot}: {e}");
                    }
                }
                self.refresh_saves();
            }
            // Provisional: there is no start menu to go back to yet.
            "GameModeController.ShutDownToStartMenuMessage" => {
                log::warn!("ui: quitting to the start menu is not available yet");
            }
            _ => return false,
        }
        true
    }

    /// Plays the game's screens over a running game: the movies act on `sim` as they send
    /// messages (see [`thedas_ui::input`]).
    pub fn attach(&mut self, sim: &Arc<Mutex<Sim>>) {
        self.stage
            .bridge
            .set_message_handler(Some(thedas_ui::input::handler_for(
                sim.clone(),
                self.shown.clone(),
            )));
    }

    /// One interface frame while playing: binds the simulation's values, brings the screens
    /// up to it, runs the movies `dt_ms` and applies the messages nothing handled yet. The
    /// simulation is not locked while the movies run (their handler locks it).
    /// Returns the abilities the movies asked to use on a target the player is to pick (see
    /// [`thedas_ui::input::aim`]).
    pub fn play(
        &mut self,
        screens: &mut Screens,
        sim: &Arc<Mutex<Sim>>,
        dt_ms: f64,
    ) -> Vec<thedas_ui::input::Aim> {
        // The journal, while its screen is up (built from the plots: it needs the simulation
        // mutably, which the bindings do not have).
        let journal = (screens.menu == Some(Screen::Journal))
            .then(|| sim.lock().unwrap_or_else(|p| p.into_inner()).journal());
        if let Ok(mut shown) = self.shown.lock() {
            shown.journal = journal;
        }
        let (want, line) = {
            let sim = sim.lock().unwrap_or_else(|p| p.into_inner());
            let shown = self.shown.lock().map(|s| s.clone()).unwrap_or_default();
            self.stage
                .bridge
                .bind(thedas_ui::binding::sim_values_for(&sim, &shown));
            let waits = thedas_ui::binding::awaits_continue(&sim);
            let line = thedas_ui::binding::conversation(&sim).map(|d| {
                let key: LineKey = (d.dialog, d.line, d.replies.iter().map(|r| r.0).collect());
                (key, waits)
            });
            (crate::screens::wanted(&sim, screens.menu), line)
        };
        let changes = screens.sync(want, &mut self.stage);
        // The store's close button: the simulation's store closes too.
        if changes.dismissed.contains(&Screen::Store) {
            sim.lock().unwrap_or_else(|p| p.into_inner()).close_store();
        }
        // A picker that closed itself without its Accept keeps the members picked so far.
        // Provisional: partypicker.gfx has no other way out.
        if changes.dismissed.contains(&Screen::PartyPicker) {
            sim.lock()
                .unwrap_or_else(|p| p.into_inner())
                .party_picker_submit();
        }
        self.line = match (line, self.line.take()) {
            (Some((key, _)), Some(mut l)) if l.key == key => {
                l.t += dt_ms;
                Some(l)
            }
            (Some((key, waits)), _) => Some(Line {
                key,
                t: 0.0,
                waits,
                continued: false,
            }),
            (None, _) => None,
        };
        if let Some(l) = self.line.as_mut() {
            let played = l.played();
            self.stage.bridge.set_value(
                "Conversation.ResponsesReady",
                thedas_ui::UiValue::Number(played as u8 as f64),
            );
            if played && l.waits && !l.continued {
                l.continued = true;
                self.continue_conversation();
            }
        }
        self.show_cutscene_line(sim);
        let messages = self.stage.tick(dt_ms);
        // A dialog focuses its text field (a save's name) once it is up. Nothing types into
        // the movies yet, and with the field focused the first press on a dialog button only
        // moves the focus (the button's press is cancelled), so the field gives its focus up
        // and its default text stands. Provisional: until keyboard input reaches the movies.
        if self.stage.dialog_up() {
            self.stage.call_in(
                thedas_ui::stage::POPUP_LAYER,
                "_global.Selection.setFocus",
                &[thedas_ui::UiValue::Null],
            );
        }
        let ignored = {
            let mut sim = sim.lock().unwrap_or_else(|p| p.into_inner());
            thedas_ui::input::apply_all(&mut sim, &messages)
        };
        // Messages for the interface itself, which the handler did not get to.
        let ignored: Vec<(String, Vec<String>)> = match self.shown.lock() {
            Ok(mut shown) => ignored
                .into_iter()
                .filter(|(n, a)| !thedas_ui::input::present(&mut shown, n, a))
                .collect(),
            Err(_) => ignored,
        };
        for m in &messages {
            if let GameMessage::Handled(name, args) | GameMessage::Send(name, args) = m {
                log::info!("ui: {name} {args:?}");
            }
        }
        let ignored: Vec<(String, Vec<String>)> = ignored
            .into_iter()
            .filter(|(n, a)| !self.popup_message(n, a) && !self.save_message(screens, sim, n, a))
            .collect();
        let mut aims = Vec::new();
        {
            let sim = sim.lock().unwrap_or_else(|p| p.into_inner());
            for (name, args) in &ignored {
                if let Some(a) = thedas_ui::input::aim(&sim, name, args) {
                    aims.push(a);
                }
            }
        }
        for (name, _) in ignored {
            // The stage closes a movie that asks for this itself; an ability to aim is the
            // app's to finish.
            if name != "GUI.HideFullScreenGUI"
                && name != thedas_ui::input::PERFORM_ABILITY
                && self.ignored.insert(name.clone())
            {
                log::warn!("ui: the simulation does not handle {name} yet");
            }
        }
        aims
    }

    /// A left click while playing reached the interface: a conversation line still playing is
    /// skipped to its end, and one with nothing to choose is continued (conversation.gfx's
    /// `ContinueButtonAction`). Provisional: the game's input bindings for the conversation's
    /// contexts are not traced; any click stands in for them.
    pub fn clicked(&mut self) {
        let Some(l) = self.line.as_mut() else {
            return;
        };
        if !l.played() {
            // Skip to the end of the line: its replies show (or it goes on) next frame.
            l.t = f64::INFINITY;
        } else if l.waits && !l.continued {
            l.continued = true;
            self.continue_conversation();
        }
    }

    /// conversation.gfx's continue action (it sends `Conversation.SelectResponse 0` when the
    /// line has nothing to choose).
    fn continue_conversation(&mut self) {
        if let Some(m) = self
            .stage
            .movies
            .iter_mut()
            .find(|m| m.name == "conversation.gfx")
        {
            m.call("_global.Conversation.ContinueButtonAction", &[]);
        }
    }

    /// Whether the party's portraits are due to be drawn (see [`PortraitClock`]): `key` is
    /// [`portrait_key`]'s. `dt_ms` is the time since the last call.
    pub fn wants_portraits(&mut self, key: Vec<(u32, u64)>, dt_ms: f64) -> bool {
        self.portraits.due(key, dt_ms)
    }

    /// Hands the movies freshly drawn portraits (RGBA8, [`PORTRAIT_SIZE`] square, by member).
    pub fn set_portraits(&mut self, shots: Vec<(u32, Vec<u8>)>) {
        let old = self
            .shown
            .lock()
            .map(|s| s.portraits.values().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        for name in old {
            self.stage.set_image(&name, None);
        }
        self.portraits.drawn += 1;
        let ids: Vec<u32> = shots.iter().map(|(id, _)| *id).collect();
        self.portraits.drew(&ids);
        let made = self.portraits.drawn;
        let mut names = std::collections::BTreeMap::new();
        for (id, pixels) in shots {
            let name = PortraitClock::name(id, made);
            let img = Rgba {
                width: PORTRAIT_SIZE,
                height: PORTRAIT_SIZE,
                pixels,
            };
            self.stage.set_image(&name, Some(&img));
            names.insert(id, name);
        }
        if let Ok(mut shown) = self.shown.lock() {
            shown.portraits = names;
        }
    }

    /// Shows the line the simulation's cutscene is saying, as subtitles
    /// (`Cutscene.SubtitleText`, which cutscene.gfx shows). Provisional: nothing is staged
    /// (no camera, animation or voice), so the world stands still behind the subtitles.
    fn show_cutscene_line(&mut self, sim: &Arc<Mutex<Sim>>) {
        let sim = sim.lock().unwrap_or_else(|p| p.into_inner());
        let text = sim
            .world
            .cutscene
            .as_ref()
            .and_then(|c| c.line_now())
            .and_then(|l| sim.string(l.strref))
            .unwrap_or_default();
        self.stage
            .bridge
            .set_value("Cutscene.SubtitleText", thedas_ui::UiValue::String(text));
    }

    /// The level being played (its layout and resources): the movies may load from them, and
    /// the minimap shows its picture. Does nothing when it is the same level.
    pub fn set_level(&mut self, layout: Option<&str>, vfs: Option<&Arc<Vfs>>) {
        if self.level.as_deref() == layout {
            return;
        }
        self.level = layout.map(str::to_string);
        self.stage.set_level_resources(vfs.cloned());
        let minimap = match (layout, vfs) {
            (Some(l), Some(v)) => thedas_ui::minimap::MinimapInfo::read(v, l),
            _ => None,
        };
        if let Ok(mut shown) = self.shown.lock() {
            shown.minimap = minimap;
        }
    }

    /// Sends the minimap where the camera and the area's creatures are (the engine writes
    /// `SharedLibrary.SharedMap`'s batch arrays: the camera first, then each creature's
    /// place on the map picture, in its pixels, and facing, three numbers each, and flags),
    /// then has it redraw. Provisional: every living creature in the area is shown, all
    /// within the map's radius.
    pub fn update_minimap(&mut self, sim: &Arc<Mutex<Sim>>, camera: &thedas_render::Camera) {
        self.minimap_frame += 1;
        if self.minimap_frame < MINIMAP_EVERY {
            return;
        }
        self.minimap_frame = 0;
        let Some(info) = self.shown.lock().ok().and_then(|s| s.minimap.clone()) else {
            return;
        };
        let Some(m) = self
            .stage
            .movies
            .iter_mut()
            .find(|m| m.name == "minimap.gfx")
        else {
            return;
        };
        let num = |x: f64| thedas_ui::UiValue::Number(x);
        // The camera's facing, as creatures' (degrees, 0 looking along +Y).
        let mut ids = vec![num(0.0)];
        let at = info.to_map(camera.position.x, camera.position.y);
        let mut places = vec![
            num(at[0] as f64),
            num(at[1] as f64),
            num(camera.yaw.to_degrees() as f64 - 90.0),
        ];
        let mut flags = vec![num(0.0)];
        {
            let sim = sim.lock().unwrap_or_else(|p| p.into_inner());
            let area = sim.world.area;
            let party = thedas_sim::engine::party_of(&sim);
            let (hero, controlled) = (sim.world.hero, sim.world.controlled());
            for o in sim.world.objects.values() {
                if o.kind != thedas_sim::world::ObjectKind::Creature
                    || !o.active
                    || o.dead
                    || o.area.is_none()
                    || o.area != area
                {
                    continue;
                }
                let mut f = MAP_WITHIN_RADIUS;
                if party.contains(&o.id) {
                    f |= MAP_IN_PARTY;
                }
                if Some(o.id) == controlled {
                    f |= MAP_PLAYER;
                }
                if hero.is_some_and(|h| sim.hostile(h, o.id)) {
                    f |= MAP_HOSTILE;
                }
                ids.push(num(o.id as f64));
                let at = info.to_map(o.position[0], o.position[1]);
                places.extend([num(at[0] as f64), num(at[1] as f64), num(o.facing as f64)]);
                flags.push(num(f as f64));
            }
        }
        let map = "_global.SharedLibrary.SharedMap";
        m.set(
            &format!("{map}.BatchCreatureIDs"),
            thedas_ui::UiValue::List(ids),
        );
        m.set(
            &format!("{map}.BatchCreaturePositionsOrientations"),
            thedas_ui::UiValue::List(places),
        );
        m.set(
            &format!("{map}.BatchCreatureFlags"),
            thedas_ui::UiValue::List(flags),
        );
        // The map picture's clip is authored fully transparent and nothing in the movie
        // raises it once the picture has loaded. Provisional: the game shows it, so it is
        // shown here.
        m.set(
            "_global.MiniMap.Instance.m_mcMapImage._alpha",
            thedas_ui::UiValue::Number(100.0),
        );
        m.call("_global.MiniMap.Instance.UpdateMinimap", &[]);
    }

    /// Renders every movie into its texture (the window does this as it composites).
    pub fn render_movies(&mut self) {
        for m in &mut self.stage.movies {
            m.render();
        }
    }

    /// Whether a movie drew at window pixel (`x`, `y`) in its last rendered frame: a click
    /// there is the interface's, not the world's. Provisional: the game hit-tests the movies'
    /// shapes; an opaque pixel stands in for that.
    pub fn covers(&self, x: f64, y: f64) -> bool {
        let textures: Vec<wgpu::Texture> = self
            .stage
            .movies
            .iter()
            .filter_map(|m| m.texture())
            .filter(|t| x >= 0.0 && y >= 0.0 && (x as u32) < t.width() && (y as u32) < t.height())
            .collect();
        if textures.is_empty() {
            return false;
        }
        const ROW: u64 = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as u64;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ui probe"),
            size: ROW * textures.len() as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("ui probe"),
            });
        for (i, t) in textures.iter().enumerate() {
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: t,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: x as u32,
                        y: y as u32,
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: &buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: ROW * i as u64,
                        bytes_per_row: Some(ROW as u32),
                        rows_per_image: Some(1),
                    },
                },
                wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
            );
        }
        self.queue.submit([encoder.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        if self
            .device
            .poll(wgpu::PollType::wait_indefinitely())
            .is_err()
            || !matches!(rx.recv(), Ok(Ok(())))
        {
            return false;
        }
        let Ok(data) = buffer.slice(..).get_mapped_range() else {
            return false;
        };
        // RGBA or BGRA: alpha is the fourth byte either way.
        (0..textures.len()).any(|i| {
            data.get(i * ROW as usize + 3)
                .is_some_and(|&a| a >= COVER_ALPHA)
        })
    }

    pub fn tick(&mut self, dt_ms: f64) -> Vec<GameMessage> {
        self.stage.tick(dt_ms)
    }

    pub fn resize(&mut self, size: (u32, u32)) {
        self.stage.resize(size);
    }

    pub fn mouse_move(&mut self, x: f64, y: f64) {
        self.stage.mouse_move(x, y);
    }

    pub fn mouse_button(&mut self, x: f64, y: f64, down: bool) {
        self.stage.mouse_button(x, y, down);
    }

    /// A click the movies see as Scaleform mouse `button` (a right click, a double click).
    pub fn click_as(&mut self, button: f64, x: f64, y: f64) {
        self.stage.click_as(button, x, y);
    }

    /// Renders every movie into its texture and draws them over `target`, in order.
    pub fn composite(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        format: wgpu::TextureFormat,
    ) {
        if self.stage.movies.is_empty() {
            return;
        }
        let comp = self
            .composite
            .get_or_insert_with(|| Composite::new(&self.device, format));
        let mut groups = Vec::new();
        for m in &mut self.stage.movies {
            m.render();
            let Some(tex) = m.texture() else { continue };
            let view = tex.create_view(&Default::default());
            groups.push(self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("ui movie"),
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
            }));
        }
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("ui composite"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&comp.pipeline);
        for g in &groups {
            pass.set_bind_group(0, g, &[]);
            pass.draw(0..3, 0..1);
        }
    }

    /// Logs what the movies asked for and did not get: failed loads and unanswered values.
    pub fn log_gaps(&self) {
        for (movie, url) in self.stage.failed_loads() {
            log::warn!("ui: {movie}: load of {url} failed");
        }
        let gaps = self.stage.bridge.unanswered();
        log::debug!("ui: unanswered: {:?}", gaps.keys().collect::<Vec<_>>());
        if !gaps.is_empty() {
            log::info!(
                "ui: {} engine values unanswered: {:?}",
                gaps.len(),
                gaps.keys().take(40).collect::<Vec<_>>()
            );
        }
    }

    pub fn capture_over(&mut self, base: &mut Rgba) -> Result<()> {
        Ok(self.stage.capture_over(base)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_play_for_a_reading_time() {
        assert_eq!(line_ms(""), 100.0);
        assert_eq!(line_ms("  "), 100.0);
        assert!(line_ms("Short.") < line_ms("A much longer line that takes a while to read."));
        assert_eq!(line_ms(&"x".repeat(1000)), 12_000.0);
    }

    #[test]
    fn portraits_are_drawn_once_and_again_only_when_what_they_show_changes() {
        let mut clock = PortraitClock::default();
        assert!(clock.due(vec![(1, 7)], 16.0));
        clock.drew(&[1]);
        // Nothing changed: never again, however long it plays.
        assert!(!clock.due(vec![(1, 7)], 16.0));
        assert!(!clock.due(vec![(1, 7)], 60_000.0));
        // A member joins, or one's equipment or appearance changes.
        assert!(clock.due(vec![(1, 7), (2, 9)], 16.0));
        clock.drew(&[1, 2]);
        assert!(!clock.due(vec![(1, 7), (2, 9)], 16.0));
        assert!(clock.due(vec![(1, 8), (2, 9)], 16.0));
        // A member left out (not drawn yet) is retried, after a while.
        clock.drew(&[1]);
        assert!(!clock.due(vec![(1, 8), (2, 9)], PORTRAIT_RETRY_MS - 1.0));
        assert!(clock.due(vec![(1, 8), (2, 9)], 1.0));
        assert_ne!(PortraitClock::name(1, 1), PortraitClock::name(1, 2));
    }
}
