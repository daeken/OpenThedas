//! Finding and loading the game's FMOD projects (`.fev`) and banks (`.fsb`), and decoding
//! bank samples with a cache.
//!
//! DA:O keeps them loose under each package's `audio/sound` and `audio/vo/<language>/vo`
//! folders (`packages/core`, `modules/single player`, DLC `module`/`core` folders). A later
//! folder in [`Library::dirs`] wins over an earlier one, as the resource stack does.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use thedas_formats::fev::Fev;
use thedas_formats::fsb::Fsb;

use crate::decode::{Pcm, decode};

struct Bank {
    path: PathBuf,
    fsb: Fsb,
}

/// Decoded samples kept around, up to a byte budget.
const CACHE_BYTES: usize = 768 << 20;

/// Decoded samples by (lowercase bank, index), and their total bytes.
type Cache = (HashMap<(String, usize), Arc<Pcm>>, usize);

pub struct Library {
    /// Folders searched for `.fev`/`.fsb`, lowest priority first.
    pub dirs: Vec<PathBuf>,
    files: HashMap<String, PathBuf>,
    projects: Mutex<HashMap<String, Option<Arc<Fev>>>>,
    banks: Mutex<HashMap<String, Option<Arc<Bank>>>>,
    cache: Mutex<Cache>,
}

fn key(name: &str) -> String {
    name.to_ascii_lowercase()
}

impl Library {
    /// A library over these folders (lowest priority first).
    pub fn new(dirs: Vec<PathBuf>) -> Library {
        let mut files = HashMap::new();
        for d in &dirs {
            let Ok(rd) = std::fs::read_dir(d) else {
                continue;
            };
            for e in rd.flatten() {
                let p = e.path();
                let ext = p
                    .extension()
                    .map(|x| x.to_string_lossy().to_ascii_lowercase());
                if matches!(ext.as_deref(), Some("fev" | "fsb"))
                    && let Some(name) = p.file_name()
                {
                    files.insert(key(&name.to_string_lossy()), p.clone());
                }
            }
        }
        Library {
            dirs,
            files,
            projects: Mutex::new(HashMap::new()),
            banks: Mutex::new(HashMap::new()),
            cache: Mutex::new((HashMap::new(), 0)),
        }
    }

    /// The audio folders of an install, lowest priority first: the core package, the
    /// single-player campaign, then each DLC folder present (`Addins/*/core` and `module`,
    /// `Offers/*/...`), each with `audio/sound` then `audio/vo/<language>/vo`.
    pub fn install_dirs(root: &Path, language: &str) -> Vec<PathBuf> {
        let mut packages = vec![
            root.join("packages/core"),
            root.join("packages/core_ep1"),
            root.join("modules/single player"),
        ];
        for top in ["Addins", "Offers"] {
            let mut dlc: Vec<PathBuf> = std::fs::read_dir(root.join(top))
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .collect();
            dlc.sort();
            for d in dlc {
                packages.push(d.join("core"));
                packages.push(d.join("module"));
            }
        }
        let mut out = Vec::new();
        for p in packages {
            for sub in [
                PathBuf::from("audio/sound"),
                Path::new("audio/vo").join(language).join("vo"),
            ] {
                let d = p.join(sub);
                if d.is_dir() {
                    out.push(d);
                }
            }
        }
        out
    }

    pub fn path(&self, file: &str) -> Option<&Path> {
        self.files.get(&key(file)).map(|p| p.as_path())
    }

    /// The project (`.fev`) named `name` (`music`, `glo_gui`, `ss_alistair`).
    pub fn project(&self, name: &str) -> Option<Arc<Fev>> {
        let k = key(name);
        let mut projects = self.projects.lock().ok()?;
        if let Some(p) = projects.get(&k) {
            return p.clone();
        }
        let loaded = self.path(&format!("{name}.fev")).and_then(|p| {
            let data = std::fs::read(p)
                .map_err(|e| log::warn!("audio: {}: {e}", p.display()))
                .ok()?;
            Fev::parse(&data)
                .map_err(|e| log::warn!("audio: {}: {e}", p.display()))
                .ok()
                .map(Arc::new)
        });
        if loaded.is_none() {
            log::debug!("audio: no project {name}");
        }
        projects.insert(k, loaded.clone());
        loaded
    }

    fn bank(&self, name: &str) -> Option<Arc<Bank>> {
        let k = key(name);
        let mut banks = self.banks.lock().ok()?;
        if let Some(b) = banks.get(&k) {
            return b.clone();
        }
        let loaded = self.path(&format!("{name}.fsb")).and_then(|p| {
            let read = || -> std::io::Result<Fsb> {
                let mut f = File::open(p)?;
                let mut head = vec![0u8; 48];
                f.read_exact(&mut head)?;
                let len = Fsb::header_len(&head).map_err(std::io::Error::other)?;
                head.resize(len, 0);
                f.read_exact(&mut head[48..])?;
                Fsb::parse(&head).map_err(std::io::Error::other)
            };
            read()
                .map_err(|e| log::warn!("audio: {}: {e}", p.display()))
                .ok()
                .map(|fsb| {
                    Arc::new(Bank {
                        path: p.to_path_buf(),
                        fsb,
                    })
                })
        });
        banks.insert(k, loaded.clone());
        loaded
    }

    /// The index of the sample named `sample` (with or without `.wav`) in bank `bank`.
    pub fn find_sample(&self, bank: &str, sample: &str) -> Option<usize> {
        self.bank(bank)?.fsb.find(sample).map(|(i, _)| i)
    }

    /// The length in seconds of the sample named `sample` in bank `bank` (from its header).
    pub fn sample_seconds(&self, bank: &str, sample: &str) -> Option<f64> {
        let b = self.bank(bank)?;
        let (_, s) = b.fsb.find(sample)?;
        (s.frequency > 0).then(|| s.length as f64 / s.frequency as f64)
    }

    /// Sample `index` of bank `bank`, decoded (cached).
    pub fn sample(&self, bank: &str, index: usize) -> Option<Arc<Pcm>> {
        let ck = (key(bank), index);
        if let Some(p) = self.cache.lock().ok()?.0.get(&ck) {
            return Some(p.clone());
        }
        let b = self.bank(bank)?;
        let s = b.fsb.samples.get(index)?;
        let read = || -> std::io::Result<Vec<u8>> {
            let mut f = File::open(&b.path)?;
            f.seek(SeekFrom::Start(s.data.start as u64))?;
            let mut v = vec![0u8; s.data.len()];
            f.read_exact(&mut v)?;
            Ok(v)
        };
        let data = read()
            .map_err(|e| log::warn!("audio: {} #{index}: {e}", b.path.display()))
            .ok()?;
        let pcm = Arc::new(
            decode(s, &data)
                .map_err(|e| log::warn!("audio: {} #{index}: {e}", b.path.display()))
                .ok()?,
        );
        let mut cache = self.cache.lock().ok()?;
        let bytes = pcm.samples.len() * 4;
        if cache.1 + bytes > CACHE_BYTES {
            // Drop what nothing is playing.
            cache.0.retain(|_, p| Arc::strong_count(p) > 1);
            cache.1 = cache.0.values().map(|p| p.samples.len() * 4).sum();
        }
        cache.1 += bytes;
        cache.0.insert(ck, pcm.clone());
        Some(pcm)
    }
}
