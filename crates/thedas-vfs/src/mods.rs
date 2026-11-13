//! Player mods as ordered layers over the game's own resources.
//!
//! A mod is never unpacked over the install. Each one is mounted where it sits (a folder, a zip
//! archive as downloaded, a `.dazip`, or a single `.erf`) as layers of the stack's overlay
//! ([`crate::Vfs::set_overlay`]), so it beats every game resource, including a level's own
//! archives, and can be removed by deleting one line.
//!
//! The order, lowest first:
//!
//! 1. The original's user override folder, `Documents/BioWare/Dragon Age/packages/core/override`,
//!    when it exists. The original registers it at priority 0, above everything else it loads
//!    (traced in the executable: the folder registrations around 0x40e33f and 0x40e385 give the
//!    install's `packages/core/override` priority 1 and the user's priority 0, lowest number
//!    winning).
//! 2. The entries of the load-order list, top to bottom: a later line beats an earlier one.
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
//! A `.dazip` is mounted the same flat way, which is right for its `packages/core/override`
//! part and only an approximation for a full add-in (its manifest, priority and module scope
//! are ignored).

use std::path::{Path, PathBuf};

use crate::vfs::{Vfs, collect_files, is_archive};
use crate::{Result, join_ci};

/// Names the list file in the player's data folder.
pub const LIST_FILE: &str = "mods.txt";

/// Environment variable overriding the list: a path to a list file, or `none` (or `off`, `0`)
/// to load no mods at all, not even the user override folder.
pub const ENV_VAR: &str = "THEDAS_MODS";

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

/// The original's user override folder, if this player has one. Looked for under the Documents
/// folder and, off Windows, under `HOME/Documents` (so a scratch `HOME` hides the real one).
pub fn original_user_override() -> Option<PathBuf> {
    let docs = if cfg!(windows) {
        dirs::document_dir()
    } else {
        home_dir().map(|h| h.join("Documents"))
    }?;
    let dir = join_ci(&docs, "BioWare/Dragon Age/packages/core/override");
    dir.is_dir().then_some(dir)
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
    /// reported and skipped; the rest still load.
    pub fn mount(&self, overlay: &mut Vfs) -> Vec<Mounted> {
        self.entries
            .iter()
            .map(|entry| {
                let before = overlay.layers().len();
                let mut problems = Vec::new();
                if let Err(e) = mount_entry(overlay, entry, &mut problems) {
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
    let mut l = format!("mod:{}", name(&entry.path));
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

fn mount_entry(overlay: &mut Vfs, entry: &ModEntry, problems: &mut Vec<String>) -> Result<()> {
    let path = &entry.path;
    if path.is_dir() {
        // Archives first (lowest), then the folder's loose files.
        let mut files = Vec::new();
        collect_files(path, true, &mut files)?;
        files.retain(|p| is_archive(p));
        files.sort();
        for archive in files {
            let l = label(entry, &archive);
            if let Err(e) = overlay.push_archive(&archive, l) {
                problems.push(e.to_string());
            }
        }
        return overlay.push_directory(path, true, label(entry, path));
    }
    if !path.is_file() {
        problems.push(format!("{}: no such file or folder", path.display()));
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
        "zip" | "dazip" | "override" => {
            let skipped = overlay.push_zip(path, entry.inner.as_deref(), label(entry, path))?;
            if !skipped.is_empty() {
                problems.push(format!(
                    "{} archive(s) inside the zip were not loaded (unpack the mod to a folder to use them), first: {}",
                    skipped.len(),
                    skipped[0]
                ));
            }
        }
        _ => problems.push(format!(
            "{}: not a folder, .zip, .dazip, .erf or .rim (unpack other archive types to a folder and list the folder)",
            path.display()
        )),
    }
    Ok(())
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

/// Builds the overlay for this player (the original's user override folder, then the list)
/// and sets it on `vfs`. With no override folder and no list this does nothing at all.
pub fn mount_default(vfs: &mut Vfs) -> Vec<Mounted> {
    let Some(list_file) = default_list_file() else {
        return Vec::new();
    };
    let mut overlay = Vfs::new();
    if let Some(dir) = original_user_override() {
        let entry = ModEntry {
            path: dir,
            inner: None,
        };
        let mut problems = Vec::new();
        if let Err(e) = mount_entry(&mut overlay, &entry, &mut problems) {
            problems.push(e.to_string());
        }
        for p in problems {
            log::warn!("mods: user override folder: {p}");
        }
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
        assert_eq!((e[0].path.clone(), e[0].inner.clone()), (base.join("first"), None));
        assert_eq!(e[1].path, PathBuf::from("/abs/second.zip"));
        assert_eq!(e[1].inner.as_deref(), Some("Pearly White"));
        assert_eq!((e[2].path.clone(), e[2].inner.clone()), (base.join("third.zip"), None));
        // A real folder whose name holds `!/` is taken whole.
        assert_eq!((e[3].path.clone(), e[3].inner.clone()), (base.join("odd!/name"), None));
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
        vfs.push_directory(base.join("game"), false, "game").unwrap();
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
        let labels: Vec<String> = vfs.hits("face.dds").into_iter().map(|p| p.layer_label).collect();
        assert_eq!(labels, ["mod:b.zip!/b mod/pearly white", "mod:a", "game"]);
        assert_eq!(vfs.len(), 4);
        assert_eq!(vfs.names().len(), 4);

        // A level mounted later still loses to the mods, and unmounts cleanly.
        let mark = vfs.mark();
        vfs.push_directory(base.join("level"), false, "level").unwrap();
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
        zip_of(&base.join("z.zip"), &[("inner.erf", b"x"), ("ok.dds", b"ok")]);
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
}
