//! DLC archive passwords from a keys file the player supplies.
//!
//! Encrypted DLC archives store only a digest of their password. Dragon Age: Origins's can be
//! found from the player's own executable or by search ([`crate::gog_dlc`]), but Dragon Age
//! II's are 16 random bytes that the game fetched from EA's servers, so the only way to read
//! its DLC is a file of passwords. The one players already pass around is `erfpasswords.txt`
//! (from the modding community, for pyGFF): a `erfpasswords = a, b, c` line and a table of
//! `archive  password  title` rows. Both are read; so is anything else that holds the
//! passwords as words:
//!
//! - a DA:O password: a word of decimal digits (`12345678`);
//! - a DA2 password: 32 hexadecimal digits, with or without dashes between them
//!   (`0123-4567-…`), the 16 key bytes in the order written.
//!
//! Which password belongs to which archive is never taken from the file: every password is
//! tried against each archive's own digest, and only one that matches is used. So a stray
//! word that happens to look like a password does no harm, and nothing in the file can make
//! an archive decrypt wrongly.
//!
//! Where the file is, first that applies: the path given to [`set_path`] (the `--keys`
//! option of `thedas` and `thedas-app`), the path in `THEDAS_KEYS`, or [`DEFAULT_FILE`] in the
//! player's data folder ([`crate::mods::user_data_dir`]). It is the player's: nothing from it
//! is written anywhere, logged or kept in the repository.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use crate::crypto::DaoKey;
use crate::erf::Encryption;

/// The keys file's name in the player's data folder.
pub const DEFAULT_FILE: &str = "keys.txt";

/// The environment variable naming a keys file elsewhere.
pub const ENV: &str = "THEDAS_KEYS";

static CHOSEN: OnceLock<PathBuf> = OnceLock::new();

/// Uses the keys file at `path` (a front end's `--keys`). Call it before any archive is read:
/// the file is read once, on the first encrypted archive. Returns `false` when a path was
/// already chosen or the file already read with another.
pub fn set_path(path: impl Into<PathBuf>) -> bool {
    CHOSEN.set(path.into()).is_ok() && KEYS.get().is_none()
}

/// Where the keys file is looked for: the path given to [`set_path`], else `THEDAS_KEYS`
/// when set, else [`DEFAULT_FILE`] in the player's data folder.
pub fn path() -> PathBuf {
    if let Some(p) = CHOSEN.get() {
        return p.clone();
    }
    match std::env::var_os(ENV).filter(|v| !v.is_empty()) {
        Some(p) => PathBuf::from(p),
        None => crate::mods::user_data_dir().join(DEFAULT_FILE),
    }
}

/// The passwords in a keys file's text, DA:O's and DA2's alike, each once.
pub fn parse(text: &str) -> Vec<DaoKey> {
    let mut out: Vec<DaoKey> = Vec::new();
    let words = text.split(|c: char| c.is_whitespace() || matches!(c, ',' | ';' | '=' | '"'));
    for word in words.filter(|w| !w.is_empty()) {
        let key = if word.len() <= 20 && word.bytes().all(|b| b.is_ascii_digit()) {
            Some(DaoKey::new(word))
        } else {
            da2_password(word).map(DaoKey::from_bytes)
        };
        if let Some(k) = key {
            if !out.contains(&k) {
                out.push(k);
            }
        }
    }
    out
}

/// A DA2 password written as 32 hexadecimal digits, dashes allowed between them.
fn da2_password(word: &str) -> Option<Vec<u8>> {
    if word.starts_with('-') || word.ends_with('-') || word.contains("--") {
        return None;
    }
    let hex: Vec<u8> = word.bytes().filter(|&b| b != b'-').collect();
    if hex.len() != 32 {
        return None;
    }
    let digit = |b: u8| (b as char).to_digit(16).map(|d| d as u8);
    hex.chunks_exact(2)
        .map(|p| Some(digit(p[0])? << 4 | digit(p[1])?))
        .collect()
}

/// The password among `keys` that opens an archive with this encryption and digest.
pub fn find<'a>(
    keys: &'a [DaoKey],
    encryption: Encryption,
    digest: &[u8; 16],
) -> Option<&'a DaoKey> {
    keys.iter().find(|k| match encryption {
        Encryption::Xor | Encryption::BlowfishDao => k.matches_digest(digest),
        Encryption::BlowfishDa2 => k.da2_cipher(digest).is_some(),
        _ => false,
    })
}

/// The passwords in the player's keys file, read once per run (none when there is no file).
static KEYS: OnceLock<Vec<DaoKey>> = OnceLock::new();

fn loaded() -> &'static [DaoKey] {
    KEYS.get_or_init(|| {
        // This crate's tests open synthetic archives and must not read the player's file.
        if cfg!(test) {
            return Vec::new();
        }
        read(&path())
    })
}

/// The passwords in the keys file at `path`, logging how many (never which).
fn read(path: &Path) -> Vec<DaoKey> {
    match std::fs::read(path) {
        Ok(bytes) => {
            let keys = parse(&String::from_utf8_lossy(&bytes));
            log::info!(
                "dlc: {} passwords in the keys file {}",
                keys.len(),
                path.display()
            );
            keys
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            log::info!("dlc: no keys file at {}", path.display());
            Vec::new()
        }
        Err(e) => {
            log::warn!("dlc: cannot read the keys file {}: {e}", path.display());
            Vec::new()
        }
    }
}

/// What each archive digest matched, by whether it is DA2's.
type Found = HashMap<(bool, [u8; 16]), Option<DaoKey>>;

/// The password from the player's keys file for an archive with this encryption and digest.
pub(crate) fn key(encryption: Encryption, digest: &[u8; 16]) -> Option<DaoKey> {
    let keys = loaded();
    if keys.is_empty() {
        return None;
    }
    // Archives of one DLC share a password; each digest is matched once.
    static FOUND: OnceLock<Mutex<Found>> = OnceLock::new();
    let da2 = encryption == Encryption::BlowfishDa2;
    let mut found = FOUND
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    found
        .entry((da2, *digest))
        .or_insert_with(|| find(keys, encryption, digest).cloned())
        .clone()
}

/// What to tell a player whose Dragon Age II DLC archive cannot be read: where to put a
/// keys file.
pub fn da2_hint() -> String {
    format!(
        "Dragon Age II DLC needs a keys file (erfpasswords.txt): put it at {}, or give its path with --keys or {ENV}",
        path().display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use md5::{Digest, Md5};

    #[test]
    fn reads_both_layouts_and_both_games() {
        let da2 = "00ff8001-0203-0405-0607-08090a0b0c0d";
        let da2 = da2.replace('-', "");
        let grouped = da2
            .as_bytes()
            .chunks(4)
            .map(|c| std::str::from_utf8(c).unwrap())
            .collect::<Vec<_>>()
            .join("-");
        let text = format!(
            "Add this to pyGFF's config.ini:\r\n\r\nerfpasswords = 12345678, {grouped}, 87654321\r\n\
             \r\n=====\r\ndao_prc_cp_1      12345678  Return to Ostagar\r\n\
             da2_prc_eye    {grouped}  Mark of the Assassin\r\n\
             https://example.invalid/DAO_PRC_CP_1_1.0.dazip\r\n"
        );
        let keys = parse(&text);
        assert_eq!(keys.len(), 3, "each password once");
        assert_eq!(keys[0], DaoKey::new("12345678"));
        assert_eq!(
            keys[1],
            DaoKey::from_bytes([0, 255, 128, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13])
        );
        assert_eq!(keys[2], DaoKey::new("87654321"));
    }

    #[test]
    fn ignores_words_that_are_not_passwords() {
        assert!(parse("Feastday Gifts (DAO) ---- 0123-4567- abcd").is_empty());
        assert!(parse("0123456789abcdef0123456789abcdeg").is_empty());
        assert!(parse("-0123456789abcdef0123456789abcdef").is_empty());
    }

    #[test]
    fn a_password_is_used_only_for_the_archive_whose_digest_it_matches() {
        let keys = parse("11111111 22222222");
        let digest: [u8; 16] = Md5::digest(b"22222222").into();
        let k = find(&keys, Encryption::BlowfishDao, &digest).unwrap();
        assert_eq!(*k, DaoKey::new("22222222"));
        assert!(find(&keys, Encryption::BlowfishDa2, &digest).is_none());
        assert!(find(&keys, Encryption::BlowfishDao, &[0; 16]).is_none());
    }

    #[test]
    fn a_da2_password_opens_the_archive_whose_challenge_digest_it_makes() {
        use blowfish::Blowfish;
        use blowfish::cipher::{BlockEncrypt, KeyInit, generic_array::GenericArray};
        let bytes = [7u8, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];
        let cipher: Blowfish = Blowfish::new_from_slice(&bytes).unwrap();
        let mut challenge = std::array::from_fn::<_, 256, _>(|i| i as u8);
        for block in challenge.chunks_exact_mut(8) {
            cipher.encrypt_block(GenericArray::from_mut_slice(block));
        }
        let digest: [u8; 16] = Md5::digest(challenge).into();
        let keys =
            parse("12345678 07010203-0405-0607-0809-0a0b0c0d0e0f 0F0E0D0C0B0A09080706050403020107");
        let k = find(&keys, Encryption::BlowfishDa2, &digest).unwrap();
        assert_eq!(*k, DaoKey::from_bytes(bytes));
        assert!(find(&keys, Encryption::BlowfishDao, &digest).is_none());
    }
}
