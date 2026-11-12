//! The simulation: a [`World`] plus the resources and scripts that drive it.
//!
//! Scripts run on the NWScript VM with [`ScriptHost`] answering engine calls (see
//! [`crate::engine`]). Events are delivered to an object's event script; `DelayEvent` and
//! non-immediate `SignalEvent` queue them; [`Sim::tick`] advances time and drains the queue.
//!
//! Provisional (not yet checked against the original): the order of load events (module
//! `MODULE_LOAD`; then, for an area, `SPAWN` to each creature and placeable,
//! `AREALOAD_SPECIAL`, `AREALOAD_PRELOADEXIT`, `ENTER` for the hero, `AREALOAD_POSTLOADEXIT`),
//! and the command executor, which completes commands as soon as it reaches them (waits last
//! their duration) and then sends `COMMAND_COMPLETE`.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use thedas_formats::gff3::{Gff3, Struct, Value as G3};
use thedas_formats::{Gff4, StructId};
use thedas_script::ldf::{ConstValue, EngineTable, Function};
use thedas_script::vm::{Host, OBJECT_INVALID, ObjectId, Value, VmError, VmResult};
use thedas_script::{Ncs, Vm};
use thedas_vfs::Vfs;

use crate::trace::TraceEntry;
use crate::world::{Command, Event, Location, Object, ObjectKind, World};

#[derive(Debug, thiserror::Error)]
pub enum SimError {
    #[error("{0}: not found")]
    NotFound(String),
    #[error("{0}: {1}")]
    Parse(String, String),
    #[error(transparent)]
    Vfs(#[from] thedas_vfs::Error),
}

/// One area in the module's area tree.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct AreaEntry {
    pub resref: String,
    pub layout: Option<String>,
    pub area_list: Option<String>,
}

#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct ModuleInfo {
    pub name: String,
    pub script: Option<String>,
    pub start_area: Option<String>,
    pub start_position: [f32; 3],
    pub areas: Vec<AreaEntry>,
}

/// How an item's template presents it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ItemLook {
    /// The icon texture (`ico_longsword.dds`).
    pub icon: String,
    pub description: Option<u32>,
    /// The template's item properties (`PropertiesList`): `itemprps.gda` row and power.
    pub properties: Vec<(i32, i32)>,
}

/// Indexes of engine structures in the engine table (`ENGINE_STRUCTURE_n`).
#[derive(Clone, Copy, Debug)]
pub struct EngineTypes {
    pub event: u8,
    pub location: u8,
    pub command: u8,
    pub effect: u8,
}

pub struct Sim {
    pub world: World,
    /// What loading the last save found of older builds' damage (see
    /// [`crate::save::SaveIssue`]).
    pub save_issues: Vec<crate::save::SaveIssue>,
    pub vfs: Arc<Vfs>,
    pub table: Arc<EngineTable>,
    pub module_info: ModuleInfo,
    scripts: HashMap<String, Option<Arc<Ncs>>>,
    pub(crate) types: EngineTypes,
    event_names: HashMap<i32, String>,
    depth: usize,
    /// Instructions a single script run may take.
    pub script_budget: u64,
    pub chargen_policy: ChargenPolicy,
    pub cutscene_policy: CutscenePolicy,
    pub party_picker_policy: PartyPickerPolicy,
    /// The player's "auto-pause when combat starts" option (`GetAutoPauseCombatStatus`);
    /// off, as the game's options start.
    pub auto_pause_combat: bool,
    /// The player's "show tutorials" option: off, no tutorial popup is queued. On, as the
    /// game's options start.
    pub show_tutorials: bool,
    /// The training modes done (bit `mode - 1`), which do not start again (see
    /// [`crate::training`]). The player's, not the game's: not saved.
    pub training_done: u32,
    /// Objects whose scripts' engine calls are all traced (log channel -6), for debugging.
    pub watch_calls: std::collections::BTreeSet<ObjectId>,
    /// Followers whose front command is the move [`Sim::keep_party_together`] gave them.
    following: std::collections::BTreeSet<ObjectId>,
    /// When followers are next looked at (see [`FOLLOW_PERIOD_MS`]).
    next_follow_ms: u64,
    /// Tags (lowercase) whose objects' engine calls are traced too, including objects
    /// created later (an area not yet entered).
    pub watch_tags: std::collections::BTreeSet<String>,
    /// The install root, for files outside the resource stack (layouts' `.arl`).
    pub install_root: Option<std::path::PathBuf>,
    /// Each loaded area's layout name, and the pathing grids loaded per layout.
    pub(crate) area_layouts: HashMap<ObjectId, String>,
    /// The length of each weapon animation tree's fire animation, in milliseconds (see
    /// `Sim::fire_ms`).
    pub(crate) fire_lengths: HashMap<String, Option<u64>>,
    /// Attack animation lengths by combat animation tree (see `Sim::melee_attack_ms`).
    pub(crate) attack_lengths: HashMap<String, Vec<u64>>,
    /// Ability animations by `tree|action` (see `Sim::ability_timing`).
    pub(crate) ability_anims: HashMap<String, Option<crate::combat::AbilityAnimLen>>,
    /// Each area's doors and their doorways' grid cells (doors do not move; see
    /// `Sim::closed_doors`).
    pub(crate) door_cells: HashMap<ObjectId, Arc<Vec<crate::nav::Doorway>>>,
    /// The closed door that stopped the last `walk`, if one did.
    pub walk_blocked_by: Option<ObjectId>,
    pub(crate) nav_grids: HashMap<String, Option<Arc<thedas_formats::level::NavGrid>>>,
    /// Conversations' line staging, read once (see [`crate::cutactors::LineCut`]).
    pub(crate) line_cuts: HashMap<String, Arc<Vec<crate::cutactors::LineCut>>>,
    /// Each layout's collision for sight lines (see `crate::collision`), loaded when first
    /// needed; `None` when the layout's files cannot be found.
    pub(crate) collisions: HashMap<String, Option<Arc<crate::collision::Collision>>>,
    /// Each area's placeables' collision and the key of the placeables it was built from.
    pub(crate) placeable_collisions: HashMap<ObjectId, (u64, Arc<crate::collision::Collision>)>,
    /// Walkers held up by another creature's body: since when, and whether that was
    /// reported (see `Sim::advance_movers`). Not saved.
    pub(crate) blocked_since: HashMap<ObjectId, (u64, bool)>,
    /// The effect an `APPLY_EFFECT` handler is looking at, and whether it is still valid.
    pub(crate) current_effect: Option<(crate::world::Effect, bool)>,
    /// Who the effect in `current_effect` is being applied to.
    pub(crate) current_effect_target: Option<ObjectId>,
    /// Whether the conversation line whose condition or action last ran is ambient
    /// (`DialogIsAmbient`; DAOrigins.exe's server flag at +0x214).
    pub(crate) dialog_ambient: bool,
    pub(crate) talk: Option<crate::dialog::Conversation>,
    pub(crate) plots: crate::plot::Plots,
    /// The talk tables' strings, read once (tests set them).
    pub(crate) strings: std::sync::OnceLock<std::collections::BTreeMap<u32, String>>,
    /// Item templates' presentation (icon, description), by template, read once.
    item_looks: std::sync::Mutex<std::collections::HashMap<String, ItemLook>>,
}

pub const MAX_DEPTH: usize = 24;

/// The length of a route from `from` through `points`.
fn path_length(from: [f32; 3], points: &[[f32; 3]]) -> f32 {
    let mut prev = from;
    let mut len = 0.0;
    for &p in points {
        len += crate::engine::dist(prev, p);
        prev = p;
    }
    len
}

/// The point a fraction `f` (0 to 1) of the way along a route from `from` through `points`.
fn along(from: [f32; 3], points: &[[f32; 3]], f: f32) -> [f32; 3] {
    let total = path_length(from, points);
    let Some(&last) = points.last() else {
        return from;
    };
    if total <= 0.0 || f >= 1.0 {
        return last;
    }
    let mut left = total * f.max(0.0);
    let mut prev = from;
    for &p in points {
        let d = crate::engine::dist(prev, p);
        if left <= d && d > 0.0 {
            let k = left / d;
            return [
                prev[0] + (p[0] - prev[0]) * k,
                prev[1] + (p[1] - prev[1]) * k,
                prev[2] + (p[2] - prev[2]) * k,
            ];
        }
        left -= d;
        prev = p;
    }
    last
}

/// How long a walker held up by bodies may make no progress before it is reported.
const BLOCKED_REPORT_MS: u64 = 3_000;

/// Moving from `at` by `d`, against a circle at `c` of radius `r`: where the move stops on
/// the circle and what is left of it slid along the circle's tangent, or `None` when the
/// move does not run into it. A move out of a circle it starts inside is not stopped; one
/// further in loses its inward part.
fn slide_circle(at: [f32; 2], d: [f32; 2], c: [f32; 2], r: f32) -> Option<([f32; 2], [f32; 2])> {
    let m = [at[0] - c[0], at[1] - c[1]];
    let dd = d[0] * d[0] + d[1] * d[1];
    if dd <= 1e-12 {
        return None;
    }
    let mm = m[0] * m[0] + m[1] * m[1];
    let md = m[0] * d[0] + m[1] * d[1];
    if mm < r * r {
        // Already inside (placed overlapping): only moves that go further in are trimmed.
        if md >= 0.0 || mm <= 1e-12 {
            return None;
        }
        let n = mm.sqrt();
        let nrm = [m[0] / n, m[1] / n];
        let k = d[0] * nrm[0] + d[1] * nrm[1];
        return Some((at, [d[0] - k * nrm[0], d[1] - k * nrm[1]]));
    }
    if md >= 0.0 {
        return None;
    }
    // First t in [0, 1] with |m + t d| = r.
    let disc = md * md - dd * (mm - r * r);
    if disc < 0.0 {
        return None;
    }
    let t = (-md - disc.sqrt()) / dd;
    if !(0.0..=1.0).contains(&t) {
        return None;
    }
    let q = [at[0] + d[0] * t, at[1] + d[1] * t];
    let qn = [q[0] - c[0], q[1] - c[1]];
    let ql = (qn[0] * qn[0] + qn[1] * qn[1]).sqrt().max(1e-6);
    let nrm = [qn[0] / ql, qn[1] / ql];
    let rest = [d[0] * (1.0 - t), d[1] * (1.0 - t)];
    let k = rest[0] * nrm[0] + rest[1] * nrm[1];
    let slid = if k < 0.0 {
        [rest[0] - k * nrm[0], rest[1] - k * nrm[1]]
    } else {
        rest
    };
    Some((q, slid))
}

/// How far along a path (as a fraction of its length) the point on it nearest `q` lies.
fn progress_of(from: [f32; 3], points: &[[f32; 3]], q: [f32; 3]) -> f32 {
    let total = path_length(from, points);
    if total <= 0.0 {
        return 1.0;
    }
    let (mut best, mut at, mut walked) = (f32::MAX, 0.0, 0.0);
    let mut prev = from;
    for &p in points {
        let d = [p[0] - prev[0], p[1] - prev[1]];
        let len2 = d[0] * d[0] + d[1] * d[1];
        let len = crate::engine::dist(prev, p);
        let t = if len2 > 0.0 {
            (((q[0] - prev[0]) * d[0] + (q[1] - prev[1]) * d[1]) / len2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let near = [prev[0] + d[0] * t, prev[1] + d[1] * t];
        let off = (near[0] - q[0]).powi(2) + (near[1] - q[1]).powi(2);
        if off < best - 1e-6 {
            best = off;
            at = walked + len * t;
        }
        walked += len;
        prev = p;
    }
    (at / total).clamp(0.0, 1.0)
}

/// The points of a path still ahead at fraction `f` of it (the end of the leg under way
/// and those after it).
fn remaining_points(from: [f32; 3], points: &[[f32; 3]], f: f32) -> Vec<[f32; 3]> {
    let total = path_length(from, points);
    let mut left = total * f.clamp(0.0, 1.0);
    let mut prev = from;
    for (i, &p) in points.iter().enumerate() {
        let d = crate::engine::dist(prev, p);
        if left < d {
            return points[i..].to_vec();
        }
        left -= d;
        prev = p;
    }
    points.last().map(|&p| vec![p]).unwrap_or_default()
}

/// The party formation (DAOrigins.exe's default table at 0xc0d240, chosen in 0x7605b0): each
/// follower slot's place as (to the leader's right, forward), in metres. Slot 0 stands 1.8 m
/// behind-left, slot 1 behind-right, slot 2 2.5 m straight behind.
pub const FORMATION: [[f32; 2]; 11] = [
    [-1.5, -1.0],
    [1.5, -1.0],
    [0.0, -2.5],
    [-1.5, -4.0],
    [1.5, -4.0],
    [-3.0, -2.0],
    [3.0, -2.0],
    [-1.0, -3.0],
    [1.0, -3.0],
    [-3.5, -1.0],
    [3.5, -1.0],
];

/// Where follower slot `k` (from 0) stands with a leader at `p` heading `facing` (DAOrigins.exe
/// 0x760810 -> 0x7605b0 -> 0xa38a40): the [`FORMATION`] entry turned by the leader's heading.
/// Provisional: slots past the table (the exe picks a random point within 4 m) take its last.
pub fn behind(p: [f32; 3], facing: f32, k: usize) -> [f32; 3] {
    let heading = crate::world::heading_of(facing).to_radians();
    let (fx, fy) = (heading.cos(), heading.sin());
    // The leader's right: forward turned a quarter turn clockwise.
    let (rx, ry) = (fy, -fx);
    let [x, y] = FORMATION[k.min(FORMATION.len() - 1)];
    [p[0] + x * rx + y * fx, p[1] + x * ry + y * fy, p[2]]
}

/// The horizontal direction of the leg of the route from `from` through `points` that a
/// mover a share `f` of the way along it is on (the first leg at the start); `None` where
/// the legs have no horizontal length.
fn direction_along(from: [f32; 3], points: &[[f32; 3]], f: f32) -> Option<(f32, f32)> {
    let total = path_length(from, points);
    let mut left = total * f.clamp(0.0, 1.0);
    let mut prev = from;
    let mut dir = None;
    for &p in points {
        let (dx, dy) = (p[0] - prev[0], p[1] - prev[1]);
        if dx * dx + dy * dy > 1e-8 {
            dir = Some((dx, dy));
        }
        let d = crate::engine::dist(prev, p);
        if left < d && dir.is_some() {
            return dir;
        }
        left -= d;
        prev = p;
    }
    dir
}

/// Where an attacker goes before its attack starts (see `Sim::approach_point`).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Approach {
    /// In reach already: the attack starts.
    InReach,
    /// Walk this far to this point first.
    Walk(f32, [f32; 3]),
    /// The melee ring is full: walk this far to wait just outside it.
    WaitAt(f32, [f32; 3]),
    /// The melee ring is full and the attacker is by it: stand a second.
    Wait([f32; 3]),
    /// Still full after waiting: the attack fails.
    Full,
}

/// How far beyond the two bodies' radii a personal-range ability reaches (DAOrigins.exe
/// 0xafebc0), and the tolerance its check allows on top (0xade218).
const MELEE_TALENT_REACH: f32 = 1.2;
const MELEE_TALENT_TOLERANCE: f32 = 1.0;

/// How often an attacker or caster walking up to a creature checks whether it can start
/// (milliseconds). DAOrigins.exe runs a command's executor every update and the attack's
/// start step (0x7a87c0) tests the two creatures' current positions (0x6a3dc0); the
/// simulation has no frames, so it looks this often. Provisional.
const APPROACH_CHECK_MS: u64 = 100;
/// How long after a script asks for an area transition it happens (milliseconds): one
/// 30 fps update, as DAOrigins.exe switches modes at the start of the next client update
/// (0x641d30). Provisional: the update length is the app's frame, not traced.
pub const TRANSITION_DELAY_MS: u64 = 33;

/// How far above or below the pathing grid's floor a creature placed in an area may stand
/// and still be put on it (metres; see `Sim::spawn_instance`). Provisional.
const SPAWN_SNAP_RANGE: f32 = 2.0;
/// How far apart a moving creature's position is tested against triggers as it walks, in metres.
const TRIGGER_STEP: f32 = 0.5;

/// The melee ring around a creature: 40 places 9 degrees apart (DAOrigins.exe ring set-up
/// 0x6ac2a0, slot count table 0xb230f8). Provisional: the slots are taken at fixed world
/// angles; whether the exe turns them with the target's facing is not traced.
const MELEE_RING_SLOTS: i32 = 40;
const MELEE_RING_STEP_DEG: f32 = 360.0 / MELEE_RING_SLOTS as f32;
/// How far from its place an attacker may stand and skip the walk to it (0x7a3090: 1.2 and
/// its 0.05 tolerance, 0xae7540, 0xaf70b0).
const MELEE_GAP_AT_SLOT: f32 = 1.25;

/// The ring slot nearest the bearing from `centre` to `p`.
fn melee_slot_toward(centre: [f32; 3], p: [f32; 3]) -> i32 {
    let deg = (p[1] - centre[1]).atan2(p[0] - centre[0]).to_degrees();
    (deg / MELEE_RING_STEP_DEG).round() as i32
}

/// How many slots either side of its own an attacker of radius `attacker` covers on the ring
/// of a target of radius `target` (DAOrigins.exe 0x7cb9e0): the attacker's circle, scaled
/// to the ring's radius, is laid on its slot, and each step out to a neighbour whose chord
/// it still touches counts. Provisional: the segment-against-circle test (0x7cb790 calling
/// 0x4eb240) is read as "the nearer end of the chord is inside the circle".
fn melee_slot_span(target: f32, attacker: f32) -> i32 {
    let ring = target + crate::combat::MELEE_GAP + 0.1;
    let d = target + attacker + crate::combat::MELEE_GAP;
    let w = attacker * ring / d;
    let half = (MELEE_RING_STEP_DEG / 2.0).to_radians();
    let mut k = 1;
    while k < MELEE_RING_SLOTS / 2 && 2.0 * ring * (k as f32 * half).sin() < w {
        k += 1;
    }
    k
}

/// How far the place an approach is heading for may drift (the target having moved) before
/// the walker sets off again for the new one (metres). Provisional: the executable's
/// re-pathing rule is not traced.
const APPROACH_REPLAN: f32 = 1.0;

/// The slack beyond a follower's body and its slot's distance before it starts after a walking
/// leader (0xc0d230), and the extra slack while the leader stands (0xae7108), in metres
/// (DAOrigins.exe's start test 0x760980).
pub const FOLLOW_START: f32 = 2.0;
pub const FOLLOW_STANDING_EXTRA: f32 = 2.0;
/// A follower is only sent when its place is more than this far from it (0xaddc94).
pub const FOLLOW_RETARGET: f32 = 1.0;
/// How often followers are looked at, in milliseconds (the follow update, 0x630f60 calling
/// 0x760dc0 every 0.5 s).
pub const FOLLOW_PERIOD_MS: u64 = 500;

/// What the simulation does when a script starts character generation, since there is no
/// player at the character-creation screens.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum ChargenPolicy {
    /// End character generation at once with the chargen script's defaults: `CHARGEN_END`
    /// with mode 0 and "quick start" 0 makes `sys_chargen` pick a pre-made character and
    /// origin ("Setting default values for player character"); 1 and 2 keep the current
    /// choices and 3 starts in Lothering. Provisional, read from `sys_chargen`.
    #[default]
    QuickStart,
    /// Leave it running; a caller sends the `CHARGEN_*` events (see [`Sim::chargen_event`]).
    Manual,
}

/// What the simulation does with a cutscene a script loads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CutscenePolicy {
    /// End it at once, as if it had played (headless runs: nobody watches).
    #[default]
    Skip,
    /// Play it as [`crate::world::World::cutscene`]: the world stands still while its clock
    /// runs its length on the simulation's ticks (a presentation shows the line being said,
    /// [`crate::cutscene::Cutscene::line_now`]), then it ends; [`Sim::skip_cutscene`] ends it
    /// early. A cutscene that cannot be read ends at once.
    Play,
}

/// What the simulation does when a script shows the party picker (`ShowPartyPickerGUI`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PartyPickerPolicy {
    /// Decide at once (headless runs: nobody picks): the choice given beforehand
    /// ([`crate::world::World::party_choice`]), else the party as it was, filled from the pool.
    #[default]
    Instant,
    /// Wait for the player: [`crate::world::World::party_picker`] holds the members picked so
    /// far, and [`Sim::party_picker_submit`] applies them.
    Player,
}

/// Looks up an integer constant from the engine table (0 if absent).
/// Movement speed (m/s) for a creature whose appearance gives none. Provisional.
pub const DEFAULT_SPEED: f32 = 5.0;

/// `apr_base.gda` speed columns DAOrigins.exe's locomotion reads (0xa6b4f6-0xa6b5ad,
/// 0xa63120): walk, run, and run in combat stance.
const COL_WALK: u32 = 0x958f_23c8;
const COL_RUN: u32 = 0xa0dc_d78c;
const COL_COMBAT_RUN: u32 = 0x0dc3_ebb2;

pub fn constant(table: &EngineTable, name: &str) -> i32 {
    match table.constant(name).map(|c| &c.value) {
        Some(ConstValue::Int(v)) => *v as i32,
        _ => 0,
    }
}

impl Sim {
    pub fn new(vfs: Arc<Vfs>, table: Arc<EngineTable>, seed: u64) -> Self {
        let tables = Arc::new(crate::m2da::Tables::new(vfs.clone()));
        let pos = |n: &str| table.structures.iter().position(|s| s == n).unwrap_or(0) as u8;
        let types = EngineTypes {
            event: pos("event"),
            location: pos("location"),
            command: pos("command"),
            effect: pos("effect"),
        };
        let event_names = table
            .constants
            .iter()
            .filter_map(|c| match (&c.value, c.name.strip_prefix("EVENT_TYPE_")) {
                (ConstValue::Int(v), Some(n)) => Some((*v as i32, n.to_string())),
                _ => None,
            })
            .collect();
        Sim {
            world: World::new(tables, seed),
            vfs,
            table,
            module_info: ModuleInfo::default(),
            scripts: HashMap::new(),
            types,
            event_names,
            depth: 0,
            script_budget: 5_000_000,
            chargen_policy: ChargenPolicy::default(),
            cutscene_policy: CutscenePolicy::default(),
            party_picker_policy: PartyPickerPolicy::default(),
            auto_pause_combat: false,
            show_tutorials: true,
            training_done: 0,
            watch_calls: Default::default(),
            following: Default::default(),
            next_follow_ms: 0,
            watch_tags: Default::default(),
            current_effect: None,
            current_effect_target: None,
            dialog_ambient: false,
            install_root: None,
            area_layouts: HashMap::new(),
            nav_grids: HashMap::new(),
            line_cuts: HashMap::new(),
            collisions: HashMap::new(),
            placeable_collisions: HashMap::new(),
            blocked_since: HashMap::new(),
            fire_lengths: HashMap::new(),
            attack_lengths: HashMap::new(),
            ability_anims: HashMap::new(),
            save_issues: Vec::new(),
            door_cells: HashMap::new(),
            walk_blocked_by: None,
            talk: None,
            plots: crate::plot::Plots::default(),
            strings: std::sync::OnceLock::new(),
            item_looks: Default::default(),
        }
    }

    /// A talk-table string (English tables: `*_en-us.tlk` in the stack, merged).
    pub fn string(&self, strref: u32) -> Option<String> {
        let map = self.strings.get_or_init(|| {
            let mut m = std::collections::BTreeMap::new();
            for name in self
                .vfs
                .names()
                .into_iter()
                .filter(|n| n.ends_with("en-us.tlk"))
            {
                if let Ok((data, _)) = self.vfs.read(&name) {
                    if let Ok(t) = thedas_formats::Tlk::parse(&data) {
                        m.extend(t.strings);
                    }
                }
            }
            m
        });
        map.get(&strref)
            .map(|s| s.trim_end_matches('\0').to_string())
    }

    pub fn event_name(&self, ty: i32) -> String {
        self.event_names
            .get(&ty)
            .cloned()
            .unwrap_or_else(|| ty.to_string())
    }

    pub fn event_type(&self, name: &str) -> i32 {
        constant(&self.table, &format!("EVENT_TYPE_{name}"))
    }

    /// A copy of the mutable state (world and conversation) to restore later.
    pub fn snapshot(&self) -> (World, Option<crate::dialog::Conversation>) {
        (self.world.clone(), self.talk.clone())
    }

    pub fn restore(&mut self, s: (World, Option<crate::dialog::Conversation>)) {
        self.world = s.0;
        self.talk = s.1;
    }

    /// Registers a compiled script under a name, ahead of the resource stack (tools, tests).
    pub fn add_script(&mut self, name: &str, ncs: Ncs) {
        self.scripts
            .insert(name.to_ascii_lowercase(), Some(Arc::new(ncs)));
    }

    fn script(&mut self, name: &str) -> Option<Arc<Ncs>> {
        let key = name.to_ascii_lowercase();
        if let Some(s) = self.scripts.get(&key) {
            return s.clone();
        }
        // Scripts name others by their compiled (`.ncs`) or source (`.nss`) resource, or
        // bare: sp_module_item_acq hands item events to `preev_item_acquired.nss`.
        let res = match key.strip_suffix(".nss") {
            Some(stem) => format!("{stem}.ncs"),
            None if key.ends_with(".ncs") => key.clone(),
            None => format!("{key}.ncs"),
        };
        let loaded = self
            .vfs
            .read(&res)
            .ok()
            .and_then(|(d, _)| Ncs::parse(&d).ok())
            .map(Arc::new);
        self.scripts.insert(key, loaded.clone());
        loaded
    }

    /// Runs a script on `caller` with an optional current event. Errors are traced, not
    /// returned: a failing script stops, the simulation goes on.
    pub fn run_script(
        &mut self,
        name: &str,
        caller: ObjectId,
        event: Option<Event>,
    ) -> Option<Value> {
        let t = self.world.time_ms;
        if self.depth >= MAX_DEPTH {
            self.world.trace.push(TraceEntry::Missing {
                t,
                what: format!("script {name}: nesting deeper than {MAX_DEPTH}"),
            });
            return None;
        }
        let Some(ncs) = self.script(name) else {
            self.world.trace.push(TraceEntry::Missing {
                t,
                what: format!("script {name}"),
            });
            return None;
        };
        self.world.trace.push(TraceEntry::ScriptStart {
            t,
            script: name.to_string(),
            caller,
            depth: self.depth,
        });
        let event = event.map(|e| self.world.store(crate::world::EngineData::Event(e)));
        let mut vm = Vm::new(ncs, self.table.clone());
        vm.budget = self.script_budget;
        self.depth += 1;
        let result = {
            let mut host = ScriptHost {
                sim: self,
                event,
                script: name.to_string(),
            };
            vm.run(&mut host, caller)
        };
        self.depth -= 1;
        // Engine structures live only while scripts run (events, effects and commands are
        // copied by value into the world), so a finished top-level run frees its handles.
        if self.depth == 0 {
            self.world.handles.clear();
        }
        let (steps, error, value) = match result {
            Ok(r) => (r.steps, None, r.value),
            Err(e) => (0, Some(e.to_string()), None),
        };
        self.world.trace.push(TraceEntry::ScriptEnd {
            t: self.world.time_ms,
            script: name.to_string(),
            steps,
            error,
        });
        value
    }

    /// Delivers an event to an object's event script (or `script`, when given).
    pub fn deliver(&mut self, target: ObjectId, mut event: Event, script: Option<String>) {
        if event.target.is_none() {
            event.target = Some(target);
        }
        let Some(obj) = self.world.get(target) else {
            self.world.trace.push(TraceEntry::Missing {
                t: self.world.time_ms,
                what: format!("event {} for invalid object {target:#x}", event.ty),
            });
            return;
        };
        let Some(script) = script.or_else(|| obj.script.clone()) else {
            return;
        };
        self.world.trace.push(TraceEntry::EventDelivered {
            t: self.world.time_ms,
            target,
            ty: event.ty,
            event_name: self.event_name(event.ty),
            script: script.clone(),
        });
        let chargen_end = self.event_type("CHARGEN_END");
        let chargen_done = chargen_end != 0
            && event.ty == chargen_end
            && event.ints.first().copied().unwrap_or(0) == 0;
        let creature = event.objects.first().copied();
        self.run_script(&script, target, Some(event));
        // Once the module has handled the end of character creation (race and gender are
        // set by then), the new character keeps a face.
        if chargen_done {
            if let Some(c) = creature {
                self.keep_chargen_face(c);
            }
        }
    }

    /// Character-creation head presets per race and gender: `<race><gender>_pcc_b01` to
    /// `_b04` (`hm_pcc_b01.mor`; each with a `.mop` of slider settings).
    pub const CHARGEN_HEAD_PRESETS: u32 = 4;

    /// The head preset `index` (1-based) for a creature's race and gender: `apr_base.gda`'s
    /// `ModelRace` letter and the `Gender` field (2 female, else male); `None` for creatures
    /// that are not part-based humanoids or a preset that does not exist.
    pub fn chargen_head_preset(&self, id: ObjectId, index: u32) -> Option<String> {
        let o = self.world.get(id)?;
        let t = self.world.tables().get(0, "apr_base")?;
        let race = match t.cell("ModelRace", o.appearance as i64)? {
            thedas_formats::gda::Cell::String(s) => {
                s.trim_end_matches('\0').trim().to_ascii_lowercase()
            }
            _ => return None,
        };
        let r = race
            .chars()
            .next()
            .filter(|c| matches!(c, 'h' | 'e' | 'd' | 'q'))?;
        let g = if o.fields.get("Gender").copied() == Some(2) {
            'f'
        } else {
            'm'
        };
        let name = format!("{r}{g}_pcc_b{index:02}");
        self.vfs
            .lookup(&format!("{name}.mor"))
            .is_some()
            .then_some(name)
    }

    /// A creature's race and gender as `chargenmorphcfg.xml` names its lists (`human_male`,
    /// `elf_female`); `None` for a race the face screen does not offer.
    pub fn face_race_gender(&self, id: ObjectId) -> Option<String> {
        let o = self.world.get(id)?;
        let t = self.world.tables().get(0, "apr_base")?;
        let race = match t.cell("ModelRace", o.appearance as i64)? {
            thedas_formats::gda::Cell::String(s) => {
                s.trim_end_matches('\0').trim().to_ascii_lowercase()
            }
            _ => return None,
        };
        let race = match race.chars().next()? {
            'h' => "human",
            'e' => "elf",
            'd' => "dwarf",
            _ => return None,
        };
        let gender = if o.fields.get("Gender").copied() == Some(2) {
            "female"
        } else {
            "male"
        };
        Some(format!("{race}_{gender}"))
    }

    /// One of the face screen's lists (`heads`, `hairs`, `hair_colors`, ...) for a
    /// creature's race and gender (see [`thedas_formats::morph::face_config_list`]).
    pub fn face_list(&self, id: ObjectId, list: &str) -> Vec<String> {
        let Some(rg) = self.face_race_gender(id) else {
            return Vec::new();
        };
        self.vfs
            .read("chargenmorphcfg.xml")
            .ok()
            .and_then(|(xml, _)| thedas_formats::morph::face_config_list(&xml, list, &rg).ok())
            .unwrap_or_default()
    }

    /// The face character creation's face screen starts on for a creature: the first head
    /// `chargenmorphcfg.xml` lists for its race and gender (`hm_cps_p01`, a `.mop` preset:
    /// face shape, hair and colours), when that preset exists.
    pub fn face_screen_default(&self, id: ObjectId) -> Option<String> {
        self.face_list(id, "heads")
            .into_iter()
            .next()
            .filter(|h| self.vfs.lookup(&format!("{h}.mop")).is_some())
    }

    /// Sets one of character creation's appearance sliders on a creature (see
    /// [`crate::appearance`]). The preset puts that head on (`head_morph`) and drops the
    /// choices made over the last one; a creature with no face yet starts on the face
    /// screen's first.
    pub fn set_face_choice(&mut self, id: ObjectId, slider: u32, value: f32) {
        if !value.is_finite() {
            return;
        }
        if self.world.get(id).is_some_and(|o| o.head_morph.is_none()) {
            let first = self.face_screen_default(id);
            if let Some(o) = self.world.get_mut(id) {
                o.head_morph = first;
            }
        }
        let head = (slider == crate::appearance::PRESET)
            .then(|| self.face_list(id, "heads"))
            .and_then(|heads| heads.get(value.max(0.0) as usize).cloned());
        let Some(o) = self.world.get_mut(id) else {
            return;
        };
        if let Some(h) = head {
            o.head_morph = Some(h);
            o.face_choices.clear();
        }
        o.face_choices.insert(slider, value);
    }

    /// The face a creature shows: its head morph with the choices made over it (see
    /// [`crate::appearance::overlay`]), as one morph name for the renderer.
    pub fn face_name(&self, id: ObjectId) -> Option<String> {
        let o = self.world.get(id)?;
        let base = o.head_morph.clone()?;
        let overlay = crate::appearance::overlay(self, id);
        Some(overlay.name(&base))
    }

    /// Keeps a face on a creature that leaves character creation without one: the face the
    /// face screen starts on ([`Sim::face_screen_default`]), else the first head preset of
    /// its race and gender. Provisional: the face screen is not simulated, and its default
    /// is taken to be its first entry; the quick start's own choice is not traced.
    pub fn keep_chargen_face(&mut self, id: ObjectId) {
        if self.world.get(id).is_none_or(|o| o.head_morph.is_some()) {
            return;
        }
        let preset = self
            .face_screen_default(id)
            .or_else(|| self.chargen_head_preset(id, 1));
        if let Some(o) = self.world.get_mut(id) {
            o.head_morph = preset;
        }
    }

    /// Sets the head morph a creature keeps (the face screen's choice); `None` returns it to
    /// its template's.
    pub fn set_head_morph(&mut self, id: ObjectId, morph: Option<String>) {
        if let Some(o) = self.world.get_mut(id) {
            o.head_morph = morph
                .map(|m| m.trim().to_ascii_lowercase())
                .map(|m| m.strip_suffix(".mor").map(str::to_string).unwrap_or(m))
                .filter(|m| !m.is_empty());
        }
    }

    pub fn new_event(&self, name: &str, creator: Option<ObjectId>) -> Event {
        Event {
            ty: self.event_type(name),
            creator,
            ..Default::default()
        }
    }

    /// Advances time by `ms`, delivering every event that falls due and running commands.
    pub fn tick(&mut self, ms: u64) {
        // A cutscene playing holds the world still: its own clock runs instead, and what
        // is left of the tick once it ends goes on as usual.
        let ms = match self.world.cutscene.is_some() {
            true => match self.run_cutscene(ms) {
                Some(left) => left,
                None => return,
            },
            false => ms,
        };
        let start = self.world.time_ms;
        let mut until = start + ms;
        // Visual effects at places that run their time out during this tick go.
        let end = until;
        self.world
            .location_vfx
            .retain(|v| v.duration_ms.is_none_or(|d| v.started_ms + d > end));
        self.leave_conversation_mode();
        self.update_perception();
        self.keep_party_together();
        let mut held = 0;
        loop {
            // Engine work that waits for the current scripts: transitions, then cutscenes.
            // A script's transition happens at the start of the next update, as
            // DAOrigins.exe queues the mode switch for the next client update (0x641d30), so
            // the loading screen is up before the old area goes (see `TRANSITION_DELAY_MS`).
            if self.world.transition.is_some()
                && self
                    .world
                    .transition_at
                    .is_none_or(|at| at <= self.world.time_ms)
            {
                if let Some((area, waypoint)) = self.world.transition.take() {
                    self.world.transition_at = None;
                    self.area_transition(&area, &waypoint);
                }
                continue;
            }
            // One cutscene at a time: the next waits for the one playing.
            if !self.world.cutscenes.is_empty() && self.world.cutscene.is_none() {
                let name = self.world.cutscenes.remove(0);
                let res = crate::cutscene::Cutscene::resource(&name);
                let from_dialog = match self.world.dialog_cutscenes.iter().position(|n| *n == res) {
                    Some(i) => {
                        self.world.dialog_cutscenes.remove(i);
                        true
                    }
                    None => false,
                };
                if self.cutscene_policy == CutscenePolicy::Play {
                    if let Some(mut c) = self.read_cutscene(&name) {
                        c.from_dialog = from_dialog;
                        (c.bound, c.placement) = self.bind_cutscene_file(&res, from_dialog);
                        let t = self.world.time_ms;
                        self.world.trace.push(TraceEntry::Log {
                            t,
                            channel: -1,
                            text: format!(
                                "cutscene {} plays ({:.1} s, {} lines)",
                                c.name,
                                c.seconds,
                                c.lines.len()
                            ),
                        });
                        self.world.cutscene = Some(c);
                        // The world stops here; the rest of the tick plays the cutscene.
                        held = until - self.world.time_ms;
                        until = self.world.time_ms;
                        break;
                    }
                }
                if from_dialog {
                    // Not played, but begun and ended all the same.
                    let (bound, _) = self.bind_cutscene_file(&res, true);
                    self.release_cutscene_actors(bound);
                    self.close_dialog_cutscene(&name, false);
                } else {
                    self.end_cutscene(&name);
                }
                continue;
            }
            // Then the earliest of the next event and the next finishing command; events
            // first on a tie. Both run at their own time, so delays chain correctly.
            let event = self.world.queue.keys().next().map(|k| k.0);
            let command = self.next_command();
            // A pending transition comes before anything else due at or after its time.
            if let (Some(_), Some(at)) = (&self.world.transition, self.world.transition_at) {
                let first = event.into_iter().chain(command.map(|c| c.0)).min();
                if at <= until && first.is_none_or(|f| at <= f) {
                    self.world.time_ms = self.world.time_ms.max(at);
                    self.advance_movers();
                    continue;
                }
            }
            // Walkers closing in on a creature look again every so often: the target may
            // have come into reach, or moved away from where they are heading.
            if let Some(w) = self.approach_check_due() {
                let first = event.into_iter().chain(command.map(|c| c.0)).min();
                if w <= until && first.is_none_or(|f| w < f) {
                    self.world.time_ms = w;
                    self.advance_movers();
                    self.close_approaches();
                    continue;
                }
            }
            match (event, command) {
                (Some(e), c) if e <= until && c.is_none_or(|(d, _)| e <= d) => {
                    if let Some((due, q)) = self.world.pop_due(until) {
                        self.world.time_ms = self.world.time_ms.max(due);
                        // Objects in an area the party left do not run; their events wait
                        // until the party is back. Provisional.
                        if self.world.get(q.target).is_some_and(|o| o.parked.is_some()) {
                            self.world.held.push(q);
                            continue;
                        }
                        self.advance_movers();
                        let (target, ty) = (q.target, q.event.ty);
                        self.deliver(q.target, q.event, q.script);
                        self.next_heartbeat(target, ty);
                    }
                }
                (_, Some((due, id))) if due <= until => {
                    self.world.time_ms = self.world.time_ms.max(due);
                    self.advance_movers();
                    let approached = self
                        .world
                        .get_mut(id)
                        .and_then(|o| o.commands.first_mut())
                        .and_then(|c| {
                            let at = c.approach.take()?;
                            c.due = None;
                            c.from = None;
                            c.arrived = c.name == "CommandAttack";
                            Some(at)
                        });
                    // An ability whose impact comes before its animation ends deals the
                    // impact now and ends later (see `Sim::ability_timing`).
                    let impact = match approached {
                        Some(_) => None,
                        None => self
                            .world
                            .get_mut(id)
                            .and_then(|o| o.commands.first_mut())
                            .and_then(|c| {
                                let rest = c.after_impact_ms.take()?;
                                c.impacted = true;
                                c.due = Some(due + rest.max(1));
                                Some(c.clone())
                            }),
                    };
                    match (approached, impact) {
                        // In reach now: the attack itself starts next.
                        (Some(at), _) => self.move_object(id, at, None),
                        (None, Some(cmd)) => self.finish_ability(id, &cmd),
                        (None, None) => self.finish_command(id),
                    }
                }
                _ => break,
            }
        }
        let elapsed = until.saturating_sub(start);
        self.world.time_ms = until;
        self.advance_movers();
        self.regenerate(elapsed);
        self.decay_corpses(elapsed);
        self.update_threat(elapsed);
        self.expire_effects();
        self.update_aoes();
        self.world.destroy_due();
        self.settle_quickbars();
        self.autopilot_idle_hero();
        if held > 0 {
            self.tick(held);
        }
    }

    /// Runs the playing cutscene's clock `ms`; once it has run its length it ends, and the
    /// milliseconds left over are returned (`None` while it still plays).
    fn run_cutscene(&mut self, ms: u64) -> Option<u64> {
        let c = self.world.cutscene.as_mut()?;
        c.elapsed_ms += ms;
        let length = (c.seconds as f64 * 1000.0).ceil() as u64;
        if c.elapsed_ms < length {
            return None;
        }
        let left = c.elapsed_ms - length;
        self.finish_cutscene();
        Some(left)
    }

    /// The player skips the cutscene playing: it ends now.
    pub fn skip_cutscene(&mut self) {
        self.finish_cutscene();
    }

    /// Ends a cutscene the way the engine does when one finishes playing. Provisional, inferred
    /// from `cutscenes_h`: the scripts leave a plot flag to set (`CUTSCENE_SET_PLOT`,
    /// `CUTSCENE_SET_PLOT_FLAG`) and a creature to talk to (`CUTSCENE_TALK_SPEAKER`) in module
    /// locals before loading the cutscene, and nothing in the scripts reads them back.
    pub fn end_cutscene(&mut self, name: &str) {
        self.close_cutscene(name, false);
    }

    /// The cutscene playing has run (or the player skipped it): it ends as
    /// [`Sim::end_cutscene`] says, and the next one loaded may start.
    fn finish_cutscene(&mut self) {
        if let Some(mut c) = self.world.cutscene.take() {
            let bound = std::mem::take(&mut c.bound);
            self.release_cutscene_actors(bound);
            if c.from_dialog {
                self.close_dialog_cutscene(&c.name, true);
            } else {
                self.close_cutscene(&c.name, true);
            }
        }
    }

    /// A conversation line's cutscene ends: the conversation goes on (it waited, as the
    /// world does, while it played). Provisional: the scripts' cutscene-ending locals are
    /// for the cutscenes they load and are left alone.
    fn close_dialog_cutscene(&mut self, name: &str, played: bool) {
        let t = self.world.time_ms;
        self.world.trace.push(TraceEntry::Log {
            t,
            channel: -1,
            text: format!(
                "conversation cutscene {name} ends{}",
                if played { "" } else { " (not played)" }
            ),
        });
    }

    /// The objects cutscene file `res`'s actors take as it starts (see
    /// [`crate::cutactors`]); for a conversation line's cutscene, as the line that queued it
    /// plays it.
    fn bind_cutscene_file(
        &mut self,
        res: &str,
        from_dialog: bool,
    ) -> (
        Vec<crate::cutactors::Bound>,
        Option<crate::cuttracks::Placement>,
    ) {
        let Some(tracks) = self
            .vfs
            .read(res)
            .ok()
            .map(|(data, _)| crate::cuttracks::Tracks::parse(&data))
        else {
            return (Vec::new(), None);
        };
        let line = if from_dialog {
            let Some(c) = self.talk.take() else {
                return (Vec::new(), None);
            };
            let ctx = c.cut_from.and_then(|l| self.line_context(&c, l, &tracks));
            self.talk = Some(c);
            // Its stage is not in the area: nobody is moved.
            let Some(ctx) = ctx else {
                return (Vec::new(), None);
            };
            Some(ctx)
        } else {
            None
        };
        (
            self.bind_cutscene_actors(&tracks, line),
            line.map(|l| l.placement),
        )
    }

    /// A loaded cutscene's file (its length and lines), if it can be read. Lines that say
    /// a conversation's lines take their text from it (see
    /// [`crate::cutscene::dialog_refs`]).
    fn read_cutscene(&self, name: &str) -> Option<crate::cutscene::Cutscene> {
        let res = crate::cutscene::Cutscene::resource(name);
        let (data, _) = self.vfs.read(&res).ok()?;
        let g = thedas_formats::Gff4::parse(&data).ok()?;
        let mut c = crate::cutscene::Cutscene::from_gff4(&g, &res)?;
        let refs = crate::cutscene::dialog_refs(&g);
        let mut walked: std::collections::HashMap<String, (Vec<u32>, usize)> = Default::default();
        for r in refs {
            let key = r.dialog.to_ascii_lowercase();
            let (said, next) = walked
                .entry(key.clone())
                .or_insert_with(|| (self.walk_dialog_lines(&key), 0));
            if let Some(&strref) = said.get(*next) {
                c.lines.push(crate::cutscene::CutsceneLine {
                    start: r.start.clamp(0.0, c.seconds),
                    end: r.end.clamp(0.0, c.seconds),
                    strref,
                    actor: r.actor,
                    conversation: Some(r.dialog),
                    vo_mode: r.vo_mode,
                });
            }
            *next += 1;
        }
        c.lines.sort_by(|a, b| a.start.total_cmp(&b.start));
        Some(c)
    }

    /// The lines (talk-table references) conversation `dlg` says as it is walked from the
    /// line whose cutscene was queued (the open conversation's, when it is this one) or
    /// from its first start: each line, then its first reply's first line, and so on.
    /// Provisional (see [`crate::cutscene::dialog_refs`]).
    fn walk_dialog_lines(&self, dlg: &str) -> Vec<u32> {
        let open = self
            .talk
            .as_ref()
            .filter(|c| c.dlg.name.eq_ignore_ascii_case(dlg));
        let owned;
        let (d, from) = match open {
            Some(c) => (&c.dlg, c.cut_from.or(c.line)),
            None => {
                let Some(d) = self
                    .vfs
                    .read(&format!("{dlg}.dlg"))
                    .ok()
                    .and_then(|(data, _)| crate::dialog::Dlg::parse(&data, dlg))
                else {
                    return Vec::new();
                };
                owned = d;
                (&owned, owned.starts.first().copied())
            }
        };
        let mut out = Vec::new();
        let mut seen = Vec::new();
        let mut at = from;
        while let Some(i) = at {
            if seen.contains(&i) || seen.len() > 256 {
                break;
            }
            seen.push(i);
            let Some(n) = d.nodes.get(i) else { break };
            if !n.reply
                && let Some(s) = n.strref
            {
                out.push(s);
            }
            at = n.children.first().copied();
        }
        out
    }

    fn close_cutscene(&mut self, name: &str, played: bool) {
        let t = self.world.time_ms;
        self.world.trace.push(TraceEntry::Log {
            t,
            channel: -1,
            text: format!(
                "cutscene {name} ends{}",
                if played { "" } else { " (not played)" }
            ),
        });
        let Some(module) = self.world.module else {
            return;
        };
        let (plot, flag, speaker) = match self.world.get_mut(module) {
            Some(m) => {
                let l = &mut m.locals;
                (
                    l.strings.remove("CUTSCENE_SET_PLOT").unwrap_or_default(),
                    l.ints.remove("CUTSCENE_SET_PLOT_FLAG").unwrap_or(0),
                    l.strings
                        .remove("CUTSCENE_TALK_SPEAKER")
                        .unwrap_or_default(),
                )
            }
            None => return,
        };
        if !plot.is_empty() {
            self.set_plot_flag(&plot, flag, true, true, OBJECT_INVALID);
        }
        if !speaker.is_empty() {
            let hero = self.world.hero.unwrap_or(OBJECT_INVALID);
            let target = self.world.by_tag(&speaker).next().map(|o| o.id);
            if let Some(target) = target {
                self.start_conversation(hero, target, "");
            }
        }
    }

    /// Moves the party to another area (`DoAreaTransition`), loading it. Provisional: the
    /// area left behind stays in the world, inactive, and is not reused on return.
    /// The party pool: recruited followers in or out of the active party (follower state
    /// ACTIVE, AVAILABLE, SUSPENDED or LOCKEDACTIVE). They travel with the party, those out
    /// of it staying inactive. Provisional.
    pub fn party_pool(&self) -> Vec<ObjectId> {
        let states: Vec<i32> = [
            "FOLLOWER_STATE_ACTIVE",
            "FOLLOWER_STATE_AVAILABLE",
            "FOLLOWER_STATE_SUSPENDED",
            "FOLLOWER_STATE_LOCKEDACTIVE",
        ]
        .iter()
        .map(|c| constant(&self.table, c))
        .filter(|&v| v != 0)
        .collect();
        self.world
            .objects
            .values()
            .filter(|o| {
                Some(o.id) != self.world.hero
                    && o.kind == ObjectKind::Creature
                    && o.fields
                        .get("FollowerState")
                        .is_some_and(|s| states.contains(s))
            })
            .map(|o| o.id)
            .collect()
    }

    /// The player answers the open message box: `EVENT_TYPE_POPUP_RESULT` to its owner (the
    /// module when it has none), object 0 the owner, integers the popup's kind and the
    /// button pressed (1 for the first). Returns false when no popup is open.
    /// `EVENT_TYPE_USE` from `user` for `placeable`: integer 0 says whether the user stands
    /// in front of it, as DAOrigins.exe's use handler sets it (0x7a7a96: the placeable's
    /// forward vector against the direction to the user, >= 0; +X when they stand at the
    /// same spot).
    pub fn use_event(&self, user: ObjectId, placeable: ObjectId) -> Event {
        let mut ev = self.new_event("USE", Some(user));
        let front = match (self.world.get(placeable), self.world.get(user)) {
            (Some(p), Some(u)) => {
                let h = crate::world::heading_of(p.facing).to_radians();
                let (dx, dy) = (u.position[0] - p.position[0], u.position[1] - p.position[1]);
                let len = (dx * dx + dy * dy).sqrt();
                let (dx, dy) = if len > 1e-6 {
                    (dx / len, dy / len)
                } else {
                    (1.0, 0.0)
                };
                h.cos() * dx + h.sin() * dy >= 0.0
            }
            _ => false,
        };
        ev.ints = vec![front as i32];
        ev
    }

    pub fn answer_popup(&mut self, button: i32) -> bool {
        self.answer_popup_with_text(button, "")
    }

    /// [`Sim::answer_popup`] with the text typed into the box: DAOrigins.exe's
    /// `POPUP_RESULT` (built at 0x709950) carries it as string 0 (0x709971), empty when the
    /// box has no input field.
    pub fn answer_popup_with_text(&mut self, button: i32, text: &str) -> bool {
        let Some(p) = self.world.popup.take() else {
            return false;
        };
        let target = if self.world.get(p.owner).is_some() {
            p.owner
        } else {
            match self.world.module {
                Some(m) => m,
                None => return false,
            }
        };
        let mut ev = self.new_event("POPUP_RESULT", self.world.hero);
        ev.objects = vec![p.owner];
        ev.ints = vec![p.kind, button];
        ev.strings = vec![text.to_string()];
        self.world.queue_event(0, target, ev, None);
        true
    }

    /// A screen opened that was not up already: the module hears `EVENT_TYPE_GUI_OPENED`
    /// (84) with integer 0 the screen's `GUI_*` id (the inventory opened as a container
    /// passes `GUI_CONTAINER_INVENTORY`, 2), as DAOrigins.exe's screen-showing routine
    /// sends it (0x60afa3-0x60b065). Provisional: target and creator are taken to be the
    /// module (0x5d4fc0 is not fully traced).
    pub fn gui_opened(&mut self, gui: i32) {
        let Some(module) = self.world.module else {
            return;
        };
        let mut ev = self.new_event("GUI_OPENED", Some(module));
        ev.ints = vec![gui];
        self.world.queue_event(0, module, ev, None);
    }

    /// Tells the module a creature joined or left the active party
    /// (`EVENT_TYPE_PARTYMEMBER_ADDED` / `EVENT_TYPE_PARTYMEMBER_DROPPED`, object 0 the
    /// follower), on which module_core keeps the party plot's `GEN_<name>_IN_PARTY` flags.
    /// DAOrigins.exe raises them only from the party picker, for each member whose state the
    /// picker changed (0x636f50: available to active posts ADDED at 0x63710e, active to
    /// available DROPPED at 0x637168; also the picker GUI, 0x766c10), never from
    /// `SetFollowerState`.
    pub(crate) fn party_membership_changed(&mut self, who: ObjectId, joined: bool) {
        let Some(module) = self.world.module else {
            return;
        };
        let name = if joined {
            "PARTYMEMBER_ADDED"
        } else {
            "PARTYMEMBER_DROPPED"
        };
        let mut ev = self.new_event(name, self.world.hero);
        ev.objects = vec![who];
        self.world.queue_event(0, module, ev, None);
    }

    /// `ShowPartyPickerGUI`: the module hears the picker open (`EVENT_TYPE_PARTYPICKER_INIT`).
    /// Under [`PartyPickerPolicy::Player`] the picker then waits for the player, starting from
    /// the party as it is; otherwise it is decided at once ([`Sim::resolve_party_picker`]).
    pub fn party_picker(&mut self) {
        let (Some(module), Some(hero)) = (self.world.module, self.world.hero) else {
            return;
        };
        let init = self.new_event("PARTYPICKER_INIT", Some(hero));
        self.world.queue_event(0, module, init, None);
        if self.party_picker_policy == PartyPickerPolicy::Player {
            let mut selected: Vec<ObjectId> = self
                .world
                .get(hero)
                .map(|h| h.followers.clone())
                .unwrap_or_default();
            for l in self.locked_followers() {
                if !selected.contains(&l) {
                    selected.push(l);
                }
            }
            self.world.party_picker = Some(crate::world::PartyPicker { selected });
            return;
        }
        self.resolve_party_picker();
    }

    /// Followers whose state is `FOLLOWER_STATE_LOCKEDACTIVE`: the picker cannot leave
    /// them out.
    pub fn locked_followers(&self) -> Vec<ObjectId> {
        let locked = constant(&self.table, "FOLLOWER_STATE_LOCKEDACTIVE");
        let mut v: Vec<ObjectId> = self
            .party_pool()
            .into_iter()
            .filter(|&p| {
                locked != 0
                    && self
                        .world
                        .get(p)
                        .is_some_and(|o| o.fields.get("FollowerState") == Some(&locked))
            })
            .collect();
        v.sort_unstable();
        v
    }

    /// The player picks or drops a follower in the open picker (clicking them in camp): one
    /// not picked joins while fewer than three are, one picked leaves unless it is locked.
    /// Returns whether the pick changed.
    pub fn party_picker_toggle(&mut self, who: ObjectId) -> bool {
        let locked = self.locked_followers();
        let in_pool = self.party_pool().contains(&who) && self.alive(who);
        let Some(p) = self.world.party_picker.as_mut() else {
            return false;
        };
        if let Some(i) = p.selected.iter().position(|&m| m == who) {
            if locked.contains(&who) {
                return false;
            }
            p.selected.remove(i);
            return true;
        }
        if !in_pool || p.selected.len() >= 3 {
            return false;
        }
        p.selected.push(who);
        true
    }

    /// The picker's portrait `slot` is clicked (partypicker.gfx's
    /// `GameModeController.UnselectPartyMember`): slot 0 is the hero, 1 to 3 the followers
    /// picked, in order.
    pub fn party_picker_unselect(&mut self, slot: usize) -> bool {
        let who = self
            .world
            .party_picker
            .as_ref()
            .and_then(|p| slot.checked_sub(1).and_then(|i| p.selected.get(i)).copied());
        who.is_some_and(|w| self.party_picker_toggle(w))
    }

    /// partypicker.gfx's Remove All (`GameModeController.ClearPartyPicker`): only the locked
    /// followers stay picked.
    pub fn party_picker_clear(&mut self) -> bool {
        let locked = self.locked_followers();
        match self.world.party_picker.as_mut() {
            Some(p) => {
                p.selected.retain(|m| locked.contains(m));
                true
            }
            None => false,
        }
    }

    /// partypicker.gfx's Accept (`GameModeController.SubmitPartyChanges`): the picked
    /// followers become the party, as [`Sim::resolve_party_picker`] applies a choice.
    pub fn party_picker_submit(&mut self) -> bool {
        let Some(p) = self.world.party_picker.take() else {
            return false;
        };
        let before = self.world.party_choice.replace(p.selected);
        self.resolve_party_picker();
        self.world.party_choice = before;
        true
    }

    /// The picker's choice applied, as DAOrigins.exe's `ApplyPartyPickerChanges` handler
    /// (0x636f50) does: each follower whose place changed is announced to the module
    /// (`EVENT_TYPE_PARTYMEMBER_ADDED` 66, `_DROPPED` 67, the follower as object 0), then
    /// `EVENT_TYPE_PARTYPICKER_CLOSED` (71). The choice is
    /// [`crate::world::World::party_choice`] when one was made, else the party as it was,
    /// free places (three followers in all) going to the pool in the order they were
    /// recruited. No script sets the chosen members' states after the picker, so the engine
    /// does, switching their event scripts as `WR_SetFollowerState` would. Provisional: the
    /// fill from the pool when nobody picks.
    pub fn resolve_party_picker(&mut self) {
        let (Some(module), Some(hero)) = (self.world.module, self.world.hero) else {
            return;
        };
        // The player's choice when one was made, else the party as it was, filled up from
        // the pool.
        let mut pool = self.party_pool();
        let mut chosen = match &self.world.party_choice {
            Some(c) => c
                .iter()
                .copied()
                .filter(|p| {
                    pool.contains(p)
                        || self
                            .world
                            .get(hero)
                            .is_some_and(|h| h.followers.contains(p))
                })
                .take(3)
                .collect(),
            None => self
                .world
                .get(hero)
                .map(|h| h.followers.clone())
                .unwrap_or_default(),
        };
        pool.sort();
        if self.world.party_choice.is_some() {
            pool.clear();
        }
        for p in pool {
            if chosen.len() >= 3 {
                break;
            }
            if !chosen.contains(&p) && self.alive(p) {
                chosen.push(p);
            }
        }
        let (active, available) = (
            constant(&self.table, "FOLLOWER_STATE_ACTIVE"),
            constant(&self.table, "FOLLOWER_STATE_AVAILABLE"),
        );
        let area = self.world.get(hero).and_then(|h| h.area);
        let group = self.world.get(hero).map_or(0, |h| h.group);
        for p in self.party_pool() {
            let on = chosen.contains(&p);
            if let Some(o) = self.world.get_mut(p) {
                let locked = o.fields.get("FollowerState")
                    == Some(&constant(&self.table, "FOLLOWER_STATE_LOCKEDACTIVE"));
                if !locked {
                    o.fields
                        .insert("FollowerState".into(), if on { active } else { available });
                    // As rules_core's WR_SetFollowerState switches them: party members fire
                    // their events into player_core, those left in the pool into
                    // creature_core (which warns when a party member's events reach it).
                    o.script = Some(if on { "player_core" } else { "creature_core" }.into());
                }
                o.active = on || locked;
                if on {
                    o.area = area;
                    o.group = group;
                }
            }
        }
        let before = self
            .world
            .get(hero)
            .map(|h| h.followers.clone())
            .unwrap_or_default();
        for &p in before.iter().filter(|p| !chosen.contains(p)) {
            self.party_membership_changed(p, false);
        }
        for &p in chosen.iter().filter(|p| !before.contains(p)) {
            self.party_membership_changed(p, true);
        }
        if let Some(h) = self.world.get_mut(hero) {
            h.followers = chosen;
        }
        let closed = self.new_event("PARTYPICKER_CLOSED", Some(hero));
        self.world.queue_event(0, module, closed, None);
    }

    pub fn area_transition(&mut self, area: &str, waypoint: &str) {
        let t = self.world.time_ms;
        self.world.trace.push(TraceEntry::Log {
            t,
            channel: -1,
            text: format!("area transition to {area} ({waypoint})"),
        });
        // The loading screen shows the hint a script set for this transition, once.
        self.world.transition_hint = self.world.load_hint.take();
        let old = self.world.area;
        let party: Vec<ObjectId> = self
            .world
            .hero
            .into_iter()
            .chain(
                self.world
                    .hero
                    .and_then(|h| self.world.get(h))
                    .map(|h| h.followers.clone())
                    .unwrap_or_default(),
            )
            .chain(self.party_pool())
            .collect();
        // The party leaves the old area: its script hears each member go
        // (EVENT_TYPE_EXIT, creator and object 0 the member), before the area unloads.
        if let Some(old) = old {
            for &id in &party {
                if self.world.get(id).is_some_and(|o| o.area == Some(old)) {
                    let mut ev = self.new_event("EXIT", Some(id));
                    ev.objects = vec![id];
                    self.deliver(old, ev, None);
                }
            }
        }
        // The old area unloads: its objects go inactive, remembering whether they were
        // active for when the party comes back.
        if let Some(old) = old {
            for o in self.world.objects.values_mut() {
                if o.area == Some(old) && !party.contains(&o.id) || o.id == old {
                    o.parked = Some(o.active);
                    o.active = false;
                }
            }
        }
        let wp = (!waypoint.is_empty()).then_some(waypoint);
        match self.enter_area(area, wp) {
            Ok(new) => {
                self.place_party_on_map(area);
                self.travel_arrived();
                let at = self
                    .world
                    .hero
                    .and_then(|h| self.world.get(h))
                    .map(|h| (h.position, h.facing));
                for (k, id) in party.iter().skip(1).enumerate() {
                    self.following.remove(id);
                    if let Some(o) = self.world.get_mut(*id) {
                        o.area = Some(new);
                        if let Some((p, facing)) = at {
                            o.position = behind(p, facing, k);
                            o.facing = facing;
                        }
                        // What a member was doing in the old area ends with it: a move under
                        // way would otherwise carry on from where it began there, putting
                        // the member back at its old coordinates in the new area (Daveth
                        // and Jory left at their Ostagar spots in the Korcari Wilds).
                        // Provisional: the exe's clearing of action queues on a transition
                        // is not traced.
                        o.commands.clear();
                        // Followers enter after the hero (whose ENTER `enter_area` sent).
                        let mut ev = self.new_event("ENTER", Some(*id));
                        ev.objects = vec![*id];
                        self.world.queue_event(0, new, ev, None);
                    }
                }
            }
            Err(e) => {
                if let Some(old) = old {
                    for o in self.world.objects.values_mut() {
                        if o.area == Some(old) || o.id == old {
                            if let Some(a) = o.parked.take() {
                                o.active = a;
                            }
                        }
                    }
                }
                self.world.trace.push(TraceEntry::Log {
                    t,
                    channel: -1,
                    text: format!("area transition to {area} failed: {e}"),
                });
            }
        }
    }

    fn chargen_event_for(&self, ty: &str, creature: ObjectId) -> Event {
        let mut ev = self.new_event(ty, Some(creature));
        ev.objects = vec![creature];
        ev
    }

    /// `StartCharGen`: tells the module character generation started, and under
    /// [`ChargenPolicy::QuickStart`] ends it at once with the quick-start flag.
    pub fn start_chargen(&mut self, creature: ObjectId, mode: i32, import: i32) {
        let Some(module) = self.world.module else {
            return;
        };
        self.world.chargen = Some(creature);
        self.world.chargen_mode = mode;
        let mut ev = self.chargen_event_for("CHARGEN_START", creature);
        ev.ints = vec![mode, import];
        self.world.queue_event(0, module, ev, None);
        // Level-up (mode 1) only starts when someone asked for it (`request_level_up`), so
        // it is left for them to finish.
        if self.chargen_policy == ChargenPolicy::QuickStart && mode == 0 {
            self.chargen_event("CHARGEN_END", vec![mode, 0], Vec::new());
        }
    }

    /// The level-up button: `EVENT_TYPE_PLAYERLEVELUP` to the module for a party member,
    /// whose chargen script opens character generation in level-up mode (`StartCharGen`
    /// mode 1); the choices then arrive as `CHARGEN_ASSIGN_ATTRIBUTES` (attribute,
    /// amount) and `CHARGEN_ASSIGN_ABILITIES` (ability) events and `CHARGEN_END`.
    /// Provisional, read from `sys_chargen`.
    pub fn request_level_up(&mut self, creature: ObjectId) {
        let Some(module) = self.world.module else {
            return;
        };
        let mut ev = self.new_event("PLAYERLEVELUP", Some(creature));
        ev.objects = vec![creature];
        self.world.queue_event(0, module, ev, None);
    }

    /// Sends a character-generation event (`CHARGEN_SELECT_RACE`, `CHARGEN_END`, ...) for the
    /// creature in character generation to the module, as the chargen screens would: the
    /// creature is object 0 and the choices are the integers and strings. Provisional: the
    /// argument layout is inferred from `sys_chargen`.
    pub fn chargen_event(&mut self, ty: &str, ints: Vec<i32>, strings: Vec<String>) -> bool {
        let (Some(module), Some(creature)) = (self.world.module, self.world.chargen) else {
            return false;
        };
        let mut ev = self.chargen_event_for(ty, creature);
        ev.ints = ints;
        ev.strings = strings;
        if ty == "CHARGEN_END" {
            self.world.chargen = None;
        }
        self.world.queue_event(0, module, ev, None);
        true
    }

    /// How long a command takes once it reaches the front of its queue. Provisional: there is
    /// no animation or pathfinding, so moves go at 5 m/s in a straight line, waits take their
    /// time, and anything else takes half a second. Every command takes at least a
    /// millisecond, so scripts that queue a new command when one completes cannot loop
    /// forever within one instant.
    fn command_duration(&self, id: ObjectId, cmd: &Command) -> u64 {
        let ms = if cmd.name == "CommandWait" {
            (cmd.seconds.max(0.0) * 1000.0) as u64
        } else if cmd.name == "CommandSwitchWeaponSet" {
            // Done as soon as it runs (its executor, 0x7a1ed0, returns complete).
            0
        } else if cmd.name.starts_with("CommandMoveTo") || cmd.name.starts_with("CommandJumpTo") {
            let from = self.world.get(id).map(|o| o.position).unwrap_or_default();
            let to = cmd
                .location
                .map(|l| l.position)
                .or_else(|| {
                    cmd.target
                        .and_then(|t| self.world.get(t))
                        .map(|o| o.position)
                })
                .unwrap_or(from);
            let d =
                ((to[0] - from[0]).powi(2) + (to[1] - from[1]).powi(2) + (to[2] - from[2]).powi(2))
                    .sqrt();
            if cmd.name.starts_with("CommandJumpTo") {
                0
            } else {
                (d / self.command_speed(id, cmd) * 1000.0) as u64
            }
        } else {
            500
        };
        ms.max(1)
    }

    /// Moves an object and sends trigger ENTER/EXIT events for creatures that cross a
    /// trigger's outline. Movement is a teleport: only the destination is tested.
    pub fn move_object(&mut self, id: ObjectId, position: [f32; 3], area: Option<ObjectId>) {
        if let Some(o) = self.world.get_mut(id) {
            o.position = position;
            if area.is_some() {
                o.area = area;
            }
        }
        self.update_triggers(id);
    }

    /// How fast a creature runs, in metres per second, as DAOrigins.exe's locomotion picks
    /// its target speed (0xa63120): an `apr_base.gda` column by the creature's appearance,
    /// 0xa0dcd78c for a run (6.0 for humans, hurlocks and walking corpses) or, in combat
    /// stance, 0x0dc3ebb2 (4.66). The stance is the creature's combat state: the locomotion's
    /// flag (+0xf9, set through 0x6a7f30) follows it each update (0x6be1b6 reads 0x6a2580,
    /// the state `SetCombatState` writes, 0x6ae0a0). Else [`DEFAULT_SPEED`].
    ///
    /// Provisional: the target speed's multiplier and bonus (+0xd4, +0xdc; 1 and 0 when the
    /// locomotion is made, 0xa633a2) and its second stance column (0x03186054, 3.5, under a
    /// flag at +0xf8 whose setter is not traced) are not modelled, and creatures reach their
    /// speed at once where the executable accelerates at 8 m/s per second (0xa635d0,
    /// 0xb1b7c8). The column 0xbd6d4446 (3.62), used before, is the run animation's
    /// reference speed for blending (0xa6aa70), not a movement speed.
    pub fn move_speed(&self, id: ObjectId) -> f32 {
        let combat = self.world.get(id).is_some_and(|o| o.combat.in_combat);
        self.appearance_speed(id, if combat { COL_COMBAT_RUN } else { COL_RUN })
            .or_else(|| self.appearance_speed(id, COL_RUN))
            .unwrap_or(DEFAULT_SPEED)
    }

    /// How fast a command moves its owner: a move ordered not to run
    /// (`CommandMoveToLocation`'s or `CommandMoveToObject`'s `bRunToLocation` FALSE) walks,
    /// at the appearance's walk speed (column 0x958f23c8, 1.54 for humans; the locomotion's
    /// mode 1 in 0xa63120); anything else runs ([`Sim::move_speed`]).
    pub fn command_speed(&self, id: ObjectId, cmd: &Command) -> f32 {
        let walks = cmd.name.starts_with("CommandMoveTo") && cmd.ints.first() == Some(&0);
        if walks {
            if let Some(v) = self.appearance_speed(id, COL_WALK) {
                return v;
            }
        }
        self.move_speed(id)
    }

    /// An `apr_base.gda` speed column for a creature's appearance, if it gives one.
    fn appearance_speed(&self, id: ObjectId, column: u32) -> Option<f32> {
        let a = self
            .world
            .get(id)
            .map(|o| o.appearance)
            .filter(|&a| a > 0)?;
        let t = self.world.tables().get(0, "apr_base")?;
        match t.cell_by_hash(column, a as i64)? {
            thedas_formats::gda::Cell::Float(v) if *v > 0.1 => Some(*v as f32),
            thedas_formats::gda::Cell::Int(v) if *v > 0 => Some(*v as f32),
            _ => None,
        }
    }

    /// Walks an object to `to` at its run speed, running the world as it goes, so triggers on the
    /// way fire and the world reacts. The route follows the area's pathing grid when there
    /// is one (see `nav`), otherwise a straight line in half-metre steps. Stops early when a
    /// conversation starts, the area changes, the walker dies or (for the hero) a fight
    /// breaks out; returns whether it arrived. A destination in another area is a teleport.
    /// A closed door in the way stops it before it sets out (the door is left in
    /// `walk_blocked_by`; the player opens doors by using them).
    pub fn walk(&mut self, id: ObjectId, to: [f32; 3], area: Option<ObjectId>) -> bool {
        self.walk_blocked_by = None;
        let Some((from, here)) = self.world.get(id).map(|o| (o.position, o.area)) else {
            return false;
        };
        if area.is_some() && area != here {
            self.move_object(id, to, area);
            self.tick(0);
            return true;
        }
        let routed = match self.route_to(id, to) {
            crate::nav::Routed::Path(p) => Some(p),
            crate::nav::Routed::Straight => None,
            crate::nav::Routed::Door(d) => {
                self.walk_blocked_by = Some(d);
                return false;
            }
        };
        let points: Vec<[f32; 3]> = match routed {
            Some(p) if !p.is_empty() => p,
            _ => {
                let d = crate::engine::dist(from, to);
                let steps = (d / 0.5).ceil().max(1.0) as usize;
                (1..=steps)
                    .map(|i| {
                        let f = i as f32 / steps as f32;
                        [
                            from[0] + (to[0] - from[0]) * f,
                            from[1] + (to[1] - from[1]) * f,
                            from[2] + (to[2] - from[2]) * f,
                        ]
                    })
                    .collect()
            }
        };
        let speed = self.move_speed(id);
        let talking = self.talk.as_ref().is_some_and(|c| !c.ended);
        let combat = constant(&self.table, "GM_COMBAT");
        let hero_at_peace = Some(id) == self.world.hero && self.world.game_mode != combat;
        let mut prev = from;
        let n = points.len();
        for (i, p) in points.into_iter().enumerate() {
            let d = crate::engine::dist(prev, p);
            if d > 0.01 {
                if let Some(o) = self.world.get_mut(id) {
                    o.facing = crate::world::facing_toward(p[0] - prev[0], p[1] - prev[1]);
                }
            }
            self.move_object(id, p, None);
            if Some(id) == self.world.hero {
                self.follow_leader(id);
            }
            self.tick(((d / speed) * 1000.0).round().max(1.0) as u64);
            prev = p;
            let o = self.world.get(id);
            let stopped = o.is_none_or(|o| o.dead || o.area != here)
                || (!talking && self.talk.as_ref().is_some_and(|c| !c.ended))
                || (hero_at_peace && self.world.game_mode == combat);
            if stopped {
                return i + 1 == n;
            }
        }
        true
    }

    /// Keeps the hero's followers with it as [`Sim::walk`] steps it along: each living
    /// follower in the same area that follows the leader (`SetFollowPartyLeader`, on unless
    /// turned off) and is not fighting or busy with anything but waiting or its own follow
    /// move is put on its formation slot behind the leader ([`behind`], the exe's table at
    /// 0xc0d240). Provisional: followers are placed rather than walked (this serves the
    /// stepped walk of the tools; in play `Sim::keep_party_together` runs them there).
    pub fn follow_leader(&mut self, leader: ObjectId) {
        let Some((pos, area, facing, followers)) = self.world.get(leader).map(|o| {
            let mut all = o.followers.clone();
            all.extend(o.escorts.iter().copied());
            (o.position, o.area, o.facing, all)
        }) else {
            return;
        };
        for (i, f) in followers.into_iter().enumerate() {
            let ok = self.world.get(f).is_some_and(|o| {
                !o.dead
                    && o.area == area
                    && !o.combat.in_combat
                    && o.commands.first().is_none_or(|c| {
                        matches!(c.name.as_str(), "CommandWait" | "CommandMoveToLocation")
                    })
                    && o.fields.get("FollowPartyLeader").copied().unwrap_or(1) != 0
            });
            if ok {
                self.move_object(f, behind(pos, facing, i), None);
            }
        }
    }

    /// Keeps the party with the member the player controls as it moves by command (a click,
    /// a script's move): out of combat, each follower that is alive, in the leader's area,
    /// follows the leader (`SetFollowPartyLeader`, on unless turned off), is not fighting
    /// and has nothing to do but wait or its own follow move runs to its place behind the
    /// leader once it is more than [`FOLLOW_START`] from it, and is sent on again when that
    /// place moves more than [`FOLLOW_RETARGET`] from where it is heading. Places are where
    /// followers arrive with the leader (`behind`). Provisional: the original's party follow
    /// (its distances, formation and pathing) is not traced; this keeps followers with the
    /// leader the way [`Sim::follow_leader`] does for [`Sim::walk`].
    pub fn keep_party_together(&mut self) {
        let combat = constant(&self.table, "GM_COMBAT");
        let Some(leader) = self.world.controlled().or(self.world.hero) else {
            return;
        };
        let Some((pos, area, facing)) = self
            .world
            .get(leader)
            .map(|o| (o.position, o.area, o.facing))
        else {
            return;
        };
        // Follow moves that have ended are no longer ours.
        let ended: Vec<ObjectId> = self
            .following
            .iter()
            .copied()
            .filter(|&f| {
                self.world.get(f).is_none_or(|o| {
                    o.commands
                        .first()
                        .is_none_or(|c| c.name != "CommandMoveToLocation")
                })
            })
            .collect();
        for f in ended {
            self.following.remove(&f);
        }
        if combat != 0 && self.world.game_mode == combat {
            return;
        }
        // The exe looks every half second, not every frame.
        if self.world.time_ms < self.next_follow_ms {
            return;
        }
        self.next_follow_ms = (self.world.time_ms / FOLLOW_PERIOD_MS + 1) * FOLLOW_PERIOD_MS;
        let leader_walking = self.world.get(leader).is_some_and(|o| {
            o.commands
                .first()
                .is_some_and(|c| c.name.starts_with("CommandMoveTo") && c.from.is_some())
        });
        let followers: Vec<ObjectId> = crate::engine::party_of(self)
            .into_iter()
            .filter(|&f| f != leader)
            .collect();
        for (i, f) in followers.into_iter().enumerate() {
            let ours = self.following.contains(&f);
            let Some(o) = self.world.get(f) else { continue };
            let free = !o.dead
                && o.area == area
                && !o.combat.in_combat
                && o.fields.get("FollowPartyLeader").copied().unwrap_or(1) != 0
                && (ours || o.commands.first().is_none_or(|c| c.name == "CommandWait"));
            if !free {
                continue;
            }
            // DAOrigins.exe's start test (0x760980): the follower must be at least its body,
            // its slot's distance and 2 m from the leader (2 m more while the leader stands),
            // then its place more than 1 m from it (0xaddc94).
            let place = behind(pos, facing, i);
            let [x, y] = FORMATION[i.min(FORMATION.len() - 1)];
            let mut start = self.body_radius(f) + (x * x + y * y).sqrt() + FOLLOW_START;
            if !leader_walking {
                start += FOLLOW_STANDING_EXTRA;
            }
            let ground =
                |a: [f32; 3], b: [f32; 3]| ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt();
            let heading = o
                .commands
                .first()
                .filter(|_| ours)
                .and_then(|c| c.location)
                .map(|l| l.position);
            let due = match heading {
                // A follow move under way is re-aimed at the new place (in place in the exe).
                Some(h) => ground(h, place) > FOLLOW_RETARGET,
                None => {
                    ground(o.position, pos) >= start && ground(o.position, place) > FOLLOW_RETARGET
                }
            };
            if !due {
                continue;
            }
            let Some(area) = area else { continue };
            let from = o.position;
            let mut cmd = Command {
                ty: constant(&self.table, "COMMAND_TYPE_MOVE_TO_LOCATION"),
                name: "CommandMoveToLocation".into(),
                target: None,
                location: Some(crate::world::Location {
                    area,
                    position: place,
                    facing: crate::world::facing_toward(place[0] - from[0], place[1] - from[1]),
                }),
                seconds: 0.0,
                ints: Vec::new(),
                strings: Vec::new(),
                due: None,
                from: None,
                approach: None,
                ring_wait: false,
                after_impact_ms: None,
                impacted: false,
                arrived: false,
                path: Vec::new(),
            };
            // A follow move already under way goes on from where it has got to.
            if ours {
                self.advance_movers();
                if let Some(now) = self.world.get(f).map(|o| o.position) {
                    cmd.location.as_mut().expect("set above").facing =
                        crate::world::facing_toward(place[0] - now[0], place[1] - now[1]);
                }
            }
            if let Some(o) = self.world.get_mut(f) {
                o.commands.clear();
                o.commands.push(cmd);
            }
            self.following.insert(f);
        }
    }

    /// Queues ENTER for triggers a creature is now in and EXIT for ones it left. Provisional:
    /// trigger outlines are tested in the XY plane, and the creature is the event's creator
    /// and object 0.
    pub fn update_triggers(&mut self, id: ObjectId) {
        let Some(o) = self.world.get(id) else { return };
        if o.kind != ObjectKind::Creature {
            return;
        }
        let (pos, area) = (o.position, o.area);
        let now: Vec<ObjectId> = self
            .world
            .objects
            .values()
            .filter(|t| {
                t.kind == ObjectKind::Trigger
                    && t.active
                    && t.area == area
                    && crate::engine::in_trigger(pos, t)
            })
            .map(|t| t.id)
            .collect();
        let before = self.world.in_triggers.get(&id).cloned().unwrap_or_default();
        for (list, name) in [(&now, "ENTER"), (&before, "EXIT")] {
            let other = if name == "ENTER" { &before } else { &now };
            for &t in list.iter().filter(|t| !other.contains(t)) {
                let mut ev = self.new_event(name, Some(id));
                ev.objects = vec![id];
                self.world.queue_event(0, t, ev, None);
            }
        }
        if now.is_empty() {
            self.world.in_triggers.remove(&id);
        } else {
            self.world.in_triggers.insert(id, now);
        }
    }

    /// Re-arms a heartbeat after it fires, while the object still has one.
    fn next_heartbeat(&mut self, target: ObjectId, ty: i32) {
        if ty != self.event_type("HEARTBEAT2") || ty == 0 {
            return;
        }
        let Some(ms) = self
            .world
            .get(target)
            .and_then(|o| o.fields.get("HeartbeatMs").copied())
        else {
            return;
        };
        let ev = self.new_event("HEARTBEAT2", Some(target));
        self.world.queue_event(ms.max(1) as u64, target, ev, None);
    }

    /// Puts every creature that is moving (a started `CommandMoveTo*` at the front of its
    /// queue) where it has got to by now, on a straight line at the command's pace. A move
    /// that is replaced before it finishes so starts the next one from there, as continuous
    /// movement does, crossing the triggers on the way.
    /// The route a mover takes from where it is to `to` in its area: the area's pathing
    /// grid's route (see `nav::route`), as waypoints ending at `to`; empty (a straight line)
    /// when there is no grid, either end is off it or no route exists.
    ///
    /// Closed doors block the way (see `Sim::closed_doors`): when the only route passes
    /// through one, the result is that door (the original fails such a move with
    /// `COMMAND_FAILED_PATH_ACTION_REQUIRED`, and the AI opens the door and tries again).
    pub(crate) fn route_to(&mut self, id: ObjectId, to: [f32; 3]) -> crate::nav::Routed {
        use crate::nav::Routed;
        let Some((from, Some(area))) = self.world.get(id).map(|o| (o.position, o.area)) else {
            return Routed::Straight;
        };
        let Some(g) = self.nav_grid(area) else {
            return Routed::Straight;
        };
        let doors = self.closed_doors(area);
        let blocked: std::collections::HashSet<usize> =
            doors.iter().flat_map(|d| d.1.iter().copied()).collect();
        if let Some(p) = crate::nav::route_avoiding(&g, from, to, &blocked) {
            return Routed::Path(p);
        }
        // Through closed doors that can be opened rather than locked ones, when such a route
        // exists (provisional: the original's pathfinder is not traced; this keeps a mover from
        // walking up to a locked door when another way is open to it).
        let locked: std::collections::HashSet<usize> = doors
            .iter()
            .filter(|d| {
                self.world.get(d.0).is_some_and(|o| {
                    o.fields
                        .get("State")
                        .or_else(|| o.fields.get("AnimContState"))
                        .copied()
                        .unwrap_or(0)
                        == 1
                })
            })
            .flat_map(|d| d.1.iter().copied())
            .collect();
        let unlocked_route = if locked.is_empty() {
            None
        } else {
            crate::nav::route_avoiding(&g, from, to, &locked)
        };
        let Some(p) = unlocked_route.or_else(|| crate::nav::route(&g, from, to)) else {
            return Routed::Straight;
        };
        if blocked.is_empty() {
            return Routed::Path(p);
        }
        let cell_of = |q: [f32; 3]| g.cell(q).map(|c| (c.1 * g.columns + c.0) as usize);
        // A destination inside a closed door (walking to the door itself) is reached at the
        // door: the walk ends where the route would enter it.
        let goal_door = cell_of(to).and_then(|i| doors.iter().find(|d| d.1.contains(&i)));
        // The first closed door the open-door route goes through.
        for (n, q) in p.iter().enumerate() {
            if let Some(i) = cell_of(*q) {
                if let Some((door, _)) = doors.iter().find(|d| d.1.contains(&i)) {
                    if goal_door.is_some_and(|g| g.0 == *door) {
                        let mut up_to = p[..n].to_vec();
                        if up_to.is_empty() {
                            up_to.push(from);
                        }
                        return Routed::Path(up_to);
                    }
                    return Routed::Door(*door);
                }
            }
        }
        Routed::Path(p)
    }

    /// A creature's collision capsule radius: 0.6 of its personal space, at least 0.1 m
    /// (DAOrigins.exe's character controller, set up at 0x6b1fb7).
    pub fn capsule_radius(&self, id: ObjectId) -> f32 {
        (0.6 * self.body_radius(id)).max(0.1)
    }

    /// Where a creature walking from `a` toward `b` ends up when other creatures' bodies are
    /// in the way, or `None` when none is. As DAOrigins.exe moves its creatures (0x6b6880
    /// calling the PhysX controller's move, 0x838240, whose active groups take in the other
    /// controllers): a living creature's capsule blocks every other creature's, the walker
    /// slides along it, nothing is pushed, and there is no steering round it. Dead creatures
    /// do not block (their controllers stop interacting: 0x6a0df0). Bodies are circles on
    /// the ground. Provisional: one slide per body met, in turn, rather than PhysX's
    /// iterated sweep; the step offset (0.5 m) is not modelled.
    fn slide_past_bodies(&self, id: ObjectId, a: [f32; 3], b: [f32; 3]) -> Option<[f32; 3]> {
        let me = self.world.get(id)?;
        let area = me.area?;
        let mine = self.capsule_radius(id);
        let reach = crate::engine::dist(a, b) + mine + 2.0;
        let bodies: Vec<([f32; 2], f32)> = self
            .world
            .objects
            .values()
            .filter(|o| {
                o.id != id
                    && o.kind == crate::world::ObjectKind::Creature
                    && o.area == Some(area)
                    && o.active
                    && !o.dead
                    && o.parked.is_none()
                    && ((o.position[0] - a[0]).powi(2) + (o.position[1] - a[1]).powi(2)).sqrt()
                        <= reach
            })
            .map(|o| {
                (
                    [o.position[0], o.position[1]],
                    mine + self.capsule_radius(o.id),
                )
            })
            .collect();
        if bodies.is_empty() {
            return None;
        }
        let mut at = [a[0], a[1]];
        let mut d = [b[0] - a[0], b[1] - a[1]];
        let mut hit = false;
        for (c, r) in bodies {
            if let Some((q, rest)) = slide_circle(at, d, c, r) {
                hit = true;
                at = q;
                d = rest;
            }
        }
        if !hit {
            return None;
        }
        Some([at[0] + d[0], at[1] + d[1], b[2]])
    }

    /// Keeps track of a walker held up by bodies, logging once when it has made no progress
    /// for [`BLOCKED_REPORT_MS`] (for finding stuck walkers).
    fn note_blocked(&mut self, id: ObjectId, progressed: bool) {
        let now = self.world.time_ms;
        let e = self.blocked_since.entry(id).or_insert((now, false));
        if progressed {
            *e = (now, false);
            return;
        }
        if !e.1 && now.saturating_sub(e.0) >= BLOCKED_REPORT_MS {
            e.1 = true;
            let tag = self
                .world
                .get(id)
                .map(|o| o.tag.clone())
                .unwrap_or_default();
            let text = format!("walker {id} ({tag}) blocked by bodies for {} ms", now - e.0);
            self.world.trace.push(crate::trace::TraceEntry::Log {
                t: now,
                channel: 0,
                text,
            });
        }
    }

    fn advance_movers(&mut self) {
        let now = self.world.time_ms;
        // (mover, start, start time, due, the points after the start)
        type Move = (ObjectId, [f32; 3], u64, u64, Vec<[f32; 3]>);
        let moving: Vec<Move> =
            self.world
                .objects
                .values()
                .filter_map(|o| {
                    let c = o.commands.first()?;
                    let (from, t0) = c.from?;
                    let due = c.due?;
                    let points = if c.path.is_empty() {
                        vec![c.approach.or(c.location.map(|l| l.position)).or_else(|| {
                            c.target.and_then(|t| self.world.get(t)).map(|t| t.position)
                        })?]
                    } else {
                        c.path.clone()
                    };
                    Some((o.id, from, t0, due, points))
                })
                .collect();
        for (id, from, t0, due, points) in moving {
            if due <= t0 {
                continue;
            }
            let f = (now.saturating_sub(t0) as f32 / (due - t0) as f32).clamp(0.0, 1.0);
            let mut p = along(from, &points, f);
            // A mover faces the way it goes, leg by leg.
            let dir = direction_along(from, &points, f);
            let Some(was) = self.world.get(id).map(|o| o.position) else {
                continue;
            };
            // Other creatures' bodies stop it: it slides along them, and its move carries on
            // from where it got to.
            if let Some(slid) = self.slide_past_bodies(id, was, p) {
                p = slid;
                // From where the walker was, not where it meant to get to: a waypoint the
                // blocked step would have passed is still ahead of it.
                let rest = remaining_points(from, &points, progress_of(from, &points, was));
                // At the speed the move was set going at (a walk stays a walk).
                let speed = (path_length(from, &points) * 1000.0 / (due - t0) as f32).max(0.01);
                let left = path_length(p, &rest);
                let progressed = crate::engine::dist(was, p) > 0.01;
                if let Some(c) = self.world.get_mut(id).and_then(|o| o.commands.first_mut()) {
                    c.from = Some((p, now));
                    c.due = Some(now + ((left / speed) * 1000.0).max(1.0) as u64);
                    c.path = rest;
                }
                self.note_blocked(id, progressed);
            } else {
                self.blocked_since.remove(&id);
            }
            // Triggers are crossed on the way, not only where a move ends: a move replaced
            // before it finishes (a held stick re-aiming the hero ahead, a new click) still
            // enters and leaves what it walked through. Sampled every half metre between
            // where the mover was and where it is now. Provisional: DAOrigins.exe's trigger
            // test during locomotion is not traced.
            let steps = (crate::engine::dist(was, p) / TRIGGER_STEP).ceil() as usize;
            for i in 1..steps {
                let t = i as f32 / steps as f32;
                let at = [
                    was[0] + (p[0] - was[0]) * t,
                    was[1] + (p[1] - was[1]) * t,
                    was[2] + (p[2] - was[2]) * t,
                ];
                if let Some(o) = self.world.get_mut(id) {
                    o.position = at;
                }
                self.update_triggers(id);
            }
            if let Some(o) = self.world.get_mut(id) {
                o.position = p;
                if let Some((dx, dy)) = dir {
                    o.facing = crate::world::facing_toward(dx, dy);
                }
            }
            if was != p {
                self.update_triggers(id);
            }
        }
    }

    /// When walkers closing in on a creature next look again (see [`APPROACH_CHECK_MS`]):
    /// `None` when nobody is.
    fn approach_check_due(&self) -> Option<u64> {
        self.world
            .objects
            .values()
            .any(|o| {
                o.commands.first().is_some_and(|c| {
                    c.approach.is_some()
                        && c.from.is_some()
                        && c.target.is_some_and(|t| t != o.id)
                        && (c.name == "CommandAttack" || c.name == "CommandUseAbility")
                })
            })
            // On a fixed grid of world time, so that ticks shorter than the period (the app's
            // 30 fps steps of 33 ms) still reach a check: one counted from "now" would always
            // fall past the end of a short tick, and walkers would never look again.
            .then_some((self.world.time_ms / APPROACH_CHECK_MS + 1) * APPROACH_CHECK_MS)
    }

    /// Stops a walker whose target has come within reach (its attack or ability starts
    /// next), and sends one whose target has moved off toward the new place, from where it
    /// stands. DAOrigins.exe's attack start step (0x7a87c0) tests the creatures' current
    /// positions with the melee range test (0x6a3dc0), so a target that walks up to its
    /// attacker is in reach at once; when the rest of the walk is re-planned is provisional
    /// ([`APPROACH_REPLAN`]).
    fn close_approaches(&mut self) {
        let walkers: Vec<(ObjectId, Command)> = self
            .world
            .objects
            .values()
            .filter_map(|o| {
                let c = o.commands.first()?;
                (c.approach.is_some()
                    && c.from.is_some()
                    && c.target.is_some_and(|t| t != o.id)
                    && (c.name == "CommandAttack" || c.name == "CommandUseAbility"))
                    .then(|| (o.id, c.clone()))
            })
            .collect();
        for (id, cmd) in walkers {
            let now = if cmd.name == "CommandAttack" {
                self.approach_point(id, &cmd)
            } else {
                self.ability_approach(id, &cmd)
            };
            // A melee attacker under way walks on until the target is within the bodies'
            // reach itself; the test's slack (`MELEE_REACH_TOLERANCE`) only spares a walk
            // to one that is already that close. Provisional.
            // So does one walking up to use a personal-range talent (its slack being
            // `MELEE_TALENT_TOLERANCE`).
            let melee_reach = match cmd.target {
                Some(t) if cmd.name == "CommandAttack" => (self.attack_reach(id)
                    <= crate::combat::MELEE_REACH)
                    .then(|| self.reach_to(id, t)),
                Some(t) if self.abi_int(cmd.ints.first().copied().unwrap_or(0), "range") == 0 => {
                    Some(self.body_radius(id) + self.body_radius(t) + MELEE_TALENT_REACH)
                }
                _ => None,
            };
            let reached = match (now, melee_reach, cmd.target) {
                (Approach::InReach, Some(reach), Some(t)) => {
                    let (a, b) = (
                        self.world.get(id).map(|o| o.position).unwrap_or_default(),
                        self.world.get(t).map(|o| o.position).unwrap_or_default(),
                    );
                    ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt() <= reach + 0.001
                }
                (Approach::InReach, _, _) => true,
                _ => false,
            };
            let again = match now {
                Approach::InReach => reached,
                Approach::Walk(_, p) => cmd.approach.is_some_and(|q| {
                    ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2)).sqrt() > APPROACH_REPLAN
                }),
                _ => false,
            };
            if again {
                if let Some(c) = self.world.get_mut(id).and_then(|o| o.commands.first_mut()) {
                    c.approach = None;
                    c.due = None;
                    c.from = None;
                    c.path.clear();
                }
            }
        }
    }

    /// Where an attacker goes before its attack can start. Ranged attackers stop at range
    /// on the straight line. Melee attackers walk to a place on the target's melee ring, as
    /// DAOrigins.exe's attack command does (0x742fd0 queues move-to-object 0x7aa560, then
    /// the start step 0x7a87c0): the ring has 40 slots 9 degrees apart at the two bodies'
    /// reach (`Sim::reach_to`); the attacker takes the free span nearest its bearing
    /// (0x7cc2b0: searching +1, -1, +2, -2 ...), a span being the slots its body covers
    /// (`melee_slot_span`), and slots being held by the target's other melee attackers. The
    /// walk is skipped when the attacker is within the reach and 1.25 m of its place
    /// (0x6a3d00); after the walk the start step allows the 1 m slack.
    ///
    /// Provisional: the exe reserves slots explicitly (0x7cb0e0) and fails the command when
    /// none is free (`COMMAND_FAILED_NO_SPACE_IN_MELEE_RING`); here an attacker's place is
    /// read from where it is heading or standing, and one that finds the ring full waits by
    /// it once (walking to just outside it, or standing a second) before its attack fails.
    /// The appearance's ring offset (apr_base column 0xed58b0c9, 0 for almost every row)
    /// and the extra rings of meleering1..9.gda (three bosses) are not read.
    fn approach_point(&self, id: ObjectId, cmd: &Command) -> Approach {
        let (Some(me), Some(target)) = (self.world.get(id), cmd.target) else {
            return Approach::InReach;
        };
        let Some(t) = self.world.get(target) else {
            return Approach::InReach;
        };
        if me.area != t.area {
            return Approach::InReach;
        }
        let (a, b) = (me.position, t.position);
        let d = crate::engine::dist(a, b);
        let reach = self.reach_to(id, target);
        let ranged = self.attack_reach(id) > crate::combat::MELEE_REACH;
        // Once the walk to its place has ended, the attack's start step (0x7a87c0) tests the
        // range with the executable's slack, across the ground (0x6a3dc0).
        let ground = ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
        if ranged && d <= reach + 0.05 {
            return Approach::InReach;
        }
        if !ranged && cmd.arrived && ground <= reach + crate::combat::MELEE_REACH_TOLERANCE {
            return Approach::InReach;
        }
        if ranged {
            let f = (d - reach) / d;
            let p = [
                a[0] + (b[0] - a[0]) * f,
                a[1] + (b[1] - a[1]) * f,
                a[2] + (b[2] - a[2]) * f,
            ];
            return Approach::Walk(d - reach, p);
        }
        // The other melee attackers of the same target hold their places on its ring.
        let mine = self.body_radius(id);
        let target_r = self.body_radius(target);
        let held: Vec<(i32, i32)> = self
            .world
            .objects
            .values()
            .filter(|o| {
                o.id != id
                    && o.id != target
                    && o.kind == crate::world::ObjectKind::Creature
                    && o.area == me.area
                    && o.active
                    && !o.dead
                    && self.attack_reach(o.id) <= crate::combat::MELEE_REACH
            })
            .filter_map(|o| {
                let c = o.commands.first()?;
                (c.name == "CommandAttack" && c.target == Some(target)).then_some(())?;
                let p = c.approach.unwrap_or(o.position);
                let r = self.body_radius(o.id);
                Some((melee_slot_toward(b, p), melee_slot_span(target_r, r)))
            })
            .collect();
        let span = melee_slot_span(target_r, mine);
        let free = |slot: i32| {
            held.iter().all(|&(s, k)| {
                let gap = (slot - s).rem_euclid(MELEE_RING_SLOTS);
                gap.min(MELEE_RING_SLOTS - gap) > span + k
            })
        };
        let base = melee_slot_toward(b, a);
        let ring = reach;
        let at = |slot: i32| {
            let ang = (slot as f32 * MELEE_RING_STEP_DEG).to_radians();
            [b[0] + ring * ang.cos(), b[1] + ring * ang.sin(), b[2]]
        };
        let slot = (0..=MELEE_RING_SLOTS / 2)
            .flat_map(|k| [k, -k])
            .map(|k| base + k)
            .find(|&s| free(s))
            .map(at);
        // The walk is skipped when the attacker is within the bodies' reach (without the
        // slack) and by its place already (0x6a3d00, 0x7a3090).
        if let Some(p) = slot {
            let off = ((p[0] - a[0]).powi(2) + (p[1] - a[1]).powi(2)).sqrt();
            if ground <= reach && off <= MELEE_GAP_AT_SLOT {
                return Approach::InReach;
            }
        }
        match slot {
            Some(p) => Approach::Walk(crate::engine::dist(a, p), p),
            None if cmd.ring_wait => Approach::Full,
            None => {
                let ang = (base as f32 * MELEE_RING_STEP_DEG).to_radians();
                let r = ring + 2.0 * mine + 0.3;
                let p = [b[0] + r * ang.cos(), b[1] + r * ang.sin(), b[2]];
                let walk = crate::engine::dist(a, p);
                if walk < 0.3 {
                    Approach::Wait(a)
                } else {
                    Approach::WaitAt(walk, p)
                }
            }
        }
    }

    /// Where a caster goes before its ability can start: within the ability's range
    /// (`Sim::ability_range`) of its target, or of the place it is aimed at, on the straight
    /// line toward it. Provisional: the range is measured from the caster's centre to the
    /// target's body (its centre plus its radius), and a cone aimed at a place (the place
    /// only gives its direction) is used from where the caster stands.
    ///
    /// An ability of personal range (`ABI_base` `range` row 0, the melee talents: Shield Bash,
    /// Pummel, Cripple) aimed at another creature needs its target within melee reach: the
    /// two creatures' radii, 1.2 m and a metre's tolerance across the ground, which
    /// DAOrigins.exe's check for the command requires (0x7a7525 calling 0x6a3dc0 and
    /// 0x695320; constants 0xafebc0, 0xade218). The caster walks until the target is within
    /// the radii and 1.2 m. Provisional: that the engine walks the caster up when the check
    /// fails (rather than failing the command) is inferred from the game's AI, which orders
    /// these talents at enemies out of reach and does not otherwise close in; the radii are
    /// taken to be `PERSPACE` ([`Sim::body_radius`]).
    fn ability_approach(&self, id: ObjectId, cmd: &Command) -> Approach {
        let ability = cmd.ints.first().copied().unwrap_or(0);
        let Some(me) = self.world.get(id) else {
            return Approach::InReach;
        };
        if self.abi_int(ability, "range") == 0 {
            let target = cmd
                .target
                .filter(|&t| t != id && t != OBJECT_INVALID)
                .and_then(|t| self.world.get(t))
                .filter(|o| o.kind == ObjectKind::Creature && o.area == me.area);
            if let Some(t) = target {
                let (dx, dy) = (
                    t.position[0] - me.position[0],
                    t.position[1] - me.position[1],
                );
                let d = (dx * dx + dy * dy).sqrt();
                let reach = self.body_radius(id) + self.body_radius(t.id) + MELEE_TALENT_REACH;
                if d.is_finite() && d > reach + MELEE_TALENT_TOLERANCE {
                    let f = (d - reach) / d;
                    let a = me.position;
                    let p = [
                        a[0] + dx * f,
                        a[1] + dy * f,
                        a[2] + (t.position[2] - a[2]) * f,
                    ];
                    return Approach::Walk(d - reach, p);
                }
            }
            return Approach::InReach;
        }
        let Some(range) = self.ability_range(ability) else {
            return Approach::InReach;
        };
        let (to, reach) = match cmd.target.filter(|&t| t != id && t != OBJECT_INVALID) {
            Some(t) => match self.world.get(t) {
                Some(o) if o.area == me.area => (o.position, range + self.body_radius(t)),
                _ => return Approach::InReach,
            },
            None => match cmd.location {
                Some(l)
                    if Some(l.area) == me.area
                        && !matches!(
                            self.ability_area(ability),
                            crate::quickbar::AbilityArea::Cone(..)
                        ) =>
                {
                    (l.position, range)
                }
                _ => return Approach::InReach,
            },
        };
        let a = me.position;
        let d = crate::engine::dist(a, to);
        if !d.is_finite() || d <= reach + 0.05 {
            return Approach::InReach;
        }
        let f = (d - reach) / d;
        let p = [
            a[0] + (to[0] - a[0]) * f,
            a[1] + (to[1] - a[1]) * f,
            a[2] + (to[2] - a[2]) * f,
        ];
        Approach::Walk(d - reach, p)
    }

    /// Drops an object's front command as failed: it hears `COMMAND_COMPLETE` with the
    /// result (a `COMMAND_FAILED_*` constant) one update later ([`APPROACH_CHECK_MS`]).
    ///
    /// Every caller fails a command as it starts (its way blocked by a closed door, or no
    /// room on the target's melee ring), before any time has passed. DAOrigins.exe runs a
    /// command's executor once per update, so a script that answers the failure by queueing
    /// the same command again (rules_core does, for a follower on autopilot whose target
    /// stands behind a closed door: only the controlled character and non-followers use the
    /// door) tries again on a later update while the game goes on. Answered at once, the
    /// command failed and was queued again forever at the same instant and the simulation
    /// stopped. Provisional: the delay is the simulation's stand-in for an update, not a
    /// value read from the executable.
    /// Fails a move or approach whose way a closed door blocks
    /// (`COMMAND_FAILED_PATH_ACTION_REQUIRED`, the door as the event's object 2, where
    /// rules_core looks for it: its AI then uses the door and repeats the command).
    pub(crate) fn fail_blocked(&mut self, id: ObjectId, door: ObjectId) {
        self.fail_front_command_with(id, "COMMAND_FAILED_PATH_ACTION_REQUIRED", Some(door));
    }

    pub(crate) fn fail_front_command(&mut self, id: ObjectId, result: &str) {
        self.fail_front_command_with(id, result, None);
    }

    fn fail_front_command_with(&mut self, id: ObjectId, result: &str, blocker: Option<ObjectId>) {
        let result = constant(&self.table, result);
        let Some(c) = self
            .world
            .get_mut(id)
            .filter(|o| !o.commands.is_empty())
            .map(|o| o.commands.remove(0))
        else {
            return;
        };
        let t = self.world.time_ms;
        self.world.trace.push(TraceEntry::Command {
            t,
            object: id,
            command: c.name.clone(),
            status: format!("failed {result}"),
        });
        let mut ev = self.new_event("COMMAND_COMPLETE", Some(id));
        ev.ints = vec![c.ty, result];
        if let Some(b) = blocker {
            ev.objects = vec![OBJECT_INVALID, OBJECT_INVALID, b];
        }
        if let Some(o) = self.world.get_mut(id) {
            o.combat.previous_command = Some(c);
        }
        self.world.queue_event(APPROACH_CHECK_MS, id, ev, None);
    }

    /// The earliest-finishing front command, starting the clock on fronts that have none.
    fn next_command(&mut self) -> Option<(u64, ObjectId)> {
        let now = self.world.time_ms;
        let fronts: Vec<(ObjectId, Command)> = self
            .world
            .objects
            .values()
            .filter_map(|o| o.commands.first().map(|c| (o.id, c.clone())))
            .collect();
        let mut best: Option<(u64, ObjectId)> = None;
        for (id, cmd) in fronts {
            let due = match cmd.due {
                Some(d) => d,
                None => {
                    // An attack asks its owner's script for the result first, which may
                    // also set how long it takes.
                    // An attacker out of reach walks up first (the attack starts when it
                    // gets there).
                    // So does a caster out of its ability's range.
                    let approach = match cmd.name.as_str() {
                        "CommandAttack" => Some(self.approach_point(id, &cmd)),
                        "CommandUseAbility" => Some(self.ability_approach(id, &cmd)),
                        _ => None,
                    };
                    if let Some(approach) = approach {
                        let (walk, stop, wait) = match approach {
                            Approach::InReach => (None, None, false),
                            Approach::Walk(d, p) => (Some(d), Some(p), false),
                            Approach::WaitAt(d, p) => (Some(d), Some(p), true),
                            Approach::Wait(p) => (None, Some(p), true),
                            Approach::Full => {
                                self.fail_front_command(
                                    id,
                                    "COMMAND_FAILED_NO_SPACE_IN_MELEE_RING",
                                );
                                continue;
                            }
                        };
                        if let Some(stop) = stop {
                            let pos = self.world.get(id).map(|o| o.position);
                            let path = if walk.is_some() {
                                match self.route_to(id, stop) {
                                    crate::nav::Routed::Path(p) => p,
                                    crate::nav::Routed::Straight => Vec::new(),
                                    crate::nav::Routed::Door(door) => {
                                        self.fail_blocked(id, door);
                                        continue;
                                    }
                                }
                            } else {
                                Vec::new()
                            };
                            let walk = walk.map(|d| match pos {
                                Some(p) if !path.is_empty() => path_length(p, &path),
                                _ => d,
                            });
                            let ms = match walk {
                                Some(d) => ((d / self.move_speed(id)) * 1000.0).max(1.0) as u64,
                                None => 1000,
                            };
                            let due = now + ms;
                            if let Some(c) =
                                self.world.get_mut(id).and_then(|o| o.commands.first_mut())
                            {
                                c.due = Some(due);
                                c.approach = Some(stop);
                                c.from = pos.map(|p| (p, now));
                                c.ring_wait |= wait;
                                c.arrived = false;
                                c.path = path;
                            }
                            if best.is_none_or(|(b, bid)| (due, id) < (b, bid)) {
                                best = Some((due, id));
                            }
                            continue;
                        }
                    }
                    let attack_ms = if cmd.name == "CommandAttack"
                        || cmd.name == "CommandUseAbility"
                    {
                        if cmd.name == "CommandAttack" {
                            self.start_attack(id, cmd.target.unwrap_or(OBJECT_INVALID), cmd.ty);
                        } else {
                            // The aim loop (`SetAimLoopDuration`) belongs to a ranged attack's
                            // executor (DAOrigins.exe 0x7a244b); one left from the last shot
                            // does not lengthen an ability.
                            if let Some(o) = self.world.get_mut(id) {
                                o.combat.attack_ms = None;
                                o.combat.aim_ms = None;
                            }
                            self.start_ability(id, &cmd);
                        }
                        let still = self.world.get(id).and_then(|o| o.commands.first()).cloned();
                        if still.as_ref().map(|c| (&c.name, c.target))
                            != Some((&cmd.name, cmd.target))
                        {
                            continue;
                        }
                        // A ranged attack draws (the attack duration), aims
                        // (SetAimLoopDuration), then shoots (its fire animation; see
                        // `Sim::fire_ms`).
                        let fire = if cmd.name == "CommandAttack" {
                            self.fire_ms(id)
                        } else {
                            0
                        };
                        // A melee swing the rules gave no duration lasts its animation.
                        let set = self.world.get(id).and_then(|o| o.combat.attack_ms);
                        let unset = if set.is_none() && cmd.name == "CommandAttack" && fire == 0 {
                            self.melee_attack_ms(id)
                        } else {
                            None
                        };
                        // An ability's impact comes at its cast animation's impact event, and
                        // the command runs on to the animation's end (see
                        // `Sim::ability_timing`); without the animations it keeps the default.
                        let timing = if set.is_none() && cmd.name == "CommandUseAbility" {
                            let ability = cmd.ints.first().copied().unwrap_or(0);
                            self.ability_timing(id, ability)
                        } else {
                            None
                        };
                        if let Some(t) = timing {
                            if let Some(c) =
                                self.world.get_mut(id).and_then(|o| o.commands.first_mut())
                            {
                                c.after_impact_ms = Some(t.end_ms.saturating_sub(t.impact_ms));
                            }
                        }
                        let unset = unset.or(timing.map(|t| t.impact_ms));
                        Some(
                            self.world
                                .get(id)
                                .map_or(crate::combat::DEFAULT_ATTACK_MS, |o| {
                                    set.or(unset).unwrap_or(crate::combat::DEFAULT_ATTACK_MS)
                                        + o.combat.aim_ms.unwrap_or(0)
                                })
                                + fire,
                        )
                    } else {
                        None
                    };
                    // A move follows the area's pathing grid (see `route_to`) and takes as
                    // long as its route at the mover's speed.
                    let pos = self.world.get(id).map(|o| o.position);
                    let mut path = Vec::new();
                    let mut ms = attack_ms.unwrap_or_else(|| self.command_duration(id, &cmd));
                    if cmd.name.starts_with("CommandMoveTo") {
                        let to = cmd.location.map(|l| l.position).or_else(|| {
                            cmd.target
                                .and_then(|t| self.world.get(t))
                                .map(|o| o.position)
                        });
                        if let (Some(to), Some(p)) = (to, pos) {
                            path = match self.route_to(id, to) {
                                crate::nav::Routed::Path(p) => p,
                                crate::nav::Routed::Straight => Vec::new(),
                                crate::nav::Routed::Door(door) => {
                                    self.fail_blocked(id, door);
                                    continue;
                                }
                            };
                            if !path.is_empty() {
                                ms = (path_length(p, &path) / self.command_speed(id, &cmd) * 1000.0)
                                    as u64;
                            }
                        }
                    }
                    let d = now + ms.max(1);
                    if let Some(c) = self.world.get_mut(id).and_then(|o| o.commands.first_mut()) {
                        c.due = Some(d);
                        if c.name.starts_with("CommandMoveTo") {
                            c.from = pos.map(|p| (p, now));
                            c.path = path;
                        }
                    }
                    d
                }
            };
            if best.is_none_or(|(b, bid)| (due, id) < (b, bid)) {
                best = Some((due, id));
            }
        }
        best
    }

    /// Completes an object's front command: applies its movement and tells the object.
    fn finish_command(&mut self, id: ObjectId) {
        let Some(cmd) = self.world.get_mut(id).and_then(|o| {
            if o.commands.is_empty() {
                None
            } else {
                Some(o.commands.remove(0))
            }
        }) else {
            return;
        };
        // A move or jump to an object ends where the object stands (the Fade's mouse holes
        // jump the hero to a waypoint this way). Provisional: the jumper keeps its facing.
        let to_object =
            cmd.name.starts_with("CommandMoveToObject") || cmd.name == "CommandJumpToObject";
        if let (Some(t), true) = (cmd.target, to_object) {
            if let Some(p) = self.world.get(t).map(|o| (o.position, o.area)) {
                if let Some(obj) = self.world.get_mut(id) {
                    obj.position = p.0;
                    obj.area = p.1;
                }
            }
        }
        // A command's location is where it goes, except an ability's, which is where it lands.
        if let Some(l) = cmd.location.filter(|_| cmd.name != "CommandUseAbility") {
            // Cross the triggers between where the move began and its end on the way (along
            // its route, or a straight line).
            let here = self.world.get(id).map(|o| (o.position, o.area));
            if let Some((now_at, Some(a))) = here.filter(|f| f.1 == Some(l.area)) {
                // A move that was under way has crossed what it walked through already
                // (`advance_movers`); only the rest, to its end, is left.
                let from = now_at;
                let points = if cmd.path.is_empty() || cmd.from.is_some() {
                    vec![l.position]
                } else {
                    cmd.path.clone()
                };
                let steps = (path_length(from, &points) / 0.5).ceil() as usize;
                for i in 1..steps {
                    let p = along(from, &points, i as f32 / steps as f32);
                    self.move_object(id, p, Some(a));
                }
            }
            if let Some(obj) = self.world.get_mut(id) {
                obj.position = l.position;
                obj.area = Some(l.area);
                obj.facing = l.facing;
            }
        }
        self.update_triggers(id);
        if cmd.name == "CommandAttack" {
            self.finish_attack(id, cmd.target.unwrap_or(OBJECT_INVALID));
        } else if cmd.name == "CommandUseAbility" && !cmd.impacted {
            self.finish_ability(id, &cmd);
        } else if cmd.name == "CommandStartConversation" {
            // DAOrigins.exe's executor (0x7a3700) sends the target EVENT_TYPE_DIALOGUE (18),
            // its creator the one talking and its resource the conversation, unless the target
            // is dead; rules_core answers it with UT_Talk, which begins the conversation.
            let target = cmd
                .target
                .filter(|&t| self.world.get(t).is_some_and(|o| !o.dead));
            if let Some(t) = target {
                let mut ev = self.new_event("DIALOGUE", Some(id));
                ev.resources = vec![cmd.strings.first().cloned().unwrap_or_default()];
                ev.objects = vec![id];
                self.world.queue_event(0, t, ev, None);
            }
        } else if cmd.name == "CommandSwitchWeaponSet" {
            // The command's executor (DAOrigins.exe 0x7a1ed0) switches the set (0x6a8fa0)
            // and reports itself done at once.
            let set = cmd.ints.first().copied().unwrap_or(-1);
            self.switch_weapon_set(id, set);
        } else if cmd.name == "CommandUseObject" {
            // Using a placeable (a door, a lever) sends it EVENT_TYPE_USE, as clicking it does.
            if let Some(t) = cmd.target.filter(|&t| self.world.get(t).is_some()) {
                let ev = self.use_event(id, t);
                self.deliver(t, ev, None);
            }
        }
        if let Some(o) = self.world.get_mut(id) {
            o.combat.previous_command = Some(cmd.clone());
        }
        self.update_perception();
        self.world.trace.push(TraceEntry::Command {
            t: self.world.time_ms,
            object: id,
            command: cmd.name.clone(),
            status: "complete".into(),
        });
        let mut ev = self.new_event("COMMAND_COMPLETE", Some(id));
        ev.ints = vec![cmd.ty, 1];
        self.world.queue_event(0, id, ev, None);
    }

    // -----------------------------------------------------------------------------------------
    // Loading

    pub(crate) fn read_gff3(&self, name: &str) -> Result<Gff3, SimError> {
        let (data, _) = self
            .vfs
            .read(name)
            .map_err(|_| SimError::NotFound(name.to_string()))?;
        Gff3::parse(&data).map_err(|e| SimError::Parse(name.to_string(), e.to_string()))
    }

    /// Loads a module's campaign file (`modules/<dir>/*.cif`): module variables, script, start
    /// area and the area tree. Creates the module object, the hero and the party.
    pub fn load_module(&mut self, install_root: &Path, module_dir: &str) -> Result<(), SimError> {
        self.install_root = Some(install_root.to_path_buf());
        let dir = install_root.join("modules").join(module_dir);
        let cif = std::fs::read_dir(&dir)
            .map_err(|_| SimError::NotFound(dir.display().to_string()))?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("cif")))
            .ok_or_else(|| SimError::NotFound(format!("{module_dir}/*.cif")))?;
        let data = std::fs::read(&cif)
            .map_err(|e| SimError::Parse(cif.display().to_string(), e.to_string()))?;
        let g = Gff4::parse(&data).map_err(|e| SimError::Parse("cif".into(), e.to_string()))?;
        let mut info = parse_cif(&g);
        info.name = module_dir.to_string();
        let module = self.world.spawn(ObjectKind::Module, "module");
        self.created(module);
        {
            let m = self.world.get_mut(module).expect("just spawned");
            m.script = info.script.clone();
            m.template = info.script.clone().unwrap_or_default();
            for (name, value) in cif_vars(&g) {
                match value {
                    thedas_formats::gff4::Value::I32(v) => {
                        m.locals.ints.insert(name, v);
                    }
                    thedas_formats::gff4::Value::F32(v) => {
                        m.locals.floats.insert(name, v);
                    }
                    thedas_formats::gff4::Value::String(Some(s)) => {
                        m.locals
                            .strings
                            .insert(name, s.trim_end_matches('\0').to_string());
                    }
                    other => {
                        if let Some(v) = other.as_i64() {
                            m.locals.ints.insert(name, v as i32);
                        }
                    }
                }
            }
        }
        self.world.module = Some(module);
        self.load_world_maps();
        // The hero. DAOrigins.exe builds the new game's player creature from the
        // `default_player` template (the string 0xb0f21c pushed at 0x69d953, a 0xd20-byte
        // creature made at 0x69d9b8-0x69d9e6): tag `player`, `player_core`, rank 100, a human
        // male (appearance 15) in common clothes. Character creation's scripts change it from
        // there. The template's `Gold` (75) is not read: the exe has no `Gold` label, only
        // `StoreGold` (0xb244c4), so the hero starts with no money, as before.
        let party = self.world.spawn(ObjectKind::Party, "party");
        let hero = self.new_game_hero();
        self.world.hero = Some(hero);
        self.world.party = Some(party);
        self.module_info = info;
        // A new game: the module loads, then starts (provisional order).
        for name in ["MODULE_LOAD", "MODULE_START"] {
            let ev = self.new_event(name, Some(module));
            self.world.queue_event(0, module, ev, None);
        }
        Ok(())
    }

    /// The template a new game's player creature is built from (0xb0f21c, 0x69d953).
    pub const PLAYER_TEMPLATE: &'static str = "default_player";

    /// The new game's player creature: built from [`Self::PLAYER_TEMPLATE`] as the executable
    /// does. Without that template (a stripped install, synthetic tests) it is a bare creature
    /// tagged `player` with `player_core` and the player rank (provisional fallback).
    fn new_game_hero(&mut self) -> ObjectId {
        if self
            .read_gff3(&format!("{}.utc", Self::PLAYER_TEMPLATE))
            .is_ok()
        {
            return self.create_from_template(
                ObjectKind::Creature,
                Self::PLAYER_TEMPLATE,
                None,
                [0.0; 3],
                0.0,
            );
        }
        let hero = self.world.spawn(ObjectKind::Creature, "player");
        {
            let h = self.world.get_mut(hero).expect("just spawned");
            h.script = Some("player_core".into());
            h.template = "player".into();
            h.rank = constant(&self.table, "CREATURE_RANK_PLAYER");
        }
        self.created(hero);
        hero
    }

    fn created(&mut self, id: ObjectId) {
        self.lay_out_if_spawned_dead(id);
        if let Some(o) = self.world.get(id) {
            let e = TraceEntry::ObjectCreated {
                t: self.world.time_ms,
                id,
                object_kind: format!("{:?}", o.kind),
                tag: o.tag.clone(),
                template: o.template.clone(),
            };
            self.world.trace.push(e);
        }
    }

    /// `CreateObject`: an object of `kind` built from its template (`.utc`, `.utp`, ...), as
    /// area loading builds them, in `area` at `position`. Creatures get their template's
    /// items. A missing template still makes the object, traced as missing.
    pub fn create_from_template(
        &mut self,
        kind: ObjectKind,
        template: &str,
        area: Option<ObjectId>,
        position: [f32; 3],
        facing: f32,
    ) -> ObjectId {
        if kind == ObjectKind::Item {
            let id = self.create_item(template);
            if let Some(o) = self.world.get_mut(id) {
                o.area = area;
                o.position = position;
                o.facing = facing;
            }
            return id;
        }
        let ext = match kind {
            ObjectKind::Creature => "utc",
            ObjectKind::Placeable => "utp",
            ObjectKind::Trigger => "utt",
            ObjectKind::Store => "utm",
            _ => "",
        };
        let template = template.to_ascii_lowercase();
        let template = template
            .strip_suffix(&format!(".{ext}"))
            .unwrap_or(&template)
            .to_string();
        let tmpl = if ext.is_empty() {
            None
        } else {
            match self.read_gff3(&format!("{template}.{ext}")) {
                Ok(g) => Some(g),
                Err(e) => {
                    let t = self.world.time_ms;
                    self.world.trace.push(TraceEntry::Missing {
                        t,
                        what: e.to_string(),
                    });
                    None
                }
            }
        };
        let tag = tmpl
            .as_ref()
            .and_then(|g| g.structs.first().and_then(|r| text(r, "Tag")))
            .unwrap_or_else(|| template.clone());
        let id = self.world.spawn(kind, &tag);
        if let Some(o) = self.world.get_mut(id) {
            o.template = template;
            o.area = area;
            o.position = position;
            o.facing = facing;
            if let Some(t) = &tmpl {
                apply_template(o, t);
            }
        }
        if let Some(t) = &tmpl {
            match kind {
                ObjectKind::Creature | ObjectKind::Placeable => self.load_template_items(id, t),
                ObjectKind::Store => self.load_store_items(id, t),
                _ => {}
            }
        }
        self.created(id);
        id
    }

    /// Loads an area and its objects, places the hero (at the waypoint with tag `waypoint`, or
    /// the area's first waypoint) and queues the load events.
    pub fn enter_area(
        &mut self,
        resref: &str,
        waypoint: Option<&str>,
    ) -> Result<ObjectId, SimError> {
        // An area the party has been to keeps its objects and their state.
        let known = self
            .world
            .objects
            .values()
            .find(|o| o.kind == ObjectKind::Area && o.template.eq_ignore_ascii_case(resref))
            .map(|o| o.id);
        if let Some(area) = known {
            for o in self.world.objects.values_mut() {
                if o.area == Some(area) || o.id == area {
                    if let Some(a) = o.parked.take() {
                        o.active = a;
                    }
                }
            }
            // Events that waited while the area was unloaded run now.
            let held = std::mem::take(&mut self.world.held);
            for q in held {
                if self.world.get(q.target).is_some_and(|o| o.parked.is_some()) {
                    self.world.held.push(q);
                } else {
                    self.world.queue_event(0, q.target, q.event, q.script);
                }
            }
            self.world.area = Some(area);
            self.arrive(area, waypoint, Vec::new());
            return Ok(area);
        }
        let are = self.read_gff3(&format!("{}.are", resref.to_ascii_lowercase()))?;
        let root = are
            .structs
            .first()
            .ok_or_else(|| SimError::Parse(resref.into(), "empty area".into()))?;
        let area = self.world.spawn(
            ObjectKind::Area,
            &text(root, "Tag").unwrap_or_else(|| resref.to_string()),
        );
        {
            let a = self.world.get_mut(area).expect("just spawned");
            a.template = resref.to_string();
            a.script = resref_field(root, "ScriptResRefID");
            a.name_strref = strref(root, "Name");
            read_vars(&are, root, &mut a.locals);
        }
        if let Some(layout) = text(root, "Layout").or_else(|| resref_field(root, "Layout")) {
            self.area_layouts.insert(area, layout.to_ascii_lowercase());
        }
        self.created(area);
        self.world.area = Some(area);

        let lists = [
            ("CreatureList", ObjectKind::Creature, "utc"),
            ("PlaceableList", ObjectKind::Placeable, "utp"),
            ("TriggerList", ObjectKind::Trigger, "utt"),
            ("WaypointList", ObjectKind::Waypoint, ""),
            ("StoreList", ObjectKind::Store, "utm"),
            ("SoundList", ObjectKind::Sound, ""),
            ("StageList", ObjectKind::Stage, ""),
        ];
        let mut spawned = Vec::new();
        for (list, kind, ext) in lists {
            for &si in list_of(root, list) {
                let Some(inst) = are.structs.get(si as usize) else {
                    continue;
                };
                let id = self.spawn_instance(&are, inst, kind, ext, area);
                spawned.push(id);
            }
        }

        self.arrive(area, waypoint, spawned);
        Ok(area)
    }

    /// The party arrives in an area: the hero stands at the waypoint (or the first one),
    /// new objects get `SPAWN`, the area its load events and the hero's `ENTER`.
    fn arrive(&mut self, area: ObjectId, waypoint: Option<&str>, spawned: Vec<ObjectId>) {
        // Place the hero.
        if let Some(hero) = self.world.hero {
            let wp = self
                .world
                .objects
                .values()
                .filter(|o| o.kind == ObjectKind::Waypoint && o.area == Some(area))
                .find(|o| waypoint.is_none_or(|w| o.tag.eq_ignore_ascii_case(w)))
                .map(|o| (o.position, o.facing));
            let h = self.world.get_mut(hero).expect("hero exists");
            h.area = Some(area);
            if let Some((p, f)) = wp {
                h.position = p;
                h.facing = f;
            }
        }

        // Load events (order provisional, see the module docs).
        for id in &spawned {
            let kind = self.world.get(*id).map(|o| o.kind);
            if matches!(kind, Some(ObjectKind::Creature | ObjectKind::Placeable)) {
                let ev = self.new_event("SPAWN", Some(*id));
                self.world.queue_event(0, *id, ev, None);
            }
        }
        for name in ["AREALOAD_SPECIAL", "AREALOAD_PRELOADEXIT"] {
            let ev = self.new_event(name, Some(area));
            self.world.queue_event(0, area, ev, None);
        }
        if let Some(hero) = self.world.hero {
            let mut ev = self.new_event("ENTER", Some(hero));
            ev.objects = vec![hero];
            self.world.queue_event(0, area, ev, None);
        }
        let ev = self.new_event("AREALOAD_POSTLOADEXIT", Some(area));
        self.world.queue_event(0, area, ev, None);
        // The hero starts wherever it was placed; triggers there see it enter.
        if let Some(hero) = self.world.hero {
            self.world.in_triggers.remove(&hero);
            self.update_triggers(hero);
        }
    }

    fn spawn_instance(
        &mut self,
        are: &Gff3,
        inst: &Struct,
        kind: ObjectKind,
        ext: &str,
        area: ObjectId,
    ) -> ObjectId {
        let tag = text(inst, "Tag").unwrap_or_default();
        let template = resref_field(inst, "TemplateResRef").unwrap_or_default();
        let id = self.world.spawn(kind, &tag);
        let tmpl = if !ext.is_empty() && !template.is_empty() {
            match self.read_gff3(&format!("{}.{ext}", template.to_ascii_lowercase())) {
                Ok(g) => Some(g),
                Err(e) => {
                    self.world.trace.push(TraceEntry::Missing {
                        t: self.world.time_ms,
                        what: e.to_string(),
                    });
                    None
                }
            }
        } else {
            None
        };
        let q = [
            float(inst, "XOrientation"),
            float(inst, "YOrientation"),
            float(inst, "ZOrientation"),
            float(inst, "WOrientation"),
        ];
        let o = self.world.get_mut(id).expect("just spawned");
        o.template = template;
        o.area = Some(area);
        o.position = [
            float(inst, "XPosition"),
            float(inst, "YPosition"),
            float(inst, "ZPosition"),
        ];
        // Rotation about Z from the quaternion, in degrees.
        o.facing = (2.0 * q[2].atan2(q[3])).to_degrees().rem_euclid(360.0);
        if let Some(a) = int(inst, "Active") {
            o.active = a != 0;
        }
        if let Some(t) = &tmpl {
            apply_template(o, t);
        }
        // Instance values override the template.
        if let Some(t) = int(inst, "TeamId") {
            o.team = t;
        }
        if let Some(r) = int(inst, "Rank") {
            o.rank = r;
        }
        // So do its other integer fields (the treasure category a designer sets on each
        // chest or creature placed, a lock's difficulty, ...).
        for fl in &inst.fields {
            if let Some(v) = int(inst, &fl.label) {
                o.fields.insert(fl.label.clone(), v);
            }
        }
        read_text_fields(o, inst);
        // A waypoint's map note text (a talk-table id), which the area map shows.
        if let Some(s) = strref(inst, "MapNote") {
            o.fields.insert("MapNote".into(), s as i32);
        }
        read_vars(are, inst, &mut o.locals);
        if kind == ObjectKind::Trigger {
            for &p in list_of(inst, "Geometry") {
                if let Some(s) = are.structs.get(p as usize) {
                    o.geometry
                        .push([float(s, "XPoint"), float(s, "YPoint"), float(s, "ZPoint")]);
                }
            }
        }
        // A creature stands on the ground: DAOrigins.exe sets a loaded creature's position
        // again through its physics controller (0x695540 -> 0x6b5ef0 -> 0x6b1790), which
        // projects it onto the walkable surface (0x73e370, 0x73e7f0), so one placed a little
        // above or below the floor lands on it (Dust Town's slum guard is placed 1.3 m up).
        // Provisional: the pathing grid's floor stands in for the walk surface; only a floor
        // within [`SPAWN_SNAP_RANGE`] with no level geometry in between is used, so
        // creatures placed on a level the grid does not hold keep their height.
        if kind == ObjectKind::Creature
            && let Some(p) = self.world.get(id).map(|o| o.position)
            && let Some(floor) = self.floor_at(area, p)
            && (p[2] - floor).abs() > 0.05
            && (p[2] - floor).abs() <= SPAWN_SNAP_RANGE
            // A creature on a ledge, balcony or bridge the grid does not hold has the
            // level's geometry between it and the grid's floor: it keeps its height.
            && !self.collision(area).is_some_and(|c| {
                c.blocked(
                    [p[0], p[1], p[2] + 0.05],
                    [p[0], p[1], floor + 0.05 * (p[2] - floor).signum()],
                )
            })
            && let Some(o) = self.world.get_mut(id)
        {
            o.position[2] = floor;
        }
        if let Some(t) = &tmpl {
            match kind {
                // A container's contents are its template's `ItemList` (an area's placed
                // instance carries none of its own): Everd's armour chest at the Proving
                // holds the armour and helmet the Dwarf Commoner needs.
                ObjectKind::Creature | ObjectKind::Placeable => self.load_template_items(id, t),
                ObjectKind::Store => self.load_store_items(id, t),
                _ => {}
            }
        }
        self.created(id);
        id
    }
}

/// The appearance a template gives: placeables keep it at the top, creatures in their
/// "Appearance Data" struct.
fn template_appearance(t: &Gff3) -> Option<i32> {
    let r = t.structs.first()?;
    int(r, "Appearance").or_else(|| match field(r, "Appearance Data") {
        Some(G3::Struct(i)) => t
            .structs
            .get(*i as usize)
            .and_then(|a| int(a, "Appearance_Type")),
        _ => None,
    })
}

impl Sim {
    /// The appearance creature `id`'s template gives it, when the template can be read and
    /// gives one.
    pub(crate) fn original_appearance(&self, id: ObjectId) -> Option<i32> {
        let o = self.world.get(id)?;
        if o.kind != ObjectKind::Creature || o.template.is_empty() {
            return None;
        }
        let t = self
            .read_gff3(&format!("{}.utc", o.template.to_ascii_lowercase()))
            .ok()?;
        template_appearance(&t).filter(|&a| a > 0)
    }
}

/// A template's values on an object: scripts, conversation, name, team, group, rank,
/// appearance, local variables and every integer field.
fn apply_template(o: &mut crate::world::Object, t: &Gff3) {
    let Some(r) = t.structs.first() else { return };
    o.script = resref_field(r, "EventScript").or_else(|| resref_field(r, "ScriptResRefID"));
    o.dialog = resref_field(r, "DialogResRef").or_else(|| resref_field(r, "Conversation"));
    o.name_strref = strref(r, "LocName");
    o.immortal = int(r, "Immortal").unwrap_or(0) != 0;
    o.plot = int(r, "Plot").unwrap_or(0) != 0;
    o.team = int(r, "TeamId").unwrap_or(-1);
    o.group = int(r, "GroupId").unwrap_or(0);
    o.rank = int(r, "Rank").unwrap_or(0);
    o.appearance = template_appearance(t).unwrap_or(0);
    read_vars(t, r, &mut o.locals);
    // The creature's sound set (`ss_alistair`): its barks, for the presentation.
    if let Some(ss) = resref_field(r, "SoundSet").filter(|s| !s.is_empty()) {
        o.text_fields.insert(SOUND_SET.to_string(), ss);
    }
    for fl in &r.fields {
        if let Some(v) = int(r, &fl.label) {
            o.fields.insert(fl.label.clone(), v);
        }
    }
    // A creature template keeps its gender, tints and the like in its "Appearance Data"
    // struct (Morrigan's Gender 2 is there); its integers count as the creature's fields
    // where the top level does not give them.
    if let Some(G3::Struct(i)) = field(r, "Appearance Data")
        && let Some(a) = t.structs.get(*i as usize)
    {
        for fl in &a.fields {
            if let Some(v) = int(a, &fl.label) {
                o.fields.entry(fl.label.clone()).or_insert(v);
            }
        }
    }
    read_text_fields(o, r);
}

/// The string fields engine functions read, as [`crate::world::Object::text_fields`].
const TEXT_FIELDS: [&str; 1] = ["KeyName"];

impl Sim {
    /// Gives creature `id` sound set `set` (its barks; `SetSoundSet`), or none for an empty
    /// name.
    pub fn set_sound_set(&mut self, id: ObjectId, set: &str) {
        let Some(o) = self.world.get_mut(id) else {
            return;
        };
        let set = set.trim().to_ascii_lowercase();
        if set.is_empty() {
            o.text_fields.remove(SOUND_SET);
        } else {
            o.text_fields.insert(SOUND_SET.to_string(), set);
        }
    }

    /// Character creation's voice choice (`GUI.SetSoundSet <set>`): the creature being made
    /// gets the set. False when no character is being made.
    pub fn chargen_sound_set(&mut self, set: &str) -> bool {
        match self.world.chargen {
            Some(c) => {
                self.set_sound_set(c, set);
                true
            }
            None => false,
        }
    }
}

/// The [`crate::world::Object::text_fields`] key of a creature's sound set: its template's
/// `SoundSet`, or what `SetSoundSet` gave it (character creation's voice choice).
pub const SOUND_SET: &str = "SoundSet";

fn read_text_fields(o: &mut crate::world::Object, s: &Struct) {
    for label in TEXT_FIELDS {
        if let Some(v) = text(s, label) {
            o.text_fields.insert(label.to_string(), v);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// GFF helpers

fn field<'a>(s: &'a Struct, label: &str) -> Option<&'a G3> {
    s.fields.iter().find(|f| f.label == label).map(|f| &f.value)
}

fn lossy(b: &[u8]) -> String {
    String::from_utf8_lossy(b)
        .trim_end_matches('\0')
        .to_string()
}

pub(crate) fn text(s: &Struct, label: &str) -> Option<String> {
    match field(s, label)? {
        G3::ExoString(b) | G3::ResRef(b) => Some(lossy(b)),
        _ => None,
    }
}

pub(crate) fn resref_field(s: &Struct, label: &str) -> Option<String> {
    text(s, label).filter(|t| !t.is_empty())
}

fn strref(s: &Struct, label: &str) -> Option<u32> {
    match field(s, label)? {
        G3::LocString(l) => Some(l.strref),
        G3::StrRef(r) => Some(*r),
        _ => None,
    }
}

pub(crate) fn int(s: &Struct, label: &str) -> Option<i32> {
    Some(match field(s, label)? {
        G3::Byte(v) => *v as i32,
        G3::Char(v) => *v as i32,
        G3::Word(v) => *v as i32,
        G3::Short(v) => *v as i32,
        G3::Dword(v) => *v as i32,
        G3::Int(v) => *v,
        G3::Dword64(v) => *v as i32,
        G3::Int64(v) => *v as i32,
        _ => return None,
    })
}

fn float(s: &Struct, label: &str) -> f32 {
    match field(s, label) {
        Some(G3::Float(v)) => *v,
        Some(G3::Double(v)) => *v as f32,
        _ => 0.0,
    }
}

impl Sim {
    /// Gives a creature the equipment and carried items its template lists
    /// (`Equip_ItemList`: each struct's type id is the `INVENTORY_SLOT_*`, provisional;
    /// `ItemList`), keeping each entry's `Droppable` flag.
    pub fn load_template_items(&mut self, id: ObjectId, t: &Gff3) {
        let Some(r) = t.structs.first() else { return };
        for (list, equip) in [("Equip_ItemList", true), ("ItemList", false)] {
            let entries: Vec<(u32, String, Option<i32>, i32)> = list_of(r, list)
                .iter()
                .filter_map(|&si| t.structs.get(si as usize))
                .filter_map(|s| {
                    resref_field(s, "TemplateResRef").map(|x| {
                        (
                            s.id,
                            x,
                            int(s, "Droppable"),
                            int(s, "SetNumber").unwrap_or(0),
                        )
                    })
                })
                .collect();
            for (slot, resref, droppable, set) in entries {
                let item = self.create_item(&resref);
                if let (Some(d), Some(o)) = (droppable, self.world.get_mut(item)) {
                    o.fields.insert("Droppable".into(), d);
                }
                // Carried items are added as any item is, merging into the stacks already
                // held (see `Sim::stack_into`); stackable ones start as stacks of one.
                if !equip {
                    if self.max_stack(item) > 1 {
                        if let Some(o) = self.world.get_mut(item) {
                            o.fields.entry("StackSize".into()).or_insert(1);
                        }
                    }
                    self.stack_into(id, item);
                    continue;
                }
                let mut worn = false;
                if let Some(o) = self.world.get_mut(id) {
                    o.inventory.push(item);
                    if equip {
                        // Weapons of the second set (SetNumber 1) wait in the inactive set;
                        // creatures start with set 0.
                        let slot = slot as i32;
                        if set == 1 && crate::world::WEAPON_SET_SLOTS.contains(&slot) {
                            o.other_set.insert(slot, item);
                        } else {
                            o.equipment.insert(slot, item);
                            worn = true;
                        }
                    }
                }
                // What it starts out wearing is put on as anything else is: the item's
                // properties take effect and the creature hears EVENT_TYPE_EQUIP
                // (DAOrigins.exe: 0x6bcc30 reads Equip_ItemList and equips each item through
                // 0x6bc7f0, whose container hook sends the event; see `Sim::wear`). Weapons
                // put in the inactive set get no event (0x797d10).
                if worn {
                    self.wear(id, item, slot as i32);
                }
            }
        }
    }

    /// `LoadItemsFromTemplate`: the template's equipment and items, replacing what the
    /// creature has when asked (the replaced items are destroyed).
    pub fn load_items_from_template(&mut self, id: ObjectId, template: &str, replace: bool) {
        let name = template.trim_end_matches(".utc").to_ascii_lowercase();
        let Ok(t) = self.read_gff3(&format!("{name}.utc")) else {
            let at = self.world.time_ms;
            self.world.trace.push(TraceEntry::Missing {
                t: at,
                what: format!("{name}.utc"),
            });
            return;
        };
        if replace {
            let old = self
                .world
                .get_mut(id)
                .map(|o| {
                    o.equipment.clear();
                    o.other_set.clear();
                    std::mem::take(&mut o.inventory)
                })
                .unwrap_or_default();
            for i in old {
                self.world.objects.remove(&i);
            }
        }
        self.load_template_items(id, &t);
    }

    /// An object's name: one given in play (`SetName`), else its template's, else its tag.
    pub fn display_name(&self, id: ObjectId) -> String {
        let Some(o) = self.world.get(id) else {
            return String::new();
        };
        o.name
            .clone()
            .or_else(|| o.name_strref.and_then(|s| self.string(s)))
            // A hero nobody named (quick start: the chargen script picks a character but no
            // name) goes by the name character creation would have offered. Provisional: the
            // executable's choice for an unnamed hero is not traced.
            .or_else(|| {
                (self.world.hero == Some(id))
                    .then(|| self.default_first_name(id))
                    .flatten()
            })
            .unwrap_or_else(|| o.tag.clone())
    }

    /// How an item's template presents it: its icon and description (for the interface).
    pub fn item_look(&self, item: ObjectId) -> ItemLook {
        let Some(template) = self.world.get(item).map(|o| o.template.clone()) else {
            return ItemLook::default();
        };
        let Ok(mut cache) = self.item_looks.lock() else {
            return ItemLook::default();
        };
        cache
            .entry(template.clone())
            .or_insert_with(|| {
                let Ok(g) = self.read_gff3(&format!("{template}.uti")) else {
                    return ItemLook::default();
                };
                let Some(r) = g.structs.first() else {
                    return ItemLook::default();
                };
                let properties = list_of(r, "PropertiesList")
                    .iter()
                    .filter_map(|&i| g.structs.get(i as usize))
                    .filter_map(|p| {
                        Some((int(p, "PropertyID")?, int(p, "PropertyPower").unwrap_or(1)))
                    })
                    .collect();
                ItemLook {
                    icon: text(r, "Icon").unwrap_or_default(),
                    description: strref(r, "Description"),
                    properties,
                }
            })
            .clone()
    }

    /// Sets how a template presents its items, as if read from it (tests).
    #[cfg(test)]
    pub(crate) fn set_item_look(&self, template: &str, look: ItemLook) {
        if let Ok(mut c) = self.item_looks.lock() {
            c.insert(template.to_string(), look);
        }
    }

    /// Creates an item from its `.uti` template, with the template's integer fields
    /// (`BaseItem`, ...) for engine functions that read them. A missing template still
    /// makes the item, traced as missing.
    pub fn create_item(&mut self, resref: &str) -> ObjectId {
        let resref = resref.trim_end_matches(".uti").to_ascii_lowercase();
        let tmpl = self.read_gff3(&format!("{resref}.uti"));
        let tag = tmpl
            .as_ref()
            .ok()
            .and_then(|g| g.structs.first().and_then(|r| text(r, "Tag")))
            .unwrap_or_else(|| resref.clone());
        let id = self.world.spawn(ObjectKind::Item, &tag);
        match tmpl {
            Ok(g) => {
                if let (Some(r), Some(o)) = (g.structs.first(), self.world.get_mut(id)) {
                    o.template = resref;
                    o.name_strref = strref(r, "LocalizedName");
                    o.plot = int(r, "Plot").unwrap_or(0) != 0;
                    for fl in &r.fields {
                        if let Some(v) = int(r, &fl.label) {
                            o.fields.insert(fl.label.clone(), v);
                        }
                    }
                    read_vars(&g, r, &mut o.locals);
                }
            }
            Err(e) => {
                if let Some(o) = self.world.get_mut(id) {
                    o.template = resref;
                }
                let t = self.world.time_ms;
                self.world.trace.push(TraceEntry::Missing {
                    t,
                    what: e.to_string(),
                });
            }
        }
        id
    }
}

pub(crate) fn list_of<'a>(s: &'a Struct, label: &str) -> &'a [u32] {
    match field(s, label) {
        Some(G3::List(v)) => v,
        _ => &[],
    }
}

/// `VarTable` entries: type 1 int, 2 float, 3 string, 4 object, 12 resource (provisional).
fn read_vars(doc: &Gff3, s: &Struct, locals: &mut crate::world::Locals) {
    for &i in list_of(s, "VarTable") {
        let Some(v) = doc.structs.get(i as usize) else {
            continue;
        };
        let Some(name) = text(v, "Name") else {
            continue;
        };
        match int(v, "Type") {
            Some(1) => {
                locals.ints.insert(name, int(v, "Value").unwrap_or(0));
            }
            Some(2) => {
                locals.floats.insert(name, float(v, "Value"));
            }
            Some(3) => {
                locals
                    .strings
                    .insert(name, text(v, "Value").unwrap_or_default());
            }
            Some(4) => {
                locals
                    .objects
                    .insert(name, thedas_script::vm::OBJECT_INVALID);
            }
            Some(12) => {
                locals
                    .resources
                    .insert(name, text(v, "Value").unwrap_or_default());
            }
            _ => {}
        }
    }
}

fn cif_vars(g: &Gff4) -> Vec<(String, thedas_formats::gff4::Value)> {
    let mut out = Vec::new();
    let Some(thedas_formats::gff4::Value::List(Some(thedas_formats::List::Struct(vars)))) =
        g.field(g.root, 17000)
    else {
        return out;
    };
    for v in vars.iter().flatten() {
        let name = g
            .field(*v, 17001)
            .and_then(|x| x.as_str())
            .map(|s| s.trim_end_matches('\0').to_string());
        let value = match g.field(*v, 17003) {
            Some(thedas_formats::gff4::Value::Generic(slot)) => slot.value.clone(),
            other => other.cloned(),
        };
        if let (Some(n), Some(val)) = (name, value) {
            out.push((n, val));
        }
    }
    out
}

/// The campaign file: script (17104), start area (17101), start position (17102) and the area
/// tree (3290). Field meanings provisional, read from `singleplayer.cif`.
fn parse_cif(g: &Gff4) -> ModuleInfo {
    let s = |l: u32| {
        g.field(g.root, l)
            .and_then(|v| v.as_str())
            .map(|s| s.trim_end_matches('\0').to_string())
            .filter(|s| !s.is_empty())
    };
    let mut info = ModuleInfo {
        script: s(17104),
        start_area: s(17101).or_else(|| s(17100)),
        ..Default::default()
    };
    if let Some(thedas_formats::gff4::Value::Vec3(p)) = g.field(g.root, 17102) {
        info.start_position = *p;
    }
    fn walk(g: &Gff4, id: StructId, out: &mut Vec<AreaEntry>, depth: usize) {
        if depth > 32 {
            return;
        }
        let names: Vec<String> = match g.field(id, 3291) {
            Some(thedas_formats::gff4::Value::List(Some(thedas_formats::List::String(v)))) => v
                .iter()
                .flatten()
                .map(|s| s.trim_end_matches('\0').to_string())
                .collect(),
            _ => Vec::new(),
        };
        let resref = g
            .field(id, 3294)
            .and_then(|v| v.as_str())
            .map(|s| s.trim_end_matches('\0').to_string())
            .filter(|s| !s.is_empty());
        if let Some(r) = resref {
            out.push(AreaEntry {
                resref: r,
                layout: names.first().cloned(),
                area_list: names.get(1).cloned(),
            });
        }
        let children: Vec<StructId> = match g.field(id, 3292) {
            Some(thedas_formats::gff4::Value::List(Some(thedas_formats::List::Struct(v)))) => {
                v.iter().flatten().copied().collect()
            }
            Some(thedas_formats::gff4::Value::Struct(Some(c))) => vec![*c],
            _ => Vec::new(),
        };
        for c in children {
            walk(g, c, out, depth + 1);
        }
    }
    if let Some(thedas_formats::gff4::Value::Struct(Some(r))) = g.field(g.root, 3290) {
        walk(g, *r, &mut info.areas, 0);
    }
    info
}

// ---------------------------------------------------------------------------------------------
// The engine host

pub struct ScriptHost<'a> {
    pub sim: &'a mut Sim,
    /// The current event's handle.
    pub event: Option<u64>,
    pub script: String,
}

impl Host for ScriptHost<'_> {
    fn action(
        &mut self,
        caller: ObjectId,
        f: &Function,
        args: Vec<Value>,
    ) -> VmResult<Option<Value>> {
        let watched =
            self.sim.watch_calls.contains(&caller)
                || (!self.sim.watch_tags.is_empty()
                    && self.sim.world.get(caller).is_some_and(|o| {
                        self.sim.watch_tags.contains(&o.tag.to_ascii_lowercase())
                    }));
        // Mana or stamina reaching 0 tells the creature (see `Sim::note_mana_change`).
        let mana_target = match f.name.as_str() {
            "SetCreatureMana" => args.first(),
            "SetCreatureProperty" | "UpdateCreatureProperty"
                if matches!(args.get(1), Some(Value::Int(8))) =>
            {
                args.first()
            }
            _ => None,
        }
        .and_then(|v| match v {
            Value::Object(o) => Some(*o),
            _ => None,
        });
        let mana_before = mana_target.map(|id| (id, self.sim.mana_now(id)));
        let result = crate::engine::call(self, caller, f, &args);
        if let Some((id, before)) = mana_before {
            self.sim.note_mana_change(id, before);
        }
        if watched {
            let t = self.sim.world.time_ms;
            let shown = match &result {
                Some(Ok(v)) => format!("{v:?}"),
                Some(Err(e)) => format!("error {e}"),
                None => "unimplemented".into(),
            };
            let mut text = format!("{} {}{:?} -> {shown}", self.script, f.name, args);
            text.truncate(400);
            self.sim.world.trace.push(TraceEntry::Log {
                t,
                channel: -6,
                text,
            });
        }
        match result {
            Some(r) => {
                *self
                    .sim
                    .world
                    .trace
                    .implemented_calls
                    .entry(f.name.clone())
                    .or_default() += 1;
                r
            }
            None => {
                let t = self.sim.world.time_ms;
                self.sim.world.trace.push(TraceEntry::Unimplemented {
                    t,
                    function: f.name.clone(),
                    script: self.script.clone(),
                });
                Ok(None)
            }
        }
    }
}

impl ScriptHost<'_> {
    pub fn err(&self, f: &Function, msg: impl Into<String>) -> VmError {
        VmError::Host {
            name: f.name.clone(),
            message: msg.into(),
        }
    }

    pub fn object(&self, id: ObjectId) -> Option<&Object> {
        self.sim.world.get(id)
    }

    pub fn location_of(&self, id: ObjectId) -> Option<Location> {
        let o = self.object(id)?;
        Some(Location {
            area: o.area.unwrap_or(thedas_script::vm::OBJECT_INVALID),
            position: o.position,
            facing: o.facing,
        })
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn creature_templates_give_their_appearance_datas_fields() {
        use thedas_formats::gff3::{Field, Gff3, Struct, Value};
        let f = |label: &str, value| Field {
            label: label.into(),
            value,
        };
        let t = Gff3 {
            file_type: *b"UTC ",
            file_version: *b"V3.2",
            structs: vec![
                Struct {
                    id: 0xFFFF_FFFF,
                    fields: vec![
                        f("Race", Value::Byte(3)),
                        f("SkinTint", Value::Int(7)),
                        f("Appearance Data", Value::Struct(1)),
                    ],
                },
                Struct {
                    id: 0,
                    fields: vec![
                        f("Gender", Value::Byte(2)),
                        f("SkinTint", Value::Int(3)),
                        f("Appearance_Type", Value::Dword(15)),
                    ],
                },
            ],
        };
        let mut o = crate::world::Object::new(1, ObjectKind::Creature, "morrigan");
        apply_template(&mut o, &t);
        assert_eq!(o.appearance, 15);
        assert_eq!(o.fields.get("Gender"), Some(&2));
        // The top level wins where both give a value.
        assert_eq!(o.fields.get("SkinTint"), Some(&7));
        assert_eq!(o.fields.get("Race"), Some(&3));
    }

    #[test]
    fn the_party_picker_switches_event_scripts_as_follower_states_change() {
        let ldf = "int FOLLOWER_STATE_ACTIVE = 1;\nint FOLLOWER_STATE_AVAILABLE = 2;\n";
        let table = Arc::new(thedas_script::EngineTable::parse(ldf.as_bytes()).unwrap());
        let mut s = Sim::new(Arc::new(Vfs::new()), table, 1);
        let module = s.world.spawn(ObjectKind::Module, "module");
        let hero = s.world.spawn(ObjectKind::Creature, "player");
        s.world.module = Some(module);
        s.world.hero = Some(hero);
        let mut members = Vec::new();
        for tag in ["alistair", "leliana", "sten", "morrigan"] {
            let id = s.world.spawn(ObjectKind::Creature, tag);
            let o = s.world.get_mut(id).unwrap();
            o.fields.insert("FollowerState".into(), 2);
            o.script = Some("creature_core".into());
            members.push(id);
        }
        s.world.party_choice = Some(vec![members[1], members[3], members[0]]);
        s.party_picker();
        let script = |s: &Sim, id| s.world.get(id).unwrap().script.clone().unwrap();
        for &m in &[members[0], members[1], members[3]] {
            assert_eq!(script(&s, m), "player_core");
        }
        assert_eq!(script(&s, members[2]), "creature_core", "left in the pool");
    }

    #[test]
    fn the_player_picks_the_party_in_an_open_picker() {
        let ldf = "int FOLLOWER_STATE_ACTIVE = 1;\nint FOLLOWER_STATE_AVAILABLE = 2;\n\
            int FOLLOWER_STATE_LOCKEDACTIVE = 5;\nint EVENT_TYPE_PARTYMEMBER_ADDED = 66;\n\
            int EVENT_TYPE_PARTYMEMBER_DROPPED = 67;\nint EVENT_TYPE_PARTYPICKER_CLOSED = 71;\n\
            int EVENT_TYPE_PARTYPICKER_INIT = 104;\n";
        let table = Arc::new(thedas_script::EngineTable::parse(ldf.as_bytes()).unwrap());
        let mut s = Sim::new(Arc::new(Vfs::new()), table, 1);
        s.party_picker_policy = PartyPickerPolicy::Player;
        let module = s.world.spawn(ObjectKind::Module, "module");
        let hero = s.world.spawn(ObjectKind::Creature, "player");
        s.world.module = Some(module);
        s.world.hero = Some(hero);
        let mut m = Vec::new();
        for tag in ["alistair", "leliana", "sten", "morrigan", "dog"] {
            let id = s.world.spawn(ObjectKind::Creature, tag);
            s.world
                .get_mut(id)
                .unwrap()
                .fields
                .insert("FollowerState".into(), 2);
            m.push(id);
        }
        // Alistair travels with the party and cannot be left out.
        s.world
            .get_mut(m[0])
            .unwrap()
            .fields
            .insert("FollowerState".into(), 5);
        s.world.get_mut(hero).unwrap().followers = vec![m[0], m[1]];
        s.party_picker();
        assert_eq!(
            s.world.party_picker.as_ref().unwrap().selected,
            vec![m[0], m[1]]
        );
        assert!(!s.party_picker_toggle(m[0]), "locked");
        assert!(s.party_picker_toggle(m[2]));
        assert!(!s.party_picker_toggle(m[3]), "three already");
        // The second portrait drops Leliana; Remove All keeps the locked.
        assert!(s.gui_message(crate::chanters::MSG_PICKER_UNSELECT, &["2".into()]));
        assert_eq!(
            s.world.party_picker.as_ref().unwrap().selected,
            vec![m[0], m[2]]
        );
        assert!(s.gui_message(crate::chanters::MSG_PICKER_CLEAR, &[]));
        assert_eq!(s.world.party_picker.as_ref().unwrap().selected, vec![m[0]]);
        assert!(s.party_picker_toggle(m[3]));
        s.world.queue.clear();
        assert!(s.gui_message(crate::chanters::MSG_PICKER_SUBMIT, &[]));
        assert!(s.world.party_picker.is_none());
        assert_eq!(s.world.get(hero).unwrap().followers, vec![m[0], m[3]]);
        let events: Vec<(i32, Vec<ObjectId>)> = s
            .world
            .queue
            .values()
            .filter(|q| q.target == module)
            .map(|q| (q.event.ty, q.event.objects.clone()))
            .collect();
        assert_eq!(
            events,
            vec![(67, vec![m[1]]), (66, vec![m[3]]), (71, vec![])]
        );
        // Nothing open: the messages do nothing.
        assert!(!s.gui_message(crate::chanters::MSG_PICKER_SUBMIT, &[]));
    }
    use super::*;

    const LDF: &str = "#define ENGINE_STRUCTURE_0 event\n\
        int GetLocalInt(object o, string s) = 63;\n\
        void SetLocalInt(object o, string s, int n) = 64;\n";

    /// n = GetLocalInt(OBJECT_SELF, "n") + 1; SetLocalInt(OBJECT_SELF, "n", n).
    fn counter_script() -> Ncs {
        let mut b: Vec<u8> = Vec::new();
        let s = |b: &mut Vec<u8>| b.extend([0x04, 0x05, 0x00, 0x01, b'n']);
        let self_ = |b: &mut Vec<u8>| b.extend([0x04, 0x06, 0, 0, 0, 0]);
        s(&mut b);
        self_(&mut b);
        b.extend([0x05, 0x00, 0x00, 63, 0x02]);
        b.extend([0x04, 0x03, 0, 0, 0, 1, 0x14, 0x20]);
        s(&mut b);
        self_(&mut b);
        b.extend([0x05, 0x00, 0x00, 64, 0x03, 0x20, 0x00]);
        let mut v = b"NCS V1.0B".to_vec();
        v.extend(((13 + b.len()) as u32).to_be_bytes());
        v.extend(b);
        Ncs::parse(&v).unwrap()
    }

    fn sim() -> Sim {
        let table = Arc::new(EngineTable::parse(LDF.as_bytes()).unwrap());
        let mut s = Sim::new(Arc::new(Vfs::new()), table, 7);
        s.world.trace.keep = true;
        s.add_script("counter", counter_script());
        s
    }

    fn with_module(s: &mut Sim) -> ObjectId {
        let m = s.world.spawn(ObjectKind::Module, "module");
        s.world.module = Some(m);
        m
    }

    #[test]
    fn a_command_failing_as_it_starts_is_reported_an_update_later() {
        let mut s = sim();
        let c = s.world.spawn(ObjectKind::Creature, "follower");
        s.world.get_mut(c).unwrap().script = Some("counter".into());
        s.world.get_mut(c).unwrap().commands.push(Command {
            ty: 1,
            name: "CommandAttack".into(),
            target: None,
            location: None,
            seconds: 0.0,
            ints: Vec::new(),
            strings: Vec::new(),
            due: None,
            from: None,
            approach: None,
            ring_wait: false,
            after_impact_ms: None,
            impacted: false,
            arrived: false,
            path: Vec::new(),
        });
        let t0 = s.world.time_ms;
        s.fail_front_command(c, "COMMAND_FAILED_PATH_ACTION_REQUIRED");
        assert!(s.world.get(c).unwrap().commands.is_empty());
        // Nothing hears it at the same instant (a script queueing the command again would
        // fail it again there forever); the owner hears it one update on.
        let due: Vec<u64> = s.world.queue.keys().map(|k| k.0).collect();
        assert_eq!(due, vec![t0 + APPROACH_CHECK_MS]);
        s.tick(APPROACH_CHECK_MS - 1);
        assert_eq!(s.world.get(c).unwrap().locals.ints.get("n"), None);
        s.tick(1);
        assert_eq!(s.world.get(c).unwrap().locals.ints.get("n"), Some(&1));
    }

    #[test]
    fn a_loaded_cutscene_ends_and_sets_its_plot_flag() {
        let mut s = sim();
        let m = with_module(&mut s);
        let l = &mut s.world.get_mut(m).unwrap().locals;
        l.strings.insert("CUTSCENE_SET_PLOT".into(), "ABC".into());
        l.ints.insert("CUTSCENE_SET_PLOT_FLAG".into(), 13);
        s.world.cutscenes.push("intro.cut".into());
        s.tick(0);
        assert!(s.world.cutscenes.is_empty());
        assert_eq!(s.world.plots.get(&("ABC".to_string(), 13)), Some(&true));
        let l = &s.world.get(m).unwrap().locals;
        assert!(!l.strings.contains_key("CUTSCENE_SET_PLOT"));
    }

    #[test]
    fn a_played_cutscene_holds_the_world_for_its_length() {
        let mut s = sim();
        s.cutscene_policy = CutscenePolicy::Play;
        let m = with_module(&mut s);
        // One that cannot be read ends at once, as when skipping.
        s.world.cutscenes.push("missing".into());
        s.tick(0);
        assert!(s.world.cutscenes.is_empty() && s.world.cutscene.is_none());
        let playing = |seconds| crate::cutscene::Cutscene {
            name: "intro.cut".into(),
            seconds,
            lines: vec![crate::cutscene::CutsceneLine {
                start: 1.0,
                end: 2.0,
                strref: 7,
                actor: "duncan".into(),
                conversation: None,
                vo_mode: 0,
            }],
            elapsed_ms: 0,
            from_dialog: false,
            bound: Vec::new(),
            placement: None,
        };
        // While one plays the world stands still; what it sets waits for its end, and so
        // does the next one loaded.
        s.world.cutscene = Some(playing(5.0));
        let l = &mut s.world.get_mut(m).unwrap().locals;
        l.strings.insert("CUTSCENE_SET_PLOT".into(), "ABC".into());
        l.ints.insert("CUTSCENE_SET_PLOT_FLAG".into(), 13);
        s.world.cutscenes.push("next".into());
        let t0 = s.world.time_ms;
        s.tick(1500);
        assert_eq!(s.world.time_ms, t0);
        let c = s.world.cutscene.as_ref().unwrap();
        assert_eq!(c.line_now().map(|l| l.strref), Some(7));
        assert_eq!(s.world.cutscenes, ["next"]);
        assert!(s.world.plots.is_empty());
        // It ends at its length; the rest of the tick runs the world.
        s.tick(4000);
        assert!(s.world.cutscene.is_none() && s.world.cutscenes.is_empty());
        assert_eq!(s.world.time_ms, t0 + 500);
        assert_eq!(s.world.plots.get(&("ABC".to_string(), 13)), Some(&true));
        assert!(s.world.trace.entries.iter().any(|e| matches!(
            e,
            TraceEntry::Log { text, .. } if text == "cutscene intro.cut ends"
        )));
        // The player may skip one.
        s.world.cutscene = Some(playing(60.0));
        s.skip_cutscene();
        assert!(s.world.cutscene.is_none());
    }

    #[test]
    fn character_creation_leaves_a_saved_face() {
        let dir = std::env::temp_dir().join(format!("thedas-face-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let preset = dir.join("hf_pcc_b01.mor");
        std::fs::write(&preset, b"MORP").unwrap();
        let mut vfs = Vfs::new();
        vfs.push_files(&[preset], "test");
        let table = Arc::new(
            EngineTable::parse(
                b"#define ENGINE_STRUCTURE_0 event\nint EVENT_TYPE_CHARGEN_END = 59;\n",
            )
            .unwrap(),
        );
        let mut s = Sim::new(Arc::new(vfs), table, 1);
        s.world.tables().insert(
            "apr_base",
            &["ModelType", "ModelRace"],
            vec![(
                15,
                vec![
                    thedas_formats::gda::Cell::String("P".into()),
                    thedas_formats::gda::Cell::String("H\0".into()),
                ],
            )],
        );
        let m = with_module(&mut s);
        s.world.get_mut(m).unwrap().script = Some("missing_module_script".into());
        let hero = s.world.spawn(ObjectKind::Creature, "player");
        {
            let o = s.world.get_mut(hero).unwrap();
            o.appearance = 15;
            o.fields.insert("Gender".into(), 2);
        }
        // Level-up's end leaves the face alone; creation's end keeps the first preset.
        let mut end = s.new_event("CHARGEN_END", Some(hero));
        end.objects = vec![hero];
        end.ints = vec![1, 0];
        s.deliver(m, end.clone(), None);
        assert_eq!(s.world.get(hero).unwrap().head_morph, None);
        end.ints = vec![0, 0];
        s.deliver(m, end.clone(), None);
        assert_eq!(
            s.world.get(hero).unwrap().head_morph.as_deref(),
            Some("hf_pcc_b01")
        );
        // A face chosen on the face screen stays, and is saved.
        s.set_head_morph(hero, Some("HF_PCC_B03.mor".into()));
        s.deliver(m, end, None);
        assert_eq!(
            s.world.get(hero).unwrap().head_morph.as_deref(),
            Some("hf_pcc_b03")
        );
        let saved = s.save_json().unwrap();
        let mut t = Sim::new(Arc::new(Vfs::new()), Arc::new(EngineTable::default()), 1);
        t.load_json(&saved).unwrap();
        assert_eq!(
            t.world.get(hero).unwrap().head_morph.as_deref(),
            Some("hf_pcc_b03")
        );
        s.set_head_morph(hero, Some("  ".into()));
        assert_eq!(s.world.get(hero).unwrap().head_morph, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_new_character_keeps_the_face_screens_first_face() {
        use thedas_formats::gda::Cell;
        let dir = std::env::temp_dir().join(format!("thedas-chargen-face-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let xml = dir.join("chargenmorphcfg.xml");
        std::fs::write(
            &xml,
            "<morph_config><heads><human_male><resource name=\"HM_CPS_P01.mop\"/>\
             <resource name=\"HM_PCC_B01.mop\"/></human_male></heads></morph_config>",
        )
        .unwrap();
        let mop = dir.join("hm_cps_p01.mop");
        std::fs::write(&mop, b"").unwrap();
        let mut vfs = Vfs::new();
        vfs.push_files(&[xml, mop], "test");
        let mut s = Sim::new(Arc::new(vfs), Arc::new(EngineTable::default()), 1);
        s.world.tables().insert(
            "apr_base",
            &["ModelRace"],
            vec![(1, vec![Cell::String("h".into())])],
        );
        let hero = s.world.spawn(ObjectKind::Creature, "player");
        s.world.get_mut(hero).unwrap().appearance = 1;
        s.world
            .get_mut(hero)
            .unwrap()
            .fields
            .insert("Gender".into(), 1);
        s.keep_chargen_face(hero);
        // The face screen's first face (hair and all), not the bald base preset.
        assert_eq!(
            s.world.get(hero).unwrap().head_morph.as_deref(),
            Some("hm_cps_p01")
        );
        // A face already kept stays.
        s.world.get_mut(hero).unwrap().head_morph = Some("hm_own".into());
        s.keep_chargen_face(hero);
        assert_eq!(
            s.world.get(hero).unwrap().head_morph.as_deref(),
            Some("hm_own")
        );
    }

    #[test]
    fn chargen_quick_start_ends_at_once_and_manual_waits() {
        let mut s = sim();
        let m = with_module(&mut s);
        let hero = s.world.spawn(ObjectKind::Creature, "player");
        s.start_chargen(hero, 0, 0);
        let queued: Vec<_> = s
            .world
            .queue
            .values()
            .map(|q| (q.target, q.event.objects.clone(), q.event.ints.clone()))
            .collect();
        assert_eq!(
            queued,
            vec![(m, vec![hero], vec![0, 0]), (m, vec![hero], vec![0, 0])]
        );
        assert_eq!(s.world.chargen, None);

        let mut s = sim();
        with_module(&mut s);
        s.chargen_policy = ChargenPolicy::Manual;
        s.start_chargen(hero, 0, 0);
        assert_eq!(s.world.queue.len(), 1);
        assert_eq!(s.world.chargen, Some(hero));
        assert!(s.chargen_event("CHARGEN_SELECT_RACE", vec![1, 0], Vec::new()));
        assert!(s.chargen_event("CHARGEN_END", vec![0, 0], Vec::new()));
        assert_eq!(s.world.chargen, None);
        assert!(!s.chargen_event("CHARGEN_END", vec![0, 0], Vec::new()));
    }

    #[test]
    fn areas_keep_their_state_between_visits() {
        let mut s = sim();
        let hero = s.world.spawn(ObjectKind::Creature, "hero");
        s.world.hero = Some(hero);
        let mut areas = Vec::new();
        for name in ["a", "b"] {
            let id = s.world.spawn(ObjectKind::Area, name);
            s.world.get_mut(id).unwrap().template = name.into();
            areas.push(id);
        }
        let (a, b) = (areas[0], areas[1]);
        s.world.area = Some(a);
        s.world.get_mut(hero).unwrap().area = Some(a);
        let awake = s.world.spawn(ObjectKind::Creature, "awake");
        let asleep = s.world.spawn(ObjectKind::Creature, "asleep");
        for (id, on) in [(awake, true), (asleep, false)] {
            let o = s.world.get_mut(id).unwrap();
            o.area = Some(a);
            o.active = on;
        }
        let count = s.world.objects.len();
        s.area_transition("b", "");
        assert_eq!(s.world.area, Some(b));
        assert!(!s.world.get(awake).unwrap().active, "the left area unloads");
        s.area_transition("a", "");
        assert_eq!(s.world.area, Some(a));
        assert_eq!(s.world.objects.len(), count, "nothing is spawned again");
        assert!(s.world.get(awake).unwrap().active);
        assert!(!s.world.get(asleep).unwrap().active);
        assert_eq!(s.world.get(hero).unwrap().area, Some(a));
    }

    #[test]
    fn the_party_leaves_and_enters_areas() {
        let mut s = sim();
        let hero = s.world.spawn(ObjectKind::Creature, "hero");
        let dog = s.world.spawn(ObjectKind::Creature, "dog");
        s.world.hero = Some(hero);
        s.world.get_mut(hero).unwrap().followers = vec![dog];
        let mut areas = Vec::new();
        for name in ["a", "b"] {
            let id = s.world.spawn(ObjectKind::Area, name);
            let o = s.world.get_mut(id).unwrap();
            o.template = name.into();
            o.script = Some("counter".into());
            areas.push(id);
        }
        let (a, b) = (areas[0], areas[1]);
        s.world.area = Some(a);
        for id in [hero, dog] {
            s.world.get_mut(id).unwrap().area = Some(a);
        }
        s.area_transition("b", "");
        // The old area heard both leave before it unloaded.
        assert_eq!(s.world.get(a).unwrap().locals.ints.get("n"), Some(&2));
        let entering: Vec<_> = s
            .world
            .queue
            .values()
            .filter(|q| q.target == b && q.event.objects.len() == 1)
            .map(|q| q.event.objects[0])
            .collect();
        assert_eq!(entering, vec![hero, dog]);
    }

    #[test]
    fn popup_answers_go_to_the_module_without_an_owner() {
        let mut s = sim();
        let m = with_module(&mut s);
        s.world.popup = Some(crate::world::Popup {
            strref: 5,
            kind: 2,
            owner: OBJECT_INVALID,
            input: false,
        });
        assert!(s.answer_popup(0));
        let q: Vec<_> = s
            .world
            .queue
            .values()
            .map(|q| (q.target, q.event.ints.clone()))
            .collect();
        assert_eq!(q, vec![(m, vec![2, 0])]);
    }

    #[test]
    fn a_failed_area_transition_keeps_the_old_area() {
        let mut s = sim();
        let area = s.world.spawn(ObjectKind::Area, "here");
        s.world.area = Some(area);
        let thing = s.world.spawn(ObjectKind::Placeable, "thing");
        s.world.get_mut(thing).unwrap().area = Some(area);
        s.world.transition = Some(("nowhere".into(), "".into()));
        s.tick(0);
        assert_eq!(s.world.area, Some(area));
        assert!(s.world.get(thing).unwrap().active);
        assert!(s.world.transition.is_none());
    }

    #[test]
    fn a_scripted_transition_waits_for_the_next_update() {
        let mut s = sim();
        let t0 = s.world.time_ms;
        s.world.transition = Some(("nowhere".into(), "".into()));
        s.world.transition_at = Some(t0 + TRANSITION_DELAY_MS);
        s.tick(TRANSITION_DELAY_MS - 1);
        assert!(s.world.transition.is_some(), "still pending this update");
        s.tick(1);
        assert!(s.world.transition.is_none());
        assert!(s.world.transition_at.is_none());
    }

    #[test]
    fn walking_into_a_trigger_sends_enter_and_exit() {
        let mut s = sim();
        let area = s.world.spawn(ObjectKind::Area, "a");
        let tr = s.world.spawn(ObjectKind::Trigger, "t");
        let c = s.world.spawn(ObjectKind::Creature, "c");
        {
            let t = s.world.get_mut(tr).unwrap();
            t.area = Some(area);
            t.position = [10.0, 10.0, 0.0];
            t.geometry = vec![
                [-1.0, -1.0, 0.0],
                [1.0, -1.0, 0.0],
                [1.0, 1.0, 0.0],
                [-1.0, 1.0, 0.0],
            ];
        }
        s.world.get_mut(c).unwrap().area = Some(area);
        s.move_object(c, [0.0, 0.0, 0.0], None);
        assert!(s.world.queue.is_empty());
        s.move_object(c, [10.5, 9.5, 0.0], None);
        let q: Vec<_> = s
            .world
            .queue
            .values()
            .map(|q| (q.target, q.event.objects.clone()))
            .collect();
        assert_eq!(q, vec![(tr, vec![c])]);
        s.world.queue.clear();
        s.move_object(c, [10.2, 10.2, 0.0], None);
        assert!(s.world.queue.is_empty(), "still inside: no new event");
        s.move_object(c, [20.0, 10.0, 0.0], None);
        assert_eq!(s.world.queue.len(), 1);
        assert!(!s.world.in_triggers.contains_key(&c));
    }

    #[test]
    fn walking_crosses_triggers_on_the_way_and_followers_keep_up() {
        let mut s = sim();
        let area = s.world.spawn(ObjectKind::Area, "a");
        let hero = s.world.spawn(ObjectKind::Creature, "hero");
        let dog = s.world.spawn(ObjectKind::Creature, "dog");
        let tr = s.world.spawn(ObjectKind::Trigger, "t");
        s.world.hero = Some(hero);
        for id in [hero, dog, tr] {
            s.world.get_mut(id).unwrap().area = Some(area);
        }
        s.world.get_mut(hero).unwrap().followers = vec![dog];
        {
            let t = s.world.get_mut(tr).unwrap();
            t.position = [5.0, 0.0, 0.0];
            t.geometry = vec![
                [-0.5, -1.0, 0.0],
                [0.5, -1.0, 0.0],
                [0.5, 1.0, 0.0],
                [-0.5, 1.0, 0.0],
            ];
        }
        assert!(s.walk(hero, [10.0, 0.0, 0.0], None));
        assert_eq!(s.world.get(hero).unwrap().position, [10.0, 0.0, 0.0]);
        // 10 m at 5 m/s.
        assert_eq!(s.world.time_ms, 2000);
        // The trigger saw the hero pass (enter and exit both delivered by now).
        assert!(!s.world.in_triggers.contains_key(&hero));
        let dog_pos = s.world.get(dog).unwrap().position;
        assert!(
            (dog_pos[0] - 9.0).abs() < 1e-4,
            "dog follows a metre behind: {dog_pos:?}"
        );
    }

    #[test]
    fn followers_take_the_formation_slots_behind_the_leader() {
        // A leader at the origin facing 0 (toward +Y): its right is +X.
        let near =
            |a: [f32; 3], b: [f32; 3]| (a[0] - b[0]).abs() < 1e-4 && (a[1] - b[1]).abs() < 1e-4;
        assert!(
            near(behind([0.0; 3], 0.0, 0), [-1.5, -1.0, 0.0]),
            "behind-left"
        );
        assert!(
            near(behind([0.0; 3], 0.0, 1), [1.5, -1.0, 0.0]),
            "behind-right"
        );
        assert!(
            near(behind([0.0; 3], 0.0, 2), [0.0, -2.5, 0.0]),
            "straight behind"
        );
        // Turned to face +X (facing 270): behind is -X, its left is +Y.
        assert!(near(behind([10.0, 0.0, 1.0], 270.0, 0), [9.0, 1.5, 1.0]));
        // Past the table, the last slot.
        assert_eq!(behind([0.0; 3], 0.0, 30), behind([0.0; 3], 0.0, 10));
    }

    #[test]
    fn a_move_replaced_before_it_ends_still_crosses_triggers() {
        let mut s = sim();
        let area = s.world.spawn(ObjectKind::Area, "a");
        let hero = s.world.spawn(ObjectKind::Creature, "hero");
        let tr = s.world.spawn(ObjectKind::Trigger, "t");
        s.world.hero = Some(hero);
        for id in [hero, tr] {
            s.world.get_mut(id).unwrap().area = Some(area);
        }
        {
            let t = s.world.get_mut(tr).unwrap();
            t.position = [2.0, 0.0, 0.0];
            t.geometry = vec![
                [-0.5, -1.0, 0.0],
                [0.5, -1.0, 0.0],
                [0.5, 1.0, 0.0],
                [-0.5, 1.0, 0.0],
            ];
        }
        // As a held stick does: aim 3 m ahead, and re-aim every 250 ms before arriving.
        for _ in 0..8 {
            let x = s.world.get(hero).unwrap().position[0];
            s.order_move(hero, [x + 3.0, 0.0, 0.0]);
            s.tick(250);
        }
        let x = s.world.get(hero).unwrap().position[0];
        assert!(x > 3.0, "the hero got past the trigger: {x}");
        let entered = s.world.trace.entries.iter().any(
            |e| matches!(e, crate::trace::TraceEntry::EventQueued { target, .. } if *target == tr),
        );
        assert!(entered, "the trigger heard the hero");
        assert!(
            !s.world.in_triggers.contains_key(&hero),
            "and the hero left it"
        );
    }

    #[test]
    fn commands_take_time() {
        let mut s = sim();
        let id = s.world.spawn(ObjectKind::Creature, "c");
        let cmd = |name: &str, seconds: f32| Command {
            ty: 0,
            name: name.into(),
            target: None,
            location: None,
            seconds,
            ints: Vec::new(),
            strings: Vec::new(),
            due: None,
            from: None,
            approach: None,
            ring_wait: false,
            after_impact_ms: None,
            impacted: false,
            arrived: false,
            path: Vec::new(),
        };
        let o = s.world.get_mut(id).unwrap();
        o.commands.push(cmd("CommandPlayAnimation", 0.0));
        o.commands.push(cmd("CommandWait", 2.0));
        s.tick(100);
        assert_eq!(s.world.get(id).unwrap().commands.len(), 2);
        s.tick(400);
        assert_eq!(s.world.get(id).unwrap().commands.len(), 1);
        s.tick(1999);
        assert_eq!(s.world.get(id).unwrap().commands.len(), 1);
        s.tick(1);
        assert!(s.world.get(id).unwrap().commands.is_empty());
        assert_eq!(s.world.time_ms, 2500);
    }

    #[test]
    fn a_weapon_set_switch_command_switches_at_once() {
        let mut s = sim();
        let id = s.world.spawn(ObjectKind::Creature, "archer");
        let bow = s.world.spawn(ObjectKind::Item, "bow");
        let axe = s.world.spawn(ObjectKind::Item, "axe");
        let o = s.world.get_mut(id).unwrap();
        o.equipment.insert(0, bow);
        o.other_set.insert(0, axe);
        o.commands.push(Command {
            ty: 0,
            name: "CommandSwitchWeaponSet".into(),
            target: None,
            location: None,
            seconds: 0.0,
            ints: vec![1],
            strings: Vec::new(),
            due: None,
            from: None,
            approach: None,
            ring_wait: false,
            after_impact_ms: None,
            impacted: false,
            arrived: false,
            path: Vec::new(),
        });
        s.tick(1);
        let o = s.world.get(id).unwrap();
        assert!(o.commands.is_empty(), "done as soon as it runs");
        assert_eq!(o.equipment.get(&0), Some(&axe), "the axe is in hand");
        assert_eq!(o.other_set.get(&0), Some(&bow));
        assert_eq!(o.fields.get("ActiveWeaponSet"), Some(&1));
    }

    #[test]
    fn a_jump_to_an_object_lands_on_it() {
        let mut s = sim();
        let area = s.world.spawn(ObjectKind::Area, "a");
        let id = s.world.spawn(ObjectKind::Creature, "c");
        let wp = s.world.spawn(ObjectKind::Waypoint, "wp");
        s.world.get_mut(id).unwrap().area = Some(area);
        let w = s.world.get_mut(wp).unwrap();
        w.area = Some(area);
        w.position = [80.0, 120.0, 29.5];
        s.world.get_mut(id).unwrap().commands.push(Command {
            ty: 0,
            name: "CommandJumpToObject".into(),
            target: Some(wp),
            location: None,
            seconds: 0.0,
            ints: Vec::new(),
            strings: Vec::new(),
            due: None,
            from: None,
            approach: None,
            ring_wait: false,
            after_impact_ms: None,
            impacted: false,
            arrived: false,
            path: Vec::new(),
        });
        s.tick(1);
        let o = s.world.get(id).unwrap();
        assert!(o.commands.is_empty());
        assert_eq!(o.position, [80.0, 120.0, 29.5]);
        assert_eq!(o.area, Some(area));
    }

    #[test]
    fn movers_keep_their_progress_when_a_move_is_reissued() {
        let mut s = sim();
        let area = s.world.spawn(ObjectKind::Area, "a");
        let id = s.world.spawn(ObjectKind::Creature, "c");
        s.world.get_mut(id).unwrap().area = Some(area);
        let to = crate::world::Location {
            area,
            position: [50.0, 0.0, 0.0],
            facing: 0.0,
        };
        let walk = || Command {
            ty: 0,
            name: "CommandMoveToLocation".into(),
            target: None,
            location: Some(to),
            seconds: 0.0,
            ints: Vec::new(),
            strings: Vec::new(),
            due: None,
            from: None,
            approach: None,
            ring_wait: false,
            after_impact_ms: None,
            impacted: false,
            arrived: false,
            path: Vec::new(),
        };
        s.world.get_mut(id).unwrap().commands.push(walk());
        // 5 m/s: two seconds in, ten metres along.
        s.tick(2000);
        let x = s.world.get(id).unwrap().position[0];
        assert!((x - 10.0).abs() < 0.01, "{x}");
        // Ordered again (as scripts re-issue moves): it goes on from there.
        let o = s.world.get_mut(id).unwrap();
        o.commands.clear();
        o.commands.push(walk());
        s.tick(8000);
        assert_eq!(s.world.get(id).unwrap().position, [50.0, 0.0, 0.0]);
        assert!(s.world.get(id).unwrap().commands.is_empty());
    }

    #[test]
    fn followers_arrive_behind_the_leader_not_on_it() {
        // A leader facing +X (heading_of 270 is 0 radians): followers stand at smaller x,
        // on either side, apart from each other.
        let at: Vec<[f32; 3]> = (0..3).map(|k| behind([10.0, 0.0, 1.0], 270.0, k)).collect();
        for p in &at {
            assert!(p[0] < 10.0 && p[2] == 1.0, "{p:?}");
        }
        assert!(at[0][1] > 0.0 && at[1][1] < 0.0);
        assert!(crate::engine::dist(at[0], at[2]) > 0.5);
    }

    #[test]
    fn followers_keep_up_with_a_leader_moved_by_an_order() {
        let mut s = sim();
        let area = s.world.spawn(ObjectKind::Area, "a");
        let hero = s.world.spawn(ObjectKind::Creature, "hero");
        let (dog, mage) = (
            s.world.spawn(ObjectKind::Creature, "dog"),
            s.world.spawn(ObjectKind::Creature, "mage"),
        );
        s.world.hero = Some(hero);
        s.world.area = Some(area);
        for (id, x) in [(hero, 0.0), (dog, -1.0), (mage, -2.0)] {
            let o = s.world.get_mut(id).unwrap();
            o.area = Some(area);
            o.position = [x, 0.0, 0.0];
        }
        s.world.get_mut(hero).unwrap().followers = vec![dog, mage];
        // The player clicks 20 m ahead: only the leader is ordered.
        s.order_move(hero, [20.0, 0.0, 0.0]);
        for _ in 0..100 {
            s.tick(100);
        }
        let at = |s: &Sim, id| s.world.get(id).unwrap().position;
        assert!((at(&s, hero)[0] - 20.0).abs() < 0.01);
        for f in [dog, mage] {
            let d = crate::engine::dist(at(&s, f), at(&s, hero));
            assert!(d < 3.0, "a follower stayed behind: {:?}", at(&s, f));
        }
        assert!(crate::engine::dist(at(&s, dog), at(&s, mage)) > 0.5);
        // A follower told to stay put does.
        s.world
            .get_mut(mage)
            .unwrap()
            .fields
            .insert("FollowPartyLeader".into(), 0);
        let before = at(&s, mage);
        s.order_move(hero, [40.0, 0.0, 0.0]);
        for _ in 0..100 {
            s.tick(100);
        }
        assert_eq!(at(&s, mage), before);
        assert!(crate::engine::dist(at(&s, dog), at(&s, hero)) < 3.0);
    }

    #[test]
    fn walkers_face_the_way_they_go() {
        let mut s = sim();
        let area = s.world.spawn(ObjectKind::Area, "a");
        let walker = s.world.spawn(ObjectKind::Creature, "w");
        // Each way: the facing taken while walking looks along it (heading_of the facing).
        for (dx, dy) in [
            (1.0, 0.0),
            (0.0, 1.0),
            (-1.0, 0.0),
            (0.0, -1.0),
            (1.0, 1.0),
            (-1.0, 1.0),
            (-1.0, -1.0),
            (1.0, -1.0),
        ] {
            let o = s.world.get_mut(walker).unwrap();
            o.area = Some(area);
            o.position = [0.0; 3];
            // Facing somewhere else to start with.
            o.facing = crate::world::facing_toward(-dy, dx);
            s.order_move(walker, [dx * 20.0, dy * 20.0, 0.0]);
            s.tick(500);
            let o = s.world.get(walker).unwrap();
            assert!(o.position[0] * dx >= 0.0 && o.position[1] * dy >= 0.0);
            let h = crate::world::heading_of(o.facing).to_radians();
            let want = dy.atan2(dx);
            let off = (h - want).sin().abs() + (1.0 - (h - want).cos());
            assert!(off < 1e-3, "walking ({dx}, {dy}): facing {}", o.facing);
        }
        // Along a route, the leg it is on: east, then north.
        let points = [[10.0, 0.0, 0.0], [10.0, 10.0, 0.0]];
        assert_eq!(direction_along([0.0; 3], &points, 0.0), Some((10.0, 0.0)));
        assert_eq!(direction_along([0.0; 3], &points, 0.25), Some((10.0, 0.0)));
        assert_eq!(direction_along([0.0; 3], &points, 0.75), Some((0.0, 10.0)));
        assert_eq!(direction_along([0.0; 3], &points, 1.0), Some((0.0, 10.0)));
        assert_eq!(direction_along([0.0; 3], &[[0.0, 0.0, 5.0]], 0.5), None);
    }

    #[test]
    fn attackers_out_of_reach_walk_up_first() {
        let mut s = sim();
        let area = s.world.spawn(ObjectKind::Area, "a");
        let (a, t) = (
            s.world.spawn(ObjectKind::Creature, "a"),
            s.world.spawn(ObjectKind::Creature, "t"),
        );
        for (id, x) in [(a, 0.0), (t, 12.0)] {
            let o = s.world.get_mut(id).unwrap();
            o.area = Some(area);
            o.position = [x, 0.0, 0.0];
        }
        s.world.get_mut(a).unwrap().commands.push(Command {
            ty: 0,
            name: "CommandAttack".into(),
            target: Some(t),
            location: None,
            seconds: 0.0,
            ints: Vec::new(),
            strings: Vec::new(),
            due: None,
            from: None,
            approach: None,
            ring_wait: false,
            after_impact_ms: None,
            impacted: false,
            arrived: false,
            path: Vec::new(),
        });
        // At 5 m/s: five metres after a second.
        s.tick(1000);
        let x = s.world.get(a).unwrap().position[0];
        assert!((x - 5.0).abs() < 0.01, "{x}");
        // It stops at the target's reach, facing it: two default bodies apart plus the
        // melee gap (its place on the ring).
        s.tick(1500);
        let reach = 2.0 * crate::combat::DEFAULT_RADIUS + crate::combat::MELEE_GAP;
        let o = s.world.get(a).unwrap();
        let x = o.position[0];
        assert!((x - (12.0 - reach)).abs() < 0.01, "{:?}", o.position);
        // There, the attack itself is under way (still the front command).
        assert_eq!(
            o.commands.first().map(|c| (c.name.as_str(), c.approach)),
            Some(("CommandAttack", None))
        );
    }

    /// Two creatures in an area, `a` at `pa` attacking `t` at `pt`, which walks to `to`.
    fn chase(pa: [f32; 3], pt: [f32; 3], to: [f32; 3]) -> (Sim, ObjectId, ObjectId) {
        let mut s = sim();
        let area = s.world.spawn(ObjectKind::Area, "a");
        let (a, t) = (
            s.world.spawn(ObjectKind::Creature, "a"),
            s.world.spawn(ObjectKind::Creature, "t"),
        );
        for (id, p) in [(a, pa), (t, pt)] {
            let o = s.world.get_mut(id).unwrap();
            o.area = Some(area);
            o.position = p;
        }
        let cmd = |name: &str, target, location| Command {
            ty: 0,
            name: String::from(name),
            target,
            location,
            seconds: 0.0,
            ints: Vec::new(),
            strings: Vec::new(),
            due: None,
            from: None,
            approach: None,
            ring_wait: false,
            after_impact_ms: None,
            impacted: false,
            arrived: false,
            path: Vec::new(),
        };
        let walk = cmd(
            "CommandMoveToLocation",
            None,
            Some(crate::world::Location {
                area,
                position: to,
                facing: 0.0,
            }),
        );
        s.world.get_mut(t).unwrap().commands.push(walk);
        let attack = cmd("CommandAttack", Some(t), None);
        s.world.get_mut(a).unwrap().commands.push(attack);
        (s, a, t)
    }

    #[test]
    fn a_target_walking_up_is_attacked_where_they_meet() {
        // They close at 10 m/s: within reach (two default bodies and the gap, 2 m) after
        // 1.8 s, the attacker at about 9 m, far short of where it was first heading.
        let (mut s, a, _) = chase([0.0; 3], [20.0, 0.0, 0.0], [0.0; 3]);
        s.tick(2000);
        let o = s.world.get(a).unwrap();
        assert!(o.position[0] < 10.0, "{:?}", o.position);
        assert_eq!(
            o.commands.first().map(|c| (c.name.as_str(), c.approach)),
            Some(("CommandAttack", None)),
            "the swing is under way"
        );
    }

    #[test]
    fn walkers_closing_in_look_again_in_short_ticks() {
        // The same meeting as above, stepped as the app steps the world (33 ms frames): the
        // attacker must still stop where they meet, not walk on through the target.
        let (mut s, a, t) = chase([0.0; 3], [20.0, 0.0, 0.0], [0.0; 3]);
        for _ in 0..61 {
            s.tick(33);
        }
        let (pa, pt) = (
            s.world.get(a).unwrap().position,
            s.world.get(t).unwrap().position,
        );
        assert!(pa[0] < 10.0, "{pa:?}");
        assert!(
            pt[0] > pa[0],
            "they did not pass through each other: {pa:?} {pt:?}"
        );
        assert_eq!(
            s.world
                .get(a)
                .unwrap()
                .commands
                .first()
                .map(|c| (c.name.as_str(), c.approach)),
            Some(("CommandAttack", None)),
            "the swing is under way"
        );
    }

    #[test]
    fn an_attacker_follows_a_target_that_moves_off() {
        // The target walks 10 m sideways; the attacker ends up beside it, not where it stood.
        let (mut s, a, t) = chase([0.0; 3], [10.0, 0.0, 0.0], [10.0, 10.0, 0.0]);
        s.tick(5000);
        let (pa, pt) = (
            s.world.get(a).unwrap().position,
            s.world.get(t).unwrap().position,
        );
        let d = ((pa[0] - pt[0]).powi(2) + (pa[1] - pt[1]).powi(2)).sqrt();
        // Within the reach and its slack, from where the swing may start (0x6a3dc0).
        let reach = 2.0 * crate::combat::DEFAULT_RADIUS
            + crate::combat::MELEE_GAP
            + crate::combat::MELEE_REACH_TOLERANCE;
        assert!(d <= reach + 0.01, "{pa:?} {pt:?}");
        assert!(pa[1] > 5.0, "it went after the target: {pa:?}");
    }

    #[test]
    fn a_melee_attack_skips_the_walk_only_within_the_reach_itself() {
        // Two default bodies and the gap: 2.4 m. From 2.3 m the attacker is by its place
        // already and the swing starts where it stands (0x6a3d00 needs no slack).
        let (mut s, a, _) = chase([0.0; 3], [2.3, 0.0, 0.0], [2.3, 0.0, 0.0]);
        if let Some(o) = s.world.get_mut(a) {
            o.commands.truncate(1);
        }
        s.tick(1);
        let o = s.world.get(a).unwrap();
        assert_eq!(o.position, [0.0; 3]);
        assert!(
            o.commands
                .first()
                .is_some_and(|c| c.approach.is_none() && c.due.is_some())
        );
        // From 2.8 m, inside the start step's slack but past the reach, it walks to its
        // place on the ring first (the exe's move step comes before the start step).
        let (mut s, a, _) = chase([0.0; 3], [2.8, 0.0, 0.0], [2.8, 0.0, 0.0]);
        if let Some(o) = s.world.get_mut(a) {
            o.commands.truncate(1);
        }
        s.tick(1);
        assert!(
            s.world
                .get(a)
                .unwrap()
                .commands
                .first()
                .is_some_and(|c| c.approach.is_some())
        );
        s.tick(1000);
        let o = s.world.get(a).unwrap();
        assert!((o.position[0] - 0.4).abs() < 0.01, "{:?}", o.position);
        assert!(
            o.commands
                .first()
                .is_some_and(|c| c.approach.is_none() && c.due.is_some())
        );
    }

    #[test]
    fn a_full_melee_ring_turns_attackers_away() {
        let mut s = sim();
        let area = s.world.spawn(ObjectKind::Area, "a");
        let t = s.world.spawn(ObjectKind::Creature, "target");
        let late = s.world.spawn(ObjectKind::Creature, "late");
        let ring = 2.0 * crate::combat::DEFAULT_RADIUS + crate::combat::MELEE_GAP;
        // Eight attackers already hold places all round the target: each covers its own
        // slot and two either side, so no span of five is left.
        let mut crowd = Vec::new();
        for k in 0..8 {
            let ang = (k as f32 * 45.0).to_radians();
            let c = s.world.spawn(ObjectKind::Creature, "crowd");
            let o = s.world.get_mut(c).unwrap();
            o.area = Some(area);
            o.position = [ring * ang.cos(), ring * ang.sin(), 0.0];
            o.commands.push(Command {
                ty: 7,
                name: "CommandAttack".into(),
                target: Some(t),
                location: None,
                seconds: 0.0,
                ints: Vec::new(),
                strings: Vec::new(),
                due: Some(u64::MAX / 2),
                from: None,
                approach: None,
                ring_wait: false,
                after_impact_ms: None,
                impacted: false,
                arrived: false,
                path: Vec::new(),
            });
            crowd.push(c);
        }
        for (id, x) in [(t, 0.0), (late, 8.0)] {
            let o = s.world.get_mut(id).unwrap();
            o.area = Some(area);
            o.position = [x, 0.0, 0.0];
        }
        s.world.get_mut(late).unwrap().commands.push(Command {
            ty: 7,
            name: "CommandAttack".into(),
            target: Some(t),
            location: None,
            seconds: 0.0,
            ints: Vec::new(),
            strings: Vec::new(),
            due: None,
            from: None,
            approach: None,
            ring_wait: false,
            after_impact_ms: None,
            impacted: false,
            arrived: false,
            path: Vec::new(),
        });
        // It walks to just outside the crowd and waits; the ring is still full, so the
        // attack fails and its owner hears so.
        s.tick(3000);
        let o = s.world.get(late).unwrap();
        assert!(o.commands.is_empty(), "{:?}", o.commands);
        let x = o.position[0];
        assert!(x > ring && x < 8.0, "{x}");
        // Its owner was told (COMMAND_COMPLETE, queued to it).
        assert!(
            s.world
                .trace
                .entries
                .iter()
                .any(|e| matches!(e, TraceEntry::EventQueued { target, .. } if *target == late))
        );
    }

    #[test]
    fn a_blocked_walker_keeps_the_corner_it_has_not_reached() {
        // An L-shaped path; the walker is held at (4.5, 0.3) before the corner at (5, 0).
        let from = [0.0, 0.0, 0.0];
        let points = [[5.0, 0.0, 0.0], [5.0, 5.0, 0.0]];
        let f = progress_of(from, &points, [4.5, 0.3, 0.0]);
        assert!((f - 0.45).abs() < 1e-3, "{f}");
        assert_eq!(remaining_points(from, &points, f), points.to_vec());
        // Past the corner, only the last leg is left.
        let f = progress_of(from, &points, [5.2, 2.0, 0.0]);
        assert_eq!(remaining_points(from, &points, f), vec![points[1]]);
    }

    #[test]
    fn a_blocked_walk_keeps_its_own_speed() {
        // A 10 m walk set going at 1 m/s (a scripted walk, slower than a run), with a body
        // just off its line: being held up must never make it arrive sooner.
        let mut s = sim();
        let area = s.world.spawn(ObjectKind::Area, "a");
        let w = s.world.spawn(ObjectKind::Creature, "walker");
        let post = s.world.spawn(ObjectKind::Creature, "post");
        for (id, p) in [(w, [0.0, 0.0, 0.0]), (post, [5.0, 0.05, 0.0])] {
            let o = s.world.get_mut(id).unwrap();
            o.area = Some(area);
            o.position = p;
        }
        let t0 = s.world.time_ms;
        s.world.get_mut(w).unwrap().commands.push(Command {
            ty: 0,
            name: "CommandMoveToLocation".into(),
            target: None,
            location: Some(crate::world::Location {
                area,
                position: [10.0, 0.0, 0.0],
                facing: 0.0,
            }),
            seconds: 0.0,
            ints: Vec::new(),
            strings: Vec::new(),
            due: Some(t0 + 10_000),
            from: Some(([0.0, 0.0, 0.0], t0)),
            approach: None,
            ring_wait: false,
            after_impact_ms: None,
            impacted: false,
            arrived: false,
            path: Vec::new(),
        });
        let mut arrived = None;
        for k in 1..=400 {
            s.tick(33);
            if arrived.is_none() && s.world.get(w).unwrap().commands.is_empty() {
                arrived = Some(k * 33);
            }
        }
        let arrived = arrived.expect("it arrives");
        assert!(arrived >= 10_000, "arrived after {arrived} ms");
    }

    #[test]
    fn a_walker_slides_round_a_creature_in_its_way() {
        let mut s = sim();
        let area = s.world.spawn(ObjectKind::Area, "a");
        let w = s.world.spawn(ObjectKind::Creature, "walker");
        let post = s.world.spawn(ObjectKind::Creature, "post");
        for (id, p) in [(w, [0.0, 0.0, 0.0]), (post, [5.0, 0.05, 0.0])] {
            let o = s.world.get_mut(id).unwrap();
            o.area = Some(area);
            o.position = p;
        }
        s.order_move(w, [10.0, 0.0, 0.0]);
        // Ticked as the app does, a frame at a time.
        let mut closest = f32::MAX;
        for _ in 0..150 {
            s.tick(33);
            let p = s.world.get(w).unwrap().position;
            closest = closest.min(((p[0] - 5.0).powi(2) + (p[1] - 0.05).powi(2)).sqrt());
        }
        // Never inside the two capsules (0.36 m each by default), and it gets there.
        let r = 2.0 * s.capsule_radius(w);
        assert!(closest >= r - 0.01, "{closest} < {r}");
        let p = s.world.get(w).unwrap().position;
        assert!((p[0] - 10.0).abs() < 0.05 && p[1].abs() < 0.05, "{p:?}");
        assert!(s.world.get(w).unwrap().commands.is_empty());
        // The dead do not block.
        s.world.get_mut(post).unwrap().dead = true;
        s.order_move(w, [0.0, 0.0, 0.0]);
        s.tick(1);
        assert!(
            s.world
                .get(w)
                .unwrap()
                .commands
                .first()
                .is_some_and(|c| c.path.len() <= 1)
        );
        s.tick(2_100);
        assert_eq!(s.world.get(w).unwrap().position, [0.0, 0.0, 0.0]);
    }

    #[test]
    fn creatures_move_at_their_appearance_speed() {
        use thedas_formats::gda::Cell;
        let mut s = sim();
        let t = crate::m2da::Tables::empty();
        // Walk, run, combat run and the run animation's reference speed (not a movement
        // speed).
        t.insert(
            "apr_base",
            &[
                "ID",
                "Label",
                "#958f23c8",
                "#a0dcd78c",
                "#0dc3ebb2",
                "#bd6d4446",
            ],
            vec![(
                15,
                vec![
                    Cell::Int(15),
                    Cell::String("Human".into()),
                    Cell::Float(1.54),
                    Cell::Float(6.0),
                    Cell::Float(4.66),
                    Cell::Float(3.62),
                ],
            )],
        );
        s.world.tables = Arc::new(t);
        let (human, other) = (
            s.world.spawn(ObjectKind::Creature, "human"),
            s.world.spawn(ObjectKind::Creature, "other"),
        );
        s.world.get_mut(human).unwrap().appearance = 15;
        assert_eq!(s.move_speed(human), 6.0, "a run out of combat");
        s.world.get_mut(human).unwrap().combat.in_combat = true;
        assert_eq!(s.move_speed(human), 4.66, "a run in combat stance");
        let walk = |run: i32| Command {
            ty: 0,
            name: "CommandMoveToLocation".into(),
            target: None,
            location: None,
            seconds: 0.0,
            ints: vec![run, 0],
            strings: Vec::new(),
            due: None,
            from: None,
            approach: None,
            ring_wait: false,
            after_impact_ms: None,
            impacted: false,
            arrived: false,
            path: Vec::new(),
        };
        assert_eq!(
            s.command_speed(human, &walk(0)),
            1.54,
            "told not to run: a walk"
        );
        assert_eq!(s.command_speed(human, &walk(1)), 4.66);
        assert_eq!(
            s.move_speed(other),
            DEFAULT_SPEED,
            "no appearance row: the default"
        );
    }

    #[test]
    fn created_objects_spawn_like_loaded_ones() {
        let mut s = sim();
        let area = s.world.spawn(ObjectKind::Area, "a");
        // No template in an empty resource stack: still created, traced as missing.
        let id = s.create_from_template(
            ObjectKind::Creature,
            "undead.utc",
            Some(area),
            [1.0, 2.0, 3.0],
            90.0,
        );
        let o = s.world.get(id).unwrap();
        assert_eq!((o.template.as_str(), o.tag.as_str()), ("undead", "undead"));
        assert_eq!(
            (o.area, o.position, o.facing),
            (Some(area), [1.0, 2.0, 3.0], 90.0)
        );
    }

    #[test]
    fn handles_are_freed_after_a_top_level_run() {
        let mut s = sim();
        let id = s.world.spawn(ObjectKind::Creature, "c");
        s.run_script("counter", id, Some(Event::default()));
        assert!(s.world.handles.is_empty());
    }

    #[test]
    fn delayed_events_run_in_order_at_their_time() {
        let mut s = sim();
        let id = s.world.spawn(ObjectKind::Creature, "c");
        s.world.get_mut(id).unwrap().script = Some("counter".into());
        s.world.queue_event(
            1000,
            id,
            Event {
                ty: 1,
                ..Default::default()
            },
            None,
        );
        s.world.queue_event(
            500,
            id,
            Event {
                ty: 2,
                ..Default::default()
            },
            None,
        );
        let n = |s: &Sim| {
            s.world
                .get(id)
                .unwrap()
                .locals
                .ints
                .get("n")
                .copied()
                .unwrap_or(0)
        };
        s.tick(400);
        assert_eq!(n(&s), 0);
        s.tick(200);
        assert_eq!(n(&s), 1);
        assert_eq!(s.world.time_ms, 600);
        s.tick(1000);
        assert_eq!(n(&s), 2);
        let delivered: Vec<(u64, i32)> = s
            .world
            .trace
            .entries
            .iter()
            .filter_map(|e| match e {
                TraceEntry::EventDelivered { t, ty, .. } => Some((*t, *ty)),
                _ => None,
            })
            .collect();
        assert_eq!(delivered, vec![(500, 2), (1000, 1)]);
    }

    #[test]
    fn runs_are_deterministic() {
        let run = || {
            let mut s = sim();
            let id = s.world.spawn(ObjectKind::Creature, "c");
            s.world.get_mut(id).unwrap().script = Some("counter".into());
            for k in 0..20u64 {
                s.world
                    .queue_event((k * 37) % 300, id, Event::default(), None);
            }
            s.tick(1000);
            (s.world.trace.to_jsonl(), s.world.rng.next_u64())
        };
        assert_eq!(run(), run());
    }

    #[test]
    fn missing_scripts_and_objects_are_traced() {
        let mut s = sim();
        let id = s.world.spawn(ObjectKind::Creature, "c");
        s.world.get_mut(id).unwrap().script = Some("nope".into());
        s.world.queue_event(0, id, Event::default(), None);
        s.world.queue_event(0, 9999, Event::default(), None);
        s.tick(10);
        let missing = s
            .world
            .trace
            .entries
            .iter()
            .filter(|e| matches!(e, TraceEntry::Missing { .. }))
            .count();
        assert_eq!(missing, 2);
    }
}
