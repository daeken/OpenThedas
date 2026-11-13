//! Journal-style saves in the app: as a game is played it keeps a checkpoint (an ordinary
//! save) and a log of every input since it (`thedas_sim::replay`), in the data folder's
//! `journal/`. A new checkpoint is made when the area changes, after a load, and every few
//! game minutes; the pair before it is kept as `*.prev`. Resuming loads the checkpoint and
//! replays the log, so the game comes back as it was when it stopped, a crash included.
//!
//! The log is written and flushed every frame, which survives the app being killed but
//! not the machine losing power (nothing calls `fsync`).
//!
//! It is kept only for interactive play: runs with `--capture`, `--steps` or `--control`
//! keep none unless `--journal` (or `--resume`) asks, so a test never replaces the
//! player's journal.
//!
//! See `OpenThedas-testdata/notes/journal-saves-plan.md`.

use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use thedas_script::vm::ObjectId;
use thedas_sim::Sim;
use thedas_sim::replay::{JournalWriter, ReplayReport};

/// Game time between checkpoints, in milliseconds.
const CHECKPOINT_EVERY_MS: u64 = 5 * 60 * 1000;

static ENABLED: AtomicBool = AtomicBool::new(true);

/// Whether this run keeps a journal (see the module's doc).
pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed) && std::env::var_os("THEDAS_NO_JOURNAL").is_none()
}

/// The running build, as logs name it: a log is replayed only by the build that wrote it.
pub fn build() -> &'static str {
    option_env!("THEDAS_GIT_COMMIT").unwrap_or("dev")
}

/// The journal's folder in the player's data folder.
pub fn default_dir() -> PathBuf {
    crate::saves::data_dir().join("journal")
}

fn checkpoint_path(dir: &Path) -> PathBuf {
    dir.join("checkpoint.json")
}

fn log_path(dir: &Path) -> PathBuf {
    dir.join("checkpoint.journal")
}

/// Present when the game that wrote the journal was quit normally.
fn closed_path(dir: &Path) -> PathBuf {
    dir.join("closed")
}

/// Whether the journal may make a checkpoint now: not in the middle of a transition, a
/// cutscene, a conversation, character creation or the party picker (what those hold
/// outside the save would be lost).
fn quiet(sim: &Sim) -> bool {
    sim.world.transition.is_none()
        && sim.world.cutscene.is_none()
        && sim.world.chargen.is_none()
        && sim.world.party_picker.is_none()
        && sim.dialog_state().is_none_or(|d| d.ended)
}

/// The journal of the game being played.
pub struct GameJournal {
    dir: PathBuf,
    writer: JournalWriter<BufWriter<File>>,
    /// The area, game time and load count at the last checkpoint.
    area: Option<ObjectId>,
    at_ms: u64,
    loads: u64,
}

impl GameJournal {
    /// Starts the journal in `dir` from the world as it is now, when the game is at a quiet
    /// point (`None` otherwise: try again next frame): writes the checkpoint and opens its
    /// log (the previous pair becomes `*.prev`).
    pub fn start(sim: &mut Sim, dir: &Path) -> std::io::Result<Option<Self>> {
        if !quiet(sim) {
            return Ok(None);
        }
        std::fs::create_dir_all(dir)?;
        let _ = std::fs::remove_file(closed_path(dir));
        set_aside(dir)?;
        let bytes = sim
            .save_json()
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        std::fs::write(checkpoint_path(dir), bytes)?;
        let out = BufWriter::new(File::create(log_path(dir))?);
        let writer = JournalWriter::start(sim, build(), out)?;
        Ok(Some(GameJournal {
            dir: dir.to_path_buf(),
            writer,
            area: sim.world.area,
            at_ms: sim.world.time_ms,
            loads: sim.loads,
        }))
    }

    /// Writes the inputs given since the last call, and makes a new checkpoint when a game
    /// was loaded in place (quickload, the load screen), the area has changed or enough game
    /// time has passed. Called each frame.
    pub fn after_frame(&mut self, sim: &mut Sim) -> std::io::Result<()> {
        self.writer.take(sim)?;
        self.writer.flush()?;
        let loaded = sim.loads != self.loads;
        let due = loaded
            || sim.world.area != self.area
            || sim.world.time_ms.saturating_sub(self.at_ms) >= CHECKPOINT_EVERY_MS;
        if due {
            if let Some(j) = GameJournal::start(sim, &self.dir)? {
                *self = j;
            } else if loaded {
                // A game loaded at a busy point: the log so far does not lead to it, so it
                // is set aside and inputs are dropped until a checkpoint can be made.
                set_aside(&self.dir)?;
                sim.input_log = Some(Vec::new());
            }
        }
        Ok(())
    }

    /// The game is being quit: what is left is written, and the journal marked as closed
    /// (so it is not offered as a session that ended unexpectedly).
    pub fn close(&mut self, sim: &mut Sim) {
        let _ = self.writer.take(sim);
        let _ = self.writer.flush();
        let _ = std::fs::write(closed_path(&self.dir), b"");
    }

    /// The checkpoint and the log as they stand, written out first (for an issue report).
    pub fn files(&mut self) -> Option<(Vec<u8>, Vec<u8>)> {
        self.writer.flush().ok()?;
        let checkpoint = std::fs::read(checkpoint_path(&self.dir)).ok()?;
        let log = std::fs::read(log_path(&self.dir)).ok()?;
        Some((checkpoint, log))
    }
}

/// The current pair, renamed `*.prev`.
fn set_aside(dir: &Path) -> std::io::Result<()> {
    for p in [checkpoint_path(dir), log_path(dir)] {
        if p.exists() {
            let mut prev = p.clone().into_os_string();
            prev.push(".prev");
            std::fs::rename(&p, prev)?;
        }
    }
    Ok(())
}

/// What a journal holds, for offering to resume it (see [`resumable`]).
#[allow(dead_code)] // For the start menu's offer to resume (UI).
#[derive(Clone, Debug, PartialEq)]
pub struct ResumeInfo {
    /// The area's tag at the checkpoint.
    pub area: String,
    /// Game time at the last input, in milliseconds.
    pub time_ms: u64,
    /// When the log was last written.
    pub written: std::time::SystemTime,
    /// Whether the game that wrote it was quit normally.
    pub closed: bool,
    /// Whether this build can replay it (it was written by this build).
    pub this_build: bool,
}

/// What the journal in `dir` holds, without replaying it; `None` when there is none.
/// Reads the checkpoint (a few megabytes) and the log.
#[allow(dead_code)] // For the start menu's offer to resume (UI).
pub fn resumable(dir: &Path) -> Option<ResumeInfo> {
    let meta = std::fs::metadata(log_path(dir)).ok()?;
    let checkpoint: serde_json::Value =
        serde_json::from_slice(&std::fs::read(checkpoint_path(dir)).ok()?).ok()?;
    let world = &checkpoint["world"];
    let area = world["area"]
        .as_u64()
        .and_then(|a| world["objects"][a.to_string()]["tag"].as_str())
        .unwrap_or("")
        .to_string();
    let mut first = None;
    let mut last_t = world["time_ms"].as_u64().unwrap_or(0);
    for line in BufReader::new(File::open(log_path(dir)).ok()?).lines() {
        let Ok(line) = line else { break };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
            break;
        };
        if let Some(t) = v["t"].as_u64() {
            last_t = t;
        }
        if first.is_none() {
            first = Some(v);
        }
    }
    Some(ResumeInfo {
        area,
        time_ms: last_t,
        written: meta.modified().ok()?,
        closed: closed_path(dir).exists(),
        this_build: first.is_some_and(|h| h["build"].as_str() == Some(build())),
    })
}

/// Sets the journal in `dir` aside (as `*.prev`), so it is not offered again.
#[allow(dead_code)] // For the start menu's offer to resume (UI).
pub fn discard(dir: &Path) -> std::io::Result<()> {
    set_aside(dir)?;
    let _ = std::fs::remove_file(closed_path(dir));
    Ok(())
}

/// The checkpoint to resume from, if there is one.
pub fn checkpoint(dir: &Path) -> Option<Vec<u8>> {
    std::fs::read(checkpoint_path(dir)).ok()
}

/// Replays the journal's log onto `sim`, which holds its checkpoint as just loaded.
pub fn replay(sim: &mut Sim, dir: &Path) -> ReplayReport {
    match File::open(log_path(dir)) {
        Ok(f) => thedas_sim::replay::replay(sim, BufReader::new(f), build(), false),
        Err(e) => ReplayReport {
            inputs: 0,
            applied: 0,
            mismatch: None,
            refused: Some(format!("no log: {e}")),
        },
    }
}
