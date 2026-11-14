//! Player mods as ordered layers over the game's own resources.
//!
//! A mod is never unpacked over the install. Each one is mounted where it sits (a folder, a zip
//! archive as downloaded, a `.dazip`, or a single `.erf`) as layers of the stack's overlay
//! ([`crate::Vfs::set_overlay`]), so it beats every game resource, including a level's own
//! archives, and can be removed by deleting one line.
//!
//! The order, lowest first:
//!
//! 1. The install's own `packages/core/override`, which [`crate::DaoInstall::global_vfs`] puts
//!    there.
//! 2. The original's user override folder, `Documents/BioWare/Dragon Age/packages/core/override`,
//!    when it exists. The original registers it at priority 0, above everything else it loads
//!    (traced in the executable: the folder registrations around 0x40e33f and 0x40e385 give the
//!    install's `packages/core/override` priority 1 and the user's priority 0, lowest number
//!    winning).
//! 3. The entries of the load-order list, top to bottom: a later line beats an earlier one.
//!    The list is our own addition; the original has no ordering between mods beyond file names.
//!
//! The list is a text file, `mods.txt` in the player's data folder ([`user_data_dir`]), one mod
//! per line:
//!
//! ```text
//! # Later lines win.
//! /mods/DragonTex
//! /mods/faces.zip
//! /mods/teeth.zip!/Pearly White
//! ```
//!
//! Blank lines and lines starting with `#` are skipped. A relative path is relative to the
//! list's folder, and a leading `~/` is the home folder. `archive.zip!/folder` mounts only that
//! folder of the archive, for mods that ship several variants with the same file names.
//!
//! Inside one mod, folders are flattened (a resource is named by its file name alone, as in
//! the original's override folder), `.erf` and `.rim` archives found in a mod folder are mounted
//! below that folder's loose files, and of two loose files with one name the later path wins.
//! Those three rules are provisional: they are not traced in the original.
//!
//! A `.7z` archive is unpacked once into the same cache folder ([`unpack_7z`]) and then mounted
//! as a folder: a 7z is one compressed stream, so serving single files out of it in place is
//! not practical. `archive.7z!/folder` picks one folder of it, as for a zip.
//!
//! A `.dazip` is the original's add-in package: a zip holding `Manifest.xml` and `Contents/`,
//! with add-ins under `Contents/addins/<uid>/` and sometimes files for
//! `Contents/packages/core/override/`. The original installs it into the player's documents.
//! Ours unpacks it once into a cache folder of our own (`mod-cache` in the data folder, never
//! the install or the original's folders; see [`unpack_dazip`]) because its add-ins hold
//! archives that have to be files to be opened. Its add-ins then join the stack as installed
//! add-ins do (their manifest's priority and campaign decide where and whether), and its
//! override files are a mod layer at the list entry's place.

use std::path::{Path, PathBuf};

use crate::install::AddIn;
use crate::vfs::{Vfs, collect_files, is_archive};
use crate::{Result, join_ci, resolve_ci};

/// Names the list file in the player's data folder.
pub const LIST_FILE: &str = "mods.txt";

/// Environment variable overriding the list: a path to a list file, or `none` (or `off`, `0`)
/// to load no mods at all, not even the user override folder.
pub const ENV_VAR: &str = "THEDAS_MODS";

/// Label prefix of a layer mounted from the original's folder in the player's documents.
pub const USER_LABEL: &str = "user:";

/// Label prefix of a layer mounted from the load-order list.
pub const MOD_LABEL: &str = "mod:";

/// A layer of a stack that does not come from the install: the player's own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayerLayer {
    /// `user:<path under the original's documents folder>`, `mod:<name>`, or
    /// [`crate::WORKSPACE_LABEL`] for mod mode's live edits.
    pub label: String,
    /// Files the layer holds, counting ones a higher layer hides.
    pub files: usize,
}

/// Every layer of `vfs` that came from the player's folders or the mod list, lowest priority
/// first: what a problem report should name, since these change what the game reads.
pub fn player_layers(vfs: &Vfs) -> Vec<PlayerLayer> {
    let of = |v: &Vfs| -> Vec<PlayerLayer> {
        v.layers()
            .iter()
            .enumerate()
            .filter(|(_, l)| l.label.starts_with(USER_LABEL) || l.label.starts_with(MOD_LABEL))
            .map(|(i, l)| PlayerLayer {
                label: l.label.clone(),
                files: v.layer_files(i),
            })
            .collect()
    };
    let mut out = of(vfs);
    if let Some(o) = vfs.overlay() {
        out.extend(of(o));
    }
    // Mod mode's live edits, when mounted: a report from such a session is a modded one.
    if let Some((_, files)) = Vfs::workspace().filter(|_| vfs.plays()) {
        out.push(PlayerLayer {
            label: crate::vfs::WORKSPACE_LABEL.to_string(),
            files,
        });
    }
    out
}

/// Logs, once per process, how much of a session's stack is the player's own.
pub(crate) fn log_player_layers(vfs: &Vfs) {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let layers = player_layers(vfs);
        let count = |prefix: &str| -> (usize, usize) {
            let mine = layers.iter().filter(|l| l.label.starts_with(prefix));
            (mine.clone().count(), mine.map(|l| l.files).sum())
        };
        let ((ul, uf), (ml, mf)) = (count(USER_LABEL), count(MOD_LABEL));
        log::info!(
            "mods: {uf} file(s) in {ul} layer(s) from the original's documents folder, {mf} file(s) in {ml} layer(s) from the mod list"
        );
    });
}

/// One line of the load-order list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModEntry {
    /// The folder or archive on disk.
    pub path: PathBuf,
    /// For a zip: the folder inside it to mount, when the line names one.
    pub inner: Option<String>,
}

/// An ordered list of mods, lowest priority first.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModList {
    pub entries: Vec<ModEntry>,
}

/// What mounting one mod did, for logs and tools.
#[derive(Clone, Debug)]
pub struct Mounted {
    pub entry: ModEntry,
    /// Layers this mod added to the overlay.
    pub layers: usize,
    /// Why it was skipped or what in it was not loaded; empty when all went well.
    pub problems: Vec<String>,
}

/// The player's own folder for saves, settings and the mod list. On Windows it is `OpenThedas`
/// in the player's Documents (the known folder, which may be redirected), where the original
/// keeps its own (`BioWare\Dragon Age`); elsewhere `.openthedas` in the home folder (`HOME`).
/// Beside the game when neither is found.
pub fn user_data_dir() -> PathBuf {
    if cfg!(windows) {
        if let Some(d) = dirs::document_dir() {
            return d.join("OpenThedas");
        }
    }
    match home_dir() {
        Some(h) => h.join(".openthedas"),
        None => PathBuf::from(".openthedas"),
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(dirs::home_dir)
}

/// The original's folder in this player's documents (`BioWare/Dragon Age`, or
/// `BioWare/Dragon Age 2`), if there is one. Looked for under the Documents folder and, off
/// Windows, under `HOME/Documents` (so a scratch `HOME` hides the real one).
pub fn original_user_dir(game: crate::install::Game) -> Option<PathBuf> {
    let docs = if cfg!(windows) {
        dirs::document_dir()
    } else {
        home_dir().map(|h| h.join("Documents"))
    }?;
    let name = match game {
        crate::install::Game::Origins => "BioWare/Dragon Age",
        crate::install::Game::DragonAge2 => "BioWare/Dragon Age 2",
    };
    resolve_ci(&docs, name).filter(|d| d.is_dir())
}

impl ModList {
    /// Parses a list. `base` is the folder relative paths are resolved against.
    pub fn parse(text: &str, base: &Path) -> Self {
        let home = home_dir();
        let entries = text
            .lines()
            .map(|l| l.trim().trim_start_matches('\u{feff}'))
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(|line| {
                let resolve = |p: &str| {
                    let p = match (p.strip_prefix("~/"), &home) {
                        (Some(rest), Some(h)) => h.join(rest),
                        _ => PathBuf::from(p),
                    };
                    if p.is_absolute() { p } else { base.join(p) }
                };
                // `archive!/folder`, unless the whole line is itself a path that exists.
                if let Some((file, inner)) = line.split_once("!/") {
                    if !resolve(line).exists() {
                        return ModEntry {
                            path: resolve(file.trim_end()),
                            inner: Some(inner.trim().to_string()).filter(|s| !s.is_empty()),
                        };
                    }
                }
                ModEntry {
                    path: resolve(line),
                    inner: None,
                }
            })
            .collect();
        Self { entries }
    }

    /// Reads a list file. A missing file is an empty list.
    pub fn load(file: &Path) -> Self {
        match std::fs::read(file) {
            Ok(bytes) => Self::parse(
                &String::from_utf8_lossy(&bytes),
                file.parent().unwrap_or(Path::new(".")),
            ),
            Err(e) => {
                if e.kind() != std::io::ErrorKind::NotFound {
                    log::warn!("mods: cannot read {}: {e}", file.display());
                }
                Self::default()
            }
        }
    }

    /// Mounts every entry onto `overlay`, in order. A mod that is missing or unreadable is
    /// reported and skipped; the rest still load. `.dazip` packages are unpacked into
    /// [`cache_dir`].
    pub fn mount(&self, overlay: &mut Vfs) -> Vec<Mounted> {
        self.mount_with_cache(overlay, &cache_dir())
    }

    /// [`ModList::mount`] with `.dazip` packages unpacked into `cache`.
    pub fn mount_with_cache(&self, overlay: &mut Vfs, cache: &Path) -> Vec<Mounted> {
        self.entries
            .iter()
            .map(|entry| {
                let before = overlay.layers().len();
                let mut problems = Vec::new();
                if let Err(e) = mount_entry(overlay, entry, cache, &mut problems) {
                    overlay.truncate(before);
                    problems.push(e.to_string());
                }
                Mounted {
                    entry: entry.clone(),
                    layers: overlay.layers().len() - before,
                    problems,
                }
            })
            .collect()
    }
}

fn label(entry: &ModEntry, what: &Path) -> String {
    let name = |p: &Path| {
        p.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| p.to_string_lossy().into_owned())
    };
    let mut l = format!("{MOD_LABEL}{}", name(&entry.path));
    if let Some(inner) = &entry.inner {
        l.push_str("!/");
        l.push_str(inner);
    }
    if what != entry.path {
        if let Ok(rel) = what.strip_prefix(&entry.path) {
            l.push('/');
            l.push_str(&rel.to_string_lossy().replace('\\', "/"));
        }
    }
    l
}

/// Mounts a folder as a mod: the archives in it first (lowest), then its loose files.
fn mount_folder(
    overlay: &mut Vfs,
    dir: &Path,
    label: &dyn Fn(&Path) -> String,
    problems: &mut Vec<String>,
) -> Result<()> {
    let mut files = Vec::new();
    collect_files(dir, true, &mut files)?;
    files.retain(|p| is_archive(p));
    files.sort();
    for archive in files {
        if let Err(e) = overlay.push_archive(&archive, label(&archive)) {
            problems.push(e.to_string());
        }
    }
    overlay.push_directory(dir, true, label(dir))
}

fn mount_entry(
    overlay: &mut Vfs,
    entry: &ModEntry,
    cache: &Path,
    problems: &mut Vec<String>,
) -> Result<()> {
    let path = &entry.path;
    if path.is_dir() {
        return mount_folder(overlay, path, &|p| label(entry, p), problems);
    }
    if !path.is_file() {
        problems.push("no such file or folder".into());
        return Ok(());
    }
    if is_archive(path) {
        return overlay.push_archive(path, label(entry, path));
    }
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "dazip" => {
            // The add-ins inside join the stack through `default_list_addins`; here only the
            // package's override files, as a mod layer.
            let package = unpack_dazip(path, cache)?;
            problems.extend(package.problems);
            let label = |p: &Path| label_under(cache, p);
            match &package.override_dir {
                Some(dir) => mount_folder(overlay, dir, &label, problems)?,
                None if package.addins.is_empty() => {
                    problems.push("no add-in and no override files found in the package".into())
                }
                None => {}
            }
        }
        "7z" => {
            let (root, skipped) = unpack_7z(path, cache)?;
            problems.extend(skipped);
            let dir = match entry.inner.as_deref() {
                Some(inner) => match resolve_ci(&root, inner).filter(|d| d.is_dir()) {
                    Some(dir) => dir,
                    None => {
                        problems.push(format!("no folder {inner:?} in the archive"));
                        return Ok(());
                    }
                },
                None => root,
            };
            let label = |p: &Path| label_under(cache, p);
            mount_folder(overlay, &dir, &label, problems)?;
        }
        "zip" | "override" => {
            let skipped = overlay.push_zip(path, entry.inner.as_deref(), label(entry, path))?;
            if !skipped.is_empty() {
                problems.push(format!(
                    "{} archive(s) inside the zip were not loaded (unpack the mod to a folder to use them), first: {}",
                    skipped.len(),
                    skipped[0]
                ));
            }
        }
        _ => problems.push(
            "not a folder, .zip, .7z, .dazip, .erf or .rim (unpack other archive types to a folder and list the folder)"
                .into(),
        ),
    }
    Ok(())
}

/// Where `.dazip` packages are unpacked: `mod-cache` in the player's data folder.
pub fn cache_dir() -> PathBuf {
    user_data_dir().join(CACHE_DIR_NAME)
}

/// The cache folder's name inside the data folder.
const CACHE_DIR_NAME: &str = "mod-cache";

/// The label of a layer served from the cache: `mod:<package folder>/<path inside it>`.
pub(crate) fn cache_label(path: &Path) -> String {
    label_under(&cache_dir(), path)
}

fn label_under(cache: &Path, path: &Path) -> String {
    let rel = path.strip_prefix(cache).unwrap_or(path);
    let rel = rel
        .to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/");
    format!("{MOD_LABEL}{rel}")
}

/// Where an archive is unpacked inside `cache`: a folder named after the file, its size and
/// its modification time, so a changed archive is unpacked afresh and an unchanged one is
/// reused. Also returns the file's name made safe for a folder name.
fn cache_slot(path: &Path, cache: &Path) -> Result<(PathBuf, String)> {
    let meta = std::fs::metadata(path).map_err(|e| crate::Error::Io {
        path: path.to_path_buf(),
        source: e,
    })?;
    let modified = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs());
    let stem: String = path
        .file_stem()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(48)
        .collect();
    let root = cache.join(format!("{stem}-{:x}-{modified:x}", meta.len()));
    Ok((root, stem))
}

/// A path inside an archive as a relative path that cannot leave the folder it is unpacked
/// into: no root, no drive, no `..`. `None` for anything else, and for an empty name.
fn contained(name: &str) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for part in name.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => return None,
            p if p.contains(':') => return None,
            p => out.push(p),
        }
    }
    (!name.starts_with(['/', '\\']) && out.components().next().is_some()).then_some(out)
}

/// Unpacks a `.7z` archive into its own folder under `cache` ([`cache_slot`]) and returns the
/// folder and what was skipped. A 7z archive is usually one compressed stream, so reading a
/// single file out of it means decoding everything before it; unpacking once is the only
/// practical way to serve it. The same rules as [`unpack_dazip`]: nothing is written outside
/// the folder, an entry whose path would leave it is skipped and reported, the size limits
/// hold, and nothing is run. Encrypted archives are refused.
pub fn unpack_7z(path: &Path, cache: &Path) -> Result<(PathBuf, Vec<String>)> {
    let bad = |reason: String| crate::Error::Malformed {
        path: path.to_path_buf(),
        reason,
    };
    let (root, stem) = cache_slot(path, cache)?;
    let mut problems = Vec::new();
    if root.is_dir() {
        return Ok((root, problems));
    }
    let mut archive = sevenz_rust2::ArchiveReader::open(path, sevenz_rust2::Password::empty())
        .map_err(|e| bad(format!("not a readable 7z: {e}")))?;
    if archive.archive().files.len() > MAX_DAZIP_ENTRIES {
        return Err(bad(format!("{} entries", archive.archive().files.len())));
    }
    let partial = cache.join(format!(".{stem}-{}.partial", std::process::id()));
    let _ = std::fs::remove_dir_all(&partial);
    std::fs::create_dir_all(&partial).map_err(|e| crate::Error::Io {
        path: partial.clone(),
        source: e,
    })?;
    let mut left = MAX_DAZIP_BYTES;
    let mut failure: Option<String> = None;
    let walked = archive.for_each_entries(|entry, reader| {
        let name = entry.name().to_string();
        let Some(rel) = contained(&name) else {
            problems.push(format!("{name}: path leaves the archive, skipped"));
            // The entry's bytes still have to be read past.
            std::io::copy(reader, &mut std::io::sink())?;
            return Ok(true);
        };
        let out = partial.join(rel);
        if entry.is_directory() {
            std::fs::create_dir_all(&out)?;
            return Ok(true);
        }
        if entry.size() > left {
            failure = Some("unpacks past the size limit".into());
            return Ok(false);
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut to = std::fs::File::create(&out)?;
        let copied = {
            use std::io::Read;
            std::io::copy(&mut reader.take(left + 1), &mut to)?
        };
        if copied > left {
            failure = Some("unpacks past the size limit".into());
            return Ok(false);
        }
        left -= copied;
        Ok(true)
    });
    if let Some(reason) = failure.or_else(|| walked.err().map(|e| e.to_string())) {
        let _ = std::fs::remove_dir_all(&partial);
        return Err(bad(reason));
    }
    // Another session may have unpacked the same archive meanwhile: keep theirs.
    if std::fs::rename(&partial, &root).is_err() {
        let _ = std::fs::remove_dir_all(&partial);
        if !root.is_dir() {
            return Err(bad(
                "could not move the unpacked archive into the cache".into()
            ));
        }
    }
    Ok((root, problems))
}

/// One unpacked archive in the mod cache.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CacheEntry {
    /// Its folder, directly inside the cache.
    pub dir: PathBuf,
    /// Bytes and files it holds.
    pub bytes: u64,
    pub files: usize,
    /// Whether an archive in the list unpacks to this folder, as the archive is now.
    pub in_use: bool,
}

/// Whether a name is one this module gives a folder in the cache: `<name>-<size>-<time>` with
/// the last two in hexadecimal ([`cache_slot`]), or a leftover `.<name>-<pid>.partial`.
fn is_cache_name(name: &str) -> bool {
    let hex = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_hexdigit());
    let safe = |s: &str| {
        s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    };
    if let Some(inner) = name
        .strip_prefix('.')
        .and_then(|n| n.strip_suffix(".partial"))
    {
        return safe(inner);
    }
    let mut parts = name.rsplitn(3, '-');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(time), Some(size), Some(stem)) => hex(time) && hex(size) && safe(stem),
        _ => false,
    }
}

/// What the mod cache holds: every folder directly inside `cache` that carries one of our
/// names, with its size and whether `list` still uses it. Anything else found there is left
/// out, and never touched.
pub fn cache_report(cache: &Path, list: &ModList) -> Vec<CacheEntry> {
    let used: Vec<PathBuf> = list
        .entries
        .iter()
        .filter(|e| {
            let ext = e.path.extension().map(|x| x.to_ascii_lowercase());
            e.path.is_file()
                && matches!(
                    ext.as_deref().and_then(|x| x.to_str()),
                    Some("7z" | "dazip")
                )
        })
        .filter_map(|e| cache_slot(&e.path, cache).ok().map(|(dir, _)| dir))
        .collect();
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(cache) else {
        return out;
    };
    for entry in rd.flatten() {
        let dir = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        // A real folder of ours: not a link, not a file, not a stranger.
        let real_dir = std::fs::symlink_metadata(&dir).is_ok_and(|m| m.file_type().is_dir());
        if !real_dir || !is_cache_name(&name) {
            continue;
        }
        let mut files = Vec::new();
        let _ = collect_files(&dir, true, &mut files);
        let bytes = files
            .iter()
            .filter_map(|f| std::fs::symlink_metadata(f).ok())
            .map(|m| m.len())
            .sum();
        out.push(CacheEntry {
            in_use: used.contains(&dir),
            dir,
            bytes,
            files: files.len(),
        });
    }
    out.sort_by(|a, b| a.dir.cmp(&b.dir));
    out
}

/// Deletes the cache folders `list` no longer uses and returns them. Only ever removes what
/// [`cache_report`] lists (our own folders, directly inside `cache`), and only when `cache` is
/// a folder named `mod-cache`: it cannot be pointed at a mod folder or anything else.
pub fn prune_cache(cache: &Path, list: &ModList) -> Vec<CacheEntry> {
    if cache.file_name().and_then(|n| n.to_str()) != Some(CACHE_DIR_NAME) {
        return Vec::new();
    }
    let mut removed = Vec::new();
    for entry in cache_report(cache, list) {
        if !entry.in_use && std::fs::remove_dir_all(&entry.dir).is_ok() {
            removed.push(entry);
        }
    }
    removed
}

/// An unpacked `.dazip`.
#[derive(Clone, Debug)]
pub struct Dazip {
    /// The folder it was unpacked into.
    pub root: PathBuf,
    /// Its add-ins (`Contents/addins/*`), described by the package's manifest and enabled.
    pub addins: Vec<AddIn>,
    /// Its `Contents/packages/core/override` folder, when it has one.
    pub override_dir: Option<PathBuf>,
    /// Entries that were not unpacked, and why.
    pub problems: Vec<String>,
}

/// The most a package may unpack to, and the most entries it may hold.
const MAX_DAZIP_BYTES: u64 = 16 << 30;
const MAX_DAZIP_ENTRIES: usize = 200_000;

/// Unpacks a `.dazip` into its own folder under `cache` (named after the file, its size and
/// its modification time, so a changed package is unpacked afresh and an unchanged one is
/// reused), and describes what it holds. Nothing is written outside that folder: an entry
/// whose path would leave it, a link, or anything past the size limits is skipped and
/// reported. Nothing in the package is ever run.
pub fn unpack_dazip(path: &Path, cache: &Path) -> Result<Dazip> {
    let io = |at: &Path| {
        let at = at.to_path_buf();
        move |e: std::io::Error| crate::Error::Io {
            path: at.clone(),
            source: e,
        }
    };
    let bad = |reason: String| crate::Error::Malformed {
        path: path.to_path_buf(),
        reason,
    };
    let (root, stem) = cache_slot(path, cache)?;
    let mut problems = Vec::new();
    if !root.is_dir() {
        let file = std::fs::File::open(path).map_err(io(path))?;
        let mut archive = zip::ZipArchive::new(file).map_err(|e| bad(format!("not a zip: {e}")))?;
        if archive.len() > MAX_DAZIP_ENTRIES {
            return Err(bad(format!("{} entries", archive.len())));
        }
        let partial = cache.join(format!(".{stem}-{}.partial", std::process::id()));
        let _ = std::fs::remove_dir_all(&partial);
        std::fs::create_dir_all(&partial).map_err(io(&partial))?;
        let mut left = MAX_DAZIP_BYTES;
        for i in 0..archive.len() {
            let mut entry = archive
                .by_index(i)
                .map_err(|e| bad(format!("entry {i}: {e}")))?;
            let name = entry.name().to_string();
            // A path that stays inside the folder, or nothing.
            let Some(rel) = entry.enclosed_name() else {
                problems.push(format!("{name}: path leaves the package, skipped"));
                continue;
            };
            let link = entry.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000);
            if link {
                problems.push(format!("{name}: a link, skipped"));
                continue;
            }
            let out = partial.join(rel);
            if entry.is_dir() {
                std::fs::create_dir_all(&out).map_err(io(&out))?;
                continue;
            }
            if entry.size() > left {
                let _ = std::fs::remove_dir_all(&partial);
                return Err(bad("unpacks past the size limit".into()));
            }
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent).map_err(io(parent))?;
            }
            let mut to = std::fs::File::create(&out).map_err(io(&out))?;
            let copied = {
                use std::io::Read;
                std::io::copy(&mut (&mut entry).take(left + 1), &mut to)
                    .map_err(|e| bad(format!("{name}: {e}")))?
            };
            if copied > left {
                let _ = std::fs::remove_dir_all(&partial);
                return Err(bad("unpacks past the size limit".into()));
            }
            left -= copied;
        }
        // Another session may have unpacked the same package meanwhile: keep theirs.
        if std::fs::rename(&partial, &root).is_err() {
            let _ = std::fs::remove_dir_all(&partial);
            if !root.is_dir() {
                return Err(bad(
                    "could not move the unpacked package into the cache".into()
                ));
            }
        }
    }
    let manifest = resolve_ci(&root, "manifest.xml")
        .filter(|p| p.is_file())
        .and_then(|p| std::fs::read(p).ok())
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default();
    let mut addins = Vec::new();
    let items = crate::install::manifest_items(&manifest);
    let addins_dir = join_ci(&root, "Contents/addins");
    if let Ok(rd) = std::fs::read_dir(&addins_dir) {
        let mut dirs: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
        dirs.retain(|p| p.is_dir());
        dirs.sort();
        for dir in dirs {
            let name = dir.file_name().unwrap().to_string_lossy().into_owned();
            // A package with several add-ins has a manifest item for each, matched by UID
            // to its folder; a folder no item names takes the first item's settings under
            // its own name.
            let mut addin = match items.iter().find(|i| i.uid.eq_ignore_ascii_case(&name)) {
                Some(i) => AddIn { dir, ..i.clone() },
                None => {
                    let mut a = crate::install::addin_from_manifest(&manifest, dir, name.clone());
                    if !addins.is_empty() || !items.is_empty() {
                        a.uid = name;
                    }
                    a
                }
            };
            // Listing the package is the player's way of enabling it.
            addin.enabled = true;
            addins.push(addin);
        }
    }
    let override_dir =
        Some(join_ci(&root, "Contents/packages/core/override")).filter(|d| d.is_dir());
    Ok(Dazip {
        root,
        addins,
        override_dir,
        problems,
    })
}

/// The add-ins packaged in the `.dazip` files of the list in effect, unpacked on first use. A
/// package that cannot be unpacked is skipped here and reported when the list is mounted.
pub fn default_list_addins() -> Vec<AddIn> {
    let Some(list_file) = default_list_file() else {
        return Vec::new();
    };
    ModList::load(&list_file).dazip_addins(&cache_dir())
}

impl ModList {
    /// The add-ins of every `.dazip` in the list, unpacked into `cache`.
    pub fn dazip_addins(&self, cache: &Path) -> Vec<AddIn> {
        self.entries
            .iter()
            .filter(|e| {
                e.path.is_file()
                    && e.path
                        .extension()
                        .is_some_and(|x| x.eq_ignore_ascii_case("dazip"))
            })
            .filter_map(|e| unpack_dazip(&e.path, cache).ok())
            .flat_map(|d| d.addins)
            .collect()
    }
}

/// The list file in effect: `THEDAS_MODS` when set, else `mods.txt` in the data folder. `None`
/// when mods are switched off.
pub fn default_list_file() -> Option<PathBuf> {
    match std::env::var_os(ENV_VAR).filter(|v| !v.is_empty()) {
        Some(v) => {
            let s = v.to_string_lossy().to_ascii_lowercase();
            (!matches!(s.as_str(), "none" | "off" | "0")).then(|| PathBuf::from(v))
        }
        None => Some(user_data_dir().join(LIST_FILE)),
    }
}

/// Adds this player's mod layers to the stack's overlay, above whatever it already holds (the
/// install's own override folder): the `packages/core/override` folder of `user` (the
/// original's folder in the player's documents), then the load-order list. With neither, this
/// does nothing at all.
pub fn mount_default(vfs: &mut Vfs, user: Option<&Path>) -> Vec<Mounted> {
    let Some(list_file) = default_list_file() else {
        return Vec::new();
    };
    let mut overlay = vfs.take_overlay().unwrap_or_default();
    let user_override = user
        .map(|u| join_ci(u, "packages/core/override"))
        .filter(|d| d.is_dir());
    if let Some(dir) = user_override {
        // Mounted as a mod folder is (archives in it below its loose files), under `user:`.
        let before = overlay.layers().len();
        let entry = ModEntry {
            path: dir.clone(),
            inner: None,
        };
        let mut problems = Vec::new();
        if let Err(e) = mount_entry(&mut overlay, &entry, &cache_dir(), &mut problems) {
            problems.push(e.to_string());
        }
        for p in problems {
            log::warn!("mods: user override folder: {p}");
        }
        overlay.relabel(before, |l| {
            let rest = l.strip_prefix(MOD_LABEL).unwrap_or(l);
            format!("{USER_LABEL}packages/core/{rest}")
        });
    }
    let report = ModList::load(&list_file).mount(&mut overlay);
    for m in &report {
        let name = m.entry.path.display();
        for p in &m.problems {
            log::warn!("mods: {name}: {p}");
        }
        log::info!("mods: mounted {name} ({} layer(s))", m.layers);
    }
    vfs.set_overlay(overlay);
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("thedas-mods-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn write(root: &Path, rel: &str, bytes: &[u8]) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    }

    fn zip_of(path: &Path, files: &[(&str, &[u8])]) {
        let mut w = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, data) in files {
            w.start_file(*name, opts).unwrap();
            w.write_all(data).unwrap();
        }
        w.finish().unwrap();
    }

    #[test]
    fn parses_comments_relative_paths_and_inner_folders() {
        let base = scratch("parse");
        write(&base, "odd!/name/x.dds", b"x");
        let list = ModList::parse(
            "\u{feff}# order\n\n  first  \n/abs/second.zip!/Pearly White\nthird.zip!/\nodd!/name\n",
            &base,
        );
        let e = &list.entries;
        assert_eq!(e.len(), 4);
        assert_eq!(
            (e[0].path.clone(), e[0].inner.clone()),
            (base.join("first"), None)
        );
        assert_eq!(e[1].path, PathBuf::from("/abs/second.zip"));
        assert_eq!(e[1].inner.as_deref(), Some("Pearly White"));
        assert_eq!(
            (e[2].path.clone(), e[2].inner.clone()),
            (base.join("third.zip"), None)
        );
        // A real folder whose name holds `!/` is taken whole.
        assert_eq!(
            (e[3].path.clone(), e[3].inner.clone()),
            (base.join("odd!/name"), None)
        );
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn later_mods_win_and_beat_layers_pushed_afterwards() {
        let base = scratch("order");
        write(&base, "game/face.dds", b"game");
        write(&base, "game/only_game.dds", b"g");
        write(&base, "level/face.dds", b"level");
        write(&base, "a/Textures/FACE.dds", b"mod a");
        write(&base, "a/a_only.dds", b"a");
        zip_of(
            &base.join("b.zip"),
            &[
                ("B Mod/readme.txt", b"hi"),
                ("B Mod/Pearly White/face.dds", b"pearly"),
                ("B Mod/Pearly White/teeth.dds", b"pearly teeth"),
                ("B Mod/Dingy Dog/face.dds", b"dingy"),
            ],
        );
        std::fs::write(
            base.join("mods.txt"),
            "a\nmissing-mod\nb.zip!/b mod/pearly white\n",
        )
        .unwrap();

        let mut vfs = Vfs::new();
        vfs.push_directory(base.join("game"), false, "game")
            .unwrap();
        let mut overlay = Vfs::new();
        let report = ModList::load(&base.join("mods.txt")).mount(&mut overlay);
        assert_eq!(
            report.iter().map(|m| m.layers).collect::<Vec<_>>(),
            [1, 0, 1]
        );
        assert!(report[0].problems.is_empty() && report[2].problems.is_empty());
        assert_eq!(report[1].problems.len(), 1);
        vfs.set_overlay(overlay);

        // The last mod wins, the first shows through where the last has no file.
        let (data, prov) = vfs.read("FACE.DDS").unwrap();
        assert_eq!(data, b"pearly");
        assert_eq!(prov.layer_label, "mod:b.zip!/b mod/pearly white");
        assert_eq!(prov.entry.as_deref(), Some("B Mod/Pearly White/face.dds"));
        assert_eq!(vfs.read("a_only.dds").unwrap().0, b"a");
        assert_eq!(vfs.read("teeth.dds").unwrap().0, b"pearly teeth");
        assert_eq!(vfs.read("only_game.dds").unwrap().0, b"g");
        assert!(vfs.lookup("readme.txt").is_none());
        let labels: Vec<String> = vfs
            .hits("face.dds")
            .into_iter()
            .map(|p| p.layer_label)
            .collect();
        assert_eq!(labels, ["mod:b.zip!/b mod/pearly white", "mod:a", "game"]);
        assert_eq!(vfs.len(), 4);
        assert_eq!(vfs.names().len(), 4);

        // A level mounted later still loses to the mods, and unmounts cleanly.
        let mark = vfs.mark();
        vfs.push_directory(base.join("level"), false, "level")
            .unwrap();
        assert_eq!(vfs.read("face.dds").unwrap().0, b"pearly");
        assert_eq!(vfs.hits("face.dds").len(), 4);
        vfs.truncate(mark);
        assert_eq!(vfs.hits("face.dds").len(), 3);

        // Without the overlay the game's own file is back.
        assert!(vfs.take_overlay().is_some());
        assert_eq!(vfs.read("face.dds").unwrap().0, b"game");
        assert_eq!(vfs.len(), 2);
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn a_folder_mods_archives_sit_below_its_loose_files() {
        let base = scratch("erf");
        // Not an archive at all: reported, and the folder's loose files still load.
        write(&base, "m/broken.erf", b"not an erf");
        write(&base, "m/sub/item.uti", b"loose");
        zip_of(
            &base.join("z.zip"),
            &[("inner.erf", b"x"), ("ok.dds", b"ok")],
        );
        write(&base, "notes.7z", b"7z");
        let list = ModList::parse("m\nz.zip\nnotes.7z\n", &base);
        let mut overlay = Vfs::new();
        let report = list.mount(&mut overlay);
        assert_eq!(report[0].layers, 1);
        assert_eq!(report[0].problems.len(), 1);
        assert_eq!(overlay.read("item.uti").unwrap().0, b"loose");
        assert!(overlay.lookup("broken.erf").is_none());
        assert_eq!(report[1].layers, 1);
        assert!(report[1].problems[0].contains("inner.erf"));
        assert_eq!(overlay.read("ok.dds").unwrap().0, b"ok");
        assert_eq!(report[2].layers, 0);
        assert_eq!(report[2].problems.len(), 1);
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn a_bad_zip_is_reported_not_fatal() {
        let base = scratch("badzip");
        write(&base, "bad.zip", b"PK\x03\x04 truncated");
        let mut overlay = Vfs::new();
        let report = ModList::parse("bad.zip\n", &base).mount(&mut overlay);
        assert_eq!(report[0].layers, 0);
        assert_eq!(report[0].problems.len(), 1);
        assert!(overlay.is_empty());
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn every_add_in_of_a_dazip_takes_its_own_manifest_item() {
        let base = std::env::temp_dir().join(format!("thedas-dazip-many-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let cache = base.join("mod-cache");
        std::fs::create_dir_all(&cache).unwrap();
        let package = base.join("two.dazip");
        let manifest = br#"<Manifest Type="AddIn"><AddInsList>
            <AddInItem UID="first_one" ExtendedModuleUID="Single Player" Priority="10" Enabled="1"><Type>2</Type></AddInItem>
            <AddInItem UID="second_one" ExtendedModuleUID="" Priority="40" Enabled="1"><Type>2</Type></AddInItem>
            </AddInsList></Manifest>"#;
        zip_of(
            &package,
            &[
                ("Manifest.xml", manifest),
                ("Contents/addins/first_one/core/data/a.uti", b"a"),
                ("Contents/addins/second_one/core/env/newlvl.arl", b"arl"),
                ("Contents/addins/third/core/data/c.uti", b"c"),
            ],
        );
        let d = unpack_dazip(&package, &cache).unwrap();
        let got: Vec<(&str, i64, bool)> = d
            .addins
            .iter()
            .map(|a| (a.uid.as_str(), a.priority, a.extends_base_campaign()))
            .collect();
        // Each by its own item; a folder no item names takes the first item's settings.
        assert_eq!(
            got,
            [
                ("first_one", 10, true),
                ("second_one", 40, false),
                ("third", 10, true)
            ]
        );
        // Their levels are searched with the install's.
        let install_root = base.join("install");
        std::fs::create_dir_all(install_root.join("packages/core/env")).unwrap();
        std::fs::create_dir_all(install_root.join("modules/single player")).unwrap();
        let install = crate::install::DaoInstall::open(&install_root).unwrap();
        let dirs = install.env_dirs_with(d.addins.clone()).unwrap();
        assert!(
            dirs.iter().any(|p| p.ends_with("second_one/core/env")),
            "{dirs:?}"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_dazip_is_unpacked_once_into_the_cache_and_split_into_add_in_and_override() {
        let base = scratch("dazip");
        let cache = base.join("cache");
        let package = base.join("My Mod (v1).dazip");
        let manifest = br#"<Manifest Type="AddIn"><AddInsList><AddInItem UID="my_mod"
            Name="My Mod" ExtendedModuleUID="Single Player" Priority="77" Enabled="0">"#;
        zip_of(
            &package,
            &[
                ("Manifest.xml", manifest),
                ("Contents/addins/my_mod/core/data/my_item.uti", b"item"),
                ("Contents/addins/my_mod/module/data/my_area.are", b"area"),
                (
                    "Contents/packages/core/override/sub/face.dds",
                    b"dazip face",
                ),
                ("../escape.txt", b"no"),
                ("Contents/../../escape2.txt", b"no"),
            ],
        );
        let d = unpack_dazip(&package, &cache).unwrap();
        assert!(d.root.starts_with(&cache));
        assert_eq!(d.problems.len(), 2, "{:?}", d.problems);
        assert!(!base.join("escape.txt").exists() && !cache.join("escape.txt").exists());
        assert!(!base.join("escape2.txt").exists());
        assert_eq!(d.addins.len(), 1);
        let a = &d.addins[0];
        assert_eq!(
            (a.uid.as_str(), a.priority, a.enabled),
            ("my_mod", 77, true)
        );
        assert!(a.extends_base_campaign());
        assert_eq!(
            std::fs::read(a.dir.join("core/data/my_item.uti")).unwrap(),
            b"item"
        );
        assert!(d.override_dir.is_some());

        // Unpacked once: a second call reuses the folder, even with the package gone bad.
        let marker = d.root.join("marker");
        std::fs::write(&marker, b"x").unwrap();
        let again = unpack_dazip(&package, &cache).unwrap();
        assert_eq!(again.root, d.root);
        assert!(marker.exists() && again.problems.is_empty());

        // In a list: its override files are a mod layer; its add-ins are handed out apart.
        let list = ModList::parse("My Mod (v1).dazip\n", &base);
        let mut overlay = Vfs::new();
        let report = list.mount_with_cache(&mut overlay, &cache);
        assert_eq!(report[0].layers, 1);
        assert_eq!(overlay.read("face.dds").unwrap().0, b"dazip face");
        assert!(overlay.lookup("my_item.uti").is_none());
        let label = &overlay.layers()[0].label;
        assert!(label.starts_with("mod:My_Mod__v1_-"), "{label}");
        assert!(
            label.ends_with("/Contents/packages/core/override"),
            "{label}"
        );
        let addins = list.dazip_addins(&cache);
        assert_eq!(addins.len(), 1);
        assert_eq!(addins[0].dir, a.dir);

        // Not a zip at all: reported, nothing mounted, nothing left in the cache.
        write(&base, "broken.dazip", b"junk");
        let mut overlay = Vfs::new();
        let report = ModList::parse("broken.dazip\n", &base).mount_with_cache(&mut overlay, &cache);
        assert_eq!((report[0].layers, report[0].problems.len()), (0, 1));
        assert_eq!(std::fs::read_dir(&cache).unwrap().count(), 1);
        std::fs::remove_dir_all(&base).unwrap();
    }

    fn sevenz_of(path: &Path, files: &[(&str, &[u8])]) {
        let mut w = sevenz_rust2::ArchiveWriter::create(path).unwrap();
        for (name, data) in files {
            w.push_archive_entry(
                sevenz_rust2::ArchiveEntry::new_file(name),
                Some(std::io::Cursor::new(data.to_vec())),
            )
            .unwrap();
        }
        w.finish().unwrap();
    }

    #[test]
    fn a_7z_is_unpacked_once_into_the_cache_and_mounted_as_a_folder() {
        let base = scratch("7z");
        let cache = base.join("cache");
        sevenz_of(
            &base.join("Faces (v2).7z"),
            &[
                ("Faces/Pearly White/face.dds", b"pearly"),
                ("Faces/Pearly White/sub/teeth.dds", b"pearly teeth"),
                ("Faces/Dingy Dog/face.dds", b"dingy"),
                ("Faces/readme.txt", b"hi"),
                ("../escape.txt", b"no"),
                ("C:/escape2.txt", b"no"),
            ],
        );
        // The whole archive, then one folder of it.
        let list = ModList::parse("Faces (v2).7z\nFaces (v2).7z!/faces/pearly white\n", &base);
        let mut overlay = Vfs::new();
        let report = list.mount_with_cache(&mut overlay, &cache);
        assert_eq!(report[0].layers, 1);
        assert_eq!(report[0].problems.len(), 2, "{:?}", report[0].problems);
        assert_eq!(report[1].layers, 1);
        assert!(report[1].problems.len() <= 2);
        assert!(!base.join("escape.txt").exists() && !cache.join("escape.txt").exists());
        assert_eq!(overlay.read("face.dds").unwrap().0, b"pearly");
        assert_eq!(overlay.read("teeth.dds").unwrap().0, b"pearly teeth");
        assert_eq!(overlay.read("readme.txt").unwrap().0, b"hi");
        let top = overlay.lookup("face.dds").unwrap();
        assert!(
            top.layer_label.starts_with("mod:Faces__v2_-"),
            "{}",
            top.layer_label
        );
        // (Lowercased: a case-insensitive volume keeps the case the list asked for.)
        let label = top.layer_label.to_ascii_lowercase();
        assert!(label.ends_with("/faces/pearly white"), "{label}");
        // Unpacked once: one folder in the cache, reused by the second entry.
        assert_eq!(std::fs::read_dir(&cache).unwrap().count(), 1);

        // A folder the archive does not have, and a file that is not a 7z.
        write(&base, "broken.7z", b"7z junk");
        let list = ModList::parse("Faces (v2).7z!/nope\nbroken.7z\n", &base);
        let mut overlay = Vfs::new();
        let report = list.mount_with_cache(&mut overlay, &cache);
        assert_eq!((report[0].layers, report[1].layers), (0, 0));
        assert!(report[0].problems.iter().any(|p| p.contains("nope")));
        assert_eq!(report[1].problems.len(), 1);
        assert_eq!(std::fs::read_dir(&cache).unwrap().count(), 1);
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn paths_that_would_leave_the_folder_are_refused() {
        assert_eq!(contained("a/b.txt"), Some(PathBuf::from("a/b.txt")));
        assert_eq!(contained("a\\b.txt"), Some(PathBuf::from("a/b.txt")));
        assert_eq!(contained("./a//b.txt"), Some(PathBuf::from("a/b.txt")));
        for bad in [
            "../a",
            "a/../../b",
            "/etc/x",
            "\\x",
            "C:/x",
            "a/C:b",
            "",
            ".",
        ] {
            assert_eq!(contained(bad), None, "{bad}");
        }
    }

    #[test]
    fn the_cache_is_reported_and_pruned_without_touching_anything_else() {
        let base = scratch("cachereport");
        let cache = base.join("mod-cache");
        sevenz_of(&base.join("kept.7z"), &[("a/face.dds", b"kept face")]);
        sevenz_of(
            &base.join("dropped.7z"),
            &[("b.dds", b"dropped"), ("c.dds", b"c")],
        );
        let both = ModList::parse("kept.7z\ndropped.7z\nloose\n", &base);
        write(&base, "loose/face.dds", b"loose mod, not in the cache");
        both.mount_with_cache(&mut Vfs::new(), &cache);
        // Things that are not ours, inside the cache folder.
        write(&cache, "notes.txt", b"mine");
        write(&cache, "My Own Folder/keep.dds", b"mine");
        write(&cache, ".dropped-123.partial/half.dds", b"stale");

        let report = cache_report(&cache, &both);
        let names = |r: &[CacheEntry]| -> Vec<(String, bool, usize)> {
            r.iter()
                .map(|e| {
                    let n = e.dir.file_name().unwrap().to_string_lossy().into_owned();
                    (n.split('-').next().unwrap().to_string(), e.in_use, e.files)
                })
                .collect()
        };
        assert_eq!(
            names(&report),
            [
                (".dropped".to_string(), false, 1),
                ("dropped".to_string(), true, 2),
                ("kept".to_string(), true, 1)
            ]
        );
        assert_eq!(report[2].bytes, 9);

        // The list no longer names one archive: only its folder and the stale one go.
        let only_kept = ModList::parse("kept.7z\nloose\n", &base);
        let removed = prune_cache(&cache, &only_kept);
        assert_eq!(removed.len(), 2);
        assert!(removed.iter().all(|e| !e.in_use));
        assert_eq!(
            names(&cache_report(&cache, &only_kept)),
            [("kept".to_string(), true, 1)]
        );
        assert!(cache.join("notes.txt").is_file());
        assert!(cache.join("My Own Folder/keep.dds").is_file());
        assert!(base.join("loose/face.dds").is_file() && base.join("dropped.7z").is_file());

        // Pointed at a folder that is not the cache, it removes nothing at all.
        std::fs::create_dir_all(base.join("mods/abc-1f-2e")).unwrap();
        assert!(prune_cache(&base.join("mods"), &ModList::default()).is_empty());
        assert!(base.join("mods/abc-1f-2e").is_dir());
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn only_our_own_names_count_as_cache_folders() {
        for ours in [
            "Faces__v2_-13b-6af7542d",
            "x-0-0",
            ".Faces-4412.partial",
            "a-b-c-1f-2e",
        ] {
            assert!(is_cache_name(ours), "{ours}");
        }
        for not in [
            "My Own Folder",
            "notes.txt",
            "abc",
            "abc-xyz-12",
            "a b-1f-2e",
            ".partial",
            "x-1f-",
        ] {
            assert!(!is_cache_name(not), "{not}");
        }
    }
}
