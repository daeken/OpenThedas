//! The world: objects, engine values, time and the event queue.
//!
//! Everything here is deterministic: time is an integer number of milliseconds that only
//! advances when the caller ticks the world, events are ordered by (due time, sequence
//! number), object ids are handed out sequentially, and randomness comes from a seeded
//! generator.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use thedas_script::vm::{OBJECT_INVALID, ObjectId};

use crate::trace::{Trace, TraceEntry};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
    /// A world map (`.map`, e.g. `wide_open_world`).
    Map,
    /// A location pin on a world map (`wml_wow_lothering`).
    MapPin,
    /// A projectile in flight (`FireProjectile`).
    Projectile,
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
            ObjectKind::Map => 262144,
            // Provisional: pins are taken to be the map patches.
            ObjectKind::MapPin => 65536,
            ObjectKind::Projectile => 256,
        }
    }
}

/// Local variables (`VarTable` in templates and instances, `SetLocal*` from scripts).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Locals {
    pub ints: BTreeMap<String, i32>,
    pub floats: BTreeMap<String, f32>,
    pub strings: BTreeMap<String, String>,
    pub objects: BTreeMap<String, ObjectId>,
    pub resources: BTreeMap<String, String>,
    pub locations: BTreeMap<String, Location>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Location {
    pub area: ObjectId,
    pub position: [f32; 3],
    /// Degrees.
    pub facing: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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
    /// Non-party followers (`AddNonPartyFollower`): they follow but are not in the party.
    #[serde(default)]
    pub escorts: Vec<ObjectId>,
    /// Inventories stored on this container (`StoreFollowerInventory`): by owner, each item
    /// with the equipment slot it was in (-1 for none).
    #[serde(default)]
    pub stored: BTreeMap<ObjectId, Vec<(ObjectId, i32)>>,
    /// Whether the object was active when the party left its area (the area is not
    /// loaded meanwhile); restored when the party comes back.
    #[serde(default)]
    pub parked: Option<bool>,
    pub effects: Vec<Effect>,
    /// Combat bookkeeping (perception, threat, the pending attack); see `combat`.
    #[serde(default)]
    pub combat: crate::combat::CombatData,
    pub abilities: std::collections::BTreeSet<i32>,
    pub money: i32,
    /// Items carried (object ids of `Item` objects).
    pub inventory: Vec<ObjectId>,
    /// Equipped items by `INVENTORY_SLOT_*`. Equipped items stay in the inventory.
    #[serde(default)]
    pub equipment: BTreeMap<i32, ObjectId>,
    /// The inactive weapon set's weapons (main hand, off hand, ammunition), swapped with
    /// `equipment`'s by `SwitchWeaponSet`. The active set is field `ActiveWeaponSet`.
    #[serde(default)]
    pub other_set: BTreeMap<i32, ObjectId>,
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
            escorts: Vec::new(),
            stored: BTreeMap::new(),
            parked: None,
            effects: Vec::new(),
            combat: Default::default(),
            abilities: std::collections::BTreeSet::new(),
            money: 0,
            inventory: Vec::new(),
            equipment: BTreeMap::new(),
            other_set: BTreeMap::new(),
            fields: BTreeMap::new(),
        }
    }
}

/// An event as scripts see it: a type plus parameter lists.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
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
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Command {
    /// `COMMAND_TYPE_*`.
    pub ty: i32,
    pub name: String,
    pub target: Option<ObjectId>,
    pub location: Option<Location>,
    pub seconds: f32,
    pub ints: Vec<i32>,
    pub strings: Vec<String>,
    /// When the command finishes, set when it reaches the front of the queue.
    #[serde(default)]
    pub due: Option<u64>,
    /// For a move: where the mover was and the time when it set off. Its position in
    /// between is interpolated (see `Sim::advance_movers`).
    #[serde(default)]
    pub from: Option<([f32; 3], u64)>,
    /// For an attack whose target is out of reach: where the attacker is walking to first
    /// (see `Sim::attack_reach`).
    #[serde(default)]
    pub approach: Option<[f32; 3]>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
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
    /// When a temporary effect runs out (simulation milliseconds).
    #[serde(default)]
    pub expires: Option<u64>,
}

/// Engine structures held by handle (scripts copy handles; every `Set*` makes a new one).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum EngineData {
    Event(Event),
    Location(Location),
    Command(Command),
    Effect(Effect),
}

/// Inventory slots that belong to a weapon set (main hand, off hand, ranged ammunition).
pub const WEAPON_SET_SLOTS: [i32; 3] = [0, 1, 2];

/// The plot actions control: the set shown (0 hides it), whether it is enabled, and each
/// action's state (0 invalid, 1 enabled, 2 used, 3 active) and count.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PlotActions {
    pub set: i32,
    pub disabled: bool,
    pub states: std::collections::BTreeMap<i32, i32>,
    pub counts: std::collections::BTreeMap<i32, i32>,
}

/// A message box shown by `ShowPopup`: its text, kind (`nPopupType`), owner and whether it
/// asks for typed input.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Popup {
    pub strref: i32,
    pub kind: i32,
    pub owner: ObjectId,
    pub input: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QueuedEvent {
    pub target: ObjectId,
    pub event: Event,
    /// Run this script instead of the target's own (`DelayEvent` with a script name).
    pub script: Option<String>,
}

/// A small, fast, deterministic generator (xorshift64*).
#[derive(Clone, Debug, Serialize, Deserialize)]
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

/// Everything a save holds. The trace and the tables are not saved: the trace starts afresh
/// and the tables come from the resources of the simulation that loads the save.
#[derive(Clone, Serialize, Deserialize)]
pub struct World {
    pub time_ms: u64,
    pub objects: BTreeMap<ObjectId, Object>,
    next_id: ObjectId,
    #[serde(with = "sorted")]
    pub handles: HashMap<u64, EngineData>,
    next_handle: u64,
    #[serde(with = "pairs")]
    pub queue: BTreeMap<(u64, u64), QueuedEvent>,
    next_seq: u64,
    pub module: Option<ObjectId>,
    pub area: Option<ObjectId>,
    pub hero: Option<ObjectId>,
    pub party: Option<ObjectId>,
    /// Plot flags by (plot resource, flag).
    #[serde(with = "pairs")]
    pub plots: BTreeMap<(String, i32), bool>,
    pub game_mode: i32,
    /// Cutscenes loaded with `LoadCutscene` that have not "played" yet. There is no cutscene
    /// playback; a loaded cutscene ends on the next tick.
    #[serde(default)]
    pub cutscenes: Vec<String>,
    /// The creature in character generation, between `StartCharGen` and `CHARGEN_END`.
    #[serde(default)]
    pub chargen: Option<ObjectId>,
    /// Whether the party was fighting at the last perception update.
    #[serde(default)]
    pub party_fighting: bool,
    /// Nobody is under the player's direct control: the party AI (tactics) drives every
    /// party member, the hero included, as it drives followers the player is not
    /// controlling. For playing without a player.
    #[serde(default)]
    pub autopilot: bool,
    /// Objects to remove at a time (projectiles when they land).
    #[serde(default)]
    pub destroy_at: Vec<(u64, ObjectId)>,
    /// Events for objects in an unloaded area, waiting for the party to come back.
    #[serde(default)]
    pub held: Vec<QueuedEvent>,
    /// Show-once-per-game conversation nodes already used ("dialog:node").
    #[serde(default)]
    pub dialog_shown: std::collections::BTreeSet<String>,
    /// A message box waiting for an answer (`ShowPopup`).
    #[serde(default)]
    pub popup: Option<Popup>,
    /// The followers the player wants when the party picker opens.
    #[serde(default)]
    pub party_choice: Option<Vec<ObjectId>>,
    /// The plot actions control (`SetPlotActionSet` and friends; see `plotactions.gda`).
    #[serde(default)]
    pub plot_actions: PlotActions,
    /// The job board on screen (`ShowChantersGUI`).
    #[serde(default)]
    pub chanter_board: Option<crate::chanters::ChanterBoard>,
    /// The store whose screen is open (`OpenStore`).
    #[serde(default)]
    pub store: Option<ObjectId>,
    /// The world map (see `worldmap`).
    #[serde(default)]
    pub world_map: crate::worldmap::WorldMapState,
    /// Group pairs set hostile with `SetGroupHostility`.
    #[serde(default, with = "pairs")]
    pub hostility: BTreeMap<(i32, i32), bool>,
    /// Triggers each creature is standing in, for ENTER/EXIT events.
    #[serde(default)]
    pub in_triggers: BTreeMap<ObjectId, Vec<ObjectId>>,
    /// A `DoAreaTransition` waiting for the current scripts to finish (area, waypoint).
    #[serde(default)]
    pub transition: Option<(String, String)>,
    pub rng: Rng,
    #[serde(skip)]
    pub trace: Trace,
    #[serde(skip, default = "no_tables")]
    pub(crate) tables: Arc<crate::m2da::Tables>,
}

fn no_tables() -> Arc<crate::m2da::Tables> {
    Arc::new(crate::m2da::Tables::empty())
}

/// Maps with tuple keys, as a list of pairs (JSON keys must be strings).
mod pairs {
    use std::collections::BTreeMap;

    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<K: Serialize, V: Serialize, S: Serializer>(
        m: &BTreeMap<K, V>,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        s.collect_seq(m.iter())
    }

    pub fn deserialize<'de, K, V, D>(d: D) -> Result<BTreeMap<K, V>, D::Error>
    where
        K: Deserialize<'de> + Ord,
        V: Deserialize<'de>,
        D: Deserializer<'de>,
    {
        Ok(Vec::<(K, V)>::deserialize(d)?.into_iter().collect())
    }
}

/// Hash maps in key order, so saves are byte-for-byte deterministic.
mod sorted {
    use std::collections::{BTreeMap, HashMap};
    use std::hash::Hash;

    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<K: Serialize + Ord, V: Serialize, S: Serializer>(
        m: &HashMap<K, V>,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        m.iter().collect::<BTreeMap<_, _>>().serialize(s)
    }

    pub fn deserialize<'de, K, V, D>(d: D) -> Result<HashMap<K, V>, D::Error>
    where
        K: Deserialize<'de> + Eq + Hash + Ord,
        V: Deserialize<'de>,
        D: Deserializer<'de>,
    {
        Ok(BTreeMap::<K, V>::deserialize(d)?.into_iter().collect())
    }
}

impl World {
    /// The merged 2DA tables (`GetM2DA*`).
    pub fn tables(&self) -> Arc<crate::m2da::Tables> {
        self.tables.clone()
    }

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
            cutscenes: Vec::new(),
            chargen: None,
            transition: None,
            in_triggers: BTreeMap::new(),
            hostility: BTreeMap::new(),
            party_fighting: false,
            autopilot: false,
            destroy_at: Vec::new(),
            held: Vec::new(),
            dialog_shown: Default::default(),
            popup: None,
            store: None,
            chanter_board: None,
            plot_actions: PlotActions::default(),
            party_choice: None,
            world_map: Default::default(),
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
    /// The `n`th object with a tag, those in the current area first (only it is loaded in
    /// the game; another area of the same list may hold its own copy of a creature), then
    /// area-less objects (module, areas), then the rest. Provisional.
    pub fn find_tag(&self, tag: &str, n: usize) -> Option<ObjectId> {
        if tag.is_empty() {
            return None;
        }
        let mut found: Vec<&Object> = self.by_tag(tag).collect();
        found.sort_by_key(|o| match o.area {
            x if x.is_some() && x == self.area => 0,
            None => 1,
            _ => 2,
        });
        found.get(n).map(|o| o.id)
    }

    pub fn by_tag(&self, tag: &str) -> impl Iterator<Item = &Object> {
        let tag = tag.to_ascii_lowercase();
        self.objects
            .values()
            .filter(move |o| o.tag.to_ascii_lowercase() == tag)
    }

    /// Removes an object `delay_ms` from now.
    pub fn queue_destroy(&mut self, delay_ms: u64, id: ObjectId) {
        self.destroy_at.push((self.time_ms + delay_ms, id));
    }

    /// Removes the objects whose time is up.
    pub fn destroy_due(&mut self) {
        let now = self.time_ms;
        let (due, keep): (Vec<_>, Vec<_>) = self.destroy_at.drain(..).partition(|(t, _)| *t <= now);
        self.destroy_at = keep;
        for (_, id) in due {
            if Some(id) != self.hero {
                self.objects.remove(&id);
            }
        }
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
