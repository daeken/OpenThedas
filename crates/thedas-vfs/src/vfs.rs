//! A stack of resource layers with provenance.
//!
//! Layers are pushed lowest priority first. A lookup returns the hit from the highest layer,
//! and [`Vfs::hits`] returns every layer that provides a name, so tools can show which files
//! a mod or patch shadows.
//!
//! Layers can be mounted for a while and removed again: take a [`Vfs::mark`], push the
//! layers (for example a level's own archives while its area is loaded), and later
//! [`Vfs::truncate`] back to the mark.
//!
//! A stack can also carry an *overlay*: a second stack whose layers beat every layer of the
//! base one, whatever is mounted on the base later. Player mods live there (see
//! [`crate::mods`]), so a level's archives mounted while its area is loaded never hide a mod's
//! file, as the original's override folders outrank its `env` folders.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::crypto::DaoKey;
use crate::erf::Erf;
use crate::{Error, Result, normalize_name};

#[derive(Debug)]
pub enum LayerSource {
    Archive {
        erf: Arc<Erf>,
        key: Option<DaoKey>,
    },
    Directory {
        root: PathBuf,
        recursive: bool,
    },
    /// An explicit list of loose files, such as a level's `.arl` beside its archives.
    Files,
    /// The files of a zip archive (a mod as downloaded, or a `.dazip`), read in place.
    Zip(ZipSource),
}

/// An open zip archive serving a layer. Entries are inflated into memory on each read; nothing
/// is ever written to disk.
pub struct ZipSource {
    path: PathBuf,
    archive: Mutex<zip::ZipArchive<std::fs::File>>,
}

impl ZipSource {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl std::fmt::Debug for ZipSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ZipSource")
            .field("path", &self.path)
            .finish()
    }
}

/// The largest zip entry a layer will inflate, so a hostile archive cannot exhaust memory.
const MAX_ZIP_ENTRY: u64 = 1 << 30;

#[derive(Debug)]
pub struct Layer {
    /// A short human-readable description, such as the archive path relative to the install.
    pub label: String,
    pub source: LayerSource,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Slot {
    /// Index into the archive's entry table.
    Entry(usize),
    /// Index into the directory layer's file list.
    File(usize),
}

/// Where a resource came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Provenance {
    /// Position of the layer in the stack; higher wins.
    pub layer: usize,
    pub layer_label: String,
    /// The file on disk: the archive, or the loose file.
    pub file: PathBuf,
    /// The entry name inside the archive, for archive hits.
    pub entry: Option<String>,
}

#[derive(Debug, Default)]
pub struct Vfs {
    layers: Vec<Layer>,
    /// Loose files per directory layer: (name, path).
    dir_files: Vec<Vec<(String, PathBuf)>>,
    /// Normalized name -> hits, lowest layer first.
    index: HashMap<String, Vec<(usize, Slot)>>,
    /// Layers that beat every layer of this stack; see the module documentation.
    overlay: Option<Box<Vfs>>,
}

impl Vfs {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn layers(&self) -> &[Layer] {
        &self.layers
    }

    /// The overlay stack, if one is set.
    pub fn overlay(&self) -> Option<&Vfs> {
        self.overlay.as_deref()
    }

    /// Sets the overlay: a stack whose layers beat every layer here, now and after later
    /// pushes. An empty overlay is dropped. An overlay's own overlay is ignored.
    pub fn set_overlay(&mut self, mut overlay: Vfs) {
        overlay.overlay = None;
        self.overlay = (!overlay.layers.is_empty()).then(|| Box::new(overlay));
    }

    /// Removes and returns the overlay.
    pub fn take_overlay(&mut self) -> Option<Vfs> {
        self.overlay.take().map(|b| *b)
    }

    /// Number of distinct resource names visible through the stack.
    pub fn len(&self) -> usize {
        let extra = self.overlay.as_ref().map_or(0, |o| {
            o.index
                .keys()
                .filter(|k| !self.index.contains_key(*k))
                .count()
        });
        self.index.len() + extra
    }

    pub fn is_empty(&self) -> bool {
        self.index.is_empty() && self.overlay.as_ref().is_none_or(|o| o.index.is_empty())
    }

    pub fn push_archive(&mut self, path: impl AsRef<Path>, label: impl Into<String>) -> Result<()> {
        let erf = Erf::open(path)?;
        self.push_erf(Arc::new(erf), None, label);
        Ok(())
    }

    pub fn push_erf(&mut self, erf: Arc<Erf>, key: Option<DaoKey>, label: impl Into<String>) {
        let layer = self.layers.len();
        for (i, e) in erf.entries.iter().enumerate() {
            if let Some(name) = &e.name {
                self.index
                    .entry(normalize_name(name))
                    .or_default()
                    .push((layer, Slot::Entry(i)));
            }
        }
        self.layers.push(Layer {
            label: label.into(),
            source: LayerSource::Archive { erf, key },
        });
        self.dir_files.push(Vec::new());
    }

    /// Adds the loose files in a directory. Archives found there are not opened; push them
    /// separately as archive layers. A missing directory adds an empty layer.
    pub fn push_directory(
        &mut self,
        root: impl AsRef<Path>,
        recursive: bool,
        label: impl Into<String>,
    ) -> Result<()> {
        let root = root.as_ref().to_path_buf();
        let mut files = Vec::new();
        if root.is_dir() {
            collect_files(&root, recursive, &mut files)?;
        }
        files.retain(|p| !is_archive(p));
        files.sort();
        let layer = self.layers.len();
        let mut named = Vec::with_capacity(files.len());
        for path in files {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let i = named.len();
            let hits = self.index.entry(normalize_name(&name)).or_default();
            // Within one recursive directory, a later path shadows an earlier one.
            if let Some(last) = hits.last() {
                if last.0 == layer {
                    hits.pop();
                }
            }
            hits.push((layer, Slot::File(i)));
            named.push((name, path));
        }
        self.layers.push(Layer {
            label: label.into(),
            source: LayerSource::Directory { root, recursive },
        });
        self.dir_files.push(named);
        Ok(())
    }

    /// Adds a layer holding exactly these loose files. Missing files are skipped.
    pub fn push_files(&mut self, files: &[PathBuf], label: impl Into<String>) {
        let layer = self.layers.len();
        let mut named = Vec::with_capacity(files.len());
        for path in files.iter().filter(|p| p.is_file()) {
            let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
                continue;
            };
            let i = named.len();
            let hits = self.index.entry(normalize_name(&name)).or_default();
            if let Some(last) = hits.last() {
                if last.0 == layer {
                    hits.pop();
                }
            }
            hits.push((layer, Slot::File(i)));
            named.push((name, path.clone()));
        }
        self.layers.push(Layer {
            label: label.into(),
            source: LayerSource::Files,
        });
        self.dir_files.push(named);
    }

    /// Adds the files of a zip archive as one layer, without unpacking it. With `inner`, only
    /// the files under that folder of the archive (matched ignoring case, `/` or `\\`
    /// separators) are added. Folders in the archive are flattened as a recursive directory
    /// layer is: a resource is named by its file name alone, and of two files with one name the
    /// later path (in byte order) wins. ERF and RIM archives inside the zip are not opened;
    /// their names are returned so the caller can tell the player.
    pub fn push_zip(
        &mut self,
        path: impl AsRef<Path>,
        inner: Option<&str>,
        label: impl Into<String>,
    ) -> Result<Vec<String>> {
        let path = path.as_ref().to_path_buf();
        let io = |e: std::io::Error| Error::Io {
            path: path.clone(),
            source: e,
        };
        let bad = |reason: String| Error::Malformed {
            path: path.clone(),
            reason,
        };
        let file = std::fs::File::open(&path).map_err(io)?;
        let archive = zip::ZipArchive::new(file).map_err(|e| bad(format!("not a zip: {e}")))?;
        let prefix: Vec<String> = inner
            .unwrap_or("")
            .split(['/', '\\'])
            .filter(|c| !c.is_empty())
            .map(|c| c.to_ascii_lowercase())
            .collect();
        let mut inner_names: Vec<String> = archive
            .file_names()
            .filter(|n| !n.ends_with(['/', '\\']))
            .filter(|n| {
                let comps: Vec<&str> = n.split(['/', '\\']).filter(|c| !c.is_empty()).collect();
                comps.len() > prefix.len()
                    && comps
                        .iter()
                        .zip(&prefix)
                        .all(|(c, p)| c.eq_ignore_ascii_case(p))
            })
            .map(str::to_string)
            .collect();
        inner_names.sort();
        let mut skipped = Vec::new();
        let layer = self.layers.len();
        let mut named = Vec::with_capacity(inner_names.len());
        for inner_name in inner_names {
            let Some(name) = inner_name
                .rsplit(['/', '\\'])
                .next()
                .filter(|n| !n.is_empty())
                .map(str::to_string)
            else {
                continue;
            };
            if is_archive(Path::new(&name)) {
                skipped.push(inner_name);
                continue;
            }
            let i = named.len();
            let hits = self.index.entry(normalize_name(&name)).or_default();
            if let Some(last) = hits.last() {
                if last.0 == layer {
                    hits.pop();
                }
            }
            hits.push((layer, Slot::File(i)));
            named.push((name, PathBuf::from(inner_name)));
        }
        self.layers.push(Layer {
            label: label.into(),
            source: LayerSource::Zip(ZipSource {
                path,
                archive: Mutex::new(archive),
            }),
        });
        self.dir_files.push(named);
        Ok(skipped)
    }

    /// The current top of the stack, for a later [`Vfs::truncate`].
    pub fn mark(&self) -> usize {
        self.layers.len()
    }

    /// Removes every layer at or above `mark`, restoring the stack to what it was when the
    /// mark was taken.
    pub fn truncate(&mut self, mark: usize) {
        if mark >= self.layers.len() {
            return;
        }
        self.layers.truncate(mark);
        self.dir_files.truncate(mark);
        self.index.retain(|_, hits| {
            hits.retain(|&(layer, _)| layer < mark);
            !hits.is_empty()
        });
    }

    fn provenance(&self, layer: usize, slot: Slot) -> Provenance {
        let l = &self.layers[layer];
        match (&l.source, slot) {
            (LayerSource::Archive { erf, .. }, Slot::Entry(i)) => Provenance {
                layer,
                layer_label: l.label.clone(),
                file: erf.path().to_path_buf(),
                entry: erf.entries[i].name.clone(),
            },
            (LayerSource::Directory { .. } | LayerSource::Files, Slot::File(i)) => Provenance {
                layer,
                layer_label: l.label.clone(),
                file: self.dir_files[layer][i].1.clone(),
                entry: None,
            },
            (LayerSource::Zip(zip), Slot::File(i)) => Provenance {
                layer,
                layer_label: l.label.clone(),
                file: zip.path.clone(),
                entry: Some(self.dir_files[layer][i].1.to_string_lossy().into_owned()),
            },
            _ => unreachable!("slot kind always matches layer kind"),
        }
    }

    /// An overlay hit's provenance, with its layer counted above this stack's layers.
    fn lifted(&self, mut p: Provenance) -> Provenance {
        p.layer += self.layers.len();
        p
    }

    /// The winning source for a name, if any layer provides it.
    pub fn lookup(&self, name: &str) -> Option<Provenance> {
        if let Some(p) = self.overlay.as_ref().and_then(|o| o.lookup(name)) {
            return Some(self.lifted(p));
        }
        let hits = self.index.get(&normalize_name(name))?;
        let &(layer, slot) = hits.last()?;
        Some(self.provenance(layer, slot))
    }

    /// Every source for a name, highest priority first.
    pub fn hits(&self, name: &str) -> Vec<Provenance> {
        let mut out: Vec<Provenance> = self
            .overlay
            .as_ref()
            .map(|o| o.hits(name).into_iter().map(|p| self.lifted(p)).collect())
            .unwrap_or_default();
        if let Some(h) = self.index.get(&normalize_name(name)) {
            out.extend(h.iter().rev().map(|&(l, s)| self.provenance(l, s)));
        }
        out
    }

    fn read_slot(&self, layer: usize, slot: Slot) -> Result<Vec<u8>> {
        match (&self.layers[layer].source, slot) {
            (LayerSource::Archive { erf, key }, Slot::Entry(i)) => {
                erf.read(&erf.entries[i], key.as_ref())
            }
            (LayerSource::Directory { .. } | LayerSource::Files, Slot::File(i)) => {
                let path = &self.dir_files[layer][i].1;
                std::fs::read(path).map_err(|e| Error::Io {
                    path: path.clone(),
                    source: e,
                })
            }
            (LayerSource::Zip(zip), Slot::File(i)) => {
                let inner = self.dir_files[layer][i].1.to_string_lossy();
                read_zip_entry(zip, &inner)
            }
            _ => unreachable!("slot kind always matches layer kind"),
        }
    }

    /// Reads the winning version of a resource.
    ///
    /// When the winner is an encrypted archive entry we have no key for (DLC archives that
    /// shadow core files), the next layer down is tried instead, and the returned provenance
    /// names the layer that actually supplied the data. With every DLC key available this
    /// is the original's lookup; without keys it is the closest readable approximation.
    pub fn read(&self, name: &str) -> Result<(Vec<u8>, Provenance)> {
        let mut first_err = None;
        if let Some(o) = &self.overlay {
            match o.read(name) {
                Ok((data, p)) => return Ok((data, self.lifted(p))),
                Err(Error::NotFound(_)) => {}
                Err(e @ Error::Encrypted { .. }) => first_err = Some(e),
                Err(e) => return Err(e),
            }
        }
        let Some(hits) = self.index.get(&normalize_name(name)) else {
            return Err(first_err.unwrap_or_else(|| Error::NotFound(name.to_string())));
        };
        for &(layer, slot) in hits.iter().rev() {
            match self.read_slot(layer, slot) {
                Ok(data) => return Ok((data, self.provenance(layer, slot))),
                Err(e @ Error::Encrypted { .. }) => {
                    first_err.get_or_insert(e);
                }
                Err(e) => return Err(e),
            }
        }
        Err(first_err.unwrap_or_else(|| Error::NotFound(name.to_string())))
    }

    /// All visible names, sorted.
    pub fn names(&self) -> Vec<String> {
        let mut v: Vec<String> = self.index.keys().cloned().collect();
        if let Some(o) = &self.overlay {
            v.extend(
                o.index
                    .keys()
                    .filter(|k| !self.index.contains_key(*k))
                    .cloned(),
            );
        }
        v.sort();
        v
    }
}

fn read_zip_entry(zip: &ZipSource, inner: &str) -> Result<Vec<u8>> {
    use std::io::Read;
    let fail = |reason: String| Error::Decompress {
        path: zip.path.clone(),
        name: inner.to_string(),
        reason,
    };
    let mut archive = zip
        .archive
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let entry = archive.by_name(inner).map_err(|e| fail(e.to_string()))?;
    if entry.size() > MAX_ZIP_ENTRY {
        return Err(fail(format!("entry is {} bytes", entry.size())));
    }
    let mut out = Vec::with_capacity(entry.size() as usize);
    entry
        .take(MAX_ZIP_ENTRY + 1)
        .read_to_end(&mut out)
        .map_err(|e| fail(e.to_string()))?;
    if out.len() as u64 > MAX_ZIP_ENTRY {
        return Err(fail("entry inflates past the size limit".into()));
    }
    Ok(out)
}

/// Whether a path has an archive extension (`.erf` or `.rim`).
pub fn is_archive_path(path: &Path) -> bool {
    is_archive(path)
}

pub(crate) fn is_archive(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref(),
        Some("erf" | "rim")
    )
}

pub(crate) fn collect_files(dir: &Path, recursive: bool, out: &mut Vec<PathBuf>) -> Result<()> {
    let rd = std::fs::read_dir(dir).map_err(|e| Error::Io {
        path: dir.to_path_buf(),
        source: e,
    })?;
    for entry in rd {
        let entry = entry.map_err(|e| Error::Io {
            path: dir.to_path_buf(),
            source: e,
        })?;
        let path = entry.path();
        let ft = entry.file_type().map_err(|e| Error::Io {
            path: path.clone(),
            source: e,
        })?;
        if ft.is_dir() {
            if recursive {
                collect_files(&path, true, out)?;
            }
        } else if ft.is_file() {
            out.push(path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn later_directory_layers_shadow_earlier_ones() {
        let base = std::env::temp_dir().join(format!("thedas-vfs-layers-{}", std::process::id()));
        let low = base.join("low");
        let high = base.join("high");
        std::fs::create_dir_all(&low).unwrap();
        std::fs::create_dir_all(&high).unwrap();
        std::fs::write(low.join("A.txt"), b"low").unwrap();
        std::fs::write(low.join("only_low.txt"), b"x").unwrap();
        std::fs::write(high.join("a.TXT"), b"high").unwrap();

        let mut vfs = Vfs::new();
        vfs.push_directory(&low, false, "low").unwrap();
        vfs.push_directory(&high, false, "high").unwrap();

        let (data, prov) = vfs.read("a.txt").unwrap();
        assert_eq!(data, b"high");
        assert_eq!(prov.layer_label, "high");
        let hits = vfs.hits("A.TXT");
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[1].layer_label, "low");
        assert_eq!(vfs.lookup("only_low.txt").unwrap().layer, 0);
        assert!(vfs.lookup("missing").is_none());
        assert_eq!(vfs.len(), 2);
    }

    #[test]
    fn mounted_layers_truncate_away() {
        let base = std::env::temp_dir().join(format!("thedas-vfs-mount-{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(base.join("shared.txt"), b"base").unwrap();
        let extra = base.join("extra");
        std::fs::create_dir_all(&extra).unwrap();
        std::fs::write(extra.join("shared.txt"), b"mounted").unwrap();
        std::fs::write(extra.join("level.arl"), b"arl").unwrap();

        let mut vfs = Vfs::new();
        vfs.push_files(&[base.join("shared.txt"), base.join("missing.txt")], "base");
        let mark = vfs.mark();
        vfs.push_files(
            &[extra.join("shared.txt"), extra.join("level.arl")],
            "level",
        );
        assert_eq!(vfs.read("shared.txt").unwrap().0, b"mounted");
        assert_eq!(vfs.lookup("LEVEL.ARL").unwrap().layer_label, "level");
        assert_eq!(vfs.hits("shared.txt").len(), 2);

        vfs.truncate(mark);
        assert_eq!(vfs.layers().len(), 1);
        assert_eq!(vfs.read("shared.txt").unwrap().0, b"base");
        assert!(vfs.lookup("level.arl").is_none());
        assert!(vfs.lookup("missing.txt").is_none());
        assert_eq!(vfs.len(), 1);
        vfs.truncate(10);
        assert_eq!(vfs.len(), 1);
    }

    /// An ERF V2.2 archive flagged as DA:O Blowfish-encrypted, holding one entry.
    fn encrypted_erf(name: &str, data: &[u8]) -> Vec<u8> {
        let utf16 = |s: &str, n: usize| {
            let mut v: Vec<u8> = s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
            v.resize(n, 0);
            v
        };
        let mut out = utf16("ERF V2.2", 16);
        out.extend(1u32.to_le_bytes()); // count
        out.extend(109u32.to_le_bytes()); // year
        out.extend(1u32.to_le_bytes()); // day
        out.extend(0xFFFF_FFFFu32.to_le_bytes());
        out.extend((2u32 << 4).to_le_bytes()); // Blowfish (DA:O)
        out.extend(0u32.to_le_bytes()); // module id
        out.extend([0u8; 16]); // password digest
        let offset = out.len() + 76;
        out.extend(utf16(name, 64));
        out.extend((offset as u32).to_le_bytes());
        out.extend((data.len() as u32).to_le_bytes());
        out.extend((data.len() as u32).to_le_bytes());
        out.extend_from_slice(data);
        out
    }

    #[test]
    fn falls_back_past_encrypted_entries() {
        let base = std::env::temp_dir().join(format!("thedas-vfs-enc-{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(base.join("tex.dds"), b"core").unwrap();
        let dlc = base.join("dlc.erf");
        std::fs::write(&dlc, encrypted_erf("tex.dds", &[0u8; 8])).unwrap();
        std::fs::write(base.join("only.erf"), encrypted_erf("only.dds", &[0u8; 8])).unwrap();

        let mut vfs = Vfs::new();
        vfs.push_files(&[base.join("tex.dds")], "core");
        vfs.push_archive(&dlc, "dlc").unwrap();
        vfs.push_archive(base.join("only.erf"), "only").unwrap();
        assert_eq!(vfs.lookup("tex.dds").unwrap().layer_label, "dlc");
        let (data, prov) = vfs.read("tex.dds").unwrap();
        assert_eq!(data, b"core");
        assert_eq!(prov.layer_label, "core");
        assert!(matches!(vfs.read("only.dds"), Err(Error::Encrypted { .. })));
    }
}
