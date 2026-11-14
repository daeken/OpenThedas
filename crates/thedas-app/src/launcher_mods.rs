//! The launcher's "what do my mods do" view: for each layer the mod list adds, how many
//! game files it replaces and how many a later mod hides, and the files two sources fight
//! over. It reads the stack the way the game does ([`DaoInstall::play_vfs`] with the saved
//! mod list), on a background thread, because mounting a big archive the first time unpacks
//! it into the mod cache.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use egui::RichText;
use thedas_vfs::install::{DaoInstall, TextureQuality};
use thedas_vfs::mods::{self, Fight, ModList, ModStats};

use crate::launcher::{BAD, GOLD_DIM, heading, live, named};

/// The most conflicts one layer's file list shows.
const SHOWN: usize = 200;
/// The most conflicts kept in memory (a texture pack can replace tens of thousands).
const KEPT: usize = 200_000;

struct Report {
    stats: Vec<ModStats>,
    fights: Vec<Fight>,
    /// Whether `fights` was cut at [`KEPT`].
    cut: bool,
    problems: Vec<String>,
}

enum State {
    Idle,
    Running(Receiver<Result<Report, String>>),
    Done(Report),
    Failed(String),
}

pub struct ModReport {
    state: State,
    /// The layer whose files are listed (an index into the report's `stats`).
    shown: Option<usize>,
}

impl ModReport {
    pub fn new() -> ModReport {
        ModReport {
            state: State::Idle,
            shown: None,
        }
    }

    /// Whether a scan is running (the window keeps redrawing).
    pub fn running(&self) -> bool {
        matches!(self.state, State::Running(_))
    }

    /// Forgets a finished report: the list or the install changed under it.
    pub fn forget(&mut self) {
        if !self.running() {
            self.state = State::Idle;
            self.shown = None;
        }
    }

    pub fn poll(&mut self) {
        let State::Running(rx) = &self.state else {
            return;
        };
        match rx.try_recv() {
            Ok(Ok(report)) => self.state = State::Done(report),
            Ok(Err(e)) => self.state = State::Failed(e),
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                self.state = State::Failed("The scan stopped unexpectedly.".into())
            }
        }
    }

    fn start(&mut self, root: PathBuf) {
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send(scan(&root));
        });
        self.state = State::Running(rx);
        self.shown = None;
    }

    /// Draws the view under the mod list. `root` is the chosen install, if any; `dirty` says
    /// the list has unsaved changes (the game, and so this scan, reads the saved list).
    pub fn draw(&mut self, ui: &mut egui::Ui, root: Option<&PathBuf>, dirty: bool) {
        ui.add_space(12.0);
        heading(ui, "What your mods do", 18.0);
        let Some(root) = root else {
            ui.label("Choose a game on the Play page to see how your mods change it.");
            return;
        };
        if dirty {
            ui.label(
                RichText::new("This reads the saved list: save your changes first.").color(BAD),
            );
        }
        let running = self.running();
        ui.horizontal(|ui| {
            let label = if matches!(self.state, State::Idle) {
                "Check what my mods do"
            } else {
                "Check again"
            };
            if ui.add_enabled(!running, egui::Button::new(label)).clicked() {
                self.start(root.clone());
            }
            if running {
                ui.spinner();
                let r = ui.label("Reading your mods (a big archive is unpacked the first time)…");
                live(&r, egui::accesskit::Live::Polite);
            }
        });
        let mut show: Option<usize> = None;
        match &self.state {
            State::Idle | State::Running(_) => {}
            State::Failed(e) => {
                ui.label(RichText::new(format!("Could not check: {e}")).color(BAD));
            }
            State::Done(r) => {
                for p in &r.problems {
                    ui.label(RichText::new(p).small().color(BAD));
                }
                if r.stats.is_empty() {
                    ui.label("No mods are loaded.");
                }
                for (i, s) in r.stats.iter().enumerate() {
                    ui.horizontal(|ui| {
                        let name = pretty(&s.label);
                        ui.label(RichText::new(&name).size(16.0));
                        ui.label(RichText::new(summary(s)).small().color(GOLD_DIM));
                        if named(
                            ui.small_button("Files"),
                            &format!("Show the files of {name}"),
                        )
                        .clicked()
                        {
                            show = Some(i);
                        }
                    });
                }
            }
        }
        if let Some(i) = show {
            self.shown = if self.shown == Some(i) { None } else { Some(i) };
        }
        if let (State::Done(r), Some(i)) = (&self.state, self.shown) {
            if let Some(s) = r.stats.get(i) {
                files(ui, r, &s.label);
            }
        }
    }
}

/// The layer's label without its `mod:` prefix, which every row shares.
fn pretty(label: &str) -> String {
    label
        .strip_prefix(mods::MOD_LABEL)
        .unwrap_or(label)
        .to_string()
}

fn summary(s: &ModStats) -> String {
    let mut parts = vec![format!(
        "{} file{}",
        s.files,
        if s.files == 1 { "" } else { "s" }
    )];
    if s.replaces > 0 {
        parts.push(format!("replaces {} of the game's", s.replaces));
    }
    if s.hidden > 0 {
        parts.push(format!("{} hidden by a later mod", s.hidden));
    }
    if s.winning == s.files && s.hidden == 0 && s.replaces == 0 {
        parts.push("no conflicts".into());
    }
    parts.join(" · ")
}

/// The files `label` fights over: those it wins and those it loses.
fn files(ui: &mut egui::Ui, r: &Report, label: &str) {
    ui.add_space(4.0);
    let mut shown = 0usize;
    let mut total = 0usize;
    egui::ScrollArea::vertical()
        .max_height(220.0)
        .id_salt("mod-fights")
        .show(ui, |ui| {
            for f in &r.fights {
                let wins = f.winner == label;
                let loses = f.beaten.iter().any(|b| b == label);
                if !wins && !loses {
                    continue;
                }
                total += 1;
                if shown >= SHOWN {
                    continue;
                }
                shown += 1;
                let text = if wins {
                    let over: Vec<String> = f.beaten.iter().take(3).map(|b| pretty(b)).collect();
                    let game = if f.in_game { " (a game file)" } else { "" };
                    format!("{}: wins over {}{game}", f.name, over.join(", "))
                } else {
                    format!("{}: loses to {}", f.name, pretty(&f.winner))
                };
                ui.label(RichText::new(text).small());
            }
        });
    if total == 0 {
        ui.label(
            RichText::new("It has no conflicts.")
                .small()
                .color(GOLD_DIM),
        );
    } else if total > shown {
        ui.label(
            RichText::new(format!("{shown} of {total} shown."))
                .small()
                .color(GOLD_DIM),
        );
    }
    if r.cut {
        ui.label(
            RichText::new(format!("Only the first {KEPT} conflicts are kept."))
                .small()
                .color(GOLD_DIM),
        );
    }
}

fn scan(root: &std::path::Path) -> Result<Report, String> {
    let install = DaoInstall::open(root).map_err(|e| e.to_string())?;
    let vfs = install
        .play_vfs(TextureQuality::High)
        .map_err(|e| e.to_string())?;
    let mut problems = Vec::new();
    if let Some(list) = mods::default_list_file() {
        // The list is mounted once more on a scratch stack only to report its problems.
        for m in ModList::load(&list).mount(&mut thedas_vfs::Vfs::new()) {
            for p in m.problems {
                problems.push(format!("{}: {p}", m.entry.path.display()));
            }
        }
    }
    let mut fights = mods::fights(&vfs);
    let cut = fights.len() > KEPT;
    fights.truncate(KEPT);
    Ok(Report {
        stats: mods::mod_stats(&vfs),
        fights,
        cut,
        problems,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stats(files: usize, winning: usize, replaces: usize, hidden: usize) -> ModStats {
        ModStats {
            label: "mod:x".into(),
            files,
            winning,
            replaces,
            hidden,
        }
    }

    #[test]
    fn the_summary_says_what_a_mod_does() {
        assert_eq!(summary(&stats(1, 1, 0, 0)), "1 file · no conflicts");
        assert_eq!(
            summary(&stats(5, 3, 2, 2)),
            "5 files · replaces 2 of the game's · 2 hidden by a later mod"
        );
        assert_eq!(summary(&stats(2, 2, 0, 0)), "2 files · no conflicts");
        assert_eq!(pretty("mod:a/b"), "a/b");
        assert_eq!(pretty("user:packages/core/x"), "user:packages/core/x");
    }
}
