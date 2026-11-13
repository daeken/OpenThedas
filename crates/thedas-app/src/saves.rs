//! The player's saved games: the simulation's own save files (`Sim::save_json`), one per slot,
//! in a folder of their own (never the install's), each with a small description the save and
//! load screens list (its name, where the party was, how long it had played, when it was
//! written).
//!
//! The screens' layout is the game's (saveload.gfx: `SaveLoad.SaveFileList.N`); the folder
//! layout is ours. Provisional: one hero's saves only (the screens' hero list holds the one
//! playing), no screenshots, every save of the manual kind.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use thedas_ui::binding::SaveListing;

/// What the list shows of a save, kept beside it (`<slot>.meta.json`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SaveMeta {
    pub name: String,
    pub area: String,
    /// The simulation's clock when it was saved, milliseconds.
    pub time_ms: u64,
    /// When the file was written (seconds since the Unix epoch), for the list's order and
    /// its "last played".
    pub written: u64,
    /// The hero's name, level and class (`properties.gda` CurrentClass), which the load
    /// screen shows before a game is running.
    #[serde(default)]
    pub hero: String,
    #[serde(default)]
    pub level: i32,
    #[serde(default)]
    pub class: i32,
}

pub struct SaveStore {
    dir: PathBuf,
}

impl SaveStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// `saves` in the player's [`data_dir`].
    pub fn default_dir() -> PathBuf {
        data_dir().join("saves")
    }

    fn save_path(&self, slot: i32) -> PathBuf {
        self.dir.join(format!("{slot:04}.json"))
    }

    fn meta_path(&self, slot: i32) -> PathBuf {
        self.dir.join(format!("{slot:04}.meta.json"))
    }

    /// The saves there are, newest first.
    pub fn list(&self) -> Vec<SaveListing> {
        let Ok(rd) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut out: Vec<(u64, SaveListing)> = rd
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                let slot: i32 = name.strip_suffix(".meta.json")?.parse().ok()?;
                if !self.save_path(slot).is_file() {
                    return None;
                }
                let meta: SaveMeta = serde_json::from_slice(&std::fs::read(e.path()).ok()?).ok()?;
                Some((
                    meta.written,
                    SaveListing {
                        slot,
                        name: meta.name,
                        area: meta.area,
                        time_played_s: meta.time_ms / 1000,
                        last_played: format_time(meta.written),
                        hero: meta.hero,
                        level: meta.level,
                        class: meta.class,
                    },
                ))
            })
            .collect();
        out.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.slot.cmp(&a.1.slot)));
        out.into_iter().map(|(_, s)| s).collect()
    }

    /// The slot a new save goes in: one past the highest used.
    pub fn open_slot(&self) -> i32 {
        self.list().iter().map(|s| s.slot).max().unwrap_or(0) + 1
    }

    /// Writes a save (the simulation's bytes) to `slot` with its description.
    pub fn write(&self, slot: i32, bytes: &[u8], meta: &SaveMeta) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        std::fs::write(self.save_path(slot), bytes)?;
        let m = serde_json::to_vec_pretty(meta).map_err(std::io::Error::other)?;
        std::fs::write(self.meta_path(slot), m)
    }

    pub fn read(&self, slot: i32) -> std::io::Result<Vec<u8>> {
        std::fs::read(self.save_path(slot))
    }

    pub fn delete(&self, slot: i32) -> std::io::Result<()> {
        std::fs::remove_file(self.save_path(slot))?;
        let _ = std::fs::remove_file(self.meta_path(slot));
        Ok(())
    }

    /// The newest save's slot (the death screen's "Load Last Save").
    pub fn latest(&self) -> Option<i32> {
        self.list().first().map(|s| s.slot)
    }
}

/// The player's own OpenThedas folder: on Windows `OpenThedas` in the user's application
/// data (`%APPDATA%`), elsewhere `.openthedas` in their home; `./openthedas` without
/// either.
pub fn data_dir() -> PathBuf {
    #[cfg(windows)]
    let dir = dirs::data_dir().map(|d| d.join("OpenThedas"));
    #[cfg(not(windows))]
    let dir = std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(dirs::home_dir)
        .map(|h| h.join(".openthedas"));
    dir.unwrap_or_else(|| PathBuf::from("openthedas"))
}

/// Seconds since the epoch as `YYYY-MM-DD HH:MM` (UTC).
pub fn format_time(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Civil from days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + (m <= 2) as i64;
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}",
        rem / 3600,
        rem % 3600 / 60
    )
}

/// Now, in seconds since the epoch.
pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_are_listed_newest_first_and_can_be_read_back_and_deleted() {
        let dir = std::env::temp_dir().join(format!("thedas-saves-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let store = SaveStore::new(&dir);
        assert!(store.list().is_empty());
        assert_eq!(store.open_slot(), 1);
        let meta = |name: &str, written| SaveMeta {
            name: name.into(),
            area: "Lothering".into(),
            time_ms: 125_000,
            written,
            ..Default::default()
        };
        store.write(1, b"one", &meta("first", 100)).unwrap();
        store.write(2, b"two", &meta("second", 200)).unwrap();
        let l = store.list();
        assert_eq!(l.iter().map(|s| s.slot).collect::<Vec<_>>(), vec![2, 1]);
        assert_eq!(l[0].time_played_s, 125);
        assert_eq!(l[0].last_played, "1970-01-01 00:03");
        assert_eq!(store.latest(), Some(2));
        assert_eq!(store.open_slot(), 3);
        assert_eq!(store.read(1).unwrap(), b"one");
        store.delete(2).unwrap();
        assert_eq!(store.latest(), Some(1));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(format_time(1_700_000_000), "2023-11-14 22:13");
    }
}
