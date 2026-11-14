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
    /// The kind of save ([`KIND_SLOT`], [`KIND_AUTO`], [`KIND_QUICK`]).
    #[serde(default)]
    pub kind: i32,
}

/// The kinds of save, as DAOrigins.exe names their folders (0x642c70): a save the player
/// made on the save screen (`Slot_`), the autosave (`AutoSave_`) and the quick save
/// (`QuickSave_`). (3 is `PostCampaign_`.)
pub const KIND_SLOT: i32 = 0;
pub const KIND_AUTO: i32 = 1;
pub const KIND_QUICK: i32 = 2;

/// The player's own folder for the game's saves and settings. On Windows it is
/// `OpenThedas` in the player's Documents (the known folder, which may be redirected, e.g. to
/// OneDrive), where the original keeps its own (`BioWare\Dragon Age`); elsewhere
/// `.openthedas` in the home folder (`HOME`). Beside the game when neither is found.
pub fn data_dir() -> PathBuf {
    thedas_vfs::mods::user_data_dir()
}

/// The player's settings file (see `thedas_ui::options::to_text`).
pub fn default_settings() -> PathBuf {
    data_dir().join("options.ini")
}

pub struct SaveStore {
    dir: PathBuf,
}

impl SaveStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// `$HOME/.openthedas/saves`, or `./openthedas-saves` without a home.
    pub fn default_dir() -> PathBuf {
        data_dir().join("saves")
    }

    fn save_path(&self, slot: i32) -> PathBuf {
        self.dir.join(format!("{slot:04}.json"))
    }

    fn meta_path(&self, slot: i32) -> PathBuf {
        self.dir.join(format!("{slot:04}.meta.json"))
    }

    fn shot_path(&self, slot: i32) -> PathBuf {
        self.dir.join(format!("{slot:04}.png"))
    }

    /// Keeps the picture of the game taken with save `slot` (a PNG), which the save and load
    /// screens show. Provisional: the original keeps a screenshot in each save's folder;
    /// its size and form are not followed.
    pub fn write_screenshot(&self, slot: i32, png: &[u8]) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        std::fs::write(self.shot_path(slot), png)
    }

    /// Save `slot`'s picture, when it has one.
    pub fn screenshot(&self, slot: i32) -> Option<Vec<u8>> {
        std::fs::read(self.shot_path(slot)).ok()
    }

    /// The name the screens load save `slot`'s picture by: new with each write of the save,
    /// so a slot written over shows its new picture.
    pub fn screenshot_name(slot: i32, written: u64) -> String {
        format!("save_{slot}_{written}.dds")
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
                        kind: meta.kind,
                        screenshot: if self.shot_path(slot).is_file() {
                            Self::screenshot_name(slot, meta.written)
                        } else {
                            String::new()
                        },
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

    /// The quick save's slot, when there is one. Provisional: one quick save is kept and
    /// written over; how many the original keeps is not traced.
    pub fn quick_slot(&self) -> Option<i32> {
        self.list()
            .iter()
            .find(|s| s.kind == KIND_QUICK)
            .map(|s| s.slot)
    }

    /// The autosave's slot, when there is one. Provisional: one autosave is kept and
    /// written over; how many the original keeps is not traced.
    pub fn auto_slot(&self) -> Option<i32> {
        self.list()
            .iter()
            .find(|s| s.kind == KIND_AUTO)
            .map(|s| s.slot)
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
        let _ = std::fs::remove_file(self.shot_path(slot));
        Ok(())
    }

    /// The newest save's slot (the death screen's "Load Last Save").
    pub fn latest(&self) -> Option<i32> {
        self.list().first().map(|s| s.slot)
    }
}

/// The width of a save's picture (pixels); its height keeps the frame's shape.
pub const SHOT_WIDTH: u32 = 512;

/// A frame (RGBA8, `w` x `h`) shrunk to a save's picture, [`SHOT_WIDTH`] wide (or the
/// frame's own width when smaller), each pixel the average of the frame's under it, as a
/// PNG. `None` for an empty or short frame.
pub fn shot_png(pixels: &[u8], w: u32, h: u32) -> Option<Vec<u8>> {
    if w == 0 || h == 0 || pixels.len() < w as usize * h as usize * 4 {
        return None;
    }
    let tw = SHOT_WIDTH.min(w);
    let th = ((u64::from(h) * u64::from(tw)) / u64::from(w)).max(1) as u32;
    let mut out = Vec::with_capacity(tw as usize * th as usize * 4);
    for ty in 0..th {
        let (y0, y1) = (ty * h / th, ((ty + 1) * h / th).max(ty * h / th + 1).min(h));
        for tx in 0..tw {
            let (x0, x1) = (tx * w / tw, ((tx + 1) * w / tw).max(tx * w / tw + 1).min(w));
            let mut sum = [0u32; 3];
            for y in y0..y1 {
                for x in x0..x1 {
                    let i = (y as usize * w as usize + x as usize) * 4;
                    for c in 0..3 {
                        sum[c] += u32::from(pixels[i + c]);
                    }
                }
            }
            let n = (y1 - y0) * (x1 - x0);
            out.extend([
                (sum[0] / n) as u8,
                (sum[1] / n) as u8,
                (sum[2] / n) as u8,
                255,
            ]);
        }
    }
    thedas_ui::stage::encode_png(&thedas_ui::Rgba {
        width: tw,
        height: th,
        pixels: out,
    })
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
    fn a_frame_shrinks_to_a_picture_of_its_shape() {
        // 1024 x 512 of one colour: a 512 x 256 PNG.
        let frame = [10u8, 20, 30, 255].repeat(1024 * 512);
        let png = shot_png(&frame, 1024, 512).unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
        // IHDR: width and height, big-endian, at bytes 16..24.
        assert_eq!(&png[16..24], &[0, 0, 2, 0, 0, 0, 1, 0]);
        // A frame smaller than the picture keeps its size; a short one gives none.
        let small = [0u8, 0, 0, 255].repeat(4 * 2);
        assert_eq!(
            &shot_png(&small, 4, 2).unwrap()[16..24],
            &[0, 0, 0, 4, 0, 0, 0, 2]
        );
        assert_eq!(shot_png(&small, 8, 8), None);
        assert_eq!(shot_png(&[], 0, 0), None);
    }

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
        // A save's picture is listed by a name that changes with each write.
        assert_eq!(l[0].screenshot, "");
        store.write_screenshot(2, b"png").unwrap();
        assert_eq!(store.list()[0].screenshot, "save_2_200.dds");
        assert_eq!(store.screenshot(2).as_deref(), Some(&b"png"[..]));
        store.delete(2).unwrap();
        assert_eq!(store.screenshot(2), None);
        assert_eq!(store.latest(), Some(1));
        // The quick save is found by its kind.
        assert_eq!(store.quick_slot(), None);
        let quick = SaveMeta {
            kind: KIND_QUICK,
            ..meta("quick", 300)
        };
        store.write(5, b"q", &quick).unwrap();
        assert_eq!(store.quick_slot(), Some(5));
        assert_eq!(store.list()[0].kind, KIND_QUICK);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(format_time(1_700_000_000), "2023-11-14 22:13");
    }
}
