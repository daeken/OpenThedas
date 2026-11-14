//! `thedas-app`: play Dragon Age: Origins, or walk through an area.
//!
//! With only `--install`, the game boots as the original does: the EA logo and intro movies
//! (decoded by `thedas-video`, with their sound; a key, a click or the pad's A, B or Start skips
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
//! F8 (or both sticks clicked in, or the game menu's Report Issue) makes an issue report
//! (see [`report`]); a build that fixes one is offered at start (see [`update`]).
//!
//! `--control 127.0.0.1:7878` lets a local client (`thedas-mcp`'s `attach`) drive the
//! running game: the same steps as `--steps`, screenshots and the simulation's state (see
//! [`control`]).

mod audio;
mod camfade;
mod chargen_stage;
mod control;
mod cursor;
mod cutactors;
mod cutcam;
mod faces;
mod focusnav;
mod follow;
mod framestats;
mod glb;
mod glb_in;
mod icon;
mod intro;
mod journal;
mod launcher;
mod launcher_download;
mod live;
mod looks;
mod modconv;
mod modmode;
mod modworld;
mod occlusion;
mod pad;
mod paddev;
mod padplay;
mod report;
mod reticle;
mod rings;
mod saves;
mod screens;
mod talkcam;
mod ui;
mod update;
mod upload;
mod workspace;

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
    /// The DLC keys file (the format of the commonly shared `erfpasswords.txt`). Defaults to
    /// $THEDAS_KEYS, then `keys.txt` in the player's data folder.
    #[arg(long)]
    keys: Option<PathBuf>,
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
    /// Resume the game from the journal (the data folder's `journal/`: the last checkpoint,
    /// and every input since it replayed), as it was when it stopped.
    #[arg(long)]
    resume: bool,
    /// Keep the journal even in a run with `--capture`, `--steps` or `--control` (which
    /// keep none otherwise, so tests never replace the player's journal).
    #[arg(long)]
    journal: bool,
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
    /// Module for `--save` and `--objects`; the install's base campaign when not given
    /// (`single player`, or `campaign_base` for Dragon Age II).
    #[arg(long, default_value = "")]
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
    /// The player's settings file (the options screen's settings, kept between runs).
    /// Playing in a window without `--control` it defaults to `options.ini` in the
    /// player's folder (`~/.openthedas`; `Documents\OpenThedas` on Windows); otherwise
    /// settings are kept only when this is given.
    #[arg(long)]
    settings: Option<PathBuf>,
    /// List the game controllers gilrs sees (names, ids, mapping, which inputs it maps) and
    /// print their events for this many seconds, then exit. No window, no install needed.
    #[arg(long)]
    pad_probe: Option<f64>,
    /// Mod mode's live edits from the start: the workspace folder (`workspace/override` in
    /// the player's data folder) mounted above every mod (see `workspace`).
    #[arg(long)]
    mod_workspace: bool,
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
    /// Ask whether the problem reported in this report folder is fixed, once the game is up
    /// (a build started by the update after a fix, see `update`).
    #[arg(long)]
    check_report: Option<PathBuf>,
    /// The voice-over language folder (`audio/vo/<language>`).
    #[arg(long, default_value = "en-us")]
    language: String,
    /// Show the launcher (installs, mods, play options) even when an install is given or the
    /// player chose to skip it. Started with no install and none of the headless options, the
    /// game shows it anyway.
    #[arg(long)]
    launcher: bool,
    /// Draw a launcher page (`welcome`, `search`, `keys`, `play`, `games`, `mods`,
    /// `settings`; see `--launcher-page`) to this PNG offscreen at `--size`, then exit.
    #[arg(long, hide = true)]
    launcher_capture: Option<PathBuf>,
    #[arg(long, hide = true, default_value = "play")]
    launcher_page: String,
    /// Pixels per point for `--launcher-capture`.
    #[arg(long, hide = true, default_value_t = 1.0)]
    launcher_scale: f32,
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
    let mut vfs = install.play_vfs(TextureQuality::High)?;
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
    /// The window's own title, before the frame-rate figures (see [`framestats`]).
    title: String,
    frames: framestats::FrameStats,
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
    /// The player's settings file, when settings are kept (see `Args::settings`).
    settings: Option<PathBuf>,
    cursor: (f64, f64),
    /// The last check of whether the interface covers the cursor while it is over a
    /// creature: (cursor, creature) → covered (see `App::update_hover`).
    hover_probe: Option<((f64, f64), thedas_script::vm::ObjectId, bool)>,
    /// The highlight key held by an `allnames` step (see `Step::AllNames`).
    all_names: bool,
    /// The keyboard's movement keys were walking the leader last frame (see
    /// `App::key_walk`).
    key_walking: bool,
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
    /// The game's mouse cursors (none before the window is up).
    cursors: Option<cursor::Cursors>,
    /// The left button is down.
    left_down: bool,
    /// Whether the interface covers the cursor, as last read back: the cursor's place and
    /// what was under it in the world then.
    cursor_probe: Option<((f64, f64), cursor::Over, bool)>,
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
    /// Issue reports (F8): the snapshots, input and frame times they keep (see [`report`]).
    recorder: report::Recorder,
    /// A report under way: its screenshot and the player's note, to come, and the client
    /// that asked for it (`--control`'s `report`), waiting for where it went.
    report_wait: Option<ReportWait>,
    /// The settings the game was started with, for reports.
    started_with: J,
    /// The way back from a report: a fix offered, fetched and checked (see [`update`]).
    returning: Returning,
    /// Answers to its questions, taken from the interface, acted on next frame.
    update_answers: Vec<ui::TextAnswer>,
    /// Answers about marking spots for a report, acted on next frame.
    mark_answers: Vec<ui::TextAnswer>,
    /// The start menu's offer to resume a session that ended unexpectedly (see
    /// [`App::resume_frame`]).
    resume_offer: ResumeOffer,
    /// Mod mode's panels over the game (see [`modmode`]), made the first time it opens.
    mod_mode: Option<modmode::ModMode>,
    /// Mod mode's live edits: when the exports were last looked at for saved textures.
    live_edits: LiveEdits,
    /// The last input came from a game controller (not the mouse or keyboard): a report
    /// then skips marking spots, which needs the mouse.
    pad_last: bool,
}

/// Where the way back from a report is (see [`update`]).
enum Returning {
    Idle,
    /// Looking for a build that fixes a report sent from here.
    Checking(std::sync::mpsc::Receiver<Option<update::Offer>>),
    /// A question up, or waiting to go up.
    Asking(Box<UpdatePrompt>),
    /// The build being fetched; the save it starts on and the report it asks about.
    Fetching(std::sync::mpsc::Receiver<Result<PathBuf>>, PathBuf, PathBuf),
}

/// A question of the way back, in the game's text box.
struct UpdatePrompt {
    kind: UpdateAsk,
    /// Frames since asked and times it failed to show (as the report's note).
    asked: Option<(u32, u32)>,
}

enum UpdateAsk {
    /// A build fixes a report: update now?
    Offer(update::Offer),
    /// Load where the player is, or where they reported it?
    Where(update::Offer),
    /// The build started on a report: is it fixed? (The report's folder, the question.)
    Verdict(PathBuf, String),
    /// Its answer given (fixed or not): anything to add? (A text box, whose first button
    /// wants text; with none the second, "No note", sends the answer as it is.)
    VerdictNote(PathBuf, bool),
    /// Something went wrong.
    Failed(String),
}

impl UpdateAsk {
    /// Its text box's id, prompt and buttons.
    fn ask(&self) -> (u32, String, &'static str, &'static str) {
        match self {
            UpdateAsk::Offer(o) => (UPDATE_ASK_ID, update::offer_prompt(o), "Update", "Later"),
            UpdateAsk::Where(_) => (
                UPDATE_ASK_ID + 1,
                "Start the new build where you are now, or where you reported the problem?"
                    .to_string(),
                "Where I am",
                "Where I reported",
            ),
            UpdateAsk::Verdict(_, prompt) => {
                (UPDATE_ASK_ID + 2, prompt.clone(), "Fixed", "Not fixed")
            }
            UpdateAsk::Failed(why) => (UPDATE_ASK_ID + 3, why.clone(), "OK", "Close"),
            UpdateAsk::VerdictNote(..) => (
                UPDATE_ASK_ID + 4,
                "Thanks. Anything to add for us? It goes with your answer.".to_string(),
                "Send note",
                "No note",
            ),
        }
    }
}

/// The ids the way back's questions go by (`UiLayer::ask_text`).
const UPDATE_ASK_ID: u32 = 0x5550_4400;

/// Frames a report's text box may take to come up before it is asked again.
const NOTE_REASK_FRAMES: u32 = 30;
/// How many times it may fail to come up before the report is kept without a note.
const NOTE_TRIES: u32 = 10;

/// Asks for a report's note in the game's own text box: Send when reports go to the
/// project's Discord (see [`upload`]), else Save. Either button keeps the report: an
/// accidental one costs a folder, a lost one costs the bug.
fn ask_report_note(ui: &mut ui::UiLayer, pad: bool) {
    let (prompt, ok, other) = match upload::webhook() {
        Some(_) => (report::NOTE_PROMPT_SEND, "Send report", "Keep here"),
        None => (report::NOTE_PROMPT, "Save report", "Save, no note"),
    };
    // The box's OK wants some text, and the pad cannot type yet: a pad's report starts
    // with a line that can go as it is.
    let text = if pad { "(sent from a controller)" } else { "" };
    ui.ask_text(report::NOTE_ID, prompt, text, ok, other);
}

/// What a report under way waits for.
struct ReportWait {
    shot: Option<std::sync::mpsc::Receiver<J>>,
    reply: Option<std::sync::mpsc::Sender<J>>,
    /// The note is asked in the game's text box: frames since it was last asked (or seen
    /// up), and how many times it has not come up.
    note_asked: Option<(u32, u32)>,
    /// The report goes on to the project's Discord once written, if set up (see
    /// [`upload`]); the text box's second button keeps it here.
    send: bool,
    /// The game is to pause once the screenshot is in (so the frame shows no "Paused"),
    /// and whether the report paused it (it carries on when the report is done).
    pause: bool,
    paused_it: bool,
    /// Marking spots on the screen before the note (see [`Marking`]).
    marking: Marking,
}

/// Marking spots on the screen for a report: offered first (a choice), then boxes dragged
/// with the mouse, each asked for its note, until F8, Enter or Escape; the note is asked
/// after. Ours, not the original's.
#[derive(Clone, Debug, Default, PartialEq)]
enum Marking {
    #[default]
    Off,
    /// The choice is asked: frames since asked (or seen up), and times it did not show.
    Offered(u32, u32),
    /// Boxes are being drawn: the one being dragged (from, to), and the one whose note
    /// is being asked, with frames and tries as for the choice.
    On {
        drag: Option<(glam::Vec2, glam::Vec2)>,
        asking: Option<([f32; 4], u32, u32)>,
    },
}

/// The start menu's offer to resume a session that ended unexpectedly: made at most once a
/// run, when the start menu first comes up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ResumeOffer {
    Unchecked,
    Asked,
    Done,
}

/// The ids of the offer to resume and of the box saying it failed.
const RESUME_ID: u32 = 0x5245_5355;
const RESUME_FAILED_ID: u32 = RESUME_ID + 1;

/// When `written` was, said as how long ago from `now`.
fn time_ago(written: std::time::SystemTime, now: std::time::SystemTime) -> String {
    let s = now.duration_since(written).map_or(0, |d| d.as_secs());
    match s {
        0..60 => "moments ago".to_string(),
        60..120 => "a minute ago".to_string(),
        120..3600 => format!("{} minutes ago", s / 60),
        3600..7200 => "an hour ago".to_string(),
        7200..86_400 => format!("{} hours ago", s / 3600),
        86_400..172_800 => "yesterday".to_string(),
        _ => format!("{} days ago", s / 86_400),
    }
}

/// The newest of the saves' files in `dir` (when it was written), if there are any.
fn newest_save(dir: &std::path::Path) -> Option<std::time::SystemTime> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().ends_with(".meta.json"))
        .filter_map(|e| e.metadata().ok()?.modified().ok())
        .max()
}

/// Smallest box (pixels a side) taken as a marked spot; smaller is a stray click.
const MIN_MARK: f32 = 8.0;

/// What the question whether to mark spots says to a player on the pad.
const MARK_PROMPT_PAD: &str = "Want to point at something? Choose Mark spots, then move the \
     pointer with the left stick, press A at one corner of the thing that is wrong and A again \
     at the other, and say what is wrong there. Press B or Start when done.";

/// The question whether to mark spots, worded for the pad when it was used last.
fn mark_prompt(pad: bool) -> &'static str {
    if pad {
        MARK_PROMPT_PAD
    } else {
        report::MARK_PROMPT
    }
}

fn ask_mark_choice(ui: &mut ui::UiLayer, pad: bool) {
    ui.ask_choice(
        report::MARK_ID,
        mark_prompt(pad),
        "Mark spots",
        "Just a note",
    );
}

fn ask_region_note(ui: &mut ui::UiLayer) {
    ui.ask_text(
        report::REGION_ID,
        report::REGION_PROMPT,
        "",
        "Add note",
        "Remove box",
    );
}

/// Whether the game's message box shows `prompt` now.
fn box_shows(ui: &ui::UiLayer, prompt: &str) -> bool {
    ui.stage.dialog_up()
        && matches!(
            ui.stage.bridge.value("GUIPopupLayer.DialogBoxText"),
            Some(thedas_ui::UiValue::String(t)) if t == prompt
        )
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
            // A script's DoAutoSave, made between frames as every save is.
            let autosave = live.sim().take_autosave_request();
            if autosave {
                ui.auto_save(&live.sim);
            }
            ui.update_minimap(&live.sim, camera);
            ui.update_floaties(&live.sim, camera, dt.min(100.0), &|c| live.head_height(c));
        }
        _ => {
            for call in ui.tick(dt.min(100.0)) {
                log::debug!("ui call: {call:?}");
            }
        }
    }
}

/// When mod mode's live edits last looked for saved textures (see [`workspace`]).
#[derive(Default)]
struct LiveEdits {
    last: Option<Instant>,
}

/// How often the exports are looked at for saved textures while live edits are on.
const LIVE_EDIT_POLL: std::time::Duration = std::time::Duration::from_millis(500);

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

/// `frame` with `paint` drawn over it on the GPU (in the window's `format`), read back.
fn paint_over(
    ctx: &GpuContext,
    format: wgpu::TextureFormat,
    frame: thedas_ui::Rgba,
    paint: impl FnOnce(&mut wgpu::CommandEncoder, &wgpu::TextureView),
) -> Result<thedas_ui::Rgba> {
    let (width, height) = (frame.width, frame.height);
    let size = wgpu::Extent3d {
        width,
        height,
        depth_or_array_layers: 1,
    };
    let texture = ctx.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("offscreen overlay"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let mut pixels = frame.pixels;
    if matches!(
        format,
        wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
    ) {
        pixels.chunks_exact_mut(4).for_each(|p| p.swap(0, 2));
    }
    ctx.queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &pixels,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width * 4),
            rows_per_image: Some(height),
        },
        size,
    );
    let view = texture.create_view(&Default::default());
    let mut encoder = ctx.device.create_command_encoder(&Default::default());
    paint(&mut encoder, &view);
    let readback = read_frame(ctx, &mut encoder, &texture);
    ctx.queue.submit([encoder.finish()]);
    finish_read_frame(ctx, readback, format)
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
        let title = match &self.loaded.scene {
            Some(s) => format!(
                "OpenThedas - {} ({})",
                s.name.as_deref().unwrap_or(""),
                s.level
            ),
            None => "OpenThedas".to_string(),
        };
        let window = Arc::new(
            event_loop.create_window(
                Window::default_attributes()
                    .with_title(title.clone())
                    .with_inner_size(LogicalSize::new(self.size.0, self.size.1))
                    .with_window_icon(icon::window_icon())
                    .with_visible(std::env::var_os("THEDAS_HIDDEN_WINDOW").is_none()),
            )?,
        );
        if std::env::var_os("THEDAS_HIDDEN_WINDOW").is_none() {
            icon::set_dock_icon();
        }
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
        if let Some(path) = &self.settings {
            ui.set_settings_file(path.clone());
        }
        if let (Some(live), Some(_)) = (&self.live, &self.screens) {
            ui.attach(&live.sim);
            ui.set_saves(saves::SaveStore::new(self.saves_dir.clone()));
        }
        let reticle = reticle::ReticlePass::new(&ctx.device, config.format);
        self.cursors = Some(cursor::Cursors::load(event_loop, &self.install));
        self.gfx = Some(Gfx {
            window,
            surface,
            config,
            ctx,
            renderer,
            ui,
            reticle,
            title,
            frames: framestats::FrameStats::default(),
        });
        Ok(())
    }

    /// The keyboard's movement keys walk the leader while they are held: W and S forward
    /// and back, Q and E to the left and right (keymap.gda's `RunCharacterForward`,
    /// `RunCharacterBackward`, `RunCharacterLeft` and `RunCharacterRight`), relative to the
    /// camera, as the pad's left stick does; letting go stops. Provisional: the keys are
    /// fixed (the player's bindings are not read), and the walk is the pad's (a move order a
    /// little ahead each frame), not the exe's own keyboard movement, which is not traced.
    fn key_walk(&mut self) {
        let k = |c| self.keys.contains(&c);
        let typing = self.gfx.as_mut().is_some_and(|g| g.ui.stage.text_focused());
        let v = if self.follow.is_some() && self.live.is_some() && !typing {
            glam::Vec2::new(
                (k(KeyCode::KeyE) as i32 - k(KeyCode::KeyQ) as i32) as f32,
                (k(KeyCode::KeyW) as i32 - k(KeyCode::KeyS) as i32) as f32,
            )
        } else {
            glam::Vec2::ZERO
        };
        if v == glam::Vec2::ZERO && !self.key_walking {
            return;
        }
        self.key_walking = v != glam::Vec2::ZERO;
        let pad = std::mem::take(&mut self.pad);
        pad.walk_keys(self, v);
        self.pad = pad;
    }

    fn update(&mut self) {
        let now = Instant::now();
        let dt = (now - self.last).as_secs_f32().min(0.1);
        self.last = now;
        self.key_walk();
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
            // A and D turn the camera round the leader: keymap.gda's `MoveCameraLeft` and
            // `MoveCameraRight`. Provisional: the rate.
            let turn = (k(KeyCode::KeyA) as i32 - k(KeyCode::KeyD) as i32) as f32;
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
                l.sim().input(
                    thedas_sim::commands::CommandSource::Player,
                    thedas_sim::commands::SimCommand::PlayerZoomedIn,
                );
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
        if let Some(g) = self.gfx.as_mut() {
            if let Some(stats) = g.frames.frame(Instant::now()) {
                g.window.set_title(&format!("{} | {stats}", g.title));
                if g.frames.log_due() {
                    log::info!("frame rate: {stats}");
                }
            }
        }
        self.audio_frame();
        if self.intro.is_some() {
            return self.intro_frame();
        }
        let dt = self.last.elapsed().as_secs_f64() * 1000.0;
        self.serve_control(dt);
        self.pad_frame(dt);
        self.report_frame(dt);
        self.resume_frame();
        self.mod_mode_frame(dt);
        self.returning_frame();
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
            draw_likenesses(live, &mut g.ui, self.screens.as_ref(), &g.ctx, dt);
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
        let hovered = self.hovered_creature();
        if let Some(live) = self.live.as_mut() {
            live.set_hovered(hovered);
        }
        let voiced = self.line_voiced();
        let marks = self.marking_lines();
        self.show_cursor();
        let Some(g) = self.gfx.as_mut() else {
            return Ok(());
        };
        g.ui.set_hovered(hovered);
        g.ui.set_line_voiced(voiced);
        // Tab held shows every creature's name (never while it is typed into a field):
        // keymap.gda binds `HighlightObjects` to Tab.
        let tab = self.all_names
            || self.pad.highlight
            || (self.keys.contains(&KeyCode::Tab) && !g.ui.stage.dialog_up());
        g.ui.set_show_all_names(tab);
        play_interface(
            &mut g.ui,
            self.screens.as_mut(),
            self.live.as_mut(),
            &self.camera,
            dt,
        );
        self.requests.append(&mut g.ui.requests);
        // A save just written gets the picture of the game as it stands (the world alone,
        // as the save and load screens show it).
        let saved = g.ui.take_saved_slots();
        if !saved.is_empty() {
            let (w, h) = (g.config.width, g.config.height);
            let shot = self
                .live
                .as_mut()
                .and_then(|l| l.capture(&g.ctx, &self.camera, (w, h)).ok().flatten())
                .and_then(|px| saves::shot_png(&px, w, h));
            if let Some(png) = shot {
                for slot in saved {
                    g.ui.set_save_screenshot(slot, &png);
                }
            }
        }
        for a in g.ui.take_text_answers() {
            // OK sends (or saves) the report with the note; the other button keeps it on
            // this computer only, unsent.
            if (UPDATE_ASK_ID..UPDATE_ASK_ID + 5).contains(&a.id) {
                self.update_answers.push(a);
                continue;
            }
            if (a.id == report::MARK_ID || a.id == report::REGION_ID) && self.recorder.busy() {
                self.mark_answers.push(a);
                continue;
            }
            // The offer to resume: Resume opens the kept game; Not now leaves the journal
            // for next time.
            if a.id == RESUME_ID {
                self.resume_offer = ResumeOffer::Done;
                if a.accepted {
                    g.ui.requests.push(ui::FrontEnd::Resume);
                }
                continue;
            }
            // The resume failed: "Set it aside" moves the journal out of the way.
            if a.id == RESUME_FAILED_ID {
                if a.accepted {
                    if let Err(e) = journal::discard(&journal::default_dir()) {
                        log::warn!("resume: setting the journal aside: {e}");
                    }
                }
                continue;
            }
            if a.id == report::NOTE_ID && self.recorder.busy() {
                if let Some(w) = self.report_wait.as_mut() {
                    w.note_asked = None;
                    w.send = a.accepted;
                }
                self.recorder.note(Some(a.text));
                continue;
            }
            log::info!(
                "text box {}: {} {:?}",
                a.id,
                if a.accepted { "OK" } else { "Cancel" },
                a.text
            );
        }
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
        let mut pointer = self.pad.pointer(self.cursor, ctx);
        pointer.extend(marks);
        g.reticle
            .draw(&g.ctx.device, &mut encoder, &view, size, &pointer);
        // Mod mode's panels go over everything, the interface included.
        if let Some(m) = self.mod_mode.as_mut() {
            m.paint(
                &g.ctx.device,
                &g.ctx.queue,
                &mut encoder,
                &view,
                [g.config.width, g.config.height],
            );
        }
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
        let mut state = audio_state(a, live, ui, &self.camera, movie);
        state.gui = self.screens.as_ref().is_some_and(|s| s.world_stopped());
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
            // The party's portraits and the paperdoll are drawn into their own targets, so
            // they are kept up with no frame to present too (a hidden window's portraits
            // stayed empty, and a client's offscreen frame showed them so).
            draw_likenesses(live, &mut g.ui, self.screens.as_ref(), &g.ctx, dt);
        }
        if self.live.is_none() {
            if let Some(r) = g.renderer.as_mut() {
                r.advance_time(dt / 1000.0);
            }
        }
        g.ui.set_pad_active(self.pad.used);
        let hovered = self.hovered_creature();
        if let Some(live) = self.live.as_mut() {
            live.set_hovered(hovered);
        }
        let voiced = self.line_voiced();
        self.show_cursor();
        let Some(g) = self.gfx.as_mut() else {
            return Ok(());
        };
        g.ui.set_hovered(hovered);
        g.ui.set_line_voiced(voiced);
        // Tab held shows every creature's name (never while it is typed into a field):
        // keymap.gda binds `HighlightObjects` to Tab.
        let tab = self.all_names
            || self.pad.highlight
            || (self.keys.contains(&KeyCode::Tab) && !g.ui.stage.dialog_up());
        g.ui.set_show_all_names(tab);
        play_interface(
            &mut g.ui,
            self.screens.as_mut(),
            self.live.as_mut(),
            &self.camera,
            dt,
        );
        self.requests.append(&mut g.ui.requests);
        // A save just written gets the picture of the game as it stands (the world alone,
        // as the save and load screens show it).
        let saved = g.ui.take_saved_slots();
        if !saved.is_empty() {
            let (w, h) = (g.config.width, g.config.height);
            let shot = self
                .live
                .as_mut()
                .and_then(|l| l.capture(&g.ctx, &self.camera, (w, h)).ok().flatten())
                .and_then(|px| saves::shot_png(&px, w, h));
            if let Some(png) = shot {
                for slot in saved {
                    g.ui.set_save_screenshot(slot, &png);
                }
            }
        }
        for a in g.ui.take_text_answers() {
            // OK sends (or saves) the report with the note; the other button keeps it on
            // this computer only, unsent.
            if (UPDATE_ASK_ID..UPDATE_ASK_ID + 5).contains(&a.id) {
                self.update_answers.push(a);
                continue;
            }
            if (a.id == report::MARK_ID || a.id == report::REGION_ID) && self.recorder.busy() {
                self.mark_answers.push(a);
                continue;
            }
            // The offer to resume: Resume opens the kept game; Not now leaves the journal
            // for next time.
            if a.id == RESUME_ID {
                self.resume_offer = ResumeOffer::Done;
                if a.accepted {
                    g.ui.requests.push(ui::FrontEnd::Resume);
                }
                continue;
            }
            // The resume failed: "Set it aside" moves the journal out of the way.
            if a.id == RESUME_FAILED_ID {
                if a.accepted {
                    if let Err(e) = journal::discard(&journal::default_dir()) {
                        log::warn!("resume: setting the journal aside: {e}");
                    }
                }
                continue;
            }
            if a.id == report::NOTE_ID && self.recorder.busy() {
                if let Some(w) = self.report_wait.as_mut() {
                    w.note_asked = None;
                    w.send = a.accepted;
                }
                self.recorder.note(Some(a.text));
                continue;
            }
            log::info!(
                "text box {}: {} {:?}",
                a.id,
                if a.accepted { "OK" } else { "Cancel" },
                a.text
            );
        }
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

    /// Opens or closes mod mode's panels (F10, and the pause menu's entry).
    pub fn toggle_mod_mode(&mut self) {
        if self.mod_mode.is_none() {
            let Some(g) = self.gfx.as_ref() else {
                return;
            };
            self.mod_mode = Some(modmode::ModMode::new(
                &g.window,
                &g.ctx.device,
                g.config.format,
            ));
        }
        if let Some(m) = self.mod_mode.as_mut() {
            m.toggle();
            log::info!("mod mode {}", if m.open { "open" } else { "closed" });
            if !m.open
                && let Some(l) = self.live.as_mut()
            {
                l.set_scrub(None);
            }
        }
    }

    /// Whether mod mode's panels are up.
    pub fn mod_mode_up(&self) -> bool {
        self.mod_mode.as_ref().is_some_and(|m| m.open)
    }

    /// Mod mode's frame: its panels laid out from the picked object, the scrubber's hold
    /// passed to the game's drawing, and what the panels asked for done.
    /// Live edits: textures saved over an export's PNGs are written into the workspace and
    /// the objects drawn again with them. Says what happened (`None`: nothing to do).
    fn live_edit_sync(&mut self) -> Option<String> {
        if !workspace::mounted() {
            return None;
        }
        let overrides = workspace::override_dir();
        let changes = workspace::changes(&workspace::blender_dir(), &overrides);
        let glbs = workspace::glb_changes(&workspace::blender_dir());
        if changes.is_empty() && glbs.is_empty() {
            return None;
        }
        // Each texture written in its original's format.
        let stack = self.live.as_ref().map(|l| l.sim().vfs.clone());
        let format_of = |name: &str| {
            let (data, _) = stack.as_ref()?.read_beneath_workspace(name).ok()?;
            Some(thedas_formats::dds::Dds::parse(&data).ok()?.format)
        };
        let (mut done, mut failed) = workspace::apply(&changes, &overrides, &format_of);
        // Objects moved in a saved file, moved in the world after the meshes.
        let mut moves = Vec::new();
        // Saved meshes: read against the game's own files, not earlier live edits.
        if !glbs.is_empty() {
            let vfs = self.live.as_ref().map(|l| l.sim().vfs.clone());
            let read = |name: &str| {
                let (data, p) = vfs.as_ref()?.read_beneath_workspace(name).ok()?;
                Some(glb::Read {
                    data,
                    layer: p.layer_label,
                    entry: p.entry,
                })
            };
            for g in &glbs {
                match workspace::import_glb(g, &overrides, &read, &format_of) {
                    Ok(r) => {
                        for n in &r.notes {
                            log::info!("live edits: {}: {n}", g.display());
                        }
                        done.extend(r.written);
                        if let Some(m) = r.moved {
                            moves.push(m);
                        }
                    }
                    Err(e) => failed.push(format!("{}: {e}", g.display())),
                }
            }
        }
        for f in &failed {
            log::warn!("live edits: {f}");
        }
        if !done.is_empty() {
            thedas_vfs::Vfs::rescan_workspace();
            if let Some(l) = self.live.as_mut() {
                l.reload(&done);
            }
        }
        let mut placed = Vec::new();
        for (id, tag, p) in moves {
            // The exported object, if it is still that one; else the one with its tag here.
            let target = self.live.as_ref().and_then(|l| {
                let sim = l.sim();
                match sim.world.get(id) {
                    Some(o) if o.tag.eq_ignore_ascii_case(&tag) => Some(id),
                    _ => sim.world.find_tag(&tag, 0),
                }
            });
            match target {
                Some(t) => {
                    let n = self.place_object(
                        thedas_sim::commands::SimCommand::MoveObject {
                            target: t,
                            position: p.position,
                            facing: p.facing,
                        },
                        format!("move {tag} (from Blender)"),
                    );
                    placed.push(n);
                }
                None => failed.push(format!("{tag}: not in the world to move")),
            }
        }
        let mut said = Vec::new();
        if !done.is_empty() {
            said.push(format!("{} updated", done.join(", ")));
        }
        said.extend(placed);
        if said.is_empty() {
            said.push("nothing written".into());
        }
        let note = format!(
            "Live edits: {}{}",
            said.join("; "),
            if failed.is_empty() {
                String::new()
            } else {
                format!("; {} failed (see the log)", failed.len())
            }
        );
        log::info!("{note}");
        Some(note)
    }

    fn mod_mode_frame(&mut self, dt_ms: f64) {
        let due = self
            .live_edits
            .last
            .is_none_or(|t| t.elapsed() >= LIVE_EDIT_POLL);
        if due && workspace::mounted() {
            self.live_edits.last = Some(Instant::now());
            if let Some(note) = self.live_edit_sync()
                && let Some(m) = self.mod_mode.as_mut()
            {
                m.live_note = Some(note);
            }
        }
        let flying = self.live.is_some() && self.follow.is_none();
        let Some(m) = self.mod_mode.as_mut() else {
            return;
        };
        m.fly = flying;
        m.live = workspace::mounted();
        let inspected = match (m.open.then_some(m.selected).flatten(), self.live.as_mut()) {
            (Some(id), Some(l)) => l.inspect(id),
            _ => None,
        };
        let Some(g) = self.gfx.as_ref() else {
            return;
        };
        let actions = m.frame(&g.window, inspected.as_ref(), (dt_ms / 1000.0) as f32);
        let hold = m.scrub.as_ref().map(|s| (s.id, s.time));
        if let Some(l) = self.live.as_mut() {
            l.set_scrub(hold);
        }
        for a in actions {
            match a {
                modmode::Action::Report(note) => self.report(Some(note), None),
                modmode::Action::ListActions(id) => {
                    let list = self
                        .live
                        .as_mut()
                        .map(|l| l.creature_actions(id))
                        .unwrap_or_default();
                    if let Some(m) = self.mod_mode.as_mut() {
                        m.actions_list = Some((id, list));
                    }
                }
                modmode::Action::PlayAnimation { id, action } => {
                    let played = self
                        .live
                        .as_mut()
                        .is_some_and(|l| l.play_action(id, &action));
                    if !played && let Some(m) = self.mod_mode.as_mut() {
                        m.scrub = None;
                    }
                }
                modmode::Action::Conv(a) => self.mod_mode_conv(a),
                modmode::Action::World(a) => self.mod_mode_world(a),
                modmode::Action::Fly(on) => self.set_fly(on),
                modmode::Action::FlyTo(id) => self.fly_to(id),
                modmode::Action::LiveEdits(on) => {
                    let note = workspace::mount(on).unwrap_or_else(|e| e);
                    if let Some(m) = self.mod_mode.as_mut() {
                        m.live_note = Some(note);
                    }
                }
                modmode::Action::Move {
                    id,
                    position,
                    facing,
                } => {
                    self.mod_mode_place(
                        thedas_sim::commands::SimCommand::MoveObject {
                            target: id,
                            position,
                            facing,
                        },
                        format!("move {id}"),
                    );
                }
                modmode::Action::MoveToView(id) => {
                    if let Some(at) = self.view_point() {
                        let facing = self
                            .live
                            .as_ref()
                            .and_then(|l| l.sim().world.get(id).map(|o| o.facing))
                            .unwrap_or(0.0);
                        self.mod_mode_place(
                            thedas_sim::commands::SimCommand::MoveObject {
                                target: id,
                                position: at,
                                facing,
                            },
                            format!("move {id} to the view"),
                        );
                    }
                }
                modmode::Action::Spawn { creature, template } => {
                    if let Some(at) = self.view_point() {
                        // Facing the camera.
                        let facing = (self.camera.yaw.to_degrees() + 90.0).rem_euclid(360.0);
                        let kind = if creature {
                            thedas_sim::world::ObjectKind::Creature
                        } else {
                            thedas_sim::world::ObjectKind::Placeable
                        };
                        self.mod_mode_place(
                            thedas_sim::commands::SimCommand::CreateObject {
                                kind,
                                template: template.clone(),
                                position: at,
                                facing,
                            },
                            format!("spawn {template}"),
                        );
                    }
                }
                modmode::Action::Remove(id) => {
                    self.mod_mode_place(
                        thedas_sim::commands::SimCommand::DestroyObject { target: id },
                        format!("remove {id}"),
                    );
                }
                modmode::Action::Export(id) => {
                    let note = self.export_glb(id);
                    if let Some(m) = self.mod_mode.as_mut() {
                        m.exported = Some(note);
                    }
                }
            }
        }
    }

    /// A mod-mode click in the world: picks the object under the cursor for the inspector.
    fn mod_mode_pick(&mut self) {
        let (Some(live), Some(g)) = (self.live.as_mut(), self.gfx.as_ref()) else {
            return;
        };
        let (w, h) = (g.config.width as f32, g.config.height as f32);
        let (x, y) = self.cursor;
        let (origin, dir) = self.camera.ray(x as f32, y as f32, w, h);
        let picked = match live.pick(origin, dir) {
            Some(live::Pick::Object(id)) => Some(id),
            _ => None,
        };
        if let Some(m) = self.mod_mode.as_mut() {
            m.selected = picked;
            if m.scrub.as_ref().is_some_and(|s| Some(s.id) != picked) {
                m.scrub = None;
            }
        }
    }

    /// A move, spawn or removal from mod mode: applied as mod mode's command, kept in its
    /// history (with what undoes it), and the objects drawn anew.
    fn mod_mode_place(
        &mut self,
        command: thedas_sim::commands::SimCommand,
        what: String,
    ) -> String {
        if self.mod_mode.is_none() {
            return "mod mode is not open".into();
        }
        self.place_object(command, what)
    }

    /// [`App::mod_mode_place`], also when mod mode's panels were never opened (live edits
    /// from the launch option): the history is kept once they are.
    fn place_object(&mut self, command: thedas_sim::commands::SimCommand, what: String) -> String {
        let Some(live) = self.live.as_mut() else {
            return "not playing".into();
        };
        let edit = {
            let mut sim = live.sim();
            modconv::object_edit(&mut sim, command, what)
        };
        let Some(m) = self.mod_mode.as_mut() else {
            log::info!("mod mode: {}", edit.text);
            return edit.text;
        };
        // Positions are read each frame, and a made or removed object changes the objects'
        // key, which rebuilds them.
        let note = edit.text.clone();
        log::info!("mod mode: {note}");
        m.placed = Some(note.clone());
        m.convs.status = note.clone();
        m.convs.history.push(edit);
        note
    }

    /// Where the middle of the view meets the ground (or the object there), in the world.
    fn view_point(&mut self) -> Option<[f32; 3]> {
        let (live, g) = (self.live.as_mut()?, self.gfx.as_ref()?);
        let (w, h) = (g.config.width as f32, g.config.height as f32);
        let (origin, dir) = self.camera.ray(w / 2.0, h / 2.0, w, h);
        let at = match live.pick(origin, dir)? {
            live::Pick::Ground(p) => p,
            live::Pick::Object(id) => live.sim().world.get(id)?.position,
        };
        Some(at)
    }

    /// Carries out a world-state browser request on the simulation.
    fn mod_mode_world(&mut self, a: modworld::WorldAction) {
        let (Some(m), Some(live)) = (self.mod_mode.as_mut(), self.live.as_ref()) else {
            return;
        };
        let picked = {
            let mut sim = live.sim();
            modworld::carry_out(&mut sim, &mut m.world, &mut m.convs, a, m.selected)
        };
        if let Some(id) = picked {
            m.selected = Some(id);
            if m.scrub.as_ref().is_some_and(|s| s.id != id) {
                m.scrub = None;
            }
        }
    }

    /// The camera flies free (`on`) or follows the leader again, as F switches it.
    fn set_fly(&mut self, on: bool) {
        if self.live.is_none() || on == self.follow.is_none() {
            return;
        }
        self.follow = (!on).then(|| follow::Follow::behind(self.camera.yaw));
    }

    /// Writes object `id`'s models for Blender into the player's folder,
    /// `workspace/blender/<tag>/`: `<tag>.glb`, its textures as PNGs and `manifest.json`
    /// (see [`glb`]). Says what it wrote, or why not.
    fn export_glb(&mut self, id: thedas_script::vm::ObjectId) -> String {
        let Some(live) = self.live.as_ref() else {
            return "Export: not playing".into();
        };
        let (subject, drawn, vfs) = {
            let sim = live.sim();
            let Some(o) = sim.world.get(id) else {
                return "Export: the object is gone".into();
            };
            let subject = glb::Subject {
                id,
                tag: o.tag.clone(),
                area: o.area.and_then(|a| sim.world.get(a)).map(|a| a.tag.clone()),
                template: o.template.clone(),
                position: o.position,
                facing: o.facing,
            };
            (
                subject,
                thedas_ui::models::object_models(&sim, o),
                sim.vfs.clone(),
            )
        };
        if drawn.models.is_empty() {
            return format!("Export: {} draws no model", subject.tag);
        }
        let read = |name: &str| {
            vfs.read(name).ok().map(|(data, p)| glb::Read {
                data,
                layer: p.layer_label,
                entry: p.entry,
            })
        };
        let mut e = glb::export(&read, &subject, &drawn.models);
        e.manifest["left_out"] = serde_json::json!({
            "attached": drawn.attached.iter().map(|a| [&a.model, &a.node]).collect::<Vec<_>>(),
            "morph": drawn.morph,
            "tints": drawn.tints,
        });
        let safe: String = subject
            .tag
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let dir = saves::data_dir()
            .join("workspace")
            .join("blender")
            .join(&safe);
        let write = || -> std::io::Result<()> {
            std::fs::create_dir_all(&dir)?;
            std::fs::write(dir.join(format!("{safe}.glb")), &e.glb)?;
            for (name, png) in &e.images {
                std::fs::write(dir.join(name), png)?;
            }
            let manifest = serde_json::to_vec_pretty(&e.manifest).unwrap_or_default();
            std::fs::write(dir.join("manifest.json"), manifest)
        };
        match write() {
            Ok(()) => {
                let meshes = glb::check(&e.glb).unwrap_or(0);
                let note = format!(
                    "Exported {} ({} models, {meshes} meshes, {} textures{}) to {}",
                    subject.tag,
                    drawn.models.len(),
                    e.images.len(),
                    if e.warnings.is_empty() {
                        String::new()
                    } else {
                        format!(", {} warnings in the manifest", e.warnings.len())
                    },
                    dir.display()
                );
                log::info!("{note}");
                note
            }
            Err(err) => format!("Export to {} failed: {err}", dir.display()),
        }
    }

    /// The camera flies free and looks at object `id`.
    fn fly_to(&mut self, id: thedas_script::vm::ObjectId) {
        let Some(at) = self
            .live
            .as_ref()
            .and_then(|l| l.sim().world.get(id).map(|o| o.position))
        else {
            return;
        };
        self.set_fly(true);
        self.camera = modworld::fly_to(&self.camera, glam::Vec3::from(at));
    }

    /// Carries out a conversation-panel request on the simulation.
    fn mod_mode_conv(&mut self, a: modconv::ConvAction) {
        let (Some(m), Some(live)) = (self.mod_mode.as_mut(), self.live.as_ref()) else {
            return;
        };
        let mut sim = live.sim();
        modconv::carry_out(&mut sim, &mut m.convs, a, m.selected);
    }

    /// The `modconv` step: the conversation panel opens on `dialog`. Its log line counts
    /// the owners found and the lines usable now.
    fn mod_mode_choose(&mut self, dialog: &str) -> String {
        if !self.mod_mode_up() {
            self.toggle_mod_mode();
        }
        if let Some(m) = self.mod_mode.as_mut() {
            m.convs.open = true;
        }
        self.mod_mode_conv(modconv::ConvAction::Choose(dialog.to_string()));
        let Some(c) = self.mod_mode.as_ref().map(|m| &m.convs) else {
            return format!("modconv {dialog}: no window");
        };
        let owners: Vec<_> = c
            .owners
            .iter()
            .map(|(id, t)| format!("{t} ({id})"))
            .collect();
        match &c.lines {
            None => format!("modconv {dialog}: could not be read"),
            Some(lines) => format!(
                "modconv {dialog}: owners [{}], {} lines, {} usable, {} flag reads",
                owners.join(", "),
                lines.len(),
                lines.iter().filter(|l| l.usable).count(),
                lines.iter().map(|l| l.reads.len()).sum::<usize>()
            ),
        }
    }

    /// The `modworld` and `modplot` steps: the browser opens, reads the world (or looks up
    /// plot `plot`), and says what it found.
    fn mod_mode_world_step(&mut self, plot: Option<&str>) -> String {
        if !self.mod_mode_up() {
            self.toggle_mod_mode();
        }
        if let Some(m) = self.mod_mode.as_mut() {
            m.world.open = true;
        }
        self.mod_mode_world(match plot {
            Some(p) => modworld::WorldAction::Lookup(p.to_string()),
            None => modworld::WorldAction::Refresh,
        });
        let Some(w) = self.mod_mode.as_ref().map(|m| &m.world) else {
            return "modworld: no window".into();
        };
        match (plot, &w.looked, &w.snapshot) {
            (Some(_), Some(Ok(p)), _) => format!(
                "modplot {}: {} flags, {} stored set",
                p.resref.as_deref().unwrap_or(&p.guid),
                p.flags.len(),
                p.flags.iter().filter(|f| f.value == Some(true)).count()
            ),
            (Some(_), Some(Err(e)), _) => format!("modplot: {e}"),
            (None, _, Some(s)) => format!(
                "modworld: {} globals, area {} with {} variables and {} objects, {} plots with \
                 {} stored flags",
                s.module.as_ref().map_or(0, |m| m.vars.len()),
                s.area.as_ref().map_or("none", |a| a.tag.as_str()),
                s.area.as_ref().map_or(0, |a| a.vars.len()),
                s.things.len(),
                s.plots.len(),
                s.plots.iter().map(|p| p.flags.len()).sum::<usize>()
            ),
            _ => "modworld: nothing read".into(),
        }
    }

    /// The `modlocal` step: a variable set (or removed) as the world-state browser does.
    fn mod_mode_local(&mut self, who: &str, name: &str, kind: &str, value: &str) -> String {
        if self.mod_mode.is_none() {
            return "mod mode is not open".into();
        }
        let target = self.live.as_ref().and_then(|l| {
            let sim = l.sim();
            match who {
                "module" => sim.world.module,
                "area" => sim.world.area,
                tag => sim.world.find_tag(tag, 0),
            }
        });
        let Some(target) = target else {
            return format!("modlocal: no {who}");
        };
        let value = match kind {
            "remove" => None,
            k => match modworld::parse_value(k, value) {
                Ok(v) => Some(v),
                Err(e) => return format!("modlocal: {e}"),
            },
        };
        self.mod_mode_world(modworld::WorldAction::SetLocal {
            target,
            name: name.to_string(),
            value,
        });
        self.mod_mode
            .as_ref()
            .map(|m| m.convs.status.clone())
            .unwrap_or_default()
    }

    /// The `modstart` and `modflag` steps: the conversation panel's Start, or a flag set
    /// from it. The log line is what the panel shows.
    fn mod_mode_command(&mut self, a: modconv::ConvAction) -> String {
        if self.mod_mode.is_none() {
            return "mod mode is not open".into();
        }
        self.mod_mode_conv(a);
        self.mod_mode
            .as_ref()
            .map(|m| m.convs.status.clone())
            .unwrap_or_default()
    }

    /// The `modselect` step: the inspector picks the object with this tag nearest the
    /// leader. Opens mod mode if it is closed.
    fn mod_mode_select(&mut self, tag: &str) -> String {
        if !self.mod_mode_up() {
            self.toggle_mod_mode();
        }
        let Some(live) = self.live.as_mut() else {
            return format!("modselect {tag}: not playing");
        };
        let sim = live.sim();
        let found = match sim.world.controlled() {
            Some(w) => nearest_tagged(&sim, w, tag),
            None => sim.world.find_tag(tag, 0),
        };
        drop(sim);
        let Some(m) = self.mod_mode.as_mut() else {
            return format!("modselect {tag}: no window");
        };
        m.selected = found;
        m.scrub = None;
        match found {
            Some(id) => format!("modselect {tag}: object {id}"),
            None => format!("modselect {tag}: nobody"),
        }
    }

    /// The `modframe` step: the inspected object's animation is held at frame `n`.
    fn mod_mode_hold(&mut self, n: i64) -> String {
        let Some(id) = self.mod_mode.as_ref().and_then(|m| m.selected) else {
            return "modframe: nothing inspected".into();
        };
        let Some(o) = self.live.as_mut().and_then(|l| l.inspect(id)) else {
            return "modframe: the object is gone".into();
        };
        let Some(m) = self.mod_mode.as_mut() else {
            return "modframe: no window".into();
        };
        if !m.hold(&o, Some(n)) {
            return format!("modframe: {} plays no animation", o.tag);
        }
        let s = m.scrub.as_ref().expect("just held");
        modmode::frame_note(&o, s)
    }

    /// An issue report (F8, the pad's two sticks clicked, or a client's `report`): the
    /// frame as presented, the game now and about a minute back, and what the game knows of
    /// itself, then the player's note (see [`report`]). The game pauses while the note is
    /// asked for. `note` answers the question instead (a client's); `reply` hears where the
    /// report went once it is written.
    fn report(&mut self, note: Option<String>, reply: Option<std::sync::mpsc::Sender<J>>) {
        if self.recorder.busy() {
            if let Some(r) = reply {
                let _ = r.send(control::failure("a report is already under way"));
            }
            return;
        }
        let mut info = json!({
            "build": report::build_info(),
            "settings": self.started_with,
            "game": self.status(),
        });
        if let Some(g) = self.gfx.as_ref() {
            let a = g.ctx.adapter.get_info();
            info["gpu"] = json!({
                "name": a.name,
                "backend": format!("{:?}", a.backend),
                "device_type": format!("{:?}", a.device_type),
                "driver": a.driver,
                "driver_info": a.driver_info,
                "vendor": a.vendor,
                "device": a.device,
            });
            info["window"] = json!({
                "size": [g.config.width, g.config.height],
                "scale_factor": g.window.scale_factor(),
                "format": format!("{:?}", g.config.format),
            });
        }
        // What the player has layered over the install (the original's override and add-in
        // folders, the mod list): these change what the game reads.
        info["mods"] = json!(
            thedas_vfs::mods::player_layers(&self.loaded.vfs)
                .iter()
                .map(|l| json!({ "layer": l.label, "files": l.files }))
                .collect::<Vec<_>>()
        );
        let mut now_save = None;
        let mut trace = None;
        let mut journal = None;
        let mut pause = false;
        if let Some(live) = self.live.as_mut() {
            if !live.front_end {
                journal = live.journal.as_mut().and_then(|j| j.files());
                let sim = live.sim();
                info["game"]["training_mode"] = json!(sim.world.training.mode);
                info["game"]["hero"] = json!(sim.world.hero.map(|h| sim.display_name(h)));
                info["game"]["area_name"] = json!(sim.world.area.map(|a| sim.display_name(a)));
                info["game"]["coverage"] = sim.world.trace.coverage();
                match sim.save_json() {
                    Ok(b) => now_save = Some(b),
                    Err(e) => log::warn!("report: the game's state: {e}"),
                }
                trace = serde_json::to_value(&sim.world.trace.recent).ok();
                drop(sim);
                pause = !live.paused;
            }
        }
        let dir = self.recorder.start(now_save, trace, journal, info);
        log::info!("report: started in {}", dir.display());
        // The frame as presented, read back as the next one is drawn; drawn again
        // offscreen where the surface cannot be read back.
        let readable = self
            .gfx
            .as_ref()
            .is_some_and(|g| g.config.usage.contains(wgpu::TextureUsages::COPY_SRC));
        let shot = if readable {
            let (tx, rx) = std::sync::mpsc::channel();
            self.frame_requests.push((tx, 1));
            Some(rx)
        } else {
            let png = self
                .screenshot_rgba()
                .and_then(|f| thedas_ui::stage::encode_png(&f).context("encoding the screenshot"));
            self.recorder.screenshot(png);
            None
        };
        // The player's note: a client's, else asked in the game's own text box (none
        // before a game: the report is kept without one).
        let in_game = self.live.as_ref().is_some_and(|l| !l.front_end)
            && self.screens.as_ref().is_some_and(|s| !s.front_end);
        let mut marking = Marking::Off;
        match (report::preset_note(note), self.gfx.as_mut()) {
            (Some(text), _) => self.recorder.note(Some(text)),
            // The player may first mark spots on the screen: boxes dragged with the mouse,
            // or drawn with the pad's pointer (see `PadPlay::mark`).
            (None, Some(g)) if in_game => {
                ask_mark_choice(&mut g.ui, self.pad_last);
                marking = Marking::Offered(0, 0);
            }
            (None, _) => self.recorder.note(Some(String::new())),
        }
        self.report_wait = Some(ReportWait {
            shot,
            reply,
            note_asked: None,
            send: true,
            pause,
            paused_it: false,
            marking,
        });
    }

    /// Ends marking spots and asks the report's note.
    fn end_marking(&mut self) {
        let pad = self.pad.used;
        if let (Some(w), Some(g)) = (self.report_wait.as_mut(), self.gfx.as_mut()) {
            w.marking = Marking::Off;
            ask_report_note(&mut g.ui, pad);
            w.note_asked = Some((0, 0));
        }
    }

    /// Whether boxes are being drawn for a report (the mouse and keys are its).
    fn marking(&self) -> bool {
        self.report_wait
            .as_ref()
            .is_some_and(|w| matches!(w.marking, Marking::On { .. }))
    }

    /// The marked spots and the box being dragged, as lines over the frame.
    fn marking_lines(&self) -> Vec<[glam::Vec2; 2]> {
        let Some(w) = self.report_wait.as_ref() else {
            return Vec::new();
        };
        let Marking::On { drag, asking } = &w.marking else {
            return Vec::new();
        };
        let height = self.gfx.as_ref().map_or(self.size.1, |g| g.config.height);
        let h = report::digit_height(height);
        let done = self.recorder.regions();
        let mut out: Vec<_> = done
            .iter()
            .enumerate()
            .flat_map(|(i, r)| report::region_lines(r.rect, Some(i + 1), h))
            .collect();
        if let Some((rect, ..)) = asking {
            out.extend(report::region_lines(*rect, Some(done.len() + 1), h));
        }
        if let Some((a, b)) = drag {
            out.extend(report::region_lines(report::rect_of(*a, *b), None, h));
        }
        out
    }

    /// An answer about marking: the choice, or a marked spot's note.
    fn mark_answer(&mut self, a: ui::TextAnswer) {
        let Some(w) = self.report_wait.as_mut() else {
            return;
        };
        match (a.id, &mut w.marking) {
            (report::MARK_ID, Marking::Offered(..)) if a.accepted => {
                w.marking = Marking::On {
                    drag: None,
                    asking: None,
                };
            }
            (report::MARK_ID, Marking::Offered(..)) => self.end_marking(),
            (report::REGION_ID, Marking::On { asking, .. }) => {
                if let Some((rect, ..)) = asking.take() {
                    if a.accepted {
                        self.recorder.add_region(report::Region {
                            rect,
                            note: a.text.trim().to_string(),
                        });
                    }
                }
            }
            _ => {}
        }
    }

    /// One frame of marking: the choice and a spot's note asked again when they do not
    /// show (as the report's note is), the choice given up after a few tries.
    fn marking_frame(&mut self) {
        let pad = self.pad_last;
        let (Some(w), Some(g)) = (self.report_wait.as_mut(), self.gfx.as_mut()) else {
            return;
        };
        let up = g.ui.stage.dialog_up();
        match &mut w.marking {
            Marking::Off => {}
            Marking::Offered(frames, tries) => {
                *frames += 1;
                if box_shows(&g.ui, report::MARK_PROMPT) || box_shows(&g.ui, MARK_PROMPT_PAD) {
                    *frames = 0;
                } else if *frames > NOTE_REASK_FRAMES {
                    *frames = 0;
                    *tries += 1;
                    if *tries > NOTE_TRIES {
                        self.end_marking();
                    } else if !up {
                        ask_mark_choice(&mut g.ui, pad);
                    }
                }
            }
            Marking::On { asking, .. } => {
                if let Some((_, frames, tries)) = asking.as_mut() {
                    *frames += 1;
                    if box_shows(&g.ui, report::REGION_PROMPT) {
                        *frames = 0;
                    } else if *frames > NOTE_REASK_FRAMES {
                        *frames = 0;
                        *tries += 1;
                        if *tries > NOTE_TRIES {
                            *asking = None;
                        } else if !up {
                            ask_region_note(&mut g.ui);
                        }
                    }
                }
            }
        }
    }

    /// The mouse while marking: a press starts a box, a release ends it and asks its note.
    /// Returns whether marking took the press.
    fn mark_button(&mut self, pressed: bool) -> bool {
        if !self.marking() {
            return false;
        }
        let at = glam::Vec2::new(self.cursor.0 as f32, self.cursor.1 as f32);
        let Some(w) = self.report_wait.as_mut() else {
            return false;
        };
        let Marking::On { drag, asking } = &mut w.marking else {
            return false;
        };
        // While a spot's note is asked, the mouse is the text box's.
        if asking.is_some() {
            return false;
        }
        if pressed {
            *drag = Some((at, at));
            return true;
        }
        let Some((from, _)) = drag.take() else {
            return true;
        };
        let rect = report::rect_of(from, at);
        if rect[2] - rect[0] >= MIN_MARK && rect[3] - rect[1] >= MIN_MARK {
            *asking = Some((rect, 0, 0));
            if let Some(g) = self.gfx.as_mut() {
                ask_region_note(&mut g.ui);
            }
        }
        true
    }

    /// One frame of reports: a snapshot of the game when one is due, and a report under
    /// way finished once its screenshot and note are in.
    fn report_frame(&mut self, dt_ms: f64) {
        self.recorder.frame(dt_ms);
        for a in std::mem::take(&mut self.mark_answers) {
            self.mark_answer(a);
        }
        self.marking_frame();
        let now = Instant::now();
        if self.recorder.snapshot_due(now) {
            self.take_snapshot(now);
        }
        let Some(w) = self.report_wait.as_mut() else {
            return;
        };
        if let Some(rx) = &w.shot {
            match rx.try_recv() {
                Ok(frame) => {
                    w.shot = None;
                    let png = match frame["png"].as_str() {
                        Some(b64) => {
                            use base64::Engine as _;
                            base64::engine::general_purpose::STANDARD
                                .decode(b64)
                                .context("decoding the screenshot")
                        }
                        None => Err(anyhow::anyhow!(
                            "{}",
                            frame["error"].as_str().unwrap_or("no frame")
                        )),
                    };
                    self.recorder.screenshot(png);
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    w.shot = None;
                    self.recorder
                        .screenshot(Err(anyhow::anyhow!("the frame was never read back")));
                }
            }
        }
        // The frame is in: the game holds still while the player writes.
        if w.pause && w.shot.is_none() {
            w.pause = false;
            if let Some(live) = self.live.as_mut().filter(|l| !l.paused) {
                live.paused = true;
                w.paused_it = true;
            }
        }
        // The text box can be lost (a script's message box shown over it, or no popup
        // layer up yet): with none up a moment after asking it is asked again, and while
        // another box keeps it out, or after a few tries, the report is kept without a note
        // so the game never waits on it.
        if let (Some((frames, tries)), Some(g)) = (w.note_asked.as_mut(), self.gfx.as_mut()) {
            let up = g.ui.stage.dialog_up();
            let ours = up
                && matches!(
                    g.ui.stage.bridge.value("GUIPopupLayer.DialogBoxText"),
                    Some(thedas_ui::UiValue::String(t))
                        if t == report::NOTE_PROMPT || t == report::NOTE_PROMPT_SEND
                );
            *frames += 1;
            if ours {
                *frames = 0;
            } else if *frames > NOTE_REASK_FRAMES {
                *frames = 0;
                *tries += 1;
                if *tries > NOTE_TRIES {
                    log::warn!("report: the note could not be asked; kept without one");
                    w.note_asked = None;
                    self.recorder.note(Some(String::new()));
                } else if !up {
                    ask_report_note(&mut g.ui, self.pad.used);
                }
            }
        }
        let Some(dir) = self.recorder.finish() else {
            return;
        };
        let Some(w) = self.report_wait.take() else {
            return;
        };
        let reply = w.reply;
        if w.paused_it {
            if let Some(live) = self.live.as_mut() {
                live.paused = false;
            }
        }
        // Sent on when the player has set that up (see [`upload`]).
        if let (true, Some(d), Some(url)) = (w.send, dir.as_ref(), upload::webhook()) {
            upload::send_in_background(d.clone(), url);
        }
        if let Some(r) = reply {
            let _ = r.send(match dir {
                Some(d) => json!({ "ok": true, "dir": d.display().to_string() }),
                None => json!({ "ok": true, "cancelled": true }),
            });
        }
    }

    /// One frame of the way back from a report (see [`update`]): the check's result, the
    /// question up (asked again when lost, as the report's note), the answers, the fetch.
    fn returning_frame(&mut self) {
        let answers = std::mem::take(&mut self.update_answers);
        for a in answers {
            self.update_answer(a);
        }
        match &mut self.returning {
            Returning::Idle => {}
            Returning::Checking(rx) => match rx.try_recv() {
                Ok(Some(offer)) => {
                    log::info!(
                        "update: build {} fixes report {}",
                        offer.build.commit,
                        offer.report.message
                    );
                    self.returning = Returning::Asking(Box::new(UpdatePrompt {
                        kind: UpdateAsk::Offer(offer),
                        asked: None,
                    }));
                }
                Ok(None) | Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.returning = Returning::Idle;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            },
            Returning::Asking(p) => {
                let in_game = self
                    .live
                    .as_ref()
                    .is_some_and(|l| !l.front_end && !l.loading())
                    && self.screens.as_ref().is_some_and(|s| !s.front_end);
                let Some(g) = self.gfx.as_mut() else {
                    return;
                };
                if !in_game || self.recorder.busy() {
                    return;
                }
                let (id, prompt, ok, other) = p.kind.ask();
                let up = g.ui.stage.dialog_up();
                match p.asked.as_mut() {
                    None if !up => {
                        if matches!(p.kind, UpdateAsk::VerdictNote(..)) {
                            g.ui.ask_text(id, &prompt, "", ok, other);
                        } else {
                            g.ui.ask_choice(id, &prompt, ok, other);
                        }
                        p.asked = Some((0, 0));
                    }
                    None => {}
                    Some((frames, tries)) => {
                        let ours = up
                            && matches!(
                                g.ui.stage.bridge.value("GUIPopupLayer.DialogBoxText"),
                                Some(thedas_ui::UiValue::String(t)) if t == prompt
                            );
                        *frames += 1;
                        if ours {
                            *frames = 0;
                        } else if *frames > NOTE_REASK_FRAMES && !up {
                            *frames = 0;
                            *tries += 1;
                            if *tries > NOTE_TRIES {
                                // Asked again later, once nothing else is up.
                                p.asked = None;
                            } else {
                                if matches!(p.kind, UpdateAsk::VerdictNote(..)) {
                                    g.ui.ask_text(id, &prompt, "", ok, other);
                                } else {
                                    g.ui.ask_choice(id, &prompt, ok, other);
                                }
                            }
                        }
                    }
                }
            }
            Returning::Fetching(rx, save, report) => match rx.try_recv() {
                Ok(Ok(exe)) => {
                    let size = self.started_with["size"].as_str().unwrap_or("1280x720");
                    // The next build is as quiet and as driven as this one: no sound where
                    // this had none, and the same control address (free once this closes).
                    let mut more = Vec::new();
                    if self.audio.is_none() {
                        more.push("--no-audio".to_string());
                    }
                    if let Some(c) = &self.control {
                        more.extend(["--control".to_string(), c.addr.to_string()]);
                    }
                    match update::start_build(&exe, &self.install, save, report, size, &more) {
                        Ok(()) => {
                            log::info!("update: started {}; closing this one", exe.display());
                            self.quit = true;
                            self.returning = Returning::Idle;
                        }
                        Err(e) => self.update_failed(format!("The new build did not start: {e:#}")),
                    }
                }
                Ok(Err(e)) => {
                    self.update_failed(format!("The new build could not be fetched: {e:#}"))
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.update_failed("The new build could not be fetched.".to_string())
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
            },
        }
    }

    fn update_failed(&mut self, why: String) {
        log::warn!("update: {why}");
        self.returning = Returning::Asking(Box::new(UpdatePrompt {
            kind: UpdateAsk::Failed(why),
            asked: None,
        }));
    }

    /// The player answered one of the way back's questions.
    fn update_answer(&mut self, a: ui::TextAnswer) {
        let Returning::Asking(p) = std::mem::replace(&mut self.returning, Returning::Idle) else {
            return;
        };
        if p.kind.ask().0 != a.id {
            self.returning = Returning::Asking(p);
            return;
        }
        match p.kind {
            UpdateAsk::Offer(o) if a.accepted => {
                self.returning = Returning::Asking(Box::new(UpdatePrompt {
                    kind: UpdateAsk::Where(o),
                    asked: None,
                }));
            }
            UpdateAsk::Offer(_) => log::info!("update: later"),
            UpdateAsk::Where(o) => {
                // Where I am: the game saved now (a save of the player's, in the list too);
                // where I reported: the report's earlier save.
                let save = if a.accepted {
                    self.save_for_update()
                } else {
                    let before = o.report.dir.join("save_before.json");
                    Ok(if before.is_file() {
                        before
                    } else {
                        o.report.dir.join("save_now.json")
                    })
                };
                match save {
                    Ok(save) => {
                        let (tx, rx) = std::sync::mpsc::channel();
                        let report = o.report.dir.clone();
                        let _ = std::thread::Builder::new()
                            .name("update fetch".into())
                            .spawn(move || {
                                let _ = tx.send(update::fetch_build(&o));
                            });
                        self.returning = Returning::Fetching(rx, save, report);
                    }
                    Err(e) => self.update_failed(format!("The game could not be saved: {e:#}")),
                }
            }
            // Fixed or not, then (a second box) anything to add.
            UpdateAsk::Verdict(dir, _) => {
                self.returning = Returning::Asking(Box::new(UpdatePrompt {
                    kind: UpdateAsk::VerdictNote(dir, a.accepted),
                    asked: None,
                }));
            }
            UpdateAsk::VerdictNote(dir, fixed) => {
                let note = if a.accepted { a.text } else { String::new() };
                update::send_verdict_in_background(dir, fixed, note);
            }
            UpdateAsk::Failed(_) => {}
        }
    }

    /// Saves the game for an update to start from: in a new slot of the player's saves
    /// (as the save screen does: the module hears PRESAVE first) and as a file the new
    /// build is started with.
    fn save_for_update(&mut self) -> Result<PathBuf> {
        let live = self.live.as_ref().context("no game is running")?;
        let mut sim = live.sim();
        if let Some(why) = sim.saving_blocked() {
            bail!("{why}");
        }
        sim.input(
            thedas_sim::commands::CommandSource::App,
            thedas_sim::commands::SimCommand::Presave,
        );
        let bytes = sim.save_json()?;
        let area = sim
            .world
            .area
            .map(|a| sim.display_name(a))
            .unwrap_or_default();
        let meta = saves::SaveMeta {
            name: format!("Before update ({area})"),
            area,
            time_ms: sim.world.time_ms,
            written: saves::now_secs(),
            hero: sim
                .world
                .hero
                .map(|h| sim.display_name(h))
                .unwrap_or_default(),
            level: sim
                .world
                .hero
                .map_or(0, |h| sim.property_total(h, 15) as i32),
            class: sim
                .world
                .hero
                .map_or(0, |h| sim.property_total(h, 27) as i32),
            kind: saves::KIND_SLOT,
        };
        drop(sim);
        let store = saves::SaveStore::new(self.saves_dir.clone());
        if let Err(e) = store.write(store.open_slot(), &bytes, &meta) {
            log::warn!("update: keeping the save in the list: {e}");
        }
        let path = saves::data_dir().join("builds").join("resume.json");
        std::fs::create_dir_all(path.parent().context("no folder")?)?;
        std::fs::write(&path, &bytes)?;
        Ok(path)
    }

    /// Keeps a snapshot of the game for reports, as a save would hold it: not while a level
    /// loads, before a game, during character creation or while saving is refused (a
    /// training lesson). Only reads the simulation (no PRESAVE event).
    fn take_snapshot(&mut self, now: Instant) {
        let in_chargen = self
            .screens
            .as_ref()
            .is_some_and(|s| s.shown().contains(&screens::Screen::CharGen));
        let Some(live) = self.live.as_ref() else {
            self.recorder.skip_snapshot(now);
            return;
        };
        if live.front_end || live.loading() || in_chargen || self.report_wait.is_some() {
            self.recorder.skip_snapshot(now);
            return;
        }
        let started = Instant::now();
        let sim = live.sim();
        if sim.saving_blocked().is_some() {
            drop(sim);
            self.recorder.skip_snapshot(now);
            return;
        }
        let time_ms = sim.world.time_ms;
        let area = sim
            .world
            .area
            .and_then(|a| sim.world.get(a))
            .map(|o| o.tag.clone())
            .unwrap_or_default();
        let saved = sim.save_json();
        drop(sim);
        match saved {
            Ok(save) => {
                log::debug!(
                    "report: snapshot at {time_ms} ms in {area}, {} bytes, {:.1} ms",
                    save.len(),
                    started.elapsed().as_secs_f64() * 1000.0
                );
                self.recorder.keep(report::Snapshot {
                    taken: now,
                    time_ms,
                    area,
                    save,
                });
            }
            Err(e) => {
                log::warn!("report: snapshot: {e}");
                self.recorder.skip_snapshot(now);
            }
        }
    }

    /// Whether the line the game is on has a voice-over the audio can play (see
    /// `ui::UiLayer::set_line_voiced`); never without sound.
    fn line_voiced(&self) -> bool {
        let (Some(a), Some(live)) = (self.audio.as_ref(), self.live.as_ref()) else {
            return false;
        };
        line_is_voiced(a, &live.sim())
    }

    /// The quick save (`save`) or quick load key, while playing with no menu up.
    fn quick(&mut self, save: bool) -> bool {
        let (Some(g), Some(live)) = (self.gfx.as_mut(), self.live.as_ref()) else {
            return false;
        };
        if save {
            g.ui.quick_save(&live.sim)
        } else {
            g.ui.quick_load(&live.sim)
        }
    }

    /// The creature the mouse is over, for its name over its head: none while the
    /// interface covers the cursor (a screen, the HUD). The interface's cover is read back
    /// from the GPU, so it is checked again only when the cursor or the creature changes.
    fn hovered_creature(&mut self) -> Option<thedas_script::vm::ObjectId> {
        let (Some(live), Some(g)) = (self.live.as_mut(), self.gfx.as_ref()) else {
            return None;
        };
        let (w, h) = (g.config.width as f32, g.config.height as f32);
        let (x, y) = self.cursor;
        let (origin, dir) = self.camera.ray(x as f32, y as f32, w, h);
        let creature = live.hovered_creature(origin, dir)?;
        let covered = match self.hover_probe {
            Some((at, c, covered)) if at == self.cursor && c == creature => covered,
            _ => {
                let covered = g.ui.covers(x, y);
                log::debug!(target: "thedas_app::hover", "{creature:?} at {x},{y}, covered {covered}");
                self.hover_probe = Some((self.cursor, creature, covered));
                covered
            }
        };
        (!covered).then_some(creature)
    }

    /// The mouse cursor's name now (see [`cursor`]): the movies' own over the interface and
    /// wherever a click does nothing, else by what is under it in the world. The interface's
    /// cover is read back from the GPU, so it is checked again only when the cursor moves or
    /// what is under it changes, and only when something is.
    fn cursor_name(&mut self) -> String {
        let pressed = self.left_down;
        let Some(g) = self.gfx.as_ref() else {
            return "standard".to_string();
        };
        let ui = g.ui.cursor().to_string();
        // Where a click would not reach the world (a screen, a cutscene, a conversation):
        // the movies' cursor.
        let playing = self.ui.is_empty()
            && self
                .screens
                .as_ref()
                .is_none_or(|s| s.exploring() || s.picking_party());
        let Some(live) = self.live.as_mut().filter(|_| playing) else {
            return cursor::name(&ui, true, None, None, pressed);
        };
        let (w, h) = (g.config.width as f32, g.config.height as f32);
        let (x, y) = self.cursor;
        let (origin, dir) = self.camera.ray(x as f32, y as f32, w, h);
        let aim = live.aim();
        let over = live.hover_over(origin, dir);
        let covered = match (&over, aim) {
            (None, None) => false,
            (over, _) => {
                // An aimed ability with nothing under the mouse is probed as a plain creature.
                let key = over.clone().unwrap_or(cursor::Over::Creature);
                match &self.cursor_probe {
                    Some((at, o, covered)) if *at == self.cursor && *o == key => *covered,
                    _ => {
                        let covered = g.ui.covers(x, y);
                        self.cursor_probe = Some((self.cursor, key, covered));
                        covered
                    }
                }
            }
        };
        cursor::name(&ui, covered, aim, over.as_ref(), pressed)
    }

    /// Shows the cursor for this frame.
    fn show_cursor(&mut self) {
        // Over mod mode's panels their own cursor shows.
        if self.cursors.is_none() || self.mod_mode.as_ref().is_some_and(|m| m.owns_cursor()) {
            return;
        }
        let name = self.cursor_name();
        if let (Some(c), Some(g)) = (self.cursors.as_mut(), self.gfx.as_ref()) {
            c.show(&g.window, &name);
        }
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
        self.recorder.input(format!("key {code:?}"));
        self.pad_last = false;
        match code {
            KeyCode::F10 => self.toggle_mod_mode(),
            // Marking spots for a report: F8, Enter or Escape is done.
            KeyCode::F8 | KeyCode::Enter | KeyCode::NumpadEnter | KeyCode::Escape
                if self.marking() =>
            {
                self.end_marking();
            }
            // An issue report (see [`report`]): ours, not the original's.
            KeyCode::F8 => self.report(None, None),
            // Escape closes a store, container, job board or world map the game put up by
            // its own Close (as the pad's B), else the player's screen up, else opens the game
            // menu over a game; the viewer closes. The start menu stays (its Exit quits).
            // Provisional: the exe's Escape on the game's screens is not traced.
            KeyCode::Escape if self.screens.as_ref().is_some_and(game_screen_up) => {
                match padplay::press_back(self) {
                    Some(path) => log::info!("escape: {path}"),
                    None => log::info!("escape: no close button on this screen"),
                }
            }
            KeyCode::Escape => match self.screens.as_mut() {
                None => return true,
                Some(s) if s.menu.is_some() => s.menu = None,
                Some(s) if !s.front_end => s.menu = Some(screens::Screen::GameMenu),
                Some(_) => {}
            },
            // While playing, P opens the spells/talents screen instead.
            KeyCode::KeyP if self.screens.is_none() => println!("camera {}", self.camera.spec()),
            // F5 quick save, F9 quick load: the exe's `QuickSave` and `QuickLoad` actions
            // (0x51e4d0, 0x51e6f6), which keymap.gda binds to F5 and F9. Provisional: the
            // keys are fixed here, not read from the player's bindings.
            KeyCode::F5 | KeyCode::F9
                if self
                    .screens
                    .as_ref()
                    .is_some_and(|s| !s.front_end && s.menu.is_none()) =>
            {
                self.quick(code == KeyCode::F5);
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
            // `SwapWeaponSets` (keymap.gda binds it to /): the controlled party member's other
            // weapon set, as the quickbar's weapon set button. Provisional: the key is fixed
            // here, not read from the player's bindings.
            KeyCode::Slash if self.live.is_some() => {
                if let Some(live) = self.live.as_ref() {
                    live.switch_weapon_set();
                    log::info!("weapon set switched");
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
        self.recorder.input(format!("wheel {steps:.2}"));
        if self.marking() {
            return;
        }
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
            l.sim().input(
                thedas_sim::commands::CommandSource::Player,
                thedas_sim::commands::SimCommand::PlayerZoomedIn,
            );
        }
    }

    /// The mouse moves to window pixel (`x`, `y`).
    fn cursor_to(&mut self, x: f64, y: f64) {
        self.cursor = (x, y);
        self.pad_last = false;
        if let Some(Marking::On {
            drag: Some((_, to)),
            ..
        }) = self.report_wait.as_mut().map(|w| &mut w.marking)
        {
            *to = glam::Vec2::new(x as f32, y as f32);
        }
        if let Some(g) = self.gfx.as_mut() {
            g.ui.mouse_move(x, y);
        }
    }

    /// The left button goes down or up at the cursor.
    fn left_button(&mut self, pressed: bool) {
        self.left_down = pressed;
        let (x, y) = self.cursor;
        self.recorder.input(format!(
            "{} {x:.0},{y:.0}",
            if pressed { "press" } else { "release" }
        ));
        // Marking spots for a report: the press draws a box, the world and interface do
        // not see it.
        if self.mark_button(pressed) {
            return;
        }
        // A click skips a cutscene. Provisional: the game's own skip key is not traced.
        if pressed
            && self
                .screens
                .as_ref()
                .is_some_and(screens::Screens::in_cutscene)
        {
            if let Some(live) = self.live.as_ref() {
                live.sim().input(
                    thedas_sim::commands::CommandSource::Player,
                    thedas_sim::commands::SimCommand::SkipCutscene,
                );
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
        // Marking spots: the camera holds still, so the boxes stay on what they mark.
        if self.marking() {
            return;
        }
        let (x, y) = self.cursor;
        self.recorder.input(format!(
            "{} {x:.0},{y:.0}",
            if pressed { "rpress" } else { "rrelease" }
        ));
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
                    l.sim().input(
                        thedas_sim::commands::CommandSource::Player,
                        thedas_sim::commands::SimCommand::PartyPicker,
                    );
                }
                log.push("party picker".into());
            }
            Step::Report => {
                self.key_down(KeyCode::F8);
                log.push("key f8".into());
            }
            &Step::Saves(mode) => {
                if let (Some(g), Some(s)) = (self.gfx.as_mut(), self.screens.as_mut()) {
                    if !s.front_end {
                        g.ui.open_saves(s, mode);
                    }
                }
                log.push(format!("saves {mode}"));
            }
            &Step::Quick(save) => {
                let done = self.quick(save);
                log.push(format!(
                    "quick {}: {}",
                    if save { "save" } else { "load" },
                    if done { "done" } else { "not done" }
                ));
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
                    l.sim().input(
                        thedas_sim::commands::CommandSource::App,
                        thedas_sim::commands::SimCommand::Transition {
                            area: area.clone(),
                            waypoint: waypoint.clone(),
                        },
                    );
                }
                log.push(format!("transition {area} {waypoint}"));
            }
            Step::Floaty(text) => {
                if let Some(l) = self.live.as_ref() {
                    test_floaty(&mut l.sim(), text);
                }
                log.push(format!("floaty {text:?}"));
            }
            Step::AskText(prompt) => {
                if let Some(g) = self.gfx.as_mut() {
                    g.ui.ask_text(0, prompt, "", "OK", "Cancel");
                }
                log.push(format!("asktext {prompt:?}"));
            }
            &Step::AllNames(on) => {
                self.all_names = on;
                log.push(format!("allnames {on}"));
            }
            &Step::HoldKey(code, down) => {
                if down {
                    self.keys.insert(code);
                } else {
                    self.keys.remove(&code);
                }
                log.push(format!("key {code:?} {}", if down { "down" } else { "up" }));
            }
            Step::AskChoice(prompt) => {
                if let Some(g) = self.gfx.as_mut() {
                    g.ui.ask_choice(0, prompt, "Yes", "No");
                }
                log.push(format!("askchoice {prompt:?}"));
            }
            Step::Use(tag) => {
                if let Some(l) = self.live.as_ref() {
                    let mut sim = l.sim();
                    let who = sim.world.controlled();
                    match (who, who.and_then(|w| nearest_tagged(&sim, w, tag))) {
                        (Some(w), Some(t)) => {
                            sim.input(
                                thedas_sim::commands::CommandSource::Player,
                                thedas_sim::commands::SimCommand::OrderUse { who: w, target: t },
                            );
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
                    l.sim().input(
                        thedas_sim::commands::CommandSource::Player,
                        thedas_sim::commands::SimCommand::SetTrainingMode { mode },
                    );
                }
                log.push(format!("training mode {mode}"));
            }
            Step::Training(name) => {
                let heard = self.live.as_ref().is_some_and(|l| {
                    l.sim().input(
                        thedas_sim::commands::CommandSource::Player,
                        thedas_sim::commands::SimCommand::TrainingEvent {
                            name: name.to_string(),
                            arg: 0,
                        },
                    )
                });
                log.push(format!(
                    "training {name}: {}",
                    if heard { "heard" } else { "not awaited" }
                ));
            }
            Step::Plot(plot, flag) => {
                if let Some(l) = self.live.as_ref() {
                    l.sim().input(
                        thedas_sim::commands::CommandSource::App,
                        thedas_sim::commands::SimCommand::SetPlotFlag {
                            plot: plot.clone(),
                            flag: *flag,
                            value: true,
                            call_script: true,
                        },
                    );
                }
                log.push(format!("plot {plot} {flag}"));
            }
            Step::Cutscene(name) => {
                if let Some(l) = self.live.as_ref() {
                    l.sim().input(
                        thedas_sim::commands::CommandSource::App,
                        thedas_sim::commands::SimCommand::QueueCutscene { name: name.clone() },
                    );
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
            Step::SwapWeapons => {
                self.key_down(KeyCode::Slash);
                log.push("weapon set key".into());
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
            Step::ModMode => {
                self.toggle_mod_mode();
                log.push(format!(
                    "mod mode {}",
                    if self.mod_mode_up() { "open" } else { "closed" }
                ));
            }
            Step::ModSelect(tag) => log.push(self.mod_mode_select(tag)),
            &Step::ModFrame(n) => log.push(self.mod_mode_hold(n)),
            Step::ModConv(d) => log.push(self.mod_mode_choose(d)),
            Step::ModStart => log.push(self.mod_mode_command(modconv::ConvAction::Start)),
            Step::ModFlag(plot, flag, value) => {
                let number = flag.parse::<i32>().ok().or_else(|| {
                    let info = self.live.as_ref()?.sim().plot_info(plot)?;
                    let found = info
                        .flags
                        .iter()
                        .find(|(_, n)| n.eq_ignore_ascii_case(flag));
                    found.map(|(k, _)| *k)
                });
                log.push(match number {
                    Some(n) => self.mod_mode_command(modconv::ConvAction::SetFlag {
                        plot: plot.clone(),
                        flag: n,
                        value: *value,
                    }),
                    None => format!("modflag: {plot} has no flag {flag}"),
                });
            }
            &Step::ModUndo(n) => log.push(self.mod_mode_command(modconv::ConvAction::Undo(n))),
            Step::ModFly => {
                self.set_fly(self.follow.is_some());
                log.push(format!(
                    "camera {}",
                    if self.follow.is_none() {
                        "flies free"
                    } else {
                        "follows"
                    }
                ));
            }
            Step::ModFlyTo => match self.mod_mode.as_ref().and_then(|m| m.selected) {
                Some(id) => {
                    self.fly_to(id);
                    log.push(format!("camera at {}", self.camera.spec()));
                }
                None => log.push("modflyto: nothing inspected".into()),
            },
            Step::ModWorld => log.push(self.mod_mode_world_step(None)),
            Step::ModPlot(name) => log.push(self.mod_mode_world_step(Some(name))),
            &Step::ModLive(on) => log.push(workspace::mount(on).unwrap_or_else(|e| e)),
            &Step::ModMove(position, facing) => {
                log.push(match self.mod_mode.as_ref().and_then(|m| m.selected) {
                    Some(id) => self.mod_mode_place(
                        thedas_sim::commands::SimCommand::MoveObject {
                            target: id,
                            position,
                            facing,
                        },
                        format!("move {id}"),
                    ),
                    None => "modmove: nothing inspected".into(),
                })
            }
            Step::ModMoveView => {
                let id = self.mod_mode.as_ref().and_then(|m| m.selected);
                log.push(match (id, self.view_point()) {
                    (Some(id), Some(at)) => {
                        let facing = self
                            .live
                            .as_ref()
                            .and_then(|l| l.sim().world.get(id).map(|o| o.facing))
                            .unwrap_or(0.0);
                        self.mod_mode_place(
                            thedas_sim::commands::SimCommand::MoveObject {
                                target: id,
                                position: at,
                                facing,
                            },
                            format!("move {id} to the view"),
                        )
                    }
                    (None, _) => "modmoveview: nothing inspected".into(),
                    (_, None) => "modmoveview: the view meets no ground".into(),
                });
            }
            Step::ModSpawn(creature, template) => {
                let kind = if *creature {
                    thedas_sim::world::ObjectKind::Creature
                } else {
                    thedas_sim::world::ObjectKind::Placeable
                };
                let facing = (self.camera.yaw.to_degrees() + 90.0).rem_euclid(360.0);
                log.push(match self.view_point() {
                    Some(at) => self.mod_mode_place(
                        thedas_sim::commands::SimCommand::CreateObject {
                            kind,
                            template: template.clone(),
                            position: at,
                            facing,
                        },
                        format!("spawn {template}"),
                    ),
                    None => "modspawn: the view meets no ground".into(),
                });
            }
            Step::ModAnims => {
                let id = self.mod_mode.as_ref().and_then(|m| m.selected);
                log.push(match (id, self.live.as_mut()) {
                    (Some(id), Some(l)) => {
                        let list = l.creature_actions(id);
                        let first: Vec<&str> = list.iter().take(5).map(|a| a.0.as_str()).collect();
                        format!("modanims: {} actions ({} …)", list.len(), first.join(", "))
                    }
                    _ => "modanims: nothing inspected".into(),
                });
            }
            Step::ModAnim(action) => {
                let id = self.mod_mode.as_ref().and_then(|m| m.selected);
                let played = match (id, self.live.as_mut()) {
                    (Some(id), Some(l)) => l.play_action(id, action).then_some(id),
                    _ => None,
                };
                if let (Some(id), Some(m)) = (played, self.mod_mode.as_mut()) {
                    m.scrub = Some(modmode::Scrub {
                        id,
                        time: 0.0,
                        length: 1.0,
                        playing: true,
                        speed: 1.0,
                        range: None,
                    });
                }
                log.push(format!(
                    "modanim {action}: {}",
                    if played.is_some() {
                        "playing"
                    } else {
                        "not drawn with a tree"
                    }
                ));
            }
            &Step::ModConvAt(n) => log.push(self.mod_mode_command(modconv::ConvAction::StartAt(n))),
            Step::ModStat(name, current, value) => {
                let id = self.mod_mode.as_ref().and_then(|m| m.selected);
                let found = match (id, self.live.as_ref()) {
                    (Some(id), Some(l)) => {
                        let sim = l.sim();
                        modworld::creature_stats(&sim, id).and_then(|st| {
                            st.stats
                                .iter()
                                .find(|s| {
                                    s.name.eq_ignore_ascii_case(name) || s.id.to_string() == *name
                                })
                                .map(|s| (id, s.id))
                        })
                    }
                    _ => None,
                };
                log.push(match found {
                    Some((id, p)) => {
                        let (base, cur) = if *current {
                            (None, Some(*value))
                        } else {
                            (Some(*value), None)
                        };
                        self.mod_mode_world(modworld::WorldAction::SetStat {
                            target: id,
                            property: p,
                            base,
                            current: cur,
                        });
                        self.mod_mode
                            .as_ref()
                            .map(|m| m.convs.status.clone())
                            .unwrap_or_default()
                    }
                    None => format!("modstat: no property {name} on the inspected creature"),
                });
            }
            Step::ModRemove => log.push(match self.mod_mode.as_ref().and_then(|m| m.selected) {
                Some(id) => self.mod_mode_place(
                    thedas_sim::commands::SimCommand::DestroyObject { target: id },
                    format!("remove {id}"),
                ),
                None => "modremove: nothing inspected".into(),
            }),
            Step::ModSync => log.push(
                self.live_edit_sync()
                    .unwrap_or_else(|| "Live edits: nothing changed".into()),
            ),
            Step::ModExport => match self.mod_mode.as_ref().and_then(|m| m.selected) {
                Some(id) => log.push(self.export_glb(id)),
                None => log.push("modexport: nothing inspected".into()),
            },
            Step::ModLocal(who, name, kind, value) => {
                log.push(self.mod_mode_local(who, name, kind, value))
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
                        sim.input(
                            thedas_sim::commands::CommandSource::Player,
                            thedas_sim::commands::SimCommand::OrderTalk { who: w, target: t },
                        );
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
                // An issue report, as F8 makes one; `"note"` is the player's note (none
                // asks, as F8 does). The reply comes when it is written.
                "report" => {
                    let note = r.body["note"].as_str().map(str::to_string);
                    self.report(note, Some(r.reply.clone()));
                    None
                }
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
            "mod_mode": self.mod_mode_up(),
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
        // The movies' input contexts, innermost last, and whether a dialog box has the input.
        if let Some(g) = self.gfx.as_ref() {
            out["contexts"] = json!(g.ui.stage.bridge.contexts());
            // The mouse cursor shown (empty with none of the game's loaded).
            out["cursor"] = json!(self.cursors.as_ref().map_or("", |c| c.shown()));
            out["popup"] = json!(g.ui.stage.dialog_up());
        }
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
        let mut over = self.pad.pointer(self.cursor, ctx);
        over.extend(self.marking_lines());
        reticle::draw_rgba(&mut frame.pixels, (w, h), &over);
        if let (Some(m), Some(g)) = (self.mod_mode.as_mut().filter(|m| m.open), self.gfx.as_ref()) {
            frame = paint_over(&g.ctx, g.config.format, frame, |encoder, view| {
                m.paint(&g.ctx.device, &g.ctx.queue, encoder, view, [w, h])
            })?;
        }
        Ok(frame)
    }

    /// One frame of the game controller: the device's state (and clients' `pad` steps)
    /// through the mapping, carried out as the mouse and keyboard would.
    fn pad_frame(&mut self, dt_ms: f64) {
        let device = self.pads.as_mut().map(|p| p.poll()).unwrap_or_default();
        let state = device.merged(&self.remote_pad);
        if !state.is_idle() {
            self.pad_last = true;
        }
        let mut pad = std::mem::take(&mut self.pad);
        for m in pad.frame(&state, self, dt_ms) {
            log::info!("pad: {m}");
            self.recorder.input(format!("pad: {m}"));
        }
        let report = std::mem::take(&mut pad.report);
        self.pad = pad;
        if report {
            self.report(None, None);
        }
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
                // The game menu's Report Issue: an issue report, as F8 makes one.
                // The game menu's Mod Mode: mod mode's panels, as F10 opens them.
                ui::FrontEnd::ModMode => {
                    self.toggle_mod_mode();
                    continue;
                }
                ui::FrontEnd::Report => {
                    self.report(None, None);
                    continue;
                }
                ui::FrontEnd::NewGame => live::Start::NewGame,
                ui::FrontEnd::Load(b) => live::Start::SaveBytes(b),
                ui::FrontEnd::StartMenu => live::Start::Menu,
                // Opened on the worker below, from the journal (`Start::Resume`).
                ui::FrontEnd::Resume => live::Start::NewGame,
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
            let resume = matches!(r, ui::FrontEnd::Resume).then(journal::default_dir);
            let (tx, rx) = std::sync::mpsc::channel();
            let spawned = std::thread::Builder::new()
                .name("game-open".into())
                .spawn(move || {
                    let start = match (&save, &resume) {
                        (Some(b), _) => live::Start::SaveBytes(b),
                        (None, Some(dir)) => live::Start::Resume(dir),
                        (None, None) => live::Start::NewGame,
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
                // A resume that failed says so, and offers to set the journal aside.
                if what == "Resume"
                    && let Some(g) = self.gfx.as_mut()
                {
                    g.ui.ask_choice(
                        RESUME_FAILED_ID,
                        &format!("The last session could not be resumed: {e:#}"),
                        "Set it aside",
                        "Keep it",
                    );
                }
            }
        }
    }

    /// The start menu's offer to resume a session that ended unexpectedly, asked once, when
    /// the start menu is up and no other box is: only for a journal this build wrote, of a
    /// game not quit normally, newer than the newest save (see [`journal::resumable`]).
    /// Ours: the original has no journal.
    fn resume_frame(&mut self) {
        if self.resume_offer != ResumeOffer::Unchecked || self.opening.is_some() {
            return;
        }
        let at_menu = self.live.as_ref().is_some_and(|l| l.front_end)
            && self.screens.as_ref().is_some_and(|s| {
                s.front_end && s.menu.is_none() && s.shown().contains(&screens::Screen::StartMenu)
            });
        let Some(g) = self.gfx.as_mut() else {
            return;
        };
        if !at_menu || g.ui.stage.dialog_up() {
            return;
        }
        self.resume_offer = ResumeOffer::Done;
        if !journal::enabled() {
            return;
        }
        let Some(info) = journal::resumable(&journal::default_dir()) else {
            return;
        };
        let newer = newest_save(&self.saves_dir).is_none_or(|s| info.written > s);
        if info.closed || !info.this_build || !newer {
            log::info!(
                "resume: not offered (quit normally {}, this build {}, newer than the saves {newer})",
                info.closed,
                info.this_build
            );
            return;
        }
        let prompt = format!(
            "Your last session ended unexpectedly ({}). Resume where you were?",
            time_ago(info.written, std::time::SystemTime::now())
        );
        g.ui.ask_choice(RESUME_ID, &prompt, "Resume", "Not now");
        self.resume_offer = ResumeOffer::Asked;
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
        // The snapshots were of the game this one replaces.
        self.recorder.forget();
    }

    fn fail(&mut self, event_loop: &ActiveEventLoop, e: anyhow::Error) {
        self.error = Some(e);
        event_loop.exit();
    }
}

impl padplay::Host for App {
    fn marking(&mut self) -> Option<bool> {
        match self.report_wait.as_ref().map(|w| &w.marking) {
            Some(Marking::On { drag, asking: None }) => Some(drag.is_some()),
            _ => None,
        }
    }

    fn end_marking(&mut self) {
        App::end_marking(self);
    }

    fn drop_mark(&mut self) {
        if let Some(Marking::On { drag, .. }) = self.report_wait.as_mut().map(|w| &mut w.marking) {
            *drag = None;
        }
    }

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
        // Mod mode's panels see input first while they are up (see [`modmode`]).
        if let (Some(m), Some(g)) = (self.mod_mode.as_mut(), self.gfx.as_ref()) {
            match m.window_event(&g.window, &event) {
                modmode::Routed::Game => {}
                modmode::Routed::Swallowed => return,
                modmode::Routed::CursorOnly => {
                    if let WindowEvent::CursorMoved { position, .. } = event {
                        self.cursor = (position.x, position.y);
                    }
                    return;
                }
                modmode::Routed::Pick => {
                    self.mod_mode_pick();
                    return;
                }
            }
        }
        match event {
            WindowEvent::CloseRequested => {
                if let Some(l) = self.live.as_mut() {
                    l.close_journal();
                }
                event_loop.exit()
            }
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
                        live.sim().input(
                            thedas_sim::commands::CommandSource::Player,
                            thedas_sim::commands::SimCommand::SkipCutscene,
                        );
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
/// Whether the line on screen is heard: its own voice-over is in the audio's banks (see
/// [`voice_line`]), or it is a cutscene's line whose voice is mixed into the cutscene's own
/// sound (`CutsceneLine::vo_mode` not 0, the intro's narration). For the subtitles: a line
/// that is heard shows no text under "No subtitles".
fn line_is_voiced(audio: &audio::GameAudio, sim: &thedas_sim::Sim) -> bool {
    if voice_line(sim).is_some_and(|l| audio.line_voice(&l).is_some()) {
        return true;
    }
    sim.conversation_line().is_none()
        && sim
            .world
            .cutscene
            .as_ref()
            .and_then(|c| c.line_now())
            .is_some_and(|l| l.vo_mode != 0)
}

/// The voice-over the line on screen wants: the conversation's line, else a cutscene's line
/// that has its own voice. With [`audio::GameAudio::line_voice`] this says whether the line
/// is voiced (and for how long) as soon as it is chosen, before its voice plays.
pub(crate) fn voice_line(sim: &thedas_sim::Sim) -> Option<audio::VoiceLine> {
    let line = sim.conversation_line().and_then(|l| {
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
    if line.is_some() {
        return line;
    }
    let c = sim.world.cutscene.as_ref()?;
    let l = c.line_now().filter(|l| l.vo_mode == 0)?;
    let conv = l.conversation.as_ref()?;
    let female = sim
        .world
        .hero
        .and_then(|h| sim.world.get(h))
        .is_some_and(|o| o.fields.get("Gender").copied() == Some(2));
    Some(audio::VoiceLine {
        dialog: conv.clone(),
        strref: l.strref,
        female,
        ambient: false,
    })
}

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
    let mut ambient = Vec::new();
    if let Some(ui) = ui {
        state.gui_sounds = ui.stage.bridge.drain_sounds();
        // Character creation's voice previews: always said, by nobody in particular.
        for (set, entry) in ui.drain_sound_set_previews() {
            state.sound_sets.push((set, entry, 1.0, None));
        }
        state.volumes = audio::volumes(|n| ui.client_option(n));
        ambient = ui.drain_ambient_lines();
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
                    height: Some(live::appearance_height(&sim, *id)),
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
        state.line = voice_line(&sim);
        // A cutscene's sound tracks.
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
        }
    }
    // Lines said in passing, at their speakers' heads.
    {
        let sim = live.sim();
        for l in ambient {
            let Some(strref) = l.strref else {
                continue;
            };
            let at = sim.world.get(l.speaker).map(|o| {
                let p = o.position;
                [p[0], p[1], p[2] + live.head_height(o.id)]
            });
            state.ambient_lines.push((
                audio::VoiceLine {
                    dialog: l.dialog,
                    strref,
                    female: l.female,
                    ambient: true,
                },
                at,
            ));
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
            // Its sound occluders (read-only: the simulation's own collision is used only
            // when it has already loaded it).
            if a.wants_occluders(&resref) {
                a.set_occluders(
                    &resref,
                    sim.loaded_collision(area_id),
                    sim.install_root.clone(),
                    sim.sound_occluders(area_id),
                );
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
    // Between the party leader's head and the camera (`sound_settings.gda` "Camera /
    // player listener balance position": 0.17 of the way to the camera). DAOrigins.exe's
    // listener update (0x5d6610) takes the controlled creature's position raised 1.8 m
    // (0xb004d8) and moves it towards the camera (0x5b6390); sound occlusion casts its rays
    // from there (0x5b731c). Without a creature to follow, the camera itself.
    let cam = camera.position;
    let head = live
        .leader_feet()
        .map(|f| f + glam::Vec3::new(0.0, 0.0, 1.8))
        .unwrap_or(cam);
    state.leader = live.leader_feet().map(|f| f.to_array());
    let p = head + (cam - head) * 0.17;
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
            self.live.sim().input(
                thedas_sim::commands::CommandSource::Player,
                thedas_sim::commands::SimCommand::SkipCutscene,
            );
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
    pin.is_some_and(|p| {
        sim.input(
            thedas_sim::commands::CommandSource::Player,
            thedas_sim::commands::SimCommand::BeginTravel { pin: p },
        )
    })
}

/// Draws the party's portraits and the paperdoll when they are due (see
/// [`ui::UiLayer::wants_portraits`] and [`ui::UiLayer::wants_paperdoll`]), with the
/// interface up.
fn draw_likenesses(
    live: &mut live::Live,
    ui: &mut ui::UiLayer,
    screens: Option<&screens::Screens>,
    ctx: &GpuContext,
    dt: f64,
) {
    let Some(screens) = screens else {
        return;
    };
    let key = ui::portrait_key(&live.sim());
    if ui.wants_portraits(key, dt) {
        let shots = live.portraits(ctx, ui::PORTRAIT_SIZE);
        ui.set_portraits(shots);
    }
    let due = ui.wants_paperdoll(screens, &live.sim(), dt);
    if let Some((who, yaw)) = due {
        let (w, h) = ui::PAPERDOLL_SIZE;
        let shot = live.paperdoll(ctx, who, yaw, w, h);
        ui.set_paperdoll(shot);
    }
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
    /// `key f5` or `key f9`: the save screen (1) or the load screen (2) opens (the keys
    /// themselves quick save and quick load, see `Quick`).
    Saves(i32),
    /// `quicksave` or `quickload`: what the F5 and F9 keys do.
    Quick(bool),
    /// `key f8`: an issue report, or done marking spots for one (see [`report`]).
    Report,
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
    /// The weapon set key (/).
    SwapWeapons,
    /// F10: mod mode's panels open or close (windowed only).
    ModMode,
    /// Mod mode's inspector picks the creature or placeable with this tag nearest the
    /// leader (as a Ctrl+click on it does).
    ModSelect(String),
    /// Mod mode holds the inspected object's animation at this frame (from 0).
    ModFrame(i64),
    /// Mod mode's conversation panel opens on this conversation (`modconv`).
    ModConv(String),
    /// The conversation panel's Start (`modstart`).
    ModStart,
    /// A plot flag (its number or name) set (1) or cleared (0) from the conversation panel
    /// (`modflag PLOT FLAG 1`).
    ModFlag(String, String, bool),
    /// The conversation panel's history entry n (from 0) is undone (`modundo`).
    ModUndo(usize),
    /// Mod mode's Fly: the camera flies free, or follows again (`modfly`).
    ModFly,
    /// The free camera looks at the inspected object (`modflyto`).
    ModFlyTo,
    /// The world-state browser opens and reads the world; its log line counts what it
    /// read (`modworld`).
    ModWorld,
    /// The world-state browser looks up a plot (`modplot NAME`).
    ModPlot(String),
    /// The inspected object's models are exported for Blender (`modexport`).
    ModExport,
    /// Live edits on or off (`modlive on`, `modlive off`).
    ModLive(bool),
    /// Live edits look for saved textures now (`modsync`).
    ModSync,
    /// The inspected object is put at a place, turned to a facing (`modmove X,Y,Z F`).
    ModMove([f32; 3], f32),
    /// The inspected object is put where the view's middle meets the ground (`modmoveview`).
    ModMoveView,
    /// A creature or placeable is made from a template where the view's middle meets the
    /// ground (`modspawn creature TEMPLATE`, `modspawn placeable TEMPLATE`).
    ModSpawn(bool, String),
    /// The inspected object is removed (`modremove`).
    ModRemove,
    /// The inspected creature's animation actions are listed (`modanims`).
    ModAnims,
    /// The inspected creature plays this action of its tree, held (`modanim ACTION`).
    ModAnim(String),
    /// The picked conversation starts at this line (`modconvat N`).
    ModConvAt(usize),
    /// The inspected creature's property (by its `PROPERTY_*` name or id) set: its base or
    /// current value (`modstat STRENGTH base 30`).
    ModStat(String, bool, f32),
    /// A variable set from the world-state browser (`modlocal WHO NAME TYPE VALUE`, WHO
    /// `module`, `area` or an object's tag; TYPE `int`, `float`, `string`, `object` or
    /// `resource`, or `remove` with no value).
    ModLocal(String, String, String, String),
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
    /// The engine's own text box (`asktext Describe the issue`), as
    /// `ui::UiLayer::ask_text` shows it; its answer is logged.
    AskText(String),
    /// A movement key held or let go (`keydown w`, `keyup w`; w, a, s, d, q or e), as the
    /// keyboard holds it.
    HoldKey(KeyCode, bool),
    /// The highlight key held or let go (`allnames on`, `allnames off`): every creature near
    /// shows its name, as while Tab is down.
    AllNames(bool),
    /// The engine's own two-button box (`askchoice Is it fixed?`), as
    /// `ui::UiLayer::ask_choice` shows it (Yes / No); its answer is logged.
    AskChoice(String),
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
            sim.input(
                thedas_sim::commands::CommandSource::Player,
                thedas_sim::commands::SimCommand::ClientOptionMessage {
                    name: thedas_sim::chargen::MSG_CLIENT_OPTION.to_string(),
                    args: [name.to_string(), v.to_string()].to_vec(),
                },
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
        l.sim().input(
            thedas_sim::commands::CommandSource::Player,
            thedas_sim::commands::SimCommand::ClientOptionMessage {
                name: thedas_sim::chargen::MSG_CLIENT_OPTION.to_string(),
                args: [name.to_string(), value.to_string()].to_vec(),
            },
        );
    }
}

/// Whether the screen up is one the game put up that the player leaves by its Close: a store,
/// a container (the inventory screen the player did not open), the job board or the world map.
fn game_screen_up(s: &screens::Screens) -> bool {
    use screens::Screen;
    !s.front_end
        && s.shown().iter().any(|m| match m {
            Screen::Store | Screen::Chanters | Screen::WorldMap => true,
            Screen::Inventory => s.menu != Some(Screen::Inventory),
            _ => false,
        })
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
    // `floaty portrait: TEXT` is a message by the controlled creature's portrait instead
    // (a script's `DisplayPortraitMessage`).
    if let (Some(text), Some(target)) = (text.strip_prefix("portrait:"), sim.world.controlled()) {
        sim.world
            .portrait_messages
            .push(thedas_sim::world::PortraitMessage {
                target,
                text: text.trim().to_string(),
                colour: 0xffffff,
            });
        return;
    }
    // `floaty banner: KIND ICON TEXT` is a large notification of notifications.gfx's kind
    // (7 new ability, 10 map area, 11 specialization), with an icon resource (`-` for none)
    // and its text, about the controlled creature.
    if let Some(rest) = text.strip_prefix("banner:") {
        let mut words = rest.split_whitespace();
        let kind = words.next().and_then(|k| k.parse().ok()).unwrap_or(7);
        let icon = words.next().filter(|i| *i != "-").map(str::to_string);
        sim.world.uber_alerts.push(thedas_sim::journal::Alert {
            kind,
            text: Some(words.collect::<Vec<_>>().join(" ")),
            icon,
            creature: sim.world.controlled(),
            ..Default::default()
        });
        return;
    }
    // `floaty status: TEXT` is a status message (a script's `DisplayStatusMessage`).
    if let Some(text) = text.strip_prefix("status:") {
        sim.world
            .status_messages
            .push((text.trim().to_string(), 0xffffff));
        return;
    }
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
        sim.input(
            thedas_sim::commands::CommandSource::App,
            thedas_sim::commands::SimCommand::Kill {
                id: m,
                killer: thedas_script::vm::OBJECT_INVALID,
            },
        );
    }
    sim.input(
        thedas_sim::commands::CommandSource::App,
        thedas_sim::commands::SimCommand::Tick { ms: 0 },
    );
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
        "asktext" => Step::AskText(arg.trim().to_string()),
        "allnames" => Step::AllNames(arg.trim() == "on"),
        "keydown" | "keyup" => {
            let code = match arg.trim().to_ascii_lowercase().as_str() {
                "w" => KeyCode::KeyW,
                "a" => KeyCode::KeyA,
                "s" => KeyCode::KeyS,
                "d" => KeyCode::KeyD,
                "q" => KeyCode::KeyQ,
                "e" => KeyCode::KeyE,
                other => bail!("{verb} takes w, a, s, d, q or e, not {other:?}"),
            };
            Step::HoldKey(code, verb == "keydown")
        }
        "askchoice" => Step::AskChoice(arg.trim().to_string()),
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
        "key" if arg == "/" => Step::SwapWeapons,
        "key" if arg.eq_ignore_ascii_case("f10") => Step::ModMode,
        "modmode" => Step::ModMode,
        "modselect" if !arg.is_empty() => Step::ModSelect(arg.to_string()),
        "modframe" => Step::ModFrame(arg.parse().context("modframe takes a frame number")?),
        "modconv" if !arg.is_empty() => Step::ModConv(arg.to_string()),
        "modstart" => Step::ModStart,
        "modflag" => {
            let mut w = arg.split_whitespace();
            let (Some(plot), Some(flag), Some(v), None) = (w.next(), w.next(), w.next(), w.next())
            else {
                bail!("modflag takes PLOT FLAG 0|1");
            };
            Step::ModFlag(
                plot.to_string(),
                flag.to_string(),
                match v {
                    "1" => true,
                    "0" => false,
                    _ => bail!("modflag's value is 0 or 1"),
                },
            )
        }
        "modundo" => Step::ModUndo(arg.parse().context("modundo takes a history entry")?),
        "modfly" => Step::ModFly,
        "modflyto" => Step::ModFlyTo,
        "modworld" => Step::ModWorld,
        "modplot" if !arg.is_empty() => Step::ModPlot(arg.to_string()),
        "modexport" => Step::ModExport,
        "modlive" => Step::ModLive(match arg {
            "on" => true,
            "off" => false,
            _ => bail!("modlive takes on or off"),
        }),
        "modsync" => Step::ModSync,
        "modmove" => {
            let (at, f) = arg.split_once(' ').context("modmove takes X,Y,Z FACING")?;
            let v: Vec<f32> = at
                .split(',')
                .map(|x| x.trim().parse::<f32>())
                .collect::<Result<_, _>>()
                .context("modmove's place is X,Y,Z")?;
            let [x, y, z] = v[..] else {
                bail!("modmove's place is X,Y,Z");
            };
            Step::ModMove([x, y, z], f.trim().parse().context("modmove's facing")?)
        }
        "modmoveview" => Step::ModMoveView,
        "modspawn" => match arg.split_once(' ') {
            Some(("creature", t)) if !t.trim().is_empty() => Step::ModSpawn(true, t.trim().into()),
            Some(("placeable", t)) if !t.trim().is_empty() => {
                Step::ModSpawn(false, t.trim().into())
            }
            _ => bail!("modspawn takes creature|placeable TEMPLATE"),
        },
        "modremove" => Step::ModRemove,
        "modanims" => Step::ModAnims,
        "modanim" if !arg.is_empty() => Step::ModAnim(arg.to_string()),
        "modconvat" => Step::ModConvAt(arg.parse().context("modconvat takes a line number")?),
        "modstat" => {
            let mut w = arg.split_whitespace();
            let (Some(name), Some(which), Some(v), None) = (w.next(), w.next(), w.next(), w.next())
            else {
                bail!("modstat takes NAME base|current VALUE");
            };
            let current = match which {
                "base" => false,
                "current" => true,
                _ => bail!("modstat sets base or current"),
            };
            Step::ModStat(name.into(), current, v.parse().context("modstat's value")?)
        }
        "modlocal" => {
            let mut w = arg.splitn(4, ' ');
            let (Some(who), Some(name), Some(kind)) = (w.next(), w.next(), w.next()) else {
                bail!("modlocal takes WHO NAME TYPE VALUE");
            };
            let value = w.next().unwrap_or_default();
            Step::ModLocal(who.into(), name.into(), kind.into(), value.into())
        }
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
        "quicksave" => Step::Quick(true),
        "quickload" => Step::Quick(false),
        "key" if arg.eq_ignore_ascii_case("f8") => Step::Report,
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
    report::init_logging(env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info,wgpu=warn,naga=warn"),
    ));
    let mut args = Args::parse();
    faces::set_language(&args.language);
    // A client's or a test's run sends and fetches nothing (see `upload::online`).
    if args.control.is_some() || std::env::var_os("THEDAS_HIDDEN_WINDOW").is_some() {
        upload::set_automated();
    }
    // A newer build an update fetched takes over from the one the player started.
    if args.capture.is_none() && args.check_report.is_none() {
        if let Some(exe) = update::newest_build() {
            match std::process::Command::new(&exe)
                .args(std::env::args_os().skip(1))
                .envs(update::program_dir().map(|d| (update::PROGRAM_DIR, d)))
                .spawn()
            {
                Ok(_) => {
                    log::info!("update: handing over to {}", exe.display());
                    return Ok(());
                }
                Err(e) => log::warn!("update: {} did not start: {e}", exe.display()),
            }
        }
    }
    if let Some(secs) = args.pad_probe {
        paddev::probe(secs);
        return Ok(());
    }
    if let Some(out) = &args.launcher_capture {
        return launcher::capture(
            out,
            &args.launcher_page,
            parse_size(&args.size)?,
            args.launcher_scale,
        );
    }
    if let Some(keys) = &args.keys {
        thedas_vfs::keys_file::set_path(keys);
    }
    let install = args
        .install
        .clone()
        .or_else(|| std::env::var_os("THEDAS_INSTALL").map(PathBuf::from));
    // No install named, playing in a window: the launcher picks one (and the game starts as
    // a process of its own). Headless and driven runs keep needing `--install`.
    let headless = args.capture.is_some()
        || args.control.is_some()
        || !args.steps.is_empty()
        || args.check_report.is_some()
        || std::env::var_os("THEDAS_HIDDEN_WINDOW").is_some();
    if args.launcher || (install.is_none() && !headless) {
        if let Some(launch) = launcher::run(args.launcher)? {
            launch.spawn()?;
        }
        return Ok(());
    }
    let install = install.context("give --install or set THEDAS_INSTALL")?;
    if args.module.is_empty() {
        args.module = thedas_vfs::install::campaign_module(&install).to_string();
    }
    let size = parse_size(&args.size)?;
    let saves_dir = args
        .saves
        .clone()
        .unwrap_or_else(saves::SaveStore::default_dir);
    let journal_dir = journal::default_dir();
    journal::set_enabled(
        args.journal
            || args.resume
            || (args.capture.is_none() && args.steps.is_empty() && args.control.is_none()),
    );
    let start = if args.resume {
        Some(live::Start::Resume(&journal_dir))
    } else if let Some(save) = &args.save {
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
        // The mouse hovers nothing until a `hover` step moves it: it starts at the centre
        // (for aiming), which is over the hero, whose name would be in every capture.
        let mut hovering = false;
        // The highlight held by an `allnames` step (the pad's RT holds it too).
        let mut all_names = false;
        // The game controller as `pad` steps hold it, and its part of the game.
        let mut pad_state = pad::PadState::default();
        let mut pad_play = padplay::PadPlay::default();
        let mut pad_lines = (Vec::new(), Vec::new());
        if let Some(l) = live.as_mut() {
            l.shader_time = args.time;
            l.movies_wait = true;
            if ui.is_empty() {
                let mut layer = ui::UiLayer::new(&ctx, &loaded.vfs, &[], size);
                if let Some(path) = &args.settings {
                    layer.set_settings_file(path.clone());
                }
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
            l.sim().input(
                thedas_sim::commands::CommandSource::App,
                thedas_sim::commands::SimCommand::Tick { ms: args.run_ms },
            );
            let mut load_screen_up = false;
            for step in &steps {
                let l = live.as_mut().expect("a game or the start menu");
                match step {
                    &Step::Hover(x, y) => {
                        cursor = (x, y);
                        hovering = true;
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
                    Step::ModMode
                    | Step::ModSelect(_)
                    | Step::ModFrame(_)
                    | Step::ModConv(_)
                    | Step::ModStart
                    | Step::ModFlag(..)
                    | Step::ModUndo(_)
                    | Step::ModFly
                    | Step::ModFlyTo
                    | Step::ModWorld
                    | Step::ModPlot(_)
                    | Step::ModExport
                    | Step::ModLive(_)
                    | Step::ModSync
                    | Step::ModMove(..)
                    | Step::ModMoveView
                    | Step::ModSpawn(..)
                    | Step::ModRemove
                    | Step::ModAnims
                    | Step::ModAnim(_)
                    | Step::ModConvAt(_)
                    | Step::ModStat(..)
                    | Step::ModLocal(..) => {
                        println!("mod mode: windowed only")
                    }
                    Step::Talk(tag) => {
                        let mut sim = l.sim();
                        let who = sim.world.controlled();
                        let target = who.and_then(|w| nearest_tagged(&sim, w, tag));
                        match (who, target) {
                            (Some(w), Some(t)) => {
                                sim.input(
                                    thedas_sim::commands::CommandSource::Player,
                                    thedas_sim::commands::SimCommand::OrderTalk {
                                        who: w,
                                        target: t,
                                    },
                                );
                                println!("talk to {tag}");
                            }
                            _ => println!("talk to {tag}: nobody"),
                        }
                    }
                    &Step::Click(x, y) => {
                        if played.as_ref().is_some_and(|(_, s)| s.in_cutscene()) {
                            l.sim().input(
                                thedas_sim::commands::CommandSource::Player,
                                thedas_sim::commands::SimCommand::SkipCutscene,
                            );
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
                                let voiced = recording
                                    .as_ref()
                                    .is_some_and(|a| line_is_voiced(a, &l.sim()));
                                layer.set_line_voiced(voiced);
                                if let Some(aim) = layer.play(s, &l.sim, dt).pop() {
                                    println!("aiming ability {}", aim.ability);
                                    l.aiming = Some(aim);
                                }
                                // No picture is taken for a save made headless.
                                layer.take_saved_slots();
                                for a in layer.take_text_answers() {
                                    println!(
                                        "text box {}: {} {:?}",
                                        a.id,
                                        if a.accepted { "OK" } else { "Cancel" },
                                        a.text
                                    );
                                }
                                layer.update_minimap(&l.sim, &camera);
                                // Headless, the interface's cover of the cursor is not
                                // checked (its movies are drawn only for the capture).
                                let (o, d) = camera.ray(
                                    cursor.0 as f32,
                                    cursor.1 as f32,
                                    size.0 as f32,
                                    size.1 as f32,
                                );
                                layer.set_show_all_names(all_names || pad_play.highlight);
                                let over = hovering.then(|| l.hovered_creature(o, d));
                                layer.set_hovered(over.flatten());
                                layer.update_floaties(&l.sim, &camera, dt, &|c| l.head_height(c));
                                ui_time += t.elapsed();
                            }
                            if let Some(a) = recording.as_mut() {
                                let gui = played.as_ref().is_some_and(|(_, s)| s.world_stopped());
                                let ui = played.as_mut().map(|(layer, _)| layer);
                                let mut state = audio_state(a, Some(&mut *l), ui, &camera, None);
                                state.gui = gui;
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
                                l.sim().input(
                                    thedas_sim::commands::CommandSource::Player,
                                    thedas_sim::commands::SimCommand::PlayerZoomedIn,
                                );
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
                        l.sim().input(
                            thedas_sim::commands::CommandSource::Player,
                            thedas_sim::commands::SimCommand::PartyPicker,
                        );
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
                    Step::Report => println!("key f8: reports need the window"),
                    &Step::Saves(mode) => {
                        if let Some((layer, s)) = played.as_mut() {
                            layer.open_saves(s, mode);
                        }
                        println!("{} screen", if mode == 1 { "save" } else { "load" });
                    }
                    &Step::Quick(save) => {
                        let done = played.as_mut().is_some_and(|(layer, _)| {
                            if save {
                                layer.quick_save(&l.sim)
                            } else {
                                layer.quick_load(&l.sim)
                            }
                        });
                        println!(
                            "quick {}: {}",
                            if save { "save" } else { "load" },
                            if done { "done" } else { "not done" }
                        );
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
                        l.sim().input(
                            thedas_sim::commands::CommandSource::App,
                            thedas_sim::commands::SimCommand::Transition {
                                area: area.clone(),
                                waypoint: waypoint.clone(),
                            },
                        );
                        println!("transition {area} {waypoint}");
                    }
                    Step::Floaty(text) => {
                        test_floaty(&mut l.sim(), text);
                        println!("floaty {text:?}");
                    }
                    Step::AskText(prompt) => {
                        if let Some((layer, _)) = played.as_mut() {
                            layer.ask_text(0, prompt, "", "OK", "Cancel");
                        }
                        println!("asktext {prompt:?}");
                    }
                    &Step::AllNames(on) => {
                        all_names = on;
                        println!("allnames {on}");
                    }
                    // The movement keys are the window's (see `App::key_walk`).
                    Step::HoldKey(..) => println!("keydown and keyup need a window"),
                    Step::AskChoice(prompt) => {
                        if let Some((layer, _)) = played.as_mut() {
                            layer.ask_choice(0, prompt, "Yes", "No");
                        }
                        println!("askchoice {prompt:?}");
                    }
                    Step::Use(tag) => {
                        let mut sim = l.sim();
                        let who = sim.world.controlled();
                        match (who, who.and_then(|w| nearest_tagged(&sim, w, tag))) {
                            (Some(w), Some(t)) => {
                                sim.input(
                                    thedas_sim::commands::CommandSource::Player,
                                    thedas_sim::commands::SimCommand::OrderUse {
                                        who: w,
                                        target: t,
                                    },
                                );
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
                        l.sim().input(
                            thedas_sim::commands::CommandSource::Player,
                            thedas_sim::commands::SimCommand::SetTrainingMode { mode },
                        );
                        println!("training mode {mode}");
                    }
                    Step::Training(name) => {
                        let heard = l.sim().input(
                            thedas_sim::commands::CommandSource::Player,
                            thedas_sim::commands::SimCommand::TrainingEvent {
                                name: name.to_string(),
                                arg: 0,
                            },
                        );
                        println!("training {name}: {heard}");
                    }
                    Step::Plot(plot, flag) => {
                        l.sim().input(
                            thedas_sim::commands::CommandSource::App,
                            thedas_sim::commands::SimCommand::SetPlotFlag {
                                plot: plot.clone(),
                                flag: *flag,
                                value: true,
                                call_script: true,
                            },
                        );
                        println!("plot {plot} {flag}");
                    }
                    Step::Cutscene(name) => {
                        l.sim().input(
                            thedas_sim::commands::CommandSource::App,
                            thedas_sim::commands::SimCommand::QueueCutscene { name: name.clone() },
                        );
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
                    Step::SwapWeapons => {
                        l.switch_weapon_set();
                        println!("weapon set switched");
                    }
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
                            l.sim().input(
                                thedas_sim::commands::CommandSource::Player,
                                thedas_sim::commands::SimCommand::PlayerZoomedIn,
                            );
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
                            ui::FrontEnd::Quit
                            | ui::FrontEnd::Report
                            | ui::FrontEnd::ModMode
                            | ui::FrontEnd::Resume => {
                                continue;
                            }
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
    if args.mod_workspace {
        match workspace::mount(true) {
            Ok(note) => log::info!("{note}"),
            Err(e) => log::warn!("{e}"),
        }
    }
    let mut app = App {
        load_screen_up: false,
        loaded,
        live,
        follow,
        screens,
        ui,
        saves_dir: saves_dir.clone(),
        settings: args
            .settings
            .clone()
            .or_else(|| args.control.is_none().then(saves::default_settings)),
        cursor: (0.0, 0.0),
        hover_probe: None,
        all_names: false,
        key_walking: false,
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
        cursors: None,
        left_down: false,
        cursor_probe: None,
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
        recorder: report::Recorder::new(report::Recorder::default_dir()),
        report_wait: None,
        returning: match &args.check_report {
            Some(dir) => Returning::Asking(Box::new(UpdatePrompt {
                kind: UpdateAsk::Verdict(dir.clone(), update::verdict_prompt(dir)),
                asked: None,
            })),
            None => update::check_in_background(report::Recorder::default_dir())
                .map_or(Returning::Idle, Returning::Checking),
        },
        update_answers: Vec::new(),
        mark_answers: Vec::new(),
        resume_offer: ResumeOffer::Unchecked,
        mod_mode: None,
        live_edits: LiveEdits::default(),
        pad_last: false,
        started_with: json!({
            "size": args.size,
            "shaders": args.shaders,
            "detail": args.detail,
            "frame_effects": args.frame_effects,
            "language": args.language,
            "module": args.module,
            "install": report::install_info(&install, &args.language),
            "no_intro": args.no_intro,
            "mod_workspace": args.mod_workspace,
            "audio": !(args.no_audio || std::env::var_os("THEDAS_NO_AUDIO").is_some()),
            "control": args.control.is_some(),
            "env": (["THEDAS_ASYNC_LOADS", "THEDAS_HIDDEN_WINDOW", "THEDAS_NO_LOAD_MOVIE"]
                .iter()
                .filter(|v| std::env::var_os(v).is_some())
                .collect::<Vec<_>>()),
        }),
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
    /// Against a real install (`THEDAS_INSTALL`), offscreen: the EA logo's frame
    /// two seconds in is drawn letterboxed in a taller window (written to `$INTRO_PNG` when
    /// set, to look at).
    #[test]
    #[ignore]
    fn the_intro_draws_its_frame_letterboxed() {
        let install = PathBuf::from(std::env::var_os("THEDAS_INSTALL").expect("THEDAS_INSTALL"));
        let mut intro = intro::Intro::boot(&install).expect("the movies");
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
            parse_steps("key F10; modselect lot110cr_bryant; modframe 12; modmode").unwrap(),
            vec![
                Step::ModMode,
                Step::ModSelect("lot110cr_bryant".into()),
                Step::ModFrame(12),
                Step::ModMode,
            ]
        );
        assert!(parse_steps("modframe twelve").is_err());
        assert_eq!(
            parse_steps("modconv alistair_main; modstart; modflag gen00pt_party 3 1; modundo 0")
                .unwrap(),
            vec![
                Step::ModConv("alistair_main".into()),
                Step::ModStart,
                Step::ModFlag("gen00pt_party".into(), "3".into(), true),
                Step::ModUndo(0),
            ]
        );
        assert!(parse_steps("modflag gen00pt_party 3").is_err());
        assert_eq!(
            parse_steps("modmove 1,2.5,-3 90; modmoveview; modspawn creature c_wolf; modremove")
                .unwrap(),
            vec![
                Step::ModMove([1.0, 2.5, -3.0], 90.0),
                Step::ModMoveView,
                Step::ModSpawn(true, "c_wolf".into()),
                Step::ModRemove,
            ]
        );
        assert!(parse_steps("modspawn trigger x").is_err());
        assert!(parse_steps("modmove 1,2 0").is_err());
        assert_eq!(
            parse_steps(
                "modfly; modflyto; modworld; modplot plt_a; modlocal module X string a b; \
                 modlocal area Y remove"
            )
            .unwrap(),
            vec![
                Step::ModFly,
                Step::ModFlyTo,
                Step::ModWorld,
                Step::ModPlot("plt_a".into()),
                Step::ModLocal("module".into(), "X".into(), "string".into(), "a b".into()),
                Step::ModLocal("area".into(), "Y".into(), "remove".into(), "".into()),
            ]
        );
        assert!(parse_steps("modflag gen00pt_party 3 yes").is_err());
        assert_eq!(
            parse_steps("modflag plt_a SOME_FLAG 0").unwrap(),
            vec![Step::ModFlag("plt_a".into(), "SOME_FLAG".into(), false)]
        );
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
