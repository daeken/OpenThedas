//! A running game inside the window: the simulation ticks at real time (unless paused), and
//! the area it is in is drawn with its creatures and placeables posed from it. The
//! simulation is only read here; input reaches it as commands elsewhere.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use anyhow::{Context, Result};
use glam::Mat4;
use thedas_render::{Animator, AreaRenderer, AreaScene, Clip, GpuContext, Prop, RenderCache};
use thedas_script::vm::ObjectId;
use thedas_sim::Sim;
use thedas_ui::models::{ModelCache, ObjectModel};
use thedas_vfs::install::TextureQuality;
use thedas_vfs::{DaoInstall, Vfs};

/// The longest step one frame advances the simulation (milliseconds): a stalled frame
/// (a level loading) does not leap the world ahead.
pub const MAX_STEP_MS: f64 = 100.0;

/// What the objects' scene was built for: each object's models. A change rebuilds it.
type ObjectsKey = Vec<(u32, Vec<String>)>;

/// The level of the current area, drawn first; built when the area's layout changes.
pub struct LiveLevel {
    pub layout: String,
    pub vfs: Arc<Vfs>,
    pub renderer: AreaRenderer,
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
    pub scene: AreaScene,
    pub renderer: AreaRenderer,
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
    animator: Animator,
    /// A quickbar ability waiting for the player to click its target.
    pub aiming: Option<thedas_ui::input::Aim>,
    /// Each drawn object's id, placement and motion at the last sync (in the objects' scene
    /// order).
    posed: Vec<(ObjectId, Mat4, Option<Clip>)>,
    models: ModelCache,
    /// Placeables' state changes, for their swings (a door opening).
    swings: thedas_ui::models::PlaceableSwings,
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
            animator: Animator::default(),
            aiming: None,
            posed: Vec::new(),
            models: ModelCache::default(),
            swings: Default::default(),
            cache: RenderCache::default(),
            owed_ms: 0.0,
            frame_effects: true,
            detail: Default::default(),
            talk_camera: Default::default(),
            cutscene_camera: Default::default(),
            cut_actors: Default::default(),
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
        if self.paused || self.front_end {
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
            self.sim().tick(whole as u64);
        }
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

    /// How high above its feet the camera looks at the party leader: its appearance's
    /// `apr_base.gda` `HEIGHT`, or `HEIGHT_F` for a female (`CAppearance` 0x748a40 picks by
    /// gender 2), else [`crate::follow::DEFAULT_LOOK_HEIGHT`] (0x6a1a30 hands the camera
    /// the appearance's height).
    pub fn leader_look_height(&self) -> f32 {
        let sim = self.sim();
        let w = &sim.world;
        let height = w.controlled().and_then(|id| w.get(id)).and_then(|o| {
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
    }

    /// The open conversation's camera, when its lines are staged (see [`crate::talkcam`]);
    /// `base` gives the clip planes. `None` leaves the game camera as it is.
    pub fn conversation_camera(
        &mut self,
        base: &thedas_render::Camera,
    ) -> Option<thedas_render::Camera> {
        let sim = self.sim.lock().unwrap_or_else(|p| p.into_inner());
        let faces = self.objects.as_ref().map(|o| &o.faces);
        let face = |id| faces.and_then(|f| f.get(&id).copied());
        self.talk_camera.camera(&sim, base, &face)
    }

    /// The playing cutscene's camera (see [`crate::cutcam`]); `base` gives the clip planes.
    /// `None` leaves the game camera as it is.
    pub fn cutscene_camera(
        &mut self,
        base: &thedas_render::Camera,
    ) -> Option<thedas_render::Camera> {
        let sim = self.sim.lock().unwrap_or_else(|p| p.into_inner());
        self.cutscene_camera.camera(&sim, base)
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
            self.staged = None;
            return Ok(());
        };
        let area = sim.world.area.unwrap_or_default();
        if self.level.as_ref().map(|l| l.layout.as_str()) != Some(layout.as_str()) {
            self.objects = None;
            self.level = Some(self.build_level(ctx, format, &layout)?);
        }
        let mut models = match &staged {
            Some((id, stage)) => stage_models(&sim, *id, stage, &mut self.models),
            None => thedas_ui::models::area_models_cached(&sim, area, &mut self.models),
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
        let shown = self.cut_actors.shown(&sim, tracks);
        let mut posing = shown.actors;
        for (m, show) in shown.own {
            posing.insert(m.id, show);
            models.push(m);
        }
        let kept = shown.kept;
        drop(sim);
        let key = objects_key(&models);
        if self.objects.as_ref().map(|o| &o.key) != Some(&key) {
            self.objects = Some(self.build_objects(ctx, format, &layout, &models, key)?);
            // A new look is framed afresh.
            self.stage_camera = None;
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
        self.posed = models
            .iter()
            .zip(posed)
            .map(|(m, (t, c))| (m.id, t, c))
            .collect();
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
        self.set_shader_time();
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
        if let Some(o) = self.objects.as_mut() {
            let depth = level.renderer.depth(&ctx.device, width, height);
            o.renderer
                .render_over(ctx, encoder, target, width, height, camera, &depth);
        }
        true
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
        if let Some(level) = self.level.as_mut() {
            level
                .renderer
                .finish_frame(ctx, encoder, target, width, height);
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
        self.set_shader_time();
        let Some(level) = self.level.as_mut() else {
            return Ok(None);
        };
        let mut overlays: Vec<&mut AreaRenderer> =
            self.objects.iter_mut().map(|o| &mut o.renderer).collect();
        Ok(Some(level.renderer.capture_with(
            ctx,
            camera,
            width,
            height,
            &mut overlays,
        )?))
    }

    fn build_level(
        &mut self,
        ctx: &GpuContext,
        format: wgpu::TextureFormat,
        layout: &str,
    ) -> Result<LiveLevel> {
        let started = std::time::Instant::now();
        let install = DaoInstall::open(&self.install)?;
        let mut vfs = install.global_vfs(TextureQuality::High)?;
        install.mount_level(&mut vfs, layout)?;
        let scene = AreaScene::load_with_props_at(&vfs, layout, &[], self.detail)?;
        // A new resource stack: nothing cached carries over.
        self.cache = RenderCache::default();
        let programs = crate::programs_cached(&vfs, &scene, &self.shaders, &mut self.cache)?;
        let mut renderer =
            AreaRenderer::new_cached(ctx, &vfs, &scene, &programs, format, &mut self.cache);
        renderer.frame_effects = self.frame_effects;
        log::info!(
            "{layout}: level of {} draws built in {:.2}s",
            scene.draws.len(),
            started.elapsed().as_secs_f64()
        );
        let collider = thedas_render::collide::Collider::from_scene(&scene);
        Ok(LiveLevel {
            layout: layout.to_string(),
            vfs: Arc::new(vfs),
            renderer,
            collider,
        })
    }

    fn build_objects(
        &mut self,
        ctx: &GpuContext,
        format: wgpu::TextureFormat,
        layout: &str,
        models: &[ObjectModel],
        key: ObjectsKey,
    ) -> Result<LiveObjects> {
        let started = std::time::Instant::now();
        let vfs = self
            .level
            .as_ref()
            .map(|l| l.vfs.clone())
            .context("objects before their level")?;
        let props: Vec<Prop> = models.iter().map(prop).collect();
        let scene = AreaScene::load_objects_at(&vfs, layout, &props, self.detail)?;
        let programs = crate::programs_cached(&vfs, &scene, &self.shaders, &mut self.cache)?;
        let renderer =
            AreaRenderer::new_cached(ctx, &vfs, &scene, &programs, format, &mut self.cache);
        log::info!(
            "{layout}: {} objects, {} draws, built in {:.2}s",
            models.len(),
            scene.draws.len(),
            started.elapsed().as_secs_f64()
        );
        // A new scene means new skeleton indices.
        self.animator = Animator::default();
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
        Ok(LiveObjects {
            key,
            bounds,
            shapes,
            faces,
            solid,
            moving,
            scene,
            renderer,
        })
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

    /// Makes party member `n` (0 is the hero) the leader the player controls.
    pub fn choose_leader(&mut self, n: usize) -> Option<String> {
        let mut sim = self.sim();
        let party = thedas_sim::engine::party_of(&sim);
        let id = *party.get(n)?;
        // The simulation learns who the player controls, so that member's AI stands down.
        sim.set_controlled(Some(id));
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
    }
}
