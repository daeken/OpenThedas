//! `thedas-app`: walk through one Dragon Age: Origins area.
//!
//! Interactive: WASD to move, Q/E (or C/Space) down/up, Shift to move faster, hold the right
//! mouse button (or use the arrow keys) to look around, P to print the camera, F12 to save a
//! capture, Esc to quit.
//!
//! Headless: `--capture out.png --camera x,y,z,yaw,pitch` renders one frame with no window.
//!
//! `--ui menu`, `--ui hud` or `--ui movie1,movie2` plays Scaleform movies over the scene;
//! `--area none` shows them alone.

mod ui;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use clap::Parser;
use glam::Vec3;
use thedas_formats::Area;
use thedas_render::{AreaRenderer, AreaScene, Camera, GpuContext, ProgramRegistry};
use thedas_vfs::install::TextureQuality;
use thedas_vfs::{DaoInstall, Vfs};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{DeviceEvent, DeviceId, ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

#[derive(Parser)]
#[command(
    name = "thedas-app",
    version,
    about = "Walk through a Dragon Age: Origins area"
)]
struct Args {
    /// Install root (the folder holding packages/ and modules/). Defaults to $THEDAS_INSTALL.
    #[arg(long)]
    install: Option<PathBuf>,
    /// Environment level (e.g. lgt101d), area resource (e.g. cir110ar_inn), or `none`.
    #[arg(long, default_value = "lgt101d")]
    area: String,
    /// Render one frame to this PNG with no window, then exit.
    #[arg(long)]
    capture: Option<PathBuf>,
    /// Camera as x,y,z,yaw,pitch (world units, degrees; yaw 0 looks along +X, Z is up).
    #[arg(long, allow_hyphen_values = true)]
    camera: Option<String>,
    /// Output or window size, WIDTHxHEIGHT.
    #[arg(long, default_value = "1280x720")]
    size: String,
    /// Which shaders draw the area: `original` (Static1 and its variants translated from the
    /// game's D3D9 bytecode, and the creature and placeable shaders; stand-in for the rest),
    /// `static` (Static1 only), `all` (every material whose original shaders translate), or
    /// `standin` (hand-written WGSL only).
    #[arg(long, default_value = "original")]
    shaders: String,
    /// Vertical field of view in degrees.
    #[arg(long, default_value_t = 60.0)]
    fov: f32,
    /// Scaleform movies to overlay: `menu`, `hud`, or a comma-separated list of movie names.
    #[arg(long)]
    ui: Option<String>,
    /// Headless: movie frames to run (at 30 fps) before capturing the interface.
    #[arg(long, default_value_t = 30)]
    ui_frames: u32,
    /// Draw the creatures and placeables of a simulation save (from the MCP's save_state),
    /// in its current area; `--area` is then ignored and the camera starts behind the hero.
    #[arg(long)]
    save: Option<PathBuf>,
    /// Draw the creatures and placeables an area starts with (runs the simulation for the
    /// area resource given as `--area`).
    #[arg(long)]
    objects: bool,
    /// Module for `--save` and `--objects`.
    #[arg(long, default_value = "single player")]
    module: String,
}

/// The simulation's view of the area to draw: the level, its objects' models and the hero.
struct SimView {
    layout: String,
    props: Vec<thedas_render::Prop>,
    /// Each prop's animation.
    clips: Vec<Option<thedas_render::Clip>>,
    hero: Option<([f32; 3], f32)>,
}

/// Opens a simulation for `area` (or the save's area) and reads its models. The simulation
/// is only read and never ticked, so the area shows its objects as placed (ticking would
/// run the module's start, which begins an origin story elsewhere).
fn sim_view(
    install: &Path,
    module: &str,
    area: Option<&str>,
    save: Option<&Path>,
) -> Result<SimView> {
    let opts = thedas_sim::session::SessionOptions {
        install: install.to_path_buf(),
        module: module.to_string(),
        area: area.map(str::to_string),
        waypoint: None,
        seed: 1,
        keep_trace: false,
    };
    let mut sim = thedas_sim::session::open(&opts)?;
    if let Some(path) = save {
        let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        sim.load_json(&bytes)?;
    }
    let area = sim.world.area.context("the simulation is in no area")?;
    let layout = sim.area_layout(area).context("the area has no layout")?;
    let models = thedas_ui::models::area_models(&sim, area);
    let clips = models
        .iter()
        .map(|m| {
            m.motion.as_ref().map(|c| thedas_render::Clip {
                trees: c.trees.clone(),
                actions: c.actions.clone(),
                time: c.time,
                looping: c.looping,
            })
        })
        .collect();
    let props = models
        .into_iter()
        .map(|m| thedas_render::Prop {
            label: format!("{} {}", m.id, m.tag),
            models: m.models,
            attached: m.attached.into_iter().map(|a| (a.model, a.node)).collect(),
            morph: m.morph,
            hide_hair: m.hide_hair,
            transform: thedas_render::Prop::placement(m.position, m.facing),
        })
        .collect();
    let hero = sim
        .world
        .hero
        .and_then(|h| sim.world.get(h))
        .filter(|h| h.area == Some(area))
        .map(|h| (h.position, h.facing));
    Ok(SimView {
        layout,
        props,
        clips,
        hero,
    })
}

/// Behind and above a hero at `pos` with the game's `facing`, looking the same way (as the
/// MCP screenshot does).
fn hero_camera(pos: [f32; 3], facing: f32) -> Camera {
    let yaw = thedas_sim::world::heading_of(facing).to_radians();
    let back = 4.0;
    Camera::new(
        Vec3::new(
            pos[0] - yaw.cos() * back,
            pos[1] - yaw.sin() * back,
            pos[2] + 2.5,
        ),
        yaw,
        -0.3,
    )
}

fn parse_size(s: &str) -> Result<(u32, u32)> {
    let (w, h) = s
        .split_once(['x', 'X'])
        .context("size must look like 1280x720")?;
    let (w, h): (u32, u32) = (w.trim().parse()?, h.trim().parse()?);
    if w == 0 || h == 0 || w > 16384 || h > 16384 {
        bail!("size {w}x{h} out of range");
    }
    Ok((w, h))
}

struct Loaded {
    vfs: Arc<Vfs>,
    scene: Option<AreaScene>,
    shaders: String,
    /// Each prop's placement and animation.
    posed: Vec<(glam::Mat4, Option<thedas_render::Clip>)>,
}

impl Loaded {
    fn programs(&self, scene: &AreaScene) -> Result<ProgramRegistry> {
        let mut reg = ProgramRegistry::default();
        let filter = match self.shaders.as_str() {
            "standin" => return Ok(reg),
            "original" => thedas_render::d3d::object_filter(),
            "static" => thedas_render::d3d::static1_filter(),
            "all" => None,
            other => bail!("--shaders must be original, static, all or standin, not {other}"),
        };
        let source = thedas_render::d3d::D3dSource::new(&self.vfs, scene, &filter);
        log::info!(
            "original shaders: {} programs, {} failed",
            source.programs().count(),
            source.failures.len()
        );
        reg.push_front(Box::new(source));
        Ok(reg)
    }
}

/// Opens the install, mounts the level (resolving an area name through its `.are` file) and
/// loads the scene.
fn load(
    install: &Path,
    area: &str,
    shaders: &str,
    props: &[thedas_render::Prop],
) -> Result<Loaded> {
    let install = DaoInstall::open(install)?;
    let mut vfs = install.global_vfs(TextureQuality::High)?;
    if area == "none" {
        return Ok(Loaded {
            vfs: Arc::new(vfs),
            scene: None,
            shaders: shaders.to_string(),
            posed: Vec::new(),
        });
    }
    let level = match install.mount_level(&mut vfs, area) {
        Ok(mount) => mount.level,
        Err(_) => {
            let are = if area.contains('.') {
                area.to_string()
            } else {
                format!("{area}.are")
            };
            let (data, _) = vfs
                .read(&are)
                .with_context(|| format!("{area} is neither a level nor an area"))?;
            let layout = Area::parse(&data)?
                .layout
                .with_context(|| format!("{are} names no layout"))?;
            install.mount_level(&mut vfs, &layout)?.level
        }
    };
    let started = Instant::now();
    let scene = AreaScene::load_with_props(&vfs, &level, props)?;
    log::info!(
        "{level} ({}): {} rooms, {} draws ({} of {} props), {} lights, {} meshes, {} materials, {} warnings, loaded in {:.2}s",
        scene.name.as_deref().unwrap_or("?"),
        scene.rooms.len(),
        scene.draws.len(),
        scene.draws.len() - scene.static_draws,
        scene.props.len(),
        scene.lights.len(),
        scene.meshes.len(),
        scene.materials.len(),
        scene.warnings.len(),
        started.elapsed().as_secs_f64()
    );
    Ok(Loaded {
        vfs: Arc::new(vfs),
        scene: Some(scene),
        shaders: shaders.to_string(),
        posed: Vec::new(),
    })
}

/// A starting camera: above the centroid of the placed models at standing height (1.7 above
/// the 20th-percentile placement height, a robust guess at the floor), looking along +X. Room bounds are not used because they often
/// include a large exterior margin.
fn default_camera(scene: &AreaScene) -> Camera {
    if scene.draws.is_empty() {
        return Camera::new(Vec3::new(0.0, 0.0, 2.0), 0.0, 0.0);
    }
    let points: Vec<Vec3> = scene
        .draws
        .iter()
        .map(|d| d.transform.w_axis.truncate())
        .collect();
    let centroid = points.iter().copied().sum::<Vec3>() / points.len() as f32;
    let mut heights: Vec<f32> = points.iter().map(|p| p.z).collect();
    heights.sort_by(f32::total_cmp);
    let floor = heights[heights.len() / 5];
    Camera::new(Vec3::new(centroid.x, centroid.y, floor + 1.7), 0.0, -0.1)
}

fn write_png(path: &Path, width: u32, height: u32, rgba: &[u8]) -> Result<()> {
    let file = std::io::BufWriter::new(
        std::fs::File::create(path).with_context(|| format!("creating {}", path.display()))?,
    );
    let mut enc = png::Encoder::new(file, width, height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(rgba)?;
    Ok(())
}

fn log_stats(r: &AreaRenderer) {
    let s = &r.stats;
    log::info!(
        "renderer: {} draws ({} skipped), {} meshes, {} textures ({} compressed, {} missing), programs {:?}",
        s.draws,
        s.draws_skipped,
        s.meshes,
        s.textures_loaded,
        s.textures_compressed,
        s.textures_missing.len(),
        s.programs
    );
    for t in &s.textures_missing {
        log::warn!("missing texture: {t}");
    }
}

fn capture(
    loaded: &Loaded,
    out: &Path,
    camera: Camera,
    (w, h): (u32, u32),
    ui: &[String],
    ui_frames: u32,
) -> Result<()> {
    let ctx = GpuContext::headless()?;
    let pixels = match &loaded.scene {
        Some(scene) => {
            let mut renderer = AreaRenderer::new(
                &ctx,
                &loaded.vfs,
                scene,
                &loaded.programs(scene)?,
                thedas_render::gpu::CAPTURE_FORMAT,
            );
            log_stats(&renderer);
            thedas_render::Animator::default().pose_props(
                &loaded.vfs,
                scene,
                &mut renderer,
                &ctx,
                &loaded.posed,
                0.0,
            );
            renderer.capture(&ctx, &camera, w, h)?
        }
        None => [0, 0, 0, 255].repeat(w as usize * h as usize),
    };
    let mut frame = thedas_ui::Rgba {
        width: w,
        height: h,
        pixels,
    };
    if !ui.is_empty() {
        let mut layer = ui::UiLayer::new(&ctx, &loaded.vfs, ui, (w, h));
        for _ in 0..ui_frames {
            for call in layer.tick(1000.0 / 30.0) {
                log::debug!("ui call: {call:?}");
            }
        }
        layer.log_gaps();
        layer.capture_over(&mut frame)?;
    }
    write_png(out, w, h, &frame.pixels)?;
    println!(
        "wrote {} ({w}x{h}, camera {})",
        out.display(),
        camera.spec()
    );
    Ok(())
}

struct Gfx {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    ctx: GpuContext,
    renderer: Option<AreaRenderer>,
    ui: ui::UiLayer,
}

struct App {
    loaded: Loaded,
    animator: thedas_render::Animator,
    /// When the window opened: looping animations play from it.
    started: Instant,
    ui: Vec<String>,
    cursor: (f64, f64),
    camera: Camera,
    size: (u32, u32),
    gfx: Option<Gfx>,
    keys: HashSet<KeyCode>,
    looking: bool,
    last: Instant,
    error: Option<anyhow::Error>,
    captures: u32,
}

impl App {
    fn init(&mut self, event_loop: &ActiveEventLoop) -> Result<()> {
        let window = Arc::new(
            event_loop.create_window(
                Window::default_attributes()
                    .with_title(match &self.loaded.scene {
                        Some(s) => format!(
                            "OpenThedas - {} ({})",
                            s.name.as_deref().unwrap_or(""),
                            s.level
                        ),
                        None => "OpenThedas".to_string(),
                    })
                    .with_inner_size(LogicalSize::new(self.size.0, self.size.1)),
            )?,
        );
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let surface = instance.create_surface(window.clone())?;
        let ctx = pollster::block_on(GpuContext::new(instance, Some(&surface)))?;
        let phys = window.inner_size();
        let mut config = surface
            .get_default_config(&ctx.adapter, phys.width.max(1), phys.height.max(1))
            .context("surface is not supported by the adapter")?;
        // Prefer a non-sRGB format: D3D9 presented shader output unconverted.
        let caps = surface.get_capabilities(&ctx.adapter);
        if let Some(f) = caps.formats.iter().find(|f| !f.is_srgb()) {
            config.format = *f;
        }
        config.present_mode = wgpu::PresentMode::AutoVsync;
        surface.configure(&ctx.device, &config);
        let renderer = match &self.loaded.scene {
            Some(scene) => {
                let r = AreaRenderer::new(
                    &ctx,
                    &self.loaded.vfs,
                    scene,
                    &self.loaded.programs(scene)?,
                    config.format,
                );
                log_stats(&r);
                println!("camera {}", self.camera.spec());
                Some(r)
            }
            None => None,
        };
        let ui = ui::UiLayer::new(
            &ctx,
            &self.loaded.vfs,
            &self.ui,
            (config.width, config.height),
        );
        self.gfx = Some(Gfx {
            window,
            surface,
            config,
            ctx,
            renderer,
            ui,
        });
        Ok(())
    }

    fn update(&mut self) {
        let now = Instant::now();
        let dt = (now - self.last).as_secs_f32().min(0.1);
        self.last = now;
        let k = |c| self.keys.contains(&c);
        let speed = if k(KeyCode::ShiftLeft) || k(KeyCode::ShiftRight) {
            15.0
        } else {
            4.0
        } * dt;
        let axis = |pos: bool, neg: bool| (pos as i32 - neg as i32) as f32;
        let fwd = axis(k(KeyCode::KeyW), k(KeyCode::KeyS));
        let right = axis(k(KeyCode::KeyD), k(KeyCode::KeyA));
        let up = axis(
            k(KeyCode::KeyE) || k(KeyCode::Space),
            k(KeyCode::KeyQ) || k(KeyCode::KeyC),
        );
        self.camera.fly(fwd * speed, right * speed, up * speed);
        let turn = 1.5 * dt;
        let yaw = axis(k(KeyCode::ArrowLeft), k(KeyCode::ArrowRight));
        let pitch = axis(k(KeyCode::ArrowUp), k(KeyCode::ArrowDown));
        self.camera.look(yaw * turn, pitch * turn);
    }

    fn redraw(&mut self) -> Result<()> {
        let dt = self.last.elapsed().as_secs_f64() * 1000.0;
        self.update();
        let Some(g) = self.gfx.as_mut() else {
            return Ok(());
        };
        let frame = match g.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f)
            | wgpu::CurrentSurfaceTexture::Suboptimal(f) => f,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                g.surface.configure(&g.ctx.device, &g.config);
                return Ok(());
            }
            _ => return Ok(()),
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = g
            .ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame"),
            });
        if let (Some(r), Some(scene)) = (g.renderer.as_mut(), self.loaded.scene.as_ref()) {
            let elapsed = self.started.elapsed().as_secs_f32();
            self.animator.pose_props(
                &self.loaded.vfs,
                scene,
                r,
                &g.ctx,
                &self.loaded.posed,
                elapsed,
            );
        }
        match &mut g.renderer {
            Some(r) => r.render(
                &g.ctx,
                &mut encoder,
                &view,
                g.config.width,
                g.config.height,
                &self.camera,
            ),
            None => {
                encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("clear"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        resolve_target: None,
                        depth_slice: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                });
            }
        }
        for call in g.ui.tick(dt.min(100.0)) {
            log::debug!("ui call: {call:?}");
        }
        g.ui.composite(&mut encoder, &view, g.config.format);
        g.ctx.queue.submit([encoder.finish()]);
        g.window.pre_present_notify();
        g.ctx.queue.present(frame);
        Ok(())
    }

    fn save_capture(&mut self) -> Result<()> {
        let Some(g) = self.gfx.as_mut() else {
            return Ok(());
        };
        self.captures += 1;
        let level = self
            .loaded
            .scene
            .as_ref()
            .map_or("ui", |s| s.level.as_str());
        let path = PathBuf::from(format!("{level}-capture-{}.png", self.captures));
        let (w, h) = (g.config.width, g.config.height);
        let Some(renderer) = g.renderer.as_mut() else {
            return Ok(());
        };
        let pixels = renderer.capture(&g.ctx, &self.camera, w, h)?;
        write_png(&path, w, h, &pixels)?;
        println!("wrote {} (camera {})", path.display(), self.camera.spec());
        Ok(())
    }

    fn fail(&mut self, event_loop: &ActiveEventLoop, e: anyhow::Error) {
        self.error = Some(e);
        event_loop.exit();
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gfx.is_none() {
            if let Err(e) = self.init(event_loop) {
                self.fail(event_loop, e);
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(g) = self.gfx.as_mut() {
                    if size.width > 0 && size.height > 0 {
                        g.config.width = size.width;
                        g.config.height = size.height;
                        g.surface.configure(&g.ctx.device, &g.config);
                        g.ui.resize((size.width, size.height));
                    }
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let PhysicalKey::Code(code) = event.physical_key else {
                    return;
                };
                match event.state {
                    ElementState::Pressed => {
                        if !event.repeat {
                            match code {
                                KeyCode::Escape => event_loop.exit(),
                                KeyCode::KeyP => println!("camera {}", self.camera.spec()),
                                KeyCode::F12 => {
                                    if let Err(e) = self.save_capture() {
                                        eprintln!("capture failed: {e:#}");
                                    }
                                }
                                _ => {}
                            }
                        }
                        self.keys.insert(code);
                    }
                    ElementState::Released => {
                        self.keys.remove(&code);
                    }
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = (position.x, position.y);
                if let Some(g) = self.gfx.as_mut() {
                    g.ui.mouse_move(position.x, position.y);
                }
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => {
                let (x, y) = self.cursor;
                if let Some(g) = self.gfx.as_mut() {
                    g.ui.mouse_button(x, y, state == ElementState::Pressed);
                }
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Right,
                ..
            } => {
                self.looking = state == ElementState::Pressed;
                if let Some(g) = &self.gfx {
                    g.window.set_cursor_visible(!self.looking);
                }
            }
            WindowEvent::Focused(false) => {
                self.keys.clear();
                self.looking = false;
            }
            WindowEvent::RedrawRequested => {
                if let Err(e) = self.redraw() {
                    self.fail(event_loop, e);
                }
            }
            _ => {}
        }
    }

    fn device_event(&mut self, _: &ActiveEventLoop, _: DeviceId, event: DeviceEvent) {
        if let DeviceEvent::MouseMotion { delta } = event {
            if self.looking {
                let s = 0.003;
                self.camera.look(-delta.0 as f32 * s, -delta.1 as f32 * s);
            }
        }
    }

    fn about_to_wait(&mut self, _: &ActiveEventLoop) {
        if let Some(g) = &self.gfx {
            g.window.request_redraw();
        }
    }
}

fn main() -> Result<()> {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info,wgpu=warn,naga=warn"),
    )
    .init();
    let args = Args::parse();
    let install = args
        .install
        .or_else(|| std::env::var_os("THEDAS_INSTALL").map(PathBuf::from))
        .context("give --install or set THEDAS_INSTALL")?;
    let size = parse_size(&args.size)?;
    let view = if args.save.is_some() || args.objects {
        Some(sim_view(
            &install,
            &args.module,
            args.save.is_none().then_some(args.area.as_str()),
            args.save.as_deref(),
        )?)
    } else {
        None
    };
    let (area, props) = match &view {
        Some(v) => (v.layout.as_str(), v.props.as_slice()),
        None => (args.area.as_str(), &[][..]),
    };
    let mut loaded = load(&install, area, &args.shaders, props)?;
    if let Some(v) = &view {
        loaded.posed = v
            .props
            .iter()
            .map(|p| p.transform)
            .zip(v.clips.iter().cloned())
            .collect();
    }
    let hero = view.as_ref().and_then(|v| v.hero);
    let mut camera = match (&args.camera, &loaded.scene) {
        (Some(spec), _) => Camera::parse(spec).context("camera must be x,y,z,yaw,pitch")?,
        (None, Some(_)) if hero.is_some() => {
            let (pos, facing) = hero.unwrap_or_default();
            hero_camera(pos, facing)
        }
        (None, Some(scene)) => default_camera(scene),
        (None, None) => Camera::new(Vec3::ZERO, 0.0, 0.0),
    };
    camera.fov_y = args.fov.clamp(10.0, 150.0).to_radians();
    let ui = args
        .ui
        .as_deref()
        .map(thedas_ui::stage::movie_names)
        .unwrap_or_default();

    if let Some(out) = &args.capture {
        return capture(&loaded, out, camera, size, &ui, args.ui_frames);
    }

    let event_loop = EventLoop::new()?;
    let mut app = App {
        loaded,
        animator: thedas_render::Animator::default(),
        started: Instant::now(),
        ui,
        cursor: (0.0, 0.0),
        camera,
        size,
        gfx: None,
        keys: HashSet::new(),
        looking: false,
        last: Instant::now(),
        error: None,
        captures: 0,
    };
    event_loop.run_app(&mut app)?;
    match app.error {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sizes() {
        assert_eq!(parse_size("1280x720").unwrap(), (1280, 720));
        assert_eq!(parse_size("64X32").unwrap(), (64, 32));
        assert!(parse_size("0x10").is_err());
        assert!(parse_size("wide").is_err());
    }
}
