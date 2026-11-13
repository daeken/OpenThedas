//! A stack of resource layers with provenance.
//!
//! Layers are pushed lowest priority first. A lookup returns the hit from the highest layer,
//! and [`Vfs::hits`] returns every layer that provides a name, so tools can show which files
//! a mod or patch shadows.
//!
//! Layers can be mounted for a while and removed again: take a [`Vfs::mark`], push the
//! layers (for example a level's own archives while its area is loaded), and later
//! [`Vfs::truncate`] back to the mark.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::crypto::DaoKey;
use crate::erf::Erf;
use crate::{Error, Result, name_hash, normalize_name};

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
}

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
    /// Name hash ([`name_hash`]) -> hits, lowest layer first. Keyed by hash rather than name
    /// because Dragon Age II archives store most entries only as hashes; a lookup hashes the
    /// name it is given.
    index: HashMap<u64, Vec<(usize, Slot)>>,
    /// Name hash -> normalized name, for every hit whose name is known.
    names: HashMap<u64, String>,
}

impl Vfs {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn layers(&self) -> &[Layer] {
        &self.layers
    }

    /// Number of distinct resource names visible through the stack.
    pub fn len(&self) -> usize {
        self.index.len()
    }

    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    pub fn push_archive(&mut self, path: impl AsRef<Path>, label: impl Into<String>) -> Result<()> {
        let erf = Erf::open(path)?;
        self.push_erf(Arc::new(erf), None, label);
        Ok(())
    }

    pub fn push_erf(&mut self, erf: Arc<Erf>, key: Option<DaoKey>, label: impl Into<String>) {
        let layer = self.layers.len();
        for (i, e) in erf.entries.iter().enumerate() {
            let key = match (&e.name, e.name_hash) {
                (Some(name), _) => self.key(name),
                (None, Some(hash)) => hash,
                (None, None) => continue,
            };
            self.index
                .entry(key)
                .or_default()
                .push((layer, Slot::Entry(i)));
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
            let key = self.key(&name);
            let hits = self.index.entry(key).or_default();
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
            let key = self.key(&name);
            let hits = self.index.entry(key).or_default();
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

    /// The index key for a name, remembering the name so [`Vfs::names`] can list it.
    fn key(&mut self, name: &str) -> u64 {
        let hash = name_hash(name);
        self.names
            .entry(hash)
            .or_insert_with(|| normalize_name(name));
        hash
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
        let index = &self.index;
        self.names.retain(|key, _| index.contains_key(key));
    }

    fn provenance(&self, layer: usize, slot: Slot) -> Provenance {
        let l = &self.layers[layer];
        match (&l.source, slot) {
            (LayerSource::Archive { erf, .. }, Slot::Entry(i)) => Provenance {
                layer,
                layer_label: l.label.clone(),
                file: erf.path().to_path_buf(),
                entry: Some(erf.entries[i].display_name()),
            },
            (LayerSource::Directory { .. } | LayerSource::Files, Slot::File(i)) => Provenance {
                layer,
                layer_label: l.label.clone(),
                file: self.dir_files[layer][i].1.clone(),
                entry: None,
            },
            _ => unreachable!("slot kind always matches layer kind"),
        }
    }

    /// The winning source for a name, if any layer provides it.
    pub fn lookup(&self, name: &str) -> Option<Provenance> {
        let hits = self.index.get(&name_hash(name))?;
        let &(layer, slot) = hits.last()?;
        Some(self.provenance(layer, slot))
    }

    /// Every source for a name, highest priority first.
    pub fn hits(&self, name: &str) -> Vec<Provenance> {
        self.index
            .get(&name_hash(name))
            .map(|h| {
                h.iter()
                    .rev()
                    .map(|&(l, s)| self.provenance(l, s))
                    .collect()
            })
            .unwrap_or_default()
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
        let hits = self
            .index
            .get(&name_hash(name))
            .ok_or_else(|| Error::NotFound(name.to_string()))?;
        let mut first_err = None;
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

    /// All visible names whose names are known, sorted. Entries a Dragon Age II archive
    /// stores only by hash can be looked up by name but are not listed here.
    pub fn names(&self) -> Vec<String> {
        let mut v: Vec<String> = self.names.values().cloned().collect();
        v.sort();
        v
    }
}

/// Whether a path has an archive extension: `.erf` or `.rim`, or Dragon Age II's `.crf`
/// (per-conversation archives) and `.rimp` (patch archives), which are ERF V3.0 as well.
pub fn is_archive_path(path: &Path) -> bool {
    is_archive(path)
}

pub(crate) fn is_archive(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref(),
        Some("erf" | "rim" | "crf" | "rimp")
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

    /// An ERF V3.0 archive (Dragon Age II layout): entries are `(stored name, name, data)`,
    /// and an entry without a stored name is identified only by its hashes.
    fn erf_v30(entries: &[(bool, &str, &[u8])]) -> Vec<u8> {
        let mut strings = Vec::new();
        let mut offsets = Vec::new();
        for (stored, name, _) in entries {
            if *stored {
                offsets.push(strings.len() as i32);
                strings.extend(name.as_bytes());
                strings.push(0);
            } else {
                offsets.push(-1);
            }
        }
        let mut out: Vec<u8> = "ERF V3.0"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        out.extend((strings.len() as u32).to_le_bytes());
        out.extend((entries.len() as u32).to_le_bytes());
        out.extend(0u32.to_le_bytes()); // flags: no encryption, no compression
        out.extend(0u32.to_le_bytes()); // module id
        out.extend([0u8; 16]); // digest
        out.extend(&strings);
        let mut offset = out.len() + 28 * entries.len();
        for ((_, name, data), name_offset) in entries.iter().zip(offsets) {
            let ext = crate::extension(name);
            out.extend(name_offset.to_le_bytes());
            out.extend(name_hash(name).to_le_bytes());
            out.extend(crate::type_hash(&ext).to_le_bytes());
            out.extend((offset as u32).to_le_bytes());
            out.extend((data.len() as u32).to_le_bytes());
            out.extend((data.len() as u32).to_le_bytes());
            offset += data.len();
        }
        for (_, _, data) in entries {
            out.extend_from_slice(data);
        }
        out
    }

    #[test]
    fn hash_only_entries_are_found_by_name() {
        let base = std::env::temp_dir().join(format!("thedas-vfs-v30-{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        let low = base.join("core.rim");
        let high = base.join("patch.rimp");
        std::fs::write(
            &low,
            erf_v30(&[
                (true, "Named_Table.gda", b"named"),
                (false, "hidden_model.mmh", b"low"),
            ]),
        )
        .unwrap();
        std::fs::write(&high, erf_v30(&[(false, "HIDDEN_MODEL.mmh", b"patched")])).unwrap();
        assert!(is_archive_path(&high));

        let mut vfs = Vfs::new();
        vfs.push_archive(&low, "core").unwrap();
        assert_eq!(vfs.read("named_table.GDA").unwrap().0, b"named");
        assert_eq!(vfs.read("Hidden_Model.mmh").unwrap().0, b"low");
        let mark = vfs.mark();
        vfs.push_archive(&high, "patch").unwrap();
        let (data, prov) = vfs.read("hidden_model.mmh").unwrap();
        assert_eq!(data, b"patched");
        assert_eq!(prov.layer_label, "patch");
        let entry = prov.entry.unwrap();
        assert!(entry.starts_with('#') && entry.ends_with(".mmh"), "{entry}");
        assert_eq!(vfs.hits("hidden_model.mmh").len(), 2);
        // Only stored names are listed.
        assert_eq!(vfs.names(), vec!["named_table.gda".to_string()]);

        vfs.truncate(mark);
        assert_eq!(vfs.read("hidden_model.mmh").unwrap().0, b"low");
        assert_eq!(vfs.len(), 2);
    }
}
