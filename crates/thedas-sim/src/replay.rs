//! Journal-style saves: a checkpoint (an ordinary save) and, after it, a log of every
//! command the simulation was given ([`crate::commands::InputEntry`], through
//! [`Sim::apply`]). The simulation is deterministic, so replaying the log onto the
//! checkpoint gives the world back as it was when the log ended. The log is one JSON
//! object per line, appended as the game is played, so a crash loses nothing that was
//! written; every so often it carries a hash of the world, and replay stops at the first
//! hash that does not match (a build that plays differently, or a non-deterministic
//! step), keeping the world as it was at the last one that did.
//!
//! See `OpenThedas-testdata/notes/journal-saves-plan.md`.

use std::io::{BufRead, Write};

use crate::commands::InputEntry;
use crate::sim::Sim;

/// The first line of a log: what it can be replayed onto.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct JournalHeader {
    /// The build that wrote it: a log is only replayed by the build that wrote it (another
    /// build may play the same inputs differently).
    pub build: String,
    /// [`crate::save::SAVE_REVISION`] of that build.
    pub save_revision: u32,
    /// [`world_hash`] of the checkpoint the log starts from.
    pub checkpoint: u64,
}

/// One line of a log.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "line", rename_all = "snake_case")]
pub enum JournalLine {
    Header(JournalHeader),
    Input(InputEntry),
    /// The input before, `n` more times (a tick each frame, a held stick's orders).
    Repeat {
        n: u32,
    },
    /// The world's hash after the `index`th input (counting from 1, repeats counted).
    Check {
        index: u64,
        t: u64,
        hash: u64,
    },
}

/// A hash of the parts of the world a replay must get right: the clock, the game mode, the
/// area, every plot flag, the random number state, and every object's kind, area,
/// position, facing, properties (health among them), death and command queue length. 64-bit FNV-1a over a fixed
/// serialization, so it is the same on every platform and build of the same code.
pub fn world_hash(sim: &Sim) -> u64 {
    let w = &sim.world;
    let mut h = Fnv(0xcbf2_9ce4_8422_2325);
    h.u64(w.time_ms);
    h.u64(w.game_mode as u64);
    h.u64(w.area.map_or(u64::MAX, u64::from));
    h.u64(w.rng.state());
    for ((plot, flag), v) in &w.plots {
        h.bytes(plot.as_bytes());
        h.u64(*flag as u64);
        h.u64(*v as u64);
    }
    for o in w.objects.values() {
        h.u64(u64::from(o.id));
        h.u64(o.kind as u64);
        h.u64(o.area.map_or(u64::MAX, u64::from));
        for c in o.position {
            h.u64(u64::from(c.to_bits()));
        }
        h.u64(u64::from(o.facing.to_bits()));
        h.u64(o.dead as u64);
        h.u64(o.active as u64);
        h.u64(o.commands.len() as u64);
        for (id, (cur, max)) in &o.properties {
            h.u64(*id as u64);
            h.u64(u64::from(cur.to_bits()));
            h.u64(u64::from(max.to_bits()));
        }
    }
    h.0
}

struct Fnv(u64);

impl Fnv {
    fn bytes(&mut self, b: &[u8]) {
        for &x in b {
            self.0 ^= u64::from(x);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
        // A separator, so ("ab", "c") and ("a", "bc") differ.
        self.0 ^= 0xff;
        self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
    }
    fn u64(&mut self, v: u64) {
        self.bytes(&v.to_le_bytes());
    }
}

/// Writes a log as the game is played: [`JournalWriter::take`] appends what the
/// simulation's input log gathered since the last call (each frame, or after each input),
/// an input given again and again as one [`JournalLine::Repeat`], and a world hash every
/// [`JournalWriter::check_every`] inputs or so.
pub struct JournalWriter<W: Write> {
    out: W,
    /// Inputs written, repeats counted.
    written: u64,
    /// Inputs at the last world hash.
    checked: u64,
    /// The last input written, and how often it came again since (not written yet).
    last: Option<(crate::commands::CommandSource, crate::commands::SimCommand)>,
    pending: u32,
    /// Inputs between world hashes.
    pub check_every: u64,
}

/// Repeats held back at most: a crash loses no more than this many repeated inputs.
const MAX_PENDING: u32 = 300;

impl<W: Write> JournalWriter<W> {
    /// Starts a log onto `out` for the checkpoint `sim` is at now, and turns the
    /// simulation's input log on.
    pub fn start(sim: &mut Sim, build: &str, mut out: W) -> std::io::Result<Self> {
        let header = JournalLine::Header(JournalHeader {
            build: build.to_string(),
            save_revision: crate::save::SAVE_REVISION,
            checkpoint: world_hash(sim),
        });
        writeln!(out, "{}", serde_json::to_string(&header)?)?;
        sim.input_log = Some(Vec::new());
        Ok(JournalWriter {
            out,
            written: 0,
            checked: 0,
            last: None,
            pending: 0,
            check_every: 600,
        })
    }

    fn line(&mut self, l: &JournalLine) -> std::io::Result<()> {
        writeln!(self.out, "{}", serde_json::to_string(l)?)
    }

    fn write_pending(&mut self) -> std::io::Result<()> {
        if self.pending > 0 {
            let n = std::mem::take(&mut self.pending);
            self.line(&JournalLine::Repeat { n })?;
        }
        Ok(())
    }

    /// Appends the inputs the simulation has gathered since the last call, and the world's
    /// hash when one is due (taken as the world is now, after all of them).
    pub fn take(&mut self, sim: &mut Sim) -> std::io::Result<()> {
        let Some(log) = sim.input_log.as_mut() else {
            return Ok(());
        };
        for e in std::mem::take(log) {
            self.written += 1;
            let same = self
                .last
                .as_ref()
                .is_some_and(|(s, c)| *s == e.source && *c == e.command);
            if same && self.pending < MAX_PENDING {
                self.pending += 1;
                continue;
            }
            self.write_pending()?;
            if same {
                self.pending = 1;
                continue;
            }
            self.last = Some((e.source, e.command.clone()));
            self.line(&JournalLine::Input(e))?;
        }
        if self.written - self.checked >= self.check_every {
            self.write_pending()?;
            self.checked = self.written;
            let check = JournalLine::Check {
                index: self.written,
                t: sim.world.time_ms,
                hash: world_hash(sim),
            };
            self.line(&check)?;
        }
        Ok(())
    }

    /// Writes what is held back and flushes the output.
    pub fn flush(&mut self) -> std::io::Result<()> {
        self.write_pending()?;
        self.out.flush()
    }

    /// The output, after what is held back is written.
    pub fn into_inner(mut self) -> W {
        let _ = self.write_pending();
        self.out
    }
}

/// What a replay did.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct ReplayReport {
    /// Inputs in the log, and those applied (and kept).
    pub inputs: u64,
    pub applied: u64,
    /// The first world hash that did not match: (input index, expected, got). The world is
    /// left as it was at the last hash that matched (or the checkpoint).
    pub mismatch: Option<(u64, u64, u64)>,
    /// Why the log was not replayed at all (another build, another checkpoint, a bad file).
    pub refused: Option<String>,
}

/// Replays a log onto the checkpoint `sim` holds. `build` is the running build: a log
/// written by another is refused, unless `any_build`.
pub fn replay(sim: &mut Sim, log: impl BufRead, build: &str, any_build: bool) -> ReplayReport {
    let mut report = ReplayReport {
        inputs: 0,
        applied: 0,
        mismatch: None,
        refused: None,
    };
    let mut lines = Vec::new();
    for line in log.lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        // A line cut short by a crash ends the log.
        match serde_json::from_str::<JournalLine>(&line) {
            Ok(l) => lines.push(l),
            Err(_) => break,
        }
    }
    let Some(JournalLine::Header(h)) = lines.first() else {
        report.refused = Some("no header".into());
        return report;
    };
    if h.build != build && !any_build {
        report.refused = Some(format!("written by build {}, this is {build}", h.build));
        return report;
    }
    if h.checkpoint != world_hash(sim) {
        report.refused = Some("the log does not start from this checkpoint".into());
        return report;
    }
    report.inputs = lines
        .iter()
        .map(|l| match l {
            JournalLine::Input(_) => 1,
            JournalLine::Repeat { n } => u64::from(*n),
            _ => 0,
        })
        .sum();
    let was_logging = sim.input_log.take();
    let mut good = (sim.world.clone(), 0u64);
    let mut applied = 0u64;
    let mut last: Option<InputEntry> = None;
    for l in lines.into_iter().skip(1) {
        match l {
            JournalLine::Input(e) => {
                sim.apply(e.source, e.command.clone());
                applied += 1;
                last = Some(e);
            }
            JournalLine::Repeat { n } => {
                let Some(e) = last.as_ref() else { break };
                for _ in 0..n {
                    sim.apply(e.source, e.command.clone());
                    applied += 1;
                }
            }
            JournalLine::Check { index, hash, .. } => {
                let got = world_hash(sim);
                if got != hash {
                    report.mismatch = Some((index, hash, got));
                    let trace = std::mem::take(&mut sim.world.trace);
                    sim.world = good.0;
                    sim.world.trace = trace;
                    applied = good.1;
                    break;
                }
                good = (sim.world.clone(), applied);
            }
            JournalLine::Header(_) => break,
        }
    }
    report.applied = applied;
    sim.input_log = was_logging;
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::{CommandSource, SimCommand};
    use crate::world::ObjectKind;

    fn sim() -> Sim {
        let mut s = Sim::new(
            std::sync::Arc::new(thedas_vfs::Vfs::new()),
            std::sync::Arc::new(thedas_script::EngineTable::default()),
            3,
        );
        let area = s.world.spawn(ObjectKind::Area, "a");
        let hero = s.world.spawn(ObjectKind::Creature, "hero");
        s.world.hero = Some(hero);
        s.world.get_mut(hero).unwrap().area = Some(area);
        s
    }

    fn play(s: &mut Sim) {
        let hero = s.world.hero.unwrap();
        s.apply(
            CommandSource::Player,
            SimCommand::OrderMove {
                who: hero,
                to: [6.0, 2.0, 0.0],
            },
        );
        for _ in 0..10 {
            s.apply(CommandSource::App, SimCommand::Tick { ms: 100 });
        }
    }

    #[test]
    fn mod_mode_object_edits_replay_to_the_same_world() {
        let mut live = sim();
        live.world.area = live.world.get(live.world.hero.unwrap()).unwrap().area;
        let area = live.world.area;
        let crate_ = live.world.spawn(ObjectKind::Placeable, "crate");
        live.world.get_mut(crate_).unwrap().area = area;
        let checkpoint = live.world.clone();
        let mut w = JournalWriter::start(&mut live, "test", Vec::new()).unwrap();
        w.check_every = 1;
        let edits = [
            SimCommand::MoveObject {
                target: crate_,
                position: [4.0, 5.0, 0.5],
                facing: 90.0,
            },
            SimCommand::CreateObject {
                kind: ObjectKind::Creature,
                template: "some_wolf".into(),
                position: [1.0, 1.0, 0.0],
                facing: 0.0,
            },
            SimCommand::Tick { ms: 100 },
            SimCommand::DestroyObject { target: crate_ },
            SimCommand::Tick { ms: 0 },
        ];
        for c in edits {
            let source = match c {
                SimCommand::Tick { .. } => CommandSource::App,
                _ => CommandSource::ModMode,
            };
            live.apply(source, c);
            w.take(&mut live).unwrap();
        }
        assert!(live.world.get(crate_).is_none());
        let made = live
            .world
            .objects
            .values()
            .filter(|o| o.tag == "some_wolf" || o.template == "some_wolf")
            .count();
        assert_eq!(made, 1);
        let log = w.into_inner();
        let mut again = sim();
        again.world = checkpoint;
        let r = replay(&mut again, &log[..], "test", false);
        assert_eq!(
            (r.inputs, r.applied, r.mismatch, r.refused),
            (5, 5, None, None)
        );
        assert_eq!(world_hash(&again), world_hash(&live));
        assert_eq!(
            again.world.command_journal.len(),
            live.world.command_journal.len()
        );
    }

    #[test]
    fn a_log_replayed_onto_its_checkpoint_gives_the_same_world() {
        let mut live = sim();
        let checkpoint = live.world.clone();
        let mut w = JournalWriter::start(&mut live, "test", Vec::new()).unwrap();
        w.check_every = 4;
        for _ in 0..11 {
            // Take after every input, as the app does.
            let hero = live.world.hero.unwrap();
            live.apply(
                CommandSource::Player,
                SimCommand::OrderMove {
                    who: hero,
                    to: [6.0, 2.0, 0.0],
                },
            );
            w.take(&mut live).unwrap();
            live.apply(CommandSource::App, SimCommand::Tick { ms: 100 });
            w.take(&mut live).unwrap();
        }
        let log = w.into_inner();
        let mut again = sim();
        again.world = checkpoint.clone();
        let r = replay(&mut again, &log[..], "test", false);
        assert_eq!(
            (r.inputs, r.applied, r.mismatch, r.refused),
            (22, 22, None, None)
        );
        assert_eq!(world_hash(&again), world_hash(&live));
        // Another build's log is refused, as is one onto another world.
        let mut other = sim();
        other.world = checkpoint;
        assert!(
            replay(&mut other, &log[..], "other", false)
                .refused
                .is_some()
        );
        let mut moved = sim();
        play(&mut moved);
        assert!(
            replay(&mut moved, &log[..], "test", false)
                .refused
                .is_some()
        );
    }

    #[test]
    fn a_replay_that_drifts_stops_at_the_last_good_hash() {
        let mut live = sim();
        let checkpoint = live.world.clone();
        let mut w = JournalWriter::start(&mut live, "test", Vec::new()).unwrap();
        w.check_every = 2;
        for _ in 0..6 {
            live.apply(CommandSource::App, SimCommand::Tick { ms: 100 });
            w.take(&mut live).unwrap();
        }
        let text = String::from_utf8(w.into_inner()).unwrap();
        // Spoil the second hash: the replay keeps the world of the first.
        let mut checks = 0;
        let spoiled: Vec<String> = text
            .lines()
            .map(|l| {
                if l.contains("\"check\"") {
                    checks += 1;
                    if checks == 2 {
                        let at = l.find("\"hash\":").unwrap() + 7;
                        return format!("{}12345}}", &l[..at]);
                    }
                }
                l.to_string()
            })
            .collect();
        let mut again = sim();
        again.world = checkpoint;
        let r = replay(&mut again, spoiled.join("\n").as_bytes(), "test", false);
        assert_eq!(r.mismatch.map(|m| m.0), Some(4));
        assert_eq!(r.applied, 2);
        assert_eq!(again.world.time_ms, 200);
    }
}
