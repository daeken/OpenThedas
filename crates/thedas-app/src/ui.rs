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

/// Draws a texture over the whole of a pass's viewport (the movies' textures; the intro
/// movies' frames).
pub(crate) struct Composite {
    pub(crate) pipeline: wgpu::RenderPipeline,
    /// Multiplies a premultiplied texture onto the target: target x (src + 1 - src alpha),
    /// Flash's `multiply` over an opaque picture (see `thedas_ui::MultiplyPass`).
    pub(crate) multiply: wgpu::RenderPipeline,
    pub(crate) layout: wgpu::BindGroupLayout,
    pub(crate) sampler: wgpu::Sampler,
}

impl Composite {
    pub(crate) fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
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
        let make = |label: &str, blend: wgpu::BlendState| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
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
                        blend: Some(blend),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let pipeline = make(
            "ui composite",
            wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING,
        );
        let multiply = make(
            "ui multiply",
            wgpu::BlendState {
                color: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::Dst,
                    dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                    operation: wgpu::BlendOperation::Add,
                },
                alpha: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::Zero,
                    dst_factor: wgpu::BlendFactor::One,
                    operation: wgpu::BlendOperation::Add,
                },
            },
        );
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self {
            pipeline,
            multiply,
            layout,
            sampler,
        }
    }
}

/// What the start menu and the game menu ask of the app, which owns the game.
pub enum FrontEnd {
    /// New Game: character creation in the single-player module.
    NewGame,
    /// A save the load screen picked before a game was running.
    Load(Vec<u8>),
    /// Back to the start menu (the game menu's Exit, the death screen's).
    StartMenu,
    /// Exit the game.
    Quit,
}

impl std::fmt::Debug for FrontEnd {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FrontEnd::NewGame => write!(f, "NewGame"),
            FrontEnd::Load(b) => write!(f, "Load({} bytes)", b.len()),
            FrontEnd::StartMenu => write!(f, "StartMenu"),
            FrontEnd::Quit => write!(f, "Quit"),
        }
    }
}

/// The multiply pass for a large notification's splash at `splash` (its
/// `UberNotification<n>.instance1`), from the visible clips' `paths`: the pass hides every
/// clip beside the splash's branch at each level up to the root, shows the splash, and draws
/// its `multiply` pieces (`is_multiply`) normally.
fn splash_pass(
    paths: &[&str],
    splash: &str,
    mut is_multiply: impl FnMut(&str) -> bool,
) -> thedas_ui::MultiplyPass {
    let child_of = |p: &str, parent: &str| {
        p.strip_prefix(parent)
            .and_then(|r| r.strip_prefix('.'))
            .is_some_and(|r| !r.contains('.'))
    };
    let mut hide = Vec::new();
    // Each ancestor of the splash (from `_level0` down to its notification), and the child
    // on the way down: the ancestor's other children are hidden.
    let mut parent = "_level0";
    let mut rest = splash.strip_prefix("_level0.").unwrap_or(splash);
    while let Some((step, tail)) = rest.split_once('.').or(Some((rest, ""))) {
        let keep = format!("{parent}.{step}");
        hide.extend(
            paths
                .iter()
                .filter(|p| child_of(p, parent) && **p != keep)
                .map(|p| p.to_string()),
        );
        if tail.is_empty() {
            break;
        }
        parent = &splash[..keep.len()];
        rest = tail;
    }
    let multiply = paths
        .iter()
        .filter(|p| child_of(p, splash))
        .filter(|p| is_multiply(p))
        .map(|p| p.to_string())
        .collect();
    thedas_ui::MultiplyPass {
        hide,
        show: vec![splash.to_string()],
        multiply,
    }
}

/// A message floaty up over a creature.
struct Floaty {
    id: u32,
    target: thedas_script::vm::ObjectId,
    /// Seconds until it goes.
    left_s: f64,
}

/// A floaty's scale at `d` metres from the camera (DAOrigins.exe 0x63bfa0): full size up to
/// 5 m, shrinking linearly to two thirds at 30 m and beyond. (Its alpha, 0x63c020, runs from
/// 1 at 2 m to 1 at 7 m: always 1.)
fn floaty_scale(d: f32) -> f64 {
    let t = ((d - 5.0) / (30.0 - 5.0)).clamp(0.0, 1.0);
    f64::from(1.0 + t * (2.0 / 3.0 - 1.0))
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
    /// What the menus asked of the app since it last looked (see [`FrontEnd`]).
    pub requests: Vec<FrontEnd>,
    /// Character creation's voice previews not yet played (see
    /// [`UiLayer::drain_sound_set_previews`]).
    sound_set_previews: Vec<(String, i32)>,
    /// The saves were listed for the start menu.
    saves_listed: bool,
    /// tooltips.gfx's tooltip texts (fixed and floating) last frame, and the frames left to lay
    /// the last tooltip out again after they changed (see `thedas_ui::avm1::TOOLTIP_RELAYOUT`).
    tooltip_text: Option<thedas_ui::UiValue>,
    tooltip_relayout: u8,
    /// The script message box shown (`World::popup`: its text, type and owner), and the frames
    /// it has waited for its values to reach the popup layer (see
    /// [`UiLayer::show_script_popup`]).
    script_popup: Option<((i32, i32, u32), u8)>,
    /// A party portrait was clicked (see [`UiLayer::mouse_button`]).
    portrait_clicked: Option<u32>,
    /// The large notification whose splash has been dealt with (see
    /// [`UiLayer::uber_alert_splash`]).
    uber_splash: Option<String>,
    /// The loading screen up, if any (see [`UiLayer::loading_begin`]).
    loading: Option<Loading>,
    /// The world map is held up as the loading screen (see [`UiLayer::hold_world_map`]).
    travel_map: bool,
    /// When the held world map began playing out, its load done.
    travel_out: Option<std::time::Instant>,
    /// The screens up last frame, for the events of opening one (see
    /// [`UiLayer::screens_opened`]).
    screens_up: std::collections::BTreeSet<Screen>,
    /// The floating messages up over creatures (see [`UiLayer::update_floaties`]).
    floaties: Vec<Floaty>,
    /// The stats floaties up (name and health bar over a creature): creature → floaty id.
    stat_floaties: std::collections::BTreeMap<thedas_script::vm::ObjectId, u32>,
    /// The player's settings file and the settings last written to it (see
    /// [`UiLayer::set_settings_file`]).
    settings_file: Option<(std::path::PathBuf, std::collections::BTreeMap<String, f64>)>,
    /// The creature the mouse is over (see [`UiLayer::set_hovered`]).
    hovered: Option<thedas_script::vm::ObjectId>,
    /// A conversation line's continue the movie could not send (see `continue_conversation`).
    continue_pending: bool,
    next_floaty: u32,
    portraits: PortraitClock,
    paperdoll: PaperdollClock,
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
    /// The movies' requests to draw them again (`Paperdoll.UpdatePortrait`) seen so far.
    requests: u32,
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

/// The paperdoll's size: the render target DAOrigins.exe's `GUIPaperdoll` makes for the
/// inventory and the character record (0x10e by 0x1cb, pushed at 0x7312ee and 0x73c310), the
/// rectangle inventory.gfx and characterrecord.gfx fill with it.
pub const PAPERDOLL_SIZE: (u32, u32) = (270, 459);

/// When the paperdoll is drawn: when a screen showing it is up and what it shows (who, their
/// appearance and equipment, how it is turned) changed, or the last drawing missed them a
/// moment ago (their model was not built yet).
#[derive(Default)]
struct PaperdollClock {
    key: Option<(u32, u64, i32)>,
    missing: bool,
    since_ms: f64,
    drawn: u32,
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
                sim.face_name(id)
                    .or_else(|| o.head_morph.clone())
                    .hash(&mut h);
                o.equipment.hash(&mut h);
                o.area.hash(&mut h);
                o.fields.get("Gender").hash(&mut h);
            }
            // Drawn on character creation's stage, or in the world.
            (sim.world.chargen == Some(id) && sim.world.chargen_mode == 0).hash(&mut h);
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
    /// How long its voice-over runs (milliseconds), when it has one: it plays that long
    /// instead of a reading time.
    voice_ms: Option<f64>,
    /// Whether conversation.gfx was last told the line has a voice-over (see
    /// `UiLayer::play`).
    voiced: bool,
    /// Nothing to choose: it continues by itself once played.
    waits: bool,
    continued: bool,
    /// A cutscene played while it was up (a line its cutscene speaks), and whether that
    /// cutscene has ended.
    cut_seen: bool,
    cut_done: bool,
}

impl Line {
    /// Whether it has played: its voice-over's (or reading) time has passed, or the
    /// cutscene that spoke it has ended. The executable gives a line that plays its own
    /// cutscene the cutscene's length (`GameModeDialog` 0x68e5be-0x68e5ff: the line's
    /// timer +0x2c4 is the cutscene's 5000, +0x8). Provisional: a line whose cutscene is
    /// played by the cutscene mode (a `CUTR` reference) is taken to end with it the same
    /// way.
    fn played(&self) -> bool {
        self.cut_done || self.t >= self.voice_ms.unwrap_or_else(|| line_ms(&self.key.1))
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
    /// Character creation's voice choice (chargen.gfx's `AppearanceSoundSetScene`):
    /// `GUI.TestSoundSet <set> <type>` previews a line of the set (`ss_types.gda` types with
    /// `bUseInChargen`, starting with HELLO) for the sound to take (see
    /// [`UiLayer::drain_sound_set_previews`]); `GUI.SetSoundSet <set>` gives the hero being
    /// made that set (`Sim::chargen_sound_set`). Provisional: the argument order is read from
    /// the movie's strings.
    fn sound_set_message(
        previews: &mut Vec<(String, i32)>,
        sim: &Arc<Mutex<Sim>>,
        name: &str,
        args: &[String],
    ) -> bool {
        let set = args
            .iter()
            .find(|a| a.chars().any(|c| c.is_ascii_alphabetic()))
            .map(|a| a.trim().to_ascii_lowercase());
        let number = args.iter().find_map(|a| a.trim().parse::<i32>().ok());
        match (name, set) {
            ("GUI.TestSoundSet", set) => {
                // The movie may send only the type; the set is then the one chosen (the
                // creature being made has it from GUI.SetSoundSet).
                let set = set.or_else(|| {
                    let sim = sim.lock().unwrap_or_else(|p| p.into_inner());
                    let c = sim.world.chargen?;
                    sim.world
                        .get(c)?
                        .text_fields
                        .get(thedas_sim::sim::SOUND_SET)
                        .cloned()
                });
                match set {
                    Some(set) => previews.push((set, number.unwrap_or(0))),
                    None => log::debug!("ui: voice preview {args:?} without a chosen set"),
                }
                true
            }
            ("GUI.SetSoundSet", Some(set)) => {
                sim.lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .chargen_sound_set(&set);
                true
            }
            _ => false,
        }
    }

    /// The voice previews character creation asked for since the last call: (sound set,
    /// `ss_types.gda` type).
    pub fn drain_sound_set_previews(&mut self) -> Vec<(String, i32)> {
        std::mem::take(&mut self.sound_set_previews)
    }

    /// The conversation line on screen speaks for this long (milliseconds; see
    /// [`crate::audio`]), or has no voice-over.
    pub fn set_line_voice_ms(&mut self, ms: Option<f64>) {
        if let Some(l) = self.line.as_mut() {
            l.voice_ms = ms;
        }
    }

    /// Sets a player setting as the options screen does (`ClientOptions.SetClientOptionMessage`
    /// `name` `value`); false when it is not one the screen keeps.
    pub fn set_client_option(&self, name: &str, value: &str) -> bool {
        let Ok(mut shown) = self.shown.lock() else {
            return false;
        };
        thedas_ui::options::option_message(
            &mut shown.options,
            thedas_ui::options::MSG_SET,
            &[name.to_string(), value.to_string()],
        )
    }

    /// Keeps the player's settings in `path` (see `thedas_ui::options::to_text`): what it
    /// holds now is taken, and every change the options screen makes is written back.
    /// A missing or unreadable file leaves the defaults.
    pub fn set_settings_file(&mut self, path: std::path::PathBuf) {
        let read = std::fs::read_to_string(&path)
            .map(|t| thedas_ui::options::from_text(&t))
            .unwrap_or_default();
        if let Ok(mut shown) = self.shown.lock() {
            shown.options.extend(read.clone());
        }
        self.settings_file = Some((path, read));
    }

    /// Writes the player's settings when they changed since last written.
    fn write_settings(&mut self) {
        let Some((path, written)) = self.settings_file.as_mut() else {
            return;
        };
        let Some(now) = self.shown.lock().ok().map(|s| s.options.clone()) else {
            return;
        };
        if now == *written {
            return;
        }
        *written = now.clone();
        let text = thedas_ui::options::to_text(&now);
        let result = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|_| std::fs::write(&*path, text));
        if let Err(e) = result {
            log::warn!("settings: writing {}: {e}", path.display());
        }
    }

    /// A player setting (`AudioOptions.MusicVolume`) as the options screen keeps it.
    pub fn client_option(&self, name: &str) -> Option<f64> {
        self.shown.lock().ok()?.options.get(name).copied()
    }

    /// Loads the movies at `size`; failures are logged and the movie is skipped.
    pub fn new(ctx: &GpuContext, vfs: &Arc<Vfs>, names: &[String], size: (u32, u32)) -> Self {
        let gpu = Gpu {
            instance: ctx.instance.clone(),
            adapter: ctx.adapter.clone(),
            device: ctx.device.clone(),
            queue: ctx.queue.clone(),
        };
        let mut stage = Stage::new(gpu, vfs, size);
        // The area map's explored-area mask (map.gfx loads `mapmask`, a picture the engine
        // makes; no resource has it): clear, so the whole map shows as explored.
        // Provisional: exploration is not tracked.
        stage.set_image(
            "mapmask.dds",
            Some(&Rgba {
                width: 1,
                height: 1,
                pixels: vec![0; 4],
            }),
        );
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
            requests: Vec::new(),
            sound_set_previews: Vec::new(),
            saves_listed: false,
            tooltip_text: None,
            tooltip_relayout: 0,
            script_popup: None,
            portrait_clicked: None,
            uber_splash: None,
            loading: None,
            travel_map: false,
            travel_out: None,
            screens_up: Default::default(),
            floaties: Vec::new(),
            stat_floaties: Default::default(),
            hovered: None,
            settings_file: None,
            continue_pending: false,
            next_floaty: 1,
            portraits: PortraitClock::default(),
            paperdoll: PaperdollClock::default(),
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

    /// The start menu's and the game menu's messages: options (`GameModeController.
    /// PushGUIScreen Options`), the game menu closing itself (`HideGUIScreen MainMenu`), and
    /// back to the start menu (`ExitToStartMenuMessage`, from the game menu;
    /// `ShutDownToStartMenuMessage`, from the death screen). Provisional: what the exe does
    /// between leaving a game and showing the start menu again is not traced.
    fn front_end_message(&mut self, screens: &mut Screens, name: &str, args: &[String]) -> bool {
        let first = args.first().map(|a| a.trim());
        match name {
            "GameModeController.PushGUIScreen" if first == Some("Options") => {
                screens.menu = Some(Screen::Options);
            }
            "GameModeController.HideGUIScreen" if first == Some("MainMenu") => {
                if screens.menu == Some(Screen::GameMenu) {
                    screens.menu = None;
                }
            }
            "GameModeController.ExitToStartMenuMessage"
            | "GameModeController.ShutDownToStartMenuMessage" => {
                self.requests.push(FrontEnd::StartMenu);
            }
            _ => return false,
        }
        true
    }

    /// A HUD notification clicked: notifications.gfx's `GUI.GoToQuest` (the quest's plot
    /// resource and whether it is done, in either order) and `GUI.GoToCodex` (the entry's
    /// resource) open the journal on that entry (its `GUIJournal.Notification*` values).
    /// Returns whether the message was one of these.
    /// navbar.gfx's page buttons (`GUI.ToggleFullScreenGUI <scene>`): the book turns to that
    /// screen, or closes when it is the one up, as the screen's key does. The main menu's
    /// button is left to the game menu. Provisional: the exe's handling of the message is not
    /// traced; the toggle follows the keys.
    fn book_page(screens: &mut Screens, name: &str, args: &[String]) -> bool {
        if name != "GUI.ToggleFullScreenGUI" {
            return false;
        }
        let page = match args.first().map(|a| a.trim()) {
            Some("CharacterRecord") => Screen::CharacterRecord,
            Some("Inventory") => Screen::Inventory,
            Some("Journal") => Screen::Journal,
            Some("Map") => Screen::AreaMap,
            Some("Tactics") => Screen::Tactics,
            Some("Skills") => Screen::Skills,
            Some("SpellsTalents") => Screen::SpellsTalents,
            Some("Options") => Screen::Options,
            _ => return true,
        };
        screens.toggle(page);
        true
    }

    fn go_to_journal(
        &mut self,
        screens: &mut Screens,
        sim: &Arc<Mutex<Sim>>,
        name: &str,
        args: &[String],
    ) -> bool {
        let codex = match name {
            "GUI.GoToQuest" => false,
            "GUI.GoToCodex" => true,
            _ => return false,
        };
        let Some(resref) = args
            .iter()
            .map(|a| a.trim())
            .find(|a| a.starts_with("plt_"))
        else {
            return true;
        };
        let view = sim.lock().unwrap_or_else(|p| p.into_inner()).journal();
        let focus = if codex {
            view.codex
                .iter()
                .find(|g| g.entries.iter().any(|e| e.resref == resref))
                .map(|g| thedas_ui::binding::JournalFocus {
                    codex: true,
                    resref: resref.to_string(),
                    group: g.kind.to_string(),
                    completed: false,
                })
        } else {
            let done = view.completed.iter().any(|q| q.resref == resref);
            view.active
                .iter()
                .chain(&view.completed)
                .find(|q| q.resref == resref)
                .map(|q| thedas_ui::binding::JournalFocus {
                    codex: false,
                    resref: resref.to_string(),
                    group: q.group.clone(),
                    completed: done,
                })
        };
        if let Ok(mut shown) = self.shown.lock() {
            shown.journal_focus = focus;
        }
        screens.menu = Some(Screen::Journal);
        true
    }

    /// The screens that opened since last frame: the module hears of each
    /// (`EVENT_TYPE_GUI_OPENED` with its `GUI_*` id, `Sim::gui_opened`, as DAOrigins.exe's
    /// screen-showing routine sends it, 0x60afa3; the inventory opened on a container is
    /// `GUI_CONTAINER_INVENTORY`), and a training step waiting for the tactics, level-up or
    /// inventory screen hears of that (`EVENT_TYPE_TRAINING_GUI_*_OPENED`, see
    /// `thedas_sim::training`). Provisional: the store, level-up and item examine screens'
    /// ids are not traced (script.ldf names none), so they send no GUI_OPENED.
    fn screens_opened(&mut self, screens: &Screens, sim: &Arc<Mutex<Sim>>) {
        let up = screens.shown().clone();
        let opened: Vec<Screen> = up.difference(&self.screens_up).copied().collect();
        self.screens_up = up;
        if opened.is_empty() {
            return;
        }
        let mut sim = sim.lock().unwrap_or_else(|p| p.into_inner());
        for s in opened {
            if let Some(id) = gui_id(s, sim.world.container.is_some()) {
                sim.gui_opened(id);
            }
            let name = match s {
                Screen::Tactics => "GUI_TACTICS_OPENED",
                Screen::LevelUp => "GUI_LEVLUP_OPENED",
                Screen::Inventory => "GUI_INVENTORY_OPENED",
                _ => continue,
            };
            sim.training_event(name, 0);
        }
    }

    /// Puts up a loading screen of `kind`; `hint` is the tip load_town.gfx shows
    /// (`GUILoading.LoadingText`; see [`thedas_ui::binding::load_hint`]). DAOrigins.exe shows
    /// load_town.gfx for a save load, a new game and a change of area list, load_light.gfx
    /// for an area change within one (the load mode's setter, 0x6c63f0). It stays up,
    /// alone, until [`UiLayer::loading_end`] and its movie has played out.
    pub fn loading_begin(&mut self, kind: LoadScreen, hint: Option<String>) {
        self.stage.bridge.set_value(
            "GUILoading.LoadingText",
            thedas_ui::UiValue::String(hint.unwrap_or_default()),
        );
        self.loading = Some(Loading {
            kind,
            faded: false,
            ending: false,
            frame: 0,
            outro_ms: 0.0,
        });
    }

    /// Holds the world map screen up, or lets it go: while the party travels and the area it
    /// arrives in loads, the map stays up as the loading screen though the simulation has
    /// closed it, as DAOrigins.exe keeps its world map GUI up until the load ends
    /// (0x6c6060).
    ///
    /// When the hold ends the movie is told the area has loaded
    /// (`WorldMap.AreaLoadedNotify`, which the exe invokes; its name is at 0xb19ed9): it
    /// plays the journey's last leg, and the screen stays up for that (see
    /// [`UiLayer::travel_map_frame`]). While it travels the movie takes no pin clicks itself
    /// (its `m_bTravelling`).
    pub fn hold_world_map(&mut self, hold: bool) {
        if hold {
            self.travel_map = true;
            self.travel_out = None;
        } else if self.travel_map && self.travel_out.is_none() {
            self.stage
                .call_in("worldmap.gfx", "_global.WorldMap.AreaLoadedNotify", &[]);
            self.travel_out = Some(std::time::Instant::now());
        }
    }

    /// The held world map playing out after its load: it goes once the movie has finished
    /// the journey's last leg (`WorldMap.GetTravelling()` false: the movie clears its
    /// `m_bTravelling` when its animation is done and does not close itself), or has closed,
    /// or after [`LOADING_OUTRO_MAX_MS`] of real time (frames while a level settles can be
    /// long, and the interface's frame time is capped).
    fn travel_map_frame(&mut self) {
        let Some(since) = self.travel_out else {
            return;
        };
        let travelling = self
            .stage
            .call_in("worldmap.gfx", "_global.WorldMap.GetTravelling", &[]);
        let done = !matches!(travelling, Some(thedas_ui::UiValue::Bool(true)));
        if done || since.elapsed().as_secs_f64() * 1000.0 >= LOADING_OUTRO_MAX_MS {
            self.travel_map = false;
            self.travel_out = None;
        }
    }

    /// Whether load_fade.gfx has faded to black (`GUIFadeToBlackLoading.FadeComplete`);
    /// the other loading screens are up at once.
    pub fn loading_fade_done(&self) -> bool {
        self.loading
            .is_some_and(|l| l.kind != LoadScreen::Fade || l.faded)
    }

    /// Whether a loading screen is up (or still playing out).
    pub fn loading(&self) -> bool {
        self.loading.is_some()
    }

    /// The load is done: the loading screen plays its `Outro` scene (`_root.PlayScene`, as
    /// the exe does) to its end, where it hides itself (see [`UiLayer::loading_frame`]).
    pub fn loading_end(&mut self) {
        let Some(l) = self.loading.as_mut() else {
            return;
        };
        if l.ending {
            return;
        }
        l.ending = true;
        let movie = l.kind.screen().movies()[0];
        // Its outro, as the exe plays it (`_root.PlayScene("Outro")`).
        let outro = self.stage.call_in(
            movie,
            "_root.PlayScene",
            &[thedas_ui::UiValue::String("Outro".into())],
        );
        if outro.is_none() {
            // No movie to play out: nothing will hide it.
            self.loading = None;
        }
    }

    /// A loading screen playing out its outro: it closes at its last frame, where it hides
    /// itself (`ExternalCommands.HideGuiScene(scene)`), or after at most
    /// [`LOADING_OUTRO_MAX_MS`].
    fn loading_frame(&mut self, dt_ms: f64) {
        let Some(l) = self.loading.filter(|l| l.ending) else {
            return;
        };
        // However the movie advances, the screen goes at most this long after the load is
        // done, so a load can never leave the player waiting on it.
        let left = l.outro_ms + dt_ms;
        if left >= LOADING_OUTRO_MAX_MS {
            log::info!("ui: the loading screen's outro ran over; closed");
            self.loading = None;
            return;
        }
        if let Some(l) = self.loading.as_mut() {
            l.outro_ms = left;
        }
        let movie = l.kind.screen().movies()[0];
        let frame = |stage: &mut thedas_ui::stage::Stage, p: &str| match stage.call_in(
            movie,
            &format!("_root.{p}.toString"),
            &[],
        ) {
            Some(thedas_ui::UiValue::String(s)) => s.parse::<u32>().ok(),
            _ => None,
        };
        // Done at its last frame, or once it has wrapped past it (a frame of the app can
        // span two of the movie's).
        let seen = l.frame;
        match (
            frame(&mut self.stage, "_currentframe"),
            frame(&mut self.stage, "_totalframes"),
        ) {
            (Some(at), Some(last)) if at < last && at >= seen => {
                if let Some(l) = self.loading.as_mut() {
                    l.frame = at;
                }
            }
            _ => self.loading = None,
        }
    }

    /// A large notification ("New Quest") draws a blood splash behind its plate, pieces with
    /// the multiply blend that darken the game's picture under it. The interface is drawn
    /// into its own transparent picture, where a multiply has nothing under it (the splash's
    /// white paper would show as a white block), so the splash is hidden from the movie's own
    /// picture and drawn alone in its multiply pass (`thedas_ui::MultiplyPass`), multiplied
    /// onto the frame under the movie.
    fn uber_alert_splash(&mut self, sim: &Arc<Mutex<Sim>>) {
        let head = {
            let sim = sim.lock().unwrap_or_else(|p| p.into_inner());
            sim.world.uber_alerts.first().map(|a| a.resref.clone())
        };
        let Some(head) = head else {
            if self.uber_splash.take().is_some() {
                for m in self.stage.movies.iter_mut() {
                    m.set_multiply_pass(None);
                }
            }
            return;
        };
        if self.uber_splash.as_deref() == Some(head.as_str()) {
            return;
        }
        // The notification's clip is named as it is added (`UberNotification<n>`); its
        // splash is its `instance1`.
        let Some(thedas_ui::UiValue::String(tree)) =
            self.stage
                .call_in("notifications.gfx", "_global.thedas_tree_list", &[])
        else {
            return;
        };
        let splash = tree.split(';').find_map(|e| {
            let path = e.split('|').next()?;
            let rest = path
                .split_once("UberNotificationArea_mc.UberNotification")?
                .1;
            let (n, tail) = rest.split_once('.')?;
            (n.chars().all(|c| c.is_ascii_digit()) && tail == "instance1").then_some(path)
        });
        if let Some(path) = splash {
            let paths: Vec<&str> = tree
                .split(';')
                .filter_map(|e| e.split('|').next())
                .collect();
            for m in self
                .stage
                .movies
                .iter_mut()
                .filter(|m| m.name == "notifications.gfx")
            {
                let pass = splash_pass(&paths, path, |p| {
                    m.call(&format!("{p}.blendMode.toString"), &[])
                        == thedas_ui::UiValue::String("multiply".into())
                });
                m.set(&format!("{path}._visible"), thedas_ui::UiValue::Bool(false));
                m.set_multiply_pass(Some(pass));
            }
            self.uber_splash = Some(head);
        }
    }

    /// Shows a script's message box (`ShowPopup`, `World::popup`) on the popup layer, as the
    /// game's own dialog boxes show (`PopupLayer.ShowDialogBoxFromGame`), once its values
    /// (`thedas_ui::binding`'s `GUIPopupLayer.*`) have reached the movie: two frames after
    /// it opened. Its buttons answer it (`thedas_ui::input::SCRIPT_DIALOG_RESULT`).
    fn show_script_popup(&mut self, sim: &Arc<Mutex<Sim>>) {
        let open = {
            let sim = sim.lock().unwrap_or_else(|p| p.into_inner());
            sim.world
                .popup
                .as_ref()
                .map(|p| (p.strref, p.kind, p.owner))
        };
        match (open, self.script_popup.as_mut()) {
            (None, _) => self.script_popup = None,
            (Some(k), Some((shown, frames))) if *shown == k => {
                if *frames < 2 {
                    *frames += 1;
                    if *frames == 2 {
                        let up = self.stage.call_in(
                            thedas_ui::stage::POPUP_LAYER,
                            "_global.PopupLayer.ShowDialogBoxFromGame",
                            &[],
                        );
                        if up.is_none() {
                            log::warn!("ui: a script message box with no popup layer up");
                        }
                    }
                }
            }
            (Some(k), _) => self.script_popup = Some((k, 0)),
        }
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
            // floatylayer.gfx tells the game its visible size; the floaties are placed from
            // the stage's own size here (see `update_floaties`).
            "GUI.UpdateFloatyLayerStageSize" => {}
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
            // The save screen's, or optionsmenu.gfx's OK and Cancel (`HideGUIScreen Options`).
            "GameModeController.HideGUIScreen" => {
                if screens.menu == Some(Screen::SaveLoad)
                    || (screens.menu == Some(Screen::Options)
                        && args.first().map(|a| a.trim()) == Some("Options"))
                {
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
                    // Before a game there is no module to load it into: the game starts
                    // from it.
                    Some(Ok(b)) if screens.front_end => self.requests.push(FrontEnd::Load(b)),
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
                let mut sim = sim.lock().unwrap_or_else(|p| p.into_inner());
                // No saving during a training tutorial (script.ldf's SetTrainingMode).
                if sim.training_paused() {
                    log::warn!("ui: no saving during a training tutorial");
                    return true;
                }
                // The module hears the save coming first (EVENT_TYPE_MODULE_PRESAVE).
                sim.presave();
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
                    hero: sim
                        .world
                        .hero
                        .map(|h| sim.display_name(h))
                        .unwrap_or_default(),
                    level: sim
                        .world
                        .hero
                        .map_or(0, |h| sim.property_total(h, 15) as i32),
                    class: sim
                        .world
                        .hero
                        .map_or(0, |h| sim.property_total(h, 27) as i32),
                };
                let saved = match sim.saving_blocked() {
                    Some(why) => Err(why.to_string()),
                    None => sim.save_json().map_err(|e| e.to_string()),
                };
                match saved {
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
            _ => return false,
        }
        true
    }

    /// Plays the game's screens over a running game: the movies act on `sim` as they send
    /// messages (see [`thedas_ui::input`]).
    pub fn attach(&mut self, sim: &Arc<Mutex<Sim>>) {
        // A new game or the start menu again: the saves are listed afresh for it.
        self.saves_listed = false;
        self.stage
            .bridge
            .set_message_handler(Some(thedas_ui::input::handler_for(
                sim.clone(),
                self.shown.clone(),
            )));
        self.stage
            .bridge
            .set_query_handler(Some(thedas_ui::input::query_for(sim.clone())));
    }

    /// Every movie closes and what the screens were showing is forgotten: a game starts or
    /// ends (the screens start over with it).
    pub fn close_all(&mut self) {
        self.stage.movies.clear();
        self.line = None;
        self.floaties.clear();
        self.tooltip_text = None;
        self.tooltip_relayout = 0;
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
        // The start menu offers loading when there are saves: they are listed from the start.
        if screens.front_end && !self.saves_listed {
            self.saves_listed = true;
            self.refresh_saves();
        }
        self.screens_opened(screens, sim);
        // The journal, while its screen is up (built from the plots: it needs the simulation
        // mutably, which the bindings do not have).
        let journal = (screens.menu == Some(Screen::Journal))
            .then(|| sim.lock().unwrap_or_else(|p| p.into_inner()).journal());
        let resolution = self.stage.size();
        if let Ok(mut shown) = self.shown.lock() {
            shown.resolution = resolution;
            // A notification's entry is the journal's to open on while it is up.
            if journal.is_none() {
                shown.journal_focus = None;
            }
            shown.journal = journal;
            shown.book_page = screens.menu.and_then(Screen::book_page);
            shown.menu_up = screens.menu.is_some();
            shown.ability_scene = match screens.menu {
                Some(Screen::Skills) => 3,
                Some(Screen::SpellsTalents) => 1,
                _ => 0,
            };
        }
        let (want, line) = {
            let sim = sim.lock().unwrap_or_else(|p| p.into_inner());
            let shown = self.shown.lock().map(|s| s.clone()).unwrap_or_default();
            self.stage
                .bridge
                .bind(thedas_ui::binding::sim_values_for(&sim, &shown));
            let waits = thedas_ui::binding::awaits_continue(&sim);
            let cutscene = sim.world.cutscene.is_some();
            let line = thedas_ui::binding::conversation(&sim).map(|d| {
                let key: LineKey = (d.dialog, d.line, d.replies.iter().map(|r| r.0).collect());
                (key, waits, cutscene)
            });
            // Before a game, the start menu, or the screen it opened (options, the load
            // screen).
            let want = match (self.loading, screens.front_end, screens.menu) {
                // A loading screen stands alone until it has played out.
                (Some(l), _, _) => std::collections::BTreeSet::from([l.kind.screen()]),
                // Travelling: the world map stays up as the loading screen.
                (None, false, _) if self.travel_map => {
                    std::collections::BTreeSet::from([Screen::WorldMap])
                }
                (None, true, None) => std::collections::BTreeSet::from([Screen::StartMenu]),
                (None, true, Some(m)) => std::collections::BTreeSet::from([m]),
                (None, false, _) => crate::screens::wanted(&sim, screens.menu),
            };
            (
                crate::screens::with_examine(want, shown.examine.is_some()),
                line,
            )
        };
        // The examine screen goes with the screen it was opened from.
        if !want.contains(&Screen::ItemExamine)
            && let Ok(mut shown) = self.shown.lock()
        {
            shown.examine = None;
        }
        let changes = screens.sync(want, &mut self.stage);
        // A screen that closes takes its tooltip with it: the movie that showed it is gone
        // and cannot clear it (a picker's tooltip stayed drawn over the HUD; a portrait's
        // over the tactics screen that replaced the HUD).
        if !changes.closed.is_empty() || !changes.opened.is_empty() {
            for k in ["GUI.TooltipText", "GUITooltips.TooltipText"] {
                self.stage
                    .bridge
                    .set_value(k, thedas_ui::UiValue::String(String::new()));
            }
        }
        // The store's close button: the simulation's store closes too.
        if changes.dismissed.contains(&Screen::Store) {
            sim.lock().unwrap_or_else(|p| p.into_inner()).close_store();
        }
        // The inventory closed (its close button or Escape) while a container was open:
        // the container closes too.
        if changes.dismissed.contains(&Screen::Inventory)
            || changes.closed.contains(&Screen::Inventory)
        {
            let mut s = sim.lock().unwrap_or_else(|p| p.into_inner());
            if s.world.container.is_some() {
                s.close_container();
            }
        }
        // A picker that closed itself without its Accept keeps the members picked so far.
        // Provisional: partypicker.gfx has no other way out.
        if changes.dismissed.contains(&Screen::PartyPicker) {
            sim.lock()
                .unwrap_or_else(|p| p.into_inner())
                .party_picker_submit();
        }
        self.line = match (line, self.line.take()) {
            (Some((key, _, cutscene)), Some(mut l)) if l.key == key => {
                l.t += dt_ms;
                l.cut_done |= l.cut_seen && !cutscene;
                l.cut_seen |= cutscene;
                Some(l)
            }
            (Some((key, waits, cutscene)), _) => Some(Line {
                key,
                t: 0.0,
                voice_ms: None,
                voiced: false,
                waits,
                continued: false,
                cut_seen: cutscene,
                cut_done: false,
            }),
            (None, _) => None,
        };
        // Whether the line is voiced (`Conversation.HasVO`): conversation.gfx's
        // `UpdateMainText` hides a voiced NPC line when the Subtitles option is "No
        // subtitles" (`ClientOptions.GameOptions.SubtitlesLevel` 0, the default), and shows
        // it otherwise. The voice is known once it plays (see `set_line_voice_ms`), a frame
        // or more after the line's text reached the movie, so the movie is given the line
        // again then. Provisional: when the exe sets HasVO (its attribute at 0x690193) is not
        // traced; a voice-over playing stands in.
        let mut retell = None;
        if let Some(l) = self.line.as_mut() {
            let voiced = l.voice_ms.is_some();
            self.stage.bridge.set_value(
                "Conversation.HasVO",
                thedas_ui::UiValue::Number(f64::from(u8::from(voiced))),
            );
            if voiced != l.voiced {
                l.voiced = voiced;
                retell = Some(l.key.1.clone());
            }
        }
        let subtitles = self
            .stage
            .bridge
            .value("ClientOptions.GameOptions.SubtitlesLevel");
        if let Some(text) = retell.filter(|_| subtitles == Some(thedas_ui::UiValue::Number(0.0))) {
            self.stage.call_in(
                "conversation.gfx",
                "_global.Conversation.UpdateMainText",
                &[thedas_ui::UiValue::String(text)],
            );
        }
        if let Some(l) = self.line.as_mut() {
            let played = l.played();
            self.stage.bridge.set_value(
                "Conversation.ResponsesReady",
                thedas_ui::UiValue::Number(played as u8 as f64),
            );
            // The conversation screen is not up while a cutscene plays: the line goes on
            // once it is.
            if played && l.waits && !l.continued {
                l.continued = true;
                self.continue_conversation();
            }
        }
        self.write_settings();
        self.show_cutscene_line(sim);
        self.show_script_popup(sim);
        self.select_clicked_portrait(sim);
        self.uber_alert_splash(sim);
        self.loading_frame(dt_ms);
        self.travel_map_frame();
        let mut messages = self.stage.tick(dt_ms);
        // A script message box's answer carries the text typed into it, which the box set
        // as `GUIPopupLayer.InputText` just before (popuplayer.gfx's `OnButtonAction`); the
        // exe passes it on as the result event's string 0 (0x709971).
        for m in &mut messages {
            if let GameMessage::Send(name, args) = m {
                if name == thedas_ui::input::SCRIPT_DIALOG_RESULT {
                    let typed = match self.stage.bridge.value("GUIPopupLayer.InputText") {
                        Some(thedas_ui::UiValue::String(s)) => s,
                        _ => String::new(),
                    };
                    args.truncate(3);
                    args.resize(3, String::new());
                    args.push(typed);
                }
            }
        }
        if std::mem::take(&mut self.continue_pending) {
            messages.push(GameMessage::Send(
                "Conversation.SelectResponse".into(),
                vec!["0".into()],
            ));
        }
        // A tooltip made for an equipment slot just emptied under the mouse is built from an
        // item that is no longer there ("undefined", "Cannot be unequipped",
        // "Indestructible"): it is cleared. Provisional: the original's own handling of an
        // emptied slot's tooltip is not traced.
        for k in ["GUI.TooltipText", "GUITooltips.TooltipText"] {
            if let Some(thedas_ui::UiValue::String(t)) = self.stage.bridge.value(k)
                && t.starts_with("<header>undefined</header>")
            {
                self.stage
                    .bridge
                    .set_value(k, thedas_ui::UiValue::String(String::new()));
            }
        }
        let tip = Some(thedas_ui::UiValue::List(
            ["GUI.TooltipText", "GUITooltips.TooltipText"]
                .map(|k| {
                    self.stage
                        .bridge
                        .value(k)
                        .unwrap_or(thedas_ui::UiValue::Undefined)
                })
                .to_vec(),
        ));
        if tip != self.tooltip_text {
            self.tooltip_relayout = 6;
            self.tooltip_text = tip;
        }
        // The fixed tooltip whose text changed is laid out again over the next few frames,
        // once the movie has shown it and its text has been laid out (the value reaches the
        // movie a frame or two after it is set; see `thedas_ui::avm1::TOOLTIP_RELAYOUT`).
        if self.tooltip_relayout > 0 {
            self.tooltip_relayout -= 1;
            self.stage
                .call_in("tooltips.gfx", thedas_ui::avm1::TOOLTIP_RELAYOUT, &[]);
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
                // load_fade.gfx's fade to black is done.
                if name == "GUIFadeToBlackLoading.FadeComplete"
                    && let Some(l) = self.loading.as_mut()
                {
                    l.faded = true;
                }
            }
            // The start menu's own fscommands: New Game loads the single-player module
            // (`getURL("FSCommand:LoadModule", "single player")`), Exit quits.
            if let GameMessage::Other(call) = m {
                match call.name.as_str() {
                    "LoadModule" => {
                        log::info!("ui: LoadModule {:?}", call.args);
                        self.requests.push(FrontEnd::NewGame);
                    }
                    "QuitGame" => self.requests.push(FrontEnd::Quit),

                    other => {
                        if self.ignored.insert(other.to_string()) {
                            log::warn!("ui: fscommand {other} {:?} is not handled", call.args);
                        }
                    }
                }
            }
        }
        let mut previews = Vec::new();
        let ignored: Vec<(String, Vec<String>)> = ignored
            .into_iter()
            .filter(|(n, a)| {
                !self.popup_message(n, a)
                    && !self.front_end_message(screens, n, a)
                    && !self.save_message(screens, sim, n, a)
                    && !self.go_to_journal(screens, sim, n, a)
                    && !Self::book_page(screens, n, a)
                    && !Self::sound_set_message(&mut previews, sim, n, a)
            })
            .collect();
        self.sound_set_previews.extend(previews);
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
                && name != "GUIFadeToBlackLoading.FadeComplete"
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

    /// The conversation line on screen: whether it has played (its replies show), and
    /// whether it has nothing to choose (it continues).
    pub fn line_state(&self) -> Option<(bool, bool)> {
        self.line.as_ref().map(|l| (l.played(), l.waits))
    }

    /// Highlights conversation reply `n` (from 0) the way the reply buttons highlight
    /// themselves when the console's focus lands on them (`NavigateOn` and `NavigateOff` on
    /// `_root.ButtonContainer_mc.Buttons_mc.Response1_mc`-`Response6_mc`, looked up afresh
    /// each time as the screen re-places them). False when the conversation screen is not
    /// up. The PC game has no such highlight (the mouse's hover is its own): a deliberate
    /// addition for the pad. Provisional: reply n is taken to be button n + 1, as the screen
    /// numbers them.
    pub fn highlight_reply(&mut self, n: usize) -> bool {
        let Some(m) = self
            .stage
            .movies
            .iter_mut()
            .find(|m| m.name == "conversation.gfx")
        else {
            return false;
        };
        for k in 1..=6 {
            let f = if k == n + 1 {
                "NavigateOn"
            } else {
                "NavigateOff"
            };
            m.call(
                &format!("_root.ButtonContainer_mc.Buttons_mc.Response{k}_mc.{f}"),
                &[],
            );
        }
        true
    }

    /// Calls a function in an open movie by path (the `call` step, for probing the movies
    /// headless); arguments that parse as numbers are passed as numbers.
    pub fn probe_call(
        &mut self,
        movie: &str,
        path: &str,
        args: &[String],
    ) -> Option<thedas_ui::UiValue> {
        let args: Vec<_> = args
            .iter()
            .map(|a| match a.parse::<f64>() {
                Ok(n) => thedas_ui::UiValue::Number(n),
                Err(_) => thedas_ui::UiValue::String(a.clone()),
            })
            .collect();
        self.stage.call_in(movie, path, &args)
    }

    /// Sets an engine value the movies read (the `setvalue` step): a number when it parses
    /// as one, else a string.
    pub fn probe_set_value(&mut self, name: &str, value: &str) {
        let v = value
            .parse::<f64>()
            .map(thedas_ui::UiValue::Number)
            .unwrap_or_else(|_| thedas_ui::UiValue::String(value.to_string()));
        self.stage.bridge.set_value(name, v);
    }

    /// Whether the player has paused the game (Space): the floaty layer's pause border and
    /// "Paused" text (`Client.Paused`, `Client.UserPaused`).
    /// The creature the mouse is over, whose name the stats floaties show (see
    /// [`crate::live::Live::hovered_creature`]); `None` when it is over none, or over the
    /// interface.
    pub fn set_hovered(&mut self, creature: Option<thedas_script::vm::ObjectId>) {
        self.hovered = creature;
    }

    pub fn set_paused(&self, paused: bool) {
        if let Ok(mut shown) = self.shown.lock() {
            shown.paused = paused;
        }
    }

    /// Whether the player is using a pad (the tutorial popup shows its pad hint).
    pub fn set_pad_active(&self, pad: bool) {
        if let Ok(mut shown) = self.shown.lock() {
            shown.pad_active = pad;
        }
    }

    /// conversation.gfx's continue action (it sends `Conversation.SelectResponse 0` when the
    /// line has nothing to choose).
    fn continue_conversation(&mut self) {
        let Some(m) = self
            .stage
            .movies
            .iter_mut()
            .find(|m| m.name == "conversation.gfx")
        else {
            // The screen is not up yet (a conversation's first line, empty, plays out
            // before its movie has loaded): the game's continue is sent for it, else the
            // conversation waited for ever.
            self.continue_pending = true;
            return;
        };
        // The movie's continue does nothing while it believes the line has replies to pick
        // (one it has not read yet): the game's continue is sent for it then too.
        let stale =
            m.call("_global.Conversation.HasResponses", &[]) == thedas_ui::UiValue::Bool(true);
        if stale {
            self.continue_pending = true;
        } else {
            m.call("_global.Conversation.ContinueButtonAction", &[]);
        }
    }

    /// Whether the party's portraits are due to be drawn (see [`PortraitClock`]): `key` is
    /// [`portrait_key`]'s. `dt_ms` is the time since the last call.
    pub fn wants_portraits(&mut self, key: Vec<(u32, u64)>, dt_ms: f64) -> bool {
        let asked = self.shown.lock().map(|s| s.portrait_requests).unwrap_or(0);
        let due = self.portraits.due(key, dt_ms);
        if asked != self.portraits.requests {
            self.portraits.requests = asked;
            self.portraits.since_ms = 0.0;
            return true;
        }
        due
    }

    /// The paperdoll's camera preset the movies chose (`Paperdoll.SetCameraPreset`; 0 the
    /// body, 1 the face), which frames character creation's stage.
    pub fn paperdoll_preset(&self) -> i32 {
        self.shown.lock().map(|s| s.paperdoll_preset).unwrap_or(0)
    }

    /// The party member to draw into the paperdoll and how far it is turned (radians), when
    /// it is due (see [`PaperdollClock`]): the one the inventory or character record shows
    /// (the member the player controls). `dt_ms` is the time since the last call.
    pub fn wants_paperdoll(
        &mut self,
        screens: &Screens,
        sim: &Sim,
        dt_ms: f64,
    ) -> Option<(u32, f32)> {
        let up = screens
            .shown()
            .iter()
            .any(|s| matches!(s, Screen::Inventory | Screen::CharacterRecord));
        let clock = &mut self.paperdoll;
        clock.since_ms += dt_ms;
        if !up {
            clock.key = None;
            return None;
        }
        let who = sim.world.controlled().or(sim.world.hero)?;
        let yaw = self.shown.lock().map(|s| s.paperdoll_yaw).unwrap_or(0.0);
        let look = portrait_key(sim)
            .into_iter()
            .find(|(id, _)| *id == who)
            .map_or(0, |(_, h)| h);
        let key = Some((who, look, (yaw.to_degrees() * 10.0).round() as i32));
        if key != clock.key || (clock.missing && clock.since_ms >= PORTRAIT_RETRY_MS) {
            clock.key = key;
            clock.since_ms = 0.0;
            return Some((who, yaw));
        }
        None
    }

    /// Hands the movies a fresh paperdoll drawing (straight RGBA8, [`PAPERDOLL_SIZE`]), or
    /// notes that the member could not be drawn.
    pub fn set_paperdoll(&mut self, shot: Option<Vec<u8>>) {
        let clock = &mut self.paperdoll;
        clock.missing = shot.is_none();
        let Some(pixels) = shot else { return };
        let old = format!("thedas_paperdoll_{}.png", clock.drawn);
        clock.drawn += 1;
        let name = format!("thedas_paperdoll_{}.png", clock.drawn);
        let (width, height) = PAPERDOLL_SIZE;
        self.stage.set_image(
            &name,
            Some(&Rgba {
                width,
                height,
                pixels,
            }),
        );
        self.stage.set_engine_image("paperdoll", Some(&name));
        self.stage.set_image(&old, None);
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

    /// Sends the maps where the camera and the area's creatures are (the engine writes
    /// `SharedLibrary.SharedMap`'s batch arrays: the camera first, then each creature's
    /// place on the map picture, in its pixels, and facing, three numbers each, and flags),
    /// then has them redraw: the HUD's minimap (minimap.gfx) and the area map screen
    /// (map.gfx), which share the picture (`Area.Minimap`) and the arrays. Provisional: every
    /// living creature in the area is shown, all within the map's radius.
    /// floatylayer.gfx's messages over creatures: each `DisplayFloatyMessage` a script made
    /// (damage, "Miss", "Immune", ...) becomes a message floaty (`FLOATY_SYMBOL_MESSAGE`)
    /// that follows its creature's head on screen until its time is up, fed as
    /// DAOrigins.exe feeds the movie: the `FloatyLayer.Batch*` arrays, then
    /// `FloatyLayer.ProcessBatchUpdates`. Positions are in the visible area's stage units
    /// (`SetFloatyPosition` adds the area's offset itself). Provisional: the head height, the
    /// time a floaty stays after its duration, and stacking several on one creature by
    /// `BatchUpdateOffsets` are inferred from the movie, not traced. A floaty stands 0.1 m
    /// over its creature's head, `head(creature)` metres above its feet (exe 0x764884).
    pub fn update_floaties(
        &mut self,
        sim: &Arc<Mutex<Sim>>,
        camera: &thedas_render::Camera,
        dt_ms: f64,
        head: &dyn Fn(thedas_script::vm::ObjectId) -> f32,
    ) {
        use thedas_ui::UiValue;
        const LAYER: &str = "_global.FloatyLayer";
        const SYMBOL_MESSAGE: f64 = 1.0;
        const SYMBOL_STATS: f64 = 0.0;
        // floatylayer.gfx's `FLOATY_STATS_*` flags.
        const SHOW_NAME: f64 = 1.0;
        const SHOW_HEALTH_BAR: f64 = 2.0;
        const SHOW_TARGET: f64 = 4.0;
        const IS_HOSTILE: f64 = 32.0;
        const IS_IN_PARTY: f64 = 64.0;
        /// Seconds a floaty stays past its duration (its own fade).
        const LINGER_S: f64 = 1.0;
        /// Metres over a creature's head (0xae7d70).
        const OVER_HEAD: f32 = 0.1;
        let Some(movie) = self
            .stage
            .movies
            .iter()
            .position(|m| m.name == "floatylayer.gfx")
        else {
            // No layer up: what scripts showed meanwhile is dropped.
            sim.lock()
                .unwrap_or_else(|p| p.into_inner())
                .world
                .floaties
                .clear();
            self.floaties.clear();
            self.stat_floaties.clear();
            return;
        };
        let num = UiValue::Number;
        let (mut create, mut symbols, mut show) = (Vec::new(), Vec::new(), Vec::new());
        let (mut msg_ids, mut texts, mut styles, mut colours, mut durations) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let (mut ids, mut offsets, mut places, mut destroy) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        let (
            mut stat_ids,
            mut stat_flags,
            mut stat_names,
            mut stat_hp,
            mut stat_max,
            mut stat_ranks,
        ) = (
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        );
        let size = self.stage.size();
        let scale = thedas_ui::stage_scale(size, (1024.0, 768.0));
        let (w, h) = (size.0 as f32, size.1 as f32);
        let vp = camera.view_proj(w / h.max(1.0));
        {
            let mut sim = sim.lock().unwrap_or_else(|p| p.into_inner());
            for f in std::mem::take(&mut sim.world.floaties) {
                let id = self.next_floaty;
                self.next_floaty += 1;
                create.push(num(f64::from(id)));
                symbols.push(num(SYMBOL_MESSAGE));
                show.push(num(f64::from(id)));
                msg_ids.push(num(f64::from(id)));
                texts.push(UiValue::String(f.text));
                styles.push(num(f64::from(f.style)));
                colours.push(num(f64::from(f.colour)));
                durations.push(num(f64::from(f.duration)));
                self.floaties.push(Floaty {
                    id,
                    target: f.target,
                    left_s: f64::from(f.duration.max(0.0)) + LINGER_S,
                });
            }
            let mut per_target: std::collections::BTreeMap<u32, u32> = Default::default();
            self.floaties.retain_mut(|f| {
                f.left_s -= dt_ms / 1000.0;
                let o = sim
                    .world
                    .get(f.target)
                    .filter(|o| o.active && o.area == sim.world.area);
                if f.left_s <= 0.0 || o.is_none() {
                    destroy.push(num(f64::from(f.id)));
                    return false;
                }
                let o = o.expect("checked");
                let z = o.position[2] + head(o.id) + OVER_HEAD;
                let p = glam::Vec3::new(o.position[0], o.position[1], z);
                let c = vp * p.extend(1.0);
                let (x, y, alpha) = if c.w > 1e-4 {
                    let n = c.truncate() / c.w;
                    ((n.x + 1.0) * 0.5 * w, (1.0 - n.y) * 0.5 * h, 1.0)
                } else {
                    (0.0, 0.0, 0.0)
                };
                let stack = per_target.entry(f.target).or_default();
                *stack += 1;
                ids.push(num(f64::from(f.id)));
                offsets.push(num(f64::from(*stack)));
                places.extend([
                    num(f64::from(x) / scale),
                    num(f64::from(y) / scale),
                    num(floaty_scale(camera.position.distance(p))),
                    num(alpha),
                    num(f64::from(f.id)),
                ]);
                true
            });
            // The stats floaties (name and health bar over a creature): the hostiles in a
            // fight with the party, the leader's attack target named. Provisional: the
            // exe's choice of creatures and flags is not traced; this follows what the
            // original shows in a fight (a health bar over each enemy, the target's name).
            let leader = sim.world.controlled();
            let target = leader
                .and_then(|l| sim.world.get(l))
                .and_then(|o| o.commands.first())
                .filter(|c| c.name == "CommandAttack")
                .and_then(|c| c.target);
            let party = thedas_sim::engine::party_of(&sim);
            let health = thedas_sim::sim::constant(&sim.table, "PROPERTY_DEPLETABLE_HEALTH");
            // The creature under the mouse gets one too (exe 0x6a52c0: creature+0xa10, the
            // hover mark, makes the floaty), while within 50 m of the leader (0x7c4ae0
            // compares the squared distance with 2500).
            let leader_at = leader
                .and_then(|l| sim.world.get(l))
                .map(|o| glam::Vec3::from_array(o.position));
            let hovered = self.hovered.filter(|&c| {
                sim.world.get(c).is_some_and(|o| {
                    o.kind == thedas_sim::world::ObjectKind::Creature
                        && o.active
                        && !o.dead
                        && o.area == sim.world.area
                        && leader_at.is_some_and(|l| {
                            l.distance_squared(glam::Vec3::from_array(o.position)) < 2500.0
                        })
                })
            });
            let mut wanted: Vec<thedas_script::vm::ObjectId> = sim
                .world
                .objects
                .values()
                .filter(|o| {
                    o.kind == thedas_sim::world::ObjectKind::Creature
                        && o.active
                        && !o.dead
                        && o.area == sim.world.area
                        && (Some(o.id) == target
                            || (o.combat.in_combat && party.iter().any(|&m| sim.hostile(m, o.id))))
                })
                .map(|o| o.id)
                .collect();
            if let Some(h) = hovered.filter(|h| !wanted.contains(h)) {
                wanted.push(h);
            }
            for (&c, &id) in &self.stat_floaties {
                if !wanted.contains(&c) {
                    destroy.push(num(f64::from(id)));
                }
            }
            self.stat_floaties.retain(|c, _| wanted.contains(c));
            for c in wanted {
                let id = match self.stat_floaties.get(&c) {
                    Some(&id) => id,
                    None => {
                        let id = self.next_floaty;
                        self.next_floaty += 1;
                        create.push(num(f64::from(id)));
                        symbols.push(num(SYMBOL_STATS));
                        show.push(num(f64::from(id)));
                        self.stat_floaties.insert(c, id);
                        id
                    }
                };
                let Some(o) = sim.world.get(c) else { continue };
                let z = o.position[2] + head(o.id) + OVER_HEAD;
                let p = glam::Vec3::new(o.position[0], o.position[1], z);
                let cl = vp * p.extend(1.0);
                let (x, y, alpha) = if cl.w > 1e-4 {
                    let n = cl.truncate() / cl.w;
                    ((n.x + 1.0) * 0.5 * w, (1.0 - n.y) * 0.5 * h, 1.0)
                } else {
                    (0.0, 0.0, 0.0)
                };
                ids.push(num(f64::from(id)));
                offsets.push(num(0.0));
                places.extend([
                    num(f64::from(x) / scale),
                    num(f64::from(y) / scale),
                    num(floaty_scale(camera.position.distance(p))),
                    num(alpha),
                    num(f64::from(id)),
                ]);
                let mut flags = if Some(c) == hovered {
                    // The exe's flags (0x764e40): the name always; the health bar for a
                    // hostile, a party member or a creature in combat; whether it is
                    // hostile or in the party. Provisional: HAS_MANA (the class's mana,
                    // 0x71a060) is not sent.
                    let hostile = party.iter().any(|&m| sim.hostile(m, c));
                    let in_party = party.contains(&c);
                    let mut f = SHOW_NAME;
                    if hostile || in_party || o.combat.in_combat {
                        f += SHOW_HEALTH_BAR;
                    }
                    if hostile {
                        f += IS_HOSTILE;
                    }
                    if in_party {
                        f += IS_IN_PARTY;
                    }
                    f
                } else {
                    SHOW_HEALTH_BAR + IS_HOSTILE
                };
                if Some(c) == target {
                    flags = SHOW_HEALTH_BAR + IS_HOSTILE + SHOW_NAME + SHOW_TARGET;
                }
                let (cur, max) = (
                    o.properties.get(&health).map_or(0.0, |p| f64::from(p.1)),
                    sim.depletable_max(c, health),
                );
                stat_ids.push(num(f64::from(id)));
                stat_flags.push(num(flags));
                stat_names.push(UiValue::String(sim.display_name(c)));
                stat_hp.push(num(cur));
                stat_max.push(num(f64::from(max)));
                stat_ranks.push(num(f64::from(o.rank)));
            }
        }
        let m = &mut self.stage.movies[movie];
        let list = |v: Vec<UiValue>| UiValue::List(v);
        for (k, v) in [
            ("BatchCreateIDs", create),
            ("BatchCreateSymbolIDs", symbols),
            ("BatchShowIDs", show),
            ("BatchHideIDs", Vec::new()),
            ("BatchDestroyIDs", destroy),
            ("BatchUpdateIDs", ids),
            ("BatchUpdateOffsets", offsets),
            ("BatchUpdatePositionsScalesAlphas", places),
            ("BatchUpdateMessageIDs", msg_ids),
            ("BatchUpdateMessageStrings", texts),
            ("BatchUpdateMessageStyles", styles),
            ("BatchUpdateMessageColours", colours),
            ("BatchUpdateMessageDurations", durations),
            ("BatchUpdateStatsIDs", stat_ids),
            ("BatchUpdateStatsFlags", stat_flags),
            ("BatchUpdateStatsNames", stat_names),
            ("BatchUpdateStatsCurrentHealths", stat_hp),
            ("BatchUpdateStatsTotalHealths", stat_max),
            ("BatchUpdateStatsRanks", stat_ranks),
            ("BatchUpdateIconIDs", Vec::new()),
            ("BatchUpdateOffscreenIDs", Vec::new()),
        ] {
            m.set(&format!("{LAYER}.{k}"), list(v));
        }
        m.call(&format!("{LAYER}.ProcessBatchUpdates"), &[]);
    }

    pub fn update_minimap(&mut self, sim: &Arc<Mutex<Sim>>, camera: &thedas_render::Camera) {
        self.minimap_frame += 1;
        if self.minimap_frame < MINIMAP_EVERY {
            return;
        }
        self.minimap_frame = 0;
        let Some(info) = self.shown.lock().ok().and_then(|s| s.minimap.clone()) else {
            return;
        };
        let num = |x: f64| thedas_ui::UiValue::Number(x);
        // The camera's icon (entry 0): its view cone, which the minimap centres on, at the
        // camera's pivot, the party member it follows (the original's cone starts at the
        // player, not at the eye up behind them; the eye, far out in the tactical view, put
        // the cone off the minimap), turned with the camera's facing (degrees, 0 looking
        // along +Y). Provisional: the exe's own camera entry is not traced.
        let mut ids = vec![num(0.0)];
        let mut places = Vec::new();
        let mut flags = vec![num(0.0)];
        {
            let sim = sim.lock().unwrap_or_else(|p| p.into_inner());
            let pivot = sim
                .world
                .controlled()
                .or(sim.world.hero)
                .and_then(|id| sim.world.get(id))
                .map_or((camera.position.x, camera.position.y), |o| {
                    (o.position[0], o.position[1])
                });
            let at = info.to_map(pivot.0, pivot.1);
            places.extend([
                num(at[0] as f64),
                num(at[1] as f64),
                num(camera.yaw.to_degrees() as f64 - 90.0),
            ]);
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
                    // A hostile shows once the party perceives it. Provisional: the
                    // exe's own filter is not traced.
                    let seen = party.iter().any(|&m| {
                        sim.world
                            .get(m)
                            .is_some_and(|p| p.combat.perceived.contains(&o.id))
                    });
                    if !seen {
                        continue;
                    }
                    f |= MAP_HOSTILE;
                } else if f & MAP_IN_PARTY == 0 {
                    // Neither the party's nor an enemy: the legend has no mark for the
                    // neutral townsfolk (they drew as a field of identical dots).
                    // Provisional: allies (`FLAG_SHOW_AS_ALLY`) are not told apart yet.
                    continue;
                }
                ids.push(num(o.id as f64));
                let at = info.to_map(o.position[0], o.position[1]);
                places.extend([num(at[0] as f64), num(at[1] as f64), num(o.facing as f64)]);
                flags.push(num(f as f64));
            }
        }
        let map = "_global.SharedLibrary.SharedMap";
        for m in self
            .stage
            .movies
            .iter_mut()
            .filter(|m| m.name == "minimap.gfx" || m.name == "map.gfx")
        {
            m.set(
                &format!("{map}.BatchCreatureIDs"),
                thedas_ui::UiValue::List(ids.clone()),
            );
            m.set(
                &format!("{map}.BatchCreaturePositionsOrientations"),
                thedas_ui::UiValue::List(places.clone()),
            );
            m.set(
                &format!("{map}.BatchCreatureFlags"),
                thedas_ui::UiValue::List(flags.clone()),
            );
            if m.name == "minimap.gfx" {
                // The map picture's clip is authored fully transparent and nothing in the
                // movie raises it once the picture has loaded. Provisional: the game shows
                // it, so it is shown here.
                m.set(
                    "_global.MiniMap.Instance.m_mcMapImage._alpha",
                    thedas_ui::UiValue::Number(100.0),
                );
                // The camera icon's view cone is two shapes placed with the `overlay` blend
                // mode (CameraIcon, sprite 33: instance7 the wedge, instance8 its edges).
                // Over the minimap's near-black surround an overlay stays black, so only the
                // part over the map picture showed; the original shows the whole wedge, as
                // if drawn normally (like map.gfx's blend modes below). Provisional:
                // inferred from the original's minimap.
                for part in ["instance7", "instance8"] {
                    m.set(
                        &format!(
                            "_global.MiniMap.Instance.m_SharedMap.CameraIcon.{part}.blendMode"
                        ),
                        thedas_ui::UiValue::String("normal".into()),
                    );
                }
                m.call("_global.MiniMap.Instance.UpdateMinimap", &[]);
            } else {
                // map.gfx draws its picture and its explored-area mask with the `alpha` and
                // `erase` blend modes, in a clip that is not a layer. The player follows
                // Flash and draws nothing of them there, while the game shows the map: they
                // are drawn as `normal`, as Scaleform appears to. Provisional: the
                // executable's renderer is not traced.
                let clip = "_global.Map.cm_MapContainer.Map_mc";
                for c in ["MapImage_mc", "MapImageAlpha_mc", "MapMask_mc"] {
                    m.set(
                        &format!("{clip}.{c}.blendMode"),
                        thedas_ui::UiValue::String("normal".into()),
                    );
                }
                m.call("_global.Map.cm_SharedMap.UpdateMap", &[]);
            }
        }
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

    /// The mouse wheel turned `lines` (up positive) at (`x`, `y`): lists and text under the
    /// mouse scroll.
    pub fn mouse_wheel(&mut self, x: f64, y: f64, lines: f64) {
        self.stage.mouse_wheel(x, y, lines);
    }

    pub fn mouse_button(&mut self, x: f64, y: f64, down: bool) {
        self.stage.mouse_button(x, y, down);
        // A click on a party portrait: portraits.gfx only says which member the mouse is
        // over (`GameModeExplore.GUITargetedPlayer`, set on roll-over and cleared on
        // roll-out; its own click handler is empty), and the game selects them.
        if !down
            && let Some(thedas_ui::UiValue::Number(id)) =
                self.stage.bridge.value("GameModeExplore.GUITargetedPlayer")
            && id > 0.0
        {
            self.portrait_clicked = Some(id as u32);
        }
    }

    /// The party member whose portrait was clicked becomes the one the player controls, as
    /// a click on them in the world does. Provisional: the exe's handling of
    /// `GUITargetedPlayer` is not traced (it may also take the portrait as an ability's
    /// target).
    fn select_clicked_portrait(&mut self, sim: &Arc<Mutex<Sim>>) {
        let Some(id) = self.portrait_clicked.take() else {
            return;
        };
        let mut sim = sim.lock().unwrap_or_else(|p| p.into_inner());
        if thedas_sim::engine::party_of(&sim).contains(&id) {
            sim.player_selects(id);
        }
    }

    /// A click the movies see as Scaleform mouse `button` (a right click, a double click).
    /// A press or release the movies see as Scaleform `button` (see
    /// [`thedas_ui::stage::Stage::button_as`]).
    pub fn button_as(&mut self, button: f64, x: f64, y: f64, down: bool) {
        self.stage.button_as(button, x, y, down);
    }

    pub fn click_as(&mut self, button: f64, x: f64, y: f64) {
        self.stage.click_as(button, x, y);
    }

    /// Whether a movie's text field has focus (keys type into it, not the game's).
    pub fn typing(&mut self) -> bool {
        self.stage.text_focus().is_some()
    }

    /// A key for the focused text field; false when there is none.
    pub fn key(&mut self, key: thedas_ui::UiKey) -> bool {
        self.stage.key(key)
    }

    /// Types text into the focused text field; false when there is none.
    pub fn type_text(&mut self, text: &str) -> bool {
        self.stage.type_text(text)
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
        let group = |tex: &wgpu::Texture| {
            let view = tex.create_view(&Default::default());
            self.device.create_bind_group(&wgpu::BindGroupDescriptor {
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
            })
        };
        // The movies' multiplied clips first, onto the game's picture under the whole
        // interface (the original's "New Quest" splash leaves the minimap above it as it
        // was: reference cx_city_elf_after_unpause_new_quest.png).
        for m in &mut self.stage.movies {
            if let Some(tex) = m.render_multiply() {
                groups.push((group(&tex), true));
            }
        }
        for m in &mut self.stage.movies {
            m.render();
            let Some(tex) = m.texture() else { continue };
            groups.push((group(&tex), false));
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
        for (g, multiply) in &groups {
            pass.set_pipeline(if *multiply {
                &comp.multiply
            } else {
                &comp.pipeline
            });
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

/// The loading screens: their movies hold on a frame while the game loads, then play on to
/// their end and hide themselves (`HideGuiScene <scene>`), as read from their code; the app
/// closes them at their last frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadScreen {
    /// load_town.gfx (scene `Loading`): parchment art, "Loading ..." and a hint.
    Town,
    /// load_light.gfx (scene `LightLoading`): black with "Loading".
    Light,
    /// load_fade.gfx (scene `FadeToBlackLoading`): fades to black first
    /// (`GUIFadeToBlackLoading.FadeComplete`). Not an area load's: the exe shows it for the
    /// party picker only.
    Fade,
}

impl LoadScreen {
    fn screen(self) -> Screen {
        match self {
            LoadScreen::Town => Screen::LoadTown,
            LoadScreen::Light => Screen::LoadLight,
            LoadScreen::Fade => Screen::LoadFade,
        }
    }
}

/// A loading screen up: whether its fade is done, and whether it is playing out.
#[derive(Clone, Copy, Debug)]
struct Loading {
    kind: LoadScreen,
    faded: bool,
    ending: bool,
    /// The movie's frame when last seen playing out.
    frame: u32,
    /// How long it has been playing out.
    outro_ms: f64,
}

/// The longest a loading screen plays out after its load (load_town's Outro is 38 frames,
/// about 1.3 s at its 30 fps).
const LOADING_OUTRO_MAX_MS: f64 = 2000.0;

/// A screen's `GUI_*` id (script.ldf's `EVENT_TYPE_GUI_OPENED` constants), the inventory on a
/// container as `GUI_CONTAINER_INVENTORY`. None for screens script.ldf names no id for.
fn gui_id(s: Screen, container: bool) -> Option<i32> {
    Some(match s {
        Screen::CharacterRecord => 0,
        Screen::CharGen => 1,
        Screen::Inventory if container => 2,
        Screen::Conversation => 4,
        Screen::Cutscene => 5,
        Screen::Hud => 7,
        Screen::Inventory => 8,
        Screen::Journal => 9,
        Screen::SpellsTalents => 10,
        Screen::Skills => 13,
        Screen::AreaMap => 15,
        Screen::WorldMap => 16,
        Screen::Options => 17,
        Screen::StartMenu => 18,
        Screen::Death => 19,
        Screen::GameMenu => 20,
        Screen::SaveLoad => 21,
        Screen::Chanters => 22,
        Screen::Tactics => 23,
        Screen::PartyPicker => 27,
        Screen::LoadTown | Screen::LoadLight | Screen::LoadFade => 14,
        Screen::Store | Screen::LevelUp | Screen::ItemExamine => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_splash_pass_hides_everything_beside_the_splash_branch() {
        let n = "_level0.UberNotificationArea_mc.UberNotification5";
        let splash = format!("{n}.instance1");
        let paths = [
            "_level0.UberNotificationArea_mc".to_string(),
            n.to_string(),
            format!("{n}.GUI_mc"),
            format!("{n}.GUI_mc.Header_mc"),
            splash.clone(),
            format!("{splash}.instance9"),
            format!("{splash}.instance9.instance11"),
            format!("{splash}.instance2"),
            "_level0.NotificationArea_mc".to_string(),
            "_level0.NotificationArea_mc.NotificationActivate4".to_string(),
        ];
        let paths: Vec<&str> = paths.iter().map(String::as_str).collect();
        let pass = splash_pass(&paths, &splash, |p| p.ends_with("instance9"));
        assert_eq!(
            pass.hide,
            [
                "_level0.NotificationArea_mc".to_string(),
                format!("{n}.GUI_mc")
            ]
        );
        assert_eq!(pass.show, std::slice::from_ref(&splash));
        assert_eq!(pass.multiply, [format!("{splash}.instance9")]);
    }

    #[test]
    fn floaties_shrink_to_two_thirds_from_5_to_30_metres() {
        assert_eq!(floaty_scale(0.0), 1.0);
        assert_eq!(floaty_scale(5.0), 1.0);
        assert!((floaty_scale(17.5) - 5.0 / 6.0).abs() < 1e-6);
        assert!((floaty_scale(30.0) - 2.0 / 3.0).abs() < 1e-6);
        assert!((floaty_scale(300.0) - 2.0 / 3.0).abs() < 1e-6);
    }

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
