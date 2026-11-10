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
