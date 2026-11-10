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
    layout: String,
    pub vfs: Arc<Vfs>,
    pub renderer: AreaRenderer,
}

/// The area's creatures and placeables, drawn over the level; rebuilt (alone) when an object
/// comes, goes or changes what it wears.
pub struct LiveObjects {
    key: ObjectsKey,
    /// Interactive placeables' bounds (centre offset from the object, radius), from their
    /// draws when the scene was built.
    bounds: std::collections::HashMap<ObjectId, (glam::Vec3, f32)>,
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
    models: ModelCache,
    /// Textures, meshes, pipelines and shader translations of the current level, kept while
    /// its objects are rebuilt.
    cache: RenderCache,
    /// Simulation time not yet ticked (fractions of a millisecond).
    owed_ms: f64,
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
        };
        let mut sim = thedas_sim::session::open(&opts)?;
        if let Start::Save(path) = start {
            let bytes =
                std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
            sim.load_json(&bytes)?;
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
            models: ModelCache::default(),
            cache: RenderCache::default(),
            owed_ms: 0.0,
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
                (
                    Prop::placement(m.position, m.facing),
                    m.motion.as_ref().map(clip),
                )
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
        let Some(level) = self.level.as_mut() else {
            return false;
        };
        level
            .renderer
            .render(ctx, encoder, target, width, height, camera);
        if let Some(o) = self.objects.as_mut() {
            let depth = level.renderer.depth(&ctx.device, width, height);
            o.renderer
                .render_over(ctx, encoder, target, width, height, camera, &depth);
        }
        true
    }

    /// The frame as RGBA8 pixels, when there is a level to draw.
    pub fn capture(
        &mut self,
        ctx: &GpuContext,
        camera: &thedas_render::Camera,
        (width, height): (u32, u32),
    ) -> Result<Option<Vec<u8>>> {
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
        let scene = AreaScene::load(&vfs, layout)?;
        // A new resource stack: nothing cached carries over.
        self.cache = RenderCache::default();
        let programs = crate::programs_cached(&vfs, &scene, &self.shaders, &mut self.cache)?;
        let renderer =
            AreaRenderer::new_cached(ctx, &vfs, &scene, &programs, format, &mut self.cache);
        log::info!(
            "{layout}: level of {} draws built in {:.2}s",
            scene.draws.len(),
            started.elapsed().as_secs_f64()
        );
        Ok(LiveLevel {
            layout: layout.to_string(),
            vfs: Arc::new(vfs),
            renderer,
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
        let scene = AreaScene::load_objects(&vfs, layout, &props)?;
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
        for (m, p) in models.iter().zip(&scene.props) {
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
            scene,
            renderer,
        })
    }
}

/// What a click lands on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Pick {
    Object(ObjectId),
    Ground([f32; 3]),
}

/// Creatures are picked as upright cylinders this tall (metres). Provisional.
pub const PICK_HEIGHT: f32 = 1.9;

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
                thedas_sim::world::ObjectKind::Placeable if o.interactive => self
                    .objects
                    .as_ref()
                    .and_then(|s| s.bounds.get(&o.id))
                    .and_then(|&(off, r)| ray_sphere(origin, dir, p + off, r)),
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
        // An ability waiting for its target takes the click: a creature is the target;
        // anything else lets it go. Provisional: abilities aimed at a place are not
        // supported yet.
        if let Some(aim) = self.aiming.take() {
            let target = match pick {
                Pick::Object(id) => sim
                    .world
                    .get(id)
                    .filter(|o| o.kind == thedas_sim::world::ObjectKind::Creature && !o.dead)
                    .map(|o| (id, o.tag.clone())),
                Pick::Ground(_) => None,
            };
            return Some(match target {
                Some((id, tag)) => {
                    sim.order_ability(aim.caster, aim.ability, id, &aim.item);
                    format!("use ability {} on {tag}", aim.ability)
                }
                None => format!("ability {} not aimed", aim.ability),
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
