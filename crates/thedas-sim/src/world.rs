//! The world: objects, engine values, time and the event queue.
//!
//! Everything here is deterministic: time is an integer number of milliseconds that only
//! advances when the caller ticks the world, events are ordered by (due time, sequence
//! number), object ids are handed out sequentially, and randomness comes from a seeded
//! generator.

use std::collections::{BTreeMap, BTreeSet, HashMap};
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
    /// A lasting area of effect (see [`crate::aoe`]).
    AreaOfEffect,
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
            ObjectKind::AreaOfEffect => 2048,
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

/// The direction a facing looks, in degrees anticlockwise from +X. A facing is a rotation
/// about Z (`GetOrientationFromLocation` returns it as the orientation's z) and models face
/// +Y in their own space (the mabari's eyes and every humanoid's toes are at +Y), so a
/// facing of 0 looks along +Y.
pub fn heading_of(facing: f32) -> f32 {
    facing + 90.0
}

/// The facing that looks along a direction (dx, dy).
pub fn facing_toward(dx: f32, dy: f32) -> f32 {
    (dy.atan2(dx).to_degrees() - 90.0).rem_euclid(360.0)
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Object {
    pub id: ObjectId,
    pub kind: ObjectKind,
    pub tag: String,
    /// Template resource (without extension).
    pub template: String,
    pub name_strref: Option<u32>,
    /// A name given in play (`SetName`: the hero's, chosen in character creation), which
    /// takes the place of the template's.
    #[serde(default)]
    pub name: Option<String>,
    pub area: Option<ObjectId>,
    pub position: [f32; 3],
    /// Degrees: the object's rotation about Z, as the game keeps it (an instance's
    /// quaternion, a location's orientation z). Models face +Y, so facing 0 looks along +Y;
    /// see [`heading_of`] and [`facing_toward`].
    pub facing: f32,
    pub active: bool,
    pub locals: Locals,
    /// The script that receives this object's events.
    pub script: Option<String>,
    /// Conversation (`.dlg`) for creatures and placeables.
    pub dialog: Option<String>,
    /// Creature properties (`properties.gda` ids): base and current value.
    pub properties: BTreeMap<i32, (f32, f32)>,
    /// What effects add to properties (`ApplyEffectModifyProperty`, from buffs and worn
    /// items), apart from the base so that scripts setting a base keep them: the
    /// `PROPERTY_VALUE_MODIFIER` part of a total.
    #[serde(default)]
    pub modifiers: BTreeMap<i32, f32>,
    /// The quickbar's layout: what each slot holds (an ability, and the item it comes
    /// from), as scripts (`SetQuickslot`) or the player put it there or the engine placed it
    /// (see [`crate::quickbar`]). Saved, so the bar keeps its layout.
    #[serde(default)]
    pub quickslots: BTreeMap<i32, Quickslot>,
    /// What has been on the quickbar ([`crate::quickbar::quick_key`]): the engine places a
    /// usable ability or quick item once, so one the player takes off stays off.
    #[serde(default)]
    pub quick_placed: std::collections::BTreeSet<String>,
    pub dead: bool,
    /// When the creature last died (simulation milliseconds); `None` while alive. For
    /// presentation (the death animation plays from it) and saves.
    #[serde(default)]
    pub died_at: Option<u64>,
    /// A head morph kept on the creature itself (`.mor` resource, no extension), as
    /// character creation leaves the player's face; others take theirs from their
    /// template's `Appearance Data`. Saved.
    #[serde(default)]
    pub head_morph: Option<String>,
    /// Character creation's appearance sliders as the player left them (slider id, see
    /// [`crate::appearance`], to value). Saved, as the face they make is the player's.
    #[serde(default)]
    pub face_choices: BTreeMap<u32, f32>,
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
    /// String fields of the template and instance that engine functions read (a lock's
    /// `KeyName`).
    #[serde(default)]
    pub text_fields: BTreeMap<String, String>,
    /// What an area-of-effect object keeps (see [`crate::aoe`]).
    #[serde(default)]
    pub aoe: Option<crate::aoe::AoeState>,
    /// An item's own property list (`itemprps.gda` row and power), once a script has
    /// changed it (`AddItemProperty`, `RemoveItemProperty`); until then its template's.
    #[serde(default)]
    pub item_properties: Option<Vec<(i32, i32)>>,
}

impl Object {
    pub fn new(id: ObjectId, kind: ObjectKind, tag: &str) -> Self {
        Object {
            id,
            kind,
            tag: tag.to_string(),
            template: String::new(),
            name_strref: None,
            name: None,
            area: None,
            position: [0.0; 3],
            facing: 0.0,
            active: true,
            locals: Locals::default(),
            script: None,
            dialog: None,
            properties: BTreeMap::new(),
            modifiers: BTreeMap::new(),
            quickslots: BTreeMap::new(),
            quick_placed: Default::default(),
            dead: false,
            died_at: None,
            head_morph: None,
            face_choices: BTreeMap::new(),
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
            text_fields: BTreeMap::new(),
            aoe: None,
            item_properties: None,
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
    /// For a melee attack: it found the target's melee ring full once and is waiting by it
    /// (see `Sim::approach_point`).
    #[serde(default)]
    pub ring_wait: bool,
    /// For a move or an approach under way: the route's waypoints after `from`, ending at
    /// the destination (empty: a straight line). See `Sim::route_to`.
    #[serde(default)]
    pub path: Vec<[f32; 3]>,
    /// For an ability whose impact comes before the end of its animation (see
    /// `Sim::ability_timing`): what is left of the command once the impact is due.
    #[serde(default)]
    pub after_impact_ms: Option<u64>,
    /// The ability's impact has been dealt; the command runs on to its end.
    #[serde(default)]
    pub impacted: bool,
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
    /// Made by a hostile constructor (`EffectModifyPropertyHostile`): a debuff.
    #[serde(default)]
    pub hostile: bool,
    /// The engine data integers scripts set (`SetEffectEngineInteger`; write-only for
    /// scripts), by index.
    #[serde(default)]
    pub engine_ints: Vec<(i32, i32)>,
    /// The engine data vectors scripts set (`SetEffectEngineVector`), by index.
    #[serde(default)]
    pub engine_vectors: Vec<(i32, [f32; 3])>,
}

/// The open party picker: the followers picked so far (at most three), in order.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PartyPicker {
    pub selected: Vec<ObjectId>,
}

/// A quickbar slot: an ability, and the tag of the item it comes from (a potion), if any.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Quickslot {
    pub ability: i32,
    pub item: String,
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

/// A floating message over a creature (`DisplayFloatyMessage(oCreature, sMessage, nStyle,
/// nColor, fDuration)`), as DAOrigins.exe hands it to floatylayer.gfx.
#[derive(Clone, Debug, PartialEq)]
pub struct FloatyMessage {
    pub target: ObjectId,
    pub text: String,
    pub style: i32,
    /// 0xRRGGBB.
    pub colour: i32,
    /// Seconds.
    pub duration: f32,
}

/// A sound a script asked for (`PlaySound`, `PlaySoundSet`, `PlaySoundObject`, `PlayMusic`)
/// or a blow made, for the presentation to play. Output only. (`SetMusicIntensity` is kept
/// in [`World::music_intensity`].)
#[derive(Clone, Debug, PartialEq)]
pub enum SoundRequest {
    /// `PlaySound(oTarget, sSoundEventName)`: an FMOD event at the object.
    Event { target: ObjectId, event: String },
    /// `PlaySoundSet(oTarget, nSoundSetEntry, fProbabilityOverride)`: an entry of the
    /// creature's sound set.
    SoundSet {
        target: ObjectId,
        entry: i32,
        probability: f32,
    },
    /// `PlaySoundObject(oSound)`: a placed sound object plays.
    Object { sound: ObjectId },
    /// `PlayMusic(sMusicName)`.
    Music { name: String },
    /// An attack's blow landed or missed (one hand's impact): the attacker, the target,
    /// the `COMBAT_RESULT_*`, and the base items (`BaseItem`) of the attacker's weapon in
    /// that hand and of the target's armour (body slot) and off-hand item (a shield).
    Impact {
        attacker: ObjectId,
        target: ObjectId,
        result: i32,
        weapon: Option<i32>,
        armour: Option<i32>,
        shield: Option<i32>,
    },
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
    /// The party member the player controls, when not the hero (see
    /// [`World::controlled`]). Saved.
    #[serde(default)]
    pub controlled: Option<ObjectId>,
    pub party: Option<ObjectId>,
    /// Plot flags by (plot resource, flag).
    #[serde(with = "pairs")]
    pub plots: BTreeMap<(String, i32), bool>,
    /// Each quest's latest journal entry, by plot GUID (see [`crate::journal`]).
    #[serde(default)]
    pub journal: BTreeMap<String, crate::journal::JournalMark>,
    /// How many journal entries have been made (orders them).
    #[serde(default)]
    pub journal_seq: u64,
    /// The crafting recipes the party knows (`CraftingRecipeID`s): a recipe item the party
    /// receives is learned and gone (see [`crate::sim::Sim::give_item`]).
    #[serde(default)]
    pub recipes: BTreeSet<i32>,
    /// The quest the player marked active in the journal (its plot resource).
    #[serde(default)]
    pub active_quest: Option<String>,
    /// The tutorial popups waiting to show, the first on screen (see
    /// [`crate::journal::Tutorial`]).
    #[serde(default)]
    pub tutorials: Vec<crate::journal::Tutorial>,
    /// Whether a tutorial paused the game: it stays paused until the queue is empty.
    #[serde(default)]
    pub tutorial_paused: bool,
    /// The HUD's notifications not yet dismissed (see [`crate::journal::Alert`]).
    #[serde(default)]
    pub alerts: Vec<crate::journal::Alert>,
    pub game_mode: i32,
    /// Cutscenes loaded with `LoadCutscene` that have not played yet, in order. Each starts
    /// once the scripts that loaded it have run (see [`crate::sim::CutscenePolicy`]).
    #[serde(default)]
    pub cutscenes: Vec<String>,
    /// Those of [`World::cutscenes`] a conversation line queued (resource names): they end
    /// without the scripts' cutscene-ending locals (see [`crate::sim::Sim::end_cutscene`]).
    #[serde(default)]
    pub dialog_cutscenes: Vec<String>,
    /// The cutscene playing, while a presentation shows it ([`crate::sim::CutscenePolicy::Play`]).
    #[serde(default)]
    pub cutscene: Option<crate::cutscene::Cutscene>,
    /// The creature in character generation, between `StartCharGen` and `CHARGEN_END`.
    #[serde(default)]
    pub chargen: Option<ObjectId>,
    /// `StartCharGen`'s mode for [`World::chargen`]: 0 character creation (chargen.gfx), 1
    /// level-up (levelup.gfx).
    #[serde(default)]
    pub chargen_mode: i32,
    /// Whether the party was fighting at the last perception update.
    #[serde(default)]
    pub party_fighting: bool,
    /// The whole party fell: the original shows its death screen and the game ends there.
    #[serde(default)]
    pub game_over: bool,
    /// The music's intensity, as scripts last set it (`SetMusicIntensity`; 1 until then).
    /// Only the presentation reads it; it is saved so a loaded game sounds as it did.
    /// Provisional: whether the original keeps it in its saves is not traced.
    #[serde(default = "music_intensity_default")]
    pub music_intensity: i32,
    /// The game difficulty (`difficulty.gda`'s rows: 0 casual, 1 normal, 2 hard, 3
    /// nightmare), as the difficulty option sets it and `GetGameDifficulty` reads it.
    #[serde(default = "normal_difficulty")]
    pub difficulty: i32,
    /// Nobody is under the player's direct control: the party AI (tactics) drives every
    /// party member, the hero included, as it drives followers the player is not
    /// controlling. For playing without a player.
    #[serde(default)]
    pub autopilot: bool,
    /// Followers waiting in the party pool get the experience the hero gets (see
    /// `Sim::share_pool_xp`). Off by default: provisional, no script does it.
    #[serde(default)]
    pub pool_xp: bool,
    /// Objects to remove at a time (projectiles when they land).
    #[serde(default)]
    pub destroy_at: Vec<(u64, ObjectId)>,
    /// Events for objects in an unloaded area, waiting for the party to come back.
    #[serde(default)]
    pub held: Vec<QueuedEvent>,
    /// Show-once-per-game conversation nodes already used ("dialog:node").
    #[serde(default)]
    pub dialog_shown: std::collections::BTreeSet<String>,
    /// The hero's "Story So Far" (`LogStoryEvent`): event ids, in order, with the game time
    /// each was logged at (ms).
    #[serde(default)]
    pub story_events: Vec<(i32, u64)>,
    /// A message box waiting for an answer (`ShowPopup`).
    #[serde(default)]
    pub popup: Option<Popup>,
    /// The followers the player wants when the party picker opens.
    #[serde(default)]
    pub party_choice: Option<Vec<ObjectId>>,
    /// The party picker waiting for the player (see [`crate::sim::PartyPickerPolicy`]).
    #[serde(default)]
    pub party_picker: Option<PartyPicker>,
    /// The plot actions control (`SetPlotActionSet` and friends; see `plotactions.gda`).
    #[serde(default)]
    pub plot_actions: PlotActions,
    /// The job board on screen (`ShowChantersGUI`).
    #[serde(default)]
    pub chanter_board: Option<crate::chanters::ChanterBoard>,
    /// The container being looted (`OpenInventory`, see [`crate::container`]).
    #[serde(default)]
    pub container: Option<ObjectId>,
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
    /// The floating messages scripts showed over creatures (`DisplayFloatyMessage`: damage,
    /// "Immune", "Miss", ...) that the interface has not taken yet. Output only: nothing in
    /// the simulation reads it.
    #[serde(skip)]
    pub floaties: Vec<FloatyMessage>,
    /// The sounds scripts asked for that the presentation has not taken yet. Output only:
    /// nothing in the simulation reads it.
    #[serde(skip)]
    pub sounds: Vec<SoundRequest>,
    #[serde(skip, default = "no_tables")]
    pub(crate) tables: Arc<crate::m2da::Tables>,
}

fn music_intensity_default() -> i32 {
    1
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
            controlled: None,
            party: None,
            plots: BTreeMap::new(),
            journal: BTreeMap::new(),
            journal_seq: 0,
            recipes: BTreeSet::new(),
            active_quest: None,
            tutorials: Vec::new(),
            tutorial_paused: false,
            alerts: Vec::new(),
            game_mode: 0,
            cutscenes: Vec::new(),
            dialog_cutscenes: Vec::new(),
            cutscene: None,
            chargen: None,
            chargen_mode: 0,
            transition: None,
            in_triggers: BTreeMap::new(),
            hostility: BTreeMap::new(),
            party_fighting: false,
            game_over: false,
            music_intensity: 1,
            difficulty: normal_difficulty(),
            autopilot: false,
            pool_xp: false,
            destroy_at: Vec::new(),
            held: Vec::new(),
            dialog_shown: Default::default(),
            story_events: Vec::new(),
            popup: None,
            container: None,
            store: None,
            chanter_board: None,
            plot_actions: PlotActions::default(),
            party_choice: None,
            party_picker: None,
            world_map: Default::default(),
            rng: Rng::new(seed),
            trace: Trace::default(),
            floaties: Vec::new(),
            sounds: Vec::new(),
            tables,
        }
    }

    /// The party member the player controls (`GetMainControlled`): the one chosen, while it
    /// is still in the party and alive enough to exist, else the hero.
    pub fn controlled(&self) -> Option<ObjectId> {
        let hero = self.hero;
        let chosen = self.controlled.filter(|&c| {
            Some(c) == hero
                || hero
                    .and_then(|h| self.get(h))
                    .is_some_and(|h| h.followers.contains(&c))
        });
        chosen.or(hero)
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

/// The difficulty a new game and an older save have: normal.
fn normal_difficulty() -> i32 {
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn facings_are_rotations_with_models_looking_along_plus_y() {
        // Facing 0 looks along +Y; looking along +X is a quarter turn clockwise.
        assert_eq!(heading_of(0.0), 90.0);
        assert!((facing_toward(0.0, 1.0) - 0.0).abs() < 1e-4);
        assert!((facing_toward(1.0, 0.0) - 270.0).abs() < 1e-4);
        assert!((facing_toward(-1.0, 0.0) - 90.0).abs() < 1e-4);
        for f in [0.0f32, 45.0, 92.5, 180.0, 300.0] {
            let h = heading_of(f).to_radians();
            assert!((facing_toward(h.cos(), h.sin()) - f).abs() < 1e-3, "{f}");
        }
    }
}
