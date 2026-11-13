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
    /// `user:<path under the original's documents folder>` or `mod:<name>`.
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
            "not a folder, .zip, .dazip, .erf or .rim (unpack other archive types to a folder and list the folder)"
                .into(),
        ),
    }
    Ok(())
}

/// Where `.dazip` packages are unpacked: `mod-cache` in the player's data folder.
pub fn cache_dir() -> PathBuf {
    user_data_dir().join("mod-cache")
}

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
    let meta = std::fs::metadata(path).map_err(io(path))?;
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
    let addins_dir = join_ci(&root, "Contents/addins");
    if let Ok(rd) = std::fs::read_dir(&addins_dir) {
        let mut dirs: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
        dirs.retain(|p| p.is_dir());
        dirs.sort();
        for dir in dirs {
            let name = dir.file_name().unwrap().to_string_lossy().into_owned();
            let mut addin = crate::install::addin_from_manifest(&manifest, dir, name.clone());
            // A package with several add-ins has one manifest item each; ours reads the
            // first, so the others go by their folder's name with the first one's settings.
            if !addin.uid.eq_ignore_ascii_case(&name) && !addins.is_empty() {
                addin.uid = name;
            }
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
}
