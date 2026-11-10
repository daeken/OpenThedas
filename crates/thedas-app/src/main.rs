//! `thedas-app`: walk through a Dragon Age: Origins area, or play.
//!
//! Interactive: WASD to move, Q/E (or C) down/up, Shift to move faster, hold the right
//! mouse button (or use the arrow keys) to look around, P to print the camera, F12 to save a
//! capture, Esc to quit.
//!
//! Playing: `--save <file>` (a save from the MCP's save_state) or `--new-game` runs the
//! simulation live at real time; Space pauses it. The camera follows the party leader: hold
//! the right mouse button to orbit, the wheel to zoom, Q/E to turn; F switches to the
//! free-flying camera and back. A left click orders the leader: walk to the ground clicked,
//! attack a hostile, talk to someone, use a door or container. F1-F4 choose the leader;
//! 1-9 and 0 use the quickbar's slots (an ability aimed at someone waits for a click on its
//! target; a right click drops it).
//! The game's own interface plays over the world: the HUD while exploring, the conversation
//! screen (click a reply), and the screens scripts open (world map, store, job board,
//! level-up); I opens the inventory. A click the interface draws under is the interface's.
//!
//! Headless: `--capture out.png --camera x,y,z,yaw,pitch` renders one frame with no window.
//! Playing headless, `--steps "click 640,400; run 3000; click 300,650; key i"` plays input
//! in order (clicks go to the interface or the world as in the window) before the capture;
//! `talk <tag>` orders the leader to talk to a creature as a click on it would.
//!
//! `--ui menu`, `--ui hud` or `--ui movie1,movie2` plays Scaleform movies over the scene;
//! `--area none` shows them alone.

mod follow;
mod live;
mod screens;
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
    /// Start a new game: the module's start, through character creation's quick start.
    #[arg(long)]
    new_game: bool,
    /// With `--new-game`: the hero's name (character creation's quick start does not ask).
    #[arg(long)]
    name: Option<String>,
    /// Headless, playing: run the simulation this many milliseconds before capturing.
    #[arg(long, default_value_t = 0)]
    run_ms: u64,
    /// Headless, playing: left-click at this pixel (`x,y`) of the view before running, as the
    /// player would (repeatable).
    #[arg(long)]
    click: Vec<String>,
    /// Headless, playing: input played in order before the capture, `;`-separated:
    /// `click X,Y` (a left click at that pixel), `run MS` (the game and its interface run
    /// that long), `key i` (the inventory toggle), `talk TAG` (the leader talks to a creature),
    /// `rebuild` (the objects are rebuilt as a spawn rebuilds them, timed), `rclick X,Y` and
    /// `dclick X,Y` (a right or double click for the interface).
    #[arg(long, default_value = "")]
    steps: String,
    /// Module for `--save` and `--objects`.
    #[arg(long, default_value = "single player")]
    module: String,
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
}

impl Loaded {
    fn programs(&self, scene: &AreaScene) -> Result<ProgramRegistry> {
        programs(&self.vfs, scene, &self.shaders)
    }
}

/// The programs drawing `scene`: the original shaders the `--shaders` choice allows, the
/// stand-in for the rest.
pub fn programs(vfs: &Vfs, scene: &AreaScene, shaders: &str) -> Result<ProgramRegistry> {
    programs_cached(
        vfs,
        scene,
        shaders,
        &mut thedas_render::RenderCache::default(),
    )
}

/// [`programs`], reusing translations kept in `cache`.
pub fn programs_cached(
    vfs: &Vfs,
    scene: &AreaScene,
    shaders: &str,
    cache: &mut thedas_render::RenderCache,
) -> Result<ProgramRegistry> {
    {
        let mut reg = ProgramRegistry::default();
        let filter = match shaders {
            "standin" => return Ok(reg),
            "original" => thedas_render::d3d::object_filter(),
            "static" => thedas_render::d3d::static1_filter(),
            "all" => None,
            other => bail!("--shaders must be original, static, all or standin, not {other}"),
        };
        let source = thedas_render::d3d::D3dSource::new_cached(vfs, scene, &filter, cache);
        log::debug!(
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

#[allow(clippy::too_many_arguments)]
fn capture(
    ctx: &GpuContext,
    loaded: &Loaded,
    live: Option<&mut live::Live>,
    out: &Path,
    camera: Camera,
    (w, h): (u32, u32),
    ui: &[String],
    ui_frames: u32,
    played: Option<&mut ui::UiLayer>,
) -> Result<()> {
    let pixels = if let Some(live) = live {
        live.sync(ctx, thedas_render::gpu::CAPTURE_FORMAT)?;
        if let Some(l) = live.level.as_ref() {
            log_stats(&l.renderer);
        }
        live.capture(ctx, &camera, (w, h))?
            .unwrap_or_else(|| [0, 0, 0, 255].repeat(w as usize * h as usize))
    } else {
        match &loaded.scene {
            Some(scene) => {
                let mut renderer = AreaRenderer::new(
                    ctx,
                    &loaded.vfs,
                    scene,
                    &loaded.programs(scene)?,
                    thedas_render::gpu::CAPTURE_FORMAT,
                );
                log_stats(&renderer);
                renderer.capture(ctx, &camera, w, h)?
            }
            None => [0, 0, 0, 255].repeat(w as usize * h as usize),
        }
    };
    let mut frame = thedas_ui::Rgba {
        width: w,
        height: h,
        pixels,
    };
    if let Some(layer) = played {
        layer.log_gaps();
        layer.capture_over(&mut frame)?;
    } else if !ui.is_empty() {
        let mut layer = ui::UiLayer::new(ctx, &loaded.vfs, ui, (w, h));
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
    /// The running game, when playing.
    live: Option<live::Live>,
    /// The game camera, when following the party leader (playing; F toggles free-fly).
    follow: Option<follow::Follow>,
    /// The game's screens over a running game (playing without `--ui`).
    screens: Option<screens::Screens>,
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
    /// The last left press (when, where), to tell a double click.
    last_press: Option<(Instant, (f64, f64))>,
    /// The release of a double click the interface already had.
    swallow_release: bool,
}

/// Two left presses this close (seconds, pixels) are a double click. Provisional.
const DOUBLE_CLICK_SECS: f64 = 0.35;
const DOUBLE_CLICK_PIXELS: f64 = 6.0;

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
        let mut ui = ui::UiLayer::new(
            &ctx,
            &self.loaded.vfs,
            &self.ui,
            (config.width, config.height),
        );
        if let (Some(live), Some(_)) = (&self.live, &self.screens) {
            ui.attach(&live.sim);
        }
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
        if let Some(f) = self.follow.as_mut() {
            let turn = (k(KeyCode::KeyQ) as i32 - k(KeyCode::KeyE) as i32) as f32;
            f.orbit(turn * 1.8 * dt, 0.0);
            if let Some(feet) = self.live.as_ref().and_then(|l| l.leader_feet()) {
                self.camera = f.camera(feet, dt, &self.camera);
            }
            return;
        }
        let speed = if k(KeyCode::ShiftLeft) || k(KeyCode::ShiftRight) {
            15.0
        } else {
            4.0
        } * dt;
        let axis = |pos: bool, neg: bool| (pos as i32 - neg as i32) as f32;
        let fwd = axis(k(KeyCode::KeyW), k(KeyCode::KeyS));
        let right = axis(k(KeyCode::KeyD), k(KeyCode::KeyA));
        let up = axis(k(KeyCode::KeyE), k(KeyCode::KeyQ) || k(KeyCode::KeyC));
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
        let stopped = self.screens.as_ref().is_some_and(|s| s.world_stopped());
        if let Some(live) = self.live.as_mut() {
            if !stopped {
                live.step(dt);
            }
            live.sync(&g.ctx, g.config.format)?;
        }
        let size = (g.config.width, g.config.height);
        let drawn = match (self.live.as_mut(), g.renderer.as_mut()) {
            (Some(live), _) => live.render(&g.ctx, &mut encoder, &view, size, &self.camera),
            (None, Some(r)) => {
                r.render(&g.ctx, &mut encoder, &view, size.0, size.1, &self.camera);
                true
            }
            (None, None) => false,
        };
        match drawn {
            true => {}
            false => {
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
        match (self.screens.as_mut(), self.live.as_mut()) {
            (Some(screens), Some(live)) => {
                for aim in g.ui.play(screens, &live.sim, dt.min(100.0)) {
                    log::info!("aiming ability {} (click a target)", aim.ability);
                    live.aiming = Some(aim);
                }
            }
            _ => {
                for call in g.ui.tick(dt.min(100.0)) {
                    log::debug!("ui call: {call:?}");
                }
            }
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
        let pixels = match (self.live.as_mut(), g.renderer.as_mut()) {
            (Some(live), _) => live.capture(&g.ctx, &self.camera, (w, h))?,
            (None, Some(r)) => Some(r.capture(&g.ctx, &self.camera, w, h)?),
            (None, None) => None,
        };
        let Some(pixels) = pixels else {
            return Ok(());
        };
        write_png(&path, w, h, &pixels)?;
        println!("wrote {} (camera {})", path.display(), self.camera.spec());
        Ok(())
    }

    /// A left click at window pixel (`x`, `y`) while playing: the party leader is ordered to
    /// whatever it lands on.
    fn click(&mut self, x: f64, y: f64) {
        let (Some(live), Some(g)) = (self.live.as_mut(), self.gfx.as_ref()) else {
            return;
        };
        let (w, h) = (g.config.width as f32, g.config.height as f32);
        let (origin, dir) = self.camera.ray(x as f32, y as f32, w, h);
        if let Some(order) = live.pick(origin, dir).and_then(|p| live.click(p)) {
            log::info!("order: {order}");
        }
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
                                KeyCode::F1 | KeyCode::F2 | KeyCode::F3 | KeyCode::F4
                                    if self.live.is_some() =>
                                {
                                    let n = match code {
                                        KeyCode::F1 => 0,
                                        KeyCode::F2 => 1,
                                        KeyCode::F3 => 2,
                                        _ => 3,
                                    };
                                    if let Some(tag) =
                                        self.live.as_mut().and_then(|l| l.choose_leader(n))
                                    {
                                        log::info!("leading with {tag}");
                                    }
                                }
                                // The quickbar's first ten slots: 1 to 9, then 0.
                                c if self.live.is_some() && quickslot_key(c).is_some() => {
                                    let slot = quickslot_key(c).unwrap_or(0);
                                    if let Some(live) = self.live.as_mut() {
                                        let aim = {
                                            let mut sim = live.sim();
                                            let who = sim.world.controlled();
                                            who.and_then(|w| {
                                                thedas_ui::input::use_quickslot(&mut sim, w, slot)
                                            })
                                        };
                                        if let Some(aim) = aim {
                                            log::info!(
                                                "aiming ability {} (click a target)",
                                                aim.ability
                                            );
                                            live.aiming = Some(aim);
                                        }
                                    }
                                }
                                KeyCode::KeyI if self.screens.is_some() => {
                                    if let Some(s) = self.screens.as_mut() {
                                        s.inventory = !s.inventory;
                                    }
                                }
                                KeyCode::KeyF if self.live.is_some() => {
                                    self.follow = match self.follow.take() {
                                        Some(_) => None,
                                        None => Some(follow::Follow::behind(self.camera.yaw)),
                                    };
                                }
                                KeyCode::Space => {
                                    if let Some(live) = self.live.as_mut() {
                                        live.paused = !live.paused;
                                        println!(
                                            "{}",
                                            if live.paused { "paused" } else { "running" }
                                        );
                                    }
                                }
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
                let pressed = state == ElementState::Pressed;
                if !pressed && std::mem::take(&mut self.swallow_release) {
                    return;
                }
                // A second press close in time and place is a double click (the movies use
                // them: equipping from the backpack).
                let double = pressed
                    && self.last_press.is_some_and(|(t, (px, py))| {
                        t.elapsed().as_secs_f64() < DOUBLE_CLICK_SECS
                            && (px - x).hypot(py - y) < DOUBLE_CLICK_PIXELS
                    });
                if pressed {
                    self.last_press = (!double).then(|| (Instant::now(), (x, y)));
                }
                if double {
                    if let Some(g) = self.gfx.as_mut() {
                        if self.screens.as_ref().is_some_and(|s| !s.exploring()) {
                            g.ui.click_as(
                                thedas_ui::avm1::MOUSE_LEFT + thedas_ui::avm1::DOUBLE_CLICK,
                                x,
                                y,
                            );
                            self.swallow_release = true;
                            return;
                        }
                    }
                }
                // Playing, a click the interface does not take orders the party leader.
                let mut to_world = pressed && self.live.is_some() && self.ui.is_empty();
                if let Some(g) = self.gfx.as_mut() {
                    if to_world {
                        to_world = self
                            .screens
                            .as_ref()
                            .is_none_or(|s| world_takes_click(&g.ui, s, x, y));
                    }
                    g.ui.mouse_button(x, y, pressed);
                    if pressed && !to_world && self.live.is_some() && self.screens.is_some() {
                        g.ui.clicked();
                    }
                }
                if to_world {
                    self.click(x, y);
                }
            }
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Right,
                ..
            } => {
                let pressed = state == ElementState::Pressed;
                let (x, y) = self.cursor;
                // Over the interface (or with a menu up) a right click is the interface's (an
                // item's actions); elsewhere it turns the camera.
                if pressed {
                    if let (Some(g), Some(s)) = (self.gfx.as_mut(), self.screens.as_ref()) {
                        if !s.exploring() || g.ui.covers(x, y) {
                            g.ui.click_as(thedas_ui::avm1::MOUSE_RIGHT, x, y);
                            return;
                        }
                    }
                }
                // A right click drops an ability waiting for its target.
                if pressed {
                    if let Some(aim) = self.live.as_mut().and_then(|l| l.aiming.take()) {
                        log::info!("ability {} not aimed", aim.ability);
                        return;
                    }
                }
                self.looking = pressed;
                if let Some(g) = &self.gfx {
                    g.window.set_cursor_visible(!self.looking);
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let steps = match delta {
                    winit::event::MouseScrollDelta::LineDelta(_, y) => y,
                    winit::event::MouseScrollDelta::PixelDelta(p) => p.y as f32 / 40.0,
                };
                if let Some(f) = self.follow.as_mut() {
                    f.zoom(steps);
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
                match self.follow.as_mut() {
                    Some(f) => f.orbit(-delta.0 as f32 * s, -delta.1 as f32 * s),
                    None => self.camera.look(-delta.0 as f32 * s, -delta.1 as f32 * s),
                }
            }
        }
    }

    fn about_to_wait(&mut self, _: &ActiveEventLoop) {
        if let Some(g) = &self.gfx {
            g.window.request_redraw();
        }
    }
}

/// The quickbar slot a digit key uses (1 is the first, 0 the tenth).
fn quickslot_key(code: KeyCode) -> Option<i32> {
    Some(match code {
        KeyCode::Digit1 => 0,
        KeyCode::Digit2 => 1,
        KeyCode::Digit3 => 2,
        KeyCode::Digit4 => 3,
        KeyCode::Digit5 => 4,
        KeyCode::Digit6 => 5,
        KeyCode::Digit7 => 6,
        KeyCode::Digit8 => 7,
        KeyCode::Digit9 => 8,
        KeyCode::Digit0 => 9,
        _ => return None,
    })
}

/// Whether a left click at window pixel (`x`, `y`) goes to the world: only the HUD is up
/// and it draws nothing there.
fn world_takes_click(layer: &ui::UiLayer, screens: &screens::Screens, x: f64, y: f64) -> bool {
    screens.exploring() && !layer.covers(x, y)
}

/// One step of `--steps`.
#[derive(Clone, Debug, PartialEq)]
enum Step {
    Click(f64, f64),
    Run(u64),
    Inventory,
    /// The leader is ordered to talk to the first creature with this tag (as a click on it
    /// does, for creatures hard to reach on screen).
    Talk(String),
    /// The objects' scene is rebuilt, as when a creature spawns (timing).
    Rebuild,
    /// A right click or a double click at a pixel, for the interface.
    ClickAs(f64, f64, f64),
}

fn parse_steps(spec: &str) -> Result<Vec<Step>> {
    spec.split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| {
            let (verb, arg) = s.split_once(' ').unwrap_or((s, ""));
            let arg = arg.trim();
            Ok(match verb {
                "click" => {
                    let (x, y) = arg.split_once(',').context("click takes x,y")?;
                    Step::Click(x.trim().parse()?, y.trim().parse()?)
                }
                "run" => Step::Run(arg.parse().context("run takes milliseconds")?),
                "key" if arg.eq_ignore_ascii_case("i") => Step::Inventory,
                "talk" if !arg.is_empty() => Step::Talk(arg.to_string()),
                "rebuild" => Step::Rebuild,
                "rclick" | "dclick" => {
                    let (x, y) = arg.split_once(',').context("click takes x,y")?;
                    let button = if verb == "rclick" {
                        thedas_ui::avm1::MOUSE_RIGHT
                    } else {
                        thedas_ui::avm1::MOUSE_LEFT + thedas_ui::avm1::DOUBLE_CLICK
                    };
                    Step::ClickAs(button, x.trim().parse()?, y.trim().parse()?)
                }
                _ => bail!("unknown step {s:?}"),
            })
        })
        .collect()
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
    let start = if let Some(save) = &args.save {
        Some(live::Start::Save(save))
    } else if args.new_game {
        Some(live::Start::NewGame)
    } else if args.objects {
        Some(live::Start::Area(&args.area))
    } else {
        None
    };
    let mut live = match start {
        Some(start) => Some(live::Live::open(
            &install,
            &args.module,
            start,
            &args.shaders,
        )?),
        None => None,
    };
    // The name character creation would have taken (its script calls SetName with it).
    if let (Some(l), Some(name), true) = (live.as_ref(), &args.name, args.new_game) {
        let mut sim = l.sim();
        if let Some(h) = sim.world.hero.and_then(|h| sim.world.get_mut(h)) {
            h.name = Some(name.trim().to_string()).filter(|n| !n.is_empty());
        }
    }
    // Playing, the scene comes from the game; the interface still reads the global stack.
    let area = if live.is_some() {
        "none"
    } else {
        args.area.as_str()
    };
    let loaded = load(&install, area, &args.shaders, &[])?;
    let hero = live.as_ref().and_then(|l| l.hero());
    let mut camera = match (&args.camera, &loaded.scene, hero) {
        (Some(spec), _, _) => Camera::parse(spec).context("camera must be x,y,z,yaw,pitch")?,
        (None, _, Some((pos, facing))) => hero_camera(pos, facing),
        (None, Some(scene), None) => default_camera(scene),
        (None, None, None) => Camera::new(Vec3::ZERO, 0.0, 0.0),
    };
    camera.fov_y = args.fov.clamp(10.0, 150.0).to_radians();
    let ui = args
        .ui
        .as_deref()
        .map(thedas_ui::stage::movie_names)
        .unwrap_or_default();

    // Playing, the camera follows the party leader from behind.
    let mut follow = match (&live, hero) {
        (Some(_), Some((_, facing))) if args.camera.is_none() => Some(follow::Follow::behind(
            thedas_sim::world::heading_of(facing).to_radians(),
        )),
        _ => None,
    };
    let steps = parse_steps(&args.steps)?;
    if let Some(out) = &args.capture {
        let ctx = GpuContext::headless()?;
        let mut played: Option<(ui::UiLayer, screens::Screens)> = None;
        if let Some(l) = live.as_mut() {
            if ui.is_empty() {
                let mut layer = ui::UiLayer::new(&ctx, &loaded.vfs, &[], size);
                layer.attach(&l.sim);
                let mut s = screens::Screens::default();
                l.aiming = layer.play(&mut s, &l.sim, 0.0).pop();
                played = Some((layer, s));
            }
            if let (Some(f), Some(feet)) = (follow.as_mut(), l.leader_feet()) {
                camera = f.camera(feet, 0.0, &camera);
            }
            for c in &args.click {
                let (x, y) = c
                    .split_once(',')
                    .and_then(|(x, y)| {
                        Some((x.trim().parse::<f32>().ok()?, y.trim().parse::<f32>().ok()?))
                    })
                    .context("--click takes x,y")?;
                let (o, d) = camera.ray(x, y, size.0 as f32, size.1 as f32);
                match l.pick(o, d).and_then(|p| l.click(p)) {
                    Some(order) => println!("click {x},{y}: {order}"),
                    None => println!("click {x},{y}: nothing"),
                }
            }
            l.sim().tick(args.run_ms);
            for step in &steps {
                match step {
                    &Step::ClickAs(button, x, y) => {
                        if let Some((layer, _)) = played.as_mut() {
                            layer.click_as(button, x, y);
                            println!("click {x},{y} as button {button}: interface");
                        }
                    }
                    Step::Rebuild => {
                        l.sync(&ctx, thedas_render::gpu::CAPTURE_FORMAT)?;
                        l.objects = None;
                        let t = Instant::now();
                        l.sync(&ctx, thedas_render::gpu::CAPTURE_FORMAT)?;
                        println!("rebuild: {:.3}s", t.elapsed().as_secs_f64());
                    }
                    Step::Talk(tag) => {
                        let mut sim = l.sim();
                        let who = sim.world.controlled();
                        let target = sim.world.by_tag(tag).next().map(|o| o.id);
                        match (who, target) {
                            (Some(w), Some(t)) => {
                                sim.order_talk(w, t);
                                println!("talk to {tag}");
                            }
                            _ => println!("talk to {tag}: nobody"),
                        }
                    }
                    &Step::Click(x, y) => {
                        let to_world = match played.as_mut() {
                            Some((layer, s)) => {
                                layer.render_movies();
                                let w = world_takes_click(layer, s, x, y);
                                layer.mouse_move(x, y);
                                layer.mouse_button(x, y, true);
                                layer.mouse_button(x, y, false);
                                if !w {
                                    layer.clicked();
                                }
                                w
                            }
                            None => true,
                        };
                        if !to_world {
                            println!("click {x},{y}: interface");
                            continue;
                        }
                        let (o, d) = camera.ray(x as f32, y as f32, size.0 as f32, size.1 as f32);
                        match l.pick(o, d).and_then(|p| l.click(p)) {
                            Some(order) => println!("click {x},{y}: {order}"),
                            None => println!("click {x},{y}: nothing"),
                        }
                    }
                    &Step::Run(ms) => {
                        const FRAME: f64 = 1000.0 / 30.0;
                        let started = Instant::now();
                        let mut ui_time = std::time::Duration::ZERO;
                        let mut frames = 0u32;
                        let mut left = ms as f64;
                        while left > 0.0 {
                            let dt = left.min(FRAME);
                            left -= dt;
                            if !played.as_ref().is_some_and(|(_, s)| s.world_stopped()) {
                                l.step(dt);
                            }
                            // As the window does: the drawn scenes follow the world (objects
                            // that come and go are rebuilt).
                            l.sync(&ctx, thedas_render::gpu::CAPTURE_FORMAT)?;
                            if let Some((layer, s)) = played.as_mut() {
                                let t = Instant::now();
                                if let Some(aim) = layer.play(s, &l.sim, dt).pop() {
                                    println!("aiming ability {}", aim.ability);
                                    l.aiming = Some(aim);
                                }
                                ui_time += t.elapsed();
                            }
                            frames += 1;
                            if let (Some(f), Some(feet)) = (follow.as_mut(), l.leader_feet()) {
                                camera = f.camera(feet, dt as f32 / 1000.0, &camera);
                            }
                        }
                        if let Some((_, s)) = played.as_ref() {
                            println!(
                                "run {ms}: screens {:?} ({frames} frames in {:.2}s, interface {:.1} ms a frame)",
                                s.shown(),
                                started.elapsed().as_secs_f64(),
                                ui_time.as_secs_f64() * 1000.0 / frames.max(1) as f64
                            );
                        }
                    }
                    Step::Inventory => {
                        if let Some((layer, s)) = played.as_mut() {
                            s.inventory = !s.inventory;
                            if let Some(aim) = layer.play(s, &l.sim, 0.0).pop() {
                                l.aiming = Some(aim);
                            }
                        }
                    }
                }
            }
            let sim = l.sim();
            if let Some(h) = sim.world.controlled().and_then(|h| sim.world.get(h)) {
                log::info!(
                    "leader at {:?}, doing {:?}, saying: {:?}",
                    h.position,
                    h.commands
                        .iter()
                        .map(|c| c.name.as_str())
                        .collect::<Vec<_>>(),
                    sim.dialog_state().map(|d| (d.line, d.replies, d.ended))
                );
            }
            drop(sim);
            if let (Some(f), Some(feet)) = (follow.as_mut(), l.leader_feet()) {
                camera = f.camera(feet, 0.0, &camera);
            }
        }
        return capture(
            &ctx,
            &loaded,
            live.as_mut(),
            out,
            camera,
            size,
            &ui,
            args.ui_frames,
            played.as_mut().map(|(layer, _)| layer),
        );
    }

    let event_loop = EventLoop::new()?;
    let screens = (live.is_some() && ui.is_empty()).then(screens::Screens::default);
    let mut app = App {
        loaded,
        live,
        follow,
        screens,
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
        last_press: None,
        swallow_release: false,
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
    fn parses_steps() {
        assert_eq!(
            parse_steps("click 10,20.5; run 300 ;key i;").unwrap(),
            vec![Step::Click(10.0, 20.5), Step::Run(300), Step::Inventory]
        );
        assert_eq!(
            parse_steps("talk lot110cr_bryant").unwrap(),
            vec![Step::Talk("lot110cr_bryant".into())]
        );
        assert_eq!(parse_steps("rebuild").unwrap(), vec![Step::Rebuild]);
        assert_eq!(
            parse_steps("rclick 1,2; dclick 3,4").unwrap(),
            vec![Step::ClickAs(2.0, 1.0, 2.0), Step::ClickAs(5.0, 3.0, 4.0)]
        );
        assert!(parse_steps("dance").is_err());
        assert!(parse_steps("click 10").is_err());
        assert!(parse_steps("").unwrap().is_empty());
    }

    #[test]
    fn parses_sizes() {
        assert_eq!(parse_size("1280x720").unwrap(), (1280, 720));
        assert_eq!(parse_size("64X32").unwrap(), (64, 32));
        assert!(parse_size("0x10").is_err());
        assert!(parse_size("wide").is_err());
    }
}
