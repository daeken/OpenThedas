//! `thedas-app`: walk through one Dragon Age: Origins area.
//!
//! Interactive: WASD to move, Q/E (or C/Space) down/up, Shift to move faster, hold the right
//! mouse button (or use the arrow keys) to look around, P to print the camera, F12 to save a
//! capture, Esc to quit.
//!
//! Headless: `--capture out.png --camera x,y,z,yaw,pitch` renders one frame with no window.

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
    /// Environment level (e.g. lgt101d) or area resource (e.g. cir110ar_inn).
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
    /// Vertical field of view in degrees.
    #[arg(long, default_value_t = 60.0)]
    fov: f32,
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
    vfs: Vfs,
    scene: AreaScene,
}

/// Opens the install, mounts the level (resolving an area name through its `.are` file) and
/// loads the scene.
fn load(install: &Path, area: &str) -> Result<Loaded> {
    let install = DaoInstall::open(install)?;
    let mut vfs = install.global_vfs(TextureQuality::High)?;
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
    let scene = AreaScene::load(&vfs, &level)?;
    log::info!(
        "{level} ({}): {} rooms, {} draws, {} meshes, {} materials, {} warnings, loaded in {:.2}s",
        scene.name.as_deref().unwrap_or("?"),
        scene.rooms.len(),
        scene.draws.len(),
        scene.meshes.len(),
        scene.materials.len(),
        scene.warnings.len(),
        started.elapsed().as_secs_f64()
    );
    Ok(Loaded { vfs, scene })
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

fn capture(loaded: &Loaded, out: &Path, camera: Camera, (w, h): (u32, u32)) -> Result<()> {
    let ctx = GpuContext::headless()?;
    let mut renderer = AreaRenderer::new(
        &ctx,
        &loaded.vfs,
        &loaded.scene,
        &ProgramRegistry::default(),
        thedas_render::gpu::CAPTURE_FORMAT,
    );
    log_stats(&renderer);
    let pixels = renderer.capture(&ctx, &camera, w, h)?;
    write_png(out, w, h, &pixels)?;
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
    renderer: AreaRenderer,
}

struct App {
    loaded: Loaded,
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
                    .with_title(format!(
                        "OpenThedas - {} ({})",
                        self.loaded.scene.name.as_deref().unwrap_or(""),
                        self.loaded.scene.level
                    ))
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
        let renderer = AreaRenderer::new(
            &ctx,
            &self.loaded.vfs,
            &self.loaded.scene,
            &ProgramRegistry::default(),
            config.format,
        );
        log_stats(&renderer);
        println!("camera {}", self.camera.spec());
        self.gfx = Some(Gfx {
            window,
            surface,
            config,
            ctx,
            renderer,
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
        g.renderer.render(
            &g.ctx,
            &mut encoder,
            &view,
            g.config.width,
            g.config.height,
            &self.camera,
        );
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
        let path = PathBuf::from(format!(
            "{}-capture-{}.png",
            self.loaded.scene.level, self.captures
        ));
        let (w, h) = (g.config.width, g.config.height);
        let pixels = g.renderer.capture(&g.ctx, &self.camera, w, h)?;
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
    let loaded = load(&install, &args.area)?;
    let mut camera = match &args.camera {
        Some(spec) => Camera::parse(spec).context("camera must be x,y,z,yaw,pitch")?,
        None => default_camera(&loaded.scene),
    };
    camera.fov_y = args.fov.clamp(10.0, 150.0).to_radians();

    if let Some(out) = &args.capture {
        return capture(&loaded, out, camera, size);
    }

    let event_loop = EventLoop::new()?;
    let mut app = App {
        loaded,
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
