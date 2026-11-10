//! The world: objects, engine values, time and the event queue.
//!
//! Everything here is deterministic: time is an integer number of milliseconds that only
//! advances when the caller ticks the world, events are ordered by (due time, sequence
//! number), object ids are handed out sequentially, and randomness comes from a seeded
//! generator.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use serde::Serialize;
use thedas_script::vm::{OBJECT_INVALID, ObjectId};

use crate::trace::{Trace, TraceEntry};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub enum ObjectKind {
    Module,
    Area,
    Creature,
    Placeable,
    Trigger,
    Waypoint,
    Store,
    Sound,
    Item,
    /// Stage (a cutscene/conversation staging point).
    Stage,
    Party,
}

impl ObjectKind {
    /// The `OBJECT_TYPE_*` value scripts compare against (from `script.ldf`).
    pub fn script_type(self) -> i32 {
        match self {
            ObjectKind::Module => 4,
            ObjectKind::Area => 8,
            ObjectKind::Store => 16,
            ObjectKind::Creature => 32,
            ObjectKind::Item => 64,
            ObjectKind::Trigger => 128,
            ObjectKind::Placeable => 512,
            ObjectKind::Waypoint => 4096,
            ObjectKind::Sound => 16384,
            ObjectKind::Party => 32768,
            ObjectKind::Stage => 0,
        }
    }
}

/// Local variables (`VarTable` in templates and instances, `SetLocal*` from scripts).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Locals {
    pub ints: BTreeMap<String, i32>,
    pub floats: BTreeMap<String, f32>,
    pub strings: BTreeMap<String, String>,
    pub objects: BTreeMap<String, ObjectId>,
    pub resources: BTreeMap<String, String>,
    pub locations: BTreeMap<String, Location>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Location {
    pub area: ObjectId,
    pub position: [f32; 3],
    /// Degrees.
    pub facing: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Object {
    pub id: ObjectId,
    pub kind: ObjectKind,
    pub tag: String,
    /// Template resource (without extension).
    pub template: String,
    pub name_strref: Option<u32>,
    pub area: Option<ObjectId>,
    pub position: [f32; 3],
    /// Degrees (provisional convention: derived from the instance quaternion's rotation
    /// about Z).
    pub facing: f32,
    pub active: bool,
    pub locals: Locals,
    /// The script that receives this object's events.
    pub script: Option<String>,
    /// Conversation (`.dlg`) for creatures and placeables.
    pub dialog: Option<String>,
    /// Creature properties (`properties.gda` ids): base and current value.
    pub properties: BTreeMap<i32, (f32, f32)>,
    pub dead: bool,
    pub immortal: bool,
    pub plot: bool,
    pub team: i32,
    pub group: i32,
    pub appearance: i32,
    pub rank: i32,
    pub interactive: bool,
    /// Trigger polygon (XY used for containment).
    pub geometry: Vec<[f32; 3]>,
    /// Pending commands (front first).
    pub commands: Vec<Command>,
    pub followers: Vec<ObjectId>,
    pub effects: Vec<Effect>,
    pub abilities: std::collections::BTreeSet<i32>,
    pub money: i32,
    /// Items carried (object ids of `Item` objects).
    pub inventory: Vec<ObjectId>,
    /// Integer fields of the template and instance (`Package`, `Race`, `Combatant`, ...),
    /// for engine functions that read them.
    pub fields: BTreeMap<String, i32>,
}

impl Object {
    pub fn new(id: ObjectId, kind: ObjectKind, tag: &str) -> Self {
        Object {
            id,
            kind,
            tag: tag.to_string(),
            template: String::new(),
            name_strref: None,
            area: None,
            position: [0.0; 3],
            facing: 0.0,
            active: true,
            locals: Locals::default(),
            script: None,
            dialog: None,
            properties: BTreeMap::new(),
            dead: false,
            immortal: false,
            plot: false,
            team: -1,
            group: 0,
            appearance: 0,
            rank: 0,
            interactive: true,
            geometry: Vec::new(),
            commands: Vec::new(),
            followers: Vec::new(),
            effects: Vec::new(),
            abilities: std::collections::BTreeSet::new(),
            money: 0,
            inventory: Vec::new(),
            fields: BTreeMap::new(),
        }
    }
}

/// An event as scripts see it: a type plus parameter lists.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Event {
    pub ty: i32,
    pub creator: Option<ObjectId>,
    pub target: Option<ObjectId>,
    pub ints: Vec<i32>,
    pub floats: Vec<f32>,
    pub strings: Vec<String>,
    pub objects: Vec<ObjectId>,
    pub resources: Vec<String>,
    pub locations: Vec<Location>,
}

/// A queued command (`AddCommand`). Provisional: commands complete as soon as the executor
/// reaches them, except `Wait`, which lasts its duration.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Command {
    /// `COMMAND_TYPE_*`.
    pub ty: i32,
    pub name: String,
    pub target: Option<ObjectId>,
    pub location: Option<Location>,
    pub seconds: f32,
    pub ints: Vec<i32>,
    pub strings: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Effect {
    pub ty: i32,
    pub creator: Option<ObjectId>,
    pub ability: i32,
    pub ints: Vec<i32>,
    pub floats: Vec<f32>,
    pub objects: Vec<ObjectId>,
    pub strings: Vec<String>,
    pub duration_type: i32,
    pub id: i32,
}

/// Engine structures held by handle (scripts copy handles; every `Set*` makes a new one).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum EngineData {
    Event(Event),
    Location(Location),
    Command(Command),
    Effect(Effect),
}

#[derive(Clone, Debug)]
pub struct QueuedEvent {
    pub target: ObjectId,
    pub event: Event,
    /// Run this script instead of the target's own (`DelayEvent` with a script name).
    pub script: Option<String>,
}

/// A small, fast, deterministic generator (xorshift64*).
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed.max(1))
    }
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    /// Uniform in `0..n` (0 when `n <= 0`).
    pub fn below(&mut self, n: i32) -> i32 {
        if n <= 0 {
            0
        } else {
            (self.next_u64() % n as u64) as i32
        }
    }
    pub fn unit(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }
}

#[derive(Clone)]
pub struct World {
    pub time_ms: u64,
    pub objects: BTreeMap<ObjectId, Object>,
    next_id: ObjectId,
    pub handles: HashMap<u64, EngineData>,
    next_handle: u64,
    pub queue: BTreeMap<(u64, u64), QueuedEvent>,
    next_seq: u64,
    pub module: Option<ObjectId>,
    pub area: Option<ObjectId>,
    pub hero: Option<ObjectId>,
    pub party: Option<ObjectId>,
    /// Plot flags by (plot resource, flag).
    pub plots: BTreeMap<(String, i32), bool>,
    pub game_mode: i32,
    pub rng: Rng,
    pub trace: Trace,
    pub(crate) tables: Arc<crate::m2da::Tables>,
}

impl World {
    pub fn new(tables: Arc<crate::m2da::Tables>, seed: u64) -> Self {
        World {
            time_ms: 0,
            objects: BTreeMap::new(),
            next_id: 1,
            handles: HashMap::new(),
            next_handle: 1,
            queue: BTreeMap::new(),
            next_seq: 0,
            module: None,
            area: None,
            hero: None,
            party: None,
            plots: BTreeMap::new(),
            game_mode: 0,
            rng: Rng::new(seed),
            trace: Trace::default(),
            tables,
        }
    }

    pub fn spawn(&mut self, kind: ObjectKind, tag: &str) -> ObjectId {
        let id = self.next_id;
        self.next_id += 1;
        self.objects.insert(id, Object::new(id, kind, tag));
        id
    }

    pub fn get(&self, id: ObjectId) -> Option<&Object> {
        if id == OBJECT_INVALID {
            return None;
        }
        self.objects.get(&id)
    }

    pub fn get_mut(&mut self, id: ObjectId) -> Option<&mut Object> {
        if id == OBJECT_INVALID {
            return None;
        }
        self.objects.get_mut(&id)
    }

    pub fn store(&mut self, d: EngineData) -> u64 {
        let h = self.next_handle;
        self.next_handle += 1;
        self.handles.insert(h, d);
        h
    }

    pub fn handle(&self, h: Option<u64>) -> Option<&EngineData> {
        self.handles.get(&h?)
    }

    /// Objects with a tag in id order (the `nth` of `GetObjectByTag`).
    pub fn by_tag(&self, tag: &str) -> impl Iterator<Item = &Object> {
        let tag = tag.to_ascii_lowercase();
        self.objects
            .values()
            .filter(move |o| o.tag.to_ascii_lowercase() == tag)
    }

    /// Queues an event `delay_ms` from now.
    pub fn queue_event(
        &mut self,
        delay_ms: u64,
        target: ObjectId,
        event: Event,
        script: Option<String>,
    ) {
        let seq = self.next_seq;
        self.next_seq += 1;
        self.trace.push(TraceEntry::EventQueued {
            t: self.time_ms,
            due: self.time_ms + delay_ms,
            target,
            ty: event.ty,
        });
        self.queue.insert(
            (self.time_ms + delay_ms, seq),
            QueuedEvent {
                target,
                event,
                script,
            },
        );
    }

    /// Removes and returns the next event due at or before `until`, with its due time.
    pub fn pop_due(&mut self, until: u64) -> Option<(u64, QueuedEvent)> {
        let key = *self.queue.keys().next()?;
        if key.0 > until {
            return None;
        }
        self.queue.remove(&key).map(|q| (key.0, q))
    }
}
