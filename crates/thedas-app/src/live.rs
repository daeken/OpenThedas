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
    pub scene: AreaScene,
    pub renderer: AreaRenderer,
}

pub struct Live {
    /// Shared with the interface, whose movies act on it as they send messages.
    pub sim: Arc<Mutex<Sim>>,
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
    /// The shaders' clock (`ApplicationTime`), in seconds: real time, running while the
    /// world is stopped too.
    pub shader_time: f64,
}

/// How to start the game.
pub enum Start<'a> {
    /// A save from the MCP's save_state.
    Save(&'a Path),
    /// The module's start, through character creation's quick start.
    NewGame,
    /// An area of the module, its objects as placed.
    Area(&'a str),
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
        };
        let mut sim = thedas_sim::session::open(&opts)?;
        if let Start::Save(path) = start {
            let bytes =
                std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
            sim.load_json(&bytes)?;
            for issue in &sim.save_issues {
                log::warn!("save {}: {issue}", path.display());
            }
            if sim.save_issues.iter().any(|i| !i.repaired) {
                log::warn!(
                    "save {}: made by an older build that damaged character properties; \
                     starting a new game is advised",
                    path.display()
                );
            }
        }
        // The player is in control (saves made by test drivers may have the autopilot on).
        sim.world.autopilot = false;
        Ok(Self {
            sim: Arc::new(Mutex::new(sim)),
            paused: false,
            install: install.to_path_buf(),
            shaders: shaders.to_string(),
            level: None,
            objects: None,
            animator: Animator::default(),
            aiming: None,
            posed: Vec::new(),
            models: ModelCache::default(),
            cache: RenderCache::default(),
            owed_ms: 0.0,
            frame_effects: true,
            detail: Default::default(),
            talk_camera: Default::default(),
            shader_time: 0.0,
        })
    }

    /// The simulation, locked. Not held across the interface's tick, which locks it too.
    pub fn sim(&self) -> MutexGuard<'_, Sim> {
        self.sim.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Advances the simulation by `dt_ms` of real time (at most [`MAX_STEP_MS`]), unless
    /// paused.
    pub fn step(&mut self, dt_ms: f64) {
        if self.paused {
            return;
        }
        self.owed_ms += dt_ms.clamp(0.0, MAX_STEP_MS);
        let whole = self.owed_ms.floor();
        if whole >= 1.0 {
            self.owed_ms -= whole;
            self.sim().tick(whole as u64);
        }
    }

    /// Where the party leader stands, when it is in the current area.
    /// Where a segment first meets the level's solid surfaces (share of the way), if it does.
    pub fn first_hit(&self, from: glam::Vec3, to: glam::Vec3) -> Option<f32> {
        self.level.as_ref()?.collider.first_hit(from, to)
    }

    /// Advances what runs while the world may be stopped (the conversation camera's clock).
    pub fn advance_presentation(&mut self, dt_ms: f64) {
        self.talk_camera.advance(dt_ms);
        if dt_ms.is_finite() && dt_ms > 0.0 {
            self.shader_time += dt_ms / 1000.0;
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
        self.talk_camera.camera(&sim, base)
    }

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
        let shared = self.sim.clone();
        let mut sim = shared.lock().unwrap_or_else(|p| p.into_inner());
        let Some(layout) = sim.world.area.and_then(|a| sim.area_layout(a)) else {
            self.level = None;
            self.objects = None;
            return Ok(());
        };
        let area = sim.world.area.unwrap_or_default();
        if self.level.as_ref().map(|l| l.layout.as_str()) != Some(layout.as_str()) {
            self.objects = None;
            self.level = Some(self.build_level(ctx, format, &layout)?);
        }
        let models = thedas_ui::models::area_models_cached(&sim, area, &mut self.models);
        // A staged conversation line draws its actors at their stage marks.
        let base = thedas_render::Camera::new(glam::Vec3::ZERO, 0.0, 0.0);
        let marks = self
            .talk_camera
            .staging(&sim, &base)
            .map(|s| s.marks.clone())
            .unwrap_or_default();
        drop(sim);
        let key = objects_key(&models);
        if self.objects.as_ref().map(|o| &o.key) != Some(&key) {
            self.objects = Some(self.build_objects(ctx, format, &layout, &models, key)?);
        }
        let (Some(level), Some(objects)) = (self.level.as_ref(), self.objects.as_mut()) else {
            return Ok(());
        };
        let posed: Vec<(Mat4, Option<Clip>)> = models
            .iter()
            .map(|m| {
                let (at, facing) = marks.get(&m.id).copied().unwrap_or((m.position, m.facing));
                (Prop::placement(at, facing), m.motion.as_ref().map(clip))
            })
            .collect();
        self.animator.pose_props(
            &level.vfs,
            &objects.scene,
            &mut objects.renderer,
            ctx,
            &posed,
            0.0,
        );
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
        self.set_shader_time();
        let Some(level) = self.level.as_mut() else {
            return false;
        };
        // Every renderer's shadow casters go into the partitions before anything is drawn.
        level
            .renderer
            .encode_shadows(ctx, encoder, camera, width, height, true);
        if let Some(o) = self.objects.as_mut() {
            o.renderer
                .encode_shadows(ctx, encoder, camera, width, height, false);
        }
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
        for (m, p) in models.iter().zip(&scene.props) {
            let interactive = {
                let sim = self.sim();
                sim.world.get(m.id).is_some_and(|o| {
                    o.interactive && o.kind == thedas_sim::world::ObjectKind::Placeable
                })
            };
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
        Ok(LiveObjects {
            key,
            bounds,
            shapes,
            faces,
            scene,
            renderer,
        })
    }
}

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
        for id in thedas_sim::engine::party_of(&sim) {
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
            let feet = glam::Vec3::from_array(o.position);
            let rest_face = match face {
                Some(f) => feet + f,
                None => feet + glam::Vec3::Z * (offset.z + radius - 0.25).max(0.3),
            };
            // The face follows the head bone as the creature moves (it crouches in combat).
            let head = match head_bone(id) {
                Some((now, rest)) => now + (rest_face - rest),
                None => rest_face,
            };
            let heading = thedas_sim::world::heading_of(o.facing).to_radians() + rotation;
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
            // Captures come back RGBA whatever the renderer draws in.
            let Ok(mut pixels) = objects.renderer.capture(ctx, &camera, size, size) else {
                continue;
            };
            // The game lights portraits with a rig of their own (portraits.gda's light
            // columns); the scene's light is brightened instead. Provisional.
            for p in pixels.chunks_exact_mut(4) {
                for c in &mut p[..3] {
                    *c = (*c as f32 * PORTRAIT_GAIN).min(255.0) as u8;
                }
            }
            out.push((id, pixels));
        }
        out
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
pub fn clip(m: &thedas_ui::models::Motion) -> Clip {
    Clip {
        trees: m.trees.clone(),
        actions: m.actions.clone(),
        time: m.time,
        looping: m.looping,
    }
}
