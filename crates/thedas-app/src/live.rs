//! A running game inside the window: the simulation ticks at real time (unless paused), and
//! the area it is in is drawn with its creatures and placeables posed from it. The
//! simulation is only read here; input reaches it as commands elsewhere.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use anyhow::{Context, Result};
use glam::Mat4;
use thedas_render::{
    Animator, AreaRenderer, AreaScene, Clip, GpuContext, Prop, PropEffect, RenderCache,
};
use thedas_script::vm::ObjectId;
use thedas_sim::Sim;
use thedas_ui::models::{ModelCache, ObjectModel};
use thedas_vfs::install::TextureQuality;
use thedas_vfs::{DaoInstall, Vfs};

/// The longest step one frame advances the simulation (milliseconds): a stalled frame
/// (a level loading) does not leap the world ahead.
pub const MAX_STEP_MS: f64 = 100.0;

/// Where drawn object `id` is in the world as last placed (see
/// [`crate::cutactors::DrawnRoots`]): its placement's position and rotation.
fn drawn_root(
    posed: &[(ObjectId, Mat4, Option<Clip>)],
    id: ObjectId,
) -> Option<([f32; 3], [f32; 4])> {
    let (_, transform, _) = posed.iter().find(|p| p.0 == id)?;
    let (_, r, t) = transform.to_scale_rotation_translation();
    let r = r.normalize();
    (t.is_finite() && r.is_finite()).then(|| (t.to_array(), r.to_array()))
}

/// What the objects' scene was built for: each object's models. A change rebuilds it.
type ObjectsKey = Vec<(u32, Vec<String>)>;

/// The level of the current area, drawn first; built when the area's layout changes.
pub struct LiveLevel {
    pub layout: String,
    pub vfs: Arc<Vfs>,
    pub renderer: AreaRenderer,
    /// The `AREA_ID` whose colour grading the renderer has (several areas share a layout).
    pub area_id: Option<i64>,
    /// How high above the leader's feet the game camera's eye cuts roofs away
    /// ([`thedas_formats::level::Arl::cutaway_height`]).
    pub cutaway_height: f32,
    /// The cutaway value its renderer last got ([`Cutaway`]).
    cutaway_shown: f32,
    /// Its solid surfaces, which the game camera keeps on the party's side of.
    pub collider: thedas_render::collide::Collider,
}

/// The area's creatures and placeables, drawn over the level; rebuilt (alone) when an object
/// comes, goes or changes what it wears.
pub struct LiveObjects {
    key: ObjectsKey,
    /// Interactive placeables' bounds (centre offset from the object, radius), from their
    /// draws when the scene was built.
    bounds: std::collections::HashMap<ObjectId, (glam::Vec3, f32)>,
    /// Interactive placeables' surfaces as drawn, which a click must touch to pick them.
    shapes: std::collections::HashMap<ObjectId, thedas_render::collide::Collider>,
    /// Creatures' faces (their eyes', else their head's mesh centre), offset from the
    /// creature, from their draws when the scene was built: where portraits look.
    faces: std::collections::HashMap<ObjectId, glam::Vec3>,
    /// The placeables' opaque surfaces as drawn (doors, furniture), which the game camera
    /// keeps on the near side of, as it does the level's.
    solid: thedas_render::collide::Collider,
    /// Placeables that animate (doors), apart from `solid`: their surfaces as posed, rebuilt
    /// as they move. DAOrigins.exe's door collision is a shape under the leaf's mesh node
    /// (`plc_doorlgdwf_05_0.phy`: `DoorLow` > `cobj` > `shap`), so it swings with the leaf.
    moving: std::collections::HashMap<ObjectId, Moving>,
    /// Each prop's object and its own models (not what it holds), in the scene's order.
    bodies: Vec<(ObjectId, Vec<String>)>,
    pub scene: AreaScene,
    pub renderer: AreaRenderer,
    /// The cutaway value its renderer last got ([`Cutaway`]).
    cutaway_shown: f32,
}

/// An animating placeable's surfaces as last posed.
struct Moving {
    /// Its prop in the objects' scene.
    prop: usize,
    interactive: bool,
    /// Its draws' transforms when its surfaces were last built.
    worlds: Vec<Mat4>,
    solid: thedas_render::collide::Collider,
}

impl LiveObjects {
    /// Rebuilds the surfaces of animating placeables that moved since their last build.
    fn follow_poses(&mut self) {
        for (id, m) in &mut self.moving {
            let Some(p) = self.scene.props.get(m.prop) else {
                continue;
            };
            let worlds: Vec<Mat4> = p
                .draws
                .clone()
                .map(|d| self.renderer.draw_world(d).unwrap_or(Mat4::IDENTITY))
                .collect();
            if worlds == m.worlds {
                continue;
            }
            let at = |d: usize| self.renderer.draw_world(d);
            m.solid = thedas_render::collide::Collider::from_draws_at(
                &self.scene,
                p.draws.clone(),
                true,
                at,
            );
            if m.interactive {
                let shape = thedas_render::collide::Collider::from_draws_at(
                    &self.scene,
                    p.draws.clone(),
                    false,
                    at,
                );
                if shape.is_empty() {
                    self.shapes.remove(id);
                } else {
                    self.shapes.insert(*id, shape);
                }
            }
            m.worlds = worlds;
        }
    }
}

/// Which objects carry which visual effects: each with its models and its effects' keys.
type EffectsKey = Vec<(ObjectId, Vec<String>, Vec<String>)>;

/// The visual effects on the area's objects (a spell's glow, a crust of flames, an impact's
/// flash), drawn over the objects in a scene of their own, so they come and go without the
/// objects' scene being rebuilt. Their emitters sit on the objects' bodies and follow them.
pub struct LiveEffects {
    key: EffectsKey,
    /// The level they were built for.
    layout: String,
    /// The scene's props' objects, in its order.
    objects: Vec<ObjectId>,
    scene: AreaScene,
    renderer: AreaRenderer,
}

/// The selection rings under creatures, as a scene of their own drawn after the level and
/// before the creatures (see [`crate::rings`]).
pub struct LiveRings {
    /// Each ring's creature and model.
    key: Vec<(ObjectId, String)>,
    layout: String,
    scene: AreaScene,
    renderer: AreaRenderer,
}

pub struct Live {
    /// Shared with the interface, whose movies act on it as they send messages.
    pub sim: Arc<Mutex<Sim>>,
    /// The start menu's, before a game is chosen: nothing is loaded and nothing runs.
    pub front_end: bool,
    pub paused: bool,
    install: PathBuf,
    shaders: String,
    pub level: Option<LiveLevel>,
    pub objects: Option<LiveObjects>,
    effects: Option<LiveEffects>,
    rings: Option<LiveRings>,
    /// The creature the mouse is over ([`Live::set_hovered`]).
    hovered: Option<ObjectId>,
    /// When each lasting effect was first seen on its object (world milliseconds): the
    /// simulation does not keep when it was applied.
    effects_seen: std::collections::HashMap<(ObjectId, String), u64>,
    /// The level whose objects' effects have been looked at: those already on when a level
    /// comes up are taken as long since started.
    effects_layout: Option<String>,
    /// Poses the effects' scene (its skeletons are its own).
    effects_animator: Animator,
    animator: Animator,
    /// The creatures the game camera has faded (see [`crate::camfade`]) and the models'
    /// creature shapes read so far.
    camera_fade: crate::camfade::CameraFade,
    /// The roof cutaway's fade, and the layout it was last placed for.
    cutaway: (Cutaway, String),
    /// The area and whether a cutscene played, as the explore camera last saw them (see
    /// [`Live::camera_reset_due`]).
    camera_seen: Option<(Option<ObjectId>, bool)>,
    /// The last script zoom request the explore camera has taken (see
    /// [`Live::zoom_request_due`]).
    zoom_seen: Option<u32>,
    creature_shapes: std::collections::HashMap<String, Arc<Vec<crate::camfade::Shape>>>,
    /// A quickbar ability waiting for the player to click its target.
    pub aiming: Option<thedas_ui::input::Aim>,
    /// Each drawn object's id, placement and motion at the last sync (in the objects' scene
    /// order).
    posed: Vec<(ObjectId, Mat4, Option<Clip>)>,
    models: ModelCache,
    /// Placeables' state changes, for their swings (a door opening).
    swings: thedas_ui::models::PlaceableSwings,
    /// Objects drawn faded, by id: their opacity (0..1; missing is 1). The camera fades a
    /// creature it is inside (DAOrigins.exe 0x6c4130 eases it to 0.05).
    pub opacity: std::collections::HashMap<ObjectId, f32>,
    /// Textures, meshes, pipelines and shader translations of the current level, kept while
    /// its objects are rebuilt.
    cache: RenderCache,
    /// Simulation time not yet ticked (fractions of a millisecond).
    owed_ms: f64,
    /// Whether frames get the level's frame buffer effects.
    pub frame_effects: bool,
    /// The graphics detail level scenes are loaded at.
    pub detail: thedas_render::scene::Detail,
    /// The conversation camera's resources and current shot.
    talk_camera: crate::talkcam::TalkCamera,
    /// The playing cutscene's camera tracks.
    cutscene_camera: crate::cutcam::CutsceneCamera,
    /// The creatures cutscenes and conversation lines pose (see [`crate::cutactors`]).
    cut_actors: crate::cutactors::CutsceneActors,
    /// The creatures' head trackers, and the shaders' clock when they were last advanced.
    looks: crate::looks::Looks,
    looks_clock: Option<f64>,
    /// The shaders' clock (`ApplicationTime`), in seconds: real time, running while the
    /// world is stopped too.
    pub shader_time: f64,
    /// Character creation's stage (see [`crate::chargen_stage`]), once read; `Some(None)`
    /// when it could not be.
    stage: Option<Option<crate::chargen_stage::Stage>>,
    /// The creature drawn on the stage at the last sync, where it stands and its facing.
    staged: Option<(ObjectId, [f32; 3], f32)>,
    /// The stage camera framed for a preset, kept until the preset or the drawn creature
    /// changes (the executable frames a preset once, when it is set).
    stage_camera: Option<(i32, thedas_render::Camera)>,
    /// The stage camera last shown and its preset, the move to a new preset under way (the
    /// camera it left, when it left on the presentation clock), and that clock (ms).
    stage_shown: Option<(i32, thedas_render::Camera)>,
    stage_blend: Option<(thedas_render::Camera, f64)>,
    stage_clock_ms: f64,
    /// The playing cutscene's movies (see [`crate::intro::CutsceneMovies`]).
    cutscene_movies: crate::intro::CutsceneMovies,
    /// Whether a cutscene's movie waits for its decoder (a capture, whose frames must show
    /// what is due) rather than dropping frames it is late with (the window).
    pub movies_wait: bool,
    /// The format frames are drawn in, as the last sync gave it.
    format: Option<wgpu::TextureFormat>,
    /// Whether a new level and its objects are built on a worker thread while the window
    /// keeps presenting (a loading frame), rather than within one frame. The window sets it;
    /// a headless capture loads within the frame, as before.
    pub async_loads: bool,
    /// The frame size renderers are warmed for when they are built.
    pub frame_size: (u32, u32),
    /// The load under way while [`Live::async_loads`] is set (see [`Loading`]).
    loading: Option<Loading>,
    /// A layout whose build failed: not tried again until the area changes.
    failed_layout: Option<String>,
    /// Held while the game that replaces it opens on a worker (see [`Live::hold`]).
    held: bool,
    /// The area (its resource) whose world was last shown, for the loading screen a change
    /// of area gets (see [`Live::area_change_screen`]).
    shown_area: Option<String>,
    /// A journey on the world map is under way, latched until its area has loaded (see
    /// [`Live::travel_load`]).
    travel: bool,
    /// The last view of a cutscene that played over a conversation line, held for the rest
    /// of that line (see [`Live::conversation_camera`]).
    cut_held: Option<(TalkLine, thedas_render::Camera)>,
}

/// A conversation line shown: its dialog, owner and node.
type TalkLine = (String, ObjectId, Option<usize>);

/// The conversation line `sim` shows, if one is up.
fn talk_line(sim: &Sim) -> Option<TalkLine> {
    sim.conversation_line()
        .map(|o| (o.dialog.to_string(), o.owner, o.line))
}

/// The depth of field an open conversation runs
/// ([`thedas_render::cut_effects::conversation_depth_of_field`]), from the game's tables 203
/// and 204 (`conversation_dof_values`, `conversation_dof_parameters`). Provisional: taken
/// to run whenever a conversation line is up, the conversation game mode's own start and
/// end not being modelled.
fn conversation_depth_of_field(sim: &Sim) -> Option<thedas_render::cut_effects::DepthOfField> {
    use thedas_formats::gda::Cell;
    sim.conversation_line()?;
    let tables = sim.world.tables();
    let values = tables.get(203, "");
    let parameters = tables.get(204, "");
    let number = |c: Option<&Cell>| match c {
        Some(Cell::Float(v)) => Some(*v as f32),
        Some(Cell::Int(v)) => Some(*v as f32),
        _ => None,
    };
    // The first row whose focus distance is below zero holds the values.
    let row = values.as_ref().and_then(|t| {
        t.order
            .iter()
            .copied()
            .find(|&r| number(t.cell("FocusDistance", r)).is_some_and(|v| v < 0.0))
    });
    let listed = |name: &str| {
        parameters.as_ref().is_some_and(|t| {
            t.order.iter().any(|&r| {
                matches!(t.cell("Parameter", r), Some(Cell::String(s))
                    if s.trim_end_matches('\0').trim() == name)
            })
        })
    };
    Some(thedas_render::cut_effects::conversation_depth_of_field(
        |name| {
            let (t, row) = (values.as_ref()?, row?);
            listed(name).then(|| number(t.cell(name, row)))?
        },
    ))
}

/// A level, or a level's objects, being built on a worker thread. While one is under way
/// the world does not run ([`Live::step`]) and the window shows a loading frame instead of
/// the world; the simulation's own load is unchanged (it happened in its tick).
struct Loading {
    started: std::time::Instant,
    layout: String,
    /// The loading screen it shows (see [`Live::load_screen`]).
    screen: crate::ui::LoadScreen,
    building: Building,
    /// How long the level's and the objects' builds took (seconds).
    level_s: f64,
}

enum Building {
    Level(std::sync::mpsc::Receiver<Result<(LiveLevel, RenderCache)>>),
    Objects {
        key: ObjectsKey,
        models: Vec<ObjectModel>,
        rx: std::sync::mpsc::Receiver<Result<(AreaScene, AreaRenderer, RenderCache)>>,
    },
}

/// What a build needs from [`Live`], to run on another thread.
#[derive(Clone)]
struct BuildSpec {
    install: PathBuf,
    shaders: String,
    detail: thedas_render::scene::Detail,
    frame_effects: bool,
    format: wgpu::TextureFormat,
    size: (u32, u32),
}

/// A handle on the same device and queue, for a worker thread.
fn share(ctx: &GpuContext) -> GpuContext {
    GpuContext {
        instance: ctx.instance.clone(),
        adapter: ctx.adapter.clone(),
        device: ctx.device.clone(),
        queue: ctx.queue.clone(),
        bc: ctx.bc,
    }
}

/// Whether a world map journey is under way (see [`Live::travel_load`]): from the pin
/// clicked until arrival, but not while stopped at a random encounter.
fn journey(sim: &Sim) -> bool {
    let wm = &sim.world.world_map;
    (wm.travelling_to.is_some() || wm.arriving) && !wm.encounter
}

/// The area the party is in (its resource).
fn current_area(sim: &Sim) -> Option<String> {
    sim.world
        .area
        .and_then(|a| sim.world.get(a))
        .map(|a| a.template.clone())
}

/// An area's area list (the module's area tree, field 3291's second name).
fn area_list(sim: &Sim, resref: &str) -> Option<String> {
    sim.module_info
        .areas
        .iter()
        .find(|a| a.resref.eq_ignore_ascii_case(resref))
        .and_then(|a| a.area_list.clone())
}

/// load_light between areas of one area list, else load_town (the exe's loading sub-modes 5
/// and 4, picked at 0x6c7080).
fn load_screen_for(from: Option<&str>, to: Option<&str>) -> crate::ui::LoadScreen {
    match (from, to) {
        (Some(a), Some(b)) if a.eq_ignore_ascii_case(b) => crate::ui::LoadScreen::Light,
        _ => crate::ui::LoadScreen::Town,
    }
}

/// Builds a level's renderer and collider (see [`Live::sync`]).
fn build_level_with(
    ctx: &GpuContext,
    spec: &BuildSpec,
    layout: &str,
    cache: &mut RenderCache,
) -> Result<LiveLevel> {
    let started = std::time::Instant::now();
    let install = DaoInstall::open(&spec.install)?;
    let mut vfs = install.play_vfs(TextureQuality::High)?;
    install.mount_level(&mut vfs, layout)?;
    let scene = AreaScene::load_with_props_at(&vfs, layout, &[], spec.detail)?;
    let programs = crate::programs_cached(&vfs, &scene, &spec.shaders, cache)?;
    let mut renderer = AreaRenderer::new_cached(ctx, &vfs, &scene, &programs, spec.format, cache);
    renderer.frame_effects = spec.frame_effects;
    // First-use work at load, not on the first frames.
    renderer.warm(ctx, spec.size.0, spec.size.1);
    log::info!(
        "{layout}: level of {} draws built in {:.2}s",
        scene.draws.len(),
        started.elapsed().as_secs_f64()
    );
    let collider = thedas_render::collide::Collider::from_scene(&scene);
    Ok(LiveLevel {
        layout: layout.to_string(),
        vfs: Arc::new(vfs),
        cutaway_height: scene.cutaway_height.unwrap_or(5.5),
        cutaway_shown: 1.0,
        renderer,
        area_id: None,
        collider,
    })
}

/// Builds the renderer for a level's objects (see [`Live::finish_objects`]).
fn build_objects_with(
    ctx: &GpuContext,
    spec: &BuildSpec,
    vfs: &Vfs,
    layout: &str,
    models: &[ObjectModel],
    cache: &mut RenderCache,
) -> Result<(AreaScene, AreaRenderer)> {
    let started = std::time::Instant::now();
    let props: Vec<Prop> = models.iter().map(prop).collect();
    let scene =
        AreaScene::load_objects_cached(vfs, layout, &props, spec.detail, &mut cache.scenes)?;
    let loaded = started.elapsed();
    let programs = crate::programs_cached(vfs, &scene, &spec.shaders, cache)?;
    let translated = started.elapsed();
    let mut renderer = AreaRenderer::new_cached(ctx, vfs, &scene, &programs, spec.format, cache);
    let made = started.elapsed();
    renderer.warm(ctx, spec.size.0, spec.size.1);
    log::info!(
        "{layout}: {} objects, {} draws, built in {:.2}s (scene {:.0} ms, programs {:.0}, \
         renderer {:.0}, warm {:.0})",
        models.len(),
        scene.draws.len(),
        started.elapsed().as_secs_f64(),
        loaded.as_secs_f64() * 1000.0,
        (translated - loaded).as_secs_f64() * 1000.0,
        (made - translated).as_secs_f64() * 1000.0,
        (started.elapsed() - made).as_secs_f64() * 1000.0
    );
    Ok((scene, renderer))
}

/// How to start the game.
pub enum Start<'a> {
    /// A save from the MCP's save_state.
    Save(&'a Path),
    /// The module's start, through the character creation screens.
    NewGame,
    /// The module's start, through character creation's quick start (a pre-made character
    /// and origin, no screens).
    NewGameQuick,
    /// An area of the module, its objects as placed.
    Area(&'a str),
    /// No game: the start menu, before one is chosen (a simulation with no module loaded).
    Menu,
    /// A saved game's bytes (`Sim::save_json`), as the load screen picked it.
    SaveBytes(&'a [u8]),
}

/// Who gets a portrait drawn: the party, and the followers picked in an open party picker
/// (partypicker.gfx shows theirs).
pub fn portrait_subjects(sim: &Sim) -> Vec<ObjectId> {
    let mut v = thedas_sim::engine::party_of(sim);
    if let Some(p) = &sim.world.party_picker {
        for &m in &p.selected {
            if !v.contains(&m) {
                v.push(m);
            }
        }
    }
    v
}

impl Live {
    pub fn open(install: &Path, module: &str, start: Start, shaders: &str) -> Result<Self> {
        let opts = thedas_sim::session::SessionOptions {
            install: install.to_path_buf(),
            module: module.to_string(),
            area: match start {
                Start::Area(a) => Some(a.to_string()),
                _ => None,
            },
            waypoint: None,
            seed: 1,
            keep_trace: false,
            cutscenes: thedas_sim::sim::CutscenePolicy::Play,
            // A new game waits at character creation for the player's choices (chargen.gfx
            // sends them); quick start lets the chargen script choose.
            chargen: match start {
                Start::NewGame => thedas_sim::sim::ChargenPolicy::Manual,
                _ => thedas_sim::sim::ChargenPolicy::QuickStart,
            },
        };
        let mut sim = if matches!(start, Start::Menu) {
            thedas_sim::session::open_bare(&opts)?
        } else {
            thedas_sim::session::open(&opts)?
        };
        // The last orders, script log lines and the like, for issue reports (a debug read
        // only; see `crate::report`).
        sim.world.trace.keep_recent = crate::report::TRACE_RECENT;
        let save = match start {
            Start::Save(path) => Some((
                std::fs::read(path).with_context(|| format!("reading {}", path.display()))?,
                path.display().to_string(),
            )),
            Start::SaveBytes(b) => Some((b.to_vec(), "picked on the load screen".to_string())),
            _ => None,
        };
        if let Some((bytes, path)) = save {
            sim.load_json(&bytes)?;
            for issue in &sim.save_issues {
                log::warn!("save {path}: {issue}");
            }
            if sim.save_issues.iter().any(|i| !i.repaired) {
                log::warn!(
                    "save {path}: made by an older build that damaged character properties; \
                     starting a new game is advised"
                );
            }
        }
        // The player is in control (saves made by test drivers may have the autopilot on).
        sim.world.autopilot = false;
        // The player chooses the party when the scripts show the picker.
        sim.party_picker_policy = thedas_sim::sim::PartyPickerPolicy::Player;
        Ok(Self {
            sim: Arc::new(Mutex::new(sim)),
            front_end: matches!(start, Start::Menu),
            paused: false,
            install: install.to_path_buf(),
            shaders: shaders.to_string(),
            level: None,
            objects: None,
            effects: None,
            effects_seen: Default::default(),
            effects_layout: None,
            rings: None,
            hovered: None,
            effects_animator: Animator::default(),
            animator: Animator::default(),
            camera_fade: Default::default(),
            cutaway: Default::default(),
            camera_seen: None,
            zoom_seen: None,
            creature_shapes: Default::default(),
            aiming: None,
            posed: Vec::new(),
            models: ModelCache::default(),
            swings: Default::default(),
            opacity: Default::default(),
            cache: RenderCache::default(),
            async_loads: false,
            frame_size: (1280, 720),
            loading: None,
            failed_layout: None,
            held: false,
            shown_area: None,
            travel: false,
            cut_held: None,
            owed_ms: 0.0,
            frame_effects: true,
            detail: Default::default(),
            talk_camera: Default::default(),
            cutscene_camera: Default::default(),
            cut_actors: Default::default(),
            looks: Default::default(),
            looks_clock: None,
            shader_time: 0.0,
            stage: None,
            staged: None,
            stage_camera: None,
            stage_shown: None,
            stage_blend: None,
            stage_clock_ms: 0.0,
            cutscene_movies: Default::default(),
            movies_wait: false,
            format: None,
        })
    }

    /// The simulation, locked. Not held across the interface's tick, which locks it too.
    pub fn sim(&self) -> MutexGuard<'_, Sim> {
        self.sim.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Advances the simulation by `dt_ms` of real time (at most [`MAX_STEP_MS`]), unless
    /// paused.
    pub fn step(&mut self, dt_ms: f64) {
        if self.paused || self.front_end || self.loading() {
            return;
        }
        // A training tutorial holds the game's time until its mode ends, but its script
        // still runs: the exe runs `genev_tutorial` for each training event while the game
        // is paused (0x692e90 pauses the client and then posts the event), so the events
        // that fall due now are delivered without time passing, a tutorial popup up or not
        // (one left up would otherwise swallow the step's event: the Wilds' "Select
        // Alistair").
        if self.sim().training_paused() {
            self.sim().tick(0);
            return;
        }
        // A tutorial that pauses the game holds it until its popup is dismissed.
        if self.sim().world.tutorial_paused {
            return;
        }
        self.owed_ms += dt_ms.clamp(0.0, MAX_STEP_MS);
        let whole = self.owed_ms.floor();
        if whole >= 1.0 {
            self.owed_ms -= whole;
            let started = std::time::Instant::now();
            let mut sim = self.sim();
            let area = sim.world.area;
            sim.tick(whole as u64);
            if sim.world.area != area {
                // The frame the window stands still for: the simulation's own load.
                log::info!(
                    "area change: the simulation's step took {:.0} ms",
                    started.elapsed().as_secs_f64() * 1000.0
                );
            }
        }
    }

    /// Whether a new level is being built (see [`Live::async_loads`]): the world does not
    /// run, and the window shows a loading frame.
    pub fn loading(&self) -> bool {
        self.loading.is_some() || self.held
    }

    /// Whether a training lesson is under way: the game holds still for it, but its events
    /// are delivered even with a screen up (see [`Live::step`]).
    pub fn training(&self) -> bool {
        self.sim().training_paused()
    }

    /// Whether a script's area transition is waiting for the simulation to perform it
    /// (`World::transition`, due at `World::transition_at`): the loading screen goes up and
    /// the world is no longer drawn, as the exe's loading mode takes over before the old
    /// area goes (0x641d30); the simulation runs on to perform it. Only with
    /// [`Live::async_loads`]: a headless capture runs as before.
    pub fn transition_pending(&self) -> bool {
        self.async_loads && self.sim().world.transition.is_some()
    }

    /// The loading screen for the pending transition: load_light for an area of the same
    /// area list as this one, else load_town (the exe's loading sub-modes 5 and 4, picked at
    /// 0x6c7080 by comparing area lists). `None` when no transition is pending.
    pub fn pending_load_screen(&self) -> Option<crate::ui::LoadScreen> {
        if !self.async_loads {
            return None;
        }
        let sim = self.sim();
        let (to, _) = sim.world.transition.as_ref()?;
        let here = current_area(&sim);
        Some(load_screen_for(
            here.as_deref().and_then(|h| area_list(&sim, h)).as_deref(),
            area_list(&sim, to).as_deref(),
        ))
    }

    /// The loading screen a load now starting gets: a change of area from the one last shown
    /// to one of the same area list gets load_light, any other load (another list, a save, a
    /// new game, the first area) load_town. Taken from the change itself, so a transition
    /// performed in the very update that asked for it (no frame saw it pending) still gets
    /// its screen.
    fn area_change_screen(&self) -> crate::ui::LoadScreen {
        if self.held {
            return crate::ui::LoadScreen::Town;
        }
        let sim = self.sim();
        let now = current_area(&sim);
        let from = self.shown_area.as_deref().and_then(|a| area_list(&sim, a));
        let to = now.as_deref().and_then(|a| area_list(&sim, a));
        load_screen_for(from.as_deref(), to.as_deref())
    }

    /// Whether the load under way (or about to start) is a world map journey's: from the pin
    /// clicked (`WorldMapState::travelling_to`) or the travel transition asked for
    /// (`arriving`) until the new area's world is up, not while stopped at a random
    /// encounter. The exe keeps the world map up as that load's screen (GUI 16, 0x6c6060)
    /// rather than load_town or load_light. Only with [`Live::async_loads`].
    pub fn travel_load(&self) -> bool {
        self.async_loads && self.travel
    }

    /// Whether a world map journey is under way in the simulation (see `journey`), whatever
    /// the load mode: the world runs on behind the map for it, which otherwise stops the
    /// world, so the travel scripts carry the party on (headless captures too).
    pub fn journey(&self) -> bool {
        journey(&self.sim())
    }

    /// Latches [`Live::travel_load`]: set while the simulation says a journey is under way;
    /// it holds through the load that ends it, until the new area's world is shown.
    fn note_travel(&mut self, sim: &Sim) {
        if journey(sim) {
            self.travel = true;
        }
    }

    /// The loading screen up now: a pending transition's, else the load under way's.
    pub fn load_screen(&self) -> Option<crate::ui::LoadScreen> {
        if let Some(s) = self.pending_load_screen() {
            return Some(s);
        }
        if self.held {
            return Some(crate::ui::LoadScreen::Town);
        }
        self.loading.as_ref().map(|l| l.screen)
    }

    /// Holds this game while another opens to replace it (a save loading, a new game): it
    /// stops running and its world is no longer drawn, as behind a loading screen. Released
    /// (`false`) when the other failed to open, it carries on, rebuilding its world.
    pub fn hold(&mut self, held: bool) {
        self.held = held;
        if held {
            self.loading = None;
            self.level = None;
            self.objects = None;
            self.effects = None;
        }
    }

    /// The new level, once its worker has built it; starts the worker first if it is not
    /// building this layout. `None` while it builds, or when the build failed (logged; the
    /// world then shows nothing until the area changes, and the game stays usable).
    fn poll_level(
        &mut self,
        ctx: &GpuContext,
        format: wgpu::TextureFormat,
        layout: &str,
    ) -> Option<LiveLevel> {
        if self.failed_layout.as_deref() == Some(layout) {
            return None;
        }
        self.failed_layout = None;
        let building = matches!(&self.loading, Some(l) if l.layout == layout
            && matches!(l.building, Building::Level(_)));
        if !building {
            let screen = self.area_change_screen();
            // Drop the old level now: its world is gone.
            self.level = None;
            self.cache = RenderCache::default();
            self.animator = Animator::default();
            let (tx, rx) = std::sync::mpsc::channel();
            let (ctx, spec, name) = (share(ctx), self.spec(format), layout.to_string());
            let spawned = std::thread::Builder::new()
                .name("level-build".into())
                .spawn(move || {
                    let mut cache = RenderCache::default();
                    let built = build_level_with(&ctx, &spec, &name, &mut cache);
                    let _ = tx.send(built.map(|l| (l, cache)));
                });
            if let Err(e) = spawned {
                log::error!("{layout}: the level could not be built: {e}");
                self.failed_layout = Some(layout.to_string());
                self.loading = None;
                return None;
            }
            log::info!("{layout}: loading");
            self.loading = Some(Loading {
                started: std::time::Instant::now(),
                layout: layout.to_string(),
                building: Building::Level(rx),
                level_s: 0.0,
                screen,
            });
            return None;
        }
        let l = self.loading.as_mut()?;
        let Building::Level(rx) = &l.building else {
            return None;
        };
        match rx.try_recv() {
            Ok(Ok((level, cache))) => {
                self.cache = cache;
                l.level_s = l.started.elapsed().as_secs_f64();
                Some(level)
            }
            Ok(Err(e)) => {
                self.load_failed(layout, &format!("{e:#}"));
                None
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.load_failed(layout, "the build stopped");
                None
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
        }
    }

    /// The new level's objects, once the worker has built them (see [`Live::poll_level`]).
    fn poll_objects(
        &mut self,
        ctx: &GpuContext,
        format: wgpu::TextureFormat,
        layout: &str,
        models: &[ObjectModel],
        key: ObjectsKey,
    ) -> Option<LiveObjects> {
        let vfs = self.level.as_ref()?.vfs.clone();
        if !matches!(&self.loading, Some(l) if matches!(l.building, Building::Objects { .. })) {
            let (tx, rx) = std::sync::mpsc::channel();
            let (ctx, spec, name) = (share(ctx), self.spec(format), layout.to_string());
            let mut cache = std::mem::take(&mut self.cache);
            let owned = models.to_vec();
            let spawned = std::thread::Builder::new()
                .name("objects-build".into())
                .spawn(move || {
                    let built = build_objects_with(&ctx, &spec, &vfs, &name, &owned, &mut cache);
                    let _ = tx.send(built.map(|(scene, r)| (scene, r, cache)));
                });
            if let Err(e) = spawned {
                self.load_failed(layout, &e.to_string());
                return None;
            }
            let building = Building::Objects {
                key,
                models: models.to_vec(),
                rx,
            };
            match self.loading.as_mut() {
                Some(l) => l.building = building,
                // Objects alone (the level was already there): a load all the same.
                None => {
                    self.loading = Some(Loading {
                        started: std::time::Instant::now(),
                        layout: layout.to_string(),
                        building,
                        level_s: 0.0,
                        screen: crate::ui::LoadScreen::Town,
                    })
                }
            }
            return None;
        }
        let l = self.loading.as_ref()?;
        let Building::Objects { rx, .. } = &l.building else {
            return None;
        };
        match rx.try_recv() {
            Ok(Ok((scene, renderer, cache))) => {
                self.cache = cache;
                let l = self.loading.take()?;
                let Building::Objects { key, models, .. } = l.building else {
                    return None;
                };
                let objects = self.finish_objects(&models, key, scene, renderer);
                let total = l.started.elapsed().as_secs_f64();
                log::info!(
                    "{layout}: loaded in {total:.2}s (level {:.2}s, objects {:.2}s)",
                    l.level_s,
                    total - l.level_s
                );
                Some(objects)
            }
            Ok(Err(e)) => {
                self.load_failed(layout, &format!("{e:#}"));
                None
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.load_failed(layout, "the build stopped");
                None
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
        }
    }

    /// A worker's build failed: say so and leave the loading state, the world empty, rather
    /// than hold the loading frame.
    fn load_failed(&mut self, layout: &str, why: &str) {
        log::error!("{layout}: loading failed: {why}");
        self.loading = None;
        self.level = None;
        self.objects = None;
        self.effects = None;
        self.failed_layout = Some(layout.to_string());
    }

    /// Where a sphere of `radius` metres (0: a segment) moving from `from` to `to` first
    /// touches a solid surface (share of the way), if it does: the level's, or a
    /// placeable's as drawn (a door shut across a doorway). DAOrigins.exe's camera sweeps
    /// its static geometry and placeables (group mask 0x1024, 0x6c3758); provisional: these
    /// are their drawn triangles rather than their physics shapes.
    pub fn sweep(&self, from: glam::Vec3, to: glam::Vec3, radius: f32) -> Option<f32> {
        let level = self
            .level
            .as_ref()
            .and_then(|l| l.collider.sweep(from, to, radius));
        let objects = self.objects.as_ref().and_then(|o| {
            std::iter::once(&o.solid)
                .chain(o.moving.values().map(|m| &m.solid))
                .filter_map(|c| c.sweep(from, to, radius))
                .reduce(f32::min)
        });
        match (level, objects) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// The game camera this frame following the party leader (see [`crate::follow`]), when
    /// the leader is in the current area.
    pub fn follow_camera(
        &self,
        follow: &mut crate::follow::Follow,
        dt: f32,
        base: &thedas_render::Camera,
    ) -> Option<thedas_render::Camera> {
        let feet = self.leader_feet()?;
        let height = self.leader_look_height();
        Some(follow.camera_clear(feet, height, dt, base, &|a, b, r| self.sweep(a, b, r)))
    }

    /// The game camera's fade of the creatures it is inside (see [`crate::camfade`]), `dt`
    /// seconds after the last: `eye` is where the game camera stands, or `None` while it is
    /// not the view (a conversation or cutscene frames the shot: no creature starts fading
    /// then, as in the executable's `GM_DIALOG` and `GM_CUTSCENE`). Creatures faded out are
    /// drawn at their opacity ([`Live::opacity`]).
    pub fn fade_near_camera(&mut self, eye: Option<glam::Vec3>, dt: f32) {
        let blocked = {
            let sim = self.sim();
            sim.world.cutscene.is_some() || sim.conversation_line().is_some()
        };
        let eye = eye.filter(|_| !blocked);
        // Only creatures fade (the creatures group); their models' shapes are tested.
        let creatures: std::collections::HashSet<ObjectId> = match eye {
            Some(_) => self
                .sim()
                .world
                .objects
                .values()
                .filter(|o| o.kind == thedas_sim::world::ObjectKind::Creature)
                .map(|o| o.id)
                .collect(),
            None => Default::default(),
        };
        let mut touching = Vec::new();
        if let (Some(eye), Some(level), Some(objects)) =
            (eye, self.level.as_ref(), self.objects.as_ref())
        {
            for (i, (id, models)) in objects.bodies.iter().enumerate() {
                let Some((_, placement, _)) = self.posed.get(i).filter(|p| p.0 == *id) else {
                    continue;
                };
                // Cheap reject: creatures stand within a few metres of their origin.
                if placement.w_axis.truncate().distance(eye) > 4.0 {
                    continue;
                }
                if !creatures.contains(id) {
                    continue;
                }
                let touches = models.iter().any(|m| {
                    creature_shapes(&mut self.creature_shapes, level, m)
                        .iter()
                        .any(|s| s.touches(placement, eye, crate::camfade::FADE_RADIUS))
                });
                if touches {
                    touching.push(*id);
                }
            }
        }
        self.camera_fade.update(touching, dt);
        self.combine_opacity();
        // Roofs: cut away while the eye is high above the leader's feet.
        let high = match (eye, self.leader_feet(), self.level.as_ref()) {
            (Some(eye), Some(feet), Some(level)) => {
                if self.cutaway.1 != level.layout {
                    // A new area starts at its state at once (the explore camera's
                    // activation forces it, 0x771019).
                    self.cutaway = (Cutaway::default(), level.layout.clone());
                }
                Some(eye.z - feet.z > level.cutaway_height)
            }
            _ => None,
        };
        self.cutaway.0.update(high, dt);
    }

    /// Hands the roof cutaway's value to the renderers that have not got it yet.
    fn apply_cutaway(&mut self, ctx: &GpuContext) {
        let v = self.cutaway.0.shown();
        if let Some(l) = self.level.as_mut()
            && l.cutaway_shown != v
        {
            l.renderer.set_cutaway(ctx, v);
            l.cutaway_shown = v;
        }
        if let Some(o) = self.objects.as_mut()
            && o.cutaway_shown != v
        {
            o.renderer.set_cutaway(ctx, v);
            o.cutaway_shown = v;
        }
    }

    /// The leader's heading (radians) when the explore camera should reset behind it
    /// ([`crate::follow::Follow::reset`]): after a new area has loaded or a cutscene has
    /// ended, as the executable resets it at the end of loading and on leaving its cutscene
    /// and movie modes (0x6949c0's callers). Not after conversations.
    pub fn camera_reset_due(&mut self) -> Option<f32> {
        let (now, heading) = {
            let sim = self.sim();
            let w = &sim.world;
            let heading = w
                .controlled()
                .and_then(|id| w.get(id))
                .map(|o| thedas_sim::world::heading_of(o.facing).to_radians());
            ((w.area, w.cutscene.is_some()), heading)
        };
        let before = self.camera_seen.replace(now);
        let (area, cut) = before?;
        let due = area != now.0 || (cut && !now.1);
        if due {
            // The roof cutaway takes its state at once on the camera's reset too (the
            // activation forces it, 0x771019), not only in a new area.
            self.cutaway.0.high = None;
        }
        heading.filter(|_| due)
    }

    /// A script's new zoom request for the explore camera (`SetZoomLevel`'s level), once
    /// each. The request the world holds when it is first looked at (a loaded save's) is
    /// taken as already applied.
    pub fn zoom_request_due(&mut self) -> Option<f32> {
        let request = self.sim().world.zoom_request;
        let seq = request.map(|r| r.1);
        let before = self.zoom_seen.replace(seq.unwrap_or(0));
        match (before, request) {
            (Some(b), Some((level, s))) if s != b => Some(level),
            _ => None,
        }
    }

    /// Rebuilds [`Live::opacity`] from every fade that sets one, multiplied together (a
    /// missing entry is opaque). Each source keeps its own values; add another fade (a
    /// corpse's decay) here rather than writing `opacity` directly. Sources: the game
    /// camera's fade ([`crate::camfade`]).
    fn combine_opacity(&mut self) {
        self.opacity.clear();
        for (id, alpha) in self.camera_fade.faded() {
            *self.opacity.entry(id).or_insert(1.0) *= alpha;
        }
        // Corpses fading out at the end of their decay (see `thedas_sim::decay`).
        let corpses = self.sim().corpse_fades();
        for (id, alpha) in corpses {
            *self.opacity.entry(id).or_insert(1.0) *= alpha;
        }
        self.opacity.retain(|_, a| *a < 1.0);
    }

    /// How high above its feet the camera looks at the party leader: its appearance's
    /// `apr_base.gda` `HEIGHT`, or `HEIGHT_F` for a female (`CAppearance` 0x748a40 picks by
    /// gender 2), else [`crate::follow::DEFAULT_LOOK_HEIGHT`] (0x6a1a30 hands the camera
    /// the appearance's height).
    pub fn leader_look_height(&self) -> f32 {
        let leader = self.sim().world.controlled();
        leader.map_or(crate::follow::DEFAULT_LOOK_HEIGHT, |id| {
            self.creature_height(id)
        })
    }

    /// A creature's height: its appearance's `apr_base.gda` `HEIGHT`, or `HEIGHT_F` for a
    /// female, else [`crate::follow::DEFAULT_LOOK_HEIGHT`].
    pub fn creature_height(&self, id: ObjectId) -> f32 {
        appearance_height(&self.sim(), id)
    }
}

/// The models a drawn object is made of, for their `.phy` shapes: its own model list and
/// every model its draws come from (a creature's head and eyes are drawn from models not in
/// its list), lowercased without an extension.
fn body_models(scene: &AreaScene, prop: usize, own: &[String]) -> Vec<String> {
    let mut out: Vec<String> = own.iter().map(|m| m.to_ascii_lowercase()).collect();
    let Some(p) = scene.props.get(prop) else {
        return out;
    };
    let draws = p
        .draws
        .clone()
        .chain(p.attachments.iter().flat_map(|a| a.draws.clone()));
    for d in draws {
        let Some(d) = scene.draws.get(d) else {
            continue;
        };
        let name = d.model.to_ascii_lowercase();
        let name = name
            .strip_suffix(".mmh")
            .or_else(|| name.strip_suffix(".msh"))
            .unwrap_or(&name)
            .to_string();
        if !name.is_empty() && !out.contains(&name) {
            out.push(name);
        }
    }
    out
}

/// A creature's height from its appearance (see [`Live::creature_height`]), with the
/// simulation already in hand.
fn appearance_height(sim: &Sim, id: ObjectId) -> f32 {
    {
        let w = &sim.world;
        let height = w.get(id).and_then(|o| {
            let t = w.tables().get(0, "apr_base")?;
            let column = if o.fields.get("Gender").copied() == Some(2) {
                "HEIGHT_F"
            } else {
                "HEIGHT"
            };
            match t.cell(column, o.appearance as i64)? {
                thedas_formats::gda::Cell::Float(v) => Some(*v as f32),
                thedas_formats::gda::Cell::Int(v) => Some(*v as f32),
                _ => None,
            }
        });
        height
            .filter(|h| h.is_finite() && *h > 0.0)
            .unwrap_or(crate::follow::DEFAULT_LOOK_HEIGHT)
    }
}

impl Live {
    /// Advances what runs while the world may be stopped (the conversation camera's clock).
    pub fn advance_presentation(&mut self, dt_ms: f64) {
        self.talk_camera.advance(dt_ms);
        self.cut_actors.advance(dt_ms);
        if dt_ms.is_finite() && dt_ms > 0.0 {
            self.shader_time += dt_ms / 1000.0;
            self.stage_clock_ms += dt_ms;
        }
        self.follow_cutscene_movies();
    }

    /// Keeps the playing cutscene's movies in step with its clock. A cutscene whose movie
    /// cannot play ends now, as though the player skipped it, rather than standing on the
    /// world until they do (chargen, after the intro, then comes up on its own).
    fn follow_cutscene_movies(&mut self) {
        let mut sim = self.sim.lock().unwrap_or_else(|p| p.into_inner());
        let cutscene = sim
            .world
            .cutscene
            .as_ref()
            .map(|c| (c.name.clone(), c.elapsed_ms));
        let state = self.cutscene_movies.follow(
            cutscene.as_ref().map(|(n, ms)| (n.as_str(), *ms)),
            &sim.vfs,
            &self.install,
            self.movies_wait,
        );
        if state == crate::intro::MovieState::CannotPlay {
            sim.skip_cutscene();
            self.cutscene_movies
                .follow(None, &sim.vfs, &self.install, false);
        }
    }

    /// Gives the renderers the shaders' clock.
    fn set_shader_time(&mut self) {
        let t = self.shader_time;
        if let Some(l) = self.level.as_mut() {
            l.renderer.set_time(t);
        }
        if let Some(o) = self.objects.as_mut() {
            o.renderer.set_time(t);
        }
        if let Some(e) = self.effects.as_mut() {
            e.renderer.set_time(t);
        }
    }

    /// The open conversation's camera, when its lines are staged (see [`crate::talkcam`]);
    /// `base` gives the clip planes. `None` leaves the game camera as it is.
    pub fn conversation_camera(
        &mut self,
        base: &thedas_render::Camera,
    ) -> Option<thedas_render::Camera> {
        let sim = self.sim.lock().unwrap_or_else(|p| p.into_inner());
        // A cutscene that played over this line (the opening `start_wake.cut` over City
        // Elf's first line) leaves its last view for the rest of the line: the original
        // shows the line's replies in that view, not the line's stage camera. Provisional:
        // from the reference frame (reference/original/triage/city_elf_opening_shianni.png),
        // not traced in the exe.
        let line = talk_line(&sim);
        // Unless the line's stage has a `decision` camera to hand back to: Vaughan's line
        // cuts (44998-45005) end on the Soris/hero two-shot in the original, the stage's
        // decision camera, while City Elf's first line (its stage has none) keeps
        // start_wake's last view. Provisional, from the two reference sequences.
        let decision = self
            .talk_camera
            .staging(&sim, base)
            .is_some_and(|st| st.decision.is_some());
        if decision {
            self.cut_held = None;
        }
        match (&self.cut_held, &line) {
            (Some((held, view)), Some(now)) if held == now => {
                let mut c = *view;
                c.near = base.near;
                c.far = base.far;
                return Some(c);
            }
            _ => self.cut_held = None,
        }
        let faces = self.objects.as_ref().map(|o| &o.faces);
        let face = |id| faces.and_then(|f| f.get(&id).copied());
        let posed = &self.posed;
        self.talk_camera
            .camera(&sim, base, &face, &|id| drawn_root(posed, id))
    }

    /// The playing cutscene's camera (see [`crate::cutcam`]); `base` gives the clip planes.
    /// `None` leaves the game camera as it is.
    pub fn cutscene_camera(
        &mut self,
        base: &thedas_render::Camera,
    ) -> Option<thedas_render::Camera> {
        let sim = self.sim.lock().unwrap_or_else(|p| p.into_inner());
        let posed = &self.posed;
        let view = self
            .cutscene_camera
            .camera(&sim, base, &|id| drawn_root(posed, id));
        // Held for the conversation line it plays over (see `Live::conversation_camera`), when
        // that line is the one that played it: City Elf's first line plays its own cuts and
        // keeps their last view for its replies (reference city_elf_opening_shianni.png),
        // while Vaughan's line 40 comes up during line 39's 45005.cut and takes its own
        // stage camera (soris_decision, reference vaughan_scene/v_15). Provisional, from the
        // two reference sequences.
        let own = talk_line(&sim).filter(|l| l.2.is_some() && l.2 == sim.cutscene_line());
        if let (Some(v), Some(line)) = (view, own) {
            self.cut_held = Some((line, v));
        }
        view
    }

    /// The camera character creation's stage is seen through, while it is up (see
    /// [`crate::chargen_stage::camera`]): camera preset `preset` framing the creature at
    /// its mark. `base` gives the clip planes. `None` leaves the game camera as it is.
    pub fn chargen_camera(
        &mut self,
        base: &thedas_render::Camera,
        preset: i32,
    ) -> Option<thedas_render::Camera> {
        let (id, mark, _) = self.staged?;
        let target = match self.stage_camera {
            Some((p, c)) if p == preset => c,
            _ => {
                let head = self.head_now(id);
                let c =
                    crate::chargen_stage::camera(preset, glam::Vec3::from_array(mark), head, base);
                // Kept only once the creature is drawn (its head found).
                if head.is_some() || preset == crate::chargen_stage::PRESET_BODY {
                    self.stage_camera = Some((preset, c));
                }
                c
            }
        };
        // A new preset is moved to from where the camera was, as the exe blends (see
        // `crate::chargen_stage::BLEND_MS`).
        if let Some((p, shown)) = self.stage_shown
            && p != preset
        {
            self.stage_blend = Some((shown, self.stage_clock_ms));
        }
        let c = match self.stage_blend {
            Some((from, start)) => {
                let t = (self.stage_clock_ms - start) / crate::chargen_stage::BLEND_MS;
                if t >= 1.0 {
                    self.stage_blend = None;
                    target
                } else {
                    crate::chargen_stage::blend(&from, &target, t.max(0.0) as f32)
                }
            }
            None => target,
        };
        self.stage_shown = Some((preset, c));
        Some(c)
    }

    /// How far drawn creature `id`'s head bone stands above its feet, at rest: what
    /// DAOrigins.exe's creature height (vtable +0x1b0, 0x6a1a30) reads, the model's `head`
    /// node over the creature's position, or 1.75 m (0xb0f44c) without one. The floaties
    /// over a creature stand 0.1 m above it. Provisional: the exe reads the node's placement
    /// when it measures (and keeps the value, creature+0x7b0); the rest pose stands in.
    pub fn head_height(&self, id: ObjectId) -> f32 {
        const NO_HEAD: f32 = 1.75;
        let (Some(objects), Some(i)) = (
            self.objects.as_ref(),
            self.posed.iter().position(|p| p.0 == id),
        ) else {
            return NO_HEAD;
        };
        let transform = self.posed[i].1;
        objects
            .scene
            .prop_skeletons(i)
            .into_iter()
            .find_map(|s| {
                let skeleton = objects.scene.skeletons.get(s)?;
                let node = skeleton.node("head")?;
                let rest = skeleton.rest().get(node)?.w_axis.truncate();
                Some(transform.transform_point3(rest).z - transform.w_axis.z)
            })
            .unwrap_or(NO_HEAD)
    }

    /// Where drawn object `id`'s head bone is now (in the world), when it has one.
    fn head_now(&mut self, id: ObjectId) -> Option<glam::Vec3> {
        let objects = self.objects.as_ref()?;
        let level = self.level.as_ref()?;
        let i = self.posed.iter().position(|p| p.0 == id)?;
        let (_, transform, clip) = &self.posed[i];
        let animator = &mut self.animator;
        objects.scene.prop_skeletons(i).into_iter().find_map(|s| {
            let skeleton = objects.scene.skeletons.get(s)?;
            let node = skeleton.node("head")?;
            let rest = skeleton.rest().get(node)?.w_axis.truncate();
            let now = clip
                .as_ref()
                .and_then(|c| animator.pose(&level.vfs, &objects.scene, s, c))
                .and_then(|(_, pose)| pose.get(node).map(|m| m.w_axis.truncate()))
                .unwrap_or(rest);
            Some(transform.transform_point3(now))
        })
    }

    /// What each drawn creature is animating now: its id, the `.ani` resource, seconds into
    /// it (wrapped when looping), the animation's length, and where it stands. For the
    /// animations' sound events (see `crate::audio`).
    pub fn animation_clocks(&mut self) -> Vec<(ObjectId, String, f32, f32, [f32; 3])> {
        let Some(level) = self.level.as_ref() else {
            return Vec::new();
        };
        let mut out: Vec<(ObjectId, String, f32, f32, [f32; 3])> = Vec::new();
        for (id, transform, clip) in &self.posed {
            let Some(clip) = clip else {
                continue;
            };
            // An object drawn as several props (a body, a head) animates once.
            if out.iter().any(|o| o.0 == *id) {
                continue;
            }
            let Some((ani, anim)) = self.animator.animation_resource(&level.vfs, clip) else {
                continue;
            };
            let t = if clip.looping && anim.length > 0.0 {
                clip.time.rem_euclid(anim.length)
            } else {
                clip.time.min(anim.length)
            };
            out.push((
                *id,
                ani,
                t,
                anim.length,
                transform.w_axis.truncate().to_array(),
            ));
        }
        out
    }

    /// Where the party leader stands, when it is in the current area.
    pub fn leader_feet(&self) -> Option<glam::Vec3> {
        let sim = self.sim();
        let w = &sim.world;
        let o = w.get(w.controlled()?)?;
        (o.area.is_some() && o.area == w.area).then(|| glam::Vec3::from_array(o.position))
    }

    /// The hero's position and facing, when it is in the current area.
    pub fn hero(&self) -> Option<([f32; 3], f32)> {
        let sim = self.sim();
        let w = &sim.world;
        let h = w.get(w.hero?)?;
        (h.area.is_some() && h.area == w.area).then_some((h.position, h.facing))
    }

    /// Brings the drawn scenes up to the simulation: the level when the area's layout
    /// changes, the objects when the set of objects (or what they wear) changes; then places
    /// and poses every object.
    pub fn sync(&mut self, ctx: &GpuContext, format: wgpu::TextureFormat) -> Result<()> {
        self.format = Some(format);
        let shared = self.sim.clone();
        let mut sim = shared.lock().unwrap_or_else(|p| p.into_inner());
        self.note_travel(&sim);
        // Character creation shows its creature on its own stage, not in the area.
        let staged = match crate::chargen_stage::subject(&sim) {
            Some(id) => {
                let vfs = sim.vfs.clone();
                self.stage
                    .get_or_insert_with(|| {
                        let s = crate::chargen_stage::Stage::load(&vfs);
                        if s.is_none() {
                            log::warn!(
                                "{}.are or its {} waypoint not found: character creation \
                                 shows the area",
                                crate::chargen_stage::STAGE_AREA,
                                crate::chargen_stage::MARK
                            );
                        }
                        s
                    })
                    .clone()
                    .map(|s| (id, s))
            }
            None => None,
        };
        let layout = match &staged {
            Some((_, s)) => Some(s.layout.clone()),
            None => sim.world.area.and_then(|a| sim.area_layout(a)),
        };
        let Some(layout) = layout else {
            self.level = None;
            self.objects = None;
            self.effects = None;
            self.staged = None;
            return Ok(());
        };
        let area = sim.world.area.unwrap_or_default();
        if self.level.as_ref().map(|l| l.layout.as_str()) != Some(layout.as_str()) {
            self.objects = None;
            self.effects = None;
            if self.async_loads {
                drop(sim);
                match self.poll_level(ctx, format, &layout) {
                    Some(level) => self.level = Some(level),
                    None => return Ok(()),
                }
                sim = shared.lock().unwrap_or_else(|p| p.into_inner());
            } else {
                self.level = Some(self.build_level(ctx, format, &layout)?);
            }
        }
        // The area's colour grading (areadata.gda by its AREA_ID), swapped as areas change.
        let area_id = sim
            .world
            .get(area)
            .and_then(|a| a.locals.ints.get("AREA_ID").map(|&v| v as i64));
        if let Some(level) = self.level.as_mut()
            && level.area_id != area_id
        {
            let grade = area_id
                .map(|id| thedas_render::gpu::area_grade(&level.vfs, id))
                .unwrap_or_default();
            log::debug!("area {area_id:?}: colour grading {grade:?}");
            level.renderer.set_area_grade(ctx, &level.vfs, &grade);
            level.area_id = area_id;
        }
        let mut models = match &staged {
            Some((id, stage)) => stage_models(&sim, *id, stage, &mut self.models),
            None => {
                let mut m = thedas_ui::models::area_models_cached(&sim, area, &mut self.models);
                m.extend(thedas_ui::models::location_vfx_models(&sim, area));
                m
            }
        };
        self.swings.apply(&sim, &mut models);
        let was = self.staged;
        self.staged = staged.map(|(id, s)| (id, s.mark, s.facing));
        if self.staged.map(|s| s.0) != was.map(|s| s.0) {
            self.stage_camera = None;
            self.stage_shown = None;
            self.stage_blend = None;
        }
        // A cutscene playing, or a conversation line's own, poses its actors' creatures
        // and shows its own.
        let tracks = self.cutscene_camera.tracks(&sim);
        // Linked actors follow the objects as last placed.
        let posed = &self.posed;
        let shown = self
            .cut_actors
            .shown(&sim, tracks, &|id| drawn_root(posed, id));
        let mut posing = shown.actors;
        for (m, show) in shown.own {
            posing.insert(m.id, show);
            models.push(m);
        }
        let kept = shown.kept;
        // A cutscene's weapon draws and hidden actors.
        let mut hidden = std::collections::HashSet::new();
        for (id, show) in &posing {
            if show.hidden {
                hidden.insert(*id);
            }
            if let (Some((main, off)), Some(o), Some(m)) = (
                show.weapons,
                sim.world.get(*id),
                models.iter_mut().find(|m| m.id == *id),
            ) {
                m.attached = thedas_ui::models::weapons_in_cutscene(&sim, o, main, off);
            }
        }
        // Who looks at whom, from where the creatures were last drawn.
        self.looks.prepare(&sim);
        let pairs = crate::looks::pairs(&sim);
        drop(sim);
        let look_dt = self
            .looks_clock
            .map_or(0.0, |was| (self.shader_time - was).clamp(0.0, 0.25) as f32);
        self.looks_clock = Some(self.shader_time);
        // A cutscene's actors look where its head tracking actions say; the others as the
        // conversation has them.
        let wants: Vec<(ObjectId, Option<ObjectId>, f32)> = pairs
            .iter()
            .filter(|(from, _)| !posing.contains_key(from))
            .map(|&(from, to)| (from, Some(to), 1.0))
            .chain(posing.iter().filter_map(|(id, show)| {
                let look = show.look?;
                Some((*id, look.target, look.speed))
            }))
            .collect();
        let sights: std::collections::HashMap<ObjectId, crate::looks::Sight> = wants
            .iter()
            .filter_map(|&(from, to, speed)| {
                let place = |id| self.posed.iter().find(|p| p.0 == id).map(|p| p.1);
                let head = |id, t: Mat4| t.w_axis.truncate() + glam::Vec3::Z * self.head_height(id);
                let a = place(from)?;
                let sight = crate::looks::Sight {
                    origin: head(from, a),
                    base: glam::Quat::from_mat4(&a),
                    target: to.and_then(|to| Some(head(to, place(to)?))),
                    speed,
                };
                Some((from, sight))
            })
            .collect();
        let key = objects_key(&models);
        if self.async_loads && self.objects.is_none() {
            // A new level's objects are built on the worker too, before the world shows.
            match self.poll_objects(ctx, format, &layout, &models, key) {
                Some(o) => {
                    self.objects = Some(o);
                    self.stage_camera = None;
                }
                None => return Ok(()),
            }
        } else if self.objects.as_ref().map(|o| &o.key) != Some(&key) {
            self.objects = Some(self.build_objects(ctx, format, &layout, &models, key)?);
            // A new look is framed afresh.
            self.stage_camera = None;
        }
        // The world shown now is this area's (see `Live::area_change_screen`).
        if !self.loading() && self.level.is_some() && self.objects.is_some() {
            let (shown, travelling) = {
                let sim = self.sim();
                (current_area(&sim), journey(&sim))
            };
            self.shown_area = shown;
            // The journey's area is up: the world map's part as its loading screen is over.
            if !travelling {
                self.travel = false;
            }
        }
        let (Some(level), Some(objects)) = (self.level.as_ref(), self.objects.as_mut()) else {
            return Ok(());
        };
        let mut posed: Vec<(Mat4, Option<Clip>)> = Vec::with_capacity(models.len());
        for m in &models {
            let mut c = m.motion.as_ref().map(clip);
            let none = Vec::new();
            let held = kept.get(&m.id).unwrap_or(&none);
            let (at, facing) = match posing.get(&m.id) {
                _ if !posing.contains_key(&m.id) && !held.is_empty() => {
                    let base_trees = c.as_ref().map(|c| c.trees.clone()).unwrap_or_default();
                    for a in held {
                        let layer = cut_layer(&mut self.animator, &level.vfs, a, &base_trees);
                        match c.as_mut() {
                            Some(c) => c.layers.push(layer),
                            None => c = Some(layer),
                        }
                    }
                    (m.position, m.facing)
                }
                Some(show) => {
                    let mut at = show.position;
                    if show.snaps {
                        let p = glam::Vec3::from_array(at);
                        let (from, to) = (p + glam::Vec3::Z * 1.5, p - glam::Vec3::Z * 3.0);
                        if let Some(f) = level.collider.first_hit(from, to) {
                            at[2] = from.lerp(to, f).z;
                        }
                    }
                    log::trace!(
                        "cutscene actor {} {}: at {:?} (pose z {}) facing {}, {:?}",
                        m.id,
                        m.tag,
                        at,
                        show.position[2],
                        show.facing,
                        show.animations
                            .iter()
                            .map(|a| format!("{}@{:.2}", a.node, a.time))
                            .collect::<Vec<_>>()
                    );
                    let base_trees = c.as_ref().map(|c| c.trees.clone()).unwrap_or_default();
                    for a in held.iter().chain(&show.animations) {
                        let layer = cut_layer(&mut self.animator, &level.vfs, a, &base_trees);
                        match c.as_mut() {
                            Some(c) => c.layers.push(layer),
                            None => c = Some(layer),
                        }
                    }
                    (at, show.facing)
                }
                None => (m.position, m.facing),
            };
            // Head tracking: the pose animations of a creature that looks at another, or is
            // turning back from it.
            let sight = sights.get(&m.id).copied();
            if (sight.is_some() || self.looks.has(m.id))
                && let Some(c) = c.as_mut()
                && crate::looks::humanoid(&c.trees)
            {
                let animator = &mut self.animator;
                let layers = self.looks.step(m.id, sight, look_dt, |action| {
                    animator
                        .action_animation(&level.vfs, "humanoid_overlay", action)
                        .is_some()
                });
                c.layers.extend(layers);
            }
            posed.push((Prop::placement(at, facing), c));
        }
        self.animator.pose_props(
            &level.vfs,
            &objects.scene,
            &mut objects.renderer,
            ctx,
            &posed,
            0.0,
        );
        objects.follow_poses();
        for (i, m) in models.iter().enumerate() {
            let o = if hidden.contains(&m.id) {
                0.0
            } else {
                self.opacity.get(&m.id).copied().unwrap_or(1.0)
            };
            objects.renderer.set_prop_opacity(ctx, &objects.scene, i, o);
        }
        self.posed = models
            .iter()
            .zip(posed)
            .map(|(m, (t, c))| (m.id, t, c))
            .collect();
        self.show_effects(ctx, format, &layout, &models)?;
        self.show_rings(ctx, format, &layout, &models)
    }

    /// Shows the selection rings of the creatures drawn (see [`crate::rings`]): a scene of
    /// the rings' models, rebuilt when a ring comes, goes or changes, each moved under its
    /// creature every time.
    fn show_rings(
        &mut self,
        ctx: &GpuContext,
        format: wgpu::TextureFormat,
        layout: &str,
        models: &[ObjectModel],
    ) -> Result<()> {
        let hovered = self.hovered;
        let wanted: Vec<(ObjectId, String)> = {
            let sim = self.sim();
            models
                .iter()
                .filter_map(|m| {
                    let o = sim.world.get(m.id)?;
                    let row = crate::rings::row(&sim, o, hovered)?;
                    let model = crate::rings::model(&sim, row, sim.body_radius(m.id))?;
                    Some((m.id, model))
                })
                .collect()
        };
        if wanted.is_empty() {
            self.rings = None;
            return Ok(());
        }
        let current = self
            .rings
            .as_ref()
            .is_some_and(|r| r.key == wanted && r.layout == layout);
        if !current {
            let started = std::time::Instant::now();
            let ring_models: Vec<ObjectModel> = wanted
                .iter()
                .enumerate()
                .map(|(i, (id, model))| ObjectModel {
                    id: crate::rings::RING_IDS + i as ObjectId,
                    tag: format!("ring of {id}"),
                    kind: thedas_sim::world::ObjectKind::Placeable,
                    models: vec![model.clone()],
                    attached: Vec::new(),
                    tints: Vec::new(),
                    morph: None,
                    hide_hair: false,
                    effects: Vec::new(),
                    motion: None,
                    position: [0.0; 3],
                    facing: 0.0,
                })
                .collect();
            let vfs = self
                .level
                .as_ref()
                .map(|l| l.vfs.clone())
                .context("rings before their level")?;
            let spec = self.spec(format);
            let props: Vec<Prop> = ring_models.iter().map(prop).collect();
            let scene = AreaScene::load_objects_cached(
                &vfs,
                layout,
                &props,
                spec.detail,
                &mut self.cache.scenes,
            )?;
            let programs = crate::programs_cached(&vfs, &scene, &spec.shaders, &mut self.cache)?;
            let mut renderer = AreaRenderer::new_cached(
                ctx,
                &vfs,
                &scene,
                &programs,
                spec.format,
                &mut self.cache,
            );
            renderer.set_time(self.shader_time);
            log::debug!(
                "{layout}: {} rings, {} draws, built in {:.1} ms: {:?}",
                wanted.len(),
                scene.draws.len(),
                started.elapsed().as_secs_f64() * 1000.0,
                wanted
            );
            self.rings = Some(LiveRings {
                key: wanted,
                layout: layout.to_string(),
                scene,
                renderer,
            });
        }
        if let Some(rings) = self.rings.as_mut() {
            for (i, (id, _)) in rings.key.iter().enumerate() {
                if let Some((_, transform, _)) = self.posed.iter().find(|p| p.0 == *id) {
                    rings.renderer.move_prop(ctx, &rings.scene, i, *transform);
                }
            }
        }
        Ok(())
    }

    /// Builds the objects' visual effects' scene when the effects on them change, and places
    /// it on the objects as posed (see [`LiveEffects`]). An effect that carries over keeps
    /// its particles.
    fn show_effects(
        &mut self,
        ctx: &GpuContext,
        format: wgpu::TextureFormat,
        layout: &str,
        models: &[ObjectModel],
    ) -> Result<()> {
        /// How old an effect already on when its level came up is taken to be, in
        /// milliseconds: past any impact.
        const LONG_SINCE_MS: u64 = 3_600_000;
        let now = self.sim().world.time_ms;
        let with: Vec<&ObjectModel> = models.iter().filter(|m| !m.effects.is_empty()).collect();
        let fresh_level = self.effects_layout.as_deref() != Some(layout);
        self.effects_layout = Some(layout.to_string());
        self.effects_seen.retain(|(id, key), _| {
            with.iter()
                .any(|m| m.id == *id && m.effects.iter().any(|e| e.key == *key))
        });
        if with.is_empty() {
            self.effects = None;
            return Ok(());
        }
        let key: EffectsKey = with
            .iter()
            .map(|m| {
                let keys = m.effects.iter().map(|e| e.key.clone()).collect();
                (m.id, m.models.clone(), keys)
            })
            .collect();
        let current = self
            .effects
            .as_ref()
            .is_some_and(|e| e.key == key && e.layout == layout);
        if !current {
            let started = std::time::Instant::now();
            let first_seen = if fresh_level {
                now.saturating_sub(LONG_SINCE_MS)
            } else {
                now
            };
            let props: Vec<Prop> = with
                .iter()
                .map(|m| Prop {
                    label: format!("{} {}", m.id, m.tag),
                    models: m.models.clone(),
                    attached: Vec::new(),
                    morph: None,
                    hide_hair: false,
                    tints: Vec::new(),
                    effects: m
                        .effects
                        .iter()
                        .map(|e| {
                            let since = e.started_ms.unwrap_or_else(|| {
                                *self
                                    .effects_seen
                                    .entry((m.id, e.key.clone()))
                                    .or_insert(first_seen)
                            });
                            PropEffect {
                                model: e.model.clone(),
                                key: e.key.clone(),
                                age: now.saturating_sub(since) as f32 / 1000.0,
                                lasting: e.lasting,
                            }
                        })
                        .collect(),
                    transform: Prop::placement(m.position, m.facing),
                })
                .collect();
            let vfs = self
                .level
                .as_ref()
                .map(|l| l.vfs.clone())
                .context("effects before their level")?;
            let spec = self.spec(format);
            let like = &self
                .objects
                .as_ref()
                .context("effects before their objects")?
                .scene;
            let scene = AreaScene::load_effects_like(&vfs, like, &props);
            let loaded = started.elapsed();
            let programs = crate::programs_cached(&vfs, &scene, &spec.shaders, &mut self.cache)?;
            let translated = started.elapsed();
            let mut renderer = AreaRenderer::new_cached(
                ctx,
                &vfs,
                &scene,
                &programs,
                spec.format,
                &mut self.cache,
            );
            // The clock first (it runs the emitters on), then the particles carried over.
            renderer.set_time(self.shader_time);
            if let Some(old) = self.effects.take() {
                let was: std::collections::HashMap<&str, &thedas_render::particles::EmitterState> =
                    old.scene
                        .emitters
                        .iter()
                        .zip(&old.renderer.emitters)
                        .filter_map(|(e, s)| Some((e.owner.as_deref()?, s)))
                        .collect();
                for (e, s) in scene.emitters.iter().zip(renderer.emitters.iter_mut()) {
                    if let Some(o) = e.owner.as_deref().and_then(|k| was.get(k)) {
                        s.carry_on_from(o);
                    }
                }
            }
            self.effects_animator.forget_skeletons();
            log::debug!(
                "{layout}: effects on {} objects, {} emitters, built in {:.1} ms (scene {:.1}, \
                 programs {:.1})",
                props.len(),
                scene.emitters.len(),
                started.elapsed().as_secs_f64() * 1000.0,
                loaded.as_secs_f64() * 1000.0,
                (translated - loaded).as_secs_f64() * 1000.0
            );
            self.effects = Some(LiveEffects {
                key,
                layout: layout.to_string(),
                objects: with.iter().map(|m| m.id).collect(),
                scene,
                renderer,
            });
        }
        let (Some(level), Some(effects)) = (self.level.as_ref(), self.effects.as_mut()) else {
            return Ok(());
        };
        // The clock here, not only when a frame is drawn: the effects' timelines run on it.
        effects.renderer.set_time(self.shader_time);
        let posed: Vec<(Mat4, Option<Clip>)> = effects
            .objects
            .iter()
            .map(|id| {
                self.posed
                    .iter()
                    .find(|p| p.0 == *id)
                    .map_or((Mat4::IDENTITY, None), |p| (p.1, p.2.clone()))
            })
            .collect();
        self.effects_animator.pose_props(
            &level.vfs,
            &effects.scene,
            &mut effects.renderer,
            ctx,
            &posed,
            0.0,
        );
        Ok(())
    }

    /// Records the frame: the level, then the objects over it.
    pub fn render(
        &mut self,
        ctx: &GpuContext,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        (width, height): (u32, u32),
        camera: &thedas_render::Camera,
    ) -> bool {
        // A cutscene's movie is all that is drawn of the world while it is due.
        if self.cutscene_movies.showing() {
            let format = self.format.unwrap_or(thedas_render::gpu::CAPTURE_FORMAT);
            self.cutscene_movies.draw(
                &ctx.device,
                &ctx.queue,
                encoder,
                target,
                format,
                (width, height),
            );
            return true;
        }
        // Nothing of the world until its load is done (the level alone, before its objects,
        // is not shown), nor once a transition is on its way.
        if self.loading() || self.transition_pending() {
            return false;
        }
        self.set_shader_time();
        self.apply_cutaway(ctx);
        let Some(level) = self.level.as_mut() else {
            return false;
        };
        // Every renderer's shadow casters go into the partitions before anything is drawn.
        level.renderer.prepare_shadows(ctx, camera, width, height);
        if let Some(o) = self.objects.as_mut() {
            o.renderer.prepare_shadows(ctx, camera, width, height);
        }
        level.renderer.encode_shadows(encoder, true);
        if let Some(o) = self.objects.as_mut() {
            o.renderer.encode_shadows(encoder, false);
        }
        level.renderer.finish_shadows(encoder);
        level
            .renderer
            .render_main(ctx, encoder, target, width, height, camera);
        if let Some(r) = self.rings.as_mut() {
            let depth = level.renderer.depth(&ctx.device, width, height);
            r.renderer
                .render_over(ctx, encoder, target, width, height, camera, &depth);
        }
        if let Some(o) = self.objects.as_mut() {
            let depth = level.renderer.depth(&ctx.device, width, height);
            o.renderer
                .render_over(ctx, encoder, target, width, height, camera, &depth);
        }
        if let Some(e) = self.effects.as_mut() {
            let depth = level.renderer.depth(&ctx.device, width, height);
            e.renderer
                .render_over(ctx, encoder, target, width, height, camera, &depth);
        }
        true
    }

    /// The frame buffer effects the playing cutscene, or the shown conversation line's own
    /// cutscene, has running (their kind-19 tracks; see [`thedas_render::cut_effects`]):
    /// depth of field and colour fades, after the depth of field a conversation itself
    /// runs while it is open ([`conversation_depth_of_field`]). Its other effects
    /// (`da_motionblur`, and the bloom and colour matrix values a cutscene sets) are not
    /// drawn.
    fn cut_effects(&mut self) -> thedas_render::cut_effects::CutEffects {
        use thedas_render::cut_effects::{ColourFade, CutEffects, DepthOfField};
        let sim = self.sim.lock().unwrap_or_else(|p| p.into_inner());
        let talk = conversation_depth_of_field(&sim);
        let states = match sim.world.cutscene.as_ref() {
            Some(c) => {
                let t = c.elapsed_ms as f32 / 1000.0;
                self.cutscene_camera
                    .tracks(&sim)
                    .map(|tracks| tracks.frame_effects_at(t))
                    .unwrap_or_default()
            }
            None => self.talk_camera.line_cut_effects(&sim),
        };
        let mut out = CutEffects::default();
        out.depth_of_field.extend(talk);
        for s in &states {
            if s.effect.eq_ignore_ascii_case("da_dof") {
                out.depth_of_field
                    .push(DepthOfField::from_parameters(|name| {
                        s.parameter(name).map(|v| v[0])
                    }));
            } else if s.effect.eq_ignore_ascii_case("da_colorfade") {
                // The effect file's defaults for what the cutscene leaves out.
                out.fades.push(ColourFade {
                    start: s.parameter("StartColour").unwrap_or([0.0; 4]),
                    end: s.parameter("EndColour").unwrap_or([1.0; 4]),
                    time_to_fade: s.parameter("TimeToFade").map_or(6.0, |v| v[0]),
                    time: s.time,
                });
            }
        }
        out
    }

    /// Records the level's frame buffer effects over a frame [`Live::render`] drew into
    /// `target` (which needs `COPY_SRC` usage).
    pub fn finish_frame(
        &mut self,
        ctx: &GpuContext,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::Texture,
        (width, height): (u32, u32),
    ) {
        if self.cutscene_movies.showing() {
            return;
        }
        let cut_effects = self.cut_effects();
        if let Some(level) = self.level.as_mut() {
            level.renderer.cut_effects = cut_effects;
            let overlays: Vec<&AreaRenderer> = self
                .objects
                .iter()
                .map(|o| &o.renderer)
                .chain(self.effects.iter().map(|e| &e.renderer))
                .collect();
            level
                .renderer
                .finish_frame_with(ctx, encoder, target, width, height, &overlays);
        }
    }

    /// The frame as RGBA8 pixels, when there is a level to draw.
    pub fn capture(
        &mut self,
        ctx: &GpuContext,
        camera: &thedas_render::Camera,
        (width, height): (u32, u32),
    ) -> Result<Option<Vec<u8>>> {
        if self.cutscene_movies.showing() {
            return Ok(Some(self.cutscene_movies.rgba((width, height))));
        }
        // As the window shows it: nothing of the world while it loads.
        if self.loading() || self.transition_pending() {
            return Ok(None);
        }
        self.set_shader_time();
        self.apply_cutaway(ctx);
        let cut_effects = self.cut_effects();
        let Some(level) = self.level.as_mut() else {
            return Ok(None);
        };
        level.renderer.cut_effects = cut_effects;
        let mut overlays: Vec<&mut AreaRenderer> = self
            .rings
            .iter_mut()
            .map(|r| &mut r.renderer)
            .chain(self.objects.iter_mut().map(|o| &mut o.renderer))
            .chain(self.effects.iter_mut().map(|e| &mut e.renderer))
            .collect();
        Ok(Some(level.renderer.capture_with(
            ctx,
            camera,
            width,
            height,
            &mut overlays,
        )?))
    }

    fn spec(&self, format: wgpu::TextureFormat) -> BuildSpec {
        BuildSpec {
            install: self.install.clone(),
            shaders: self.shaders.clone(),
            detail: self.detail,
            frame_effects: self.frame_effects,
            format,
            size: self.frame_size,
        }
    }

    fn build_level(
        &mut self,
        ctx: &GpuContext,
        format: wgpu::TextureFormat,
        layout: &str,
    ) -> Result<LiveLevel> {
        // A new resource stack: nothing cached carries over.
        self.cache = RenderCache::default();
        self.animator = Animator::default();
        build_level_with(ctx, &self.spec(format), layout, &mut self.cache)
    }

    fn build_objects(
        &mut self,
        ctx: &GpuContext,
        format: wgpu::TextureFormat,
        layout: &str,
        models: &[ObjectModel],
        key: ObjectsKey,
    ) -> Result<LiveObjects> {
        let vfs = self
            .level
            .as_ref()
            .map(|l| l.vfs.clone())
            .context("objects before their level")?;
        let spec = self.spec(format);
        let (scene, renderer) =
            build_objects_with(ctx, &spec, &vfs, layout, models, &mut self.cache)?;
        Ok(self.finish_objects(models, key, scene, renderer))
    }

    /// The objects' picking, camera and portrait data, from their built scene.
    fn finish_objects(
        &mut self,
        models: &[ObjectModel],
        key: ObjectsKey,
        scene: AreaScene,
        renderer: AreaRenderer,
    ) -> LiveObjects {
        // A new scene means new skeleton indices; the animations read stay.
        self.animator.forget_skeletons();
        let mut bounds = std::collections::HashMap::new();
        let mut faces = std::collections::HashMap::new();
        let mut shapes = std::collections::HashMap::new();
        let mut placeable_draws = Vec::new();
        let mut moving = std::collections::HashMap::new();
        for (i, (m, p)) in models.iter().zip(&scene.props).enumerate() {
            let (interactive, placeable) = {
                let sim = self.sim();
                let placeable = sim
                    .world
                    .get(m.id)
                    .is_some_and(|o| o.kind == thedas_sim::world::ObjectKind::Placeable);
                let interactive = placeable && sim.world.get(m.id).is_some_and(|o| o.interactive);
                (interactive, placeable)
            };
            if placeable && m.motion.is_some() {
                moving.insert(
                    m.id,
                    Moving {
                        prop: i,
                        interactive,
                        worlds: Vec::new(),
                        solid: Default::default(),
                    },
                );
            } else if placeable {
                placeable_draws.extend(p.draws.clone());
            }
            if interactive {
                let shape =
                    thedas_render::collide::Collider::from_draws(&scene, p.draws.clone(), false);
                if !shape.is_empty() {
                    shapes.insert(m.id, shape);
                }
            }
            // A creature's eyes (`_uem_` meshes) or head (`_uhm_`). Provisional: from the
            // character models' names.
            let part = |tag: &str| {
                p.draws.clone().find_map(|d| {
                    let d = scene.draws.get(d)?;
                    d.model
                        .to_ascii_lowercase()
                        .contains(tag)
                        .then_some(d.sphere.0)
                })
            };
            if let Some(face) = part("_uem_").or_else(|| part("_uhm_")) {
                faces.insert(m.id, face - glam::Vec3::from_array(m.position));
            }
            let spheres: Vec<(glam::Vec3, f32)> = p
                .draws
                .clone()
                .filter_map(|d| scene.draws.get(d).map(|d| d.sphere))
                .collect();
            if spheres.is_empty() {
                continue;
            }
            let centre = spheres.iter().map(|s| s.0).sum::<glam::Vec3>() / spheres.len() as f32;
            let radius = spheres
                .iter()
                .map(|s| s.0.distance(centre) + s.1)
                .fold(0.0, f32::max);
            bounds.insert(m.id, (centre - glam::Vec3::from_array(m.position), radius));
        }
        let solid = thedas_render::collide::Collider::from_draws(&scene, placeable_draws, true);
        LiveObjects {
            key,
            bounds,
            shapes,
            faces,
            solid,
            moving,
            bodies: models
                .iter()
                .enumerate()
                .map(|(i, m)| (m.id, body_models(&scene, i, &m.models)))
                .collect(),
            scene,
            renderer,
            cutaway_shown: 1.0,
        }
    }
}

/// The light model and light probe model lighting the paperdoll (DAOrigins.exe's
/// `paperdoll_lightrig` and `paperdoll_lightprobe`, 0x7bbb30-0x7bbbb1).
const PAPERDOLL_LIGHT_RIG: &str = "el_lr_new_day2";
const PAPERDOLL_LIGHT_PROBE: &str = "el_lp_outside_day";

/// How much brighter portraits are than the scene they are drawn in. Provisional.
const PORTRAIT_GAIN: f32 = 1.8;

/// What a click lands on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Pick {
    Object(ObjectId),
    Ground([f32; 3]),
}

/// A model's creature shapes (its `.phy`'s creatures group, see [`crate::camfade`]), read
/// once per model and kept in `cache`.
fn creature_shapes(
    cache: &mut std::collections::HashMap<String, Arc<Vec<crate::camfade::Shape>>>,
    level: &LiveLevel,
    model: &str,
) -> Arc<Vec<crate::camfade::Shape>> {
    let key = model.to_ascii_lowercase();
    cache
        .entry(key.clone())
        .or_insert_with(|| {
            let read = |n: &str| level.vfs.read(n).ok().map(|(d, _)| d);
            let phy =
                read(&format!("{key}.phy")).and_then(|d| thedas_formats::phy::Phy::parse(&d).ok());
            let mmh = read(&format!("{key}.mmh"))
                .and_then(|d| thedas_formats::model::Mmh::parse(&d).ok());
            Arc::new(
                phy.map(|p| crate::camfade::model_shapes(&p, mmh.as_ref()))
                    .unwrap_or_default(),
            )
        })
        .clone()
}

/// Creatures are picked as upright cylinders this tall (metres). Provisional.
pub const PICK_HEIGHT: f32 = 1.9;
/// How far a click reaches into the scene (metres).
pub const PICK_REACH: f32 = 300.0;

/// Where a ray first meets an upright cylinder (base `p`, radius `r`, height `h`).
fn ray_cylinder(o: glam::Vec3, d: glam::Vec3, p: glam::Vec3, r: f32, h: f32) -> Option<f32> {
    let (ox, oy) = (o.x - p.x, o.y - p.y);
    let a = d.x * d.x + d.y * d.y;
    if a < 1e-9 {
        return None;
    }
    let b = 2.0 * (ox * d.x + oy * d.y);
    let c = ox * ox + oy * oy - r * r;
    let disc = b * b - 4.0 * a * c;
    if disc < 0.0 {
        return None;
    }
    let s = disc.sqrt();
    [(-b - s) / (2.0 * a), (-b + s) / (2.0 * a)]
        .into_iter()
        .filter(|&t| t > 0.0)
        .find(|&t| {
            let z = o.z + d.z * t;
            z >= p.z && z <= p.z + h
        })
}

/// Where a ray first meets a sphere.
fn ray_sphere(o: glam::Vec3, d: glam::Vec3, c: glam::Vec3, r: f32) -> Option<f32> {
    let oc = o - c;
    let b = oc.dot(d);
    let disc = b * b - (oc.length_squared() - r * r);
    if disc < 0.0 {
        return None;
    }
    let s = disc.sqrt();
    [-b - s, -b + s].into_iter().find(|&t| t > 0.0)
}

impl Live {
    /// What a ray from the camera (`origin`, unit `dir`) lands on: the nearest creature
    /// (not the party leader) or interactive placeable it passes through, else the ground.
    pub fn pick(&mut self, origin: glam::Vec3, dir: glam::Vec3) -> Option<Pick> {
        let shared = self.sim.clone();
        let mut sim = shared.lock().unwrap_or_else(|p| p.into_inner());
        let area = sim.world.area?;
        let leader = sim.world.controlled();
        let mut best: Option<(f32, ObjectId)> = None;
        for o in sim.world.objects.values() {
            if o.area != Some(area) || !o.active || Some(o.id) == leader {
                continue;
            }
            let p = glam::Vec3::from_array(o.position);
            let t = match o.kind {
                thedas_sim::world::ObjectKind::Creature => {
                    let r = sim.body_radius(o.id).max(0.35);
                    ray_cylinder(origin, dir, p, r, PICK_HEIGHT)
                }
                // A placeable is picked where the click meets its surfaces (a door's
                // bounding sphere reaches far past the door, over whoever stands by it);
                // one with nothing drawn, by its bounds.
                thedas_sim::world::ObjectKind::Placeable if o.interactive => {
                    let objects = self.objects.as_ref();
                    match objects.and_then(|s| s.shapes.get(&o.id)) {
                        Some(shape) => {
                            let len = dir.length().max(1e-6);
                            shape
                                .first_hit(origin, origin + dir * (PICK_REACH / len))
                                .map(|f| f * PICK_REACH / len)
                        }
                        None => objects
                            .and_then(|s| s.bounds.get(&o.id))
                            .and_then(|&(off, r)| ray_sphere(origin, dir, p + off, r)),
                    }
                }
                _ => None,
            };
            if let Some(t) = t {
                if best.is_none_or(|(b, _)| t < b) {
                    best = Some((t, o.id));
                }
            }
        }
        let ground = sim.ground_hit(area, origin.to_array(), dir.to_array(), 300.0);
        match (best, ground) {
            (Some((t, id)), Some(g)) if t <= origin.distance(glam::Vec3::from_array(g)) => {
                Some(Pick::Object(id))
            }
            (Some((_, id)), None) => Some(Pick::Object(id)),
            (_, Some(g)) => Some(Pick::Ground(g)),
            (None, None) => None,
        }
    }

    /// The creature the mouse is over (a ray from the camera, `origin` and unit `dir`): the
    /// nearest living creature in the area it passes through, the party leader included,
    /// unless the ground is nearer. DAOrigins.exe marks that creature (creature+0xa10, set
    /// by 0x712710 for the creature under the cursor and cleared on the one before), and a
    /// marked creature gets its stats floaty, its name over its head (0x6a52c0). Provisional:
    /// the exe's picker and its rule for several creatures under the cursor at once are not
    /// traced; the nearest stands in.
    pub fn hovered_creature(&mut self, origin: glam::Vec3, dir: glam::Vec3) -> Option<ObjectId> {
        let shared = self.sim.clone();
        let mut sim = shared.lock().unwrap_or_else(|p| p.into_inner());
        let area = sim.world.area?;
        let mut best: Option<(f32, ObjectId)> = None;
        for o in sim.world.objects.values() {
            if o.area != Some(area)
                || !o.active
                || o.dead
                || o.kind != thedas_sim::world::ObjectKind::Creature
            {
                continue;
            }
            // The models' creature shapes where the creature is drawn (see
            // [`crate::camfade`]); a cylinder for one without any.
            let posed = self
                .objects
                .as_ref()
                .and_then(|s| s.bodies.iter().position(|(id, _)| *id == o.id))
                .and_then(|i| self.posed.get(i).filter(|p| p.0 == o.id).map(|p| (i, p.1)));
            let mut t = None;
            if let (Some((i, placement)), Some(level), Some(objects)) =
                (posed, self.level.as_ref(), self.objects.as_ref())
            {
                for m in &objects.bodies[i].1 {
                    for shape in creature_shapes(&mut self.creature_shapes, level, m).iter() {
                        if let Some(h) = shape.ray(&placement, origin, dir) {
                            t = Some(t.map_or(h, |b: f32| b.min(h)));
                        }
                    }
                }
            }
            let shaped = posed.is_some_and(|(i, _)| {
                self.objects.as_ref().is_some_and(|objects| {
                    objects.bodies[i].1.iter().any(|m| {
                        self.creature_shapes
                            .get(&m.to_ascii_lowercase())
                            .is_some_and(|s| !s.is_empty())
                    })
                })
            });
            if !shaped {
                let r = sim.body_radius(o.id).max(0.35);
                t = ray_cylinder(
                    origin,
                    dir,
                    glam::Vec3::from_array(o.position),
                    r,
                    PICK_HEIGHT,
                );
            }
            if let Some(t) = t {
                if best.is_none_or(|(b, _)| t < b) {
                    best = Some((t, o.id));
                }
            }
        }
        let (t, id) = best?;
        match sim.ground_hit(area, origin.to_array(), dir.to_array(), 300.0) {
            Some(g) if origin.distance(glam::Vec3::from_array(g)) < t => None,
            _ => Some(id),
        }
    }

    /// Tells the world which creature the mouse is over, as the interface has it (none
    /// while the interface covers the cursor): its ring shows it (see [`crate::rings`]).
    pub fn set_hovered(&mut self, hovered: Option<ObjectId>) {
        self.hovered = hovered;
    }

    /// Orders the party leader as a click on `pick` does: lead with a party member, attack a
    /// living hostile, talk to another creature that has a conversation, use an interactive
    /// placeable, walk to a point on the ground. The order is an ordinary command queue; the simulation runs it.
    pub fn click(&mut self, pick: Pick) -> Option<String> {
        let shared = self.sim.clone();
        let mut sim = shared.lock().unwrap_or_else(|p| p.into_inner());
        // An ability waiting for its target takes the click: a creature is the target, or
        // for an ability aimed at places, a point on the ground; anything else lets it go.
        if let Some(aim) = self.aiming.take() {
            let target = match pick {
                Pick::Object(id) => sim
                    .world
                    .get(id)
                    .filter(|o| o.kind == thedas_sim::world::ObjectKind::Creature && !o.dead)
                    .map(|o| (id, o.tag.clone(), o.position)),
                Pick::Ground(_) => None,
            };
            let place = sim.ability_targets_place(aim.ability);
            let only_place = place
                && sim.ability_target_type(aim.ability) & !thedas_sim::quickbar::TARGET_PLACE == 0;
            return Some(match (target, pick) {
                // An ability only for places is cast at the creature's feet.
                (Some((_, tag, at)), _) if only_place => {
                    sim.order_ability_at(aim.caster, aim.ability, at, &aim.item);
                    format!("use ability {} at {tag}", aim.ability)
                }
                (Some((id, tag, _)), _) => {
                    sim.order_ability(aim.caster, aim.ability, id, &aim.item);
                    format!("use ability {} on {tag}", aim.ability)
                }
                (None, Pick::Ground(at)) if place => {
                    sim.order_ability_at(aim.caster, aim.ability, at, &aim.item);
                    format!("use ability {} at {:.1} {:.1}", aim.ability, at[0], at[1])
                }
                _ => format!("ability {} not aimed", aim.ability),
            });
        }
        // The party picker is open: a click on a follower picks or drops them.
        if sim.world.party_picker.is_some() {
            return Some(match pick {
                Pick::Object(id) if sim.party_picker_toggle(id) => {
                    let tag = sim.world.get(id).map(|o| o.tag.clone()).unwrap_or_default();
                    format!("party picker: {tag} picked or dropped")
                }
                _ => "party picker: nobody to pick there".to_string(),
            });
        }
        let leader = sim.world.controlled()?;
        match pick {
            Pick::Ground(p) => {
                sim.order_move(leader, p);
                sim.training_event("MOVEMENT_COMMAND_ISSUED", 0);
                Some(format!("move to {:.1} {:.1}", p[0], p[1]))
            }
            Pick::Object(id) => {
                // A party member is chosen to lead.
                let party = thedas_sim::engine::party_of(&sim);
                if let Some(n) = party.iter().position(|&m| m == id) {
                    drop(sim);
                    return self.choose_leader(n).map(|t| format!("lead with {t}"));
                }
                let o = sim.world.get(id)?;
                let (kind, dead, tag, talks) = (o.kind, o.dead, o.tag.clone(), o.dialog.is_some());
                match kind {
                    thedas_sim::world::ObjectKind::Creature if !dead && sim.hostile(leader, id) => {
                        sim.order_attack(leader, id);
                        Some(format!("attack {tag}"))
                    }
                    thedas_sim::world::ObjectKind::Creature if !dead && talks => {
                        sim.order_talk(leader, id);
                        Some(format!("talk to {tag}"))
                    }
                    thedas_sim::world::ObjectKind::Placeable => {
                        sim.order_use(leader, id);
                        Some(format!("use {tag}"))
                    }
                    _ => None,
                }
            }
        }
    }

    /// The aiming reticle on a `size` view with the mouse at `cursor` (pixels), while an
    /// ability aimed at places waits for its click: empty otherwise, or when the mouse is off
    /// the ground.
    pub fn reticle(
        &mut self,
        camera: &thedas_render::Camera,
        size: (u32, u32),
        cursor: (f64, f64),
    ) -> Vec<[glam::Vec2; 2]> {
        let Some(aim) = self.aiming.clone() else {
            return Vec::new();
        };
        let (origin, dir) = camera.ray(
            cursor.0 as f32,
            cursor.1 as f32,
            size.0 as f32,
            size.1 as f32,
        );
        let shared = self.sim.clone();
        let mut sim = shared.lock().unwrap_or_else(|p| p.into_inner());
        if !sim.ability_targets_place(aim.ability) {
            return Vec::new();
        }
        let (Some(area), Some(from)) = (
            sim.world.area,
            sim.world
                .get(aim.caster)
                .map(|o| glam::Vec3::from_array(o.position)),
        ) else {
            return Vec::new();
        };
        let Some(at) = sim.ground_hit(area, origin.to_array(), dir.to_array(), 300.0) else {
            return Vec::new();
        };
        let lines = crate::reticle::outline(
            sim.ability_area(aim.ability),
            from,
            glam::Vec3::from_array(at),
        );
        crate::reticle::project(camera, size, &lines)
    }

    /// Each party member's portrait: its head drawn from the camera `portraits.gda` gives it
    /// (by tag, else the `default` row), `size` pixels square, RGBA8. Members not drawn are
    /// left out. Provisional: the camera orbits the face by `CameraRotation` (degrees about
    /// the facing) and `CameraPitch`, `CameraDistance` away, looking at the eyes (or head)
    /// mesh, else 0.25 m below the top of the member's drawn bounds; other columns are
    /// unused.
    pub fn portraits(&mut self, ctx: &GpuContext, size: u32) -> Vec<(ObjectId, Vec<u8>)> {
        let staged = self.staged;
        let (Some(objects), Some(level)) = (self.objects.as_mut(), self.level.as_ref()) else {
            return Vec::new();
        };
        let animator = &mut self.animator;
        let posed = &self.posed;
        // Where the head bone is now, and where it is at rest (both in the world), for a drawn
        // object.
        let mut head_bone = |id: ObjectId| -> Option<(glam::Vec3, glam::Vec3)> {
            let i = posed.iter().position(|p| p.0 == id)?;
            let (_, transform, clip) = &posed[i];
            objects.scene.prop_skeletons(i).into_iter().find_map(|s| {
                let skeleton = objects.scene.skeletons.get(s)?;
                let node = skeleton.node("head")?;
                let rest = skeleton.rest().get(node)?.w_axis.truncate();
                let now = clip
                    .as_ref()
                    .and_then(|c| animator.pose(&level.vfs, &objects.scene, s, c))
                    .and_then(|(_, pose)| pose.get(node).map(|m| m.w_axis.truncate()))
                    .unwrap_or(rest);
                Some((
                    transform.transform_point3(now),
                    transform.transform_point3(rest),
                ))
            })
        };
        let shared = self.sim.clone();
        let sim = shared.lock().unwrap_or_else(|p| p.into_inner());
        let tables = sim.world.tables();
        let table = tables.get(0, "portraits");
        let row = |tag: &str| {
            let t = table.as_ref()?;
            t.order.iter().copied().find(|&r| {
                matches!(t.cell("Tag", r), Some(thedas_formats::gda::Cell::String(s))
                    if s.trim_end_matches('\0').trim().eq_ignore_ascii_case(tag))
            })
        };
        let num = |r: Option<i64>, c: &str, d: f32| {
            let t = table.as_ref();
            match r.and_then(|r| t?.cell(c, r).cloned()) {
                Some(thedas_formats::gda::Cell::Float(v)) => v as f32,
                Some(thedas_formats::gda::Cell::Int(v)) => v as f32,
                _ => d,
            }
        };
        let mut out = Vec::new();
        for id in portrait_subjects(&sim) {
            let Some(o) = sim.world.get(id) else { continue };
            if o.area.is_none() || o.area != sim.world.area {
                continue;
            }
            let Some(&(offset, radius)) = objects.bounds.get(&id) else {
                continue;
            };
            let face = objects.faces.get(&id).copied();
            let r = row(&o.tag).or_else(|| row("default"));
            let rotation = num(r, "CameraRotation", 0.0).to_radians();
            let pitch = num(r, "CameraPitch", 0.0).to_radians();
            let distance = num(r, "CameraDistance", 0.7).max(0.2);
            // On character creation's stage the creature stands at the stage's mark.
            let on_stage = staged.filter(|s| s.0 == id);
            let (position, facing) = on_stage.map_or((o.position, o.facing), |s| (s.1, s.2));
            let feet = glam::Vec3::from_array(position);
            let rest_face = match face {
                Some(f) => feet + f,
                None => feet + glam::Vec3::Z * (offset.z + radius - 0.25).max(0.3),
            };
            // The face follows the head bone as the creature moves (it crouches in combat). A
            // creature with no face mesh (the mabari) is looked at on its head bone: the top
            // of its bounds is its back, not its head.
            let head = match (face, head_bone(id)) {
                (Some(_), Some((now, rest))) => now + (rest_face - rest),
                (None, Some((now, _))) => now,
                (_, None) => rest_face,
            };
            let heading = thedas_sim::world::heading_of(facing).to_radians() + rotation;
            // From in front of the face, looking back at it.
            let eye = head
                + glam::Vec3::new(
                    heading.cos() * pitch.cos(),
                    heading.sin() * pitch.cos(),
                    pitch.sin(),
                ) * distance;
            let to = head - eye;
            let mut camera = thedas_render::Camera::new(
                eye,
                to.y.atan2(to.x),
                to.z.atan2(to.truncate().length()),
            );
            camera.fov_y = 35f32.to_radians();
            camera.near = 0.05;
            // The member alone, as the original's portrait shows them: drawing the whole
            // scene put a companion standing beside them into their portrait.
            let own: std::collections::HashSet<usize> = posed
                .iter()
                .position(|p| p.0 == id)
                .and_then(|i| objects.scene.props.get(i))
                .map(|p| p.draws.clone().collect())
                .unwrap_or_default();
            objects.renderer.only_draws = Some(own);
            // The creature on character creation's stage is lit by the portrait rig
            // (`portrait_lightrig` el_lr_new_day2 and `portrait_lightprobe`
            // el_lp_outside_day, 0x7f4037-0x7f40c0), turned to face the camera as the
            // paperdoll's is. Provisional: placed at the feet.
            if on_stage.is_some() {
                let rig = thedas_render::scene::LightRig::load(
                    &level.vfs,
                    PAPERDOLL_LIGHT_RIG,
                    PAPERDOLL_LIGHT_PROBE,
                    glam::Mat4::from_translation(feet)
                        * glam::Mat4::from_rotation_z(heading + std::f32::consts::FRAC_PI_2),
                );
                objects.renderer.set_light_rig(Some(rig));
            }
            // Captures come back RGBA whatever the renderer draws in.
            let shot = objects.renderer.capture(ctx, &camera, size, size);
            objects.renderer.only_draws = None;
            objects.renderer.set_light_rig(None);
            let Ok(mut pixels) = shot else {
                continue;
            };
            // The game lights portraits with a rig of their own (portraits.gda's light
            // columns); elsewhere the scene's light is brightened instead. Provisional.
            if on_stage.is_none() {
                for p in pixels.chunks_exact_mut(4) {
                    for c in &mut p[..3] {
                        *c = (*c as f32 * PORTRAIT_GAIN).min(255.0) as u8;
                    }
                }
            }
            out.push((id, pixels));
        }
        out
    }

    /// Party member `who` drawn whole for the inventory's and character record's paperdoll,
    /// `width` by `height` pixels, straight RGBA8 with a clear background, turned `yaw`
    /// radians from facing the viewer. DAOrigins.exe draws it into the render target its
    /// `GUIPaperdoll` makes for those screens (270 by 459, 0x7312ee / 0x73c310; the target is
    /// named `paperdoll`, 0x7bc659), in a scene of its own lit by `paperdoll_lightrig`
    /// (el_lr_new_day2) and `paperdoll_lightprobe` (el_lp_outside_day) (0x7bbb30-0x7bbbb1).
    /// The member is drawn where it stands, alone, lit by that rig and probe (in place of the
    /// level's lights), from a level camera framing its height. Provisional: the game's
    /// camera presets are not used, and the rig is placed at the member's feet turned to face
    /// the viewer. The coverage comes from drawing over black and over white.
    pub fn paperdoll(
        &mut self,
        ctx: &GpuContext,
        who: ObjectId,
        yaw: f32,
        width: u32,
        height: u32,
    ) -> Option<Vec<u8>> {
        let vfs = self.level.as_ref()?.vfs.clone();
        let objects = self.objects.as_mut()?;
        let (feet, facing) = {
            let sim = self.sim.lock().unwrap_or_else(|p| p.into_inner());
            let o = sim.world.get(who)?;
            if o.area.is_none() || o.area != sim.world.area {
                return None;
            }
            (
                glam::Vec3::from_array(o.position),
                thedas_sim::world::heading_of(o.facing).to_radians(),
            )
        };
        let &(offset, radius) = objects.bounds.get(&who)?;
        // The member stands in the picture from head to foot: its height is about twice its
        // bounds' centre above its feet (the sphere's radius takes in what it holds out).
        let tall = (offset.z * 2.0).clamp(0.5, radius * 2.0);
        let centre = feet + glam::Vec3::Z * (tall * 0.5);
        let fov = 30f32.to_radians();
        let distance = (tall * 0.56 / (fov * 0.5).tan()).max(0.5);
        let heading = facing + yaw;
        let eye = centre + glam::Vec3::new(heading.cos(), heading.sin(), 0.0) * distance;
        let to = centre - eye;
        let mut camera =
            thedas_render::Camera::new(eye, to.y.atan2(to.x), to.z.atan2(to.truncate().length()));
        camera.fov_y = fov;
        camera.near = 0.05;
        let own: std::collections::HashSet<usize> = self
            .posed
            .iter()
            .position(|p| p.0 == who)
            .and_then(|i| objects.scene.props.get(i))
            .map(|p| p.draws.clone().collect())
            .unwrap_or_default();
        objects.renderer.only_draws = Some(own);
        // The rig faces its subject along its −Y (its key light in front, its rim behind);
        // here it turns with the viewer, so the light stays on the side facing the picture.
        let rig = thedas_render::scene::LightRig::load(
            &vfs,
            PAPERDOLL_LIGHT_RIG,
            PAPERDOLL_LIGHT_PROBE,
            glam::Mat4::from_translation(feet)
                * glam::Mat4::from_rotation_z(heading + std::f32::consts::FRAC_PI_2),
        );
        objects.renderer.set_light_rig(Some(rig));
        let black = objects.renderer.capture(ctx, &camera, width, height);
        objects.renderer.clear = wgpu::Color::WHITE;
        let white = objects.renderer.capture(ctx, &camera, width, height);
        objects.renderer.clear = wgpu::Color::BLACK;
        objects.renderer.only_draws = None;
        objects.renderer.set_light_rig(None);
        let (black, white) = (black.ok()?, white.ok()?);
        let mut out = vec![0u8; black.len()];
        for ((o, b), w) in out
            .chunks_exact_mut(4)
            .zip(black.chunks_exact(4))
            .zip(white.chunks_exact(4))
        {
            // Over black a pixel is c·a; over white c·a + (1 - a).
            let a = 1.0
                - (0..3)
                    .map(|i| (w[i] as f32 - b[i] as f32) / 255.0)
                    .sum::<f32>()
                    / 3.0;
            let a = a.clamp(0.0, 1.0);
            if a <= 0.0 {
                continue;
            }
            for i in 0..3 {
                o[i] = (b[i] as f32 / a).min(255.0) as u8;
            }
            o[3] = (a * 255.0).round() as u8;
        }
        Some(out)
    }

    /// The player's pause key (Space). During a training tutorial the game may not be
    /// unpaused: the key is the training action `EVENT_TYPE_TRAINING_UNPAUSE` instead (see
    /// `thedas_sim::training`), and the player's own pause is left as it was.
    pub fn toggle_pause(&mut self) {
        let mut sim = self.sim();
        if sim.training_paused() {
            sim.training_event("UNPAUSE", 0);
            return;
        }
        drop(sim);
        self.paused = !self.paused;
    }

    /// Makes party member `n` (0 is the hero) the leader the player controls.
    pub fn choose_leader(&mut self, n: usize) -> Option<String> {
        let mut sim = self.sim();
        let party = thedas_sim::engine::party_of(&sim);
        let id = *party.get(n)?;
        // The simulation learns who the player controls, so that member's AI stands down.
        sim.player_selects(id);
        sim.world.get(id).map(|o| o.tag.clone())
    }
}

fn objects_key(models: &[ObjectModel]) -> ObjectsKey {
    models
        .iter()
        .map(|m| {
            let mut v = m.models.clone();
            v.extend(m.attached.iter().map(|a| format!("{}@{}", a.model, a.node)));
            v.extend(m.morph.iter().map(|x| format!("{x}.mor{}", m.hide_hair)));
            v.extend(m.tints.iter().map(|(a, t)| format!("{a}:{t}")));
            (m.id, v)
        })
        .collect()
}

/// What the character creation stage draws: creature `id` alone, standing at the stage's
/// mark with its facing (0x7bb9d6-0x7bba2d). The simulation builds the new game's hero from
/// the `default_player` template (0x69d953, #188), so it is drawn from the first page.
fn stage_models(
    sim: &Sim,
    id: ObjectId,
    stage: &crate::chargen_stage::Stage,
    cache: &mut ModelCache,
) -> Vec<ObjectModel> {
    let Some(o) = sim.world.get(id) else {
        return Vec::new();
    };
    let drawn = o.area.and_then(|area| {
        thedas_ui::models::area_models_cached(sim, area, cache)
            .into_iter()
            .find(|m| m.id == id)
    });
    let Some(mut m) = drawn else {
        return Vec::new();
    };
    m.position = stage.mark;
    m.facing = stage.facing;
    vec![m]
}

/// An object's models as a scene prop.
pub fn prop(m: &ObjectModel) -> Prop {
    Prop {
        label: format!("{} {}", m.id, m.tag),
        models: m.models.clone(),
        attached: m
            .attached
            .iter()
            .map(|a| (a.model.clone(), a.node.clone()))
            .collect(),
        morph: m.morph.clone(),
        hide_hair: m.hide_hair,
        tints: m.tints.clone(),
        // Drawn in a scene of their own (see `Live::show_effects`).
        effects: Vec::new(),
        transform: Prop::placement(m.position, m.facing),
    }
}

/// A creature's motion as an animation clip.
/// A cutscene animation as a layer over a creature's own motion (whose trees are `trees`):
/// it loops when those trees loop its animation, else when its track spans the whole
/// cutscene (see [`crate::cutactors`]).
fn cut_layer(
    animator: &mut Animator,
    vfs: &Vfs,
    a: &crate::cutactors::CutAnimation,
    trees: &[String],
) -> Clip {
    let once = animator
        .action_animation(vfs, &a.tree, &a.node)
        .and_then(|ani| animator.plays_once(vfs, trees, &ani));
    Clip {
        trees: vec![a.tree.clone()],
        actions: vec![a.node.clone()],
        time: a.time,
        looping: once.map_or(a.unbounded, |o| !o),
        layers: Vec::new(),
        absolute: true,
        ..Default::default()
    }
}

pub fn clip(m: &thedas_ui::models::Motion) -> Clip {
    Clip {
        trees: m.trees.clone(),
        actions: m.actions.clone(),
        time: m.time,
        looping: m.looping,
        layers: Vec::new(),
        absolute: false,
        ..Default::default()
    }
}

/// The roof cutaway (DAOrigins.exe 0x76f5a0, 0x640250, 0x641f90): meshes flagged `CutAway`
/// fade out over 0.2 s on a cosine ease while the game camera's eye is more than the
/// layout's cutaway height above the leader's feet, and back in when it is not.
/// Conversations and cutscenes show them at once and restore the value afterwards
/// (0x6403f0). A new area takes its state at once.
#[derive(Clone, Copy, Debug)]
pub struct Cutaway {
    value: f32,
    from: f32,
    target: f32,
    elapsed: f32,
    /// Whether the camera was high when last checked; `None` before the first check.
    high: Option<bool>,
    /// A conversation or cutscene frames the shot: shown at 1.
    held: bool,
}

impl Default for Cutaway {
    fn default() -> Self {
        Cutaway {
            value: 1.0,
            from: 1.0,
            target: 1.0,
            elapsed: 0.0,
            high: None,
            held: false,
        }
    }
}

impl Cutaway {
    const DURATION: f32 = 0.2;

    /// Advances by `dt` seconds; `high` is whether the eye is high, `None` while the game
    /// camera is not the view.
    pub fn update(&mut self, high: Option<bool>, dt: f32) {
        let Some(high) = high else {
            self.held = true;
            return;
        };
        self.held = false;
        let target = if high { 0.0 } else { 1.0 };
        match self.high {
            None => {
                self.value = target;
                self.from = target;
                self.target = target;
                self.elapsed = Self::DURATION;
            }
            Some(was) if was != high => {
                self.from = self.value;
                self.target = target;
                self.elapsed = 0.0;
            }
            _ => {}
        }
        self.high = Some(high);
        self.elapsed = (self.elapsed + dt.max(0.0)).min(Self::DURATION);
        let t = self.elapsed / Self::DURATION;
        let ease = (1.0 - (std::f32::consts::PI * t).cos()) * 0.5;
        self.value = self.from + (self.target - self.from) * ease;
    }

    /// The value the cut-away meshes draw with: 1 as made, 0 not drawn.
    pub fn shown(&self) -> f32 {
        if self.held { 1.0 } else { self.value }
    }
}

#[cfg(test)]
mod cutaway_tests {
    use super::Cutaway;

    #[test]
    fn a_new_area_takes_its_state_at_once_then_fades_over_a_fifth_of_a_second() {
        let mut c = Cutaway::default();
        c.update(Some(true), 0.0);
        assert_eq!(c.shown(), 0.0);
        c.update(Some(false), 0.1);
        assert!((c.shown() - 0.5).abs() < 1e-5);
        c.update(Some(false), 0.1);
        assert_eq!(c.shown(), 1.0);
    }

    #[test]
    fn conversations_show_roofs_and_restore_the_cut() {
        let mut c = Cutaway::default();
        c.update(Some(true), 0.0);
        c.update(None, 0.5);
        assert_eq!(c.shown(), 1.0);
        c.update(Some(true), 0.0);
        assert_eq!(c.shown(), 0.0);
    }
}

#[cfg(test)]
mod load_screen_tests {
    use super::load_screen_for;
    use crate::ui::LoadScreen;

    #[test]
    fn same_area_list_gets_the_light_screen() {
        let list = Some("al_bec01al_alienage");
        assert_eq!(load_screen_for(list, list), LoadScreen::Light);
        assert_eq!(
            load_screen_for(list, Some("AL_BEC01AL_ALIENAGE")),
            LoadScreen::Light
        );
        assert_eq!(
            load_screen_for(Some("al_bdc02al_slums"), Some("al_bdc01al_commons")),
            LoadScreen::Town
        );
        // No area before (the first load), or an area outside the tree.
        assert_eq!(load_screen_for(None, list), LoadScreen::Town);
        assert_eq!(load_screen_for(list, None), LoadScreen::Town);
    }
}
