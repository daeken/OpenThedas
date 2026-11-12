//! `thedas-app`: play Dragon Age: Origins, or walk through an area.
//!
//! With only `--install`, the game boots as the original does: the EA logo and intro movies
//! (decoded by `ffmpeg`, silent for now; a key, a click or the pad's A, B or Start skips
//! one; `--no-intro` skips them), then its start menu (startmenu2.gfx: New Game, Load,
//! Options, Quit); in a game Escape (or the pad's Start) opens the game menu (mainmenu.gfx),
//! whose Exit Game returns to the start menu.
//!
//! The viewer (`--viewer`, or `--area`/`--camera`): WASD to move, Q/E (or C) down/up, Shift to move faster, hold the right
//! mouse button (or use the arrow keys) to look around, P to print the camera (when not
//! playing), F12 to save a
//! capture, Esc to quit.
//!
//! Playing: `--save <file>` (a save from the MCP's save_state) or `--new-game` (through the
//! character creation screens; `--quick-start` skips them) runs the simulation live at real
//! time; Space pauses it. The camera follows the party leader: hold
//! the right mouse button to orbit, the wheel to zoom, Q/E to turn; F switches to the
//! free-flying camera and back. A left click orders the leader: walk to the ground clicked,
//! attack a hostile, talk to someone, use a door or container. F1-F4 choose the leader;
//! 1-9 and 0 use the quickbar's slots (an ability aimed at someone waits for a click on its
//! target; a right click drops it).
//! The game's own interface plays over the world: the HUD while exploring, the conversation
//! screen (click a reply), and the screens scripts open (world map, store, job board,
//! level-up); I opens the inventory. A click the interface draws under is the interface's.
//! A cutscene that plays a movie (New Game's intro, `game_intro.cut`) shows the movie full
//! screen for its span, silent; a key, a click or the pad's A, B or Start skips the cutscene.
//!
//! Headless: `--capture out.png --camera x,y,z,yaw,pitch` renders one frame with no window.
//! Playing headless, `--steps "click 640,400; run 3000; click 300,650; key i"` plays input
//! in order (clicks go to the interface or the world as in the window) before the capture;
//! `talk <tag>` orders the leader to talk to a creature as a click on it would.
//!
//! `--ui menu`, `--ui hud` or `--ui movie1,movie2` plays Scaleform movies over the scene;
//! `--area none` shows them alone.
//!
//! A game controller (an Xbox pad; see [`pad`] for the whole map) plays too: the left stick
//! walks the leader, the right stick turns and zooms the camera, A acts on the target in
//! front, X/Y/B (RT for a second set) use the quickbar, LB/RB change the leader, LT opens a
//! quickbar picker, the D-pad changes the target, Back opens the player's screens and Start
//! the game menu (pausing is in the quickbar picker). On screens the stick moves a pointer that A clicks and B closes; in conversations
//! the D-pad or stick picks a reply and A chooses it. The PC original has no controller
//! support: this is a deliberate addition, following the Xbox 360 version's controls.
//!
//! `--control 127.0.0.1:7878` lets a local client (`thedas-mcp`'s `attach`) drive the
//! running game: the same steps as `--steps`, screenshots and the simulation's state (see
//! [`control`]).

mod audio;
mod camfade;
mod chargen_stage;
mod control;
mod cutactors;
mod cutcam;
mod focusnav;
mod follow;
mod intro;
mod live;
mod pad;
mod paddev;
mod padplay;
mod reticle;
mod saves;
mod screens;
mod talkcam;
mod ui;

use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use clap::Parser;
use glam::Vec3;
use serde_json::{Value as J, json};
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
    /// Environment level (e.g. lgt101d), area resource (e.g. cir110ar_inn), or `none`, for
    /// the viewer (lgt101d when not given). Giving it opens the viewer.
    #[arg(long)]
    area: Option<String>,
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
    /// `standin` (hand-written WGSL only). `original+Foo.vsh` adds vertex shaders to the
    /// original set.
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
    /// Start a new game: the module's start, through the character creation screens.
    #[arg(long)]
    new_game: bool,
    /// The area viewer (a free camera over `--area`) instead of the game. With none of
    /// `--save`, `--new-game`, `--objects`, `--ui`, `--area` or `--camera`, the game starts at
    /// its start menu.
    #[arg(long)]
    viewer: bool,
    /// With `--new-game`: skip the character creation screens (the chargen script's quick
    /// start: a pre-made character and origin).
    #[arg(long)]
    quick_start: bool,
    /// With `--new-game --quick-start`: the hero's name (quick start does not ask). The
    /// creation screen names the hero itself: its name entry offers the background's default
    /// name, which stands, as nothing types into the movies yet.
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
    /// that long), `key i` (the inventory toggle; `c`, `t`, `m`, `j` and `o` likewise toggle the
    /// character record, tactics, area map, journal and options), `key f1`-`key f4` (who
    /// leads), `key 1`-`key 0` (quickbar slots), `key space` (pause), `talk TAG` (the leader
    /// talks to a creature), `cutscene NAME` (a cutscene plays, as a script would play it),
    /// `reply N` (the conversation's reply N, from 0), `press X,Y` and `release X,Y` (the
    /// left button down and up, for drags), `rebuild` (the objects are rebuilt as a
    /// spawn rebuilds them, timed), `picker` (the party picker opens as a script would open
    /// it), `wipe` (the whole party falls), `key f5` and `key f9` (the save and load
    /// screens), `rclick X,Y` and `dclick X,Y` (a right or double click
    /// for the interface), `rpress X,Y` and `rrelease X,Y` (the right button down and up,
    /// for an item's radial menu), `hover X,Y` (the mouse rests there, for the aiming
    /// reticle), `pad a` (a game controller's button pressed and let go; `pad press rt`
    /// and `pad release rt` hold and let go), `pad stick-left X,Y MS` and
    /// `pad stick-right X,Y MS` (a stick held there while the game runs MS, then let go).
    #[arg(long, default_value = "")]
    steps: String,
    /// Module for `--save` and `--objects`.
    #[arg(long, default_value = "single player")]
    module: String,
    /// Window, playing: take input from local clients on this loopback address
    /// (`127.0.0.1:7878`), as `thedas-mcp`'s `attach` sends it.
    #[arg(long)]
    control: Option<String>,
    /// The level's frame buffer effects (bloom), as the original's "Frame-buffer effects"
    /// video option: `on` or `off`.
    #[arg(long, default_value = "on", value_parser = ["on", "off"])]
    frame_effects: String,
    /// The original's "Graphics Detail" video option, which picks the materials' techniques
    /// and the bloom's blur: `low`, `medium` or `high` (High's materials draw as Medium's; its
    /// sun shadows are 1024 texels a side and reach further).
    #[arg(long, default_value = "medium", value_parser = ["low", "medium", "high"])]
    detail: String,
    /// Headless: the shaders' clock (`ApplicationTime`) at the capture, in seconds; it runs
    /// on from there through `run` steps. The clouds drift with it.
    #[arg(long, default_value_t = 0.0)]
    time: f64,
    /// Where the player's games are saved and loaded (F5 the save screen, F9 the load
    /// screen, the death screen's load). Defaults to `$HOME/.openthedas/saves`.
    #[arg(long)]
    saves: Option<PathBuf>,
    /// List the game controllers gilrs sees (names, ids, mapping, which inputs it maps) and
    /// print their events for this many seconds, then exit. No window, no install needed.
    #[arg(long)]
    pad_probe: Option<f64>,
    /// Booting to the start menu: skip the logo and intro movies the game plays first.
    #[arg(long)]
    no_intro: bool,
    /// No sound. Sound is also off with `--control` (clients driving the game headless or
    /// unattended) unless `--audio` asks for it, and when THEDAS_NO_AUDIO is set.
    #[arg(long)]
    no_audio: bool,
    /// Sound even with `--control`.
    #[arg(long)]
    audio: bool,
    /// Headless (`--steps`): record the game's sound to this WAV file (stereo, 48 kHz),
    /// never to the speakers.
    #[arg(long)]
    audio_wav: Option<PathBuf>,
    /// The voice-over language folder (`audio/vo/<language>`).
    #[arg(long, default_value = "en-us")]
    language: String,
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
    /// Whether frames get the level's frame buffer effects (`--frame-effects`).
    frame_effects: bool,
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
        // `original+Foo.vsh+...` adds vertex shaders (with any pixel shader) to the
        // original set, for trying shaders not yet drawn by default.
        let mut parts = shaders.split('+');
        let base = parts.next().unwrap_or_default();
        let mut filter = match base {
            "standin" => return Ok(reg),
            "original" => thedas_render::d3d::object_filter(),
            "static" => thedas_render::d3d::static1_filter(),
            "all" => None,
            other => bail!("--shaders must be original, static, all or standin, not {other}"),
        };
        if let Some(f) = filter.as_mut() {
            f.extend(parts.map(|vs| (vs.to_string(), "*".to_string())));
        }
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
    detail: thedas_render::scene::Detail,
) -> Result<Loaded> {
    let install = DaoInstall::open(install)?;
    let mut vfs = install.global_vfs(TextureQuality::High)?;
    if area == "none" {
        return Ok(Loaded {
            vfs: Arc::new(vfs),
            scene: None,
            shaders: shaders.to_string(),
            frame_effects: true,
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
    let scene = AreaScene::load_with_props_at(&vfs, &level, props, detail)?;
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
        frame_effects: true,
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
        "renderer: {} draws ({} skipped), {} emitters ({} drawn), {} meshes, {} textures ({} compressed, {} missing), programs {:?}",
        s.draws,
        s.draws_skipped,
        s.emitters,
        s.particle_draws,
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
    cursor: (f64, f64),
    shader_time: f64,
    (overlay, pointer): (Vec<[glam::Vec2; 2]>, Vec<[glam::Vec2; 2]>),
) -> Result<()> {
    let mut reticle = overlay;
    let pixels = if let Some(live) = live {
        live.frame_size = (w, h);
        live.sync(ctx, thedas_render::gpu::CAPTURE_FORMAT)?;
        // A load still building on its worker (THEDAS_ASYNC_LOADS) finishes first.
        while live.loading() {
            std::thread::sleep(std::time::Duration::from_millis(5));
            live.sync(ctx, thedas_render::gpu::CAPTURE_FORMAT)?;
        }
        if let Some(l) = live.level.as_ref() {
            log_stats(&l.renderer);
        }
        reticle.extend(live.reticle(&camera, (w, h), cursor));
        // THEDAS_BENCH_FRAMES=n: draws the frame n more times first and reports how long
        // each took (with its read back to memory), for comparing builds.
        if let Some(n) = std::env::var("THEDAS_BENCH_FRAMES")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .filter(|n| *n > 0)
        {
            let mut ms = Vec::new();
            for _ in 0..n {
                let t = Instant::now();
                live.capture(ctx, &camera, (w, h))?;
                ms.push(t.elapsed().as_secs_f64() * 1000.0);
            }
            let avg = ms.iter().sum::<f64>() / ms.len() as f64;
            let worst = ms.iter().cloned().fold(0.0, f64::max);
            println!("bench: {n} frames, avg {avg:.1} ms, worst {worst:.1} ms");
            // THEDAS_BENCH_DUMP: every frame's time too, in order (to tell a first-frame
            // cost from a recurring one).
            if std::env::var_os("THEDAS_BENCH_DUMP").is_some() {
                let all: Vec<String> = ms.iter().map(|m| format!("{m:.1}")).collect();
                println!("bench frames: {}", all.join(" "));
            }
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
                renderer.frame_effects = loaded.frame_effects;
                renderer.set_time(shader_time);
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
    reticle::draw_rgba(&mut frame.pixels, (w, h), &reticle);
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
    // The pad's pointer goes over the interface.
    reticle::draw_rgba(&mut frame.pixels, (w, h), &pointer);
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
    reticle: reticle::ReticlePass,
}

struct App {
    /// A loading screen is up for the level being built (see `Live::loading`).
    load_screen_up: bool,
    loaded: Loaded,
    /// The running game, when playing.
    live: Option<live::Live>,
    /// The game camera, when following the party leader (playing; F toggles free-fly).
    follow: Option<follow::Follow>,
    /// The game's screens over a running game (playing without `--ui`).
    screens: Option<screens::Screens>,
    ui: Vec<String>,
    /// Where the player's games are saved.
    saves_dir: PathBuf,
    cursor: (f64, f64),
    camera: Camera,
    size: (u32, u32),
    gfx: Option<Gfx>,
    keys: HashSet<KeyCode>,
    looking: bool,
    /// The right button went down over the interface: its release is the interface's too (an
    /// item's radial menu opens on the press and acts on the release).
    ui_right: bool,
    last: Instant,
    error: Option<anyhow::Error>,
    captures: u32,
    /// The last left press (when, where), to tell a double click.
    last_press: Option<(Instant, (f64, f64))>,
    /// The release of a double click the interface already had.
    swallow_release: bool,
    /// Where clients' requests arrive (`--control`).
    control: Option<control::Control>,
    /// Clients' input still playing, first come first played.
    remote: VecDeque<Remote>,
    /// Clients waiting for the next presented frame, and how much smaller they want it.
    frame_requests: Vec<(std::sync::mpsc::Sender<J>, u32)>,
    /// Game controllers (none headless, or where the platform has no support).
    pads: Option<paddev::Pads>,
    /// The pad's part of the game: its mapping's state, the target, the reply picked.
    pad: padplay::PadPlay,
    /// The pad as clients' `pad` steps hold it (merged with the device's).
    remote_pad: pad::PadState,
    /// The install, module and shaders a game the start menu starts is opened with.
    install: PathBuf,
    module: String,
    shaders: String,
    /// Exit was chosen on the start menu.
    quit: bool,
    /// What the menus asked this frame, acted on once it is done.
    requests: Vec<ui::FrontEnd>,
    /// A game opening on a worker (what was asked, its result to come, when it started).
    opening: Option<(
        String,
        std::sync::mpsc::Receiver<Result<live::Live>>,
        Instant,
    )>,
    /// The boot movies, while they play (before the start menu takes input).
    intro: Option<intro::Intro>,
    /// Music, interface sounds and voice-over (none with `--no-audio` or no output device).
    audio: Option<audio::GameAudio>,
    /// The pad's skip buttons were held last frame (a skip is a fresh press).
    intro_pad_held: bool,
    /// A zoom key a step holds down (`key pagedown MS`): +1 in, -1 out, 0 none.
    zoom_key: f32,
}

/// What the window's surface gave a redraw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SurfaceState {
    /// A frame to draw into and present.
    Ready,
    /// The surface must be configured again (outdated or lost); no frame this time.
    Stale,
    /// No frame for now (timed out or occluded: the window hidden, the display asleep).
    Unavailable,
}

/// A redraw's work for a surface state: the game always runs (the simulation, the clients'
/// requests and the interface tick whether or not anything is drawn, so a hidden window
/// neither freezes the game nor leaves a client waiting); only drawing needs a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FramePlan {
    /// The frame is drawn and presented (and waiting clients read it back).
    draw: bool,
    /// The surface is configured again first.
    reconfigure: bool,
    /// Clients waiting for a presented frame get one drawn offscreen instead.
    offscreen_frames: bool,
}

fn frame_plan(surface: SurfaceState, frames_waiting: bool) -> FramePlan {
    let draw = surface == SurfaceState::Ready;
    FramePlan {
        draw,
        reconfigure: surface == SurfaceState::Stale,
        offscreen_frames: !draw && frames_waiting,
    }
}

/// The interface's frame: the screens shown over a game (and the aims they start), or the
/// movies alone.
fn play_interface(
    ui: &mut ui::UiLayer,
    screens: Option<&mut screens::Screens>,
    live: Option<&mut live::Live>,
    camera: &Camera,
    dt: f64,
) {
    match (screens, live) {
        (Some(screens), Some(live)) => {
            let level = live.level.as_ref();
            ui.set_level(level.map(|l| l.layout.as_str()), level.map(|l| &l.vfs));
            ui.set_paused(live.paused);
            for aim in ui.play(screens, &live.sim, dt.min(100.0)) {
                log::info!("aiming ability {} (click a target)", aim.ability);
                live.aiming = Some(aim);
            }
            ui.update_minimap(&live.sim, camera);
            ui.update_floaties(&live.sim, camera, dt.min(100.0));
        }
        _ => {
            for call in ui.tick(dt.min(100.0)) {
                log::debug!("ui call: {call:?}");
            }
        }
    }
}

/// A presented frame being copied out: the buffer and its size and row pitch.
struct FrameReadback {
    buffer: wgpu::Buffer,
    width: u32,
    height: u32,
    row: u32,
}

/// Copies the frame `texture` (as drawn so far by `encoder`) into a buffer to read.
fn read_frame(
    ctx: &GpuContext,
    encoder: &mut wgpu::CommandEncoder,
    texture: &wgpu::Texture,
) -> FrameReadback {
    let (width, height) = (texture.width(), texture.height());
    let row = (width * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
        * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("frame readback"),
        size: row as u64 * height as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
    FrameReadback {
        buffer,
        width,
        height,
        row,
    }
}

/// The copied frame, as opaque RGBA.
fn finish_read_frame(
    ctx: &GpuContext,
    r: FrameReadback,
    format: wgpu::TextureFormat,
) -> Result<thedas_ui::Rgba> {
    let (tx, rx) = std::sync::mpsc::channel();
    r.buffer.slice(..).map_async(wgpu::MapMode::Read, move |m| {
        let _ = tx.send(m);
    });
    ctx.device
        .poll(wgpu::PollType::wait_indefinitely())
        .map_err(|e| anyhow::anyhow!("reading the frame back: {e}"))?;
    rx.recv()?
        .map_err(|e| anyhow::anyhow!("reading the frame back: {e}"))?;
    let bgra = matches!(
        format,
        wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
    );
    let mapped = r
        .buffer
        .slice(..)
        .get_mapped_range()
        .map_err(|e| anyhow::anyhow!("reading the frame back: {e:?}"))?;
    let mut pixels = Vec::with_capacity((r.width * r.height * 4) as usize);
    for y in 0..r.height as usize {
        let line = &mapped[y * r.row as usize..][..r.width as usize * 4];
        for p in line.chunks_exact(4) {
            let (red, blue) = if bgra { (p[2], p[0]) } else { (p[0], p[2]) };
            pixels.extend_from_slice(&[red, p[1], blue, 255]);
        }
    }
    Ok(thedas_ui::Rgba {
        width: r.width,
        height: r.height,
        pixels,
    })
}

/// A presented frame as a control reply: a PNG in base64, `scale` times smaller (blocks
/// averaged), with the window's size.
fn frame_reply(frame: &thedas_ui::Rgba, scale: u32) -> Result<J> {
    let small = shrink(frame, scale);
    let png = thedas_ui::stage::encode_png(&small).context("encoding the frame")?;
    use base64::Engine as _;
    Ok(json!({
        "ok": true,
        "width": frame.width,
        "height": frame.height,
        "scale": scale,
        "presented": true,
        "png": base64::engine::general_purpose::STANDARD.encode(png),
    }))
}

/// `img` made `k` times smaller in each direction by averaging `k` x `k` blocks.
fn shrink(img: &thedas_ui::Rgba, k: u32) -> thedas_ui::Rgba {
    let k = k.max(1);
    let (w, h) = ((img.width / k).max(1), (img.height / k).max(1));
    if k == 1 {
        return thedas_ui::Rgba {
            width: img.width,
            height: img.height,
            pixels: img.pixels.clone(),
        };
    }
    let mut pixels = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let mut sum = [0u32; 4];
            let mut n = 0;
            for dy in 0..k {
                for dx in 0..k {
                    let (sx, sy) = (x * k + dx, y * k + dy);
                    if sx >= img.width || sy >= img.height {
                        continue;
                    }
                    let i = ((sy * img.width + sx) * 4) as usize;
                    for (c, s) in sum.iter_mut().enumerate() {
                        *s += img.pixels.get(i + c).copied().unwrap_or(0) as u32;
                    }
                    n += 1;
                }
            }
            pixels.extend(sum.map(|s| (s / n.max(1)) as u8));
        }
    }
    thedas_ui::Rgba {
        width: w,
        height: h,
        pixels,
    }
}

/// Steps a client sent, playing.
struct Remote {
    steps: VecDeque<Step>,
    /// Milliseconds of a `run` still to go.
    wait_ms: f64,
    log: Vec<String>,
    reply: std::sync::mpsc::Sender<J>,
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
                    .with_inner_size(LogicalSize::new(self.size.0, self.size.1))
                    .with_visible(std::env::var_os("THEDAS_HIDDEN_WINDOW").is_none()),
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
        // The control channel's screenshots read the presented frame back.
        if caps.usages.contains(wgpu::TextureUsages::COPY_SRC) {
            config.usage |= wgpu::TextureUsages::COPY_SRC;
        }
        surface.configure(&ctx.device, &config);
        let renderer = match &self.loaded.scene {
            Some(scene) => {
                let mut r = AreaRenderer::new(
                    &ctx,
                    &self.loaded.vfs,
                    scene,
                    &self.loaded.programs(scene)?,
                    config.format,
                );
                r.frame_effects = self.loaded.frame_effects;
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
            ui.set_saves(saves::SaveStore::new(self.saves_dir.clone()));
        }
        let reticle = reticle::ReticlePass::new(&ctx.device, config.format);
        self.gfx = Some(Gfx {
            window,
            surface,
            config,
            ctx,
            renderer,
            ui,
            reticle,
        });
        Ok(())
    }

    fn update(&mut self) {
        let now = Instant::now();
        let dt = (now - self.last).as_secs_f32().min(0.1);
        self.last = now;
        let k = |c| self.keys.contains(&c);
        // The camera waits for a load to finish: it resets as the world comes up.
        let loading = self.live.as_ref().is_some_and(|l| l.loading());
        if let (Some(f), Some(l), false) = (self.follow.as_mut(), self.live.as_mut(), loading) {
            if let Some(heading) = l.camera_reset_due() {
                f.reset(heading);
            }
            if let Some(level) = l.zoom_request_due() {
                f.set_zoom_level(level);
            }
        }
        if let Some(f) = self.follow.as_mut() {
            let turn = (k(KeyCode::KeyQ) as i32 - k(KeyCode::KeyE) as i32) as f32;
            f.orbit(turn * 1.8 * dt, 0.0);
            // Page Down zooms in and Page Up out while held, the original's second zoom
            // bindings (KeyBindings.ini: ZoomIn_1=Keyboard::Button_Next,
            // ZoomOut_1=Keyboard::Button_Prior). Provisional: the rate (the pad's).
            let zoom =
                (k(KeyCode::PageDown) as i32 - k(KeyCode::PageUp) as i32) as f32 + self.zoom_key;
            if zoom != 0.0
                && f.zoom(zoom * pad::ZOOM_SPEED * dt)
                && let Some(l) = self.live.as_ref()
            {
                l.sim().player_zoomed_in();
            }
            let eye = self
                .live
                .as_ref()
                .and_then(|l| l.follow_camera(f, dt, &self.camera));
            if let Some(c) = eye {
                self.camera = c;
            }
            // Creatures the game camera is inside fade (see `camfade`).
            if let Some(l) = self.live.as_mut() {
                l.fade_near_camera(eye.map(|c| c.position), dt);
            }
            // A staged conversation line frames itself.
            if let Some(c) = self
                .live
                .as_mut()
                .and_then(|l| l.conversation_camera(&self.camera))
            {
                self.camera = c;
            }
            // A playing cutscene's camera (see `cutcam`).
            if let Some(c) = self
                .live
                .as_mut()
                .and_then(|l| l.cutscene_camera(&self.camera))
            {
                self.camera = c;
            }
            // Character creation frames its stage (see `chargen_stage`).
            let preset = self.gfx.as_ref().map_or(0, |g| g.ui.paperdoll_preset());
            if let Some(c) = self
                .live
                .as_mut()
                .and_then(|l| l.chargen_camera(&self.camera, preset))
            {
                self.camera = c;
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
        // E up, Q down. C, which also lowered it, opens the character record while playing.
        let c_lowers = self.screens.is_none();
        let up = axis(
            k(KeyCode::KeyE),
            k(KeyCode::KeyQ) || (c_lowers && k(KeyCode::KeyC)),
        );
        self.camera.fly(fwd * speed, right * speed, up * speed);
        let turn = 1.5 * dt;
        let yaw = axis(k(KeyCode::ArrowLeft), k(KeyCode::ArrowRight));
        let pitch = axis(k(KeyCode::ArrowUp), k(KeyCode::ArrowDown));
        self.camera.look(yaw * turn, pitch * turn);
    }

    /// A frame of the boot movies: only the movie is drawn, and only a key, a click or the
    /// pad's A, B or Start does anything (it skips the movie playing). The start menu waits
    /// behind them.
    fn intro_frame(&mut self) -> Result<()> {
        let now = Instant::now();
        let dt = (now - self.last).as_secs_f64() * 1000.0;
        self.last = now;
        let state = self.pads.as_mut().map(|p| p.poll()).unwrap_or_default();
        let held = [pad::Button::A, pad::Button::B, pad::Button::Start]
            .iter()
            .any(|&b| state.held(b));
        let pressed = held && !self.intro_pad_held;
        self.intro_pad_held = held;
        let Some(intro) = self.intro.as_mut() else {
            return Ok(());
        };
        if pressed {
            intro.skip();
        }
        if !intro.advance(dt) {
            self.intro = None;
            // The start menu's clock starts now, not at boot.
            self.last = Instant::now();
            return Ok(());
        }
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
                label: Some("intro"),
            });
        let size = (g.config.width, g.config.height);
        intro.draw(
            &g.ctx.device,
            &g.ctx.queue,
            &mut encoder,
            &view,
            g.config.format,
            size,
        );
        g.ctx.queue.submit([encoder.finish()]);
        g.window.pre_present_notify();
        g.ctx.queue.present(frame);
        Ok(())
    }

    fn redraw(&mut self) -> Result<()> {
        self.audio_frame();
        if self.intro.is_some() {
            return self.intro_frame();
        }
        let dt = self.last.elapsed().as_secs_f64() * 1000.0;
        self.serve_control(dt);
        self.pad_frame(dt);
        self.update();
        let Some(g) = self.gfx.as_mut() else {
            return Ok(());
        };
        let (frame, surface) = match g.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f)
            | wgpu::CurrentSurfaceTexture::Suboptimal(f) => (Some(f), SurfaceState::Ready),
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                (None, SurfaceState::Stale)
            }
            _ => (None, SurfaceState::Unavailable),
        };
        let plan = frame_plan(surface, !self.frame_requests.is_empty());
        if plan.reconfigure {
            g.surface.configure(&g.ctx.device, &g.config);
        }
        debug_assert_eq!(plan.draw, frame.is_some());
        // No frame to draw into (the window hidden or occluded, the display asleep, a
        // resize): the game runs on all the same, only nothing is drawn, and clients waiting
        // for a presented frame get one drawn offscreen instead.
        let Some(frame) = frame else {
            self.tick_unpresented(dt)?;
            if plan.offscreen_frames {
                self.answer_frames_offscreen();
            }
            return Ok(());
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
            // A pending transition, or a world map journey, runs on behind its loading screen
            // (which stops the world otherwise); a training lesson's events are delivered
            // with a screen up too (its steps go on inside the inventory, the tactics), no
            // time passing (see `Live::step`).
            if !stopped || live.transition_pending() || live.journey() || live.training() {
                live.step(dt);
            }
            // While a level loads the world holds still: its presentation too.
            if !live.loading() {
                live.advance_presentation(dt);
            }
            live.frame_size = (g.config.width, g.config.height);
            live.sync(&g.ctx, g.config.format)?;
            if self.screens.is_some() {
                loading_screen(live, &mut g.ui, &mut self.load_screen_up);
            }
            // Portraits are drawn by the objects' renderers, whose constants this frame's
            // draws use too: they are drawn (and submitted) before the frame is encoded, so
            // the frame is not drawn from the portrait camera.
            if self.screens.is_some() {
                let key = ui::portrait_key(&live.sim());
                if g.ui.wants_portraits(key, dt) {
                    let shots = live.portraits(&g.ctx, ui::PORTRAIT_SIZE);
                    g.ui.set_portraits(shots);
                }
                let due = self
                    .screens
                    .as_ref()
                    .and_then(|s| g.ui.wants_paperdoll(s, &live.sim(), dt));
                if let Some((who, yaw)) = due {
                    let (w, h) = ui::PAPERDOLL_SIZE;
                    let shot = live.paperdoll(&g.ctx, who, yaw, w, h);
                    g.ui.set_paperdoll(shot);
                }
            }
        }
        let size = (g.config.width, g.config.height);
        let drawn = match (self.live.as_mut(), g.renderer.as_mut()) {
            (Some(live), _) => live.render(&g.ctx, &mut encoder, &view, size, &self.camera),
            (None, Some(r)) => {
                r.advance_time(dt / 1000.0);
                r.render(&g.ctx, &mut encoder, &view, size.0, size.1, &self.camera);
                true
            }
            (None, None) => false,
        };
        // The level's frame buffer effects go over the world, under the interface; they copy
        // the frame, which needs a surface that allows it.
        if drawn && g.config.usage.contains(wgpu::TextureUsages::COPY_SRC) {
            let texture = &frame.texture;
            match (self.live.as_mut(), g.renderer.as_mut()) {
                (Some(live), _) => live.finish_frame(&g.ctx, &mut encoder, texture, size),
                (None, Some(r)) => r.finish_frame(&g.ctx, &mut encoder, texture, size.0, size.1),
                (None, None) => {}
            }
        }
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
        g.ui.set_pad_active(self.pad.used);
        play_interface(
            &mut g.ui,
            self.screens.as_mut(),
            self.live.as_mut(),
            &self.camera,
            dt,
        );
        self.requests.append(&mut g.ui.requests);
        // An ability aimed at a place shows where it would land, under the interface.
        if let Some(live) = self.live.as_mut() {
            let mut lines = live.reticle(&self.camera, size, self.cursor);
            let ctx = self
                .pad
                .context_for(self.screens.as_ref(), g.ui.stage.dialog_up());
            lines.extend(self.pad.overlay(Some(&*live), &self.camera, size, ctx));
            g.reticle
                .draw(&g.ctx.device, &mut encoder, &view, size, &lines);
        }
        g.ui.composite(&mut encoder, &view, g.config.format);
        // The pad's pointer goes over the interface.
        let ctx = self
            .pad
            .context_for(self.screens.as_ref(), g.ui.stage.dialog_up());
        let pointer = self.pad.pointer(self.cursor, ctx);
        g.reticle
            .draw(&g.ctx.device, &mut encoder, &view, size, &pointer);
        let readback = (!self.frame_requests.is_empty())
            .then(|| read_frame(&g.ctx, &mut encoder, &frame.texture));
        g.ctx.queue.submit([encoder.finish()]);
        if let Some(readback) = readback {
            let frame = finish_read_frame(&g.ctx, readback, g.config.format);
            for (reply, scale) in self.frame_requests.drain(..) {
                let _ = reply.send(match &frame {
                    Ok(f) => frame_reply(f, scale).unwrap_or_else(control::failure),
                    Err(e) => control::failure(e),
                });
            }
        }
        g.window.pre_present_notify();
        g.ctx.queue.present(frame);
        Ok(())
    }

    /// The sound follows the game: the menus' music, the interface's sounds, the open
    /// conversation line's voice (see [`audio`]).
    fn audio_frame(&mut self) {
        let Some(a) = self.audio.as_mut() else {
            return;
        };
        let movie = self
            .intro
            .as_ref()
            .and_then(|i| i.now_playing())
            .map(|(p, ms)| (p.to_path_buf(), ms));
        let live = self.live.as_mut().filter(|_| self.intro.is_none());
        let ui = self.gfx.as_mut().map(|g| &mut g.ui);
        let state = audio_state(a, live, ui, &self.camera, movie);
        a.frame(state);
        let voice = a.voice_ms();
        if let Some(g) = self.gfx.as_mut() {
            g.ui.set_line_voice_ms(voice);
        }
    }

    /// A frame with nothing to draw into: the simulation, its presentation and the interface
    /// run as on a drawn frame (see [`App::redraw`]); the drawing alone is skipped.
    fn tick_unpresented(&mut self, dt: f64) -> Result<()> {
        let Some(g) = self.gfx.as_mut() else {
            return Ok(());
        };
        let stopped = self.screens.as_ref().is_some_and(|s| s.world_stopped());
        if let Some(live) = self.live.as_mut() {
            // A pending transition, or a world map journey, runs on behind its loading screen
            // (which stops the world otherwise); a training lesson's events are delivered
            // with a screen up too (its steps go on inside the inventory, the tactics), no
            // time passing (see `Live::step`).
            if !stopped || live.transition_pending() || live.journey() || live.training() {
                live.step(dt);
            }
            // While a level loads the world holds still: its presentation too.
            if !live.loading() {
                live.advance_presentation(dt);
            }
            live.frame_size = (g.config.width, g.config.height);
            live.sync(&g.ctx, g.config.format)?;
            if self.screens.is_some() {
                loading_screen(live, &mut g.ui, &mut self.load_screen_up);
            }
        }
        if self.live.is_none() {
            if let Some(r) = g.renderer.as_mut() {
                r.advance_time(dt / 1000.0);
            }
        }
        g.ui.set_pad_active(self.pad.used);
        play_interface(
            &mut g.ui,
            self.screens.as_mut(),
            self.live.as_mut(),
            &self.camera,
            dt,
        );
        self.requests.append(&mut g.ui.requests);
        Ok(())
    }

    /// Answers the clients waiting for a presented frame with one drawn offscreen (marked
    /// `"presented": false`), when the window has no frame to present.
    fn answer_frames_offscreen(&mut self) {
        let frame = self.screenshot_rgba();
        for (reply, scale) in std::mem::take(&mut self.frame_requests) {
            let answer = match &frame {
                Ok(f) => frame_reply(f, scale)
                    .map(|mut j| {
                        j["presented"] = json!(false);
                        j
                    })
                    .unwrap_or_else(control::failure),
                Err(e) => control::failure(e),
            };
            let _ = reply.send(answer);
        }
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

    /// A key for a movie's focused text field: an edit (backspace, delete, enter, the arrows,
    /// home, end, Ctrl/Cmd+A) or the text typed. Returns false when no text field has focus,
    /// so the key is the game's. Provisional: the game forwards keys and characters to its
    /// movies (Scaleform key and char events) by rules not traced here.
    fn text_key(&mut self, code: KeyCode, text: Option<&str>) -> bool {
        let Some(g) = self.gfx.as_mut() else {
            return false;
        };
        if !g.ui.typing() {
            return false;
        }
        use thedas_ui::UiKey;
        let command = self.keys.iter().any(|k| {
            matches!(
                k,
                KeyCode::ControlLeft
                    | KeyCode::ControlRight
                    | KeyCode::SuperLeft
                    | KeyCode::SuperRight
            )
        });
        let key = match code {
            KeyCode::Backspace => Some(UiKey::Backspace),
            KeyCode::Delete => Some(UiKey::Delete),
            KeyCode::Enter | KeyCode::NumpadEnter => Some(UiKey::Enter),
            KeyCode::Escape => Some(UiKey::Escape),
            KeyCode::Tab => Some(UiKey::Tab),
            KeyCode::ArrowLeft => Some(UiKey::Left),
            KeyCode::ArrowRight => Some(UiKey::Right),
            KeyCode::Home => Some(UiKey::Home),
            KeyCode::End => Some(UiKey::End),
            KeyCode::KeyA if command => Some(UiKey::SelectAll),
            _ => None,
        };
        match key {
            Some(k) => {
                g.ui.key(k);
            }
            None if !command => {
                for c in text.unwrap_or_default().chars().filter(|c| !c.is_control()) {
                    g.ui.key(UiKey::Char(c));
                }
            }
            None => {}
        }
        true
    }

    /// A key pressed (not a repeat). Returns whether the game should close.
    fn key_down(&mut self, code: KeyCode) -> bool {
        match code {
            // Escape closes the screen up, else opens the game menu over a game; the viewer
            // closes. The start menu stays (its Exit quits).
            KeyCode::Escape => match self.screens.as_mut() {
                None => return true,
                Some(s) if s.menu.is_some() => s.menu = None,
                Some(s) if !s.front_end => s.menu = Some(screens::Screen::GameMenu),
                Some(_) => {}
            },
            // While playing, P opens the spells/talents screen instead.
            KeyCode::KeyP if self.screens.is_none() => println!("camera {}", self.camera.spec()),
            // F5 the save screen, F9 the load screen. Provisional: the game's own bindings
            // (quick save and quick load) are not traced.
            KeyCode::F5 | KeyCode::F9 if self.screens.as_ref().is_some_and(|s| !s.front_end) => {
                if let (Some(g), Some(s)) = (self.gfx.as_mut(), self.screens.as_mut()) {
                    g.ui.open_saves(s, if code == KeyCode::F5 { 1 } else { 2 });
                }
            }
            KeyCode::F1 | KeyCode::F2 | KeyCode::F3 | KeyCode::F4 if self.live.is_some() => {
                let n = match code {
                    KeyCode::F1 => 0,
                    KeyCode::F2 => 1,
                    KeyCode::F3 => 2,
                    _ => 3,
                };
                if let Some(tag) = self.live.as_mut().and_then(|l| l.choose_leader(n)) {
                    log::info!("leading with {tag}");
                }
            }
            // The quickbar's first ten slots: 1 to 9, then 0.
            c if self.live.is_some() && quickslot_key(c).is_some() => {
                let slot = quickslot_key(c).unwrap_or(0);
                if let Some(ability) = self.live.as_mut().and_then(|l| use_quickslot(l, slot)) {
                    log::info!("aiming ability {ability} (click a target)");
                }
            }
            // The player's screens: I the inventory, C the character record, T tactics, M the
            // area map, J the journal (the executable's `ToggleJournal` action), K the skills
            // and P the spells/talents (`ToggleSkills`, `ToggleSpellsTalents`; I, C, J, K and P
            // are the game's default KeyBindings.ini), O the options. Provisional: the
            // others' bindings are not traced (the game opens its options from the game menu).
            KeyCode::KeyI
            | KeyCode::KeyC
            | KeyCode::KeyT
            | KeyCode::KeyM
            | KeyCode::KeyJ
            | KeyCode::KeyK
            | KeyCode::KeyP
            | KeyCode::Backslash
            | KeyCode::KeyO
                if self.screens.is_some() =>
            {
                if let (Some(s), Some(m)) = (self.screens.as_mut(), menu_key(code)) {
                    s.toggle(m);
                }
            }
            KeyCode::KeyF if self.live.is_some() => {
                self.follow = match self.follow.take() {
                    Some(_) => None,
                    None => Some(follow::Follow::behind(self.camera.yaw)),
                };
            }
            // Not during character creation or before a game: there is nothing to pause.
            KeyCode::Space
                if self.screens.as_ref().is_none_or(|s| {
                    !s.front_end && !s.shown().contains(&screens::Screen::CharGen)
                }) =>
            {
                if let Some(live) = self.live.as_mut() {
                    live.toggle_pause();
                    println!("{}", if live.paused { "paused" } else { "running" });
                }
            }
            KeyCode::F12 => {
                if let Err(e) = self.save_capture() {
                    eprintln!("capture failed: {e:#}");
                }
            }
            _ => {}
        }
        false
    }

    /// The mouse wheel turns `steps` (up positive): over the interface, or with a screen up,
    /// the list or text under the mouse scrolls; elsewhere the game camera zooms.
    fn wheel(&mut self, steps: f64) {
        let (x, y) = self.cursor;
        if let Some(g) = self.gfx.as_mut() {
            let on_ui = self
                .screens
                .as_ref()
                .is_some_and(|s| !s.exploring() || g.ui.covers(x, y));
            if on_ui {
                g.ui.mouse_wheel(x, y, steps);
                return;
            }
        }
        if let Some(f) = self.follow.as_mut()
            && f.zoom(steps as f32)
            && let Some(l) = self.live.as_ref()
        {
            l.sim().player_zoomed_in();
        }
    }

    /// The mouse moves to window pixel (`x`, `y`).
    fn cursor_to(&mut self, x: f64, y: f64) {
        self.cursor = (x, y);
        if let Some(g) = self.gfx.as_mut() {
            g.ui.mouse_move(x, y);
        }
    }

    /// The left button goes down or up at the cursor.
    fn left_button(&mut self, pressed: bool) {
        let (x, y) = self.cursor;
        // A click skips a cutscene. Provisional: the game's own skip key is not traced.
        if pressed
            && self
                .screens
                .as_ref()
                .is_some_and(screens::Screens::in_cutscene)
        {
            if let Some(live) = self.live.as_ref() {
                live.sim().skip_cutscene();
            }
            return;
        }
        if !pressed && std::mem::take(&mut self.swallow_release) {
            return;
        }
        // A second press close in time and place is a double click (the movies use them:
        // equipping from the backpack).
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

    /// The right button goes down or up at the cursor.
    fn right_button(&mut self, pressed: bool) {
        let (x, y) = self.cursor;
        // Over the interface (or with a menu up) a right click is the interface's (an item's
        // actions); elsewhere it turns the camera.
        if pressed {
            if let (Some(g), Some(s)) = (self.gfx.as_mut(), self.screens.as_ref()) {
                if !s.exploring() || g.ui.covers(x, y) {
                    g.ui.button_as(thedas_ui::avm1::MOUSE_RIGHT, x, y, true);
                    self.ui_right = true;
                    return;
                }
            }
        } else if std::mem::take(&mut self.ui_right) {
            if let Some(g) = self.gfx.as_mut() {
                g.ui.button_as(thedas_ui::avm1::MOUSE_RIGHT, x, y, false);
            }
            return;
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

    /// Plays one step a client sent (not `run`, which the frames play), as the player's own
    /// input would.
    fn play_step(&mut self, step: &Step, log: &mut Vec<String>) {
        match step {
            &Step::Click(x, y) => {
                self.cursor_to(x, y);
                self.left_button(true);
                self.left_button(false);
                log.push(format!("click {x},{y}"));
            }
            &Step::ClickAs(button, x, y) => {
                self.cursor_to(x, y);
                if let Some(g) = self.gfx.as_mut() {
                    g.ui.click_as(button, x, y);
                }
                log.push(format!("click {x},{y} as button {button}"));
            }
            &Step::RightAt(x, y, down) => {
                self.cursor_to(x, y);
                if let Some(g) = self.gfx.as_mut() {
                    g.ui.button_as(thedas_ui::avm1::MOUSE_RIGHT, x, y, down);
                }
                log.push(format!(
                    "right {} {x},{y}",
                    if down { "press" } else { "release" }
                ));
            }
            &Step::Hover(x, y) => {
                self.cursor_to(x, y);
                log.push(format!("hover {x},{y}"));
            }
            Step::Picker => {
                if let Some(l) = self.live.as_ref() {
                    l.sim().party_picker();
                }
                log.push("party picker".into());
            }
            &Step::Saves(mode) => {
                self.key_down(if mode == 1 { KeyCode::F5 } else { KeyCode::F9 });
                log.push(format!("saves {mode}"));
            }
            Step::Type(t) => {
                let typed = self.gfx.as_mut().is_some_and(|g| g.ui.type_text(t));
                log.push(format!(
                    "type {t:?}: {}",
                    if typed { "typed" } else { "no text field" }
                ));
            }
            &Step::TextKey(k) => {
                let typed = self.gfx.as_mut().is_some_and(|g| g.ui.key(k));
                log.push(format!(
                    "key {k:?}: {}",
                    if typed { "typed" } else { "no text field" }
                ));
            }
            Step::Wipe => {
                if let Some(l) = self.live.as_ref() {
                    wipe(&mut l.sim());
                }
                log.push("the party falls".into());
            }
            Step::Call(movie, path, args) => {
                let r = self
                    .gfx
                    .as_mut()
                    .and_then(|g| g.ui.probe_call(movie, path, args));
                log.push(format!("call {movie} {path}{args:?}: {r:?}"));
            }
            Step::SetValue(k, v) => {
                if let Some(g) = self.gfx.as_mut() {
                    g.ui.probe_set_value(k, v);
                }
                log.push(format!("setvalue {k} = {v}"));
            }
            Step::ClientOption(k, v) => {
                if let Some(g) = self.gfx.as_ref() {
                    set_client_option(Some(&g.ui), self.live.as_ref(), k, v);
                }
                log.push(format!("option {k} = {v}"));
            }
            &Step::ZoomKey(dir) => {
                self.zoom_key = dir;
            }
            Step::Travel(tag) => {
                if let Some(l) = self.live.as_ref() {
                    let ok = travel_to_pin(&mut l.sim(), tag);
                    log.push(format!(
                        "travel {tag}: {}",
                        if ok { "set out" } else { "no such active pin" }
                    ));
                }
            }
            Step::Transition(area, waypoint) => {
                if let Some(l) = self.live.as_ref() {
                    // As a script's DoAreaTransition does: performed at the start of the next
                    // update (`TRANSITION_DELAY_MS`).
                    let mut sim = l.sim();
                    sim.world.transition = Some((area.clone(), waypoint.clone()));
                    sim.world.transition_at =
                        Some(sim.world.time_ms + thedas_sim::sim::TRANSITION_DELAY_MS);
                }
                log.push(format!("transition {area} {waypoint}"));
            }
            Step::Floaty(text) => {
                if let Some(l) = self.live.as_ref() {
                    test_floaty(&mut l.sim(), text);
                }
                log.push(format!("floaty {text:?}"));
            }
            Step::Use(tag) => {
                if let Some(l) = self.live.as_ref() {
                    let mut sim = l.sim();
                    let who = sim.world.controlled();
                    match (who, who.and_then(|w| nearest_tagged(&sim, w, tag))) {
                        (Some(w), Some(t)) => {
                            sim.order_use(w, t);
                            log.push(format!("use {tag}"));
                        }
                        _ => log.push(format!("use {tag}: nothing")),
                    }
                }
            }
            Step::Loading(kind) => {
                let hint = self
                    .live
                    .as_ref()
                    .and_then(|l| thedas_ui::binding::load_hint(&l.sim()));
                if let Some(g) = self.gfx.as_mut() {
                    match kind {
                        Some(k) => g.ui.loading_begin(*k, hint),
                        None => g.ui.loading_end(),
                    }
                }
                log.push(format!("loading {kind:?}"));
            }
            &Step::TrainingMode(mode) => {
                if let Some(l) = self.live.as_ref() {
                    l.sim().set_training_mode(mode);
                }
                log.push(format!("training mode {mode}"));
            }
            Step::Training(name) => {
                let heard = self
                    .live
                    .as_ref()
                    .is_some_and(|l| l.sim().training_event(name, 0));
                log.push(format!(
                    "training {name}: {}",
                    if heard { "heard" } else { "not awaited" }
                ));
            }
            Step::Plot(plot, flag) => {
                if let Some(l) = self.live.as_ref() {
                    l.sim().set_plot_flag(
                        plot,
                        *flag,
                        true,
                        true,
                        thedas_script::vm::OBJECT_INVALID,
                    );
                }
                log.push(format!("plot {plot} {flag}"));
            }
            Step::Cutscene(name) => {
                if let Some(l) = self.live.as_ref() {
                    l.sim().world.cutscenes.push(name.clone());
                }
                log.push(format!("cutscene {name}"));
            }
            &Step::Menu(m) => {
                self.key_down(match m {
                    screens::Screen::CharacterRecord => KeyCode::KeyC,
                    screens::Screen::Tactics => KeyCode::KeyT,
                    screens::Screen::AreaMap => KeyCode::KeyM,
                    screens::Screen::Journal => KeyCode::KeyJ,
                    screens::Screen::Skills => KeyCode::KeyK,
                    screens::Screen::SpellsTalents => KeyCode::KeyP,
                    screens::Screen::Options => KeyCode::KeyO,
                    screens::Screen::GameMenu => KeyCode::Escape,
                    _ => KeyCode::KeyI,
                });
                log.push(format!("{m:?}"));
            }
            &Step::Leader(n) => {
                let tag = self.live.as_mut().and_then(|l| l.choose_leader(n));
                log.push(match tag {
                    Some(tag) => format!("leading with {tag}"),
                    None => format!("no party member {}", n + 1),
                });
            }
            &Step::Quickslot(slot) => {
                let aim = self.live.as_mut().and_then(|l| use_quickslot(l, slot));
                log.push(match aim {
                    Some(ability) => format!("aiming ability {ability} (click a target)"),
                    None => format!("quickslot {}", slot + 1),
                });
            }
            Step::Pause => {
                // As the Space key does (a training tutorial takes it as its unpause).
                if let Some(live) = self.live.as_mut() {
                    live.toggle_pause();
                    log.push(if live.paused { "paused" } else { "running" }.into());
                }
            }
            &Step::Reply(n) => {
                if let Some(live) = self.live.as_mut() {
                    log.push(reply(live, n));
                }
            }
            Step::Talk(tag) => {
                let Some(live) = self.live.as_mut() else {
                    return;
                };
                let mut sim = live.sim();
                let who = sim.world.controlled();
                let target = who.and_then(|w| nearest_tagged(&sim, w, tag));
                log.push(match (who, target) {
                    (Some(w), Some(t)) => {
                        sim.order_talk(w, t);
                        format!("talk to {tag}")
                    }
                    _ => format!("talk to {tag}: nobody"),
                });
            }
            &Step::Press(x, y) | &Step::Release(x, y) => {
                let down = matches!(step, Step::Press(..));
                self.cursor_to(x, y);
                self.left_button(down);
                log.push(format!(
                    "{} {x},{y}",
                    if down { "press" } else { "release" }
                ));
            }
            &Step::Orbit(dyaw, dpitch) => {
                orbit_camera(self.follow.as_mut(), &mut self.camera, dyaw, dpitch);
                log.push(format!("orbit {dyaw},{dpitch}"));
            }
            &Step::Zoom(n) => {
                // As the wheel turns (`WindowEvent::MouseWheel` after its delta's conversion):
                // over the interface it scrolls, elsewhere the camera zooms.
                self.wheel(n as f64);
                log.push(format!("zoom {n}"));
            }
            &Step::PadButton(b, down) => {
                self.remote_pad.set(b, down);
                self.pad_frame(0.0);
                log.push(format!("pad {b:?} {}", if down { "down" } else { "up" }));
            }
            &Step::PadStick(stick, x, y) => {
                self.remote_pad.set_stick(stick, glam::Vec2::new(x, y));
                self.pad_frame(0.0);
                log.push(format!("pad stick {stick:?} {x},{y}"));
            }
            Step::Rebuild => log.push("rebuild: headless only".into()),
            Step::Run(_) => {}
        }
    }

    /// Serves the control channel's requests, and plays the steps clients sent: one client's
    /// at a time, a `run` lasting that long in frames (`dt_ms` is this frame's).
    fn serve_control(&mut self, dt_ms: f64) {
        let requests = self.control.as_ref().map(|c| c.poll()).unwrap_or_default();
        for r in requests {
            let cmd = r.body["cmd"].as_str().unwrap_or_default().to_string();
            let answer = match cmd.as_str() {
                "status" => Some(self.status()),
                // The frame as presented, read back as the next one is drawn; a surface that
                // cannot be read back is drawn again offscreen instead.
                "screenshot" => match self
                    .gfx
                    .as_ref()
                    .map(|g| g.config.usage.contains(wgpu::TextureUsages::COPY_SRC))
                {
                    Some(true) => {
                        let scale = r.body["scale"].as_u64().unwrap_or(1).clamp(1, 16) as u32;
                        self.frame_requests.push((r.reply.clone(), scale));
                        None
                    }
                    _ => Some(self.screenshot().unwrap_or_else(control::failure)),
                },
                "state" => Some(match self.live.as_ref().map(|l| l.sim().save_json()) {
                    Some(Ok(bytes)) => json!({
                        "ok": true,
                        "save": String::from_utf8_lossy(&bytes),
                    }),
                    Some(Err(e)) => control::failure(e),
                    None => control::failure("not playing"),
                }),
                "input" => match parse_steps(r.body["steps"].as_str().unwrap_or_default()) {
                    Ok(steps) => {
                        self.remote.push_back(Remote {
                            steps: steps.into(),
                            wait_ms: 0.0,
                            log: Vec::new(),
                            reply: r.reply.clone(),
                        });
                        None
                    }
                    Err(e) => Some(control::failure(e)),
                },
                other => Some(control::failure(format!("unknown cmd {other:?}"))),
            };
            if let Some(a) = answer {
                let _ = r.reply.send(a);
            }
        }
        while let Some(mut r) = self.remote.pop_front() {
            if r.wait_ms > 0.0 {
                r.wait_ms -= dt_ms;
                if r.wait_ms > 0.0 {
                    self.remote.push_front(r);
                    return;
                }
            }
            match r.steps.pop_front() {
                Some(Step::Run(ms)) => {
                    r.log.push(format!("run {ms}"));
                    r.wait_ms = ms as f64;
                    self.remote.push_front(r);
                }
                Some(step) => {
                    self.play_step(&step, &mut r.log);
                    self.remote.push_front(r);
                }
                None => {
                    let mut out = self.status();
                    out["log"] = json!(r.log);
                    let _ = r.reply.send(out);
                }
            }
        }
    }

    /// Where the game is, for clients.
    fn status(&self) -> J {
        let mut out = json!({
            "ok": true,
            "camera": self.camera.spec(),
            "size": self.gfx.as_ref().map(|g| [g.config.width, g.config.height]),
            "playing": self.live.is_some(),
        });
        let Some(live) = self.live.as_ref() else {
            return out;
        };
        let sim = live.sim();
        let tag = |id: Option<thedas_script::vm::ObjectId>| {
            id.and_then(|i| sim.world.get(i)).map(|o| o.tag.clone())
        };
        let leader = sim.world.controlled().and_then(|l| sim.world.get(l));
        out["time_ms"] = json!(sim.world.time_ms);
        out["paused"] = json!(live.paused);
        out["game_over"] = json!(sim.world.game_over);
        out["area"] = json!(tag(sim.world.area));
        out["leader"] = json!(leader.map(|o| json!({
            "id": o.id,
            "tag": o.tag,
            "position": o.position,
            "commands": o.commands.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
        })));
        out["party"] = json!(
            thedas_sim::engine::party_of(&sim)
                .into_iter()
                .map(|p| tag(Some(p)))
                .collect::<Vec<_>>()
        );
        out["dialog"] = json!(sim.dialog_state().map(|d| json!({
            "speaker": d.speaker,
            "line": d.line,
            "replies": d.replies.iter().map(|(_, t)| t.as_str()).collect::<Vec<_>>(),
            "ended": d.ended,
        })));
        out["aiming"] = json!(live.aiming.as_ref().map(|a| a.ability));
        out["cutscene"] = json!(sim.world.cutscene.as_ref().map(|c| json!({
            "name": c.name,
            "seconds": c.seconds,
            "lines": c.lines.len(),
        })));
        out["screens"] = json!(self.screens.as_ref().map(|s| format!("{:?}", s.shown())));
        out
    }

    /// The frame as the player sees it (world, reticle, interface), as a base64 PNG.
    fn screenshot(&mut self) -> Result<J> {
        let frame = self.screenshot_rgba()?;
        let (w, h) = (frame.width, frame.height);
        let png = thedas_ui::stage::encode_png(&frame).context("encoding the screenshot")?;
        use base64::Engine as _;
        Ok(json!({
            "ok": true,
            "width": w,
            "height": h,
            "png": base64::engine::general_purpose::STANDARD.encode(png),
        }))
    }

    /// The frame as the player would see it, drawn offscreen: the world, the aiming reticle
    /// and the interface.
    fn screenshot_rgba(&mut self) -> Result<thedas_ui::Rgba> {
        let g = self.gfx.as_mut().context("the window is not up yet")?;
        let (w, h) = (g.config.width, g.config.height);
        let mut reticle = Vec::new();
        let pixels = match (self.live.as_mut(), g.renderer.as_mut()) {
            (Some(live), _) => {
                reticle = live.reticle(&self.camera, (w, h), self.cursor);
                let ctx = self
                    .pad
                    .context_for(self.screens.as_ref(), g.ui.stage.dialog_up());
                reticle.extend(self.pad.overlay(Some(&*live), &self.camera, (w, h), ctx));
                live.capture(&g.ctx, &self.camera, (w, h))?
            }
            (None, Some(r)) => Some(r.capture(&g.ctx, &self.camera, w, h)?),
            (None, None) => None,
        };
        let mut frame = thedas_ui::Rgba {
            width: w,
            height: h,
            pixels: pixels.unwrap_or_else(|| [0, 0, 0, 255].repeat(w as usize * h as usize)),
        };
        reticle::draw_rgba(&mut frame.pixels, (w, h), &reticle);
        g.ui.capture_over(&mut frame)?;
        let ctx = self
            .pad
            .context_for(self.screens.as_ref(), g.ui.stage.dialog_up());
        reticle::draw_rgba(
            &mut frame.pixels,
            (w, h),
            &self.pad.pointer(self.cursor, ctx),
        );
        Ok(frame)
    }

    /// One frame of the game controller: the device's state (and clients' `pad` steps)
    /// through the mapping, carried out as the mouse and keyboard would.
    fn pad_frame(&mut self, dt_ms: f64) {
        let device = self.pads.as_mut().map(|p| p.poll()).unwrap_or_default();
        let state = device.merged(&self.remote_pad);
        let mut pad = std::mem::take(&mut self.pad);
        for m in pad.frame(&state, self, dt_ms) {
            log::info!("pad: {m}");
        }
        self.pad = pad;
    }

    /// Acts on what the start menu and the game menu asked (see [`ui::FrontEnd`]): a new game
    /// or a loaded one replaces the running one (or the start menu's), leaving a game goes
    /// back to the start menu, Exit quits.
    fn front_end(&mut self, requests: Vec<ui::FrontEnd>) {
        for r in requests {
            log::info!("front end: {r:?}");
            let start = match &r {
                ui::FrontEnd::Quit => {
                    self.quit = true;
                    continue;
                }
                ui::FrontEnd::NewGame => live::Start::NewGame,
                ui::FrontEnd::Load(b) => live::Start::SaveBytes(b),
                ui::FrontEnd::StartMenu => live::Start::Menu,
            };
            // A game (new or loaded) opens on a worker while the window keeps presenting,
            // the running one held behind it (see `Live::hold`); the start menu opens at once.
            if matches!(r, ui::FrontEnd::StartMenu) {
                match live::Live::open(&self.install, &self.module, start, &self.shaders) {
                    Ok(l) => self.install_live(l),
                    Err(e) => log::warn!("front end: {r:?} failed: {e:#}"),
                }
                continue;
            }
            if self.opening.is_some() {
                log::info!("front end: {r:?} ignored, a game is already opening");
                continue;
            }
            let (install, module, shaders) = (
                self.install.clone(),
                self.module.clone(),
                self.shaders.clone(),
            );
            let save = match &r {
                ui::FrontEnd::Load(b) => Some(b.clone()),
                _ => None,
            };
            let (tx, rx) = std::sync::mpsc::channel();
            let spawned = std::thread::Builder::new()
                .name("game-open".into())
                .spawn(move || {
                    let start = match &save {
                        Some(b) => live::Start::SaveBytes(b),
                        None => live::Start::NewGame,
                    };
                    let _ = tx.send(live::Live::open(&install, &module, start, &shaders));
                });
            match spawned {
                Ok(_) => {
                    if let Some(l) = self.live.as_mut() {
                        l.hold(true);
                    }
                    self.opening = Some((format!("{r:?}"), rx, Instant::now()));
                }
                Err(e) => log::warn!("front end: {r:?} failed: {e}"),
            }
        }
        self.poll_opening();
    }

    /// The game opening on its worker (see [`App::front_end`]), once it is ready: it
    /// replaces the running one. One that failed to open leaves the running one to carry on.
    fn poll_opening(&mut self) {
        let Some((what, rx, started)) = self.opening.as_ref() else {
            return;
        };
        let opened = match rx.try_recv() {
            Ok(r) => r,
            Err(std::sync::mpsc::TryRecvError::Empty) => return,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                Err(anyhow::anyhow!("the game's loading stopped"))
            }
        };
        let (what, elapsed) = (what.clone(), started.elapsed());
        self.opening = None;
        match opened {
            Ok(l) => {
                log::info!(
                    "front end: {what}: the game opened in {:.2}s",
                    elapsed.as_secs_f64()
                );
                self.install_live(l);
            }
            Err(e) => {
                log::warn!("front end: {what} failed: {e:#}");
                if let Some(l) = self.live.as_mut() {
                    l.hold(false);
                }
            }
        }
    }

    /// A newly opened game (or the start menu) becomes the running one.
    fn install_live(&mut self, mut l: live::Live) {
        if let Some(g) = self.gfx.as_mut() {
            l.frame_effects = self.loaded.frame_effects;
            g.ui.close_all();
            g.ui.attach(&l.sim);
            seed_client_options(&l, &g.ui);
            // Closing everything took the loading screen down too: it comes up again for
            // the new game's own build (see `loading_screen`).
            self.load_screen_up = false;
        }
        // One explore camera for the whole process: a load turns it behind the hero again
        // but keeps its zoom (see `Follow::reset`).
        self.follow = l.hero().map(|(_, facing)| {
            let heading = thedas_sim::world::heading_of(facing).to_radians();
            match self.follow.take() {
                Some(mut f) => {
                    f.reset(heading);
                    f
                }
                None => follow::Follow::behind(heading),
            }
        });
        if let Some((pos, facing)) = l.hero() {
            let fov = self.camera.fov_y;
            self.camera = hero_camera(pos, facing);
            self.camera.fov_y = fov;
        }
        self.screens = Some(screens::Screens::new(l.front_end));
        l.async_loads = true;
        self.live = Some(l);
    }

    fn fail(&mut self, event_loop: &ActiveEventLoop, e: anyhow::Error) {
        self.error = Some(e);
        event_loop.exit();
    }
}

impl padplay::Host for App {
    fn live(&mut self) -> Option<&mut live::Live> {
        self.live.as_mut()
    }

    fn camera(&self) -> Camera {
        self.camera
    }

    fn follow(&mut self) -> Option<&mut follow::Follow> {
        self.follow.as_mut()
    }

    fn look(&mut self, yaw: f32, pitch: f32) {
        self.camera.look(yaw, pitch);
    }

    fn size(&self) -> (u32, u32) {
        self.gfx
            .as_ref()
            .map_or(self.size, |g| (g.config.width, g.config.height))
    }

    fn cursor(&self) -> (f64, f64) {
        self.cursor
    }

    fn pointer_to(&mut self, x: f64, y: f64) {
        self.cursor_to(x, y);
    }

    fn left_button(&mut self, down: bool) {
        App::left_button(self, down);
    }

    fn right_button(&mut self, down: bool) {
        App::right_button(self, down);
    }

    fn double_click(&mut self) {
        let (x, y) = self.cursor;
        if let Some(g) = self.gfx.as_mut() {
            g.ui.click_as(
                thedas_ui::avm1::MOUSE_LEFT + thedas_ui::avm1::DOUBLE_CLICK,
                x,
                y,
            );
        }
    }

    fn screens(&mut self) -> Option<&mut screens::Screens> {
        self.screens.as_mut()
    }

    fn popup(&mut self) -> bool {
        self.gfx.as_ref().is_some_and(|g| g.ui.stage.dialog_up())
    }

    fn focus_targets(&mut self) -> Vec<thedas_ui::FocusTarget> {
        self.gfx
            .as_mut()
            .map(|g| g.ui.stage.focus_targets())
            .unwrap_or_default()
    }

    fn text_focused(&mut self) -> bool {
        self.gfx.as_mut().is_some_and(|g| g.ui.stage.text_focused())
    }

    fn text_key(&mut self, key: thedas_ui::UiKey) -> bool {
        self.gfx.as_mut().is_some_and(|g| g.ui.stage.key(key))
    }

    fn movie_value(&mut self, movie: &str, path: &str) -> Option<thedas_ui::UiValue> {
        self.gfx.as_mut()?.ui.probe_call(movie, path, &[])
    }

    fn open_saves(&mut self, mode: i32) {
        if let (Some(g), Some(s)) = (self.gfx.as_mut(), self.screens.as_mut()) {
            g.ui.open_saves(s, mode);
        }
    }

    fn line(&self) -> Option<(bool, bool)> {
        self.gfx.as_ref().and_then(|g| g.ui.line_state())
    }

    fn interface_clicked(&mut self) {
        if let Some(g) = self.gfx.as_mut() {
            g.ui.clicked();
        }
    }

    fn scroll(&mut self, lines: f64) {
        App::wheel(self, lines);
    }

    fn highlight_reply(&mut self, n: usize) -> bool {
        self.gfx.as_mut().is_some_and(|g| g.ui.highlight_reply(n))
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
            WindowEvent::KeyboardInput { event, .. } if self.intro.is_some() => {
                if event.state == ElementState::Pressed && !event.repeat {
                    if let Some(i) = self.intro.as_mut() {
                        i.skip();
                    }
                }
            }
            WindowEvent::MouseInput { state, .. } if self.intro.is_some() => {
                if state == ElementState::Pressed {
                    if let Some(i) = self.intro.as_mut() {
                        i.skip();
                    }
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let PhysicalKey::Code(code) = event.physical_key else {
                    return;
                };
                // A key skips a cutscene (and the movie it plays), as a click does.
                // Provisional: the game's own skip key is not traced.
                if event.state == ElementState::Pressed
                    && !event.repeat
                    && self
                        .screens
                        .as_ref()
                        .is_some_and(screens::Screens::in_cutscene)
                {
                    if let Some(live) = self.live.as_ref() {
                        live.sim().skip_cutscene();
                    }
                    return;
                }
                // While a movie's text field has focus, keys type into it (repeats too).
                if event.state == ElementState::Pressed
                    && self.text_key(code, event.text.as_deref())
                {
                    return;
                }
                match event.state {
                    ElementState::Pressed => {
                        if !event.repeat && self.key_down(code) {
                            event_loop.exit();
                        }
                        self.keys.insert(code);
                    }
                    ElementState::Released => {
                        self.keys.remove(&code);
                    }
                }
            }
            WindowEvent::CursorMoved { position, .. } => self.cursor_to(position.x, position.y),
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => self.left_button(state == ElementState::Pressed),
            WindowEvent::MouseInput {
                state,
                button: MouseButton::Right,
                ..
            } => self.right_button(state == ElementState::Pressed),
            WindowEvent::MouseWheel { delta, .. } => {
                let steps = match delta {
                    winit::event::MouseScrollDelta::LineDelta(_, y) => y,
                    winit::event::MouseScrollDelta::PixelDelta(p) => p.y as f32 / 40.0,
                };
                self.wheel(steps as f64);
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
                // The player's control options: mouse sensitivity scales the turn (1, the
                // default, is the old fixed 0.003 radians a pixel) and the axes may be
                // inverted. Provisional: the exe's sensitivity curve is not traced.
                let opt = |name: &str, default: f64| {
                    self.gfx
                        .as_ref()
                        .and_then(|g| g.ui.client_option(name))
                        .unwrap_or(default) as f32
                };
                let s = 0.003 * opt("ControlOptions.MouseSensitivity", 1.0).clamp(0.05, 4.0);
                let sign = |name: &str| if opt(name, 0.0) != 0.0 { -1.0 } else { 1.0 };
                let (dx, dy) = (
                    -delta.0 as f32 * s * sign("ControlOptions.InvertXAxis"),
                    -delta.1 as f32 * s * sign("ControlOptions.InvertYAxis"),
                );
                match self.follow.as_mut() {
                    Some(f) => f.orbit(dx, dy),
                    None => self.camera.look(dx, dy),
                }
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let requests = std::mem::take(&mut self.requests);
        self.front_end(requests);
        if self.quit {
            event_loop.exit();
            return;
        }
        if let Some(g) = &self.gfx {
            g.window.request_redraw();
        }
    }
}

/// What the sound follows this frame (see [`audio`]): the game playing (`live`, none during
/// the boot movies), its interface (`ui`: the sounds it asked for, the player's volumes),
/// the camera, and a boot movie playing. A cutscene's or area's sound data is read here
/// when it starts.
fn audio_state(
    a: &mut audio::GameAudio,
    live: Option<&mut live::Live>,
    ui: Option<&mut ui::UiLayer>,
    camera: &Camera,
    movie: Option<(PathBuf, f64)>,
) -> audio::AudioState {
    let mut state = audio::AudioState {
        movie,
        ..Default::default()
    };
    if let Some(ui) = ui {
        state.gui_sounds = ui.stage.bridge.drain_sounds();
        // Character creation's voice previews: always said, by nobody in particular.
        for (set, entry) in ui.drain_sound_set_previews() {
            state.sound_sets.push((set, entry, 1.0, None));
        }
        state.volumes = audio::volumes(|n| ui.client_option(n));
    }
    let Some(live) = live else {
        return state;
    };
    // The loading frame is up: the world's sounds duck as in the exe's loading mode.
    state.loading = live.loading();
    state.paused = live.paused;
    {
        let sim = live.sim();
        state.container = sim
            .world
            .container
            .and_then(|c| Some((c, sim.world.get(c)?.appearance as i64)));
    }
    // Animations' sound events (see `Live::animation_clocks`), with what each creature
    // wears and wields.
    if !live.front_end {
        state.animations = live.animation_clocks();
        let sim = live.sim();
        let grid = sim.world.area.and_then(|a| sim.loaded_nav_grid(a));
        let appearances = sim.world.tables().get(0, "apr_base");
        let items = sim.world.tables().get(0, "bitm_base");
        // A creature's sound material as DAOrigins.exe keeps it (+0x930, 0x6a0a00..0x6a0cb0):
        // its appearance's (`apr_base.gda` column 0x8ea8dc24), replaced by its chest
        // armour's unless the appearance says not to (column 0x429ca38f); anything but
        // armour (10-13), hides (16-18) and golems' and bone (23-25) counts as flesh (16).
        // Provisional: the armour's material is its base item's (`BITM_base.gda` column
        // 0x7e0505a1); the exe goes through the item's material type (worksheet 89).
        let sound_material = |id: u32| -> i64 {
            let int = |c: Option<&thedas_formats::gda::Cell>| match c {
                Some(thedas_formats::gda::Cell::Int(v)) => Some(*v),
                _ => None,
            };
            let Some(o) = sim.world.get(id) else {
                return 16;
            };
            let apr = |h: u32| {
                appearances
                    .as_ref()
                    .and_then(|t| int(t.cell_by_hash(h, o.appearance as i64)))
            };
            let mut m = apr(0x8ea8_dc24).unwrap_or(16);
            if apr(0x429c_a38f).unwrap_or(0) == 0 {
                let chest = thedas_sim::sim::constant(&sim.table, "INVENTORY_SLOT_CHEST");
                let armour = o
                    .equipment
                    .get(&chest)
                    .and_then(|i| sim.world.get(*i))
                    .and_then(|i| i.fields.get("BaseItem").copied())
                    .and_then(|b| {
                        items
                            .as_ref()
                            .and_then(|t| int(t.cell_by_hash(0x7e05_05a1, b as i64)))
                    });
                if let Some(a) = armour.filter(|a| *a > 0) {
                    m = a;
                }
            }
            if matches!(m, 10..=13 | 16..=18 | 23..=25) {
                m
            } else {
                16
            }
        };
        for (id, ..) in &state.animations {
            let Some(o) = sim.world.get(*id) else {
                continue;
            };
            let base = |slot: &str| {
                let slot = thedas_sim::sim::constant(&sim.table, slot);
                o.equipment
                    .get(&slot)
                    .and_then(|i| sim.world.get(*i))
                    .and_then(|i| i.fields.get("BaseItem").copied())
            };
            state.gear.insert(
                *id,
                audio::Gear {
                    armour: base("INVENTORY_SLOT_CHEST"),
                    main: base("INVENTORY_SLOT_MAIN"),
                    off: base("INVENTORY_SLOT_OFFHAND"),
                    footstep: Some(audio::footstep_column(appearances.as_ref().and_then(|t| {
                        t.cell_by_hash(audio::APR_FOOTSTEP, o.appearance as i64)
                    }))),
                    surface: grid.as_ref().map_or(0, |g| g.surface(o.position)),
                    hands: [0, 1].map(|hand| {
                        sim.hand_attack(*id, hand).and_then(|(result, target)| {
                            let at = sim.world.get(target)?.position;
                            Some((result, sound_material(target), at))
                        })
                    }),
                    attack_target: sim.attack_target(*id).and_then(|t| {
                        let at = sim.world.get(t)?.position;
                        Some((sound_material(t), at))
                    }),
                    unarmed: appearances
                        .as_ref()
                        .and_then(|t| t.cell_by_hash(audio::APR_UNARMED, o.appearance as i64))
                        .and_then(|c| match c {
                            thedas_formats::gda::Cell::Int(v) => Some(*v),
                            _ => None,
                        }),
                    size: appearances
                        .as_ref()
                        .and_then(|t| t.cell_by_hash(audio::APR_SIZE, o.appearance as i64))
                        .and_then(|c| match c {
                            thedas_formats::gda::Cell::String(s) => {
                                let s = s.trim_matches(|c| c == '\0' || c == ' ' || c == '\t');
                                Some(thedas_formats::gda::column_hash(s))
                            }
                            _ => None,
                        }),
                    placeable: (o.kind == thedas_sim::world::ObjectKind::Placeable)
                        .then_some(o.appearance as i64),
                },
            );
        }
    }
    {
        let sim = live.sim();
        state.front_end = live.front_end;
        state.chargen = sim.world.chargen.is_some();
        state.hero_female = sim
            .world
            .hero
            .and_then(|h| sim.world.get(h))
            .is_some_and(|o| o.fields.get("Gender").copied() == Some(2));
        state.line = sim.conversation_line().and_then(|l| {
            let female = sim
                .world
                .get(l.player)
                .is_some_and(|o| o.fields.get("Gender").copied() == Some(2));
            Some(audio::VoiceLine {
                dialog: l.dialog.to_string(),
                strref: l.strref?,
                female,
                ambient: l.ambient,
            })
        });
        // A cutscene's sound tracks, and its line's voice when the line has its own.
        if let Some(c) = sim.world.cutscene.as_ref() {
            if a.wants_cutscene(&c.name) {
                let tracks = sim
                    .vfs
                    .read(&c.name)
                    .map(|(data, _)| thedas_sim::cutscene::sounds(&data))
                    .unwrap_or_default();
                a.set_cutscene(&c.name, tracks);
            }
            state.cutscene = Some((c.name.clone(), c.elapsed_ms as f32 / 1000.0));
            if state.line.is_none()
                && let Some(l) = c.line_now()
                && l.vo_mode == 0
                && let Some(conv) = l.conversation.as_ref()
            {
                let female = sim
                    .world
                    .hero
                    .and_then(|h| sim.world.get(h))
                    .is_some_and(|o| o.fields.get("Gender").copied() == Some(2));
                state.line = Some(audio::VoiceLine {
                    dialog: conv.clone(),
                    strref: l.strref,
                    female,
                    ambient: false,
                });
            }
        }
    }
    // Sounds the simulation asked for (scripts, character creation's voice preview).
    {
        let mut sim = live.sim();
        for req in std::mem::take(&mut sim.world.sounds) {
            match req {
                thedas_sim::world::SoundRequest::Event { target, event } => {
                    let at = sim.world.get(target).map(|o| o.position);
                    state.script_sounds.push((event, at));
                }
                thedas_sim::world::SoundRequest::SoundSet {
                    target,
                    entry,
                    probability,
                } => {
                    let o = sim.world.get(target);
                    let set = o.and_then(|o| {
                        o.text_fields
                            .get(thedas_sim::sim::SOUND_SET)
                            .cloned()
                            .or_else(|| a.template_sound_set(&sim.vfs, &o.template))
                    });
                    if let Some(set) = set {
                        let at = o.map(|o| o.position);
                        state.sound_sets.push((set, entry, probability, at));
                    }
                }
                // PlayMusic and StopMusic do nothing in DAOrigins.exe: their shared handler
                // (0x65cfd0, commands 484 and 485) reads the name and drops it.
                other => log::debug!("audio: not done yet: {other:?}"),
            }
        }
    }
    // The party picker, while open: who is picked (and the pool, for unpicked lines).
    {
        let sim = live.sim();
        if let Some(p) = sim.world.party_picker.as_ref() {
            let mut describe = |id: u32| {
                let o = sim.world.get(id)?;
                let set = o
                    .text_fields
                    .get(thedas_sim::sim::SOUND_SET)
                    .cloned()
                    .or_else(|| a.template_sound_set(&sim.vfs, &o.template));
                Some((id, o.template.clone(), set, o.position))
            };
            state.picker = Some(p.selected.iter().filter_map(|&id| describe(id)).collect());
            for id in sim.party_pool() {
                if let Some(m) = describe(id) {
                    state.picker_left.insert(id, m);
                }
            }
        }
    }
    if live.front_end {
        return state;
    }
    // The area: its music and placed sounds.
    {
        let sim = live.sim();
        let area = sim
            .world
            .area
            .and_then(|a| sim.world.get(a))
            .map(|a| (a.id, a.template.clone()));
        if let Some((area_id, resref)) = area.filter(|a| !a.1.is_empty()) {
            if a.wants_area(&resref) {
                let data = sim
                    .vfs
                    .read(&format!("{resref}.are"))
                    .map(|(d, _)| d)
                    .unwrap_or_default();
                a.set_area(&resref, &data);
            }
            state.area = Some(resref);
            state.combat = sim.world.party_fighting;
            state.music_intensity = Some(sim.world.music_intensity);
            state.dead = sim.world.game_over;
            state.game_mode = sim.world.game_mode;
            state.inactive_sounds = sim
                .world
                .objects
                .values()
                .filter(|o| o.kind == thedas_sim::world::ObjectKind::Sound)
                .filter(|o| o.area == Some(area_id) && !o.active)
                .map(|o| o.tag.to_ascii_lowercase())
                .collect();
        }
    }
    // Between the party leader and the camera (`sound_settings.gda` "Camera / player
    // listener balance position": 0.17 of the way to the camera).
    let cam = camera.position;
    let feet = live.leader_feet().unwrap_or(cam);
    state.leader = live.leader_feet().map(|f| f.to_array());
    let p = feet + (cam - feet) * 0.17;
    let right = [camera.yaw.sin(), -camera.yaw.cos(), 0.0];
    state.listener = Some((p.to_array(), right));
    state
}

/// The party leader uses quickbar slot `slot`; an ability that needs aiming waits for its
/// target (its id is returned).
fn use_quickslot(live: &mut live::Live, slot: i32) -> Option<i32> {
    let aim = {
        let mut sim = live.sim();
        let who = sim.world.controlled();
        who.and_then(|w| thedas_ui::input::use_quickslot(&mut sim, w, slot))
    }?;
    let ability = aim.ability;
    live.aiming = Some(aim);
    Some(ability)
}

/// The conversation's reply `n` is chosen; what is said next.
fn reply(live: &mut live::Live, n: usize) -> String {
    let mut sim = live.sim();
    if sim.dialog_state().is_none() {
        return format!("reply {n}: no conversation");
    }
    // As the conversation screen's continue does: past the last line it closes.
    if thedas_ui::input::reply(&mut sim, n) {
        return format!("reply {n}: the conversation closes");
    }
    match sim.dialog_state() {
        Some(d) if d.ended => format!("reply {n}: the conversation ends"),
        Some(d) => format!("reply {n}: {}: {}", d.speaker, d.line),
        None => format!("reply {n}: the conversation closes"),
    }
}

/// The creature tagged `tag` nearest to `who`, in `who`'s area (an area left behind keeps its
/// own creatures of the same tag, which a talk order cannot reach).
fn nearest_tagged(
    sim: &thedas_sim::Sim,
    who: thedas_script::vm::ObjectId,
    tag: &str,
) -> Option<thedas_script::vm::ObjectId> {
    let me = sim.world.get(who)?;
    sim.world
        .by_tag(tag)
        .filter(|o| o.area.is_some() && o.area == me.area && o.id != who && o.active)
        .min_by(|a, b| {
            let d =
                |o: &thedas_sim::world::Object| thedas_sim::engine::dist(o.position, me.position);
            d(a).total_cmp(&d(b))
        })
        .map(|o| o.id)
}

/// Turns the camera `dyaw` and `dpitch` degrees: the game camera when it follows the leader,
/// else the free camera.
fn orbit_camera(follow: Option<&mut follow::Follow>, camera: &mut Camera, dyaw: f32, dpitch: f32) {
    let (y, p) = (dyaw.to_radians(), dpitch.to_radians());
    match follow {
        Some(f) => f.orbit(y, p),
        None => camera.look(y, p),
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
    (screens.exploring() || screens.picking_party()) && !layer.covers(x, y)
}

/// The headless capture's game, for the pad (see [`padplay::Host`]): the same steps a
/// `click` takes for the pointer's button.
struct HeadlessHost<'a> {
    live: &'a mut live::Live,
    played: Option<&'a mut (ui::UiLayer, screens::Screens)>,
    follow: Option<&'a mut follow::Follow>,
    camera: &'a mut Camera,
    cursor: &'a mut (f64, f64),
    size: (u32, u32),
}

impl padplay::Host for HeadlessHost<'_> {
    fn live(&mut self) -> Option<&mut live::Live> {
        Some(self.live)
    }

    fn camera(&self) -> Camera {
        *self.camera
    }

    fn follow(&mut self) -> Option<&mut follow::Follow> {
        self.follow.as_deref_mut()
    }

    fn look(&mut self, yaw: f32, pitch: f32) {
        self.camera.look(yaw, pitch);
    }

    fn size(&self) -> (u32, u32) {
        self.size
    }

    fn cursor(&self) -> (f64, f64) {
        *self.cursor
    }

    fn pointer_to(&mut self, x: f64, y: f64) {
        *self.cursor = (x, y);
        if let Some((layer, _)) = self.played.as_deref_mut() {
            layer.mouse_move(x, y);
        }
    }

    fn left_button(&mut self, down: bool) {
        let (x, y) = *self.cursor;
        if down && self.played.as_deref().is_some_and(|(_, s)| s.in_cutscene()) {
            self.live.sim().skip_cutscene();
            println!("pad click {x:.0},{y:.0}: skips the cutscene");
            return;
        }
        let to_world = match self.played.as_deref_mut() {
            Some((layer, s)) => {
                layer.render_movies();
                let w = world_takes_click(layer, s, x, y);
                layer.mouse_move(x, y);
                layer.mouse_button(x, y, down);
                if down && !w {
                    layer.clicked();
                }
                w
            }
            None => true,
        };
        if !down {
            return;
        }
        if !to_world {
            println!("pad click {x:.0},{y:.0}: interface");
            return;
        }
        let (w, h) = (self.size.0 as f32, self.size.1 as f32);
        let (o, d) = self.camera.ray(x as f32, y as f32, w, h);
        match self.live.pick(o, d).and_then(|p| self.live.click(p)) {
            Some(order) => println!("pad click {x:.0},{y:.0}: {order}"),
            None => println!("pad click {x:.0},{y:.0}: nothing"),
        }
    }

    fn right_button(&mut self, down: bool) {
        let (x, y) = *self.cursor;
        match self.played.as_deref_mut() {
            Some((layer, s)) if !s.exploring() || layer.covers(x, y) => {
                layer.render_movies();
                layer.button_as(thedas_ui::avm1::MOUSE_RIGHT, x, y, down);
                println!(
                    "pad right {} {x:.0},{y:.0}: interface",
                    if down { "press" } else { "release" }
                );
            }
            _ if down => {
                if let Some(aim) = self.live.aiming.take() {
                    println!("pad right click: ability {} not aimed", aim.ability);
                }
            }
            _ => {}
        }
    }

    fn double_click(&mut self) {
        let (x, y) = *self.cursor;
        if let Some((layer, _)) = self.played.as_deref_mut() {
            layer.render_movies();
            layer.click_as(
                thedas_ui::avm1::MOUSE_LEFT + thedas_ui::avm1::DOUBLE_CLICK,
                x,
                y,
            );
        }
    }

    fn screens(&mut self) -> Option<&mut screens::Screens> {
        self.played.as_deref_mut().map(|(_, s)| s)
    }

    fn popup(&mut self) -> bool {
        self.played
            .as_deref()
            .is_some_and(|(layer, _)| layer.stage.dialog_up())
    }

    fn focus_targets(&mut self) -> Vec<thedas_ui::FocusTarget> {
        self.played
            .as_deref_mut()
            .map(|(layer, _)| {
                layer.render_movies();
                layer.stage.focus_targets()
            })
            .unwrap_or_default()
    }

    fn text_focused(&mut self) -> bool {
        self.played
            .as_deref_mut()
            .is_some_and(|(layer, _)| layer.stage.text_focused())
    }

    fn text_key(&mut self, key: thedas_ui::UiKey) -> bool {
        self.played
            .as_deref_mut()
            .is_some_and(|(layer, _)| layer.stage.key(key))
    }

    fn movie_value(&mut self, movie: &str, path: &str) -> Option<thedas_ui::UiValue> {
        self.played.as_deref_mut()?.0.probe_call(movie, path, &[])
    }

    fn open_saves(&mut self, mode: i32) {
        if let Some((layer, s)) = self.played.as_deref_mut() {
            layer.open_saves(s, mode);
        }
    }

    fn line(&self) -> Option<(bool, bool)> {
        self.played
            .as_deref()
            .and_then(|(layer, _)| layer.line_state())
    }

    fn interface_clicked(&mut self) {
        if let Some((layer, _)) = self.played.as_deref_mut() {
            layer.clicked();
        }
    }

    fn scroll(&mut self, lines: f64) {
        let (x, y) = *self.cursor;
        if let Some((layer, _)) = self.played.as_deref_mut() {
            layer.mouse_wheel(x, y, lines);
            println!("pad scroll {lines} at {x:.0},{y:.0}");
        }
    }

    fn highlight_reply(&mut self, n: usize) -> bool {
        self.played
            .as_deref_mut()
            .is_some_and(|(layer, _)| layer.highlight_reply(n))
    }
}

/// The loading screen, while a new level builds (`Live::loading`): up when the build starts,
/// with the hint a script set for the transition, played out when the world is ready. While
/// the world map is open (travelling) the map itself stays up instead, as in the exe.
/// Provisional: load_town.gfx for every load; the exe shows load_light.gfx for an area
/// change within the same area list, which the app does not tell apart yet.
/// Sets out on the world map to the map pin tagged `tag` (see [`Step::Travel`]).
fn travel_to_pin(sim: &mut thedas_sim::Sim, tag: &str) -> bool {
    let pin = sim
        .world
        .objects
        .values()
        .find(|o| {
            o.kind == thedas_sim::world::ObjectKind::MapPin && o.tag.eq_ignore_ascii_case(tag)
        })
        .map(|o| o.id);
    pin.is_some_and(|p| sim.begin_travel(p))
}

fn loading_screen(live: &live::Live, ui: &mut ui::UiLayer, up: &mut bool) {
    // Travelling on the world map: the map stays up as that load's screen (GUI 16 in the
    // exe, 0x6c6060; the simulation closes it to input when travel starts), from the pin
    // clicked until the new area's world is up, but not during a random encounter's stop
    // (see `Live::travel_load`).
    let travelling = live.travel_load();
    ui.hold_world_map(travelling);
    // THEDAS_NO_LOAD_MOVIE: no loading movie; a load shows the interface over black.
    if std::env::var_os("THEDAS_NO_LOAD_MOVIE").is_some() {
        return;
    }
    let pending = live.pending_load_screen();
    match (live.loading() || pending.is_some(), *up) {
        (true, false) => {
            let sim = live.sim();
            // The world map is the journey's loading screen: no loading movie over it.
            if sim.world.world_map.open || travelling {
                return;
            }
            let hint = thedas_ui::binding::load_hint(&sim);
            drop(sim);
            let kind = live.load_screen().unwrap_or(ui::LoadScreen::Town);
            log::info!("loading screen: {kind:?}");
            ui.loading_begin(kind, hint);
            *up = true;
        }
        (false, true) => {
            ui.loading_end();
            *up = false;
        }
        _ => {}
    }
}

/// One step of `--steps`.
#[derive(Clone, Debug, PartialEq)]
enum Step {
    Click(f64, f64),
    Run(u64),
    /// I, C, T or M: the player's inventory, character record, tactics or area map opens or
    /// closes.
    Menu(screens::Screen),
    /// The party picker opens as a script's `ShowPartyPickerGUI` opens it (for trying the
    /// picker headless).
    Picker,
    /// F5 or F9: the save screen (1) or the load screen (2) opens.
    Saves(i32),
    /// Text typed into the focused text field (`type <text>`).
    Type(String),
    /// A key for the focused text field (`key backspace`, `enter`, `delete`, `left`, `right`,
    /// `home`, `end`, `tab`, `escape`, `ctrl+a`).
    TextKey(thedas_ui::UiKey),
    /// The whole party falls, as the game sees it after a wipe (for trying the death screen
    /// headless).
    Wipe,
    /// The leader is ordered to talk to the first creature with this tag (as a click on it
    /// does, for creatures hard to reach on screen).
    Talk(String),
    /// The objects' scene is rebuilt, as when a creature spawns (timing).
    Rebuild,
    /// A right click or a double click at a pixel, for the interface.
    ClickAs(f64, f64, f64),
    /// The mouse rests at a pixel (where an aimed ability's reticle shows).
    Hover(f64, f64),
    /// F1-F4: party member n leads.
    Leader(usize),
    /// 1-9, 0: the quickbar's slot n is used.
    Quickslot(i32),
    /// Space: the game pauses or carries on.
    Pause,
    /// The conversation's reply n (from 0) is chosen.
    Reply(usize),
    /// The game camera turns (degrees of yaw, then pitch), as dragging with the right
    /// button or Q/E does.
    Orbit(f32, f32),
    /// The game camera zooms in (positive) or out, in wheel steps.
    Zoom(f32),
    /// The left button goes down, or up, at a pixel: with `hover` and `run` between, a drag
    /// (the quickbar's icons, the inventory's items).
    Press(f64, f64),
    /// The right button down (true) or up at a pixel, for the interface (`rpress`,
    /// `rrelease`).
    RightAt(f64, f64, bool),
    Release(f64, f64),
    /// A game controller's button goes down (true) or up: `pad a` presses and lets go,
    /// `pad press rt` and `pad release rt` hold and let go.
    PadButton(pad::Button, bool),
    /// A game controller's stick is pushed (x, y; +y up): `pad stick-left 0,1 500` holds it
    /// there for a `run` of 500 ms, then lets go.
    PadStick(pad::Stick, f32, f32),
    /// A movie function called by path, its answer printed, for probing the movies headless:
    /// `call inventory.gfx _global.GUISystem.InConsoleMode [arg ...]` (numbers, else strings).
    Call(String, String, Vec<String>),
    /// An engine value the movies read with `GetValue` (`setvalue DebugOptions.EnableController 1`).
    SetValue(String, String),
    /// `option GameOptions.ShowTutorials 0`: a player setting, as the options screen sets it
    /// (the interface keeps it, the simulation hears it).
    ClientOption(String, String),
    /// `transition AREA WAYPOINT`: the party moves to another area as a script's
    /// `DoAreaTransition` moves it (taken when the current scripts finish).
    Transition(String, String),
    /// `travel PIN`: the party sets out on the world map to the pin with that tag, as
    /// clicking it does (`Sim::begin_travel`).
    Travel(String),
    /// `key pagedown MS` / `key pageup MS`: the zoom keys held (+1 in, -1 out, 0 let go).
    ZoomKey(f32),
    /// A floating message over the controlled creature, as a script's
    /// `DisplayFloatyMessage` makes one (`floaty 12`), for trying the floaty layer headless.
    Floaty(String),
    /// Sets a plot flag as a script would (`plot plt_tut_explore 0`), running the plot's
    /// script, for trying journal entries and tutorial popups headless.
    Plot(String, i32),
    /// A cutscene plays as a script's `CS_LoadCutscene` queues it (`cutscene 25860_m`), for
    /// trying cutscenes headless where the story would play them.
    Cutscene(String),
    /// The player does a training action (`training MAXIMUM_ZOOM_IN`): the training step
    /// waiting for it hears of it (`EVENT_TYPE_TRAINING_<name>`, see `thedas_sim::training`).
    Training(String),
    /// `trainingmode N`: a training lesson starts as a script's `SetTrainingMode(N)` starts it
    /// (0 ends the one under way), for stepping through the lessons.
    TrainingMode(i32),
    /// A loading screen goes up (`loading town|light|fade`, with the hint the simulation
    /// would show) or plays out (`loading end`): for trying the loading movies headless.
    Loading(Option<ui::LoadScreen>),
    /// Uses the nearest object with a tag, as clicking it would (`use bdc140ip_everds_chest`).
    Use(String),
}

/// A `pad` step: a button by name (pressed and let go), `press`/`release` and a button, or
/// `stick-left`/`stick-right` with `x,y` and milliseconds (held while the game runs that
/// long, then let go).
fn pad_steps(arg: &str) -> Result<Vec<Step>> {
    let parts: Vec<&str> = arg.split_whitespace().collect();
    let button =
        |name: &str| pad::Button::parse(name).with_context(|| format!("no pad button {name:?}"));
    Ok(match parts.as_slice() {
        [b] => {
            let b = button(b)?;
            vec![Step::PadButton(b, true), Step::PadButton(b, false)]
        }
        ["press", b] => vec![Step::PadButton(button(b)?, true)],
        ["release", b] => vec![Step::PadButton(button(b)?, false)],
        [stick @ ("stick-left" | "stick-right"), xy, ms] => {
            let stick = if *stick == "stick-left" {
                pad::Stick::Left
            } else {
                pad::Stick::Right
            };
            let (x, y) = xy.split_once(',').context("a stick takes x,y")?;
            let (x, y): (f32, f32) = (x.trim().parse()?, y.trim().parse()?);
            vec![
                Step::PadStick(stick, x.clamp(-1.0, 1.0), y.clamp(-1.0, 1.0)),
                Step::Run(ms.parse().context("a stick takes milliseconds")?),
                Step::PadStick(stick, 0.0, 0.0),
            ]
        }
        _ => bail!(
            "pad takes a button, press/release and a button, or stick-left/stick-right x,y ms"
        ),
    })
}

/// The player settings the simulation reads, from the interface's, for a game just started
/// or loaded: a new game's simulation starts with the defaults, so a setting changed in the
/// options screen before New Game reaches it here (the tutorials, auto-pause in combat).
fn seed_client_options(l: &live::Live, ui: &ui::UiLayer) {
    let mut sim = l.sim();
    for name in ["GameOptions.ShowTutorials", "GameOptions.AutoPauseCombat"] {
        if let Some(v) = ui.client_option(name) {
            sim.client_option_message(
                thedas_sim::chargen::MSG_CLIENT_OPTION,
                &[name.to_string(), v.to_string()],
            );
        }
    }
}

/// An `option` step: a player setting changed as the options screen changes it.
fn set_client_option(ui: Option<&ui::UiLayer>, live: Option<&live::Live>, name: &str, value: &str) {
    if let Some(ui) = ui {
        ui.set_client_option(name, value);
    }
    if let Some(l) = live {
        l.sim().client_option_message(
            thedas_sim::chargen::MSG_CLIENT_OPTION,
            &[name.to_string(), value.to_string()],
        );
    }
}

/// A `key` step's text-field key, by name.
fn text_key(name: &str) -> Option<thedas_ui::UiKey> {
    use thedas_ui::UiKey;
    Some(match name.to_ascii_lowercase().as_str() {
        "backspace" => UiKey::Backspace,
        "delete" => UiKey::Delete,
        "enter" => UiKey::Enter,
        "escape" => UiKey::Escape,
        "tab" => UiKey::Tab,
        "left" => UiKey::Left,
        "right" => UiKey::Right,
        "home" => UiKey::Home,
        "end" => UiKey::End,
        "ctrl+a" => UiKey::SelectAll,
        _ => return None,
    })
}

/// The player's screen a key opens (I, C, T, M, J, K, P, O).
fn menu_key(code: KeyCode) -> Option<screens::Screen> {
    match code {
        KeyCode::KeyI => Some(screens::Screen::Inventory),
        KeyCode::KeyC => Some(screens::Screen::CharacterRecord),
        // T, and Backslash as the game binds it (`ToggleTactics_0`).
        KeyCode::KeyT | KeyCode::Backslash => Some(screens::Screen::Tactics),
        KeyCode::KeyM => Some(screens::Screen::AreaMap),
        KeyCode::KeyJ => Some(screens::Screen::Journal),
        KeyCode::KeyK => Some(screens::Screen::Skills),
        KeyCode::KeyP => Some(screens::Screen::SpellsTalents),
        KeyCode::KeyO => Some(screens::Screen::Options),
        _ => None,
    }
}

/// A floating message over the controlled creature (the `floaty` step): white, the default
/// style, two seconds.
fn test_floaty(sim: &mut thedas_sim::Sim, text: &str) {
    if let Some(target) = sim.world.controlled() {
        sim.world.floaties.push(thedas_sim::world::FloatyMessage {
            target,
            text: text.to_string(),
            style: 0,
            colour: 0xffffff,
            duration: 2.0,
        });
    }
}

/// Every party member falls (the `wipe` step): what a lost fight leaves, for the death screen.
fn wipe(sim: &mut thedas_sim::Sim) {
    for m in thedas_sim::engine::party_of(sim) {
        sim.kill(m, thedas_script::vm::OBJECT_INVALID);
    }
    sim.tick(0);
}

fn parse_steps(spec: &str) -> Result<Vec<Step>> {
    let mut out = Vec::new();
    for s in spec.split(';').map(str::trim).filter(|s| !s.is_empty()) {
        let (verb, arg) = s.split_once(' ').unwrap_or((s, ""));
        let key_hold = arg
            .trim()
            .split_once(' ')
            .filter(|(k, _)| matches!(k.to_ascii_lowercase().as_str(), "pagedown" | "pageup"));
        if verb == "pad" {
            out.extend(pad_steps(arg.trim())?);
        } else if let (true, Some((k, ms))) = (verb == "key", key_hold) {
            // Page Down zooms in, Page Up out, held for `ms`.
            let dir = if k.eq_ignore_ascii_case("pagedown") {
                1.0
            } else {
                -1.0
            };
            out.push(Step::ZoomKey(dir));
            out.push(Step::Run(
                ms.trim().parse().context("a held key takes milliseconds")?,
            ));
            out.push(Step::ZoomKey(0.0));
        } else if verb == "travel" && !arg.trim().is_empty() {
            out.push(Step::Travel(arg.trim().to_string()));
        } else if verb == "transition" {
            let (area, waypoint) = arg
                .trim()
                .split_once(' ')
                .context("transition takes an area and a waypoint")?;
            out.push(Step::Transition(
                area.trim().to_string(),
                waypoint.trim().to_string(),
            ));
        } else if verb == "option" {
            let (k, v) = arg
                .trim()
                .split_once(' ')
                .context("option takes a name and a value")?;
            out.push(Step::ClientOption(k.to_string(), v.trim().to_string()));
        } else {
            out.push(parse_step(s)?);
        }
    }
    Ok(out)
}

fn parse_step(s: &str) -> Result<Step> {
    let (verb, arg) = s.split_once(' ').unwrap_or((s, ""));
    let arg = arg.trim();
    Ok(match verb {
        "click" => {
            let (x, y) = arg.split_once(',').context("click takes x,y")?;
            Step::Click(x.trim().parse()?, y.trim().parse()?)
        }
        "run" => Step::Run(arg.parse().context("run takes milliseconds")?),
        "call" => {
            let mut w = arg.split_whitespace().map(str::to_string);
            let movie = w.next().context("call takes a movie and a path")?;
            let path = w.next().context("call takes a movie and a path")?;
            Step::Call(movie, path, w.collect())
        }
        "setvalue" => {
            let (k, v) = arg
                .split_once(' ')
                .context("setvalue takes a name and a value")?;
            Step::SetValue(k.to_string(), v.trim().to_string())
        }
        "floaty" => Step::Floaty(arg.to_string()),
        "training" => Step::Training(arg.trim().to_ascii_uppercase()),
        "trainingmode" => {
            Step::TrainingMode(arg.trim().parse().context("trainingmode takes a number")?)
        }
        "loading" => Step::Loading(match arg.trim() {
            "town" => Some(ui::LoadScreen::Town),
            "light" => Some(ui::LoadScreen::Light),
            "fade" => Some(ui::LoadScreen::Fade),
            "end" => None,
            _ => anyhow::bail!("loading takes town, light, fade or end"),
        }),
        "plot" => {
            let (plot, flag) = arg
                .rsplit_once(' ')
                .context("plot takes a plot and a flag")?;
            Step::Plot(
                plot.trim().to_string(),
                flag.trim().parse().context("plot's flag is a number")?,
            )
        }
        // The game menu (Escape while playing).
        "gamemenu" => Step::Menu(screens::Screen::GameMenu),
        "key" if arg.eq_ignore_ascii_case("i") => Step::Menu(screens::Screen::Inventory),
        "key" if arg.eq_ignore_ascii_case("c") => Step::Menu(screens::Screen::CharacterRecord),
        "key" if arg.eq_ignore_ascii_case("t") => Step::Menu(screens::Screen::Tactics),
        "key" if arg.eq_ignore_ascii_case("m") => Step::Menu(screens::Screen::AreaMap),
        "key" if arg.eq_ignore_ascii_case("j") => Step::Menu(screens::Screen::Journal),
        "key" if arg.eq_ignore_ascii_case("k") => Step::Menu(screens::Screen::Skills),
        "key" if arg.eq_ignore_ascii_case("p") => Step::Menu(screens::Screen::SpellsTalents),
        "key" if arg.eq_ignore_ascii_case("o") => Step::Menu(screens::Screen::Options),
        "key" if matches!(arg.to_ascii_lowercase().as_str(), "f1" | "f2" | "f3" | "f4") => {
            Step::Leader(arg[1..].parse::<usize>()? - 1)
        }
        "key" if arg.eq_ignore_ascii_case("space") => Step::Pause,
        "key" if arg.len() == 1 && arg.as_bytes()[0].is_ascii_digit() => {
            // 1 is the first slot, 0 the tenth.
            Step::Quickslot((arg.as_bytes()[0] - b'0' + 9) as i32 % 10)
        }
        "reply" => Step::Reply(arg.parse().context("reply takes a number")?),
        "orbit" => {
            let (x, y) = arg.split_once(',').unwrap_or((arg, "0"));
            Step::Orbit(x.trim().parse()?, y.trim().parse()?)
        }
        "zoom" => Step::Zoom(arg.parse().context("zoom takes a number of steps")?),
        // Q and E turn the camera; a step turns it an eighth of the way round.
        "key" if arg.eq_ignore_ascii_case("q") => Step::Orbit(45.0, 0.0),
        "key" if arg.eq_ignore_ascii_case("e") => Step::Orbit(-45.0, 0.0),
        "press" | "release" => {
            let (x, y) = arg.split_once(',').context("press takes x,y")?;
            let (x, y) = (x.trim().parse()?, y.trim().parse()?);
            if verb == "press" {
                Step::Press(x, y)
            } else {
                Step::Release(x, y)
            }
        }
        "talk" if !arg.is_empty() => Step::Talk(arg.to_string()),
        "cutscene" if !arg.is_empty() => Step::Cutscene(arg.to_string()),
        "use" if !arg.is_empty() => Step::Use(arg.to_string()),
        "rebuild" => Step::Rebuild,
        "picker" => Step::Picker,
        "wipe" => Step::Wipe,
        "type" => Step::Type(arg.to_string()),
        "key" if text_key(arg).is_some() => {
            Step::TextKey(text_key(arg).unwrap_or(thedas_ui::UiKey::Enter))
        }
        "key" if arg.eq_ignore_ascii_case("f5") => Step::Saves(1),
        "key" if arg.eq_ignore_ascii_case("f9") => Step::Saves(2),
        "hover" => {
            let (x, y) = arg.split_once(',').context("hover takes x,y")?;
            Step::Hover(x.trim().parse()?, y.trim().parse()?)
        }
        "rpress" | "rrelease" => {
            let (x, y) = arg.split_once(',').context("rpress takes x,y")?;
            Step::RightAt(x.trim().parse()?, y.trim().parse()?, verb == "rpress")
        }
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
}

/// The viewer's level when `--area` is not given.
const DEFAULT_AREA: &str = "lgt101d";

fn main() -> Result<()> {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info,wgpu=warn,naga=warn"),
    )
    .init();
    let args = Args::parse();
    if let Some(secs) = args.pad_probe {
        paddev::probe(secs);
        return Ok(());
    }
    let install = args
        .install
        .or_else(|| std::env::var_os("THEDAS_INSTALL").map(PathBuf::from))
        .context("give --install or set THEDAS_INSTALL")?;
    let size = parse_size(&args.size)?;
    let saves_dir = args
        .saves
        .clone()
        .unwrap_or_else(saves::SaveStore::default_dir);
    let start = if let Some(save) = &args.save {
        Some(live::Start::Save(save))
    } else if args.new_game && args.quick_start {
        Some(live::Start::NewGameQuick)
    } else if args.new_game {
        Some(live::Start::NewGame)
    } else if args.objects {
        Some(live::Start::Area(
            args.area.as_deref().unwrap_or(DEFAULT_AREA),
        ))
    } else if args.viewer || args.ui.is_some() || args.area.is_some() || args.camera.is_some() {
        None
    } else {
        // As the game boots: the logo and intro movies (below), then the start menu.
        Some(live::Start::Menu)
    };
    let boot = matches!(start, Some(live::Start::Menu));
    let mut live = match start {
        Some(start) => Some(live::Live::open(
            &install,
            &args.module,
            start,
            &args.shaders,
        )?),
        None => None,
    };
    // THEDAS_ASYNC_LOADS: headless too, a new level builds on a worker while frames run on
    // (as the window does), for testing that path.
    if let Some(l) = live.as_mut() {
        l.async_loads = std::env::var_os("THEDAS_ASYNC_LOADS").is_some();
    }
    // The name character creation would have taken (its script calls SetName with it).
    if let (Some(l), Some(name), true) =
        (live.as_ref(), &args.name, args.new_game && args.quick_start)
    {
        let mut sim = l.sim();
        if let Some(h) = sim.world.hero.and_then(|h| sim.world.get_mut(h)) {
            h.name = Some(name.trim().to_string()).filter(|n| !n.is_empty());
        }
    }
    // Playing, the scene comes from the game; the interface still reads the global stack.
    let area = if live.is_some() {
        "none"
    } else {
        args.area.as_deref().unwrap_or(DEFAULT_AREA)
    };
    let detail = thedas_render::scene::Detail::parse(&args.detail).unwrap_or_default();
    let mut loaded = load(&install, area, &args.shaders, &[], detail)?;
    loaded.frame_effects = args.frame_effects == "on";
    if let Some(l) = live.as_mut() {
        l.frame_effects = loaded.frame_effects;
        l.detail = detail;
    }
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
    // A zoom key a step holds down (`key pagedown MS`).
    let mut zoom_key = 0.0f32;
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
        // The game's sound, recorded rather than played (`--audio-wav`).
        let mut recording = args.audio_wav.as_ref().map(|_| {
            audio::GameAudio::recording(&install, &args.module, &loaded.vfs, &args.language, 48000)
        });
        let mut cursor = (size.0 as f64 / 2.0, size.1 as f64 / 2.0);
        // The game controller as `pad` steps hold it, and its part of the game.
        let mut pad_state = pad::PadState::default();
        let mut pad_play = padplay::PadPlay::default();
        let mut pad_lines = (Vec::new(), Vec::new());
        if let Some(l) = live.as_mut() {
            l.shader_time = args.time;
            l.movies_wait = true;
            if ui.is_empty() {
                let mut layer = ui::UiLayer::new(&ctx, &loaded.vfs, &[], size);
                layer.attach(&l.sim);
                layer.set_saves(saves::SaveStore::new(saves_dir.clone()));
                let mut s = screens::Screens::new(l.front_end);
                l.aiming = layer.play(&mut s, &l.sim, 0.0).pop();
                played = Some((layer, s));
            }
            if let Some(c) = follow
                .as_mut()
                .and_then(|f| l.follow_camera(f, 0.0, &camera))
            {
                camera = c;
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
            let mut load_screen_up = false;
            for step in &steps {
                let l = live.as_mut().expect("a game or the start menu");
                match step {
                    &Step::Hover(x, y) => {
                        cursor = (x, y);
                        if let Some((layer, _)) = played.as_mut() {
                            layer.mouse_move(x, y);
                        }
                    }
                    &Step::Press(x, y) | &Step::Release(x, y) => {
                        let down = matches!(step, Step::Press(..));
                        if let Some((layer, _)) = played.as_mut() {
                            layer.render_movies();
                            layer.mouse_move(x, y);
                            layer.mouse_button(x, y, down);
                        }
                        cursor = (x, y);
                        println!("{} {x},{y}", if down { "press" } else { "release" });
                    }
                    &Step::Leader(n) => match l.choose_leader(n) {
                        Some(tag) => println!("leading with {tag}"),
                        None => println!("no party member {}", n + 1),
                    },
                    &Step::ClickAs(button, x, y) => {
                        if let Some((layer, _)) = played.as_mut() {
                            layer.click_as(button, x, y);
                            println!("click {x},{y} as button {button}: interface");
                        }
                    }
                    &Step::RightAt(x, y, down) => {
                        if let Some((layer, _)) = played.as_mut() {
                            layer.render_movies();
                            layer.button_as(thedas_ui::avm1::MOUSE_RIGHT, x, y, down);
                        }
                        cursor = (x, y);
                        println!("right {} {x},{y}", if down { "press" } else { "release" });
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
                        let target = who.and_then(|w| nearest_tagged(&sim, w, tag));
                        match (who, target) {
                            (Some(w), Some(t)) => {
                                sim.order_talk(w, t);
                                println!("talk to {tag}");
                            }
                            _ => println!("talk to {tag}: nobody"),
                        }
                    }
                    &Step::Click(x, y) => {
                        if played.as_ref().is_some_and(|(_, s)| s.in_cutscene()) {
                            l.sim().skip_cutscene();
                            println!("click {x},{y}: skips the cutscene");
                            continue;
                        }
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
                            let mut host = HeadlessHost {
                                live: &mut *l,
                                played: played.as_mut(),
                                follow: follow.as_mut(),
                                camera: &mut camera,
                                cursor: &mut cursor,
                                size,
                            };
                            for m in pad_play.frame(&pad_state, &mut host, dt) {
                                println!("pad: {m}");
                            }
                            if !played.as_ref().is_some_and(|(_, s)| s.world_stopped())
                                || l.transition_pending()
                                || l.journey()
                                || l.training()
                            {
                                l.step(dt);
                            }
                            l.advance_presentation(dt);
                            // As the window does: the drawn scenes follow the world (objects
                            // that come and go are rebuilt).
                            l.sync(&ctx, thedas_render::gpu::CAPTURE_FORMAT)?;
                            // As the window's frames do: a cutscene's view is noted for the
                            // conversation line it plays over (see `Live::conversation_camera`).
                            if follow.is_some() {
                                let _ = l.cutscene_camera(&camera);
                            }
                            // A level building on its worker (THEDAS_ASYNC_LOADS) is waited
                            // for here, so the next frame, capture or input finds the world
                            // up: headless frames run far faster than a load.
                            if l.loading() {
                                if let Some((layer, _)) = played.as_mut() {
                                    loading_screen(l, layer, &mut load_screen_up);
                                }
                                while l.loading() {
                                    std::thread::sleep(std::time::Duration::from_millis(5));
                                    l.sync(&ctx, thedas_render::gpu::CAPTURE_FORMAT)?;
                                }
                            }
                            if let Some((layer, _)) = played.as_mut() {
                                loading_screen(l, layer, &mut load_screen_up);
                            }
                            if let Some((layer, s)) = played.as_mut() {
                                let t = Instant::now();
                                let level = l.level.as_ref();
                                layer.set_level(
                                    level.map(|x| x.layout.as_str()),
                                    level.map(|x| &x.vfs),
                                );
                                let key = ui::portrait_key(&l.sim());
                                if layer.wants_portraits(key, dt) {
                                    let shots = l.portraits(&ctx, ui::PORTRAIT_SIZE);
                                    layer.set_portraits(shots);
                                }
                                let due = layer.wants_paperdoll(s, &l.sim(), dt);
                                if let Some((who, yaw)) = due {
                                    let (w, h) = ui::PAPERDOLL_SIZE;
                                    let shot = l.paperdoll(&ctx, who, yaw, w, h);
                                    layer.set_paperdoll(shot);
                                }
                                layer.set_paused(l.paused);
                                layer.set_pad_active(pad_play.used);
                                if let Some(aim) = layer.play(s, &l.sim, dt).pop() {
                                    println!("aiming ability {}", aim.ability);
                                    l.aiming = Some(aim);
                                }
                                layer.update_minimap(&l.sim, &camera);
                                layer.update_floaties(&l.sim, &camera, dt);
                                ui_time += t.elapsed();
                            }
                            if let Some(a) = recording.as_mut() {
                                let ui = played.as_mut().map(|(layer, _)| layer);
                                let state = audio_state(a, Some(&mut *l), ui, &camera, None);
                                a.record(state, dt as f32 / 1000.0);
                                let voice = a.voice_ms();
                                if let Some((layer, _)) = played.as_mut() {
                                    layer.set_line_voice_ms(voice);
                                }
                            }
                            frames += 1;
                            if zoom_key != 0.0
                                && let Some(f) = follow.as_mut()
                                && f.zoom(zoom_key * pad::ZOOM_SPEED * dt as f32 / 1000.0)
                            {
                                l.sim().player_zoomed_in();
                            }
                            if let (Some(f), Some(heading)) =
                                (follow.as_mut(), l.camera_reset_due())
                            {
                                f.reset(heading);
                            }
                            if let (Some(f), Some(level)) = (follow.as_mut(), l.zoom_request_due())
                            {
                                f.set_zoom_level(level);
                            }
                            let eye = follow
                                .as_mut()
                                .and_then(|f| l.follow_camera(f, dt as f32 / 1000.0, &camera));
                            if let Some(c) = eye {
                                camera = c;
                            }
                            l.fade_near_camera(eye.map(|c| c.position), dt as f32 / 1000.0);
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
                    Step::Picker => {
                        l.sim().party_picker();
                        println!("party picker");
                    }
                    Step::Type(t) => {
                        let typed = played.as_mut().is_some_and(|(layer, _)| layer.type_text(t));
                        println!(
                            "type {t:?}: {}",
                            if typed { "typed" } else { "no text field" }
                        );
                    }
                    &Step::TextKey(k) => {
                        let typed = played.as_mut().is_some_and(|(layer, _)| layer.key(k));
                        println!(
                            "key {k:?}: {}",
                            if typed { "typed" } else { "no text field" }
                        );
                    }
                    &Step::Saves(mode) => {
                        if let Some((layer, s)) = played.as_mut() {
                            layer.open_saves(s, mode);
                        }
                        println!("{} screen", if mode == 1 { "save" } else { "load" });
                    }
                    Step::Wipe => {
                        wipe(&mut l.sim());
                        println!("the party falls");
                    }
                    Step::Call(movie, path, args) => {
                        let r = played
                            .as_mut()
                            .and_then(|(layer, _)| layer.probe_call(movie, path, args));
                        println!("call {movie} {path}{args:?}: {r:?}");
                    }
                    Step::SetValue(k, v) => {
                        if let Some((layer, _)) = played.as_mut() {
                            layer.probe_set_value(k, v);
                        }
                        println!("setvalue {k} = {v}");
                    }
                    Step::ClientOption(k, v) => {
                        set_client_option(played.as_ref().map(|(layer, _)| layer), Some(l), k, v);
                        println!("option {k} = {v}");
                    }
                    &Step::ZoomKey(dir) => zoom_key = dir,
                    Step::Travel(tag) => {
                        let ok = travel_to_pin(&mut l.sim(), tag);
                        println!(
                            "travel {tag}: {}",
                            if ok { "set out" } else { "no such active pin" }
                        );
                    }
                    Step::Transition(area, waypoint) => {
                        // As a script's DoAreaTransition does: performed at the start of the next
                        // update (`TRANSITION_DELAY_MS`).
                        let mut sim = l.sim();
                        sim.world.transition = Some((area.clone(), waypoint.clone()));
                        sim.world.transition_at =
                            Some(sim.world.time_ms + thedas_sim::sim::TRANSITION_DELAY_MS);
                        println!("transition {area} {waypoint}");
                    }
                    Step::Floaty(text) => {
                        test_floaty(&mut l.sim(), text);
                        println!("floaty {text:?}");
                    }
                    Step::Use(tag) => {
                        let mut sim = l.sim();
                        let who = sim.world.controlled();
                        match (who, who.and_then(|w| nearest_tagged(&sim, w, tag))) {
                            (Some(w), Some(t)) => {
                                sim.order_use(w, t);
                                println!("use {tag}");
                            }
                            _ => println!("use {tag}: nothing"),
                        }
                    }
                    Step::Loading(kind) => {
                        let hint = thedas_ui::binding::load_hint(&l.sim());
                        if let Some((layer, _)) = played.as_mut() {
                            match kind {
                                Some(k) => layer.loading_begin(*k, hint.clone()),
                                None => layer.loading_end(),
                            }
                        }
                        let state = played
                            .as_ref()
                            .map(|(layer, _)| (layer.loading(), layer.loading_fade_done()));
                        println!("loading {kind:?}: hint {hint:?}, up and faded {state:?}");
                    }
                    &Step::TrainingMode(mode) => {
                        l.sim().set_training_mode(mode);
                        println!("training mode {mode}");
                    }
                    Step::Training(name) => {
                        let heard = l.sim().training_event(name, 0);
                        println!("training {name}: {heard}");
                    }
                    Step::Plot(plot, flag) => {
                        l.sim().set_plot_flag(
                            plot,
                            *flag,
                            true,
                            true,
                            thedas_script::vm::OBJECT_INVALID,
                        );
                        println!("plot {plot} {flag}");
                    }
                    Step::Cutscene(name) => {
                        l.sim().world.cutscenes.push(name.clone());
                        println!("cutscene {name}");
                    }
                    &Step::Menu(m) => {
                        if let Some((layer, s)) = played.as_mut() {
                            s.toggle(m);
                            if let Some(aim) = layer.play(s, &l.sim, 0.0).pop() {
                                l.aiming = Some(aim);
                            }
                        }
                    }
                    &Step::Quickslot(slot) => match use_quickslot(l, slot) {
                        Some(ability) => println!("aiming ability {ability}"),
                        None => println!("quickslot {}", slot + 1),
                    },
                    Step::Pause => {
                        // As the Space key does (a training tutorial takes it as its unpause).
                        l.toggle_pause();
                        println!("{}", if l.paused { "paused" } else { "running" });
                    }
                    &Step::Reply(n) => println!("{}", reply(l, n)),
                    &Step::PadButton(_, _) | &Step::PadStick(..) => {
                        match *step {
                            Step::PadButton(b, down) => pad_state.set(b, down),
                            Step::PadStick(stick, x, y) => {
                                pad_state.set_stick(stick, glam::Vec2::new(x, y))
                            }
                            _ => {}
                        }
                        let mut host = HeadlessHost {
                            live: &mut *l,
                            played: played.as_mut(),
                            follow: follow.as_mut(),
                            camera: &mut camera,
                            cursor: &mut cursor,
                            size,
                        };
                        for m in pad_play.frame(&pad_state, &mut host, 0.0) {
                            println!("pad: {m}");
                        }
                        // A layer's frame shows what the pad's clicks did.
                        if let Some((layer, s)) = played.as_mut() {
                            if let Some(aim) = layer.play(s, &l.sim, 0.0).pop() {
                                l.aiming = Some(aim);
                            }
                        }
                    }
                    &Step::Orbit(dyaw, dpitch) => {
                        orbit_camera(follow.as_mut(), &mut camera, dyaw, dpitch);
                        if let Some(c) = follow
                            .as_mut()
                            .and_then(|f| l.follow_camera(f, 0.0, &camera))
                        {
                            camera = c;
                        }
                        println!("orbit {dyaw},{dpitch}");
                    }
                    &Step::Zoom(n) => {
                        if let Some(f) = follow.as_mut()
                            && f.zoom(n)
                        {
                            l.sim().player_zoomed_in();
                        }
                        println!("zoom {n}");
                    }
                }
                // A staged conversation line's shot is held for the lines after it.
                if follow.is_some() {
                    let _ = l.conversation_camera(&camera);
                }
                // What the menus asked: a game started or loaded, or back to the start menu.
                if let Some((layer, s)) = played.as_mut() {
                    for r in std::mem::take(&mut layer.requests) {
                        println!("front end: {r:?}");
                        let start = match &r {
                            ui::FrontEnd::Quit => continue,
                            ui::FrontEnd::NewGame => live::Start::NewGame,
                            ui::FrontEnd::Load(b) => live::Start::SaveBytes(b),
                            ui::FrontEnd::StartMenu => live::Start::Menu,
                        };
                        let mut n = live::Live::open(&install, &args.module, start, &args.shaders)?;
                        n.shader_time = args.time;
                        n.movies_wait = true;
                        layer.close_all();
                        layer.attach(&n.sim);
                        seed_client_options(&n, layer);
                        *s = screens::Screens::new(n.front_end);
                        follow = n.hero().map(|(_, facing)| {
                            let heading = thedas_sim::world::heading_of(facing).to_radians();
                            match follow.take() {
                                Some(mut f) => {
                                    f.reset(heading);
                                    f
                                }
                                None => follow::Follow::behind(heading),
                            }
                        });
                        if let Some((pos, facing)) = n.hero() {
                            let fov = camera.fov_y;
                            camera = hero_camera(pos, facing);
                            camera.fov_y = fov;
                        }
                        live = Some(n);
                    }
                }
            }
            let l = live.as_mut().expect("a game or the start menu");
            let sim = l.sim();
            if let Some(h) = sim.world.controlled().and_then(|h| sim.world.get(h)) {
                log::info!(
                    "leader at {:?}, doing {:?}, cooling down {:?}, quickbar {:?}, saying: {:?}",
                    h.position,
                    h.commands
                        .iter()
                        .map(|c| c.name.as_str())
                        .collect::<Vec<_>>(),
                    h.combat.cooldowns.keys().collect::<Vec<_>>(),
                    h.quickslots
                        .iter()
                        .map(|(s, q)| (*s, q.ability))
                        .collect::<Vec<_>>(),
                    sim.dialog_state().map(|d| (d.line, d.replies, d.ended))
                );
            }
            let party: Vec<_> = thedas_sim::engine::party_of(&sim)
                .into_iter()
                .filter_map(|m| sim.world.get(m))
                .map(|o| {
                    (
                        o.tag.clone(),
                        o.position,
                        o.commands.first().map(|c| c.name.clone()),
                    )
                })
                .collect();
            log::info!("party: {party:?}");
            drop(sim);
            let eye = follow
                .as_mut()
                .and_then(|f| l.follow_camera(f, 0.0, &camera));
            if let Some(c) = eye {
                camera = c;
            }
            // The capture's camera is the settled one, so its fade is settled too (a fade
            // reaches its opacity within a second).
            l.fade_near_camera(eye.map(|c| c.position), 1.0);
            if follow.is_some()
                && let Some(c) = l.conversation_camera(&camera)
            {
                camera = c;
            }
            // A playing cutscene's camera (see `cutcam`).
            if follow.is_some()
                && let Some(c) = l.cutscene_camera(&camera)
            {
                camera = c;
            }
            // Character creation frames its stage (see `chargen_stage`).
            let preset = played
                .as_ref()
                .map_or(0, |(layer, _)| layer.paperdoll_preset());
            if follow.is_some()
                && let Some(c) = l.chargen_camera(&camera, preset)
            {
                camera = c;
            }
            let popup = played
                .as_ref()
                .is_some_and(|(layer, _)| layer.stage.dialog_up());
            let ctx = pad_play.context_for(played.as_ref().map(|(_, s)| s), popup);
            pad_lines = (
                pad_play.overlay(Some(&*l), &camera, size, ctx),
                pad_play.pointer(cursor, ctx),
            );
        }
        if let (Some(a), Some(path)) = (recording.as_ref(), args.audio_wav.as_ref()) {
            let n = a.recorded.len().max(1) as f32;
            let peak = a.recorded.iter().fold(0.0f32, |m, s| m.max(s.abs()));
            let rms = (a.recorded.iter().map(|s| s * s).sum::<f32>() / n).sqrt();
            thedas_audio::wav::write_wav(
                std::io::BufWriter::new(std::fs::File::create(path)?),
                48000,
                2,
                &a.recorded,
            )?;
            println!(
                "audio: {} ({:.1}s, peak {:.1} dBFS, rms {:.1} dBFS)",
                path.display(),
                a.recorded.len() as f32 / 96000.0,
                20.0 * peak.max(1e-9).log10(),
                20.0 * rms.max(1e-9).log10()
            );
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
            cursor,
            args.time,
            pad_lines,
        );
    }

    let control = match &args.control {
        Some(addr) => {
            let c = control::Control::listen(addr)?;
            println!("control: listening on {}", c.addr);
            Some(c)
        }
        None => None,
    };
    // A test hook. THEDAS_HIDDEN_WINDOW: the window is never shown (and on macOS the app neither takes
    // the foreground nor shows in the Dock), for driving it with --control in tests; it
    // runs as an occluded window does, its frames drawn offscreen for screenshots.
    let event_loop = {
        let mut b = EventLoop::builder();
        #[cfg(target_os = "macos")]
        if std::env::var_os("THEDAS_HIDDEN_WINDOW").is_some() {
            use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
            b.with_activation_policy(ActivationPolicy::Prohibited)
                .with_activate_ignoring_other_apps(false);
        }
        b.build()?
    };
    // The window keeps presenting while a new level builds (a loading frame).
    if let Some(l) = live.as_mut() {
        l.async_loads = true;
    }
    let screens = (live.is_some() && ui.is_empty())
        .then(|| screens::Screens::new(live.as_ref().is_some_and(|l| l.front_end)));
    let loaded_vfs = loaded.vfs.clone();
    let mut app = App {
        load_screen_up: false,
        loaded,
        live,
        follow,
        screens,
        ui,
        saves_dir: saves_dir.clone(),
        cursor: (0.0, 0.0),
        camera,
        size,
        gfx: None,
        keys: HashSet::new(),
        looking: false,
        ui_right: false,
        last: Instant::now(),
        error: None,
        captures: 0,
        last_press: None,
        swallow_release: false,
        control,
        remote: VecDeque::new(),
        frame_requests: Vec::new(),
        pads: paddev::Pads::new(),
        pad: padplay::PadPlay::default(),
        remote_pad: pad::PadState::default(),
        install: install.clone(),
        module: args.module.clone(),
        shaders: args.shaders.clone(),
        quit: false,
        requests: Vec::new(),
        opening: None,
        // Clients driving the window (`--control`) get the start menu at once.
        intro: (boot && !args.no_intro && args.control.is_none())
            .then(|| intro::Intro::boot(&install))
            .flatten(),
        intro_pad_held: false,
        zoom_key: 0.0,
        audio: {
            let off = args.no_audio
                || std::env::var_os("THEDAS_NO_AUDIO").is_some()
                || (args.control.is_some() && !args.audio);
            if off {
                log::info!("audio: off");
                None
            } else {
                audio::GameAudio::open(&install, &args.module, &loaded_vfs, &args.language)
            }
        },
    };
    for line in pad::describe() {
        log::debug!("pad binding: {line}");
    }
    event_loop.run_app(&mut app)?;
    match app.error {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    /// Against a real install (`THEDAS_INSTALL`) and `ffmpeg`, offscreen: the EA logo's frame
    /// two seconds in is drawn letterboxed in a taller window (written to `$INTRO_PNG` when
    /// set, to look at).
    #[test]
    #[ignore]
    fn the_intro_draws_its_frame_letterboxed() {
        let install = PathBuf::from(std::env::var_os("THEDAS_INSTALL").expect("THEDAS_INSTALL"));
        let mut intro = intro::Intro::boot(&install).expect("ffmpeg and the movies");
        let ctx = GpuContext::headless().unwrap();
        let (w, h) = (1280, 960);
        let format = wgpu::TextureFormat::Bgra8Unorm;
        let tex = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = tex.create_view(&Default::default());
        let mut ms = 0.0;
        while ms < 2000.0 {
            assert!(intro.advance(1000.0 / 30.0));
            ms += 1000.0 / 30.0;
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let mut encoder = ctx.device.create_command_encoder(&Default::default());
        intro.draw(&ctx.device, &ctx.queue, &mut encoder, &view, format, (w, h));
        let readback = read_frame(&ctx, &mut encoder, &tex);
        ctx.queue.submit([encoder.finish()]);
        let frame = finish_read_frame(&ctx, readback, format).unwrap();
        if let Some(out) = std::env::var_os("INTRO_PNG") {
            write_png(Path::new(&out), w, h, &frame.pixels).unwrap();
        }
        let px = |x: u32, y: u32| &frame.pixels[((y * w + x) * 4) as usize..][..3];
        // 1280x720 in 1280x960: black bars of 120 above and below.
        assert_eq!(px(640, 10), [0, 0, 0]);
        assert_eq!(px(640, 950), [0, 0, 0]);
        let lit = (120..840)
            .step_by(8)
            .flat_map(|y| (0..w).step_by(8).map(move |x| (x, y)))
            .filter(|&(x, y)| px(x, y).iter().any(|&c| c > 16))
            .count();
        assert!(lit > 0, "the movie's frame is all black");
    }

    #[test]
    fn a_frame_without_a_surface_still_runs_the_game() {
        // Drawn and presented only with a frame; never reconfigured unless stale.
        let ready = frame_plan(SurfaceState::Ready, true);
        assert!(ready.draw && !ready.reconfigure && !ready.offscreen_frames);
        for s in [SurfaceState::Stale, SurfaceState::Unavailable] {
            let p = frame_plan(s, false);
            assert!(!p.draw && !p.offscreen_frames);
            // A client waiting for a frame is answered offscreen rather than left waiting.
            assert!(frame_plan(s, true).offscreen_frames);
        }
        assert!(frame_plan(SurfaceState::Stale, false).reconfigure);
        assert!(!frame_plan(SurfaceState::Unavailable, false).reconfigure);
    }

    use super::*;

    #[test]
    fn shrinks_frames_by_averaging_blocks() {
        let img = thedas_ui::Rgba {
            width: 4,
            height: 2,
            pixels: [[0, 0, 0, 255], [200, 100, 0, 255]].repeat(4).concat(),
        };
        let small = shrink(&img, 2);
        assert_eq!((small.width, small.height), (2, 1));
        assert_eq!(&small.pixels[..4], &[100, 50, 0, 255]);
        assert_eq!(shrink(&img, 1).pixels, img.pixels);
    }

    #[test]
    fn parses_steps() {
        assert_eq!(
            parse_steps("click 10,20.5; run 300 ;key i;").unwrap(),
            vec![
                Step::Click(10.0, 20.5),
                Step::Run(300),
                Step::Menu(screens::Screen::Inventory)
            ]
        );
        assert_eq!(
            parse_steps("key c; key t; key m").unwrap(),
            vec![
                Step::Menu(screens::Screen::CharacterRecord),
                Step::Menu(screens::Screen::Tactics),
                Step::Menu(screens::Screen::AreaMap)
            ]
        );
        assert_eq!(
            parse_steps("talk lot110cr_bryant").unwrap(),
            vec![Step::Talk("lot110cr_bryant".into())]
        );
        assert_eq!(parse_steps("rebuild").unwrap(), vec![Step::Rebuild]);
        assert_eq!(
            parse_steps("type Rowan Cousland; key backspace; key ctrl+a; key Enter").unwrap(),
            vec![
                Step::Type("Rowan Cousland".into()),
                Step::TextKey(thedas_ui::UiKey::Backspace),
                Step::TextKey(thedas_ui::UiKey::SelectAll),
                Step::TextKey(thedas_ui::UiKey::Enter),
            ]
        );
        assert_eq!(
            parse_steps("hover 5,6; key F4").unwrap(),
            vec![Step::Hover(5.0, 6.0), Step::Leader(3)]
        );
        assert_eq!(
            parse_steps("rclick 1,2; dclick 3,4").unwrap(),
            vec![Step::ClickAs(2.0, 1.0, 2.0), Step::ClickAs(5.0, 3.0, 4.0)]
        );
        assert_eq!(
            parse_steps("key 1; key 0; key Space; reply 2").unwrap(),
            vec![
                Step::Quickslot(0),
                Step::Quickslot(9),
                Step::Pause,
                Step::Reply(2)
            ]
        );
        assert_eq!(
            parse_steps("orbit 30,-10; orbit 15; zoom -2; key q; key E").unwrap(),
            vec![
                Step::Orbit(30.0, -10.0),
                Step::Orbit(15.0, 0.0),
                Step::Zoom(-2.0),
                Step::Orbit(45.0, 0.0),
                Step::Orbit(-45.0, 0.0)
            ]
        );
        assert!(parse_steps("zoom far").is_err());
        assert_eq!(
            parse_steps("press 1,2; release 3,4").unwrap(),
            vec![Step::Press(1.0, 2.0), Step::Release(3.0, 4.0)]
        );
        assert_eq!(
            parse_steps("pad a; pad press RT; pad release rt; pad stick-left 0,1.5 500").unwrap(),
            vec![
                Step::PadButton(pad::Button::A, true),
                Step::PadButton(pad::Button::A, false),
                Step::PadButton(pad::Button::RT, true),
                Step::PadButton(pad::Button::RT, false),
                Step::PadStick(pad::Stick::Left, 0.0, 1.0),
                Step::Run(500),
                Step::PadStick(pad::Stick::Left, 0.0, 0.0),
            ]
        );
        assert!(parse_steps("pad q").is_err());
        assert!(parse_steps("pad stick-left 0,1").is_err());
        assert!(parse_steps("pad").is_err());
        assert!(parse_steps("reply x").is_err());
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
