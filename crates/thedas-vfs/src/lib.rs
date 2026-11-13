//! Archive readers and layered resource lookup for Dragon Age: Origins and Dragon Age II data.
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

/// How ERF V3.0 (Dragon Age II) identifies a resource: FNV-1 (multiply, then xor) 64 over
/// the bytes of the lowercased name, extension included. Verified against the shipped DA2
/// archives, whose few stored names hash to their stored hashes.
pub fn name_hash(name: &str) -> u64 {
    name.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        h.wrapping_mul(0x0000_0100_0000_01b3) ^ b.to_ascii_lowercase() as u64
    })
}

/// How ERF V3.0 identifies a resource type: FNV-1 32 over the lowercased extension, without
/// the dot. Verified against the shipped DA2 archives.
pub fn type_hash(ext: &str) -> u32 {
    ext.bytes().fold(0x811c_9dc5, |h, b| {
        h.wrapping_mul(0x0100_0193) ^ b.to_ascii_lowercase() as u32
    })
}

/// Extensions that occur in Dragon Age II archives, so an entry stored only by hashes can
/// still be given its type. Collected from the shipped DA2 data; several of these (`vlm`,
/// `assembly`, `fdl`, `ncc`) have no reader yet.
const KNOWN_EXTENSIONS: &[&str] = &[
    "anb", "ani", "are", "arl", "assembly", "bnk", "cif", "cl", "cnv", "cre", "cur", "cut", "dat",
    "dds", "dx11", "evt", "fdl", "fxa", "fxe", "fxo", "gad", "gda", "gff", "gfx", "lst", "ldf",
    "lua", "mao", "map", "mat", "mmh", "mop", "mor", "msh", "mtx", "ncc", "ncs", "ogg", "opf",
    "phy", "plo", "pwk", "rml", "s", "spt", "sta", "stg", "tlk", "tnt", "ttf", "utc", "uti", "utm",
    "utp", "utt", "vlm", "xml",
];

/// The extension whose [`type_hash`] this is, if it is one Dragon Age II uses.
pub fn extension_for_type_hash(hash: u32) -> Option<&'static str> {
    KNOWN_EXTENSIONS
        .iter()
        .copied()
        .find(|e| type_hash(e) == hash)
}

/// The extension of a resource name, lowercased, without the dot. Empty if there is none.
pub fn extension(name: &str) -> String {
    match name.rfind('.') {
        Some(i) => name[i + 1..].to_ascii_lowercase(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fnv1_hashes_match_published_vectors() {
        // FNV-1 test vectors for "a" and "foobar" (Fowler, Noll and Vo's reference set).
        assert_eq!(name_hash("a"), 0xaf63_bd4c_8601_b7be);
        assert_eq!(name_hash("foobar"), 0x340d_8765_a4dd_a9c2);
        assert_eq!(type_hash("a"), 0x050c_5d7e);
        assert_eq!(type_hash("foobar"), 0x31f0_b262);
        // Case-insensitive, like every Dragon Age resource name.
        assert_eq!(name_hash("FooBar"), name_hash("foobar"));
        assert_eq!(extension_for_type_hash(type_hash("MMH")), Some("mmh"));
        assert_eq!(extension_for_type_hash(type_hash("nope")), None);
    }
}
