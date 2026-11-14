//! The launcher's settings: `launcher.json` in the player's data folder
//! ([`thedas_vfs::mods::user_data_dir`]). Our own file; the original has no launcher settings.
//!
//! Unknown fields are dropped and missing ones take their defaults, so an older or newer
//! launcher reads it; a file that does not parse is kept aside as `launcher.json.bad` rather
//! than silently replaced.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thedas_vfs::Game;

pub const FILE_NAME: &str = "launcher.json";

/// Which game, as the file writes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GameKind {
    Origins,
    Da2,
}

impl From<Game> for GameKind {
    fn from(g: Game) -> Self {
        match g {
            Game::Origins => GameKind::Origins,
            Game::DragonAge2 => GameKind::Da2,
        }
    }
}

impl GameKind {
    pub fn name(self) -> &'static str {
        match self {
            GameKind::Origins => "Dragon Age: Origins",
            GameKind::Da2 => "Dragon Age II",
        }
    }
}

/// One install the player kept.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Install {
    pub game: GameKind,
    pub root: PathBuf,
    /// Where it was found ("Steam, This computer"), for the list.
    #[serde(default)]
    pub found: String,
    /// An `erfpasswords.txt`-format keys file for its encrypted DLC, handed to the game as
    /// `THEDAS_KEYS`.
    #[serde(default)]
    pub keys_file: Option<PathBuf>,
}

/// How the game is started.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlayOptions {
    /// Window size, `WIDTHxHEIGHT` (`--size`).
    pub size: String,
    /// `low`, `medium` or `high` (`--detail`).
    pub detail: String,
    /// Bloom and the other frame buffer effects (`--frame-effects`).
    pub frame_effects: bool,
    /// `faithful` (the original's shaders) or `deferred` (ours) (`--renderer`); F11 flips
    /// between them in game.
    pub renderer: String,
    /// The logo and intro movies before the start menu (off: `--no-intro`).
    pub intro: bool,
    /// Voice-over language folder (`--language`).
    pub language: String,
    /// Sound (off: `--no-audio`).
    pub audio: bool,
}

impl Default for PlayOptions {
    fn default() -> Self {
        PlayOptions {
            size: "1280x720".into(),
            detail: "medium".into(),
            frame_effects: true,
            renderer: "faithful".into(),
            intro: true,
            language: "en-us".into(),
            audio: true,
        }
    }
}

impl PlayOptions {
    /// The game's command line for these options (the install is added by the caller).
    pub fn args(&self) -> Vec<String> {
        let mut a = vec![
            "--size".to_string(),
            self.size.clone(),
            "--detail".into(),
            self.detail.clone(),
            "--frame-effects".into(),
            if self.frame_effects { "on" } else { "off" }.into(),
            "--renderer".into(),
            self.renderer.clone(),
            "--language".into(),
            self.language.clone(),
        ];
        if !self.intro {
            a.push("--no-intro".into());
        }
        if !self.audio {
            a.push("--no-audio".into());
        }
        a
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LauncherConfig {
    pub installs: Vec<Install>,
    /// Index into `installs` of the one Play starts.
    pub selected: Option<usize>,
    /// Started with no arguments, play the selected install at once instead of showing the
    /// launcher (`--launcher` still shows it).
    pub skip_launcher: bool,
    pub play: PlayOptions,
    /// The first-run wizard has been finished (or skipped) once.
    pub wizard_done: bool,
}

impl LauncherConfig {
    /// `launcher.json` in the player's data folder.
    pub fn default_path() -> PathBuf {
        // A scripted run points this at a scratch file so it never touches the player's.
        if let Some(p) = std::env::var_os("THEDAS_LAUNCHER_CONFIG").filter(|p| !p.is_empty()) {
            return PathBuf::from(p);
        }
        thedas_vfs::mods::user_data_dir().join(FILE_NAME)
    }

    /// Reads the file; a missing one is the default settings. One that does not parse is
    /// renamed to `.bad` (so the player's file is not lost) and the defaults are used.
    pub fn load(path: &Path) -> LauncherConfig {
        let Ok(bytes) = std::fs::read(path) else {
            return LauncherConfig::default();
        };
        match serde_json::from_slice::<LauncherConfig>(&bytes) {
            Ok(mut c) => {
                if c.selected.is_some_and(|i| i >= c.installs.len()) {
                    c.selected = None;
                }
                c
            }
            Err(e) => {
                log::warn!(
                    "launcher: {} does not parse ({e}); starting afresh",
                    path.display()
                );
                let _ = std::fs::rename(path, path.with_extension("json.bad"));
                LauncherConfig::default()
            }
        }
    }

    /// Writes the file whole (to a temporary name, then renamed over the old one).
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, path)
    }

    pub fn selected_install(&self) -> Option<&Install> {
        self.selected.and_then(|i| self.installs.get(i))
    }

    /// Keeps an install (once per folder), returning its index.
    pub fn add_install(&mut self, install: Install) -> usize {
        if let Some(i) = self
            .installs
            .iter()
            .position(|i| same_dir(&i.root, &install.root))
        {
            return i;
        }
        self.installs.push(install);
        self.installs.len() - 1
    }

    pub fn remove_install(&mut self, index: usize) {
        if index >= self.installs.len() {
            return;
        }
        self.installs.remove(index);
        self.selected = match self.selected {
            Some(s) if s == index => (!self.installs.is_empty()).then_some(0),
            Some(s) if s > index => Some(s - 1),
            other => other,
        };
    }
}

fn same_dir(a: &Path, b: &Path) -> bool {
    a == b
        || matches!((std::fs::canonicalize(a), std::fs::canonicalize(b)), (Ok(x), Ok(y)) if x == y)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdir::TestDir;

    #[test]
    fn round_trip_and_defaults() {
        let t = TestDir::new("config");
        let path = t.path().join("launcher.json");
        assert_eq!(LauncherConfig::load(&path), LauncherConfig::default());
        let mut c = LauncherConfig::default();
        let i = c.add_install(Install {
            game: GameKind::Origins,
            root: "/games/dao".into(),
            found: "Steam".into(),
            keys_file: None,
        });
        assert_eq!(
            c.add_install(Install {
                game: GameKind::Origins,
                root: "/games/dao".into(),
                found: String::new(),
                keys_file: None,
            }),
            i
        );
        c.selected = Some(i);
        c.play.intro = false;
        c.save(&path).unwrap();
        assert_eq!(LauncherConfig::load(&path), c);
        // An older file without most fields still reads.
        std::fs::write(&path, r#"{"installs":[],"selected":3,"future":1}"#).unwrap();
        let old = LauncherConfig::load(&path);
        assert_eq!(old.selected, None);
        assert_eq!(old.play, PlayOptions::default());
    }

    #[test]
    fn bad_file_is_kept_aside() {
        let t = TestDir::new("config-bad");
        let path = t.path().join("launcher.json");
        std::fs::write(&path, "{not json").unwrap();
        assert_eq!(LauncherConfig::load(&path), LauncherConfig::default());
        assert!(t.path().join("launcher.json.bad").is_file());
    }

    #[test]
    fn removing_keeps_the_selection_right() {
        let mut c = LauncherConfig::default();
        for r in ["/a", "/b", "/c"] {
            c.add_install(Install {
                game: GameKind::Origins,
                root: r.into(),
                found: String::new(),
                keys_file: None,
            });
        }
        c.selected = Some(2);
        c.remove_install(0);
        assert_eq!(c.selected, Some(1));
        c.remove_install(1);
        assert_eq!(c.selected, Some(0));
        c.remove_install(9);
        assert_eq!(c.installs.len(), 1);
    }

    #[test]
    fn options_as_arguments() {
        let o = PlayOptions {
            intro: false,
            audio: false,
            frame_effects: false,
            ..Default::default()
        };
        let a = o.args().join(" ");
        assert!(a.contains("--no-intro"));
        assert!(a.contains("--no-audio"));
        assert!(a.contains("--frame-effects off"));
        assert!(a.contains("--renderer faithful"));
    }
}
