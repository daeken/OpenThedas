//! What the simulation did, for parity diffs and debugging. Each entry carries the world time
//! in milliseconds.

use std::collections::BTreeMap;

use serde::Serialize;
use thedas_script::vm::ObjectId;

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TraceEntry {
    ObjectCreated {
        t: u64,
        id: ObjectId,
        object_kind: String,
        tag: String,
        template: String,
    },
    EventQueued {
        t: u64,
        due: u64,
        target: ObjectId,
        ty: i32,
    },
    EventDelivered {
        t: u64,
        target: ObjectId,
        ty: i32,
        event_name: String,
        script: String,
    },
    ScriptStart {
        t: u64,
        script: String,
        caller: ObjectId,
        depth: usize,
    },
    ScriptEnd {
        t: u64,
        script: String,
        steps: u64,
        error: Option<String>,
    },
    /// An engine function the simulation does not implement yet; it returned the default
    /// for its return type.
    Unimplemented {
        t: u64,
        function: String,
        script: String,
    },
    Log {
        t: u64,
        channel: i32,
        text: String,
    },
    Missing {
        t: u64,
        what: String,
    },
    PlotFlag {
        t: u64,
        plot: String,
        flag: i32,
        value: bool,
    },
    Command {
        t: u64,
        object: ObjectId,
        command: String,
        status: String,
    },
    Dialog {
        t: u64,
        speaker: ObjectId,
        text: String,
    },
    /// An effect took health.
    Damage {
        t: u64,
        #[serde(flatten)]
        loss: HealthLoss,
    },
    /// An attack landed or missed: its `COMBAT_RESULT_*` (of the main hand) and the damage
    /// its impact effects carry.
    Attack {
        t: u64,
        attacker: ObjectId,
        target: ObjectId,
        result: i32,
        damage: f32,
    },
}

/// Health an effect took from a creature: its creator, the ability it came from (0 for a
/// weapon attack) and the effect's type. A killing blow counts the health that was left.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct HealthLoss {
    pub source: ObjectId,
    pub target: ObjectId,
    pub ability: i32,
    pub effect: i32,
    pub amount: f32,
}

/// Attacks one creature made: swings, those that hit (hit, critical, deathblow or
/// backstab) and the damage they carried.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AttackTally {
    pub swings: u32,
    pub hits: u32,
    pub damage: f32,
}

/// The trace, plus counters that summarise it.
#[derive(Clone, Debug, Default)]
pub struct Trace {
    pub entries: Vec<TraceEntry>,
    /// Calls per unimplemented engine function.
    pub unimplemented: BTreeMap<String, u64>,
    pub implemented_calls: BTreeMap<String, u64>,
    pub script_errors: BTreeMap<String, u64>,
    pub scripts_run: u64,
    /// Attacks per attacker (kept even when entries are not).
    pub attacks: BTreeMap<ObjectId, AttackTally>,
    /// Health taken, per source and ability (kept even when entries are not).
    pub damage: BTreeMap<(ObjectId, i32), f32>,
    /// Keep full entries (otherwise only the counters are updated).
    pub keep: bool,
}

impl Trace {
    pub fn push(&mut self, e: TraceEntry) {
        match &e {
            TraceEntry::Unimplemented { function, .. } => {
                *self.unimplemented.entry(function.clone()).or_default() += 1;
            }
            TraceEntry::ScriptStart { .. } => self.scripts_run += 1,
            TraceEntry::Damage { loss, .. } => {
                *self.damage.entry((loss.source, loss.ability)).or_default() += loss.amount;
            }
            TraceEntry::Attack {
                attacker,
                result,
                damage,
                ..
            } => {
                let a = self.attacks.entry(*attacker).or_default();
                a.swings += 1;
                // COMBAT_RESULT_HIT, _CRITICALHIT, _DEATHBLOW, _BACKSTAB.
                if matches!(result, 1 | 2 | 4 | 9) {
                    a.hits += 1;
                    a.damage += damage;
                }
            }
            TraceEntry::ScriptEnd {
                error: Some(err), ..
            } => {
                let key = err.split(" at ").next().unwrap_or(err).to_string();
                *self.script_errors.entry(key).or_default() += 1;
            }
            _ => {}
        }
        if self.keep {
            self.entries.push(e);
        }
    }

    /// The trace as JSON lines.
    pub fn to_jsonl(&self) -> String {
        let mut out = String::new();
        for e in &self.entries {
            if let Ok(s) = serde_json::to_string(e) {
                out.push_str(&s);
                out.push('\n');
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attacks_are_tallied_by_attacker_even_without_entries() {
        let mut t = Trace::default();
        for (result, damage) in [(1, 9.0), (3, 0.0), (2, 14.5), (5, 0.0)] {
            t.push(TraceEntry::Attack {
                t: 0,
                attacker: 7,
                target: 8,
                result,
                damage,
            });
        }
        assert!(t.entries.is_empty());
        assert_eq!(
            t.attacks.get(&7),
            Some(&AttackTally {
                swings: 4,
                hits: 2,
                damage: 23.5
            })
        );
    }
}
