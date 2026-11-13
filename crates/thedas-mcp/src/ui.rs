//! The interface for MCP clients: Scaleform movies on a headless GPU, on their own thread
//! (Ruffle players are not `Send`). Tools send closures to the thread and await the result.

use std::io::Cursor;
use std::sync::Arc;
use std::sync::mpsc;

use thedas_ui::models::ObjectModel;
use thedas_ui::protocol::GameMessage;
use thedas_ui::stage::Stage;
use thedas_ui::{Gpu, Rgba, UiValue};
use thedas_vfs::Vfs;

/// A level and the props placed in it, uploaded.
pub struct LoadedScene {
    /// The layout and each prop's object and models: a change reloads the scene.
    key: (String, Vec<(u32, Vec<String>)>),
    scene: thedas_render::AreaScene,
    renderer: thedas_render::AreaRenderer,
    vfs: thedas_vfs::Vfs,
    animator: thedas_render::Animator,
}

/// A creature's motion as a clip for the animator.
pub fn clip(m: &thedas_ui::models::Motion) -> thedas_render::Clip {
    thedas_render::Clip {
        trees: m.trees.clone(),
        actions: m.actions.clone(),
        time: m.time,
        looping: m.looping,
        layers: Vec::new(),
        absolute: false,
        ..Default::default()
    }
}

/// What a capture shows: the level `layout` and the objects in it.
fn scene_key(layout: &str, props: &[ObjectModel]) -> (String, Vec<(u32, Vec<String>)>) {
    (
        layout.to_string(),
        props
            .iter()
            .map(|p| {
                let mut models = p.models.clone();
                models.extend(p.attached.iter().map(|a| format!("{}@{}", a.model, a.node)));
                models.extend(p.morph.iter().map(|m| format!("{m}.mor{}", p.hide_hair)));
                models.extend(p.tints.iter().map(|(m, t)| format!("{m}:{t}")));

                (p.id, models)
            })
            .collect(),
    )
}

/// Everything the UI thread owns.
#[derive(Default)]
pub struct UiWorld {
    gpu: Option<Gpu>,
    ctx: Option<thedas_render::GpuContext>,
    /// The level loaded for scene captures, with the simulation's models in it.
    scene: Option<LoadedScene>,
    pub stage: Option<Stage>,
    /// Messages for the engine since the last read.
    pub messages: Vec<GameMessage>,
}

impl UiWorld {
    fn gpu(&mut self) -> Result<Gpu, String> {
        if self.gpu.is_none() {
            let ctx = thedas_render::GpuContext::headless().map_err(|e| e.to_string())?;
            self.gpu = Some(Gpu {
                instance: ctx.instance.clone(),
                adapter: ctx.adapter.clone(),
                device: ctx.device.clone(),
                queue: ctx.queue.clone(),
            });
            self.ctx = Some(ctx);
        }
        self.gpu.clone().ok_or_else(|| "no GPU".to_string())
    }

    /// Renders the level `layout` with the simulation's creatures and placeables (`props`)
    /// from `camera` (RGBA8). The level loads again when the layout or the set of objects
    /// changes; objects that only moved are moved. Uses the game's Static1, character and prop
    /// shaders where they translate and the stand-in elsewhere.
    pub fn render_scene(
        &mut self,
        install: &std::path::Path,
        layout: &str,
        props: &[ObjectModel],
        camera: &thedas_render::Camera,
        (w, h): (u32, u32),
    ) -> Result<Rgba, String> {
        self.gpu()?;
        let ctx = self.ctx.as_ref().ok_or("no GPU")?;
        let key = scene_key(layout, props);
        let placed: Vec<thedas_render::Prop> = props
            .iter()
            .map(|p| thedas_render::Prop {
                label: format!("{} {}", p.id, p.tag),
                models: p.models.clone(),
                attached: p
                    .attached
                    .iter()
                    .map(|a| (a.model.clone(), a.node.clone()))
                    .collect(),
                morph: p.morph.clone(),
                hide_hair: p.hide_hair,
                tints: p.tints.clone(),
                effects: Vec::new(),

                transform: thedas_render::Prop::placement(p.position, p.facing),
            })
            .collect();
        if self.scene.as_ref().map(|s| &s.key) != Some(&key) {
            self.scene = None;
            let inst = thedas_vfs::DaoInstall::open(install).map_err(|e| e.to_string())?;
            let mut vfs = inst
                .play_vfs(thedas_vfs::install::TextureQuality::High)
                .map_err(|e| e.to_string())?;
            inst.mount_level(&mut vfs, layout)
                .map_err(|e| e.to_string())?;
            let scene = thedas_render::AreaScene::load_with_props(&vfs, layout, &placed)
                .map_err(|e| e.to_string())?;
            let mut programs = thedas_render::ProgramRegistry::default();
            let filter = thedas_render::d3d::object_filter();
            programs.push_front(Box::new(thedas_render::d3d::D3dSource::new(
                &vfs, &scene, &filter,
            )));
            let renderer = thedas_render::AreaRenderer::new(
                ctx,
                &vfs,
                &scene,
                &programs,
                thedas_render::gpu::CAPTURE_FORMAT,
            );
            self.scene = Some(LoadedScene {
                key,
                scene,
                renderer,
                vfs,
                animator: thedas_render::Animator::default(),
            });
        }
        let loaded = self.scene.as_mut().ok_or("no scene")?;
        let posed: Vec<(glam::Mat4, Option<thedas_render::Clip>)> = placed
            .iter()
            .zip(props)
            .map(|(p, m)| (p.transform, m.motion.as_ref().map(clip)))
            .collect();
        loaded.animator.pose_props(
            &loaded.vfs,
            &loaded.scene,
            &mut loaded.renderer,
            ctx,
            &posed,
            0.0,
        );
        let pixels = loaded
            .renderer
            .capture(ctx, camera, w, h)
            .map_err(|e| e.to_string())?;
        Ok(Rgba {
            width: w,
            height: h,
            pixels,
        })
    }

    /// The scene (if any) with the open movies over it, as PNG bytes.
    pub fn compose(&mut self, base: Option<Rgba>, ui: bool) -> Result<Vec<u8>, String> {
        let size = self.stage.as_ref().map(|s| s.size()).unwrap_or((1280, 720));
        let mut frame = base.unwrap_or_else(|| Rgba {
            width: size.0,
            height: size.1,
            pixels: [0, 0, 0, 255].repeat(size.0 as usize * size.1 as usize),
        });
        if ui {
            if let Some(stage) = self.stage.as_mut() {
                if stage.size() == (frame.width, frame.height) {
                    stage.capture_over(&mut frame).map_err(|e| e.to_string())?;
                }
            }
        }
        thedas_ui::stage::encode_png(&frame).ok_or_else(|| "PNG encoding failed".to_string())
    }

    /// The stage, created for `vfs` at `size` if there is none (or the size changed).
    pub fn stage(&mut self, vfs: &Arc<Vfs>, size: (u32, u32)) -> Result<&mut Stage, String> {
        if self.stage.as_ref().map(|s| s.size()) != Some(size) {
            let gpu = self.gpu()?;
            self.stage = Some(Stage::new(gpu, vfs, size));
        }
        self.stage.as_mut().ok_or_else(|| "no stage".to_string())
    }

    /// Runs `frames` movie frames at 30 fps with the given engine values bound.
    pub fn step(&mut self, frames: u32, values: &[(String, UiValue)]) {
        let Some(stage) = self.stage.as_mut() else {
            return;
        };
        // Bound once: messages handled during these frames update the values themselves.
        stage.bridge.bind(values.iter().cloned());
        for _ in 0..frames.max(1) {
            self.messages.extend(stage.tick(1000.0 / 30.0));
        }
    }

    /// The movies composited over black, as PNG bytes.
    pub fn screenshot(&mut self) -> Result<Vec<u8>, String> {
        let stage = self.stage.as_mut().ok_or("no movies open; call ui_open")?;
        let (w, h) = stage.size();
        let mut frame = Rgba {
            width: w,
            height: h,
            pixels: [0, 0, 0, 255].repeat(w as usize * h as usize),
        };
        stage.capture_over(&mut frame).map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        {
            let mut enc = png::Encoder::new(Cursor::new(&mut out), w, h);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            let mut wr = enc.write_header().map_err(|e| e.to_string())?;
            wr.write_image_data(&frame.pixels)
                .map_err(|e| e.to_string())?;
        }
        Ok(out)
    }
}

type Job = Box<dyn FnOnce(&mut UiWorld) + Send>;

/// Handle to the UI thread.
#[derive(Clone)]
pub struct UiThread {
    tx: mpsc::Sender<Job>,
}

impl UiThread {
    pub fn spawn() -> Self {
        let (tx, rx) = mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("thedas-ui".into())
            .spawn(move || {
                let mut world = UiWorld::default();
                while let Ok(job) = rx.recv() {
                    job(&mut world);
                }
            })
            .ok();
        Self { tx }
    }

    /// Runs `f` on the UI thread and returns its result.
    pub async fn run<R: Send + 'static>(
        &self,
        f: impl FnOnce(&mut UiWorld) -> R + Send + 'static,
    ) -> Result<R, String> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.tx
            .send(Box::new(move |w: &mut UiWorld| {
                let _ = tx.send(f(w));
            }))
            .map_err(|_| "the UI thread is gone".to_string())?;
        rx.await
            .map_err(|_| "the UI thread dropped the job".to_string())
    }
}
