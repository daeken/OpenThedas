//! A running game inside the window: the simulation ticks at real time (unless paused), and
//! the area it is in is drawn with its creatures and placeables posed from it. The
//! simulation is only read here; input reaches it as commands elsewhere.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use glam::Mat4;
use thedas_render::{Animator, AreaRenderer, AreaScene, Clip, GpuContext, Prop};
use thedas_sim::Sim;
use thedas_ui::models::{ModelCache, ObjectModel};
use thedas_vfs::install::TextureQuality;
use thedas_vfs::{DaoInstall, Vfs};

/// The longest step one frame advances the simulation (milliseconds): a stalled frame
/// (a level loading) does not leap the world ahead.
pub const MAX_STEP_MS: f64 = 100.0;

/// What a drawn scene was built for: the level and each object's models. A change rebuilds
/// it.
type SceneKey = (String, Vec<(u32, Vec<String>)>);

pub struct LiveScene {
    key: SceneKey,
    pub vfs: Arc<Vfs>,
    pub scene: AreaScene,
    pub renderer: AreaRenderer,
}

pub struct Live {
    pub sim: Sim,
    pub paused: bool,
    /// The party member the player controls; `None` for the hero.
    pub leader: Option<thedas_script::vm::ObjectId>,
    install: PathBuf,
    shaders: String,
    pub scene: Option<LiveScene>,
    animator: Animator,
    models: ModelCache,
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
        Ok(Self {
            sim,
            paused: false,
            leader: None,
            install: install.to_path_buf(),
            shaders: shaders.to_string(),
            scene: None,
            animator: Animator::default(),
            models: ModelCache::default(),
            owed_ms: 0.0,
        })
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
            self.sim.tick(whole as u64);
        }
    }

    /// The party leader the player controls (the hero until another is chosen).
    pub fn leader(&self) -> Option<thedas_script::vm::ObjectId> {
        self.leader.or(self.sim.world.hero)
    }

    /// Where the party leader stands, when it is in the current area.
    pub fn leader_feet(&self) -> Option<glam::Vec3> {
        let w = &self.sim.world;
        let o = w.get(self.leader()?)?;
        (o.area.is_some() && o.area == w.area).then(|| glam::Vec3::from_array(o.position))
    }

    /// The hero's position and facing, when it is in the current area.
    pub fn hero(&self) -> Option<([f32; 3], f32)> {
        let w = &self.sim.world;
        let h = w.get(w.hero?)?;
        (h.area.is_some() && h.area == w.area).then_some((h.position, h.facing))
    }

    /// Brings the drawn scene up to the simulation: rebuilds it when the area or the set
    /// of objects changed, then places and poses every object.
    pub fn sync(&mut self, ctx: &GpuContext, format: wgpu::TextureFormat) -> Result<()> {
        let Some(area) = self.sim.world.area else {
            self.scene = None;
            return Ok(());
        };
        let Some(layout) = self.sim.area_layout(area) else {
            self.scene = None;
            return Ok(());
        };
        let models = thedas_ui::models::area_models_cached(&self.sim, area, &mut self.models);
        let key = scene_key(&layout, &models);
        if self.scene.as_ref().map(|s| &s.key) != Some(&key) {
            self.scene = Some(self.build(ctx, format, &layout, &models, key)?);
        }
        let Some(live) = self.scene.as_mut() else {
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
        self.animator
            .pose_props(&live.vfs, &live.scene, &mut live.renderer, ctx, &posed, 0.0);
        Ok(())
    }

    fn build(
        &mut self,
        ctx: &GpuContext,
        format: wgpu::TextureFormat,
        layout: &str,
        models: &[ObjectModel],
        key: SceneKey,
    ) -> Result<LiveScene> {
        let started = std::time::Instant::now();
        let install = DaoInstall::open(&self.install)?;
        let mut vfs = install.global_vfs(TextureQuality::High)?;
        install.mount_level(&mut vfs, layout)?;
        let props: Vec<Prop> = models.iter().map(prop).collect();
        let scene = AreaScene::load_with_props(&vfs, layout, &props)?;
        let programs = crate::programs(&vfs, &scene, &self.shaders)?;
        let renderer = AreaRenderer::new(ctx, &vfs, &scene, &programs, format);
        log::info!(
            "{layout}: {} objects, {} draws, built in {:.2}s",
            models.len(),
            scene.draws.len(),
            started.elapsed().as_secs_f64()
        );
        // A new scene means new skeleton indices.
        self.animator = Animator::default();
        Ok(LiveScene {
            key,
            vfs: Arc::new(vfs),
            scene,
            renderer,
        })
    }
}

fn scene_key(layout: &str, models: &[ObjectModel]) -> SceneKey {
    (
        layout.to_string(),
        models
            .iter()
            .map(|m| {
                let mut v = m.models.clone();
                v.extend(m.attached.iter().map(|a| format!("{}@{}", a.model, a.node)));
                v.extend(m.morph.iter().map(|x| format!("{x}.mor{}", m.hide_hair)));
                v.extend(m.tints.iter().map(|(a, t)| format!("{a}:{t}")));
                (m.id, v)
            })
            .collect(),
    )
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
