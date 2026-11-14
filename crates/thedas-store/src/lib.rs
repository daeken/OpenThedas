//! Downloading Dragon Age from the player's own Steam or GOG account, inside the launcher.
//!
//! Sera's rule (2026-11-14): credentials are never saved, and are only used for the download.
//! So in this crate:
//!
//! - A password, a Steam Guard code, a GOG sign-in code and every token Steam or GOG hands
//!   back live in memory only, in [`Secret`] (zeroed on drop, redacted in `Debug`) or in the
//!   store library's own session object, and are dropped when the download ends.
//! - Nothing is written to disk except the game's own files: no token store, no manifest
//!   cache, no depot-key file (steamroom-client's `TokenStore`, `ManifestCache` and
//!   `DepotConfig` are never used).
//! - Nothing secret is logged. The app caps the HTTP and Steam libraries' logs at warnings
//!   (`thedas-app`'s `report.rs`), so a report's log never carries a URL with a token in it.
//!
//! The work runs on its own thread with its own async runtime; the launcher talks to it
//! through a [`Job`]: [`Event`]s out, [`Reply`]s in. See `docs/launcher.md`.

pub mod gog;
mod job;
mod secret;
pub mod steam;

use std::path::PathBuf;

pub use job::{Event, Job, Reply};
pub use secret::Secret;
pub use thedas_launcher::GameKind;

/// Which store to download from, and how to sign in.
pub enum Login {
    /// Steam: the player scans a QR code with the Steam mobile app. No password is typed.
    SteamQr,
    /// Steam: account name and password, then a Steam Guard code or a confirmation in the
    /// mobile app if the account asks for one.
    SteamPassword { account: String, password: Secret },
    /// GOG: the code from GOG's own sign-in page in the player's browser (see
    /// [`gog::login_url`] and [`gog::code_from`]). The app never sees the password.
    Gog { code: Secret },
}

impl Login {
    pub fn store(&self) -> Store {
        match self {
            Login::SteamQr | Login::SteamPassword { .. } => Store::Steam,
            Login::Gog { .. } => Store::Gog,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Store {
    Steam,
    Gog,
}

impl Store {
    pub fn name(self) -> &'static str {
        match self {
            Store::Steam => "Steam",
            Store::Gog => "GOG",
        }
    }

    /// Whether the store sells `game`. GOG sells Origins only.
    pub fn sells(self, game: GameKind) -> bool {
        match self {
            Store::Steam => true,
            Store::Gog => game == GameKind::Origins,
        }
    }
}

/// What to download and where.
#[derive(Clone, Debug)]
pub struct Request {
    pub game: GameKind,
    /// The folder the game goes in (made if missing). The game's files go straight in it.
    pub dest: PathBuf,
    /// The voice and text language wanted, as the game names it (`en-us`, `fr-fr`, ...).
    /// Files for other languages are skipped where the store splits them out.
    pub language: String,
}

/// The default folder for a downloaded game: `Games/<name>` in the player's home folder,
/// one of the places the launcher's search looks.
pub fn default_dest(game: GameKind) -> Option<PathBuf> {
    let name = match game {
        GameKind::Origins => "Dragon Age Origins",
        GameKind::Da2 => "Dragon Age II",
    };
    dirs_home().map(|h| h.join("Games").join(name))
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
}

/// Whether a path from a store's file list is safe to join onto the install folder: relative,
/// no `..`, no drive or root, no empty or odd parts. Both `/` and `\` separate parts.
pub(crate) fn safe_relative(path: &str) -> Option<PathBuf> {
    if path.is_empty() || path.len() > 1024 {
        return None;
    }
    let mut out = PathBuf::new();
    for part in path.split(['/', '\\']) {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".."
            || part.contains(':')
            || part.chars().any(|c| c.is_control())
            || part.ends_with(' ')
        {
            return None;
        }
        out.push(part);
    }
    if path.starts_with(['/', '\\']) || out.as_os_str().is_empty() {
        return None;
    }
    Some(out)
}

/// Bytes as the launcher shows them: "38.8 GB".
pub fn size_text(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= 1e9 {
        format!("{:.1} GB", b / 1e9)
    } else if b >= 1e6 {
        format!("{:.0} MB", b / 1e6)
    } else {
        format!("{:.0} KB", (b / 1e3).max(1.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths_are_checked() {
        assert_eq!(
            safe_relative(r"packages\core\data\x.erf"),
            Some(PathBuf::from("packages/core/data/x.erf"))
        );
        assert_eq!(safe_relative("a/./b"), Some(PathBuf::from("a/b")));
        for bad in [
            "",
            "..",
            r"..\x",
            "a/../../b",
            "/etc/passwd",
            r"\x",
            r"C:\Windows\x",
            "a/b:c",
            "a/\u{1}",
            "a /b",
            "/",
        ] {
            assert_eq!(safe_relative(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn stores_and_sizes() {
        assert!(Store::Gog.sells(GameKind::Origins));
        assert!(!Store::Gog.sells(GameKind::Da2));
        assert_eq!(size_text(38_766_984_639), "38.8 GB");
        assert_eq!(size_text(12_414_580), "12 MB");
        assert_eq!(size_text(10), "1 KB");
    }
}

/// A QR code for `text` (Steam's sign-in address): the side in modules and the dark modules,
/// row by row. The quiet zone around it is the caller's to draw.
pub fn qr(text: &str) -> Option<(usize, Vec<bool>)> {
    let code = qrcode::QrCode::new(text.as_bytes()).ok()?;
    let side = code.width();
    let dark = code
        .to_colors()
        .into_iter()
        .map(|c| c == qrcode::Color::Dark)
        .collect();
    Some((side, dark))
}

#[cfg(test)]
mod qr_tests {
    #[test]
    fn qr_is_square() {
        let (side, dark) = super::qr("https://s.team/q/1/2345678901234567890").unwrap();
        assert_eq!(dark.len(), side * side);
        assert!(side >= 21 && dark.iter().any(|d| *d) && dark.iter().any(|d| !*d));
        // The finder pattern's corner is dark.
        assert!(dark[0]);
    }
}
