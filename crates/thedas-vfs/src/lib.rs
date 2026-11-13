//! Archive readers and layered resource lookup for Dragon Age: Origins data.
//!
//! The pieces, bottom up:
//!
//! - [`erf`]: the ERF container family (V2.0, V2.1, V2.2, V3.0). DA:O `.rim` files are ERF V2.0.
//! - [`crypto`]: the Blowfish scheme DA:O uses for encrypted DLC archives (ERF V2.2).
//! - [`vfs`]: an ordered stack of layers (archives and loose directories). Lookups return the
//!   highest-priority hit together with where it came from, so later tooling can show mod
//!   conflicts and shadowed files.
//! - [`install`]: discovers a DA:O install, builds the default layer stack for it, and mounts
//!   a level's own archives on top while its area is loaded.
//! - [`path`]: case-insensitive file-system paths, for names the original (a Windows game)
//!   treats case-insensitively but a case-sensitive volume would not.

pub mod crypto;
pub mod erf;
mod gog_dlc;
pub mod install;
pub mod path;
pub mod vfs;

use std::path::PathBuf;

pub use erf::{Erf, ErfEntry, ErfVersion};
pub use install::{DaoInstall, LevelMount};
pub use path::{join_ci, resolve_ci};
pub use vfs::{Layer, Provenance, Vfs};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error on {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{path}: not an ERF archive")]
    NotErf { path: PathBuf },
    #[error("{path}: unsupported ERF version {version:?}")]
    UnsupportedVersion { path: PathBuf, version: String },
    #[error("{path}: malformed archive: {reason}")]
    Malformed { path: PathBuf, reason: String },
    #[error("{path}: entry {name} is encrypted and no key was supplied")]
    Encrypted { path: PathBuf, name: String },
    #[error("{path}: key does not match the archive's password digest")]
    WrongKey { path: PathBuf },
    #[error("{path}: entry {name}: unsupported compression {compression}")]
    UnsupportedCompression {
        path: PathBuf,
        name: String,
        compression: u32,
    },
    #[error("{path}: entry {name}: decompression failed: {reason}")]
    Decompress {
        path: PathBuf,
        name: String,
        reason: String,
    },
    #[error("resource not found: {0}")]
    NotFound(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Normalizes a resource name for lookup: Dragon Age resource names are case-insensitive.
pub fn normalize_name(name: &str) -> String {
    name.to_ascii_lowercase()
}

/// The extension of a resource name, lowercased, without the dot. Empty if there is none.
pub fn extension(name: &str) -> String {
    match name.rfind('.') {
        Some(i) => name[i + 1..].to_ascii_lowercase(),
        None => String::new(),
    }
}
