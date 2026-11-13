//! How long a conversation line's voice-over runs, read from the voice banks' headers.
//!
//! A conversation's voice-over is the bank named after it (`<dialog>.fsb`), loose under
//! each package's `audio/vo/<language>/vo` folder, with one sample per line named
//! `<strref>_m` (and `<strref>_f` where a female player character's reading differs). The
//! simulation needs the length to time the lines said in passing (see
//! [`crate::dialog::AmbientTalk`]); nothing is decoded and nothing is played here.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use thedas_formats::fsb::Fsb;
use thedas_vfs::join_ci;

/// The voice banks of an install, found and read as they are asked for.
#[derive(Debug, Default)]
pub struct Voices {
    /// The campaign's module folder (`single player`).
    pub module: String,
    /// The voice-over language folder (`audio/vo/<language>`).
    pub language: String,
    /// The bank files by lowercase file name, found once (`None` until then).
    files: Option<HashMap<String, PathBuf>>,
    /// Each bank's sample lengths in milliseconds by lowercase sample name (without
    /// `.wav`); `None` for a bank that is not there or does not parse.
    banks: HashMap<String, Option<HashMap<String, u32>>>,
}

impl Voices {
    pub fn new() -> Voices {
        Voices {
            language: "en-us".into(),
            ..Default::default()
        }
    }

    /// The voice folders of the campaign, lowest priority first: the core package, each
    /// add-in's `core` folder, then the campaign's module, as the audio library searches
    /// them (`thedas_audio::Library::install_dirs`, its voice half).
    fn dirs(&self, root: &Path) -> Vec<PathBuf> {
        let mut packages = vec![join_ci(root, "packages/core")];
        for top in ["Addins", "Offers"] {
            let mut dlc: Vec<PathBuf> = std::fs::read_dir(join_ci(root, top))
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| join_ci(&e.path(), "core"))
                .filter(|p| p.is_dir())
                .collect();
            dlc.sort();
            packages.extend(dlc);
        }
        if !self.module.is_empty() {
            packages.push(join_ci(root, &format!("modules/{}", self.module)));
        }
        packages
            .iter()
            .map(|p| join_ci(p, &format!("audio/vo/{}/vo", self.language)))
            .filter(|d| d.is_dir())
            .collect()
    }

    fn bank(&mut self, root: &Path, dialog: &str) -> Option<&HashMap<String, u32>> {
        let key = dialog.to_ascii_lowercase();
        if !self.banks.contains_key(&key) {
            if self.files.is_none() {
                let mut files = HashMap::new();
                for d in self.dirs(root) {
                    for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                        let p = e.path();
                        if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("fsb"))
                            && let Some(name) = p.file_name()
                        {
                            files.insert(name.to_string_lossy().to_ascii_lowercase(), p.clone());
                        }
                    }
                }
                self.files = Some(files);
            }
            let path = self
                .files
                .as_ref()
                .and_then(|f| f.get(&format!("{key}.fsb")))
                .cloned();
            let read = |p: &Path| -> Option<HashMap<String, u32>> {
                let mut f = std::fs::File::open(p).ok()?;
                let mut head = vec![0u8; 48];
                f.read_exact(&mut head).ok()?;
                let len = Fsb::header_len(&head).ok()?;
                // A bank's headers are a few hundred bytes a sample; one that claims more
                // than this is not read.
                if !(48..=64 << 20).contains(&len) {
                    return None;
                }
                head.resize(len, 0);
                f.read_exact(&mut head[48..]).ok()?;
                let fsb = Fsb::parse(&head).ok()?;
                Some(
                    fsb.samples
                        .iter()
                        .filter(|s| s.frequency > 0)
                        .map(|s| {
                            let ms = u64::from(s.length) * 1000 / u64::from(s.frequency);
                            (
                                s.stem().to_ascii_lowercase(),
                                ms.min(u64::from(u32::MAX)) as u32,
                            )
                        })
                        .collect(),
                )
            };
            let loaded = path.as_deref().and_then(read);
            self.banks.insert(key.clone(), loaded);
        }
        self.banks.get(&key)?.as_ref()
    }

    /// How long line `strref` of conversation `dialog` is voiced for, in milliseconds, or
    /// `None` when it has no voice-over: the female reading for a female player character
    /// when the bank has one, else the male one (as the game's voice-over is picked).
    pub fn line_ms(&mut self, root: &Path, dialog: &str, strref: u32, female: bool) -> Option<u32> {
        let bank = self.bank(root, dialog)?;
        female
            .then(|| bank.get(&format!("{strref}_f")))
            .flatten()
            .or_else(|| bank.get(&format!("{strref}_m")))
            .copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_install_without_voice_folders_has_no_voiced_lines() {
        let dir = std::env::temp_dir().join(format!("thedas-voices-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut v = Voices::new();
        assert_eq!(v.line_ms(&dir, "some_dialog", 12, false), None);
        // A bank that is not a bank is no voice either.
        let vo = dir.join("packages/core/audio/vo/en-us/vo");
        std::fs::create_dir_all(&vo).unwrap();
        std::fs::write(vo.join("broken.fsb"), b"not a bank").unwrap();
        let mut v = Voices::new();
        assert_eq!(v.line_ms(&dir, "broken", 12, true), None);
        std::fs::remove_dir_all(&dir).ok();
    }
}
