//! The launcher's logic, apart from its window: finding Dragon Age installs ([`detect`]), the
//! launcher's own settings ([`config`]), editing the mod list ([`modlist`]), and the links
//! that hand the player to Steam or GOG to get the game ([`store`]).
//!
//! No GPU and no window here, so everything is tested against synthetic folders. The window
//! is `thedas-app`'s (`launcher.rs`). See `docs/launcher.md`.

pub mod config;
pub mod detect;
pub mod modlist;
pub mod registry;
pub mod store;
pub mod vdf;

#[cfg(test)]
mod testdir;

pub use config::{GameKind, Install, LauncherConfig, PlayOptions};
pub use detect::{Found, Inspected, Os, Source};
pub use modlist::{ModFile, ModRow};

/// Environment variable the game reads its DLC keys file from (an `erfpasswords.txt`-format
/// file), after `--keys` and before `keys.txt` in the data folder.
pub const KEYS_ENV: &str = thedas_vfs::keys_file::ENV;
