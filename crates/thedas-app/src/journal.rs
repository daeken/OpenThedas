//! Journal-style saves in the app: as a game is played it keeps a checkpoint (an ordinary
//! save) and a log of every input since it (`thedas_sim::replay`), in the data folder's
//! `journal/`. A new checkpoint is made when the area changes and every few game minutes;
//! the pair before it is kept as `*.prev`. `--resume` loads the checkpoint and replays the
//! log, so the game comes back as it was when it stopped, a crash included.
//!
//! See `OpenThedas-testdata/notes/journal-saves-plan.md`.

use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf};

use thedas_script::vm::ObjectId;
use thedas_sim::Sim;
use thedas_sim::replay::{JournalWriter, ReplayReport};

/// Game time between checkpoints, in milliseconds.
const CHECKPOINT_EVERY_MS: u64 = 5 * 60 * 1000;

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

/// The journal of the game being played.
pub struct GameJournal {
    dir: PathBuf,
    writer: JournalWriter<BufWriter<File>>,
    /// The area and game time at the last checkpoint.
    area: Option<ObjectId>,
    at_ms: u64,
}

impl GameJournal {
    /// Starts the journal in `dir` from the world as it is now: writes the checkpoint and
    /// opens its log (the previous pair becomes `*.prev`).
    pub fn start(sim: &mut Sim, dir: &Path) -> std::io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        for p in [checkpoint_path(dir), log_path(dir)] {
            if p.exists() {
                let mut prev = p.clone().into_os_string();
                prev.push(".prev");
                std::fs::rename(&p, prev)?;
            }
        }
        let bytes = sim
            .save_json()
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        std::fs::write(checkpoint_path(dir), bytes)?;
        let out = BufWriter::new(File::create(log_path(dir))?);
        let writer = JournalWriter::start(sim, build(), out)?;
        Ok(GameJournal {
            dir: dir.to_path_buf(),
            writer,
            area: sim.world.area,
            at_ms: sim.world.time_ms,
        })
    }

    /// Writes the inputs given since the last call, and makes a new checkpoint when the
    /// area has changed or enough game time has passed. Called each frame.
    pub fn after_frame(&mut self, sim: &mut Sim) -> std::io::Result<()> {
        self.writer.take(sim)?;
        let due = sim.world.area != self.area
            || sim.world.time_ms.saturating_sub(self.at_ms) >= CHECKPOINT_EVERY_MS;
        // Not in the middle of a transition, a cutscene or a conversation: what those hold
        // outside the save (lines said in passing, a conversation's staging) would be lost.
        let quiet = sim.world.transition.is_none()
            && sim.world.cutscene.is_none()
            && sim.dialog_state().is_none_or(|d| d.ended);
        if due && quiet {
            self.writer.flush()?;
            *self = GameJournal::start(sim, &self.dir)?;
        } else {
            self.writer.flush()?;
        }
        Ok(())
    }
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
