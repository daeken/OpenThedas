//! DLC archive passwords found from the archive's own digest, for any build of the game.
//!
//! An encrypted DLC archive (ERF V2.2) stores the MD5 digest of its password, and the password
//! is eight decimal digits, so the password is one of 10^8 strings and can be found by trying
//! them against the digest. That takes a few seconds on all cores, once per password: what is
//! found is kept in the player's own data folder ([`CACHE_FILE`] in
//! [`crate::mods::user_data_dir`]), never in the repository, and every cached password is
//! checked against the digest again before use.
//!
//! The order for each archive: the password from the player's own `DAOrigins.exe` when it is
//! the GOG build ([`crate::gog_dlc`], instant), then the cache, then the search.
//!
//! **Provisional:** that every DA:O DLC password is exactly eight digits (leading zeroes kept)
//! is known from the GOG build's table, whose passwords all are; one that is not is reported
//! as not found and remembered as such, so it is not searched again on every start.

use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};

use md5::{Digest, Md5};

use crate::crypto::DaoKey;

/// The cache's file name in the player's data folder: one line per digest, the digest in hex
/// and then the password, or `-` when the search found none.
pub const CACHE_FILE: &str = "dlc-keys.txt";

/// How many passwords there are to try: every eight-digit decimal string.
const CANDIDATES: u32 = 100_000_000;

/// What is known this run, by digest: the password, or `None` when there is none to find.
fn known() -> &'static Mutex<HashMap<[u8; 16], Option<DaoKey>>> {
    static KNOWN: OnceLock<Mutex<HashMap<[u8; 16], Option<DaoKey>>>> = OnceLock::new();
    KNOWN.get_or_init(Default::default)
}

/// The password for an archive with ERF module ID `module` and password digest `digest`: from
/// the player's executable, else the cache in the player's data folder, else by search.
pub(crate) fn key(archive: &Path, module: u32, digest: &[u8; 16]) -> Option<DaoKey> {
    // `THEDAS_DLC_NO_EXE=1` skips the executable, so the search can be checked on a GOG
    // install (a test hook).
    if std::env::var_os("THEDAS_DLC_NO_EXE").is_none_or(|v| v != "1") {
        if let Some(k) = crate::gog_dlc::key(archive, module, digest) {
            return Some(k);
        }
    }
    // This crate's tests open synthetic encrypted archives without a key: they must neither
    // write to the real data folder nor spend a minute searching. The search and the cache
    // are tested directly below.
    if cfg!(test) {
        return None;
    }
    key_with_cache(
        &crate::mods::user_data_dir().join(CACHE_FILE),
        digest,
        &format!("module ID {module} ({})", archive.display()),
    )
}

/// [`key`] without the executable, with the cache at `cache`. One search runs at a time (each
/// uses every core); an archive waiting for one whose digest it shares gets its answer.
fn key_with_cache(cache: &Path, digest: &[u8; 16], what: &str) -> Option<DaoKey> {
    let mut known = known().lock().unwrap_or_else(|p| p.into_inner());
    if let Some(k) = known.get(digest) {
        return k.clone();
    }
    let found = match read_cache(cache, digest) {
        Some(k) => k,
        None => {
            log::info!(
                "dlc: searching the password for {what}; this happens once, the result is kept \
                 in {}",
                cache.display()
            );
            let started = std::time::Instant::now();
            let k = search(digest);
            match &k {
                Some(_) => log::info!(
                    "dlc: found the password for {what} in {:.1} s",
                    started.elapsed().as_secs_f32()
                ),
                None => log::info!(
                    "dlc: no eight-digit password matches {what}'s digest; its archives stay \
                     unread"
                ),
            }
            append_cache(cache, digest, k.as_ref());
            k
        }
    };
    known.insert(*digest, found.clone());
    found
}

/// The cache's answer for `digest`: `Some(Some(key))` for a password that matches the digest,
/// `Some(None)` for "searched, none found", `None` when the cache has nothing usable.
fn read_cache(cache: &Path, digest: &[u8; 16]) -> Option<Option<DaoKey>> {
    let text = std::fs::read_to_string(cache).ok()?;
    let hex = to_hex(digest);
    text.lines().rev().find_map(|line| {
        let (d, password) = line.trim().split_once(' ')?;
        if !d.eq_ignore_ascii_case(&hex) {
            return None;
        }
        if password == "-" {
            return Some(None);
        }
        let k = DaoKey::new(password);
        // A line that does not match its digest (edited, damaged) is not trusted.
        k.matches_digest(digest).then_some(Some(k))
    })
}

/// Adds a line for `digest` to the cache, creating its folder. Failing to write only costs a
/// search on the next start, so it is logged and otherwise ignored.
fn append_cache(cache: &Path, digest: &[u8; 16], key: Option<&DaoKey>) {
    let password = key
        .and_then(|k| std::str::from_utf8(k.password()).ok().map(str::to_owned))
        .unwrap_or_else(|| "-".to_owned());
    let line = format!("{} {password}\n", to_hex(digest));
    let written = cache
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|_| {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(cache)
        })
        .and_then(|mut f| f.write_all(line.as_bytes()));
    if let Err(e) = written {
        log::warn!(
            "dlc: cannot keep a found password in {}: {e}",
            cache.display()
        );
    }
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The eight-digit password whose MD5 is `digest`, tried on every core.
pub(crate) fn search(digest: &[u8; 16]) -> Option<DaoKey> {
    search_range(digest, CANDIDATES)
}

/// [`search`] over the first `count` candidates (all of them in the game; fewer in tests).
fn search_range(digest: &[u8; 16], count: u32) -> Option<DaoKey> {
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get()) as u32;
    let done = AtomicBool::new(false);
    let found = AtomicU32::new(u32::MAX);
    // Work is taken in blocks from a shared counter, so a fast core takes more.
    const BLOCK: u32 = 1 << 16;
    let next = AtomicU32::new(0);
    std::thread::scope(|s| {
        for _ in 0..threads.max(1) {
            s.spawn(|| {
                let mut text = [b'0'; 8];
                while !done.load(Ordering::Relaxed) {
                    let start = next.fetch_add(BLOCK, Ordering::Relaxed);
                    if start >= count {
                        break;
                    }
                    for n in start..start.saturating_add(BLOCK).min(count) {
                        digits(n, &mut text);
                        if Md5::digest(text).as_slice() == digest {
                            found.store(n, Ordering::Relaxed);
                            done.store(true, Ordering::Relaxed);
                            break;
                        }
                    }
                }
            });
        }
    });
    let n = found.load(Ordering::Relaxed);
    (n != u32::MAX).then(|| {
        let mut text = [b'0'; 8];
        digits(n, &mut text);
        DaoKey::from_bytes(text.to_vec())
    })
}

/// `n` as eight decimal digits, leading zeroes kept.
fn digits(mut n: u32, out: &mut [u8; 8]) {
    for d in out.iter_mut().rev() {
        *d = b'0' + (n % 10) as u8;
        n /= 10;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(text: &[u8]) -> [u8; 16] {
        Md5::digest(text).into()
    }

    /// The worst case, every candidate tried: `cargo test --release -p thedas-vfs -- --ignored
    /// --nocapture whole_range` prints how long a first start can take on this machine.
    #[test]
    #[ignore]
    fn whole_range() {
        let started = std::time::Instant::now();
        let k = search(&digest(b"99999999")).unwrap();
        assert!(k.matches_digest(&digest(b"99999999")));
        println!("whole range in {:.1} s", started.elapsed().as_secs_f32());
    }

    #[test]
    fn eight_digits_keep_their_leading_zeroes() {
        let mut t = [0u8; 8];
        digits(119922, &mut t);
        assert_eq!(&t, b"00119922");
        digits(99_999_999, &mut t);
        assert_eq!(&t, b"99999999");
    }

    #[test]
    fn finds_a_password_from_its_digest_and_not_one_outside_the_range() {
        let k = search_range(&digest(b"00119922"), 200_000).unwrap();
        assert!(k.matches_digest(&digest(b"00119922")));
        assert_eq!(k.blowfish_key(), Some(119922u64.to_le_bytes()));
        assert!(search_range(&digest(b"00119922"), 100_000).is_none());
        assert!(search_range(&digest(b"119922"), 200_000).is_none());
    }

    #[test]
    fn the_cache_is_read_back_checked_and_remembers_failures() {
        let dir = std::env::temp_dir().join(format!("thedas-dlc-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cache = dir.join("sub").join(CACHE_FILE);
        let (good, none) = (digest(b"00000042"), digest(b"not-a-number"));
        assert_eq!(read_cache(&cache, &good), None);
        append_cache(&cache, &good, Some(&DaoKey::new("00000042")));
        append_cache(&cache, &none, None);
        assert!(
            read_cache(&cache, &good)
                .unwrap()
                .unwrap()
                .matches_digest(&good)
        );
        assert!(read_cache(&cache, &none).unwrap().is_none());
        // A line whose password does not match its digest is ignored.
        let other = digest(b"00000043");
        std::fs::write(&cache, format!("{} 00000042\n", to_hex(&other))).unwrap();
        assert_eq!(read_cache(&cache, &other), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_cached_answer_skips_the_search() {
        let dir = std::env::temp_dir().join(format!("thedas-dlc-skip-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let cache = dir.join(CACHE_FILE);
        // A digest whose password lies at the far end of the range: found only from the cache
        // in the time a test has, so the answer proves the cache was used.
        let far = digest(b"99999998");
        append_cache(&cache, &far, Some(&DaoKey::new("99999998")));
        let k = key_with_cache(&cache, &far, "test").unwrap();
        assert!(k.matches_digest(&far));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
