//! The launcher: a small window shown when the game is started with no install (no
//! `--install`, no `THEDAS_INSTALL`), or with `--launcher`. On first run it is a wizard that
//! finds the player's Dragon Age installs (or hands them to Steam or GOG to get one) and asks
//! for a DLC keys file where one is needed. After that it lists the installs, edits the mod
//! list, keeps the play options, and starts the game.
//!
//! **Play** starts the game as a new process of this same executable with `--install` and the
//! options, then closes: winit allows one event loop per process. The logic (finding installs,
//! the settings file, the mod list, store links) is `thedas_launcher`'s; this is its window.
//! See `docs/launcher.md`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use egui::accesskit::Live;
use egui::{Color32, RichText};
use thedas_launcher::config::{GameKind, Install, LauncherConfig};
use thedas_launcher::detect::{self, Found, Inspected, Os};
use thedas_launcher::{ModFile, store};
use thedas_store::Store;

use crate::launcher_control::{Host, Need, Script};
use crate::launcher_download::{Download, Want};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::{Window, WindowId};

/// The emblem's gold and the dark red behind it.
pub(crate) const GOLD: Color32 = Color32::from_rgb(0xe8, 0xa8, 0x4c);
pub(crate) const GOLD_DIM: Color32 = Color32::from_rgb(0xb8, 0x80, 0x3c);
/// Height of the bottom bar (Back, status, primary button).
const FOOTER_HEIGHT: f32 = 56.0;
const BG: Color32 = Color32::from_rgb(0x26, 0x0d, 0x0b);
const BG_DEEP: Color32 = Color32::from_rgb(0x17, 0x07, 0x06);
pub(crate) const BAD: Color32 = Color32::from_rgb(0xff, 0x7a, 0x5c);

/// What starting the game takes.
#[derive(Clone, Debug, PartialEq)]
pub struct Launch {
    pub install: PathBuf,
    /// The game's other arguments (the play options).
    pub args: Vec<String>,
    /// The DLC keys file, handed over as `THEDAS_KEYS`.
    pub keys: Option<PathBuf>,
}

impl Launch {
    fn of(cfg: &LauncherConfig, install: &Install) -> Launch {
        let mut play = cfg.play.clone();
        // A language the install does not have would leave it silent.
        if let Some(i) = detect::inspect(&install.root) {
            if !i.languages.is_empty() && !i.languages.contains(&play.language) {
                play.language = if i.languages.iter().any(|l| l == "en-us") {
                    "en-us".into()
                } else {
                    i.languages[0].clone()
                };
            }
        }
        Launch {
            install: install.root.clone(),
            args: play.args(),
            keys: install.keys_file.clone(),
        }
    }

    /// Starts the game: this executable again, with the install and the options.
    pub fn spawn(&self) -> Result<()> {
        let exe = std::env::current_exe().context("finding this program")?;
        let mut cmd = std::process::Command::new(&exe);
        cmd.arg("--install").arg(&self.install).args(&self.args);
        if let Some(k) = &self.keys {
            cmd.env(thedas_launcher::KEYS_ENV, k);
        }
        log::info!(
            "launcher: starting {} --install {} {}",
            exe.display(),
            self.install.display(),
            self.args.join(" ")
        );
        cmd.spawn()
            .with_context(|| format!("starting {}", exe.display()))?;
        Ok(())
    }
}

/// Shows the launcher (or, when the player chose to skip it and its install is still there,
/// goes straight to the game) and returns what to start, or `None` when the player closed it.
pub fn run(force: bool, script: Option<Script>) -> Result<Option<Launch>> {
    let cfg_path = LauncherConfig::default_path();
    let cfg = LauncherConfig::load(&cfg_path);
    if !force && cfg.skip_launcher {
        if let Some(i) = cfg.selected_install() {
            if detect::game_at(&i.root).is_some() {
                return Ok(Some(Launch::of(&cfg, i)));
            }
            log::warn!(
                "launcher: {} is no longer a game install; showing the launcher",
                i.root.display()
            );
        }
    }
    let event_loop = EventLoop::<LauncherEvent>::with_user_event().build()?;
    let mut app = LauncherApp {
        ui: Ui::new(cfg, cfg_path),
        gfx: None,
        error: None,
        proxy: event_loop.create_proxy(),
        script,
    };
    event_loop.run_app(&mut app)?;
    if let Some(e) = app.error {
        return Err(e);
    }
    app.ui.save_config();
    if app.script.as_ref().is_some_and(|s| s.failed()) {
        anyhow::bail!("a launcher step failed");
    }
    Ok(app.ui.launch)
}

/// Draws the launcher's `page` (`welcome`, `search`, `keys`, `play`, `games`, `mods`,
/// `settings`) offscreen into a PNG, `size` pixels at `scale` pixels per point, with the
/// player's settings as they are (point `HOME` at a scratch folder to show others). For
/// screenshots and checks; no window opens. `search` runs the search first.
pub fn capture(out: &Path, page: &str, size: (u32, u32), scale: f32) -> Result<()> {
    let cfg_path = LauncherConfig::default_path();
    let mut ui = Ui::new(LauncherConfig::load(&cfg_path), cfg_path);
    ui.page = match page {
        "welcome" => Page::Welcome,
        "search" => Page::Search,
        "keys" => Page::Keys,
        "play" => Page::Play,
        "games" => Page::Games,
        "mods" => Page::Mods,
        "settings" => Page::Settings,
        "download" | "download-gog" | "download-progress" => Page::Download,
        other => anyhow::bail!("no launcher page {other}"),
    };
    if page == "download-gog" {
        ui.dl.preset(GameKind::Origins, Store::Gog);
    }
    let demo = page == "download-progress";
    if demo {
        ui.dl.demo_running();
    }
    if ui.page == Page::Search {
        ui.start_search(Page::Play);
        let started = Instant::now();
        while ui.search.is_some() && started.elapsed() < Duration::from_secs(120) {
            std::thread::sleep(Duration::from_millis(20));
            ui.poll_search();
        }
    }
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let gpu = pollster::block_on(thedas_render::GpuContext::new(instance, None))?;
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let ctx = egui::Context::default();
    style(&ctx);
    let mut renderer = egui_wgpu::Renderer::new(&gpu.device, format, Default::default());
    let (w, h) = size;
    let mut textures = egui::TexturesDelta::default();
    let mut shapes = None;
    // A few passes, so sizes measured in one pass lay out the next.
    for pass in 0..4 {
        let mut input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(w as f32 / scale, h as f32 / scale),
            )),
            time: Some(pass as f64 * 0.1),
            ..Default::default()
        };
        input
            .viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .native_pixels_per_point = Some(scale);
        ui.demo = demo;
        let output = ctx.run_ui(input, |eui| ui.draw(eui));
        textures.append(output.textures_delta);
        shapes = Some((
            ctx.tessellate(output.shapes, output.pixels_per_point),
            output.pixels_per_point,
        ));
    }
    ui.actions.clear();
    let (primitives, ppp) = shapes.context("nothing drawn")?;
    let bg = thedas_ui::Rgba {
        width: w,
        height: h,
        pixels: [BG.r(), BG.g(), BG.b(), 255].repeat((w * h) as usize),
    };
    let painted = crate::paint_over(&gpu, format, bg, |encoder, view| {
        for (id, deltas) in &textures.set {
            for delta in deltas {
                renderer.update_texture(&gpu.device, &gpu.queue, *id, delta);
            }
        }
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [w, h],
            pixels_per_point: ppp,
        };
        let extra = renderer.update_buffers(&gpu.device, &gpu.queue, encoder, &primitives, &screen);
        if !extra.is_empty() {
            gpu.queue.submit(extra);
        }
        let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("launcher capture"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        let mut pass = pass.forget_lifetime();
        renderer.render(&mut pass, &primitives, &screen);
    });
    textures.clear();
    let img = painted?;
    crate::write_png(out, w, h, &img.pixels)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    Welcome,
    Search,
    Keys,
    Play,
    Games,
    Mods,
    Settings,
    Download,
}

impl Page {
    fn wizard(self) -> bool {
        matches!(self, Page::Welcome | Page::Search | Page::Keys)
    }
}

/// What the page asked for, done after the frame (file dialogs block, so not mid-frame).
enum Action {
    PickInstall,
    PickKeys(usize),
    ClearKeys(usize),
    AddModFolder,
    AddModArchive,
    OpenUrl(String),
    OpenFolder(PathBuf),
    Search,
    Play,
    PickDownloadDest,
    /// The download page, set up for a game and store.
    Download(GameKind, Store),
}

/// One install the search found, looked over.
type Searched = (Found, Option<Inspected>);

struct Candidate {
    found: Found,
    inspected: Option<Inspected>,
    keep: bool,
    /// Already among the kept installs.
    known: bool,
}

struct Ui {
    cfg: LauncherConfig,
    cfg_path: PathBuf,
    saved_cfg: LauncherConfig,
    page: Page,
    /// The page the search returns to.
    after_search: Page,
    search: Option<Receiver<Vec<Searched>>>,
    candidates: Vec<Candidate>,
    inspected: HashMap<PathBuf, Option<Inspected>>,
    mods: ModFile,
    status: Option<(String, bool)>,
    actions: Vec<Action>,
    launch: Option<Launch>,
    emblem: Option<egui::TextureHandle>,
    os: Os,
    custom_size: String,
    dl: Download,
    /// The page the download page's Back returns to.
    before_download: Page,
    /// Screenshots only: draw the download page mid-download.
    demo: bool,
    /// What the next file or folder dialog answers, from a script (`choose`).
    chosen: Option<PathBuf>,
    /// The page drawn last frame, to see a page change (and move keyboard focus into the new
    /// page unless a navigation tab made it).
    drawn_page: Option<Page>,
    /// A navigation tab changed the page this frame: focus stays on the tab.
    nav_changed: bool,
    /// A search just ended: keyboard focus moves to the results (or what to do instead).
    refocus: bool,
    /// `THEDAS_LAUNCHER_DRY_RUN`: Play records what it would start and the launcher stays open
    /// (regression scripts check the command line).
    dry_run: bool,
    dry_launch: Option<Launch>,
}

const SIZES: &[&str] = &[
    "1280x720",
    "1600x900",
    "1920x1080",
    "2560x1440",
    "3840x2160",
];

impl Ui {
    fn new(cfg: LauncherConfig, cfg_path: PathBuf) -> Ui {
        let page = if cfg.wizard_done && !cfg.installs.is_empty() {
            Page::Play
        } else {
            Page::Welcome
        };
        let mods = ModFile::load(&mods_path().unwrap_or_else(|| {
            thedas_vfs::mods::user_data_dir().join(thedas_vfs::mods::LIST_FILE)
        }));
        let cfg_language = cfg.play.language.clone();
        Ui {
            custom_size: cfg.play.size.clone(),
            saved_cfg: cfg.clone(),
            cfg,
            cfg_path,
            page,
            after_search: Page::Play,
            search: None,
            candidates: Vec::new(),
            inspected: HashMap::new(),
            mods,
            status: None,
            actions: Vec::new(),
            launch: None,
            emblem: None,
            os: Os::current(),
            dl: Download::new(&cfg_language),
            before_download: Page::Welcome,
            demo: false,
            chosen: None,
            drawn_page: None,
            nav_changed: false,
            refocus: false,
            dry_run: std::env::var_os("THEDAS_LAUNCHER_DRY_RUN").is_some(),
            dry_launch: None,
        }
    }

    fn save_config(&mut self) {
        if self.cfg == self.saved_cfg {
            return;
        }
        match self.cfg.save(&self.cfg_path) {
            Ok(()) => self.saved_cfg = self.cfg.clone(),
            Err(e) => {
                self.status = Some((
                    format!("Could not save {}: {e}", self.cfg_path.display()),
                    true,
                ))
            }
        }
    }

    fn inspect(&mut self, root: &Path) -> Option<Inspected> {
        self.inspected
            .entry(root.to_path_buf())
            .or_insert_with(|| detect::inspect(root))
            .clone()
    }

    fn start_search(&mut self, back_to: Page) {
        let (tx, rx) = channel();
        let os = self.os;
        let home = std::env::var_os("HOME")
            .filter(|h| !h.is_empty())
            .map(PathBuf::from)
            .or_else(dirs::home_dir);
        std::thread::spawn(move || {
            let found = detect::search(os, home.as_deref());
            let out = found
                .into_iter()
                .map(|f| {
                    let i = detect::inspect(&f.root);
                    (f, i)
                })
                .collect();
            let _ = tx.send(out);
        });
        self.search = Some(rx);
        self.candidates.clear();
        self.after_search = back_to;
        self.page = Page::Search;
    }

    fn poll_search(&mut self) {
        let Some(rx) = &self.search else { return };
        match rx.try_recv() {
            Ok(found) => {
                self.candidates = found
                    .into_iter()
                    .map(|(found, inspected)| {
                        let known = self.cfg.installs.iter().any(|i| i.root == found.root);
                        Candidate {
                            keep: !known,
                            known,
                            found,
                            inspected,
                        }
                    })
                    .collect();
                self.search = None;
                self.refocus = true;
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.status = Some(("The search stopped unexpectedly.".into(), true));
                self.search = None;
            }
        }
    }

    fn add_root(&mut self, root: PathBuf, found: String) -> Option<usize> {
        let game = detect::game_at(&root)?;
        let i = self.cfg.add_install(Install {
            game: game.into(),
            root,
            found,
            keys_file: None,
        });
        if self.cfg.selected.is_none() {
            self.cfg.selected = Some(i);
        }
        Some(i)
    }

    /// Adds the search results the player left ticked.
    fn keep_candidates(&mut self) {
        let kept: Vec<Found> = self
            .candidates
            .iter()
            .filter(|c| c.keep && !c.known)
            .map(|c| c.found.clone())
            .collect();
        for f in kept {
            let label = format!("{}, {}", f.source.label(), f.place);
            self.add_root(f.root, label);
        }
    }

    /// Whether any kept install would want a keys file.
    fn keys_step_wanted(&mut self) -> bool {
        let roots: Vec<(GameKind, PathBuf)> = self
            .cfg
            .installs
            .iter()
            .map(|i| (i.game, i.root.clone()))
            .collect();
        roots
            .into_iter()
            .any(|(g, r)| g == GameKind::Da2 || self.inspect(&r).is_some_and(|i| i.exe.is_none()))
    }

    fn finish_wizard(&mut self) {
        self.cfg.wizard_done = true;
        self.page = Page::Play;
    }

    // ---- drawing ----

    fn draw(&mut self, ui: &mut egui::Ui) {
        if self.emblem.is_none() {
            if let Some((w, h, px)) = crate::icon::rgba() {
                let img = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &px);
                self.emblem = Some(ui.ctx().load_texture(
                    "emblem",
                    img,
                    egui::TextureOptions::LINEAR,
                ));
            }
        }
        egui::Panel::top("header")
            .frame(
                egui::Frame::new()
                    .fill(BG_DEEP)
                    .inner_margin(egui::Margin::symmetric(16, 10)),
            )
            .show(ui, |ui| self.header(ui));
        // The footer's room is set aside here, but its controls are made after the page's, so
        // Tab and screen readers go header, page, footer. (Panels must come before the
        // central panel to take their space.)
        let footer = egui::Panel::bottom("footer")
            .exact_size(FOOTER_HEIGHT)
            .resizable(false)
            .frame(egui::Frame::new().fill(BG_DEEP))
            .show(ui, |_| {});
        // A page change that wasn't a click on a navigation tab (the wizard's steps, a
        // finished download) moves keyboard focus to the new page's first control.
        let page_changed = self.drawn_page.is_some_and(|p| p != self.page);
        let from_nav = std::mem::take(&mut self.nav_changed);
        self.drawn_page = Some(self.page);
        if (page_changed && !from_nav) || std::mem::take(&mut self.refocus) {
            ui.memory_mut(|m| {
                if let Some(id) = m.focused() {
                    m.surrender_focus(id);
                }
                m.move_focus(egui::FocusDirection::Next);
            });
        }
        egui::CentralPanel::default_margins()
            .frame(
                egui::Frame::new()
                    .fill(BG)
                    .inner_margin(egui::Margin::symmetric(24, 16)),
            )
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| match self.page {
                        Page::Welcome => self.welcome(ui),
                        Page::Search => self.search_page(ui),
                        Page::Keys => self.keys_page(ui),
                        Page::Play => self.play_page(ui),
                        Page::Games => self.games_page(ui),
                        Page::Mods => self.mods_page(ui),
                        Page::Settings => self.settings_page(ui),
                        Page::Download if self.demo => self.dl.draw_demo(ui),
                        Page::Download => {
                            if let Some(w) = self.dl.draw(ui) {
                                self.download_wants(w);
                            }
                        }
                    });
            });
        let rect = footer.response.rect.shrink2(egui::vec2(16.0, 10.0));
        ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| self.footer(ui));
    }

    fn header(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if let Some(t) = &self.emblem {
                ui.add(
                    egui::Image::new(egui::load::SizedTexture::new(t.id(), [56.0, 56.0]))
                        .alt_text("OpenThedas emblem"),
                );
            }
            ui.vertical(|ui| {
                ui.label(RichText::new("OpenThedas").size(28.0).strong().color(GOLD));
                ui.label(RichText::new("Dragon Age on an open engine").color(GOLD_DIM));
            });
            if !self.page.wizard() {
                // Right-aligned but made left to right, so Tab and screen readers go Play,
                // Games, Mods, Settings as they read. The row's width is what the last
                // frame measured.
                let id = egui::Id::unique("nav-width");
                let width: f32 = ui.data(|d| d.get_temp(id)).unwrap_or(0.0);
                ui.add_space((ui.available_width() - width).max(0.0));
                let row = ui.horizontal(|ui| {
                    for (page, name) in [
                        (Page::Play, "Play"),
                        (Page::Games, "Games"),
                        (Page::Mods, "Mods"),
                        (Page::Settings, "Settings"),
                    ] {
                        let text = RichText::new(name).size(18.0);
                        if ui.selectable_label(self.page == page, text).clicked() {
                            self.page = page;
                            self.nav_changed = true;
                        }
                    }
                });
                let measured = row.response.rect.width();
                if (measured - width).abs() > 0.5 {
                    ui.data_mut(|d| d.insert_temp(id, measured));
                    ui.ctx().request_repaint();
                }
            }
        });
    }

    fn footer(&mut self, ui: &mut egui::Ui) {
        // Back first (left), then the message, then the page's main button at the right: Tab
        // goes Back, then forward.
        let back = match self.page {
            Page::Search if self.cfg.wizard_done => Some(self.after_search),
            Page::Search => Some(Page::Welcome),
            Page::Keys => Some(Page::Search),
            Page::Download => Some(self.before_download),
            _ => None,
        };
        ui.horizontal(|ui| {
            if let Some(to) = back {
                if ui.button("Back").clicked() {
                    self.page = to;
                }
            }
            if let Some((msg, bad)) = &self.status {
                let r = ui.label(RichText::new(msg).color(if *bad { BAD } else { GOLD_DIM }));
                live(&r, if *bad { Live::Assertive } else { Live::Polite });
            }
            ui.with_layout(
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| match self.page {
                    Page::Welcome => {
                        if !self.cfg.installs.is_empty() && ui.button("Skip setup").clicked() {
                            self.finish_wizard();
                        }
                    }
                    Page::Search => {
                        let any =
                            self.candidates.iter().any(|c| c.keep) || !self.cfg.installs.is_empty();
                        if ui
                            .add_enabled(any && self.search.is_none(), big_button("Continue"))
                            .clicked()
                        {
                            self.keep_candidates();
                            if self.cfg.wizard_done {
                                self.page = self.after_search;
                            } else if self.keys_step_wanted() {
                                self.page = Page::Keys;
                            } else {
                                self.finish_wizard();
                            }
                        }
                    }
                    Page::Keys => {
                        if ui.add(big_button("Finish")).clicked() {
                            self.finish_wizard();
                        }
                    }
                    Page::Download => {}
                    _ => {
                        let label = match self.cfg.selected_install() {
                            Some(i) => format!("Play {}", i.game.name()),
                            None => "Play".to_string(),
                        };
                        if ui
                            .add_enabled(self.cfg.selected_install().is_some(), big_button(&label))
                            .clicked()
                        {
                            self.actions.push(Action::Play);
                        }
                    }
                },
            );
        });
    }

    fn welcome(&mut self, ui: &mut egui::Ui) {
        ui.add_space(12.0);
        heading(ui, "Welcome", 26.0);
        ui.add_space(8.0);
        ui.label(
            "OpenThedas plays Dragon Age: Origins with your own copy of the game. It reads \
             the game's files where they are and never changes them.",
        );
        ui.add_space(6.0);
        let where_ = match self.os {
            Os::Windows => "Steam, GOG, the EA app and the usual install folders on every drive",
            Os::MacOs => {
                "Steam, the usual folders, and CrossOver, Whisky and Heroic bottles \
                 (the game is a Windows game, so on a Mac it usually lives in one)"
            }
            Os::Linux => "Steam (Proton), and Wine, Lutris, Heroic and Bottles prefixes",
        };
        ui.label(format!(
            "First, let's find your copy. The search looks in {where_}."
        ));
        ui.add_space(18.0);
        ui.horizontal(|ui| {
            if ui.add(big_button("Search this computer")).clicked() {
                self.actions.push(Action::Search);
            }
            if ui.button("Pick the game's folder myself…").clicked() {
                self.actions.push(Action::PickInstall);
            }
        });
        if !self.cfg.installs.is_empty() {
            ui.add_space(12.0);
            ui.label(format!(
                "Already added: {}",
                self.cfg
                    .installs
                    .iter()
                    .map(|i| i.game.name())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        ui.add_space(24.0);
        self.get_the_game(ui, true);
    }

    fn search_page(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        if self.search.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                let r = ui.label(RichText::new("Searching…").size(20.0));
                live(&r, Live::Polite);
            });
            return;
        }
        if self.candidates.is_empty() {
            heading(ui, "No copy of Dragon Age was found.", 22.0);
            ui.add_space(6.0);
            ui.label(
                "If you have the game somewhere the search doesn't look, pick its folder. \
                 Otherwise, get it from a store below, then search again.",
            );
        } else {
            heading(
                ui,
                &format!(
                    "Found {} install{}",
                    self.candidates.len(),
                    if self.candidates.len() == 1 { "" } else { "s" }
                ),
                22.0,
            );
            ui.label("Tick the ones to keep. You can add or remove them later under Games.");
            ui.add_space(8.0);
            for c in &mut self.candidates {
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.add_enabled_ui(!c.known, |ui| {
                            let name = format!(
                                "Keep {}, {}",
                                GameKind::from(c.found.game).name(),
                                tilde(&c.found.root)
                            );
                            let r = ui.checkbox(&mut c.keep, "Keep");
                            let (enabled, keep) = (r.enabled(), c.keep);
                            r.widget_info(|| {
                                egui::WidgetInfo::selected(
                                    egui::accesskit::Role::CheckBox,
                                    enabled,
                                    keep,
                                    &name,
                                )
                            });
                        });
                        ui.vertical(|ui| {
                            let game: GameKind = c.found.game.into();
                            ui.label(RichText::new(game.name()).strong().size(18.0));
                            let mut facts =
                                vec![format!("{} ({})", c.found.source.label(), c.found.place)];
                            if let Some(i) = &c.inspected {
                                facts.extend(describe(i));
                            }
                            if c.known {
                                facts.push("already added".into());
                            }
                            ui.label(facts.join(" · "));
                            ui.label(RichText::new(tilde(&c.found.root)).small().color(GOLD_DIM));
                        });
                    });
                });
            }
        }
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if ui.button("Pick a folder…").clicked() {
                self.actions.push(Action::PickInstall);
            }
            if ui.button("Search again").clicked() {
                self.actions.push(Action::Search);
            }
        });
        if self.candidates.is_empty() {
            ui.add_space(18.0);
            self.get_the_game(ui, true);
        }
    }

    fn keys_page(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        heading(ui, "Downloadable content", 22.0);
        ui.label(
            "Some DLC archives are encrypted. Origins' passwords come from your own \
             DAOrigins.exe. Dragon Age II needs a keys file (the erfpasswords.txt format). \
             The base game plays without one.",
        );
        ui.add_space(10.0);
        self.installs_keys(ui, false);
    }

    /// A card per install: where it is, what its DLC needs, its keys file; with `manage`, also
    /// choosing, opening and removing it.
    fn installs_keys(&mut self, ui: &mut egui::Ui, manage: bool) {
        let mut remove = None;
        for idx in 0..self.cfg.installs.len() {
            let inst = self.cfg.installs[idx].clone();
            let info = self.inspect(&inst.root);
            let whose = format!("{} ({})", inst.game.name(), inst.found);
            egui::Frame::group(ui.style()).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new(inst.game.name()).strong().size(18.0));
                ui.label(RichText::new(tilde(&inst.root)).small().color(GOLD_DIM));
                let has_exe = info.as_ref().is_some_and(|i| i.exe.is_some());
                match (inst.game, has_exe) {
                    (GameKind::Origins, true) => {
                        ui.label(
                            "DLC passwords come from your DAOrigins.exe. No keys file needed.",
                        );
                    }
                    (GameKind::Origins, false) => {
                        ui.label(
                            RichText::new(
                                "No bin_ship/DAOrigins.exe here, so encrypted DLC can't be \
                                 opened unless you give a keys file.",
                            )
                            .color(BAD),
                        );
                    }
                    (GameKind::Da2, _) => {
                        ui.label("Encrypted DLC loads only with a keys file.");
                    }
                }
                ui.horizontal(|ui| {
                    ui.label("Keys file:");
                    match &inst.keys_file {
                        Some(k) => {
                            let ok = k.is_file();
                            ui.label(RichText::new(tilde(k)).color(if ok { GOLD } else { BAD }));
                            if !ok {
                                ui.label(RichText::new("(missing)").color(BAD));
                            }
                        }
                        None => {
                            ui.label("none");
                        }
                    }
                    if named(
                        ui.button("Choose…"),
                        &format!("Choose a keys file for {whose}"),
                    )
                    .clicked()
                    {
                        self.actions.push(Action::PickKeys(idx));
                    }
                    if inst.keys_file.is_some()
                        && named(
                            ui.button("Clear"),
                            &format!("Clear the keys file for {whose}"),
                        )
                        .clicked()
                    {
                        self.actions.push(Action::ClearKeys(idx));
                    }
                });
                if manage {
                    ui.horizontal(|ui| {
                        if self.cfg.selected == Some(idx) {
                            ui.label(RichText::new("Plays when you press Play").color(GOLD));
                        } else if named(ui.button("Play this one"), &format!("Play {whose}"))
                            .clicked()
                        {
                            self.cfg.selected = Some(idx);
                        }
                        if named(
                            ui.button("Open folder"),
                            &format!("Open the folder of {whose}"),
                        )
                        .clicked()
                        {
                            self.actions.push(Action::OpenFolder(inst.root.clone()));
                        }
                        if named(
                            ui.button("Remove")
                                .on_hover_text("Forget it here; nothing is deleted"),
                            &format!("Remove {whose}"),
                        )
                        .clicked()
                        {
                            remove = Some(idx);
                        }
                    });
                }
            });
        }
        if let Some(i) = remove {
            self.cfg.remove_install(i);
        }
    }

    fn play_page(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        if self.cfg.installs.is_empty() {
            heading(ui, "No game added yet.", 22.0);
            if ui.add(big_button("Set up")).clicked() {
                self.page = Page::Welcome;
            }
            return;
        }
        heading(ui, "Choose a game", 22.0);
        ui.add_space(6.0);
        for idx in 0..self.cfg.installs.len() {
            let inst = self.cfg.installs[idx].clone();
            let info = self.inspect(&inst.root);
            let selected = self.cfg.selected == Some(idx);
            let frame = egui::Frame::group(ui.style()).stroke(egui::Stroke::new(
                if selected { 2.0 } else { 1.0 },
                if selected {
                    GOLD
                } else {
                    GOLD_DIM.gamma_multiply(0.4)
                },
            ));
            let mut picked = false;
            let resp = frame
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        let radio = ui.radio(selected, "");
                        // Named for screen readers and scripts: the game and where it was found.
                        let name = format!("{} ({})", inst.game.name(), inst.found);
                        radio.widget_info(|| {
                            egui::WidgetInfo::selected(
                                egui::accesskit::Role::RadioButton,
                                true,
                                selected,
                                &name,
                            )
                        });
                        if radio.clicked() {
                            picked = true;
                        }
                        ui.vertical(|ui| {
                            ui.label(RichText::new(inst.game.name()).strong().size(18.0));
                            let mut facts = Vec::new();
                            match &info {
                                Some(i) => facts.extend(describe(i)),
                                None => facts.push("not found any more".into()),
                            }
                            if !inst.found.is_empty() {
                                facts.push(inst.found.clone());
                            }
                            ui.label(facts.join(" · "));
                            ui.label(RichText::new(tilde(&inst.root)).small().color(GOLD_DIM));
                        });
                    });
                })
                .response
                .interact(egui::Sense::click());
            if picked || resp.clicked() {
                self.cfg.selected = Some(idx);
            }
        }
        ui.add_space(12.0);
        let rows = self.mods.rows();
        let on = rows.iter().filter(|r| r.on).count();
        ui.horizontal(|ui| {
            ui.label(format!(
                "Mods: {on} on{}",
                if rows.len() > on {
                    format!(", {} off", rows.len() - on)
                } else {
                    String::new()
                }
            ));
            if ui.small_button("Manage").clicked() {
                self.page = Page::Mods;
            }
        });
        if self.mods.dirty() {
            ui.label(
                RichText::new(
                    "The mod list has changes that are not saved; the game reads the saved list.",
                )
                .color(BAD),
            );
        }
        if self
            .cfg
            .selected_install()
            .is_some_and(|i| i.game == GameKind::Da2)
        {
            ui.label(
                RichText::new("Dragon Age II support is early work: expect missing pieces.")
                    .color(GOLD_DIM),
            );
        }
    }

    fn games_page(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        heading(ui, "Your games", 22.0);
        ui.add_space(6.0);
        if self.cfg.installs.is_empty() {
            ui.label("None yet.");
        }
        self.installs_keys(ui, true);
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button("Search again").clicked() {
                self.actions.push(Action::Search);
            }
            if ui.button("Add a folder…").clicked() {
                self.actions.push(Action::PickInstall);
            }
        });
        ui.add_space(12.0);
        self.get_the_game(ui, false);
    }

    fn mods_page(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        heading(ui, "Mods", 22.0);
        if let Some(v) = std::env::var_os(thedas_vfs::mods::ENV_VAR) {
            ui.label(
                RichText::new(format!(
                    "THEDAS_MODS is set ({}), so the game reads that instead of your list.",
                    v.to_string_lossy()
                ))
                .color(BAD),
            );
        }
        ui.label(
            "Mods load in this order: a mod lower in the list wins over the ones above it. \
             The order only changes when you move a mod.",
        );
        ui.label(
            RichText::new(format!("List: {}", tilde(&self.mods.path)))
                .small()
                .color(GOLD_DIM),
        );
        if let Some(dir) = thedas_vfs::mods::original_user_dir(thedas_vfs::Game::Origins)
            .and_then(|d| thedas_vfs::resolve_ci(&d, "packages/core/override"))
            .filter(|d| d.is_dir())
        {
            ui.label(
                RichText::new(format!(
                    "Also loaded, below every mod here: {}",
                    tilde(&dir)
                ))
                .small()
                .color(GOLD_DIM),
            );
        }
        ui.add_space(8.0);
        let rows = self.mods.rows();
        if rows.is_empty() {
            ui.label("No mods yet.");
        }
        let n = rows.len();
        let mut change: Option<(usize, u8)> = None;
        for (i, r) in rows.iter().enumerate() {
            ui.horizontal(|ui| {
                let mut on = r.on;
                if ui
                    .checkbox(&mut on, "")
                    .on_hover_text("Load this mod")
                    .changed()
                {
                    change = Some((i, if on { 1 } else { 0 }));
                }
                if ui
                    .add_enabled(i > 0, egui::Button::new("Up").small())
                    .on_hover_text("Load earlier: mods below it win over it")
                    .clicked()
                {
                    change = Some((i, 2));
                }
                if ui
                    .add_enabled(i + 1 < n, egui::Button::new("Down").small())
                    .on_hover_text("Load later: it wins over the mods above it")
                    .clicked()
                {
                    change = Some((i, 3));
                }
                if ui
                    .small_button("Remove")
                    .on_hover_text("Take it off the list; nothing is deleted")
                    .clicked()
                {
                    change = Some((i, 4));
                }
                let text = RichText::new(mod_name(&r.text, &r.path)).size(17.0);
                ui.label(if r.on {
                    text
                } else {
                    text.color(GOLD_DIM).strikethrough()
                })
                .on_hover_text(tilde(&r.path));
                if !r.exists {
                    ui.label(RichText::new("missing").color(BAD));
                }
            });
        }
        match change {
            Some((i, 0)) => self.mods.set_on(i, false),
            Some((i, 1)) => self.mods.set_on(i, true),
            Some((i, 2)) => self.mods.move_up(i),
            Some((i, 3)) => self.mods.move_down(i),
            Some((i, 4)) => self.mods.remove(i),
            _ => {}
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button("Add a folder…").clicked() {
                self.actions.push(Action::AddModFolder);
            }
            if ui.button("Add an archive…").clicked() {
                self.actions.push(Action::AddModArchive);
            }
        });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let dirty = self.mods.dirty();
            if ui.add_enabled(dirty, big_button("Save")).clicked() {
                match self.mods.save() {
                    Ok(()) => self.status = Some(("Mod list saved.".into(), false)),
                    Err(e) => {
                        self.status = Some((format!("Could not save the mod list: {e}"), true))
                    }
                }
            }
            if ui
                .add_enabled(dirty, egui::Button::new("Undo changes"))
                .clicked()
            {
                self.mods.revert();
            }
            if dirty {
                ui.label(RichText::new("Not saved yet").color(BAD));
            }
        });
    }

    fn settings_page(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        heading(ui, "Settings", 22.0);
        ui.add_space(6.0);
        let languages = self
            .cfg
            .selected_install()
            .map(|i| i.root.clone())
            .and_then(|r| self.inspect(&r))
            .map(|i| i.languages)
            .unwrap_or_default();
        let play = &mut self.cfg.play;
        egui::Grid::new("settings")
            .num_columns(2)
            .spacing([16.0, 10.0])
            .show(ui, |ui| {
                let label = ui.label("Window size");
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt("size")
                        .selected_text(play.size.clone())
                        .show_ui(ui, |ui| {
                            for s in SIZES {
                                if ui.selectable_label(play.size == *s, *s).clicked() {
                                    play.size = s.to_string();
                                    self.custom_size = play.size.clone();
                                }
                            }
                        })
                        .response
                        .labelled_by(label.id);
                    let r = ui
                        .add(egui::TextEdit::singleline(&mut self.custom_size).desired_width(110.0))
                        .labelled_by(label.id);
                    if r.lost_focus() {
                        if valid_size(&self.custom_size) {
                            play.size = self.custom_size.trim().to_string();
                        } else {
                            self.custom_size = play.size.clone();
                        }
                    }
                });
                ui.end_row();

                ui.label("Graphics detail");
                ui.horizontal(|ui| {
                    for d in ["low", "medium", "high"] {
                        ui.radio_value(&mut play.detail, d.to_string(), capitalise(d));
                    }
                });
                ui.end_row();

                ui.label("Frame-buffer effects");
                ui.checkbox(
                    &mut play.frame_effects,
                    "Bloom and the other screen effects",
                );
                ui.end_row();

                ui.label("Renderer");
                ui.horizontal(|ui| {
                    ui.radio_value(&mut play.renderer, "faithful".to_string(), "Original")
                        .on_hover_text("The game's own shaders and lighting (the reference)");
                    ui.radio_value(&mut play.renderer, "deferred".to_string(), "Deferred")
                        .on_hover_text("OpenThedas's own per-pixel lighting; F11 switches in game");
                });
                ui.end_row();

                ui.label("Intro movies");
                ui.checkbox(&mut play.intro, "Play the logos and intro before the menu");
                ui.end_row();

                ui.label("Sound");
                ui.checkbox(&mut play.audio, "On");
                ui.end_row();

                let label = ui.label("Voice language");
                egui::ComboBox::from_id_salt("language")
                    .selected_text(play.language.clone())
                    .show_ui(ui, |ui| {
                        let mut list = languages.clone();
                        if !list.contains(&play.language) {
                            list.insert(0, play.language.clone());
                        }
                        for l in list {
                            if ui.selectable_label(play.language == l, &l).clicked() {
                                play.language = l;
                            }
                        }
                    })
                    .response
                    .labelled_by(label.id);
                ui.end_row();
            });
        ui.add_space(12.0);
        ui.checkbox(
            &mut self.cfg.skip_launcher,
            "Start the game directly next time, without the launcher",
        );
        ui.label(
            RichText::new("Start with --launcher to see the launcher again.")
                .small()
                .color(GOLD_DIM),
        );
        ui.add_space(12.0);
        let data = thedas_vfs::mods::user_data_dir();
        ui.horizontal(|ui| {
            ui.label(format!("Saves, settings and mods list: {}", tilde(&data)));
            if named(
                ui.small_button("Open"),
                "Open the saves and settings folder",
            )
            .clicked()
            {
                self.actions.push(Action::OpenFolder(data.clone()));
            }
        });
    }

    fn get_the_game(&mut self, ui: &mut egui::Ui, open: bool) {
        egui::CollapsingHeader::new(RichText::new("Don't have the game?").size(18.0).color(GOLD))
            .default_open(open)
            .show(ui, |ui| {
                ui.label(
                    "OpenThedas needs a copy of the game you own. Download it here with your \
                     Steam or GOG account (the sign-in is never saved), or use the store's own \
                     app or page and search again once it's installed.",
                );
                for game in [GameKind::Origins, GameKind::Da2] {
                    ui.add_space(6.0);
                    ui.label(RichText::new(game.name()).strong());
                    let links = store::links(game, self.os);
                    ui.horizontal_wrapped(|ui| {
                        for st in [Store::Steam, Store::Gog] {
                            if st.sells(game)
                                && named(
                                    ui.add(big_button(&format!("Download from {}", st.name()))),
                                    &format!("Download {} from {}", game.name(), st.name()),
                                )
                                .clicked()
                            {
                                self.actions.push(Action::Download(game, st));
                            }
                        }
                        for l in &links {
                            if named(
                                ui.button(&l.label),
                                &format!("{} ({})", l.label, game.name()),
                            )
                            .clicked()
                            {
                                self.actions.push(Action::OpenUrl(l.url.clone()));
                            }
                        }
                    });
                    let mut notes: Vec<&str> = links
                        .iter()
                        .map(|l| l.note.as_str())
                        .filter(|n| !n.is_empty())
                        .collect();
                    notes.dedup();
                    // The notes are about the store's own app; a download here needs none of it.
                    for n in notes {
                        ui.label(
                            RichText::new(format!("With the store's own app: {n}"))
                                .small()
                                .color(GOLD_DIM),
                        );
                    }
                }
            });
    }

    fn download_wants(&mut self, w: Want) {
        match w {
            Want::OpenUrl(url) => self.actions.push(Action::OpenUrl(url)),
            Want::PickDest => self.actions.push(Action::PickDownloadDest),
            Want::Installed(root, st) => {
                match self.add_root(root.clone(), format!("Downloaded from {}", st.name())) {
                    Some(i) => {
                        self.cfg.selected = Some(i);
                        let name = self.cfg.installs[i].game.name();
                        self.status = Some((format!("{name} is downloaded and ready."), false));
                        if !self.cfg.wizard_done && self.keys_step_wanted() {
                            self.page = Page::Keys;
                        } else {
                            self.finish_wizard();
                        }
                        self.save_config();
                    }
                    None => {
                        self.status = Some((
                            format!(
                                "The download finished, but {} doesn't look like a game install.",
                                root.display()
                            ),
                            true,
                        ))
                    }
                }
            }
        }
    }

    /// Whether the window should keep redrawing: a search or a download is running.
    fn busy(&self) -> bool {
        self.search.is_some() || self.dl.running()
    }

    // ---- actions ----

    /// A file (or with `folder`, a folder) from `dialog`, or what a script chose instead.
    fn pick(&mut self, dialog: rfd::FileDialog, folder: bool) -> Option<PathBuf> {
        if let Some(p) = self.chosen.take() {
            return Some(p);
        }
        if folder {
            dialog.pick_folder()
        } else {
            dialog.pick_file()
        }
    }

    fn act(&mut self) {
        for action in std::mem::take(&mut self.actions) {
            match action {
                Action::Search => {
                    let back = if self.page.wizard() {
                        Page::Play
                    } else {
                        self.page
                    };
                    self.start_search(back);
                }
                Action::PickInstall => {
                    let Some(dir) = self.pick(
                        rfd::FileDialog::new().set_title("Choose the game's folder"),
                        true,
                    ) else {
                        continue;
                    };
                    match detect::install_root(&dir) {
                        Some(root) => {
                            let game: GameKind = match detect::game_at(&root) {
                                Some(g) => g.into(),
                                None => continue,
                            };
                            // Picking a folder in the wizard must not drop what the search found.
                            if self.page == Page::Search {
                                self.keep_candidates();
                            }
                            self.add_root(root, "Added by hand".into());
                            self.status = Some((format!("Added {}.", game.name()), false));
                            if self.page.wizard() {
                                if self.keys_step_wanted() {
                                    self.page = Page::Keys;
                                } else {
                                    self.finish_wizard();
                                }
                            }
                        }
                        None => {
                            self.status = Some((
                                format!(
                                    "{} doesn't hold a Dragon Age install (no packages/core).",
                                    dir.display()
                                ),
                                true,
                            ))
                        }
                    }
                }
                Action::PickKeys(i) => {
                    if let Some(f) = self.pick(
                        rfd::FileDialog::new()
                            .set_title("Choose a keys file (erfpasswords.txt format)")
                            .add_filter("Text", &["txt"])
                            .add_filter("All files", &["*"]),
                        false,
                    ) {
                        if let Some(inst) = self.cfg.installs.get_mut(i) {
                            inst.keys_file = Some(f);
                        }
                    }
                }
                Action::ClearKeys(i) => {
                    if let Some(inst) = self.cfg.installs.get_mut(i) {
                        inst.keys_file = None;
                    }
                }
                Action::AddModFolder => {
                    if let Some(d) = self.pick(
                        rfd::FileDialog::new().set_title("Choose a mod's folder"),
                        true,
                    ) {
                        self.mods.add(&d.to_string_lossy());
                    }
                }
                Action::AddModArchive => {
                    if let Some(f) = self.pick(
                        rfd::FileDialog::new()
                            .set_title("Choose a mod archive")
                            .add_filter(
                                "Mod archives",
                                &["zip", "dazip", "7z", "rar", "erf", "rim"],
                            ),
                        false,
                    ) {
                        self.mods.add(&f.to_string_lossy());
                    }
                }
                Action::OpenUrl(url) => {
                    if let Err(e) = store::open_url(&url) {
                        self.status = Some((format!("Could not open {url}: {e}"), true));
                    }
                }
                Action::OpenFolder(dir) => {
                    let _ = std::fs::create_dir_all(&dir);
                    if let Err(e) = store::open_folder(&dir) {
                        self.status = Some((format!("Could not open the folder: {e}"), true));
                    }
                }
                Action::Download(game, st) => {
                    self.dl.preset(game, st);
                    if self.page != Page::Download {
                        self.before_download = self.page;
                    }
                    self.page = Page::Download;
                }
                Action::PickDownloadDest => {
                    if let Some(d) = self.pick(
                        rfd::FileDialog::new().set_title("Choose where to put the game"),
                        true,
                    ) {
                        self.dl.dest = Some(d);
                    }
                }
                Action::Play => {
                    let Some(inst) = self.cfg.selected_install().cloned() else {
                        continue;
                    };
                    if detect::game_at(&inst.root).is_none() {
                        self.status = Some((
                            format!("{} is no longer a game install.", inst.root.display()),
                            true,
                        ));
                        continue;
                    }
                    let launch = Launch::of(&self.cfg, &inst);
                    if self.dry_run {
                        // For scripts: say what would start, and stay open.
                        self.status = Some((
                            format!("Would start the game from {}.", inst.root.display()),
                            false,
                        ));
                        self.dry_launch = Some(launch);
                    } else {
                        self.launch = Some(launch);
                    }
                }
            }
        }
        self.save_config();
    }
}

fn describe(i: &Inspected) -> Vec<String> {
    let mut v = Vec::new();
    if let Some(ver) = i.version {
        v.push(format!("version {ver:.2}"));
    }
    if i.addins > 0 {
        v.push(format!("{} DLC", i.addins));
    }
    if !i.languages.is_empty() {
        v.push(i.languages.join(", "));
    }
    v
}

/// A mod's name for the list: its file or folder name, and the folder picked inside an
/// archive (`teeth.zip › Pearly White`).
fn mod_name(line: &str, path: &Path) -> String {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| line.to_string());
    match line.split_once("!/") {
        Some((_, inner)) if !inner.is_empty() => format!("{name} › {inner}"),
        _ => name,
    }
}

/// A path for showing, with the home folder as `~`.
pub(crate) fn tilde(p: &Path) -> String {
    let home = std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
        .or_else(dirs::home_dir);
    match home.as_deref().and_then(|h| p.strip_prefix(h).ok()) {
        Some(rest) if !cfg!(windows) => format!("~/{}", rest.display()),
        _ => p.display().to_string(),
    }
}

/// Gives a button the name screen readers and scripts use, where its text alone is ambiguous
/// ("Remove" on every install's card).
fn named(r: egui::Response, name: &str) -> egui::Response {
    let enabled = r.enabled();
    r.widget_info(|| egui::WidgetInfo::labeled(egui::accesskit::Role::Button, enabled, name));
    r
}

/// A page title that screen readers announce as a heading.
pub(crate) fn heading(ui: &mut egui::Ui, text: &str, size: f32) -> egui::Response {
    let r = ui.label(RichText::new(text.to_string()).size(size).color(GOLD));
    r.widget_info(|| egui::WidgetInfo::labeled(egui::accesskit::Role::Heading, true, text));
    r
}

/// Marks a label as a live region: screen readers announce when its text changes.
pub(crate) fn live(r: &egui::Response, l: Live) {
    r.ctx.accesskit_node_builder(r.id, |b| b.set_live(l));
}

pub(crate) fn big_button(text: &str) -> egui::Button<'static> {
    egui::Button::new(
        RichText::new(text.to_string())
            .size(18.0)
            .strong()
            .color(BG_DEEP),
    )
    .fill(GOLD)
    .min_size(egui::vec2(140.0, 36.0))
}

fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

fn valid_size(s: &str) -> bool {
    crate::parse_size(s).is_ok()
}

/// The mod list the game reads, when it is a file (`THEDAS_MODS` may name another, or none).
fn mods_path() -> Option<PathBuf> {
    thedas_vfs::mods::default_list_file()
}

fn style(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = BG;
    v.extreme_bg_color = BG_DEEP;
    v.faint_bg_color = Color32::from_rgb(0x30, 0x12, 0x0f);
    v.selection.bg_fill = Color32::from_rgb(0x8a, 0x52, 0x1a);
    v.selection.stroke.color = GOLD;
    v.hyperlink_color = GOLD;
    v.widgets.inactive.weak_bg_fill = Color32::from_rgb(0x44, 0x1a, 0x14);
    v.widgets.inactive.bg_fill = Color32::from_rgb(0x44, 0x1a, 0x14);
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(0x5c, 0x24, 0x1a);
    v.widgets.hovered.bg_fill = Color32::from_rgb(0x5c, 0x24, 0x1a);
    v.widgets.active.weak_bg_fill = Color32::from_rgb(0x70, 0x2e, 0x1e);
    v.widgets.active.bg_fill = Color32::from_rgb(0x70, 0x2e, 0x1e);
    ctx.all_styles_mut(|s| {
        s.visuals = v.clone();
        use egui::{FontId, TextStyle};
        s.text_styles
            .insert(TextStyle::Body, FontId::proportional(16.0));
        s.text_styles
            .insert(TextStyle::Button, FontId::proportional(16.0));
        s.text_styles
            .insert(TextStyle::Small, FontId::proportional(13.0));
        s.spacing.item_spacing = egui::vec2(10.0, 8.0);
        s.spacing.button_padding = egui::vec2(10.0, 5.0);
    });
}

struct Gfx {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    device: wgpu::Device,
    queue: wgpu::Queue,
    ctx: egui::Context,
    state: egui_winit::State,
    renderer: egui_wgpu::Renderer,
}

/// The launcher window's own events: AccessKit's (a screen reader or UI Automation asking for
/// the tree, or acting on a control).
struct LauncherEvent(egui_winit::accesskit_winit::Event);

impl From<egui_winit::accesskit_winit::Event> for LauncherEvent {
    fn from(e: egui_winit::accesskit_winit::Event) -> Self {
        LauncherEvent(e)
    }
}

impl Host for Ui {
    fn working(&self) -> bool {
        self.busy()
    }

    fn status(&self) -> serde_json::Value {
        serde_json::json!({
            "page": format!("{:?}", self.page),
            "message": self.status.as_ref().map(|(m, _)| m.clone()),
            "message_is_error": self.status.as_ref().is_some_and(|(_, bad)| *bad),
            "searching": self.search.is_some(),
            "downloading": self.dl.running(),
            "wizard_done": self.cfg.wizard_done,
            "installs": self.cfg.installs.iter().map(|i| serde_json::json!({
                "game": i.game.name(),
                "root": i.root,
                "found": i.found,
                "keys_file": i.keys_file,
            })).collect::<Vec<_>>(),
            "selected": self.cfg.selected,
            "launch": self.launch.as_ref().or(self.dry_launch.as_ref()).map(|l| serde_json::json!({
                "install": l.install,
                "args": l.args,
            })),
        })
    }

    fn choose(&mut self, path: PathBuf) {
        self.chosen = Some(path);
    }
}

struct LauncherApp {
    ui: Ui,
    gfx: Option<Gfx>,
    error: Option<anyhow::Error>,
    proxy: EventLoopProxy<LauncherEvent>,
    /// Steps or a control client driving the launcher (`--launcher-steps`, `--control`).
    script: Option<Script>,
}

impl LauncherApp {
    fn init(&mut self, event_loop: &ActiveEventLoop) -> Result<()> {
        let window = Arc::new(
            event_loop.create_window(
                // Hidden until AccessKit is attached: its adapter must exist before the
                // window is first shown.
                Window::default_attributes()
                    .with_visible(false)
                    .with_title("OpenThedas")
                    .with_inner_size(LogicalSize::new(960.0, 680.0))
                    .with_min_inner_size(LogicalSize::new(640.0, 480.0))
                    .with_window_icon(crate::icon::window_icon()),
            )?,
        );
        crate::icon::set_dock_icon();
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let surface = instance.create_surface(window.clone())?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: Some(&surface),
            ..Default::default()
        }))
        .context("no graphics adapter for the launcher")?;
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("launcher"),
                ..Default::default()
            }))?;
        let phys = window.inner_size();
        let mut config = surface
            .get_default_config(&adapter, phys.width.max(1), phys.height.max(1))
            .context("the window's surface is not supported")?;
        // egui blends in gamma space: a non-sRGB target shows its colours as given.
        let caps = surface.get_capabilities(&adapter);
        if let Some(f) = caps.formats.iter().find(|f| !f.is_srgb()) {
            config.format = *f;
        }
        config.present_mode = wgpu::PresentMode::AutoVsync;
        surface.configure(&device, &config);
        let ctx = egui::Context::default();
        style(&ctx);
        let state = egui_winit::State::new(
            ctx.clone(),
            egui::ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            None,
            Some(device.limits().max_texture_dimension_2d as usize),
        );
        let mut state = state;
        state.init_accesskit(event_loop, &window, self.proxy.clone());
        if self.script.is_some() {
            // A script reads the same tree a screen reader would; build it from the start.
            ctx.enable_accesskit();
        }
        window.set_visible(true);
        let renderer = egui_wgpu::Renderer::new(&device, config.format, Default::default());
        self.gfx = Some(Gfx {
            window,
            surface,
            config,
            device,
            queue,
            ctx,
            state,
            renderer,
        });
        Ok(())
    }

    fn redraw(&mut self, event_loop: &ActiveEventLoop) {
        let Some(g) = self.gfx.as_mut() else { return };
        self.ui.poll_search();
        if let Some(w) = self.ui.dl.poll() {
            self.ui.download_wants(w);
        }
        let mut input = g.state.take_egui_input(&g.window);
        let mut need = Need::Nothing;
        if let Some(script) = self.script.as_mut() {
            script.poll();
            need = script.before_frame(&mut input.events, &mut self.ui);
        }
        if matches!(need, Need::Quit) {
            event_loop.exit();
            return;
        }
        let ui = &mut self.ui;
        let mut output = g.ctx.run_ui(input, |eui| ui.draw(eui));
        if let Some(script) = self.script.as_mut() {
            script.after_frame(output.platform_output.accesskit_update.as_ref());
        }
        g.state
            .handle_platform_output(&g.window, std::mem::take(&mut output.platform_output));
        let ppp = output.pixels_per_point;
        let primitives = g.ctx.tessellate(output.shapes, ppp);
        let frame = match g.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f)
            | wgpu::CurrentSurfaceTexture::Suboptimal(f) => Some(f),
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                g.surface.configure(&g.device, &g.config);
                None
            }
            _ => None,
        };
        for (id, deltas) in &output.textures_delta.set {
            for delta in deltas {
                g.renderer.update_texture(&g.device, &g.queue, *id, delta);
            }
        }
        if matches!(need, Need::Screenshot) {
            let png = shot(g, &primitives, ppp);
            if let Some(script) = self.script.as_mut() {
                script.screenshot_taken(png);
            }
        }
        if let Some(frame) = frame {
            let view = frame
                .texture
                .create_view(&wgpu::TextureViewDescriptor::default());
            let mut encoder = g
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
            let screen = egui_wgpu::ScreenDescriptor {
                size_in_pixels: [g.config.width, g.config.height],
                pixels_per_point: ppp,
            };
            let extra =
                g.renderer
                    .update_buffers(&g.device, &g.queue, &mut encoder, &primitives, &screen);
            {
                let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("launcher"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        resolve_target: None,
                        depth_slice: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color {
                                r: BG.r() as f64 / 255.0,
                                g: BG.g() as f64 / 255.0,
                                b: BG.b() as f64 / 255.0,
                                a: 1.0,
                            }),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                });
                let mut pass = pass.forget_lifetime();
                g.renderer.render(&mut pass, &primitives, &screen);
            }
            g.queue.submit(extra.into_iter().chain([encoder.finish()]));
            g.window.pre_present_notify();
            g.queue.present(frame);
        }
        for id in &output.textures_delta.free {
            g.renderer.free_texture(id);
        }
        // Handled: egui checks, in debug builds, that no delta is dropped unseen.
        let mut done = output.textures_delta;
        done.clear();
        self.ui.act();
        if self.ui.launch.is_some() {
            event_loop.exit();
            return;
        }
        let soon = output
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .is_some_and(|v| v.repaint_delay < Duration::from_millis(100));
        if soon || self.ui.busy() || self.script.as_ref().is_some_and(|s| s.busy()) {
            g.window.request_redraw();
        }
    }
}

/// The launcher as drawn this frame, read back from an offscreen target, as a PNG.
fn shot(g: &mut Gfx, primitives: &[egui::ClippedPrimitive], ppp: f32) -> Result<Vec<u8>> {
    let (w, h) = (g.config.width, g.config.height);
    let tex = g.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("launcher shot"),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: g.config.format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
    let row = (w * 4).div_ceil(256) * 256;
    let buf = g.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("launcher shot readback"),
        size: (row * h) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = g
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    let screen = egui_wgpu::ScreenDescriptor {
        size_in_pixels: [w, h],
        pixels_per_point: ppp,
    };
    let extra = g
        .renderer
        .update_buffers(&g.device, &g.queue, &mut encoder, primitives, &screen);
    {
        let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("launcher shot"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: BG.r() as f64 / 255.0,
                        g: BG.g() as f64 / 255.0,
                        b: BG.b() as f64 / 255.0,
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        let mut pass = pass.forget_lifetime();
        g.renderer.render(&mut pass, primitives, &screen);
    }
    encoder.copy_texture_to_buffer(
        tex.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buf,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(h),
            },
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    g.queue.submit(extra.into_iter().chain([encoder.finish()]));
    let slice = buf.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    let _ = g.device.poll(wgpu::PollType::wait_indefinitely());
    rx.recv()??;
    let bgra = matches!(
        g.config.format,
        wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
    );
    let mut px = Vec::with_capacity((w * h * 4) as usize);
    {
        let data = slice
            .get_mapped_range()
            .map_err(|e| anyhow::anyhow!("reading the screenshot back: {e:?}"))?;
        for y in 0..h as usize {
            let line = &data[y * row as usize..][..w as usize * 4];
            for p in line.chunks_exact(4) {
                if bgra {
                    px.extend_from_slice(&[p[2], p[1], p[0], 255]);
                } else {
                    px.extend_from_slice(&[p[0], p[1], p[2], 255]);
                }
            }
        }
    }
    buf.unmap();
    let mut png = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut png, w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header()?.write_image_data(&px)?;
    }
    Ok(png)
}

impl ApplicationHandler<LauncherEvent> for LauncherApp {
    fn user_event(&mut self, _: &ActiveEventLoop, event: LauncherEvent) {
        use egui_winit::accesskit_winit::WindowEvent as A;
        let Some(g) = self.gfx.as_mut() else { return };
        match event.0.window_event {
            A::InitialTreeRequested => g.ctx.enable_accesskit(),
            A::ActionRequested(request) => g.state.on_accesskit_action_request(request),
            A::AccessibilityDeactivated => {
                // A script keeps reading the tree after a screen reader stops.
                if self.script.is_none() {
                    g.ctx.disable_accesskit();
                }
            }
        }
        g.window.request_redraw();
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gfx.is_none() {
            if let Err(e) = self.init(event_loop) {
                self.error = Some(e);
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        let Some(g) = self.gfx.as_mut() else { return };
        let response = g.state.on_window_event(&g.window, &event);
        match event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
                return;
            }
            WindowEvent::Resized(size) => {
                g.config.width = size.width.max(1);
                g.config.height = size.height.max(1);
                g.surface.configure(&g.device, &g.config);
                g.window.request_redraw();
            }
            WindowEvent::RedrawRequested => {
                self.redraw(event_loop);
                return;
            }
            _ => {}
        }
        if response.repaint {
            g.window.request_redraw();
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // While a search or download runs, or a script drives the launcher, look for news now
        // and then.
        if self.ui.busy()
            || self
                .script
                .as_ref()
                .is_some_and(|s| s.busy() || s.has_control())
        {
            event_loop.set_control_flow(ControlFlow::WaitUntil(
                Instant::now() + Duration::from_millis(100),
            ));
            if let Some(g) = &self.gfx {
                g.window.request_redraw();
            }
        } else {
            event_loop.set_control_flow(ControlFlow::Wait);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_carries_options_and_keys() {
        let mut cfg = LauncherConfig::default();
        cfg.play.intro = false;
        let inst = Install {
            game: GameKind::Origins,
            root: "/nowhere/dao".into(),
            found: String::new(),
            keys_file: Some("/k/erfpasswords.txt".into()),
        };
        let l = Launch::of(&cfg, &inst);
        assert_eq!(l.install, PathBuf::from("/nowhere/dao"));
        assert!(l.args.iter().any(|a| a == "--no-intro"));
        assert_eq!(l.keys.as_deref(), Some(Path::new("/k/erfpasswords.txt")));
    }

    #[test]
    fn sizes_are_checked() {
        assert!(valid_size("1920x1080"));
        assert!(!valid_size("big"));
        assert!(!valid_size("0x0"));
        for s in SIZES {
            assert!(valid_size(s));
        }
    }
}
