//! DLC archive passwords from the player's own `DAOrigins.exe`.
//!
//! Encrypted DLC archives (ERF V2.2) carry only the MD5 digest of their password. The game
//! keeps an offline password per ERF module ID: a routine selects an encoded eight-character
//! string by module ID, and each character becomes a decimal digit, `(c - 'A') / 2`. The
//! digits are the password, whose MD5 the archive's digest confirms. Nothing here is stored
//! in the repository: the strings are read from the installed executable at run time.
//!
//! **Provisional, verified only on the GOG Ultimate Edition build of `DAOrigins.exe`:**
//! - the jump table at file offset [`TABLE`] is indexed by module ID minus 100;
//! - each case starts with `push imm32`, the encoded string's address;
//! - the image base is `0x400000`, and the RVAs used here equal file offsets;
//! - the case code lies in [`CASES`] and the strings in [`STRINGS`].
//!
//! Other builds (Steam, EA, retail 1.04/1.05, other languages) are not recognised; their
//! encrypted archives stay unread, as before (FMT-001). A wrong layout cannot supply a wrong
//! key: every key must match the archive's own digest.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use crate::crypto::DaoKey;
use crate::path::resolve_ci;

/// Provisional (GOG build only): the image base the table's addresses assume.
const IMAGE_BASE: u32 = 0x40_0000;
/// Provisional (GOG build only): the file offset of the jump table, one entry per module ID
/// from 100.
const TABLE: usize = 0x8_84f8;
/// Module IDs the table covers: 100 to 150.
const MODULES: u32 = 51;
/// Provisional (GOG build only): the file offsets the offline cases lie in. The online
/// entitlement fallback lies after them.
const CASES: std::ops::RangeInclusive<usize> = 0x8_78c0..=0x8_8346;
/// Provisional (GOG build only): the file offsets the encoded strings lie in.
const STRINGS: std::ops::RangeInclusive<usize> = 0x6e_b0e0..=0x6e_b320;

/// What one executable gives: the encoded password per module ID, or why it gives none.
#[derive(Debug, PartialEq, Eq)]
enum Table {
    Found(HashMap<u32, [u8; 8]>),
    Unrecognised(&'static str),
}

/// The tables read so far, by executable path, and what has been logged.
#[derive(Default)]
struct Cache {
    tables: HashMap<PathBuf, Arc<Table>>,
    logged_no_exe: bool,
    logged_mismatch: HashSet<(PathBuf, u32)>,
}

fn cache() -> &'static Mutex<Cache> {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// The password for an archive with ERF module ID `module` and password digest `digest`,
/// from the first `bin_ship/DAOrigins.exe` (in any case) in the archive's folder or above it.
/// Each executable is read once per run. `None`, logged once, when there is no executable,
/// it is not a recognised build, or it has no password for the module that matches the
/// digest.
pub(crate) fn key(archive: &Path, module: u32, digest: &[u8; 16]) -> Option<DaoKey> {
    let exe = archive
        .parent()?
        .ancestors()
        .find_map(|dir| resolve_ci(dir, "bin_ship/DAOrigins.exe").filter(|p| p.is_file()));
    let mut cache = cache().lock().unwrap_or_else(|p| p.into_inner());
    let Some(exe) = exe else {
        if !std::mem::replace(&mut cache.logged_no_exe, true) {
            log::info!(
                "dlc: no bin_ship/DAOrigins.exe above {}; encrypted DLC archives stay unread",
                archive.display()
            );
        }
        return None;
    };
    let table = match cache.tables.get(&exe) {
        Some(t) => t.clone(),
        None => {
            let table = Arc::new(match std::fs::read(&exe) {
                Ok(bytes) => read_table(&bytes),
                Err(_) => Table::Unrecognised("it cannot be read"),
            });
            match &*table {
                Table::Found(keys) => log::info!(
                    "dlc: {} has DLC passwords for {} module IDs (GOG Ultimate layout)",
                    exe.display(),
                    keys.len()
                ),
                Table::Unrecognised(why) => log::info!(
                    "dlc: {} is not a recognised build ({why}); encrypted DLC archives stay \
                     unread",
                    exe.display()
                ),
            }
            cache.tables.insert(exe.clone(), table.clone());
            table
        }
    };
    let Table::Found(keys) = &*table else {
        return None;
    };
    match keys.get(&module).and_then(|e| decode(*e)) {
        Some(k) if k.matches_digest(digest) => Some(k),
        _ => {
            if cache.logged_mismatch.insert((exe.clone(), module)) {
                log::info!(
                    "dlc: {} has no password matching module ID {module}'s digest ({}); that \
                     module's archives stay unread",
                    exe.display(),
                    archive.display()
                );
            }
            None
        }
    }
}

/// Reads the password table from an executable's bytes, checking every step against the GOG
/// layout. Never panics on short or foreign files.
fn read_table(exe: &[u8]) -> Table {
    let at = |offset: usize, len: usize| exe.get(offset..offset.checked_add(len)?);
    let u32_at = |offset: usize| Some(u32::from_le_bytes(at(offset, 4)?.try_into().ok()?));
    let address = |offset: usize| u32_at(offset)?.checked_sub(IMAGE_BASE).map(|a| a as usize);
    let mut keys = HashMap::new();
    for index in 0..MODULES {
        let entry = TABLE + 4 * index as usize;
        if at(entry, 4).is_none() {
            return Table::Unrecognised("too short for the password table");
        }
        let Some(case) = address(entry).filter(|c| CASES.contains(c)) else {
            continue;
        };
        if at(case, 1) != Some(&[0x68]) {
            continue;
        }
        let Some(string) = address(case + 1).filter(|s| STRINGS.contains(s)) else {
            continue;
        };
        let Some([encoded @ .., 0]) = at(string, 9) else {
            continue;
        };
        let Ok(encoded) = <[u8; 8]>::try_from(encoded) else {
            continue;
        };
        if decode(encoded).is_some() {
            keys.insert(100 + index, encoded);
        }
    }
    if keys.is_empty() {
        Table::Unrecognised("no password table where the GOG build has it")
    } else {
        Table::Found(keys)
    }
}

/// An encoded password (`A`–`T`, two letters per digit) as its decimal text.
fn decode(encoded: [u8; 8]) -> Option<DaoKey> {
    let mut digits = encoded;
    for byte in &mut digits {
        if !(b'A'..=b'T').contains(byte) {
            return None;
        }
        *byte = b'0' + (*byte - b'A') / 2;
    }
    Some(DaoKey::from_bytes(digits))
}

#[cfg(test)]
mod tests {
    use super::*;
    use md5::{Digest, Md5};

    /// A synthetic executable laid out as the GOG build is: module `module`'s table entry
    /// points at a `push imm32` of the made-up string `encoded`. Everything else is zero.
    fn exe(module: u32, encoded: &[u8; 8]) -> Vec<u8> {
        let (case, string) = (*CASES.start() + 0x10, *STRINGS.start() + 0x20);
        let mut exe = vec![0u8; *STRINGS.end() + 16];
        let entry = TABLE + 4 * (module - 100) as usize;
        exe[entry..entry + 4].copy_from_slice(&(IMAGE_BASE + case as u32).to_le_bytes());
        exe[case] = 0x68;
        exe[case + 1..case + 5].copy_from_slice(&(IMAGE_BASE + string as u32).to_le_bytes());
        exe[string..string + 8].copy_from_slice(encoded);
        exe
    }

    #[test]
    fn decodes_digit_pairs_without_losing_leading_zeroes() {
        let key = decode(*b"ABCDSTEF").unwrap();
        assert!(key.matches_digest(&Md5::digest(b"00119922").into()));
        assert_eq!(key.blowfish_key(), Some(119922u64.to_le_bytes()));
        assert!(decode(*b"ABCDEFGU").is_none());
    }

    #[test]
    fn reads_the_table_and_rejects_foreign_or_short_files() {
        let good = exe(107, b"ABCDSTEF");
        assert_eq!(
            read_table(&good),
            Table::Found(HashMap::from([(107, *b"ABCDSTEF")]))
        );
        let unrecognised = |exe: &[u8]| matches!(read_table(exe), Table::Unrecognised(_));
        // Truncated: before the table, inside the table, inside the string.
        for len in [0, 0x1000, TABLE + 10, *STRINGS.start() + 0x24] {
            assert!(unrecognised(&good[..len]), "{len}");
        }
        // Unknown builds: no push, a letter outside A-T, no NUL after the string, all 0xff.
        let mut b = good.clone();
        b[*CASES.start() + 0x10] = 0x90;
        assert!(unrecognised(&b));
        assert!(unrecognised(&exe(107, b"ABCDSTEZ")));
        let mut b = good.clone();
        b[*STRINGS.start() + 0x28] = b'A';
        assert!(unrecognised(&b));
        assert!(unrecognised(&vec![0xff; good.len()]));
    }

    #[test]
    fn finds_the_exe_in_any_case_above_the_archive_and_checks_the_digest() {
        let root = std::env::temp_dir().join(format!("thedas-gog-dlc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let data = root.join("Addins/some_dlc/core/data");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::create_dir_all(root.join("BIN_SHIP")).unwrap();
        std::fs::write(root.join("BIN_SHIP/daorigins.EXE"), exe(107, b"ABCDSTEF")).unwrap();
        let archive = data.join("some_dlc.erf");

        let right: [u8; 16] = Md5::digest(b"00119922").into();
        let found = key(&archive, 107, &right).unwrap();
        assert_eq!(found.blowfish_key(), Some(119922u64.to_le_bytes()));
        // A digest the password does not match, a module the table lacks, no executable.
        let wrong: [u8; 16] = Md5::digest(b"00119923").into();
        assert!(key(&archive, 107, &wrong).is_none());
        assert!(key(&archive, 108, &right).is_none());
        let elsewhere = std::env::temp_dir().join("thedas-gog-dlc-none/x.erf");
        assert!(key(&elsewhere, 107, &right).is_none());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
