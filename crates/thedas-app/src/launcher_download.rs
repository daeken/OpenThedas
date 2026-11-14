//! The launcher's Download page: sign in to Steam or GOG and download the game into a folder,
//! with `thedas_store` doing the work on its own thread. Sera's rule: credentials are never
//! saved and only used for the download; the page says so plainly. A typed password moves
//! straight into a [`Secret`] when the download starts, and the text field's own state (its
//! undo history) is wiped at the same moment.

use std::path::PathBuf;
use std::time::Instant;

use egui::{Color32, RichText};
use thedas_launcher::config::GameKind;
use thedas_store::{Event, Job, Login, Reply, Request, Secret, Store, gog, size_text};

use crate::launcher::{BAD, GOLD, GOLD_DIM, big_button, tilde};

/// Voice-over languages the stores offer, as the game names them.
const LANGUAGES: &[(&str, &str)] = &[
    ("en-us", "English"),
    ("fr-fr", "French"),
    ("de-de", "German"),
    ("it-it", "Italian"),
    ("es-es", "Spanish"),
    ("pl-pl", "Polish"),
    ("ru-ru", "Russian"),
    ("cs-cz", "Czech"),
    ("hu-hu", "Hungarian"),
];

/// What the page wants from the launcher, after the frame.
pub enum Want {
    OpenUrl(String),
    PickDest,
    /// The game finished downloading into this folder.
    Installed(PathBuf, Store),
}

pub struct Download {
    pub game: GameKind,
    pub store: Store,
    steam_qr: bool,
    account: String,
    password: String,
    gog_pasted: String,
    guard: String,
    pub dest: Option<PathBuf>,
    pub language: String,
    job: Option<Job>,
    run: Run,
}

/// What the running (or last) download has said.
#[derive(Default)]
struct Run {
    status: String,
    qr: Option<(String, Option<egui::TextureHandle>)>,
    guard: Option<bool>,
    wrong_guard: bool,
    mobile: bool,
    signed_in: bool,
    plan: Option<(u64, u64)>,
    progress: (u64, u64),
    /// When the bytes started moving, and how many were already there (a resumed download).
    started: Option<(Instant, u64)>,
    done: Option<PathBuf>,
    failed: Option<String>,
    cancelled: bool,
}

const PASSWORD_ID: &str = "dl-password";
const GUARD_ID: &str = "dl-guard";
const GOG_ID: &str = "dl-gog";

impl Download {
    pub fn new(language: &str) -> Download {
        let game = GameKind::Origins;
        Download {
            game,
            store: Store::Steam,
            steam_qr: true,
            account: String::new(),
            password: String::new(),
            gog_pasted: String::new(),
            guard: String::new(),
            dest: thedas_store::default_dest(game),
            language: if LANGUAGES.iter().any(|(l, _)| *l == language) {
                language.to_string()
            } else {
                "en-us".into()
            },
            job: None,
            run: Run::default(),
        }
    }

    /// Sets up the page for `game` from `store` (from a "Download" button elsewhere).
    pub fn preset(&mut self, game: GameKind, store: Store) {
        if self.running() {
            return;
        }
        if self.game != game {
            self.dest = thedas_store::default_dest(game);
        }
        self.game = game;
        self.store = if store.sells(game) {
            store
        } else {
            Store::Steam
        };
    }

    /// Whether a download is running (the window keeps redrawing while it is).
    pub fn running(&self) -> bool {
        self.job.is_some()
    }

    /// Reads what the download said since the last frame.
    pub fn poll(&mut self) -> Option<Want> {
        let job = self.job.as_ref()?;
        let mut want = None;
        for e in job.poll() {
            let r = &mut self.run;
            match e {
                Event::Status(s) => r.status = s,
                Event::SteamQr(url) => r.qr = Some((url, None)),
                Event::NeedGuardCode { email } => {
                    r.guard = Some(email);
                    r.wrong_guard = false;
                }
                Event::WrongGuardCode => r.wrong_guard = true,
                Event::NeedMobileConfirm => r.mobile = true,
                Event::SignedIn => {
                    r.signed_in = true;
                    r.qr = None;
                    r.guard = None;
                    r.mobile = false;
                }
                Event::Plan { bytes, files } => r.plan = Some((bytes, files)),
                Event::Progress { done, total } => {
                    if r.started.is_none() {
                        r.started = Some((Instant::now(), done));
                    }
                    r.progress = (done, total);
                }
                Event::Done { root } => {
                    want = Some(Want::Installed(root.clone(), job.store()));
                    r.done = Some(root);
                }
                Event::Failed(e) => r.failed = Some(e),
                Event::Cancelled => r.cancelled = true,
            }
        }
        let r = &self.run;
        if r.done.is_some() || r.failed.is_some() || r.cancelled {
            self.job = None;
        }
        want
    }

    fn ready(&self) -> bool {
        let signed = match self.store {
            Store::Steam => {
                self.steam_qr || (!self.account.trim().is_empty() && !self.password.is_empty())
            }
            Store::Gog => gog::code_from(&self.gog_pasted).is_some(),
        };
        signed && self.dest.is_some() && self.store.sells(self.game)
    }

    fn start(&mut self, ctx: &egui::Context) {
        let Some(dest) = self.dest.clone() else {
            return;
        };
        let login = match self.store {
            Store::Steam if self.steam_qr => Login::SteamQr,
            Store::Steam => Login::SteamPassword {
                account: self.account.trim().to_string(),
                password: Secret::new(std::mem::take(&mut self.password)),
            },
            Store::Gog => match gog::code_from(&std::mem::take(&mut self.gog_pasted)) {
                Some(code) => Login::Gog { code },
                None => return,
            },
        };
        forget_fields(ctx);
        self.run = Run {
            status: "Starting".into(),
            ..Default::default()
        };
        self.job = Some(Job::start(
            Request {
                game: self.game,
                dest,
                language: self.language.clone(),
            },
            login,
        ));
    }

    // ---- drawing ----

    pub fn draw(&mut self, ui: &mut egui::Ui) -> Option<Want> {
        let mut want = None;
        ui.add_space(8.0);
        ui.label(RichText::new("Download the game").size(24.0).color(GOLD));
        ui.add_space(4.0);
        notice(ui, self.store);
        ui.add_space(10.0);
        if self.running() || self.run.done.is_some() || self.run.failed.is_some() {
            self.progress(ui);
            if self.running() {
                return want;
            }
            ui.add_space(14.0);
        }

        egui::Grid::new("dl-what")
            .num_columns(2)
            .spacing([16.0, 10.0])
            .show(ui, |ui| {
                ui.label("Game");
                ui.horizontal(|ui| {
                    for g in [GameKind::Origins, GameKind::Da2] {
                        if ui.radio(self.game == g, g.name()).clicked() && self.game != g {
                            self.preset(g, self.store);
                        }
                    }
                });
                ui.end_row();
                ui.label("From");
                ui.horizontal(|ui| {
                    for s in [Store::Steam, Store::Gog] {
                        let sells = s.sells(self.game);
                        let r = ui
                            .add_enabled(sells, egui::RadioButton::new(self.store == s, s.name()));
                        if r.clicked() {
                            self.store = s;
                        }
                        if !sells {
                            r.on_disabled_hover_text("GOG doesn't sell Dragon Age II.");
                        }
                    }
                });
                ui.end_row();
                ui.label("Into");
                ui.horizontal(|ui| {
                    match &self.dest {
                        Some(d) => ui.label(RichText::new(tilde(d)).color(GOLD_DIM)),
                        None => ui.label(RichText::new("Choose a folder").color(BAD)),
                    };
                    if ui.button("Change…").clicked() {
                        want = Some(Want::PickDest);
                    }
                });
                ui.end_row();
                ui.label("Language");
                let name = LANGUAGES
                    .iter()
                    .find(|(l, _)| *l == self.language)
                    .map_or("English", |(_, n)| n);
                egui::ComboBox::from_id_salt("dl-lang")
                    .selected_text(name)
                    .show_ui(ui, |ui| {
                        for (code, name) in LANGUAGES {
                            ui.selectable_value(&mut self.language, code.to_string(), *name);
                        }
                    });
                ui.end_row();
            });
        ui.add_space(12.0);

        match self.store {
            Store::Steam => self.steam_sign_in(ui),
            Store::Gog => {
                if let Some(w) = self.gog_sign_in(ui) {
                    want = Some(w);
                }
            }
        }
        ui.add_space(14.0);
        let label = format!("Download {}", self.game.name());
        if ui.add_enabled(self.ready(), big_button(&label)).clicked() {
            self.start(ui.ctx());
        }
        let size = match self.store {
            Store::Gog => "About 39 GB from GOG.",
            Store::Steam => "The size shows once you're signed in.",
        };
        ui.label(
            RichText::new(format!(
                "{size} The game's own files are all OpenThedas needs: no {} client, Wine or \
                 CrossOver to play.",
                self.store.name()
            ))
            .small()
            .color(GOLD_DIM),
        );
        want
    }

    fn steam_sign_in(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Sign in to Steam").strong().size(18.0));
        ui.horizontal(|ui| {
            ui.radio_value(&mut self.steam_qr, true, "Scan a code with the Steam app");
            ui.radio_value(&mut self.steam_qr, false, "Account name and password");
        });
        if self.steam_qr {
            ui.label(
                RichText::new(
                    "A QR code appears when the download starts. Scan it in the Steam app \
                     on your phone; no password is typed here.",
                )
                .color(GOLD_DIM),
            );
            return;
        }
        egui::Grid::new("dl-steam")
            .num_columns(2)
            .spacing([16.0, 8.0])
            .show(ui, |ui| {
                ui.label("Account name");
                ui.add(egui::TextEdit::singleline(&mut self.account).desired_width(260.0));
                ui.end_row();
                ui.label("Password");
                ui.add(
                    egui::TextEdit::singleline(&mut self.password)
                        .id(egui::Id::unique(PASSWORD_ID))
                        .password(true)
                        .desired_width(260.0),
                );
                ui.end_row();
            });
        ui.label(
            RichText::new("If your account uses Steam Guard, the code is asked for next.")
                .small()
                .color(GOLD_DIM),
        );
    }

    fn gog_sign_in(&mut self, ui: &mut egui::Ui) -> Option<Want> {
        let mut want = None;
        ui.label(RichText::new("Sign in to GOG").strong().size(18.0));
        ui.horizontal(|ui| {
            ui.label("1.");
            if ui
                .button("Open GOG's sign-in page in your browser")
                .clicked()
            {
                want = Some(Want::OpenUrl(gog::login_url()));
            }
        });
        ui.label(
            "2. Sign in there. GOG then shows an almost empty page. Copy that page's whole \
             address from the browser's address bar and paste it here:",
        );
        ui.add(
            egui::TextEdit::singleline(&mut self.gog_pasted)
                .id(egui::Id::unique(GOG_ID))
                .hint_text("https://embed.gog.com/on_login_success?origin=client&code=…")
                .desired_width(f32::INFINITY),
        );
        if !self.gog_pasted.trim().is_empty() && gog::code_from(&self.gog_pasted).is_none() {
            ui.label(RichText::new("That address has no sign-in code in it.").color(BAD));
        }
        want
    }

    fn progress(&mut self, ui: &mut egui::Ui) {
        let running = self.running();
        let r = &mut self.run;
        if let Some(root) = &r.done {
            ui.label(
                RichText::new(format!("Downloaded to {}", tilde(root)))
                    .size(18.0)
                    .color(GOLD),
            );
            return;
        }
        if let Some(e) = &r.failed {
            ui.label(RichText::new(e).size(17.0).color(BAD));
            ui.label(
                "Nothing was saved from the sign-in. Files already downloaded stay, so \
                      starting again into the same folder picks up where it stopped.",
            );
            return;
        }
        if r.cancelled {
            ui.label("Stopped. Files already downloaded stay in the folder.");
            return;
        }
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(RichText::new(&r.status).size(18.0));
        });
        if let Some((url, tex)) = &mut r.qr {
            let tex = tex.get_or_insert_with(|| qr_texture(ui.ctx(), url));
            ui.add_space(6.0);
            ui.add(egui::Image::new(egui::load::SizedTexture::new(
                tex.id(),
                [240.0, 240.0],
            )));
            ui.label(RichText::new("Steam app → Steam Guard → scan the QR code").color(GOLD_DIM));
        }
        if r.mobile {
            ui.label("Approve the sign-in in the Steam app on your phone.");
        }
        if let Some(email) = r.guard {
            ui.add_space(6.0);
            ui.label(if email {
                "Steam sent a code to your email. Type it here:"
            } else {
                "Type the code from the Steam app on your phone:"
            });
            let mut send = false;
            ui.horizontal(|ui| {
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut self.guard)
                        .id(egui::Id::unique(GUARD_ID))
                        .desired_width(120.0),
                );
                send = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                send |= ui.button("Send").clicked();
            });
            if r.wrong_guard {
                ui.label(RichText::new("Steam didn't accept that code. Try again.").color(BAD));
            }
            if send && !self.guard.trim().is_empty() {
                if let Some(job) = &self.job {
                    job.reply(Reply::GuardCode(Secret::new(std::mem::take(
                        &mut self.guard,
                    ))));
                }
                r.wrong_guard = false;
                forget_fields(ui.ctx());
            }
        }
        if let Some((bytes, files)) = r.plan {
            let (done, total) = r.progress;
            let frac = if total > 0 {
                done as f32 / total as f32
            } else {
                0.0
            };
            ui.add_space(6.0);
            ui.add(egui::ProgressBar::new(frac).desired_width(ui.available_width().min(600.0)));
            let mut line = format!(
                "{} of {} ({} files)",
                size_text(done),
                size_text(bytes),
                files
            );
            if let Some((t0, d0)) = r.started {
                let secs = t0.elapsed().as_secs_f64();
                let rate = done.saturating_sub(d0) as f64 / secs.max(0.001);
                if secs > 3.0 && rate > 1.0 {
                    let left = (total.saturating_sub(done) as f64 / rate) as u64;
                    line.push_str(&format!(
                        ", {}/s, about {} left",
                        size_text(rate as u64),
                        duration_text(left)
                    ));
                }
            }
            ui.label(line);
        }
        if running {
            ui.add_space(8.0);
            if ui.button("Cancel").clicked() {
                if let Some(job) = &self.job {
                    job.cancel();
                }
            }
        }
    }

    /// For screenshots: the page as it looks mid-download.
    pub fn demo_running(&mut self) {
        // GOG's Origins build in English: 32,418 files, 38.8 GB (its public file lists).
        self.store = Store::Gog;
        self.run = Run {
            status: format!("Downloading {}", self.game.name()),
            signed_in: true,
            plan: Some((38_766_984_639, 32_418)),
            progress: (15_506_793_855, 38_766_984_639),
            ..Default::default()
        };
    }

    /// For screenshots: mid-download state is drawn as if running.
    pub fn draw_demo(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        ui.label(RichText::new("Download the game").size(24.0).color(GOLD));
        ui.add_space(4.0);
        notice(ui, self.store);
        ui.add_space(10.0);
        let r = &self.run;
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(RichText::new(&r.status).size(18.0));
        });
        if let Some((bytes, files)) = r.plan {
            let (done, total) = r.progress;
            ui.add_space(6.0);
            ui.add(
                egui::ProgressBar::new(done as f32 / total as f32)
                    .desired_width(ui.available_width().min(600.0)),
            );
            ui.label(format!(
                "{} of {} ({} files), 24 MB/s, about {} left",
                size_text(done),
                size_text(bytes),
                files,
                duration_text((total - done) / 24_000_000)
            ));
        }
        ui.add_space(8.0);
        let _ = ui.button("Cancel");
    }
}

/// The promise about credentials, on every view of the page.
fn notice(ui: &mut egui::Ui, store: Store) {
    egui::Frame::new()
        .fill(Color32::from_rgb(0x1f, 0x2a, 0x1a))
        .inner_margin(egui::Margin::symmetric(12, 8))
        .corner_radius(4.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                RichText::new("Your sign-in is only used for this download, and never saved.")
                    .strong()
                    .color(Color32::from_rgb(0xc8, 0xe6, 0xa8)),
            );
            let how = match store {
                Store::Steam => {
                    "OpenThedas never writes your password, Steam Guard codes or Steam's sign-in \
                     tokens anywhere: not to disk, not to logs, not to issue reports. They stay \
                     in memory while the download runs and are forgotten when it ends."
                }
                Store::Gog => {
                    "You sign in on GOG's own page in your browser, so OpenThedas never sees \
                     your GOG password. The code GOG gives back, and GOG's sign-in tokens, are \
                     never written anywhere: not to disk, not to logs, not to issue reports. \
                     They stay in memory while the download runs and are forgotten when it ends."
                }
            };
            ui.label(RichText::new(how).color(Color32::from_rgb(0xb0, 0xc8, 0x98)));
        });
}

/// Drops what egui keeps of the credential fields (their undo history holds old text).
fn forget_fields(ctx: &egui::Context) {
    for id in [PASSWORD_ID, GUARD_ID, GOG_ID] {
        egui::TextEdit::store_state(ctx, egui::Id::unique(id), Default::default());
    }
}

fn qr_texture(ctx: &egui::Context, url: &str) -> egui::TextureHandle {
    // Four modules of white around the code, as scanners expect; 6 px a module.
    const QUIET: usize = 4;
    const PX: usize = 6;
    let (side, dark) = thedas_store::qr(url).unwrap_or((1, vec![false]));
    let n = (side + 2 * QUIET) * PX;
    let mut img = egui::ColorImage::filled([n, n], Color32::WHITE);
    for y in 0..side {
        for x in 0..side {
            if dark[y * side + x] {
                for dy in 0..PX {
                    for dx in 0..PX {
                        img[((x + QUIET) * PX + dx, (y + QUIET) * PX + dy)] = Color32::BLACK;
                    }
                }
            }
        }
    }
    ctx.load_texture("steam-qr", img, egui::TextureOptions::NEAREST)
}

fn duration_text(secs: u64) -> String {
    if secs >= 3600 {
        format!("{} h {} min", secs / 3600, (secs % 3600) / 60)
    } else if secs >= 60 {
        format!("{} min", secs.div_ceil(60))
    } else {
        "a minute".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_read_well() {
        assert_eq!(duration_text(30), "a minute");
        assert_eq!(duration_text(61), "2 min");
        assert_eq!(duration_text(3 * 3600 + 120), "3 h 2 min");
    }

    #[test]
    fn gog_only_for_origins() {
        let mut d = Download::new("fr-fr");
        assert_eq!(d.language, "fr-fr");
        d.preset(GameKind::Da2, Store::Gog);
        assert_eq!(d.store, Store::Steam);
        d.preset(GameKind::Origins, Store::Gog);
        assert_eq!(d.store, Store::Gog);
        assert_eq!(Download::new("xx-yy").language, "en-us");
    }

    #[test]
    fn password_moves_out_of_the_field() {
        let mut d = Download::new("en-us");
        d.steam_qr = false;
        d.account = "someone".into();
        d.password = "pw".into();
        d.dest = Some(std::env::temp_dir().join("thedas-dl-never-used"));
        assert!(d.ready());
        // Not started (that would reach Steam); the move itself is what `start` does.
        let s = Secret::new(std::mem::take(&mut d.password));
        assert!(d.password.is_empty() && s.expose() == "pw");
        assert!(!d.ready());
    }
}
