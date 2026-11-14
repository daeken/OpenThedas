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
//!
//! Above both sits the *workspace*: one folder of loose files that mod mode mounts while the
//! player asks for live edits ([`Vfs::set_workspace`]). It beats every layer, the last line
//! of the mod list included. It is one for the process, seen by every stack the game plays
//! from (those [`crate::DaoInstall::play_vfs`] builds for Dragon Age: Origins with mods on,
//! marked by [`Vfs::play_from`]; other stacks never see it), and unlike the layers it can be
//! mounted, unmounted and scanned again ([`Vfs::rescan_workspace`]) while those stacks are
//! shared. A changed file is read afresh on its next read; what was already loaded from
//! the old file stays as it was until its user reads it again.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};

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

/// The workspace's layer label (see [`Vfs::set_workspace`]).
pub const WORKSPACE_LABEL: &str = "workspace:override";

/// The workspace: a folder of loose files, every file at any depth by name (a later path
/// wins, as in a recursive directory layer).
#[derive(Debug)]
struct Workspace {
    root: PathBuf,
    files: HashMap<u64, (String, PathBuf)>,
}

impl Workspace {
    fn scan(root: &Path) -> Workspace {
        let mut paths = Vec::new();
        if root.is_dir()
            && let Err(e) = collect_files(root, true, &mut paths)
        {
            log::warn!("{WORKSPACE_LABEL}: {e}");
        }
        paths.retain(|p| !is_archive(p));
        paths.sort();
        let mut files = HashMap::new();
        for path in paths {
            let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
                continue;
            };
            files.insert(name_hash(&name), (normalize_name(&name), path));
        }
        Workspace {
            root: root.to_path_buf(),
            files,
        }
    }
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
    /// Layers that beat every layer of this stack; see the module documentation.
    overlay: Option<Box<Vfs>>,
    /// The game plays from this stack: the workspace, when mounted, beats its layers.
    plays: bool,
    /// The UIDs of the add-ins mounted in this stack (see [`Vfs::addin_enabled`]).
    addins: Vec<String>,
}

/// Whether a workspace is mounted: the fast path every read checks first. Set after the
/// slot is filled and cleared before it is emptied.
static WORKSPACE_UP: AtomicBool = AtomicBool::new(false);
static WORKSPACE: RwLock<Option<Workspace>> = RwLock::new(None);

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
            let hits = {
                let key = self.key(&name);
                self.index.entry(key).or_default()
            };
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

    /// The index key for a name, remembering the name so [`Vfs::names`] can list it.
    fn key(&mut self, name: &str) -> u64 {
        let hash = name_hash(name);
        self.names
            .entry(hash)
            .or_insert_with(|| normalize_name(name));
        hash
    }

    /// For a name whose winning source is an archive: that archive's layer (counted as
    /// [`Provenance::layer`] counts it) and how many entries its table of contents holds.
    /// `None` when the name is unknown or its winner is a loose file or a zip.
    pub fn winning_archive(&self, name: &str) -> Option<(usize, usize)> {
        if let Some(o) = &self.overlay {
            if o.index.contains_key(&name_hash(name)) {
                return o
                    .winning_archive(name)
                    .map(|(layer, n)| (layer + self.layers.len(), n));
            }
        }
        let &(layer, _) = self.index.get(&name_hash(name))?.last()?;
        match &self.layers[layer].source {
            LayerSource::Archive { erf, .. } => Some((layer, erf.entries.len())),
            _ => None,
        }
    }

    /// Rewrites the labels of the layers at or above `from`.
    pub fn relabel(&mut self, from: usize, f: impl Fn(&str) -> String) {
        for l in self.layers.iter_mut().skip(from) {
            l.label = f(&l.label);
        }
    }

    /// How many files a layer of this stack holds (archive entries, or loose files), counting
    /// ones a higher layer hides. 0 for a layer that does not exist.
    pub fn layer_files(&self, layer: usize) -> usize {
        match self.layers.get(layer).map(|l| &l.source) {
            Some(LayerSource::Archive { erf, .. }) => erf.entries.len(),
            Some(_) => self.dir_files[layer].len(),
            None => 0,
        }
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
        if let Some(p) = self.workspace_hit(name) {
            return Some(p);
        }
        if let Some(p) = self.overlay.as_ref().and_then(|o| o.lookup(name)) {
            return Some(self.lifted(p));
        }
        let hits = self.index.get(&name_hash(name))?;
        let &(layer, slot) = hits.last()?;
        Some(self.provenance(layer, slot))
    }

    /// Every source for a name, highest priority first.
    pub fn hits(&self, name: &str) -> Vec<Provenance> {
        let mut out: Vec<Provenance> = self.workspace_hit(name).into_iter().collect();
        out.extend(self.layer_hits(name));
        out
    }

    /// [`Vfs::hits`] without the workspace.
    fn layer_hits(&self, name: &str) -> Vec<Provenance> {
        let mut out = Vec::new();
        if let Some(o) = &self.overlay {
            out.extend(o.layer_hits(name).into_iter().map(|p| self.lifted(p)));
        }
        if let Some(h) = self.index.get(&name_hash(name)) {
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

    /// Marks this stack as one the game plays from, so a mounted workspace beats its layers.
    pub fn play_from(&mut self) {
        self.plays = true;
    }

    /// Records the UIDs of the add-ins this stack mounts.
    pub fn set_addins(&mut self, uids: Vec<String>) {
        self.addins = uids;
    }

    /// Whether the add-in `uid` is mounted in this stack (ASCII case ignored): what the
    /// script function `GetPRCEnabled` answers.
    pub fn addin_enabled(&self, uid: &str) -> bool {
        self.addins
            .iter()
            .any(|a| a.eq_ignore_ascii_case(uid.trim()))
    }

    /// Whether the game plays from this stack (see [`Vfs::play_from`]).
    pub fn plays(&self) -> bool {
        self.plays
    }

    /// Mounts folder `dir` as the workspace, above every layer of every stack (`None`
    /// unmounts it). Returns how many files it holds.
    pub fn set_workspace(dir: Option<&Path>) -> usize {
        let mut slot = WORKSPACE.write().unwrap_or_else(|p| p.into_inner());
        match dir {
            Some(d) => {
                let w = Workspace::scan(d);
                let n = w.files.len();
                *slot = Some(w);
                WORKSPACE_UP.store(true, Ordering::Release);
                n
            }
            None => {
                WORKSPACE_UP.store(false, Ordering::Release);
                *slot = None;
                0
            }
        }
    }

    /// Scans the mounted workspace's folder again (for files added or removed); returns how
    /// many files it holds (0 when none is mounted).
    pub fn rescan_workspace() -> usize {
        if !WORKSPACE_UP.load(Ordering::Acquire) {
            return 0;
        }
        let mut slot = WORKSPACE.write().unwrap_or_else(|p| p.into_inner());
        match slot.as_mut() {
            Some(w) => {
                *w = Workspace::scan(&w.root);
                w.files.len()
            }
            None => 0,
        }
    }

    /// The mounted workspace's folder and file count.
    pub fn workspace() -> Option<(PathBuf, usize)> {
        if !WORKSPACE_UP.load(Ordering::Acquire) {
            return None;
        }
        let slot = WORKSPACE.read().unwrap_or_else(|p| p.into_inner());
        slot.as_ref().map(|w| (w.root.clone(), w.files.len()))
    }

    /// The workspace's file for a name, with its provenance (counted above every layer).
    fn workspace_hit(&self, name: &str) -> Option<Provenance> {
        if !self.plays || !WORKSPACE_UP.load(Ordering::Acquire) {
            return None;
        }
        let slot = WORKSPACE.read().unwrap_or_else(|p| p.into_inner());
        let (n, path) = slot.as_ref()?.files.get(&name_hash(name))?;
        let top = self.layers.len() + self.overlay.as_ref().map_or(0, |o| o.layers.len());
        Some(Provenance {
            layer: top,
            layer_label: WORKSPACE_LABEL.to_string(),
            file: path.clone(),
            entry: Some(n.clone()),
        })
    }

    /// Reads the winning version of a resource.
    ///
    /// When the winner is an encrypted archive entry we have no key for (DLC archives that
    /// shadow core files), the next layer down is tried instead, and the returned provenance
    /// names the layer that actually supplied the data. With every DLC key available this
    /// is the original's lookup; without keys it is the closest readable approximation.
    pub fn read(&self, name: &str) -> Result<(Vec<u8>, Provenance)> {
        if let Some(p) = self.workspace_hit(name) {
            // A file removed since the last scan falls through to the layers.
            if let Ok(data) = std::fs::read(&p.file) {
                return Ok((data, p));
            }
        }
        self.read_beneath_workspace(name)
    }

    /// [`Vfs::read`] as if no workspace were mounted: what the layers hold (mod mode reads
    /// the originals of what it rewrites this way).
    pub fn read_beneath_workspace(&self, name: &str) -> Result<(Vec<u8>, Provenance)> {
        let mut first_err = None;
        if let Some(o) = &self.overlay {
            match o.read(name) {
                Ok((data, p)) => return Ok((data, self.lifted(p))),
                Err(Error::NotFound(_)) => {}
                Err(e @ Error::Encrypted { .. }) => first_err = Some(e),
                Err(e) => return Err(e),
            }
        }
        let Some(hits) = self.index.get(&name_hash(name)) else {
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

    /// The talk tables of one language (`en-us`), in the order to merge them: the tables
    /// named `*<language>.tlk`, sorted by name, then the patch tables `*<language>_p.tlk`,
    /// sorted by name. Dragon Age II ships its corrections to shipped strings as `_p` tables
    /// in each package's `patch` folder; Dragon Age: Origins has none. Provisional: that a
    /// patch table overrides every base table, whichever package it is in, is not traced.
    pub fn talk_tables(&self, language: &str) -> Vec<String> {
        let language = language.to_ascii_lowercase();
        let base = format!("{language}.tlk");
        let patch = format!("{language}_p.tlk");
        let names = self.names();
        let mut out: Vec<String> = names
            .iter()
            .filter(|n| n.ends_with(&base))
            .cloned()
            .collect();
        out.extend(names.iter().filter(|n| n.ends_with(&patch)).cloned());
        out
    }

    /// All visible names whose names are known, sorted. Entries a Dragon Age II archive
    /// stores only by hash can be looked up by name but are not listed here.
    pub fn names(&self) -> Vec<String> {
        let mut v: Vec<String> = self.names.values().cloned().collect();
        if let Some(o) = &self.overlay {
            v.extend(
                o.names
                    .iter()
                    .filter(|(k, _)| !self.names.contains_key(*k))
                    .map(|(_, n)| n.clone()),
            );
        }
        if self.plays && WORKSPACE_UP.load(Ordering::Acquire) {
            let slot = WORKSPACE.read().unwrap_or_else(|p| p.into_inner());
            if let Some(w) = slot.as_ref() {
                v.extend(w.files.values().map(|(n, _)| n.clone()));
            }
        }
        v.sort();
        v.dedup();
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
    fn patch_talk_tables_come_after_the_base_tables() {
        let base = std::env::temp_dir().join(format!("thedas-vfs-tlk-{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        for name in [
            "core_en-us.tlk",
            "core_EN-US_p.tlk",
            "campaign_base_en-us.tlk",
            "aaa_en-us_p.tlk",
            "core_fr-fr.tlk",
            "notes.txt",
        ] {
            std::fs::write(base.join(name), b"x").unwrap();
        }
        let mut vfs = Vfs::new();
        vfs.push_directory(&base, false, "tables").unwrap();
        assert_eq!(
            vfs.talk_tables("en-us"),
            [
                "campaign_base_en-us.tlk",
                "core_en-us.tlk",
                "aaa_en-us_p.tlk",
                "core_en-us_p.tlk"
            ]
        );
        assert_eq!(vfs.talk_tables("fr-fr"), ["core_fr-fr.tlk"]);
        std::fs::remove_dir_all(&base).ok();
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
