//! Getting the game: links that hand the player to the store's own client or page. We never
//! download game files ourselves.

use crate::config::GameKind;
use crate::detect::{GOG_ORIGINS, Os, STEAM_DA2, STEAM_ORIGINS_UE};

/// One way to get a game.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoreLink {
    pub label: String,
    pub url: String,
    /// What the player should know first, or empty.
    pub note: String,
}

/// The ways to get `game` on `os`, the most direct first.
pub fn links(game: GameKind, os: Os) -> Vec<StoreLink> {
    let steam_id = match game {
        GameKind::Origins => STEAM_ORIGINS_UE,
        GameKind::Da2 => STEAM_DA2,
    };
    let wine_note = match os {
        Os::Windows => "",
        Os::MacOs => {
            "Steam sells the Windows version. Install it with Steam running in a CrossOver \
             or Whisky bottle; the launcher finds it there."
        }
        Os::Linux => {
            "Steam sells the Windows version. Steam installs it through Proton; turn on \
             Steam Play for it first. The launcher finds it in your Steam library."
        }
    };
    let mut out = vec![
        StoreLink {
            label: "Install with Steam".into(),
            url: format!("steam://install/{steam_id}"),
            note: wine_note.into(),
        },
        StoreLink {
            label: "Steam store page".into(),
            url: format!("https://store.steampowered.com/app/{steam_id}/"),
            note: String::new(),
        },
    ];
    if game == GameKind::Origins {
        if os == Os::Windows {
            // Provisional: Galaxy's URL scheme, from GOG's own links, not documented.
            out.push(StoreLink {
                label: "Open in GOG Galaxy".into(),
                url: format!("goggalaxy://openGameView/{GOG_ORIGINS}"),
                note: String::new(),
            });
        }
        out.push(StoreLink {
            label: "GOG store page".into(),
            url: "https://www.gog.com/en/game/dragon_age_origins".into(),
            note: match os {
                Os::Windows => String::new(),
                Os::MacOs => "GOG sells the Windows version with an offline installer. Run it \
                              in a CrossOver or Whisky bottle, then search again."
                    .into(),
                Os::Linux => "GOG sells the Windows version with an offline installer. Install \
                              it with Heroic, Lutris or Wine, then search again."
                    .into(),
            },
        });
    }
    out
}

/// Whether a URL is one the launcher may hand to the system: only the schemes it builds.
pub fn allowed(url: &str) -> bool {
    let ok_scheme = ["https://", "steam://", "goggalaxy://"]
        .iter()
        .any(|s| url.starts_with(s));
    ok_scheme
        && !url
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || c == '"')
}

/// Opens a URL with the system's handler (the browser, Steam, Galaxy).
pub fn open_url(url: &str) -> std::io::Result<()> {
    if !allowed(url) {
        return Err(std::io::Error::other(format!("not a store link: {url}")));
    }
    open_with_system(std::ffi::OsStr::new(url))
}

/// Opens a folder in the system's file browser.
pub fn open_folder(dir: &std::path::Path) -> std::io::Result<()> {
    if !dir.is_dir() {
        return Err(std::io::Error::other(format!(
            "{} is not a folder",
            dir.display()
        )));
    }
    open_with_system(dir.as_os_str())
}

fn open_with_system(what: &std::ffi::OsStr) -> std::io::Result<()> {
    let mut cmd = if cfg!(windows) {
        // Not `cmd /c start`: cmd would read `&` in a URL as a command separator.
        let mut c = std::process::Command::new("rundll32");
        c.arg("url.dll,FileProtocolHandler");
        c
    } else if cfg!(target_os = "macos") {
        std::process::Command::new("open")
    } else {
        std::process::Command::new("xdg-open")
    };
    cmd.arg(what);
    cmd.spawn().map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_link_is_allowed() {
        for game in [GameKind::Origins, GameKind::Da2] {
            for os in [Os::Windows, Os::MacOs, Os::Linux] {
                let l = links(game, os);
                assert!(!l.is_empty());
                assert!(l.iter().all(|l| allowed(&l.url)), "{l:?}");
            }
        }
        assert!(
            links(GameKind::Origins, Os::Windows)
                .iter()
                .any(|l| l.url == "steam://install/47810")
        );
        assert!(
            !links(GameKind::Da2, Os::Windows)
                .iter()
                .any(|l| l.url.contains("gog"))
        );
    }

    #[test]
    fn other_urls_are_refused() {
        for bad in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "https://x\" & calc",
            "steam://run/1 --x",
            "",
        ] {
            assert!(!allowed(bad), "{bad}");
        }
    }
}
