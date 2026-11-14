//! Engine functions, dispatched by name (names and signatures come from `script.ldf` at run
//! time). Implemented in order of how often the shipped scripts call them; see
//! `thedas script stats`. Anything not here is traced as unimplemented and returns the default
//! for its return type.
//!
//! Behaviour that is inferred rather than verified against the original is noted per function.
//! Functions that only affect presentation (visual effects, sounds, floaty text, UI hints)
//! are accepted as no-ops: the simulation is headless and presentation reads it, never the
//! other way round.

use crate::fmath::DetMath;
use thedas_formats::gda::Cell;
use thedas_script::ldf::{Function, Type};
use thedas_script::vm::{OBJECT_INVALID, ObjectId, Value, VmResult};

use crate::sim::{ScriptHost, Sim, constant};
use crate::trace::TraceEntry;
use crate::world::{Command, Effect, EngineData, Event, Location, ObjectKind};

fn int(a: &[Value], i: usize) -> i32 {
    match a.get(i) {
        Some(Value::Int(v)) => *v,
        Some(Value::Float(v)) => *v as i32,
        _ => 0,
    }
}

fn float(a: &[Value], i: usize) -> f32 {
    match a.get(i) {
        Some(Value::Float(v)) => *v,
        Some(Value::Int(v)) => *v as f32,
        _ => 0.0,
    }
}

fn string(a: &[Value], i: usize) -> String {
    match a.get(i) {
        Some(Value::String(s)) | Some(Value::Resource(s)) => s.clone(),
        _ => String::new(),
    }
}

fn obj(a: &[Value], i: usize) -> ObjectId {
    match a.get(i) {
        Some(Value::Object(o)) => *o,
        _ => OBJECT_INVALID,
    }
}

/// What `GetHasEffects`, `GetEffects` and `RemoveEffectsByParameters` select effects by, as
/// script.ldf documents them: an effect type (`EFFECT_TYPE_INVALID`, 0, for any), an ability
/// (−1 for any, `GetHasEffects`' default; 0, `ABILITY_INVALID`, for any, the other two's
/// default), a creator (`OBJECT_INVALID` for any) and a duration type
/// (`EFFECT_DURATION_TYPE_INVALID`, 0, for any). DAOrigins.exe agrees on the ability: the one
/// handler of both queries (0x673d70) turns −1 into 0, and its filter (0xa64950) skips the
/// ability test for 0. Provisional: `GetEffects`' effect id filter is not applied (effects
/// carry no ids yet).
#[derive(Clone, Copy, Debug)]
struct EffectFilter {
    ty: i32,
    ability: Option<i32>,
    creator: Option<ObjectId>,
    duration_type: i32,
}

impl EffectFilter {
    fn new(ty: i32, ability: i32, creator: ObjectId, duration_type: i32) -> Self {
        EffectFilter {
            ty,
            ability: (ability != -1 && ability != 0).then_some(ability),
            creator: (creator != OBJECT_INVALID).then_some(creator),
            duration_type,
        }
    }

    fn matches(&self, e: &Effect) -> bool {
        (self.ty == 0 || e.ty == self.ty)
            && self.ability.is_none_or(|a| e.ability == a)
            && self.creator.is_none_or(|c| e.creator == Some(c))
            && (self.duration_type == 0 || e.duration_type == self.duration_type)
    }
}

fn handle(a: &[Value], i: usize) -> Option<u64> {
    match a.get(i) {
        Some(Value::Engine { handle, .. }) => *handle,
        _ => None,
    }
}

fn vector(a: &[Value], i: usize) -> [f32; 3] {
    match a.get(i) {
        Some(Value::Vector(v)) => *v,
        _ => [0.0; 3],
    }
}

fn ok(v: Value) -> Option<VmResult<Option<Value>>> {
    Some(Ok(Some(v)))
}

fn void() -> Option<VmResult<Option<Value>>> {
    Some(Ok(None))
}

/// Dragon Age II creature fields its scripts set and read (`SetCreatureAmbientActivityPattern`
/// and the rest); DA2's templates do not carry them.
const DA2_AMBIENT_ACTIVITY: &str = "AmbientActivityPattern";
const DA2_AMBIENT_MOVEMENT: &str = "AmbientMovementPattern";

/// DA2's rating conversion (DragonAge2.exe 0x697c00, command 861 at 0x4ed090).
/// Table 371 selects one of four level curves in table 405. Below the optimal rating
/// the fraction follows a power curve; above it, each further optimal-sized increment
/// halves the distance to the ceiling. Column names are unknown, so use their hashes.
fn da2_percentage_from_scaled(sim: &Sim, level: i32, scaled: f32, property: i32) -> f32 {
    let tables = sim.world.tables();
    let Some(functions) = tables.get(371, "") else {
        return 0.0;
    };
    let row = property as i64;
    let Some(Cell::Int(selector)) = functions.cell_by_hash(0x1d78_b8e3, row) else {
        return 0.0;
    };
    let columns = [0x73a8_6afe, 0xe337_46bf, 0xf9fc_7170, 0x6e1d_17ea];
    let Some(&column) = usize::try_from(*selector).ok().and_then(|i| columns.get(i)) else {
        return 0.0;
    };
    let optimal = cell_float(
        tables
            .get(405, "")
            .and_then(|t| t.cell_by_hash(column, level as i64).cloned())
            .as_ref(),
    );
    if optimal <= 0.0 || optimal.is_nan() {
        return 0.0;
    }
    let Some(knee) = functions
        .cell_by_hash(0x2c4e_dea1, row)
        .filter(|c| !matches!(c, Cell::Empty))
    else {
        return 0.0;
    };
    let knee = cell_float(Some(knee)) as f64;
    let ratio = scaled as f64 / optimal as f64;
    let fraction = if scaled <= optimal {
        let exponent = cell_float(functions.cell_by_hash(0x7fc1_781c, row)) as f64;
        knee * ratio.dpowf(exponent)
    } else {
        let ceiling = cell_float(functions.cell_by_hash(0x5ee2_2fe7, row)) as f64;
        ceiling - (ceiling - knee) * 0.5_f64.dpowf(ratio - 1.0)
    };
    fraction as f32
}

/// DA2 command 646 (0x51af60 -> 0x560a00). Item abilities (type 4) are never
/// returned. The cooldown-only path enumerates active cooldown records, including an
/// ability since removed from the creature; the ordinary path reads known abilities.
fn da2_ability_list(sim: &Sim, who: ObjectId, ty: i32, cooling: bool) -> Vec<i32> {
    let Some(o) = sim
        .world
        .get(who)
        .filter(|o| o.kind == ObjectKind::Creature)
    else {
        return Vec::new();
    };
    if ty == 4 {
        return Vec::new();
    }
    let tables = sim.world.tables();
    let abilities = tables.get(1, "");
    // DA2 uses this column, not DA:O's `AbilityType` column (0xe1a8f4ea).
    let cell = |id| {
        abilities
            .as_ref()
            .and_then(|t| t.cell_by_hash(0x100b_492a, id))
    };
    let matches_type = |id| {
        let kind = cell_int(cell(id));
        kind != 4 && (ty == 0 || ty == kind)
    };
    if cooling {
        let mut ids: Vec<i32> = o
            .combat
            .cooldowns
            .iter()
            .filter(|(_, until)| **until > sim.world.time_ms)
            .filter_map(|(key, _)| key.split_once(':')?.0.parse().ok())
            .filter(|&id| matches_type(id as i64))
            .collect();
        // Keep the simulation's ability lists in deterministic id order. Distinct
        // cooldown records may name the same ability, and retain their multiplicity.
        ids.sort_unstable();
        ids
    } else {
        o.abilities
            .iter()
            .copied()
            .filter(|&id| cell(id as i64).is_some_and(|c| !matches!(c, Cell::Empty)))
            .filter(|&id| matches_type(id as i64))
            .collect()
    }
}

/// The party's inventory slots before any backpack (provisional).
pub const DEFAULT_INVENTORY_SIZE: i32 = 70;

fn store_field(h: &ScriptHost, store: ObjectId, name: &str, default: i32) -> i32 {
    h.object(store)
        .and_then(|o| o.fields.get(name).copied())
        .unwrap_or(default)
}

fn set_store_field(
    h: &mut ScriptHost,
    store: ObjectId,
    name: &str,
    v: i32,
) -> Option<VmResult<Option<Value>>> {
    if let Some(o) = h.sim.world.get_mut(store) {
        o.fields.insert(name.to_string(), v);
    }
    void()
}

fn b(v: bool) -> Value {
    Value::Int(v as i32)
}

fn objects(items: Vec<ObjectId>) -> Value {
    Value::Array {
        elem: Type::Object,
        items: items.into_iter().map(Value::Object).collect(),
    }
}

pub(crate) fn cell_int(c: Option<&Cell>) -> i32 {
    match c {
        Some(Cell::Int(v)) => *v as i32,
        Some(Cell::Float(v)) => *v as i32,
        Some(Cell::String(s)) => clean(s).parse().unwrap_or(0),
        _ => 0,
    }
}

/// GDA strings carry a trailing NUL and sometimes padding.
pub(crate) fn clean(s: &str) -> &str {
    s.trim_matches(|c: char| c == '\0' || c.is_whitespace())
}

pub(crate) fn cell_float(c: Option<&Cell>) -> f32 {
    match c {
        Some(Cell::Int(v)) => *v as f32,
        Some(Cell::Float(v)) => *v as f32,
        Some(Cell::String(s)) => clean(s).parse().unwrap_or(0.0),
        _ => 0.0,
    }
}

pub(crate) fn cell_string(c: Option<&Cell>) -> String {
    match c {
        Some(Cell::Int(v)) => v.to_string(),
        Some(Cell::Float(v)) => v.to_string(),
        Some(Cell::String(s)) => clean(s).to_string(),
        _ => String::new(),
    }
}

/// The name of the local variable argument `a[i]` names on object `o`. Dragon Age: Origins
/// names it; Dragon Age II's scripts pass a number instead: the variable's row in the
/// object's variable table (`var_module`, `var_area`, `var_creature`, `var_placeable`,
/// `var_item`, `var_trigger`), whose `Label` column is the name templates use. An object
/// whose template's variables are not that table's rows (a talk trigger's `TRIG_TALK_*`,
/// which are `var_trigger_talk`'s) numbers them by their place in its template's list,
/// which follows its own table's rows (row 0 `TRIG_TALK_SPEAKER`, 2 the conversation). A
/// number neither answers keeps its digits as its name. Provisional: read from DA2's scripts,
/// tables and templates (`PlayCutscene`'s callers set `var_module` rows 1014 to 1016,
/// `gen00tr_talk` reads its trigger's speaker at 0), not traced in the executable.
pub(crate) fn local_name(sim: &Sim, o: ObjectId, a: &[Value], i: usize) -> String {
    let Some(Value::Int(row)) = a.get(i) else {
        return string(a, i);
    };
    let obj = sim.world.get(o);
    let table = match obj.map(|o| o.kind) {
        Some(ObjectKind::Module) => "var_module",
        Some(ObjectKind::Area) => "var_area",
        Some(ObjectKind::Creature) => "var_creature",
        Some(ObjectKind::Placeable) => "var_placeable",
        Some(ObjectKind::Item) => "var_item",
        Some(ObjectKind::Trigger) => "var_trigger",
        _ => return row.to_string(),
    };
    let Some(t) = sim.world.tables().get(0, table) else {
        return row.to_string();
    };
    let name = cell_string(t.cell("Label", *row as i64));
    // An object whose template's variables are another table's (its first is none of this
    // table's labels) numbers them by their place in its template's list.
    let order = obj.map(|o| &o.locals.order);
    if let Some(first) = order.and_then(|l| l.first()) {
        let foreign = !t
            .order
            .iter()
            .any(|&r| cell_string(t.cell("Label", r)) == *first);
        if foreign {
            if let Some(placed) = order.and_then(|l| l.get(usize::try_from(*row).ok()?)) {
                return placed.clone();
            }
        }
    }
    if name.is_empty() {
        row.to_string()
    } else {
        name
    }
}

/// NWScript's float-to-string: `%<width>.<decimals>f`, right-aligned. Provisional for
/// `ToString` on floats (assumed `%f` with six decimals).
fn format_float(f: f32, width: i32, decimals: i32) -> String {
    let d = decimals.clamp(0, 9) as usize;
    let s = format!("{f:.d$}");
    format!("{s:>w$}", w = width.clamp(0, 18) as usize)
}

fn origins_trim_number_space(s: &str) -> &str {
    // The low-byte C-locale table accepts space, 9..13 and NBSP. Above U+00ff,
    // retain our Unicode-space approximation of the native OS classification.
    s.trim_start_matches(|c: char| {
        matches!(c, ' ' | '\t'..='\r' | '\u{a0}') || (c as u32 >= 0x100 && c.is_whitespace())
    })
}

/// Origins' shared StringToInt/HexStringToInt handler uses signed wide strtol,
/// base zero (0x64f510 -> 0x4c0950 -> 0x8e835f). See notes/script-integer-parsing.md.
fn origins_string_to_int(s: &str) -> i32 {
    // The CRT's wide digit helper (0x8fc637) recognizes these decimal families.
    let digit = |c: char| {
        [
            0x30, 0x660, 0x6f0, 0x966, 0x9e6, 0xa66, 0xae6, 0xb66, 0xc66, 0xce6, 0xd66, 0xe50,
            0xed0, 0xf20, 0x1040, 0x17e0, 0x1810, 0xff10,
        ]
        .into_iter()
        .find_map(|start| {
            (start..start + 10)
                .contains(&(c as u32))
                .then(|| c as u32 - start)
        })
    };
    let mut chars = origins_trim_number_space(s).chars().peekable();
    let negative = chars.peek() == Some(&'-');
    if matches!(chars.peek(), Some('+' | '-')) {
        chars.next();
    }
    let radix = if chars.peek().and_then(|&c| digit(c)) == Some(0) {
        let mut prefix = chars.clone();
        prefix.next();
        if matches!(prefix.peek(), Some('x' | 'X')) {
            chars.next();
            chars.next();
            16
        } else {
            8
        }
    } else {
        10
    };
    let limit = i64::from(i32::MAX) + i64::from(negative);
    let mut value = 0i64;
    for c in chars {
        let Some(d) = digit(c).or_else(|| c.to_digit(36)) else {
            break;
        };
        if d >= radix {
            break;
        }
        value = (value * i64::from(radix) + i64::from(d)).min(limit);
    }
    (if negative { -value } else { value }) as i32
}

/// Origins' wide strtod grammar (0x8effcf); see notes/script-float-parsing.md.
/// The old CRT's intermediate decimal-to-binary rounding remains unverified.
fn origins_string_to_float(s: &str) -> f32 {
    let s = origins_trim_number_space(s);
    let bytes = s.as_bytes();
    let mut end = usize::from(matches!(bytes.first(), Some(b'+' | b'-')));
    let mut digits = 0;
    while bytes.get(end).is_some_and(u8::is_ascii_digit) {
        end += 1;
        digits += 1;
    }
    if bytes.get(end) == Some(&b'.') {
        end += 1;
        while bytes.get(end).is_some_and(u8::is_ascii_digit) {
            end += 1;
            digits += 1;
        }
    }
    if digits == 0 {
        return 0.0;
    }
    if matches!(bytes.get(end), Some(b'd' | b'D' | b'e' | b'E')) {
        let mut exponent = end + 1;
        if matches!(bytes.get(exponent), Some(b'+' | b'-')) {
            exponent += 1;
        }
        let start = exponent;
        while bytes.get(exponent).is_some_and(u8::is_ascii_digit) {
            exponent += 1;
        }
        if exponent > start {
            end = exponent;
        }
    }
    // The native member narrows a double to a float; it does not parse directly
    // to single precision. Normalize the CRT's alternate exponent spelling.
    s[..end]
        .replace(['d', 'D'], "e")
        .parse::<f64>()
        .unwrap_or(0.0) as f32
}

/// Functions that only drive presentation; accepted as no-ops.
const PRESENTATION: &[&str] = &[
    "SetAtmosphere",
    "SetAtmosphereRGB",
    "FB_SetEffectResource",
    "FB_SetEffectEnabled",
    "PlaySoundSet",
    "PlaySound",
    "PlaySoundObject",
    "LinkAreaMiniMaps",
    "SendGameEventRunDatabase",
    "SetPlaceableHealth",
    "SetMapPinState",
    "SetCameraTarget",
    "ApplyEffectVisualEffectOnLocation",
    "SetLookAtEnabled",
    "SetSoundSet",
    "PlayMusic",
    "SetMusicIntensity",
    "SendToRunDatabase",
    "SetCreatureGoreLevel",
    // Only the indicator on the quickbar; the modal effect itself is the scripts'.
    "Engine_SetModalAbilityGUI",
    // An icon over a creature's head (DAOrigins.exe 0x662db0): interface only.
    "ShowFloatyIcon",
];

/// Engine functions answered with a fixed placeholder because the simulation lacks what
/// they need (the plot summary text, ...). Calls to them are counted (`Trace::stubbed`) so
/// coverage work can follow what the game uses.
pub const STUBS: &[&str] = &["GetPlotSummary"];

/// Whether an object lies in a script shape at `l` (DAOrigins.exe 0x670eb0): its body is a
/// circle on the ground (z is ignored) of the creature's personal space (0 for other
/// objects). A sphere (`SHAPE_SPHERE`) of radius `fa` takes it when the circles touch
/// (0x4eb340). A cone (`SHAPE_CONE`) of `fa` degrees (0 to 180) and length `fb` along the
/// location's facing takes it when it touches the cone's circle and lies between the
/// cone's edges or within its radius of one (0x4eb3b0). Provisional: the body radius
/// (the engine asks the object, virtual 0x9c) is taken to be [`Sim::body_radius`].
fn in_shape(sim: &Sim, id: ObjectId, shape: i32, l: &Location, fa: f32, fb: f32) -> bool {
    let Some(o) = sim.world.get(id) else {
        return false;
    };
    let r = if o.kind == ObjectKind::Creature {
        sim.body_radius(id)
    } else {
        0.0
    };
    let (px, py) = (o.position[0] - l.position[0], o.position[1] - l.position[1]);
    let within = |reach: f32| px * px + py * py <= (r + reach) * (r + reach);
    match shape {
        1 => within(fa),
        2 => {
            if !(0.0..=180.0).contains(&fa) || !within(fb) {
                return false;
            }
            let heading = (l.facing + 90.0).to_radians();
            let half = (fa * 0.5).to_radians();
            let edge = |a: f32| [a.dcos() * fb, a.dsin() * fb];
            let (left, right) = (edge(heading + half), edge(heading - half));
            let cross = |e: [f32; 2]| e[0] * py - e[1] * px;
            if cross(right) >= 0.0 && cross(left) <= 0.0 {
                return true;
            }
            let near_edge = |e: [f32; 2]| {
                let len2 = e[0] * e[0] + e[1] * e[1];
                let t = if len2 > 0.0 {
                    ((px * e[0] + py * e[1]) / len2).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let (dx, dy) = (px - e[0] * t, py - e[1] * t);
                dx * dx + dy * dy <= r * r
            };
            near_edge(left) || near_edge(right)
        }
        _ => false,
    }
}

/// `ABI_base`'s column (hash 0x5d3e0787, name unknown) naming the `VFX Persistent` row
/// (M2DA 96) of an ability's lasting area, whose `Radius` then sizes its clusters.
const ABI_PERSISTENT_AREA: u32 = 0x5d3e_0787;
/// The M2DA id of `VFX Persistent`.
const VFX_PERSISTENT_TABLE: i32 = 96;

/// `GetClusterCenter`, as DAOrigins.exe works it out (0x671980):
/// - The cluster's span is the ability's `aoe_param1` in `ABI_base` (or, for an ability with
///   a lasting area, its `VFX Persistent` row's `Radius`), doubled for a sphere
///   (`aoe_type` 1).
/// - The candidates are the creatures the creator perceives. Each hostile one in turn is a
///   cluster's center; the others join it, in candidate order, when within the span of the
///   center and of every member already in (a quirk kept: distances are read from a table
///   filled row by row, so a member after the center, or one that was never a center, is
///   not measured against and lets anyone in).
/// - A cluster with at least `size` foes scores foes less allies; one scoring at least the
///   best so far replaces it unless a d100 roll falls below allies × `ally_fail`.
/// - The answer is the mean position of the best cluster's foes. The first candidate's
///   cluster can never be chosen (the engine tests the best index for being above 0).
///
/// `bReturnFirstMatch` is read but unused. Provisional: the candidates are the
/// simulation's perception list (the engine's perception query, which also drops creatures
/// beyond a per-creature limit, is not traced), and creatures the engine skips (virtual
/// 0x1d0 returning 1) are taken to be the dead.
fn cluster_center(
    sim: &mut Sim,
    creator: ObjectId,
    ability: i32,
    size: i32,
    ally_fail: i32,
) -> Option<[f32; 3]> {
    let me = sim.world.get(creator)?;
    if me.kind != ObjectKind::Creature {
        return None;
    }
    let candidates: Vec<(ObjectId, [f32; 3], bool)> = me
        .combat
        .perceived
        .iter()
        .filter_map(|&c| sim.world.get(c).map(|o| (c, o.position, o.dead)))
        .collect();
    let tables = sim.world.tables.clone();
    let abi = tables.get(0, "ABI_base");
    let abi_cell = |col: &str| {
        abi.as_ref()
            .and_then(|t| t.cell(col, ability as i64).cloned())
    };
    let persistent = abi.as_ref().map_or(0, |t| {
        cell_int(t.cell_by_hash(ABI_PERSISTENT_AREA, ability as i64))
    });
    let mut span = if persistent == 0 {
        cell_float(abi_cell("aoe_param1").as_ref())
    } else {
        tables
            .get(VFX_PERSISTENT_TABLE, "")
            .map_or(0.0, |t| cell_float(t.cell("Radius", persistent as i64)))
    };
    if cell_int(abi_cell("aoe_type").as_ref()) == 1 {
        span *= 2.0;
    }
    let n = candidates.len();
    let hostile: Vec<bool> = candidates
        .iter()
        .map(|c| sim.hostile(creator, c.0))
        .collect();
    let mut table = vec![vec![0.0f32; n]; n];
    let mut clusters: Vec<Vec<usize>> = vec![Vec::new(); n];
    let (mut best_score, mut best) = (-1i32, -1i64);
    for i in 0..n {
        if !hostile[i] || candidates[i].2 {
            continue;
        }
        let at = candidates[i].1;
        let mut members: Vec<usize> = Vec::new();
        let mut allies = 0;
        'join: for j in 0..n {
            let d = if j == i {
                0.0
            } else {
                if candidates[j].2 {
                    continue;
                }
                let cached = if j < i { table[j][i] } else { 0.0 };
                if cached != 0.0 {
                    cached
                } else {
                    dist(at, candidates[j].1)
                }
            };
            table[i][j] = d;
            if span < d {
                continue;
            }
            if d != 0.0 {
                for &m in &members {
                    // As DAOrigins.exe reads it (0x671ecc-0x671ef4): row m of the table for
                    // m <= i, which every iteration sizes to the candidates, zero-filled,
                    // before its center is tested (0x671d34, 0x652430's memset), so the row
                    // of a candidate that was never a center (an ally, the dead) reads 0 and
                    // excludes no one; for m > i, the center's own distance to m
                    // (0x671ef6-0x671f49), which is within the span already.
                    let dm = if m <= i {
                        table[m][j]
                    } else {
                        dist(at, candidates[m].1)
                    };
                    if span < dm {
                        continue 'join;
                    }
                }
            }
            members.push(j);
            if !hostile[j] {
                allies += 1;
            }
        }
        let foes = members.len() as i32 - allies;
        if foes >= size {
            let score = foes - allies;
            if score >= best_score && sim.world.rng.below(100) >= allies * ally_fail {
                best_score = score;
                best = i as i64;
            }
        }
        clusters[i] = members;
    }
    if best <= 0 {
        return None;
    }
    let best = best as usize;
    let foes: Vec<[f32; 3]> = clusters[best]
        .iter()
        .filter(|&&m| hostile[m])
        .map(|&m| candidates[m].1)
        .collect();
    let k = foes.len() as f32;
    let sum = foes
        .iter()
        .fold([0.0f32; 3], |s, p| [s[0] + p[0], s[1] + p[1], s[2] + p[2]]);
    Some([sum[0] / k, sum[1] / k, sum[2] / k])
}

/// DAOrigins.exe's degrees-to-radians factor (0xaf14c0; a little under pi/180).
const DEG_TO_RAD: f64 = 0.017_453_279_346_227_646;
/// DAOrigins.exe's radians-to-degrees factor (0xaf0e50).
const RAD_TO_DEG: f64 = 57.295_825_958_251_95;

/// `PROPERTY_DEPLETABLE_MANA_STAMINA`, the property the mana functions use (DAOrigins.exe
/// indexes the creature's property list with 8 directly).
const PROPERTY_MANA_STAMINA: i32 = 8;

/// The effect behind `effect` as an apply function hands it on: with its duration type, a
/// temporary effect's end (`duration` seconds from now), its creator (if valid) and the
/// ability it comes from (`GetEffectAbilityID`; if given and not 0).
fn applied_effect(
    h: &ScriptHost,
    effect: Option<u64>,
    duration_type: i32,
    duration: f32,
    creator: ObjectId,
    ability: Option<i32>,
) -> Option<Effect> {
    let Some(EngineData::Effect(e)) = h.sim.world.handle(effect) else {
        return None;
    };
    let mut e = e.clone();
    e.duration_type = duration_type;
    if creator != OBJECT_INVALID && creator != 0 {
        e.creator = Some(creator);
    }
    if let Some(ability) = ability.filter(|&x| x != 0) {
        e.ability = ability;
    }
    let temporary = constant(&h.sim.table, "EFFECT_DURATION_TYPE_TEMPORARY");
    if e.duration_type == temporary && duration > 0.0 {
        e.expires = Some(h.sim.world.time_ms + (duration.min(1.0e6) * 1000.0) as u64);
    }
    Some(e)
}

/// Applies a visual effect (type 4) to an object as any effect is applied; an instant one,
/// which leaves nothing on the object, is noted for presentation ([`crate::world::InstantVfx`]).
fn apply_visual_or_effect(h: &mut ScriptHost, target: ObjectId, e: Effect) {
    let instant = constant(&h.sim.table, "EFFECT_DURATION_TYPE_INSTANT");
    if e.ty == 4 && e.duration_type == instant && h.object(target).is_some() {
        let note = crate::world::InstantVfx {
            target,
            vfx: e.ints.first().copied().unwrap_or(0),
            started_ms: h.sim.world.time_ms,
        };
        h.sim.world.instant_vfx.push(note);
    }
    // DA2 area save-load handlers rebuild environmental visuals (the prologue
    // firewall does this on AREALOADSAVE_POSTLOADEXIT). Our snapshots already
    // contain those effects. Reconcile an exact replay after running APPLY_EFFECT,
    // retaining its script side effects and any unrelated or temporary visuals.
    // This is snapshot compatibility, not a general rule against stacking VFX.
    let replay = (h.sim.game == thedas_vfs::Game::DragonAge2
        && e.ty == 4
        && e.ability == 0
        && e.expires.is_none()
        && e.duration_type == constant(&h.sim.table, "EFFECT_DURATION_TYPE_PERMANENT")
        && e.creator.is_some_and(|area| {
            h.object(area).is_some_and(|o| o.kind == ObjectKind::Area)
                && h.object(target)
                    .is_some_and(|o| o.kind == ObjectKind::Placeable && o.area == Some(area))
        })
        && matches!(h.sim.world.handle(h.event), Some(EngineData::Event(ev)) if matches!(
            h.sim.event_name(ev.ty).as_str(),
            "AREALOADSAVE_SPECIAL" | "AREALOADSAVE_PRELOADEXIT" | "AREALOADSAVE_POSTLOADEXIT"
        )))
    .then(|| e.clone());
    h.sim.apply_effect(target, e);
    if let (Some(replay), Some(o)) = (replay, h.sim.world.get_mut(target)) {
        let mut kept = false;
        o.effects.retain(|e| {
            if *e != replay {
                return true;
            }
            let first = !kept;
            kept = true;
            first
        });
    }
}

/// Converts `CommandMoveToObject` to `COMMAND_TYPE_MOVE_TO_OBJECT`.
fn command_constant(name: &str) -> String {
    // Where script.ldf spells the type differently from the constructor.
    match name {
        "CommandDeathBlow" => return "COMMAND_TYPE_DEATHBLOW".into(),
        "CommandSwitchWeaponSet" => return "COMMAND_TYPE_SWITCH_WEAPON_SETS".into(),
        _ => {}
    }
    let base = name.trim_start_matches("Command");
    let mut out = String::from("COMMAND_TYPE_");
    for (i, c) in base.chars().enumerate() {
        if c.is_ascii_uppercase() && i > 0 {
            out.push('_');
        }
        out.push(c.to_ascii_uppercase());
    }
    out
}

/// Origins constructors whose runtime type script.ldf does not name. Traced assignments
/// go through 0x742090 and are read back by GetCommandType (0x66eba0). Different
/// constructors may share a type; their payload and executor still distinguish them.
const UNNAMED_COMMANDS: &[(&str, i32)] = &[
    ("CommandMoveAwayFromObject", 21),   // 0x66f564
    ("CommandMoveToMultiLocations", 22), // 0x673383
    ("CommandSheatheWeapons", 39),       // 0x66fbb9
    ("CommandUnsheatheWeapons", 39),     // same handler, different payload
    ("CommandTurn", 41),                 // 0x66eabe
];

/// A command constructor's `COMMAND_TYPE_*` value (see [`UNNAMED_COMMANDS`]).
fn command_type(table: &thedas_script::EngineTable, name: &str, game: thedas_vfs::Game) -> i32 {
    match UNNAMED_COMMANDS.iter().position(|(n, _)| *n == name) {
        Some(i) if game == thedas_vfs::Game::Origins => UNNAMED_COMMANDS[i].1,
        // DA2's assignments have not been traced; keep its existing placeholders.
        Some(i) => 1000 + i as i32,
        None => constant(table, &command_constant(name)),
    }
}

/// Runs engine function `f`; `None` if it is not implemented.
#[allow(clippy::too_many_lines)]
pub fn call(
    h: &mut ScriptHost,
    caller: ObjectId,
    f: &Function,
    a: &[Value],
) -> Option<VmResult<Option<Value>>> {
    let name = f.name.as_str();
    // Sounds go to the presentation (see `crate::world::SoundRequest`).
    let sound = match name {
        "PlaySound" => Some(crate::world::SoundRequest::Event {
            target: obj(a, 0),
            event: string(a, 1),
        }),
        "PlaySoundSet" => Some(crate::world::SoundRequest::SoundSet {
            target: obj(a, 0),
            entry: int(a, 1),
            probability: float(a, 2),
        }),
        "PlaySoundObject" => Some(crate::world::SoundRequest::Object { sound: obj(a, 0) }),
        "PlayMusic" => Some(crate::world::SoundRequest::Music { name: string(a, 0) }),
        _ => None,
    };
    // `SetSoundSet(oTarget, sSoundSet)`: the creature's barks come from that set.
    if name == "SetMusicIntensity" {
        h.sim.world.music_intensity = int(a, 0);
    }
    if name == "SetSoundSet" {
        h.sim.set_sound_set(obj(a, 0), &string(a, 1));
    }
    if let Some(req) = sound {
        // Kept short when nothing takes them (a headless run).
        if h.sim.world.sounds.len() >= 64 {
            h.sim.world.sounds.remove(0);
        }
        h.sim.world.sounds.push(req);
    }
    if PRESENTATION.contains(&name) {
        *h.sim
            .world
            .trace
            .presentation
            .entry(name.to_string())
            .or_default() += 1;
        return void();
    }
    if STUBS.contains(&name) {
        *h.sim
            .world
            .trace
            .stubbed
            .entry(name.to_string())
            .or_default() += 1;
    }
    let ty = h.sim.types;
    let t = h.sim.world.time_ms;
    match name {
        // --- Conversions and strings -------------------------------------------------------
        "ToString" => ok(Value::String(match a.first() {
            Some(Value::Int(v)) => v.to_string(),
            Some(Value::Float(v)) => format_float(*v, 0, 6),
            Some(Value::String(s)) | Some(Value::Resource(s)) => s.clone(),
            Some(Value::Object(o)) => o.to_string(),
            Some(other) => format!("{other:?}"),
            None => String::new(),
        })),
        "IntToString" => ok(Value::String(int(a, 0).to_string())),
        "FloatToString" => ok(Value::String(format_float(
            float(a, 0),
            if a.len() > 1 { int(a, 1) } else { 18 },
            if a.len() > 2 { int(a, 2) } else { 9 },
        ))),
        "IntToFloat" => ok(Value::Float(int(a, 0) as f32)),
        "FloatToInt" => ok(Value::Int(float(a, 0) as i32)),
        "StringToInt" | "HexStringToInt" if h.sim.game == thedas_vfs::Game::Origins => {
            ok(Value::Int(origins_string_to_int(&string(a, 0))))
        }
        "StringToInt" => ok(Value::Int(string(a, 0).trim().parse().unwrap_or(0))),
        "StringToFloat" if h.sim.game == thedas_vfs::Game::Origins => {
            ok(Value::Float(origins_string_to_float(&string(a, 0))))
        }
        "StringToFloat" => ok(Value::Float(string(a, 0).trim().parse().unwrap_or(0.0))),
        "ResourceToString" => ok(Value::String(string(a, 0))),
        "StringToResource" => ok(Value::Resource(string(a, 0))),
        // The running script's name with ".ncs" (DAOrigins.exe 0x654d70).
        "GetCurrentScriptResource" => ok(Value::Resource(format!(
            "{}.ncs",
            h.script.trim_end_matches(".ncs")
        ))),

        "GetStringLength" => ok(Value::Int(string(a, 0).chars().count() as i32)),
        "StringUpperCase" => ok(Value::String(string(a, 0).to_uppercase())),
        "StringLowerCase" => ok(Value::String(string(a, 0).to_lowercase())),
        "StringLeft" => ok(Value::String(
            string(a, 0)
                .chars()
                .take(int(a, 1).max(0) as usize)
                .collect(),
        )),
        "StringRight" => {
            let s: Vec<char> = string(a, 0).chars().collect();
            let n = (int(a, 1).max(0) as usize).min(s.len());
            ok(Value::String(s[s.len() - n..].iter().collect()))
        }
        "SubString" => {
            let s: Vec<char> = string(a, 0).chars().collect();
            let start = (int(a, 1).max(0) as usize).min(s.len());
            let n = (int(a, 2).max(0) as usize).min(s.len() - start);
            ok(Value::String(s[start..start + n].iter().collect()))
        }
        // Counted in characters, as SubString, StringLeft and StringRight are.
        "FindSubString" => {
            let s: Vec<char> = string(a, 0).chars().collect();
            let sub: Vec<char> = string(a, 1).chars().collect();
            let start = int(a, 2).max(0) as usize;
            let found = (start..=s.len().saturating_sub(sub.len()))
                .find(|&i| i + sub.len() <= s.len() && s[i..i + sub.len()] == sub[..]);
            ok(Value::Int(found.map_or(-1, |i| i as i32)))
        }
        "abs" => ok(Value::Int(int(a, 0).wrapping_abs())),
        "fabs" => ok(Value::Float(float(a, 0).abs())),
        "sqrt" => ok(Value::Float(float(a, 0).max(0.0).sqrt())),
        // Radians, per script.ldf; out-of-domain inputs are clamped rather than giving NaN.
        "sin" => ok(Value::Float(float(a, 0).dsin())),
        "cos" => ok(Value::Float(float(a, 0).dcos())),
        "tan" => ok(Value::Float(float(a, 0).dtan())),
        "asin" => ok(Value::Float(float(a, 0).clamp(-1.0, 1.0).dasin())),
        "acos" => ok(Value::Float(float(a, 0).clamp(-1.0, 1.0).dacos())),
        "atan" => ok(Value::Float(float(a, 0).datan())),
        "log" => ok(Value::Float(float(a, 0).max(f32::MIN_POSITIVE).dln())),
        "pow" => ok(Value::Float(float(a, 0).dpowf(float(a, 1)))),
        "Random" => ok(Value::Int(h.sim.world.rng.below(int(a, 0)))),
        "RandomFloat" => ok(Value::Float(h.sim.world.rng.unit())),
        "Vector" => ok(Value::Vector([float(a, 0), float(a, 1), float(a, 2)])),
        // Whether x, y and z each compare equal to 0.0 (DAOrigins.exe's handler for function
        // 40, 0x64f010): -0.0 counts as empty, NaN does not, as float comparison gives.
        "IsVectorEmpty" => ok(b(vector(a, 0) == [0.0; 3])),
        // DAOrigins.exe's conversions (0x64f260, 0x64f310), in its own constants: an angle
        // a (degrees) is the ground vector (-sin a, cos a, 0), and a vector's angle is
        // atan2(-x, y) in degrees, -180 to 180 (0x4ee570).
        "AngleToVector" => {
            let r = -(float(a, 0) as f64) * DEG_TO_RAD;
            ok(Value::Vector([r.dsin() as f32, r.dcos() as f32, 0.0]))
        }
        "VectorToAngle" => {
            let v = vector(a, 0);
            let (x, y) = (-(v[0] as f64), v[1] as f64);
            ok(Value::Float((x.datan2(y) * RAD_TO_DEG) as f32))
        }
        // "%f %f %f" (0x64f9d0).
        "VectorToString" => {
            let v = vector(a, 0);
            ok(Value::String(format!(
                "{:.6} {:.6} {:.6}",
                v[0] as f64, v[1] as f64, v[2] as f64
            )))
        }

        "GetArraySize" => ok(Value::Int(match a.first() {
            Some(Value::Array { items, .. }) => items.len() as i32,
            _ => 0,
        })),

        // --- Logging ----------------------------------------------------------------------
        "LogTrace"
        | "PrintToLog"
        | "PrintString"
        | "PrintInteger"
        | "PrintFloat"
        | "Warning"
        | "DisplayFloatyMessage"
        | "DisplayPortraitMessage"
        | "PrintToLogWindow" => {
            let (channel, text) = match name {
                "LogTrace" => (int(a, 0), string(a, 1)),
                "PrintInteger" => (0, int(a, 0).to_string()),
                "PrintFloat" => (0, float(a, 0).to_string()),
                // A message by a party member's portrait (DAOrigins.exe 0x661f40 hands it
                // to the interface): logged as floaty messages are.
                "DisplayFloatyMessage" | "DisplayPortraitMessage" => (-1, string(a, 1)),
                _ => (0, string(a, 0)),
            };
            if name == "DisplayFloatyMessage" {
                // Kept short when nothing takes them (a headless run).
                if h.sim.world.floaties.len() >= 64 {
                    h.sim.world.floaties.remove(0);
                }
                h.sim.world.floaties.push(crate::world::FloatyMessage {
                    target: obj(a, 0),
                    text: text.clone(),
                    style: int(a, 2),
                    colour: int(a, 3),
                    duration: float(a, 4),
                });
            }
            if name == "DisplayPortraitMessage" {
                // Kept short when nothing takes them (a headless run).
                if h.sim.world.portrait_messages.len() >= 64 {
                    h.sim.world.portrait_messages.remove(0);
                }
                h.sim
                    .world
                    .portrait_messages
                    .push(crate::world::PortraitMessage {
                        target: obj(a, 0),
                        text: text.clone(),
                        colour: int(a, 2),
                    });
            }
            if name == "Warning" {
                let key: String = text.chars().take(160).collect();
                *h.sim.world.trace.warnings.entry(key).or_default() += 1;
            }
            h.sim.world.trace.push(TraceEntry::Log { t, channel, text });
            void()
        }

        // --- Objects ----------------------------------------------------------------------
        "GetModule" => ok(Value::Object(h.sim.world.module.unwrap_or(OBJECT_INVALID))),
        "GetHero" => ok(Value::Object(h.sim.world.hero.unwrap_or(OBJECT_INVALID))),
        "GetPartyLeader" | "GetMainControlled" => ok(Value::Object(
            h.sim.world.controlled().unwrap_or(OBJECT_INVALID),
        )),
        "IsHero" => ok(b(Some(obj(a, 0)) == h.sim.world.hero)),
        "IsControlled" => ok(b(
            !h.sim.world.autopilot && Some(obj(a, 0)) == h.sim.world.controlled()
        )),
        "IsFollower" => {
            let o = obj(a, 0);
            let hero = h.sim.world.hero;
            let follower = hero
                .and_then(|x| h.object(x))
                .is_some_and(|x| x.followers.contains(&o));
            ok(b(Some(o) == hero || follower))
        }
        "GetParty" => ok(Value::Object(h.sim.world.party.unwrap_or(OBJECT_INVALID))),
        "GetPartyList" => {
            let v: Vec<ObjectId> = party_of(h.sim)
                .into_iter()
                .filter(|&m| h.object(m).is_some())
                .collect();
            ok(objects(v))
        }
        "GetArea" => ok(Value::Object(
            h.object(obj(a, 0))
                .and_then(|o| {
                    if o.kind == ObjectKind::Area {
                        Some(o.id)
                    } else {
                        o.area
                    }
                })
                .unwrap_or(OBJECT_INVALID),
        )),
        "IsObjectValid" => ok(b(h.object(obj(a, 0)).is_some())),
        "GetObjectType" => ok(Value::Int(
            h.object(obj(a, 0)).map_or(0, |o| o.kind.script_type()),
        )),
        "GetTag" => ok(Value::String(
            h.object(obj(a, 0))
                .map(|o| o.tag.clone())
                .unwrap_or_default(),
        )),
        "SetTag" => {
            if let Some(o) = h.sim.world.get_mut(obj(a, 0)) {
                o.tag = string(a, 1);
            }
            void()
        }
        "GetResRef" => ok(Value::String(
            h.object(obj(a, 0))
                .map(|o| o.template.clone())
                .unwrap_or_default(),
        )),
        "GetName" => ok(Value::String(h.sim.display_name(obj(a, 0)))),
        // An empty name gives the template's back. Provisional.
        "SetName" => {
            let name = string(a, 1);
            if let Some(o) = h.sim.world.get_mut(obj(a, 0)) {
                o.name = (!name.is_empty()).then_some(name);
            }
            void()
        }
        "GetObjectByTag" => {
            let n = if a.len() > 1 {
                int(a, 1).max(0) as usize
            } else {
                0
            };
            ok(Value::Object(
                h.sim
                    .world
                    .find_tag(&string(a, 0), n)
                    .unwrap_or(OBJECT_INVALID),
            ))
        }
        "GetObjectsInArea" => {
            let area = obj(a, 0);
            let tag = string(a, 1).to_ascii_lowercase();
            ok(objects(
                h.sim
                    .world
                    .objects
                    .values()
                    .filter(|o| {
                        o.area == Some(area)
                            && (tag.is_empty() || o.tag.to_ascii_lowercase() == tag)
                    })
                    .map(|o| o.id)
                    .collect(),
            ))
        }
        "GetNearestObjectByTag"
        | "GetNearestObject"
        | "UT_GetNearestObjectByTag"
        | "GetNearestObjectByGroup"
        | "GetNearestObjectByHostility" => {
            let from = obj(a, 0);
            let by_group = (name == "GetNearestObjectByGroup").then(|| int(a, 1));
            let by_hostility = (name == "GetNearestObjectByHostility").then(|| int(a, 1) != 0);
            let (tag, kind, count, include_self) = match name {
                "GetNearestObjectByTag" => (string(a, 1), int(a, 2), int(a, 3), int(a, 6)),
                "GetNearestObjectByGroup" | "GetNearestObjectByHostility" => {
                    (String::new(), int(a, 2), int(a, 3), int(a, 6))
                }
                "GetNearestObject" => (String::new(), int(a, 1), int(a, 2), int(a, 5)),
                _ => (string(a, 1), -1, 1, int(a, 2)),
            };
            let (living, perceived) = match name {
                "GetNearestObjectByTag"
                | "GetNearestObjectByGroup"
                | "GetNearestObjectByHostility" => (int(a, 4) != 0, int(a, 5) != 0),
                "GetNearestObject" => (int(a, 3) != 0, int(a, 4) != 0),
                _ => (false, false),
            };
            let seen = h
                .object(from)
                .map(|o| o.combat.perceived.clone())
                .unwrap_or_default();
            // From an area, search that area (its own `area` is none).
            let origin = h.object(from).map(|o| {
                if o.kind == ObjectKind::Area {
                    (o.position, Some(o.id))
                } else {
                    (o.position, o.area)
                }
            });
            let Some(origin) = origin else {
                return ok(if name == "UT_GetNearestObjectByTag" {
                    Value::Object(OBJECT_INVALID)
                } else {
                    objects(Vec::new())
                });
            };
            let tag = tag.to_ascii_lowercase();
            let mut found: Vec<(f32, ObjectId)> = h
                .sim
                .world
                .objects
                .values()
                .filter(|o| o.area == origin.1 && o.area.is_some())
                .filter(|o| include_self != 0 || o.id != from)
                .filter(|o| tag.is_empty() || o.tag.to_ascii_lowercase() == tag)
                .filter(|o| !living || !o.dead)
                .filter(|o| by_group.is_none_or(|g| o.group == g))
                .filter(|o| by_hostility.is_none_or(|x| h.sim.hostile(from, o.id) == x))
                .filter(|o| !perceived || seen.contains(&o.id))
                .filter(|o| {
                    kind == -1
                        || kind == 0
                        || (kind as u32 as i64 & o.kind.script_type() as i64) != 0
                })
                .map(|o| (dist(o.position, origin.0), o.id))
                .collect();
            found.sort_by(|x, y| x.0.total_cmp(&y.0).then(x.1.cmp(&y.1)));
            let ids: Vec<ObjectId> = found.into_iter().map(|(_, id)| id).collect();
            if name == "UT_GetNearestObjectByTag" {
                // Nothing here: the object anywhere (the party scripts find followers
                // left in another area this way). Provisional.
                let any = ids.first().copied().or_else(|| {
                    h.sim
                        .world
                        .find_tag(&tag, 0)
                        .filter(|&o| include_self != 0 || o != from)
                });
                ok(Value::Object(any.unwrap_or(OBJECT_INVALID)))
            } else {
                ok(objects(
                    ids.into_iter().take(count.max(1) as usize).collect(),
                ))
            }
        }
        "GetObjectActive" => ok(b(h.object(obj(a, 0)).is_some_and(|o| o.active))),
        "SetObjectActive" => {
            if let Some(o) = h.sim.world.get_mut(obj(a, 0)) {
                o.active = int(a, 1) != 0;
            }
            void()
        }
        "IsDead" => ok(b(h.object(obj(a, 0)).is_some_and(|o| o.dead))),
        "IsImmortal" => ok(b(h.object(obj(a, 0)).is_some_and(|o| o.immortal))),
        "SetImmortal" | "Engine_SetImmortal" => {
            if let Some(o) = h.sim.world.get_mut(obj(a, 0)) {
                o.immortal = int(a, 1) != 0;
            }
            void()
        }
        "IsPlot" => ok(b(h.object(obj(a, 0)).is_some_and(|o| o.plot))),
        "SetPlot" => {
            if let Some(o) = h.sim.world.get_mut(obj(a, 0)) {
                o.plot = int(a, 1) != 0;
            }
            void()
        }
        // DAOrigins.exe 0x666450 -> 0x6a25a0 -> 0x69f030: a creature's
        // 16-bit appearance indexes table 2, column 0xde80b418 (combat tree).
        // The predicate is a case-sensitive substring search, not a creature-kind test
        // or an exact match of the whole resource name. DA2 uses different tree names;
        // preserve its prior creature-kind answer until its predicate is traced.
        "IsHumanoid" => ok(b(h
            .object(obj(a, 0))
            .filter(|o| o.kind == ObjectKind::Creature)
            .is_some_and(|o| {
                h.sim.game != thedas_vfs::Game::Origins
                    || h.sim.world.tables.get(2, "").is_some_and(|t| {
                        matches!(t.cell_by_hash(0xde80b418, i64::from(o.appearance as u16)),
                            Some(Cell::String(s)) if clean(s).contains("combat_humanoid"))
                    })
            }))),
        "GetAppearanceType" => ok(Value::Int(h.object(obj(a, 0)).map_or(0, |o| o.appearance))),
        "GetCreatureRank" => ok(Value::Int(h.object(obj(a, 0)).map_or(0, |o| o.rank))),
        "GetGroupId" => ok(Value::Int(h.object(obj(a, 0)).map_or(0, |o| o.group))),
        "SetGroupId" => {
            if let Some(o) = h.sim.world.get_mut(obj(a, 0)) {
                o.group = int(a, 1);
            }
            void()
        }
        "GetTeamId" => ok(Value::Int(h.object(obj(a, 0)).map_or(-1, |o| o.team))),
        "SetTeamId" => {
            if let Some(o) = h.sim.world.get_mut(obj(a, 0)) {
                o.team = int(a, 1);
            }
            void()
        }
        "GetPosition" => ok(Value::Vector(
            h.object(obj(a, 0)).map_or([0.0; 3], |o| o.position),
        )),
        "SetPosition" => {
            if let Some(o) = h.sim.world.get_mut(obj(a, 0)) {
                o.position = vector(a, 1);
            }
            void()
        }
        // DAOrigins.exe (0x65b3b0) puts the object in the location's area if it is in
        // another and that area exists, then sets its position and orientation. Provisional:
        // like SetPosition, no trigger is told of the move.
        "SetLocation" => {
            let l = match h.sim.world.handle(handle(a, 1)) {
                Some(EngineData::Location(l)) => *l,
                _ => return void(),
            };
            let area_exists = h.object(l.area).is_some_and(|o| o.kind == ObjectKind::Area);
            if let Some(o) = h.sim.world.get_mut(obj(a, 0)) {
                if area_exists {
                    o.area = Some(l.area);
                }
                o.position = l.position;
                o.facing = l.facing;
            }
            void()
        }
        "GetFacing" => ok(Value::Float(h.object(obj(a, 0)).map_or(0.0, |o| o.facing))),
        "SetFacing" => {
            if let Some(o) = h.sim.world.get_mut(obj(a, 0)) {
                o.facing = float(a, 1);
            }
            void()
        }
        "GetDistanceBetween" => {
            let (x, y) = (h.object(obj(a, 0)), h.object(obj(a, 1)));
            ok(Value::Float(match (x, y) {
                (Some(x), Some(y)) if x.area == y.area => dist(x.position, y.position),
                _ => -1.0,
            }))
        }
        "GetLocation" => {
            let l = h.location_of(obj(a, 0)).unwrap_or(Location {
                area: OBJECT_INVALID,
                position: [0.0; 3],
                facing: 0.0,
            });
            let hd = h.sim.world.store(EngineData::Location(l));
            ok(Value::Engine {
                ty: ty.location,
                handle: Some(hd),
            })
        }
        "Location" => {
            let l = Location {
                area: obj(a, 0),
                position: vector(a, 1),
                facing: float(a, 2),
            };
            let hd = h.sim.world.store(EngineData::Location(l));
            ok(Value::Engine {
                ty: ty.location,
                handle: Some(hd),
            })
        }
        // A location with a new position or facing (DAOrigins.exe 0x65b580, 0x65b6f0: the
        // popped copy is changed and pushed back). An invalid location stays invalid
        // (with its new part).
        "SetLocationPosition" | "SetLocationFacing" => {
            let mut l = match h.sim.world.handle(handle(a, 0)) {
                Some(EngineData::Location(l)) => *l,
                _ => Location {
                    area: OBJECT_INVALID,
                    position: [0.0; 3],
                    facing: 0.0,
                },
            };
            if name == "SetLocationPosition" {
                l.position = vector(a, 1);
            } else {
                l.facing = float(a, 1);
            }
            let hd = h.sim.world.store(EngineData::Location(l));
            ok(Value::Engine {
                ty: ty.location,
                handle: Some(hd),
            })
        }
        // DAOrigins.exe 0x65b0d0: the location moved to a safe spot by the safe-location
        // search (see [`Sim::safe_location`]); an invalid location comes back as it is.
        "GetSafeLocation" => {
            let l = match h.sim.world.handle(handle(a, 0)) {
                Some(EngineData::Location(l)) => h.sim.safe_location(*l),
                _ => Location {
                    area: OBJECT_INVALID,
                    position: [0.0; 3],
                    facing: 0.0,
                },
            };
            let hd = h.sim.world.store(EngineData::Location(l));
            ok(Value::Engine {
                ty: ty.location,
                handle: Some(hd),
            })
        }
        "GetPositionFromLocation"
        | "GetAreaFromLocation"
        | "GetFacingFromLocation"
        | "GetOrientationFromLocation" => {
            let l = match h.sim.world.handle(handle(a, 0)) {
                Some(EngineData::Location(l)) => *l,
                _ => Location {
                    area: OBJECT_INVALID,
                    position: [0.0; 3],
                    facing: 0.0,
                },
            };
            ok(match name {
                "GetPositionFromLocation" => Value::Vector(l.position),
                "GetAreaFromLocation" => Value::Object(l.area),
                // Provisional: the facing as a rotation about z, in degrees.
                "GetOrientationFromLocation" => Value::Vector([0.0, 0.0, l.facing]),
                _ => Value::Float(l.facing),
            })
        }
        "IsInTrigger" => {
            let (o, tr) = (h.object(obj(a, 0)), h.object(obj(a, 1)));
            ok(b(match (o, tr) {
                (Some(o), Some(tr)) => o.area == tr.area && in_trigger(o.position, tr),
                _ => false,
            }))
        }
        "DestroyObject" => {
            h.sim.destroy_object(obj(a, 0));
            void()
        }
        "CreateObject" => {
            let kind = match int(a, 0) {
                32 => ObjectKind::Creature,
                512 => ObjectKind::Placeable,
                64 => ObjectKind::Item,
                4096 => ObjectKind::Waypoint,
                128 => ObjectKind::Trigger,
                _ => ObjectKind::Placeable,
            };
            let template = string(a, 1)
                .trim_end_matches(".utc")
                .trim_end_matches(".utp")
                .to_string();
            let loc = match h.sim.world.handle(handle(a, 2)) {
                Some(EngineData::Location(l)) => Some(*l),
                _ => None,
            };
            let (area, pos, facing) = match loc {
                Some(l) => (Some(l.area), l.position, l.facing),
                None => (None, [0.0; 3], 0.0),
            };
            let id = h
                .sim
                .create_from_template(kind, &template, area, pos, facing);
            let active = a.get(4).is_none_or(|_| int(a, 4) != 0);
            if let Some(o) = h.sim.world.get_mut(id) {
                let script = string(a, 3);
                if !script.is_empty() {
                    o.script = Some(script);
                }
                o.active = active;
            }
            // Created creatures and placeables hear EVENT_TYPE_SPAWN, as those an area loads
            // do.
            if matches!(kind, ObjectKind::Creature | ObjectKind::Placeable) {
                let ev = h.sim.new_event("SPAWN", Some(id));
                h.sim.world.queue_event(0, id, ev, None);
            }
            ok(Value::Object(id))
        }

        // --- Locals -----------------------------------------------------------------------
        "GetLocalInt" => ok(Value::Int(
            h.object(obj(a, 0))
                .and_then(|o| {
                    o.locals
                        .ints
                        .get(&local_name(h.sim, obj(a, 0), a, 1))
                        .copied()
                })
                .unwrap_or(0),
        )),
        // DAOrigins.exe 0x66be50 (function 813): the creature's answer to a restriction
        // script's question (+0xc70), read by [`Sim::can_use_item`].
        "SetCanUseItem" => {
            h.sim.can_use_answers.insert(obj(a, 0), int(a, 1));
            void()
        }
        // DAOrigins.exe 0x665710: 0x6ae8b0 on the creature and item ([`Sim::can_use_item`]).
        "CanUseItem" => {
            let ok_ = h.sim.can_use_item(obj(a, 0), obj(a, 1));
            ok(Value::Int(ok_ as i32))
        }
        "SetLocalInt" => {
            let k = local_name(h.sim, obj(a, 0), a, 1);
            if let Some(o) = h.sim.world.get_mut(obj(a, 0)) {
                o.locals.ints.insert(k, int(a, 2));
            }
            void()
        }
        "GetLocalFloat" => ok(Value::Float(
            h.object(obj(a, 0))
                .and_then(|o| {
                    o.locals
                        .floats
                        .get(&local_name(h.sim, obj(a, 0), a, 1))
                        .copied()
                })
                .unwrap_or(0.0),
        )),
        "SetLocalFloat" => {
            let k = local_name(h.sim, obj(a, 0), a, 1);
            if let Some(o) = h.sim.world.get_mut(obj(a, 0)) {
                o.locals.floats.insert(k, float(a, 2));
            }
            void()
        }
        "GetLocalString" => ok(Value::String(
            h.object(obj(a, 0))
                .and_then(|o| {
                    o.locals
                        .strings
                        .get(&local_name(h.sim, obj(a, 0), a, 1))
                        .cloned()
                })
                .unwrap_or_default(),
        )),
        "SetLocalString" => {
            let k = local_name(h.sim, obj(a, 0), a, 1);
            if let Some(o) = h.sim.world.get_mut(obj(a, 0)) {
                o.locals.strings.insert(k, string(a, 2));
            }
            void()
        }
        "GetLocalObject" => ok(Value::Object(
            h.object(obj(a, 0))
                .and_then(|o| {
                    o.locals
                        .objects
                        .get(&local_name(h.sim, obj(a, 0), a, 1))
                        .copied()
                })
                .unwrap_or(OBJECT_INVALID),
        )),
        "SetLocalObject" => {
            let k = local_name(h.sim, obj(a, 0), a, 1);
            if let Some(o) = h.sim.world.get_mut(obj(a, 0)) {
                o.locals.objects.insert(k, obj(a, 2));
            }
            void()
        }
        "GetLocalResource" => ok(Value::Resource(
            h.object(obj(a, 0))
                .and_then(|o| {
                    o.locals
                        .resources
                        .get(&local_name(h.sim, obj(a, 0), a, 1))
                        .cloned()
                })
                .unwrap_or_default(),
        )),
        "SetLocalResource" => {
            let k = local_name(h.sim, obj(a, 0), a, 1);
            if let Some(o) = h.sim.world.get_mut(obj(a, 0)) {
                o.locals.resources.insert(k, string(a, 2));
            }
            void()
        }
        "GetLocalLocation" => {
            let l = h.object(obj(a, 0)).and_then(|o| {
                o.locals
                    .locations
                    .get(&local_name(h.sim, obj(a, 0), a, 1))
                    .copied()
            });
            let hd = l.map(|l| h.sim.world.store(EngineData::Location(l)));
            ok(Value::Engine {
                ty: ty.location,
                handle: hd,
            })
        }
        "SetLocalLocation" => {
            let k = local_name(h.sim, obj(a, 0), a, 1);
            if let Some(EngineData::Location(l)) = h.sim.world.handle(handle(a, 2)).cloned() {
                if let Some(o) = h.sim.world.get_mut(obj(a, 0)) {
                    o.locals.locations.insert(k, l);
                }
            }
            void()
        }

        // --- Events -----------------------------------------------------------------------
        "Event" => {
            let hd = h.sim.world.store(EngineData::Event(Event {
                ty: int(a, 0),
                creator: Some(caller),
                ..Default::default()
            }));
            ok(Value::Engine {
                ty: ty.event,
                handle: Some(hd),
            })
        }
        "GetCurrentEvent" => ok(Value::Engine {
            ty: ty.event,
            handle: h.event,
        }),
        // Dragon Age II: the type of the event the script is handling, without the event
        // (provisional: read from its script.ldf signature, `int GetCurrentEventType()`).
        "GetCurrentEventType" => ok(Value::Int(match h.sim.world.handle(h.event) {
            Some(EngineData::Event(e)) => e.ty,
            _ => 0,
        })),
        "GetEventType" | "GetEventCreator" | "GetEventTarget" | "GetEventInteger"
        | "GetEventFloat" | "GetEventString" | "GetEventObject" | "GetEventResource"
        | "GetEventVector" => {
            let ev = match h.sim.world.handle(handle(a, 0)) {
                Some(EngineData::Event(e)) => e.clone(),
                _ => Event::default(),
            };
            let i = int(a, 1).max(0) as usize;
            ok(match name {
                "GetEventType" => Value::Int(ev.ty),
                "GetEventCreator" => Value::Object(ev.creator.unwrap_or(OBJECT_INVALID)),
                "GetEventTarget" => Value::Object(ev.target.unwrap_or(OBJECT_INVALID)),
                "GetEventInteger" => Value::Int(ev.ints.get(i).copied().unwrap_or(0)),
                "GetEventFloat" => Value::Float(ev.floats.get(i).copied().unwrap_or(0.0)),
                "GetEventString" => Value::String(ev.strings.get(i).cloned().unwrap_or_default()),
                "GetEventResource" => {
                    Value::Resource(ev.resources.get(i).cloned().unwrap_or_default())
                }
                // Dragon Age II's vector parameters; one never set reads as the zero vector.
                "GetEventVector" => Value::Vector(ev.vectors.get(i).copied().unwrap_or([0.0; 3])),
                _ => Value::Object(ev.objects.get(i).copied().unwrap_or(OBJECT_INVALID)),
            })
        }
        "SetEventType" | "SetEventCreator" | "SetEventInteger" | "SetEventFloat"
        | "SetEventString" | "SetEventObject" | "SetEventResource" | "SetEventVector" => {
            let mut ev = match h.sim.world.handle(handle(a, 0)) {
                Some(EngineData::Event(e)) => e.clone(),
                _ => Event::default(),
            };
            let i = int(a, 1).max(0) as usize;
            fn put<T: Clone + Default>(v: &mut Vec<T>, i: usize, x: T) {
                if i < 64 {
                    if v.len() <= i {
                        v.resize(i + 1, T::default());
                    }
                    v[i] = x;
                }
            }
            match name {
                "SetEventType" => ev.ty = int(a, 1),
                "SetEventCreator" => ev.creator = Some(obj(a, 1)),
                "SetEventInteger" => put(&mut ev.ints, i, int(a, 2)),
                "SetEventFloat" => put(&mut ev.floats, i, float(a, 2)),
                "SetEventString" => put(&mut ev.strings, i, string(a, 2)),
                "SetEventResource" => put(&mut ev.resources, i, string(a, 2)),
                "SetEventVector" => put(&mut ev.vectors, i, vector(a, 2)),
                _ => put(&mut ev.objects, i, obj(a, 2)),
            }
            let hd = h.sim.world.store(EngineData::Event(ev));
            ok(Value::Engine {
                ty: ty.event,
                handle: Some(hd),
            })
        }
        "SignalEvent" | "DelayEvent" => {
            let (target, hi, delay) = if name == "SignalEvent" {
                (obj(a, 0), 1, 0.0)
            } else {
                (obj(a, 1), 2, float(a, 0))
            };
            let Some(EngineData::Event(ev)) = h.sim.world.handle(handle(a, hi)).cloned() else {
                return void();
            };
            let script = if name == "DelayEvent" {
                Some(string(a, 3)).filter(|s| !s.is_empty())
            } else {
                None
            };
            if name == "SignalEvent" && int(a, 2) != 0 {
                h.sim.deliver(target, ev, None);
            } else {
                let ms = (delay.max(0.0) * 1000.0).round() as u64;
                h.sim.world.queue_event(ms, target, ev, script);
            }
            void()
        }
        "HandleEvent" => {
            let ev = match h.sim.world.handle(handle(a, 0)) {
                Some(EngineData::Event(e)) => e.clone(),
                _ => return void(),
            };
            let script = string(a, 1);
            if script.is_empty() {
                // With no script, the engine's default handling applies; we have none.
                return void();
            }
            let target = ev.target.unwrap_or(caller);
            h.sim.run_script(&script, target, Some(ev));
            void()
        }

        // --- 2DA --------------------------------------------------------------------------
        "GetM2DAInt" | "GetM2DAFloat" | "GetM2DAString" | "GetM2DAResource" => {
            let table = h.sim.world.tables.get(int(a, 0), &string(a, 3));
            let c = table
                .as_ref()
                .and_then(|t| t.cell(&string(a, 1), int(a, 2) as i64));
            // A missing table or column (not a missing row, which scripts probe for) is
            // counted for coverage.
            let missing = match &table {
                None => Some(format!("table {} {}", int(a, 0), string(a, 3))),
                Some(t)
                    if !t
                        .columns
                        .contains_key(&thedas_formats::gda::column_hash(&string(a, 1))) =>
                {
                    Some(format!(
                        "table {} {} column {}",
                        int(a, 0),
                        string(a, 3),
                        string(a, 1)
                    ))
                }
                _ => None,
            };
            if let Some(m) = missing {
                *h.sim.world.trace.m2da_missing.entry(m).or_default() += 1;
            }
            ok(match name {
                "GetM2DAInt" => Value::Int(cell_int(c)),
                "GetM2DAFloat" => Value::Float(cell_float(c)),
                "GetM2DAString" => Value::String(cell_string(c)),
                _ => Value::Resource(cell_string(c)),
            })
        }
        // `(n2DA, nColumnHash, nRow)`: the column is given by its name's hash, as the
        // tables key their columns. DA:O has the Int one; Dragon Age II adds the others
        // (provisional: read like their `GetM2DA*` counterparts; Bool is the cell as an
        // integer, non-zero for true).
        "GetHashedM2DAInt"
        | "GetHashedM2DABool"
        | "GetHashedM2DAFloat"
        | "GetHashedM2DAString"
        | "GetHashedM2DAResource" => {
            let table = h.sim.world.tables.get(int(a, 0), "");
            let c = table
                .as_ref()
                .and_then(|t| t.cell_by_hash(int(a, 1) as u32, int(a, 2) as i64));
            ok(match name {
                "GetHashedM2DAInt" => Value::Int(cell_int(c)),
                "GetHashedM2DABool" => Value::Int((cell_int(c) != 0) as i32),
                "GetHashedM2DAFloat" => Value::Float(cell_float(c)),
                "GetHashedM2DAString" => Value::String(cell_string(c)),
                _ => Value::Resource(cell_string(c)),
            })
        }
        // DragonAge2.exe 0x4e1700/0x4e17a0 -> 0x6a1300/0x6a1b50:
        // row 0 marks which columns accept additions from the four learned upgrades.
        // Read the live ability set, so grants/removals and reload need no cached totals.
        "GetUpgradableAbilityHashedM2DAInt" | "GetUpgradableAbilityHashedM2DAFloat" => {
            let Some(creature) = h
                .object(obj(a, 0))
                .filter(|o| o.kind == ObjectKind::Creature)
            else {
                return ok(if name.ends_with("Float") {
                    Value::Float(0.0)
                } else {
                    Value::Int(0)
                });
            };
            let table = h.sim.world.tables.get(1, "");
            let column = int(a, 1) as u32;
            let ability = i64::from(int(a, 2));
            let cell = |row| table.as_ref().and_then(|t| t.cell_by_hash(column, row));
            let mut rows = vec![ability];
            let upgradeable = if name.ends_with("Float") {
                cell_float(cell(0)) != 0.0
            } else {
                cell_int(cell(0)) != 0
            };
            if upgradeable && cell(ability).is_some() {
                for upgrade in [0xd300_f4d9, 0xf82d_a71a, 0xe136_965b, 0xae77_009c] {
                    let id = cell_int(
                        table
                            .as_ref()
                            .and_then(|t| t.cell_by_hash(upgrade, ability)),
                    );
                    if creature.abilities.contains(&id) {
                        rows.push(i64::from(id));
                    }
                }
            }
            ok(if name.ends_with("Float") {
                Value::Float(
                    rows.into_iter()
                        .fold(0.0, |n, row| n + cell_float(cell(row))),
                )
            } else {
                Value::Int(
                    rows.into_iter()
                        .fold(0i32, |n, row| n.wrapping_add(cell_int(cell(row)))),
                )
            })
        }
        "GetM2DARows" => ok(Value::Int(
            h.sim
                .world
                .tables
                .get(int(a, 0), &string(a, 1))
                .map_or(0, |t| t.rows.len() as i32),
        )),
        "GetM2DAColumns" => ok(Value::Int(
            h.sim
                .world
                .tables
                .get(int(a, 0), "")
                .map_or(0, |t| t.columns.len() as i32),
        )),
        "GetM2DARowIdFromRowIndex" => ok(Value::Int(
            h.sim
                .world
                .tables
                .get(int(a, 0), &string(a, 2))
                .and_then(|t| t.order.get(int(a, 1).max(0) as usize).copied())
                .map_or(-1, |v| v as i32),
        )),

        // --- Creature properties ----------------------------------------------------------
        // Creature properties as DAOrigins.exe keeps them (see `Sim::property_total`): a base
        // for every kind, a modifier for attributes and depletables, a current value for
        // depletables only. By value type (0x66cf60, setters 0x669890, adders 0x6699c0):
        // - TOTAL: the base, derivation and modifiers (simple ones: base and derivation),
        //   within properties.gda's bounds; BASE: the base as set; MODIFIER: the modifiers
        //   (0 for a simple property); CURRENT: a depletable's current value, -1 otherwise
        //   (0xae2e00).
        // - Setting TOTAL or BASE sets the base; CURRENT sets a depletable's current value
        //   (within its minimum and total; nothing for other kinds); MODIFIER sets the
        //   modifiers (nothing for a simple property). A depletable's current value is kept
        //   within its total when the base or modifiers change.
        // - UpdateCreatureProperty adds to the current value or the modifiers in the same
        //   way; for TOTAL and BASE it does nothing.
        "GetCreatureProperty" => {
            let (id, p, vt) = (obj(a, 0), int(a, 1), int(a, 2));
            let kind = property_kind(h.sim, p);
            let v = if vt == constant(&h.sim.table, "PROPERTY_VALUE_BASE") {
                h.object(id)
                    .and_then(|o| o.properties.get(&p))
                    .map_or(0.0, |v| v.0)
            } else if vt == constant(&h.sim.table, "PROPERTY_VALUE_MODIFIER") {
                if kind == PropertyKind::Simple {
                    0.0
                } else {
                    h.sim.property_modifier(id, p)
                }
            } else if vt == constant(&h.sim.table, "PROPERTY_VALUE_CURRENT") {
                if kind == PropertyKind::Depletable {
                    h.object(id)
                        .and_then(|o| o.properties.get(&p))
                        .map_or(0.0, |v| v.1)
                } else {
                    -1.0
                }
            } else {
                h.sim.property_total(id, p)
            };
            ok(Value::Float(v))
        }
        // Mana (or stamina): DAOrigins.exe reads and writes the creature's depletable
        // property 8 (PROPERTY_DEPLETABLE_MANA_STAMINA) directly: GetCreatureMana its current
        // value (0x66cc30), GetCreatureMaxMana its total (0x66cdc0), both truncated to
        // whole numbers and -1 for what is not a creature; SetCreatureMana its current value
        // (0x66cd00 -> 0x79c8d0). That setter clamps between the property's current floor
        // and computed total. Mana's floor stays at its initialized zero; the immortality
        // setter changes health's floor only (0x6ad130). See notes/mana-property-floor.md.
        "GetCreatureMana" | "GetCreatureMaxMana" => {
            let (id, p) = (obj(a, 0), PROPERTY_MANA_STAMINA);
            let creature = h.object(id).is_some_and(|o| o.kind == ObjectKind::Creature);
            ok(Value::Int(if !creature {
                -1
            } else if name == "GetCreatureMana" {
                h.object(id)
                    .and_then(|o| o.properties.get(&p))
                    .map_or(0.0, |v| v.1) as i32
            } else {
                h.sim.property_total(id, p) as i32
            }))
        }
        "SetCreatureMana" => {
            let (id, p) = (obj(a, 0), PROPERTY_MANA_STAMINA);
            if h.object(id).is_some_and(|o| o.kind == ObjectKind::Creature) {
                let max = h.sim.property_total(id, p);
                if let Some(o) = h.sim.world.get_mut(id) {
                    o.properties.entry(p).or_insert((0.0, 0.0)).1 =
                        (int(a, 1) as f32).clamp(0.0, max.max(0.0));
                }
            }
            void()
        }
        "SetCreatureProperty" | "UpdateCreatureProperty" => {
            let (id, p, value, vt) = (obj(a, 0), int(a, 1), float(a, 2), int(a, 3));
            let update = name == "UpdateCreatureProperty";
            let by = h.script.clone();
            h.sim
                .set_creature_property(caller, by, id, p, value, vt, update);
            void()
        }

        // --- Plots ------------------------------------------------------------------------
        // The GUID of a plot named by resource or GUID; blank when the plot is unknown.
        "GetPlotGUID" => {
            let p = h.sim.plot_info(&string(a, 0));
            ok(Value::String(p.map(|p| p.guid.clone()).unwrap_or_default()))
        }
        "GetPlotResRef" => {
            let p = h.sim.plot_info(&string(a, 0));
            ok(Value::String(
                p.map(|p| p.resref.clone()).unwrap_or_else(|| string(a, 0)),
            ))
        }
        "GetPlotFlagName" => {
            let p = h.sim.plot_info(&string(a, 0));
            let flag = int(a, 1);
            ok(Value::String(
                p.and_then(|p| p.flags.get(&flag).cloned())
                    .unwrap_or_else(|| flag.to_string()),
            ))
        }
        "GetPlotEntryName" => {
            let p = h.sim.plot_info(&string(a, 0));
            ok(Value::Int(
                p.and_then(|p| p.name_strref).map_or(0, |s| s as i32),
            ))
        }
        "GetPartyPlotFlag" => {
            let v = h
                .sim
                .get_plot_flag(&string(a, 1), int(a, 2), int(a, 3) != 0, caller);
            ok(b(v))
        }
        "SetPartyPlotFlag" => {
            h.sim.set_plot_flag(
                &string(a, 1),
                int(a, 2),
                int(a, 3) != 0,
                int(a, 4) != 0,
                caller,
            );
            void()
        }
        "GetPlotFlagRewardId" => {
            let p = h.sim.plot_info(&string(a, 0));
            ok(Value::Int(
                p.and_then(|p| p.rewards.get(&int(a, 1)).copied())
                    .unwrap_or(0),
            ))
        }

        // --- Game state -------------------------------------------------------------------
        "GetGameMode" => ok(Value::Int(h.sim.world.game_mode)),
        "SetGameMode" => {
            // While a conversation holds the game, other modes wait for it to end
            // (DAOrigins.exe's mode controller keeps them pending, 0x640c60).
            if h.sim.world.mode_before_talk.is_some() && int(a, 0) != h.sim.world.game_mode {
                h.sim.world.mode_after_talk = Some(int(a, 0));
                return void();
            }
            let old = h.sim.world.game_mode;
            h.sim.world.game_mode = int(a, 0);
            if let Some(m) = h.sim.world.module {
                let mut ev = h.sim.new_event("GAMEMODE_CHANGE", Some(m));
                ev.ints = vec![int(a, 0), old];
                h.sim.world.queue_event(0, m, ev, None);
            }
            void()
        }
        "GetGameDifficulty" => ok(Value::Int(h.sim.world.difficulty)),
        // The headless simulation has no graphics setting; scripts only pick visual effects
        // by it. Provisional: medium (1).
        "GetGraphicsDetailLevel" => ok(Value::Int(1)),
        // The one active ability an item grants on the quickbar: its template's AbilityID.
        "GetItemAbilityId" => ok(Value::Int(
            h.object(obj(a, 0))
                .and_then(|o| o.fields.get("AbilityID").copied())
                .unwrap_or_else(|| constant(&h.sim.table, "ABILITY_INVALID")),
        )),
        // Casting: an ability in hand whose cast has started (it has a time to land).
        // Provisional: the simulation's cast runs from ABILITY_CAST_START to its impact.
        "IsConjuring" => ok(b(h.object(obj(a, 0)).is_some_and(|o| {
            o.commands.first().is_some_and(|c| {
                c.name == "CommandUseAbility"
                    && c.due.is_some()
                    && c.approach.is_none()
                    && c.from.is_none()
            })
        }))),
        "GetMaxLevel" => ok(Value::Int(20)),
        "GetTime" => ok(Value::Int((h.sim.world.time_ms / 1000) as i32)),
        "ReadIniEntry" => ok(Value::String(String::new())),
        // Achievements belong to the player's profile, not the game: the simulation's profile has
        // none (by id or, the older form, by name).
        "GetHasAchievementByID" | "GetHasAchievement" => ok(Value::Int(0)),
        // Traced (COMMAND_GETPRCENABLED, handler 0x66d6d0): the add-in record whose UID matches
        // gives its Enabled byte (+0x15d), cleared when it needs authorization (+0x15e) and has
        // none (+0x15f) or when an unidentified pair (+0x175 set, +0x176 clear) says so; no
        // such record gives 0. Ours: whether the stack mounts that add-in, which applies the
        // registry's own gates and not the authorization (FMT-011).
        "GetPRCEnabled" => ok(b(h.sim.vfs.addin_enabled(&string(a, 0)))),
        "IsPackageLoaded" => {
            // Inferred (#862): whether a package's archives are in our resource stack.
            let pkg = format!("packages/{}/", string(a, 0).to_ascii_lowercase());
            let addin = format!("addins/{}/", string(a, 0).to_ascii_lowercase());
            ok(b(h.sim.vfs.layers().iter().any(|l| {
                let lab = l.label.to_ascii_lowercase();
                lab.starts_with(&pkg) || lab.starts_with(&addin)
            })))
        }
        "GetCurrentScriptName" => ok(Value::String(h.script.clone())),
        "GetStringByStringId" => ok(Value::String(
            h.sim.string(int(a, 0) as u32).unwrap_or_default(),
        )),

        // --- Money, abilities, items --------------------------------------------------------
        "AddCreatureMoney" => {
            if let Some(o) = h.sim.world.get_mut(obj(a, 1)) {
                o.money = o.money.saturating_add(int(a, 0));
            }
            void()
        }
        "GetCreatureMoney" => ok(Value::Int(h.object(obj(a, 0)).map_or(0, |o| o.money))),
        // DAOrigins.exe's shared handler (0x669ab0, commands 252-254) looks the ability up in
        // the Abilities M2DA (column 0xe1a8f4ea, 0x669b4c) and does nothing for an id of 0 or
        // below or one with no row; HasAbility answers false for it.
        "AddAbility" => {
            let (who, id) = (obj(a, 0), int(a, 1));
            if h.sim.ability_known(id)
                && let Some(o) = h.sim.world.get_mut(who)
            {
                o.abilities.insert(id);
                // With bSendNotification, a party member's new ability shows as a large
                // alert with its name, its icon and who got it (DAOrigins.exe 0x669bd0:
                // the flag, then the party test 0x6316f0, then the builder 0x6291c0).
                // Provisional: the exe skips abilities of type 4 before adding them at all,
                // which is not modelled here.
                if a.len() > 2 && int(a, 2) != 0 && h.sim.in_party(who) {
                    let cell = |h: &ScriptHost, col: &str| {
                        h.sim
                            .world
                            .tables()
                            .get(0, "ABI_base")
                            .and_then(|t| t.cell(col, id as i64).cloned())
                    };
                    let name = match cell(h, "NameStrRef") {
                        Some(Cell::Int(n)) if n > 0 => h.sim.string(n as u32),
                        _ => None,
                    };
                    let icon = match cell(h, "Icon") {
                        Some(Cell::String(s)) => Some(clean(&s).to_string()),
                        _ => None,
                    };
                    h.sim
                        .raise_large_alert(crate::journal::ALERT_ABILITY, name, icon, Some(who));
                }
            }
            void()
        }
        // DAOrigins.exe (0x663b80 -> 0x628e10): a large alert with the specialization's
        // name (`NameStrRef`, column 0x3273f70d) and icon (`Icon`, 0xbcddb871) from its
        // row of the classes table (`CLA_base`, as script.ldf says of nSpecID).
        "ShowSpecUnlockedNotification" => {
            let spec = int(a, 0) as i64;
            let tables = h.sim.world.tables();
            let row = tables.get(0, "CLA_base");
            let name = match row
                .as_ref()
                .and_then(|t| t.cell("NameStrRef", spec).cloned())
            {
                Some(Cell::Int(n)) if n > 0 => h.sim.string(n as u32),
                _ => None,
            };
            let icon = match row.as_ref().and_then(|t| t.cell("Icon", spec).cloned()) {
                Some(Cell::String(s)) => Some(clean(&s).to_string()),
                _ => None,
            };
            h.sim
                .raise_large_alert(crate::journal::ALERT_SPECIALIZATION, name, icon, None);
            void()
        }
        "RemoveAbility" => {
            let id = int(a, 1);
            if h.sim.ability_known(id)
                && let Some(o) = h.sim.world.get_mut(obj(a, 0))
            {
                o.abilities.remove(&id);
            }
            void()
        }
        "HasAbility" => {
            let id = int(a, 1);
            ok(b(h.sim.ability_known(id)
                && h.object(obj(a, 0))
                    .is_some_and(|o| o.abilities.contains(&id))))
        }
        "IsModalAbilityActive" => ok(b(h.sim.modal_active(obj(a, 0), int(a, 1)))),
        "GetConditionedAbilities" => ok(Value::Array {
            elem: Type::Int,
            items: h
                .sim
                .conditioned_abilities(obj(a, 0), int(a, 1))
                .into_iter()
                .map(Value::Int)
                .collect(),
        }),
        // DAOrigins.exe 0x66c1f0: the same check, for one ability (no script calls it).
        "CanUseConditionedAbility" => ok(b(h.sim.ability_conditions_met(
            obj(a, 0),
            int(a, 1),
            a.get(2).map_or(-1, |_| int(a, 2)),
        ))),
        "GetItemInEquipSlot" => {
            let (slot, who) = (int(a, 0), obj(a, 1));
            let set = a.get(2).map_or(-1, |_| int(a, 2));
            ok(Value::Object(
                h.object(who)
                    .and_then(|o| {
                        let active = o.fields.get("ActiveWeaponSet").copied().unwrap_or(0);
                        if (set == 0 || set == 1)
                            && set != active
                            && crate::world::WEAPON_SET_SLOTS.contains(&slot)
                        {
                            o.other_set.get(&slot).copied()
                        } else {
                            o.equipment.get(&slot).copied()
                        }
                    })
                    .unwrap_or(OBJECT_INVALID),
            ))
        }
        // An unspecified slot (INVENTORY_SLOT_INVALID) means the base type's first free
        // slot (bitm_base EquippableSlots). Weapon sets are not modelled (provisional).
        "EquipItem" => {
            let (who, item, slot) = (obj(a, 0), obj(a, 1), int(a, 2));
            if slot == constant(&h.sim.table, "INVENTORY_SLOT_INVALID") {
                // The base type's first free slot.
                let done = h.object(item).is_some() && h.sim.equip_default(who, item).is_some();
                return ok(b(done));
            }
            let set = a.get(3).map_or(-1, |_| int(a, 3));
            let ok_ = h.object(item).is_some();
            if ok_ && h.sim.world.get(who).is_some() {
                // Off wherever it was, and what the slot held comes off (the engine's side
                // of both: see `Sim::take_off`).
                h.sim.displace(who, item);
                let (active, in_hand, other) = h
                    .sim
                    .world
                    .get(who)
                    .map(|o| {
                        (
                            o.fields.get("ActiveWeaponSet").copied().unwrap_or(0),
                            o.equipment.get(&slot).copied(),
                            o.other_set.get(&slot).copied(),
                        )
                    })
                    .unwrap_or((0, None, None));
                // A weapon for the set not in hand goes to the inactive set.
                let inactive = (set == 0 || set == 1)
                    && set != active
                    && crate::world::WEAPON_SET_SLOTS.contains(&slot);
                let held = if inactive { other } else { in_hand };
                if let Some(old) = held {
                    h.sim.displace(who, old);
                }
                if let Some(o) = h.sim.world.get_mut(who) {
                    if !o.inventory.contains(&item) {
                        o.inventory.push(item);
                    }
                    if inactive {
                        o.other_set.insert(slot, item);
                    } else {
                        o.equipment.insert(slot, item);
                    }
                }
                // Only what is in hand takes effect. Provisional.
                if !inactive {
                    h.sim.wear(who, item, slot);
                }
            }
            ok(b(ok_))
        }
        "UnequipItem" => {
            let found = h.sim.unequip(obj(a, 0), obj(a, 1));
            ok(b(found))
        }
        "RemoveItemsByTag" => {
            let (who, tag, n) = (obj(a, 0), string(a, 1), int(a, 2));
            let items: Vec<ObjectId> = h
                .object(who)
                .map(|o| o.inventory.clone())
                .unwrap_or_default()
                .into_iter()
                .filter(|i| {
                    h.object(*i)
                        .is_some_and(|x| x.tag.eq_ignore_ascii_case(&tag))
                })
                .take(if n < 0 { usize::MAX } else { n as usize })
                .collect();
            // A worn item is taken off first (its effects end, the wearer hears UNEQUIP),
            // as DestroyObject does.
            for &i in &items {
                h.sim.unequip(who, i);
            }
            if let Some(o) = h.sim.world.get_mut(who) {
                o.inventory.retain(|i| !items.contains(i));
            }
            for i in items {
                h.sim.world.objects.remove(&i);
            }
            void()
        }
        "SetGroupHostility" => {
            let (x, y, hostile) = (int(a, 0), int(a, 1), int(a, 2) != 0);
            h.sim.world.hostility.insert((x.min(y), x.max(y)), hostile);
            void()
        }
        "GetGroupHostility" => {
            let (x, y) = (int(a, 0), int(a, 1));
            ok(b(h
                .sim
                .world
                .hostility
                .get(&(x.min(y), x.max(y)))
                .copied()
                .unwrap_or(false)))
        }
        "CreateItemOnObject" => {
            // (resource, target, stack size, tag, suppress notification, droppable)
            // DAOrigins.exe splits the count before adding (0x7595a0): full stacks and a
            // remainder of a stackable item, or that many separate items of one that does not
            // stack; each is then added (and may merge into a stack already held). The first
            // created item is returned.
            let template = string(a, 0);
            let stack = if a.len() > 2 { int(a, 2).max(1) } else { 1 };
            let droppable = a.len() <= 5 || int(a, 5) != 0;
            let tag = string(a, 3);
            let to = obj(a, 1);
            let mut first = None;
            let mut left = stack;
            while left > 0 {
                let id = h.sim.create_item(&template);
                let max = h.sim.max_stack(id);
                let count = if max > 1 { left.min(max) } else { 1 };
                if let Some(o) = h.sim.world.get_mut(id) {
                    if !tag.is_empty() {
                        o.tag = tag.clone();
                    }
                    if max > 1 {
                        o.fields.insert("StackSize".into(), count);
                    }
                    o.fields.insert("Droppable".into(), droppable as i32);
                }
                h.sim.give_item(id, to);
                first.get_or_insert(id);
                left -= count;
            }
            ok(Value::Object(first.unwrap_or(OBJECT_INVALID)))
        }

        // --- Effects ----------------------------------------------------------------------
        "Effect" => {
            let hd = h.sim.world.store(EngineData::Effect(Effect {
                ty: int(a, 0),
                creator: Some(caller),
                ..Default::default()
            }));
            ok(Value::Engine {
                ty: ty.effect,
                handle: Some(hd),
            })
        }
        "GetEffectType" | "GetEffectAbilityID" | "GetEffectInteger" | "GetEffectFloat"
        | "GetEffectObject" | "GetEffectCreator" => {
            let e = match h.sim.world.handle(handle(a, 0)) {
                Some(EngineData::Effect(e)) => e.clone(),
                _ => Effect::default(),
            };
            let i = int(a, 1).max(0) as usize;
            ok(match name {
                "GetEffectType" => Value::Int(e.ty),
                "GetEffectAbilityID" => Value::Int(e.ability),
                "GetEffectInteger" => Value::Int(e.ints.get(i).copied().unwrap_or(0)),
                "GetEffectFloat" => Value::Float(e.floats.get(i).copied().unwrap_or(0.0)),
                "GetEffectCreator" => Value::Object(e.creator.unwrap_or(OBJECT_INVALID)),
                _ => Value::Object(e.objects.get(i).copied().unwrap_or(OBJECT_INVALID)),
            })
        }
        // Engine data integers are write-only for scripts: kept, and the effect returned
        // (scripts assign the result, so answering nothing made the effect invalid).
        "SetEffectEngineInteger" => {
            let mut e = match h.sim.world.handle(handle(a, 0)) {
                Some(EngineData::Effect(e)) => e.clone(),
                _ => Effect::default(),
            };
            let (i, v) = (int(a, 1), int(a, 2));
            e.engine_ints.retain(|x| x.0 != i);
            e.engine_ints.push((i, v));
            let hd = h.sim.world.store(EngineData::Effect(e));
            ok(Value::Engine {
                ty: ty.effect,
                handle: Some(hd),
            })
        }
        "SetEffectEngineVector" => {
            let mut e = match h.sim.world.handle(handle(a, 0)) {
                Some(EngineData::Effect(e)) => e.clone(),
                _ => Effect::default(),
            };
            let i = int(a, 1);
            e.engine_vectors.retain(|x| x.0 != i);
            e.engine_vectors.push((i, vector(a, 2)));
            let hd = h.sim.world.store(EngineData::Effect(e));
            ok(Value::Engine {
                ty: ty.effect,
                handle: Some(hd),
            })
        }
        // DAOrigins.exe 0x65c360 (functions 609 and 611, one handler for the effect
        // setters): the popped effect gets the flags (+0x3c) or the animation (+0x34) and is
        // pushed back.
        "SetEffectAnimation" | "SetEffectFlags" => {
            let mut e = match h.sim.world.handle(handle(a, 0)) {
                Some(EngineData::Effect(e)) => e.clone(),
                _ => Effect::default(),
            };
            if name == "SetEffectAnimation" {
                e.animation = int(a, 1);
            } else {
                e.flags = int(a, 1);
            }
            let hd = h.sim.world.store(EngineData::Effect(e));
            ok(Value::Engine {
                ty: ty.effect,
                handle: Some(hd),
            })
        }
        "SetEffectInteger"
        | "SetEffectFloat"
        | "SetEffectObject"
        | "SetEffectCreator"
        | "SetEffectAbilityID"
        | "SetEffectEngineFloat" => {
            let mut e = match h.sim.world.handle(handle(a, 0)) {
                Some(EngineData::Effect(e)) => e.clone(),
                _ => Effect::default(),
            };
            let i = int(a, 1).max(0) as usize;
            // Engine floats are kept apart from script floats (provisional layout).
            let (name, i) = if name == "SetEffectEngineFloat" {
                ("SetEffectFloat", i + crate::sim::ENGINE_FLOAT_BASE)
            } else {
                (name, i)
            };
            match name {
                "SetEffectInteger" if i < 64 => {
                    if e.ints.len() <= i {
                        e.ints.resize(i + 1, 0);
                    }
                    e.ints[i] = int(a, 2);
                }
                "SetEffectFloat" if i < 64 => {
                    if e.floats.len() <= i {
                        e.floats.resize(i + 1, 0.0);
                    }
                    e.floats[i] = float(a, 2);
                }
                "SetEffectObject" if i < 64 => {
                    if e.objects.len() <= i {
                        e.objects.resize(i + 1, OBJECT_INVALID);
                    }
                    e.objects[i] = obj(a, 2);
                }
                "SetEffectCreator" => e.creator = Some(obj(a, 1)),
                "SetEffectAbilityID" => e.ability = int(a, 1),
                _ => {}
            }
            let hd = h.sim.world.store(EngineData::Effect(e));
            ok(Value::Engine {
                ty: ty.effect,
                handle: Some(hd),
            })
        }
        // DAOrigins.exe (0x65d300, function 635): makes an effect of type 4 (as
        // EffectVisualEffect does) with the VFX as its integer 0, gives it the duration type,
        // the duration (seconds x 1000), the creator and the ability, and applies it to the
        // target through the same call as Engine_ApplyEffectOnObject (0x7a1a20).
        "ApplyEffectVisualEffect" => {
            let e = Effect {
                ty: 4,
                ints: vec![int(a, 2)],
                ..Default::default()
            };
            let hd = h.sim.world.store(EngineData::Effect(e));
            let ability = (a.len() > 5).then(|| int(a, 5));
            if let Some(e) = applied_effect(h, Some(hd), int(a, 3), float(a, 4), obj(a, 0), ability)
            {
                apply_visual_or_effect(h, obj(a, 1), e);
            }
            void()
        }
        "Engine_ApplyEffectOnObject" | "ApplyEffectOnObject" => {
            let ability = (a.len() > 5).then(|| int(a, 5));
            if let Some(mut e) =
                applied_effect(h, handle(a, 1), int(a, 0), float(a, 3), obj(a, 4), ability)
            {
                let target = obj(a, 2);
                if e.ty == 4 {
                    apply_visual_or_effect(h, target, e);
                    return void();
                }
                // An area of effect on an object goes with it (see [`crate::aoe`]); the
                // effect names it (object 0, as 0x7a0230 sets it) and takes it away when it
                // goes.
                let instant = constant(&h.sim.table, "EFFECT_DURATION_TYPE_INSTANT");
                let at = h
                    .object(target)
                    .and_then(|o| Some((o.area?, o.position, o.facing)));
                if let (true, Some((area, p, f))) = (
                    e.ty == crate::aoe::EFFECT_TYPE_AOE && e.duration_type != instant,
                    at,
                ) {
                    let id = h.sim.create_aoe(&e, area, p, f, Some(target), None);
                    e.objects = vec![id];
                }
                h.sim.apply_effect(target, e);
            }
            void()
        }
        // DAOrigins.exe (0x672200) gives the effect its duration, creator and ability as
        // Engine_ApplyEffectOnObject does, then applies a copy to every member of the party
        // list (the one GetPartyList returns, 0x672ba0), skipping the creator when
        // bExcludeCreator is set.
        "Engine_ApplyEffectOnParty" => {
            let creator = obj(a, 3);
            let exclude = int(a, 5) != 0;
            let ability = (a.len() > 4).then(|| int(a, 4));
            if let Some(e) =
                applied_effect(h, handle(a, 1), int(a, 0), float(a, 2), creator, ability)
            {
                for m in party_of(h.sim) {
                    if !(exclude && m == creator) {
                        h.sim.apply_effect(m, e.clone());
                    }
                }
            }
            void()
        }
        "GetCurrentEffect" => {
            let e = h
                .sim
                .current_effect
                .as_ref()
                .map(|x| x.0.clone())
                .unwrap_or_default();
            let hd = h.sim.world.store(EngineData::Effect(e));
            ok(Value::Engine {
                ty: ty.effect,
                handle: Some(hd),
            })
        }
        "SetIsCurrentEffectValid" => {
            if let Some(c) = h.sim.current_effect.as_mut() {
                c.1 = int(a, 0) != 0;
            }
            void()
        }
        "GetHasEffects" => {
            let f = EffectFilter::new(int(a, 1), int(a, 2), OBJECT_INVALID, 0);
            ok(b(h
                .object(obj(a, 0))
                .is_some_and(|o| o.effects.iter().any(|e| f.matches(e)))))
        }
        "GetEffects" => {
            let f = EffectFilter::new(int(a, 1), int(a, 2), obj(a, 3), int(a, 4));
            let target = obj(a, 0);
            let mut list: Vec<Effect> = h
                .object(target)
                .map(|o| o.effects.iter().filter(|e| f.matches(e)).cloned().collect())
                .unwrap_or_default();
            // An area's effects include the visual effects applied at its places.
            if h.object(target).is_some_and(|o| o.kind == ObjectKind::Area) {
                list.extend(
                    h.sim
                        .world
                        .location_vfx
                        .iter()
                        .filter(|v| v.area == target && f.matches(&v.effect))
                        .map(|v| v.effect.clone()),
                );
            }
            let items = list
                .into_iter()
                .map(|e| Value::Engine {
                    ty: ty.effect,
                    handle: Some(h.sim.world.store(EngineData::Effect(e))),
                })
                .collect();
            ok(Value::Array {
                elem: Type::Engine(ty.effect),
                items,
            })
        }
        "RemoveEffect" => {
            if let Some(EngineData::Effect(e)) = h.sim.world.handle(handle(a, 1)).cloned() {
                let target = obj(a, 0);
                h.sim
                    .world
                    .location_vfx
                    .retain(|v| !(v.area == target && v.effect.id == e.id && e.id != 0));
                h.sim.remove_effect(target, &e);
            }
            void()
        }
        // DAOrigins.exe (0x673ab0) removes every effect but those of duration types 4, 5
        // and 6 (EFFECT_DURATION_TYPE_DEATH; 0x7a0c90); with bIgnoreInjuries (default TRUE)
        // also keeps injuries, abilities 500001 to 500099 (0x651850), and with bDeath keeps
        // effects whose type's effects.gda `IgnoreDeath` is set (0x651830).
        // A lasting visual effect on an item (a weapon's glow): DAOrigins.exe (0x65d4a0)
        // makes an effect of type 4 (EffectVisualEffect), permanent, with the VFX id as its
        // integer 0, and gives it to the item. It stays on the item for presentation.
        // DAOrigins.exe (0x665510) sets the item's indestructible flag (0x752c30); kept
        // in the item's fields.
        "SetItemIndestructible" => {
            if let Some(o) = h
                .sim
                .world
                .get_mut(obj(a, 0))
                .filter(|o| o.kind == ObjectKind::Item)
            {
                o.fields
                    .insert("Indestructible".into(), (int(a, 1) != 0) as i32);
            }
            void()
        }
        "AddItemVisualEffect" => {
            let item = obj(a, 0);
            let permanent = constant(&h.sim.table, "EFFECT_DURATION_TYPE_PERMANENT");
            if let Some(o) = h
                .sim
                .world
                .get_mut(item)
                .filter(|o| o.kind == ObjectKind::Item)
            {
                o.effects.push(Effect {
                    ty: 4,
                    duration_type: permanent,
                    ints: vec![int(a, 1)],
                    ..Default::default()
                });
            }
            void()
        }
        "RemoveAllEffects" => {
            let (who, keep_injuries) = (obj(a, 0), a.len() < 2 || int(a, 1) != 0);
            let death = int(a, 2) != 0;
            let table = h.sim.world.tables.get(0, "effects");
            let gone: Vec<Effect> = h
                .object(who)
                .map(|o| {
                    o.effects
                        .iter()
                        .filter(|e| !(4..=6).contains(&e.duration_type))
                        .filter(|e| !(keep_injuries && e.ability > 500_000 && e.ability < 500_100))
                        .filter(|e| {
                            !death
                                || table.as_ref().is_none_or(|t| {
                                    cell_int(t.cell("IgnoreDeath", e.ty as i64)) == 0
                                })
                        })
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
            for e in gone {
                h.sim.remove_effect(who, &e);
            }
            void()
        }
        "RemoveEffectsByParameters" => {
            let who = obj(a, 0);
            let f = EffectFilter::new(int(a, 1), int(a, 2), obj(a, 3), 0);
            let gone: Vec<Effect> = h
                .object(who)
                .map(|o| o.effects.iter().filter(|e| f.matches(e)).cloned().collect())
                .unwrap_or_default();
            for e in gone {
                h.sim.remove_effect(who, &e);
            }
            void()
        }
        // The flags (EFFECT_FLAG_DISABLE_MOVEMENT, _COMBAT, _TALENTS, ...) of the effects a
        // creature holds: each effect type's `Flags` in effects.gda, combined.
        "GetEffectsFlags" => {
            let types: Vec<i32> = h
                .object(obj(a, 0))
                .map(|o| o.effects.iter().map(|e| e.ty).collect())
                .unwrap_or_default();
            let table = h.sim.world.tables.get(0, "effects");
            let flags = types.iter().fold(0, |acc, &ty| {
                acc | table
                    .as_ref()
                    .map_or(0, |t| cell_int(t.cell("Flags", ty as i64)))
            });
            ok(Value::Int(flags))
        }

        // --- Commands ---------------------------------------------------------------------
        n if n.starts_with("Command") && f.ret == Type::Engine(ty.command) => {
            let mut c = Command {
                ty: command_type(&h.sim.table, n, h.sim.game),
                name: n.to_string(),
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
            };
            // Only the first float is kept (a wait's seconds, a move's minimum range): a
            // later one (CommandMoveToObject's fMaxRange) used to land there when the first
            // was 0.
            let mut first_float = true;
            for (p, v) in f.params.iter().zip(a) {
                match (&p.ty, v) {
                    (Type::Object, Value::Object(o)) if c.target.is_none() => c.target = Some(*o),
                    (Type::Float, Value::Float(x)) if first_float => {
                        first_float = false;
                        c.seconds = *x;
                    }
                    (Type::Int, Value::Int(x)) => c.ints.push(*x),
                    (Type::String | Type::Resource, Value::String(s) | Value::Resource(s)) => {
                        c.strings.push(s.clone())
                    }
                    (Type::Engine(_), Value::Engine { handle, .. }) => {
                        if let Some(EngineData::Location(l)) = h.sim.world.handle(*handle) {
                            c.location = Some(*l);
                        }
                    }
                    // CommandMoveToMultiLocations' list: kept as the walk's points, the last
                    // one as where it ends.
                    (Type::Array(_), Value::Array { items, .. }) => {
                        for v in items {
                            if let Value::Engine { handle, .. } = v
                                && let Some(EngineData::Location(l)) = h.sim.world.handle(*handle)
                            {
                                c.path.push(l.position);
                                c.location = Some(*l);
                            }
                        }
                    }
                    // A place in the caller's area (CommandUseAbility's vTarget); the
                    // default [0, 0, 0] is none. Provisional.
                    (Type::Vector, Value::Vector(v)) if *v != [0.0; 3] && c.location.is_none() => {
                        if let Some(area) = h.object(caller).and_then(|o| o.area) {
                            c.location = Some(crate::world::Location {
                                area,
                                position: *v,
                                facing: 0.0,
                            });
                        }
                    }
                    _ => {}
                }
            }
            // The list's walk starts at nStartingWP (integer 1).
            if n == "CommandMoveToMultiLocations" {
                let start = c.ints.get(1).copied().unwrap_or(0).max(0) as usize;
                if start < c.path.len() {
                    c.path.drain(..start);
                }
            }
            let hd = h.sim.world.store(EngineData::Command(c));
            ok(Value::Engine {
                ty: ty.command,
                handle: Some(hd),
            })
        }
        "SetCommandFloat" => {
            // Provisional: float 0 is the command's time (`seconds`); others are dropped.
            let mut c = match h.sim.world.handle(handle(a, 0)) {
                Some(EngineData::Command(c)) => c.clone(),
                _ => {
                    return ok(Value::Engine {
                        ty: ty.command,
                        handle: None,
                    });
                }
            };
            if int(a, 2) == 0 {
                c.seconds = float(a, 1);
            }
            let hd = h.sim.world.store(EngineData::Command(c));
            ok(Value::Engine {
                ty: ty.command,
                handle: Some(hd),
            })
        }
        "IsLocationValid" => ok(b(matches!(
            h.sim.world.handle(handle(a, 0)),
            Some(EngineData::Location(l)) if h.sim.world.get(l.area).is_some()
        ))),
        "GetCommandType" => ok(Value::Int(match h.sim.world.handle(handle(a, 0)) {
            Some(EngineData::Command(c)) => c.ty,
            _ => 0,
        })),
        // A command's arguments, as its constructor took them: integers in order, float 0
        // (a wait's seconds and the like), object 0 its target. Provisional.
        "GetCommandInt" | "GetCommandFloat" | "GetCommandObject" => {
            let c = match h.sim.world.handle(handle(a, 0)) {
                Some(EngineData::Command(c)) => Some(c.clone()),
                _ => None,
            };
            let i = int(a, 1);
            ok(match name {
                "GetCommandInt" => Value::Int(
                    c.as_ref()
                        .and_then(|c| c.ints.get(usize::try_from(i).ok()?).copied())
                        .unwrap_or(0),
                ),
                "GetCommandFloat" => {
                    Value::Float(c.as_ref().filter(|_| i == 0).map_or(0.0, |c| c.seconds))
                }
                _ => Value::Object(
                    c.as_ref()
                        .filter(|_| i == 0)
                        .and_then(|c| c.target)
                        .unwrap_or(OBJECT_INVALID),
                ),
            })
        }
        "AddCommand" => {
            let Some(EngineData::Command(c)) = h.sim.world.handle(handle(a, 1)).cloned() else {
                return ok(Value::Int(0));
            };
            let target = obj(a, 0);
            let front = int(a, 2) != 0;
            h.sim.world.trace.push(TraceEntry::Command {
                t,
                object: target,
                command: c.name.clone(),
                status: "queued".into(),
            });
            match h.sim.world.get_mut(target) {
                Some(o) => {
                    if front {
                        o.commands.insert(0, c);
                    } else {
                        o.commands.push(c);
                    }
                    ok(Value::Int(1))
                }
                None => ok(Value::Int(0)),
            }
        }
        "GetPreviousCommand" => {
            let c = h
                .object(obj(a, 0))
                .and_then(|o| o.combat.previous_command.clone());
            let hd = c.map(|c| h.sim.world.store(EngineData::Command(c)));
            ok(Value::Engine {
                ty: ty.command,
                handle: hd,
            })
        }
        "GetCurrentCommand" => {
            let c = h
                .object(obj(a, 0))
                .and_then(|o| o.commands.first().cloned());
            let hd = c.map(|c| h.sim.world.store(EngineData::Command(c)));
            ok(Value::Engine {
                ty: ty.command,
                handle: hd,
            })
        }
        // Where to aim an area ability for the most foes: see [`cluster_center`]. The
        // location is in the creator's area; with no cluster good enough it is the invalid
        // location (0x7abd00).
        "GetClusterCenter" => {
            let creator = obj(a, 0);
            let area = h.object(creator).and_then(|o| o.area);
            let center = cluster_center(h.sim, creator, int(a, 1), int(a, 2), int(a, 3));
            let l = match (center, area) {
                (Some(p), Some(area)) => Location {
                    area,
                    position: p,
                    facing: 0.0,
                },
                _ => Location {
                    area: OBJECT_INVALID,
                    position: [0.0; 3],
                    facing: 0.0,
                },
            };
            let hd = h.sim.world.store(EngineData::Location(l));
            ok(Value::Engine {
                ty: ty.location,
                handle: Some(hd),
            })
        }
        // DAOrigins.exe answers it in the handler of the GetNearestObject family (0x671510):
        // fewer than one object asked for means one; the location's area then gives the
        // objects whose type is in the mask, nearest first by 3D distance (0x684dd0), with
        // no living, perception, active or self test. Provisional: the order of objects at
        // the same distance (here by id).
        "GetNearestObjectToLocation" => {
            let l = match h.sim.world.handle(handle(a, 0)) {
                Some(EngineData::Location(l)) => Some(*l),
                _ => None,
            };
            let (mask, count) = (int(a, 1), int(a, 2).max(1) as usize);
            let mut found: Vec<(f32, ObjectId)> = match l {
                Some(l) if l.area != OBJECT_INVALID => h
                    .sim
                    .world
                    .objects
                    .values()
                    .filter(|o| o.area == Some(l.area))
                    .filter(|o| o.kind.script_type() & mask != 0)
                    .map(|o| (dist(o.position, l.position), o.id))
                    .collect(),
                _ => Vec::new(),
            };
            found.sort_by(|x, y| x.0.total_cmp(&y.0).then(x.1.cmp(&y.1)));
            ok(objects(
                found.into_iter().take(count).map(|(_, id)| id).collect(),
            ))
        }
        // DAOrigins.exe answers both in one handler (0x670eb0): GetObjectsInShape looks at
        // the objects of the location's area, FilterObjectsInShape at the array it is given
        // (wherever they are). Each must be of a type in the mask, in the world and, with
        // bExcludeDead, alive, and lie in the shape (see [`in_shape`]); an unknown shape
        // takes nothing. Provisional: "in the world" (virtual 0x6c) is taken to be active.
        "GetObjectsInShape" | "FilterObjectsInShape" => {
            let (given, a) = match name {
                "FilterObjectsInShape" => {
                    let list: Vec<ObjectId> = match a.first() {
                        Some(Value::Array { items, .. }) => items
                            .iter()
                            .filter_map(|v| match v {
                                Value::Object(o) => Some(*o),
                                _ => None,
                            })
                            .collect(),
                        _ => Vec::new(),
                    };
                    (Some(list), &a[1.min(a.len())..])
                }
                _ => (None, a),
            };
            let (mask, shape, excl_dead) = (int(a, 0), int(a, 1), int(a, 6) != 0);
            let (fa, fb) = (float(a, 3), float(a, 4));
            let l = match h.sim.world.handle(handle(a, 2)) {
                Some(EngineData::Location(l)) => Some(*l),
                _ => None,
            };
            let candidates: Vec<ObjectId> = match (given, l) {
                (Some(list), _) => list,
                (None, Some(l)) => h
                    .sim
                    .world
                    .objects
                    .values()
                    .filter(|o| o.area == Some(l.area))
                    .map(|o| o.id)
                    .collect(),
                (None, None) => Vec::new(),
            };
            let list: Vec<ObjectId> = match l {
                Some(l) => candidates
                    .into_iter()
                    .filter(|&id| {
                        h.object(id).is_some_and(|o| {
                            o.active && o.kind.script_type() & mask != 0 && !(excl_dead && o.dead)
                        }) && in_shape(h.sim, id, shape, &l, fa, fb)
                    })
                    .collect(),
                None => Vec::new(),
            };
            ok(objects(list))
        }
        "EffectAreaOfEffect" => {
            // EffectAreaOfEffect is type 5 ("Area of Effect (AOE)") in effects.gda.
            let e = Effect {
                ty: 5,
                creator: Some(caller),
                ints: vec![int(a, 0), int(a, 2)],
                strings: vec![string(a, 1)],
                ..Default::default()
            };
            let hd = h.sim.world.store(EngineData::Effect(e));
            ok(Value::Engine {
                ty: ty.effect,
                handle: Some(hd),
            })
        }
        "SetTrapDetected" => {
            if let Some(o) = h.sim.world.get_mut(obj(a, 0)) {
                o.fields.insert("TrapDetected".into(), int(a, 1));
            }
            void()
        }
        "SetRoamRadius" => {
            if let Some(o) = h.sim.world.get_mut(obj(a, 0)) {
                o.locals.floats.insert("__roam_radius".into(), float(a, 1));
            }
            void()
        }
        "SetRoamLocation" => {
            let l = match h.sim.world.handle(handle(a, 1)) {
                Some(EngineData::Location(l)) => Some(*l),
                _ => None,
            };
            if let (Some(l), Some(o)) = (l, h.sim.world.get_mut(obj(a, 0))) {
                o.locals.locations.insert("__roam_location".into(), l);
            }
            void()
        }
        "GetHealth" => {
            let health = constant(&h.sim.table, "PROPERTY_DEPLETABLE_HEALTH");
            ok(Value::Int(
                h.object(obj(a, 0))
                    .and_then(|o| o.properties.get(&health))
                    .map_or(0, |p| p.1.round() as i32),
            ))
        }
        "MoveItem" => {
            let (from, to, item) = (obj(a, 0), obj(a, 1), obj(a, 2));
            let had = h.object(from).is_some_and(|o| o.inventory.contains(&item));
            if had && h.object(to).is_some() {
                h.sim.unequip(from, item);
                if let Some(o) = h.sim.world.get_mut(from) {
                    o.inventory.retain(|&i| i != item);
                }
                if let Some(o) = h.sim.world.get_mut(to) {
                    o.inventory.push(item);
                }
            }
            void()
        }
        // Area effects and map notifications are not simulated yet.
        // An area of effect at a location (DAOrigins.exe 0x65c830 gives the effect its
        // duration, creator and ability, its position and orientation, and applies it to
        // the location's area, whose type-5 handler 0x7a0230 makes the object): see
        // [`crate::aoe`]. A temporary one lasts its duration.
        "Engine_ApplyEffectAtLocation"
            if matches!(h.sim.world.handle(handle(a, 1)),
                Some(EngineData::Effect(e)) if e.ty == crate::aoe::EFFECT_TYPE_AOE) =>
        {
            let l = match h.sim.world.handle(handle(a, 2)) {
                Some(EngineData::Location(l)) => *l,
                _ => return void(),
            };
            if !h.object(l.area).is_some_and(|o| o.kind == ObjectKind::Area) {
                return void();
            }
            let ability = (a.len() > 5).then(|| int(a, 5));
            let Some(e) =
                applied_effect(h, handle(a, 1), int(a, 0), float(a, 3), obj(a, 4), ability)
            else {
                return void();
            };
            let instant = constant(&h.sim.table, "EFFECT_DURATION_TYPE_INSTANT");
            if e.duration_type != instant {
                h.sim
                    .create_aoe(&e, l.area, l.position, l.facing, None, e.expires);
            }
            void()
        }
        // DAOrigins.exe (0x663bd0 -> 0x629360): a large alert with the area's name as the
        // script gives it and, when given, the image to show with it.
        "ShowAreaUnlockedNotification" => {
            let image = (a.len() > 1).then(|| string(a, 1));
            h.sim.raise_large_alert(
                crate::journal::ALERT_WORLD_MAP,
                Some(string(a, 0)),
                image,
                None,
            );
            void()
        }
        "Engine_ApplyEffectAtLocation" => {
            let effect = match h.sim.world.handle(handle(a, 1)) {
                Some(EngineData::Effect(e)) => Some(e.ty),
                _ => None,
            };
            // A visual effect at a location (EffectVisualEffect, type 4 in effects.gda) is
            // presentation; anything else there (an area of effect, which the simulation
            // does not hold yet) is a placeholder, counted for coverage.
            // A visual effect is kept for rendering to draw (`World::location_vfx`).
            if name == "Engine_ApplyEffectAtLocation" && effect == Some(4) {
                let at = match h.sim.world.handle(handle(a, 2)) {
                    Some(EngineData::Location(l)) => Some(*l),
                    _ => None,
                };
                let applied =
                    applied_effect(h, handle(a, 1), int(a, 0), float(a, 3), obj(a, 4), None);
                if let (Some(l), Some(mut e)) = (at, applied) {
                    let instant = constant(&h.sim.table, "EFFECT_DURATION_TYPE_INSTANT");
                    if e.duration_type != instant {
                        h.sim.world.next_vfx += 1;
                        let handle = h.sim.world.next_vfx;
                        if e.id == 0 {
                            e.id = (handle as i32).saturating_add(1_000_000);
                        }
                        let now = h.sim.world.time_ms;
                        let entry = crate::world::LocationVfx {
                            handle,
                            vfx: e.ints.first().copied().unwrap_or(0),
                            area: l.area,
                            position: l.position,
                            facing: l.facing,
                            started_ms: now,
                            duration_ms: e.expires.map(|x| x.saturating_sub(now)),
                            effect: e,
                        };
                        h.sim.world.location_vfx.push(entry);
                    }
                }
            }
            if name == "Engine_ApplyEffectAtLocation" {
                let tally = if effect == Some(4) {
                    &mut h.sim.world.trace.presentation
                } else {
                    &mut h.sim.world.trace.stubbed
                };
                *tally.entry(name.to_string()).or_default() += 1;
            }
            let ty = effect.map_or(String::new(), |e| format!(" effect type {e}"));
            h.sim.world.trace.push(TraceEntry::Log {
                t,
                channel: -3,
                text: format!("{name}{:?}{ty} (no effect in the simulation)", a),
            });
            void()
        }
        "GetCommandByIndex" => {
            let c = h
                .object(obj(a, 0))
                .and_then(|o| o.commands.get(int(a, 1).max(0) as usize).cloned());
            let hd = c.map(|c| h.sim.world.store(EngineData::Command(c)));
            ok(Value::Engine {
                ty: ty.command,
                handle: hd,
            })
        }
        // Where a follower would stand behind the party leader (see `Sim::follow_leader`).
        "GetFollowerWouldBeLocation" => {
            let who = obj(a, 0);
            let hero = h.sim.world.hero.and_then(|x| h.object(x));
            let l = match (hero, h.object(who)) {
                // DAOrigins.exe's handler (0x666650) gives the follower's slot of the default
                // formation table around the leader (see `crate::sim::behind`).
                (Some(leader), Some(me)) => {
                    let i = leader.followers.iter().position(|&f| f == who).unwrap_or(0);
                    Location {
                        area: leader.area.or(me.area).unwrap_or(OBJECT_INVALID),
                        position: crate::sim::behind(leader.position, leader.facing, i),
                        facing: leader.facing,
                    }
                }
                _ => Location {
                    area: OBJECT_INVALID,
                    position: [0.0; 3],
                    facing: 0.0,
                },
            };
            let hd = h.sim.world.store(EngineData::Location(l));
            ok(Value::Engine {
                ty: ty.location,
                handle: Some(hd),
            })
        }
        // Non-party followers (an escort that tags along) follow like party members.
        "AddNonPartyFollower" | "RemoveNonPartyFollower" => {
            let who = obj(a, 0);
            let exists = h.object(who).is_some();
            if let Some(ho) = h.sim.world.hero.and_then(|x| h.sim.world.get_mut(x)) {
                ho.escorts.retain(|&f| f != who);
                if name == "AddNonPartyFollower" && exists {
                    ho.escorts.push(who);
                }
            }
            ok(Value::Int(exists as i32))
        }
        "GetCommandQueueSize" => ok(Value::Int(
            h.object(obj(a, 0)).map_or(0, |o| o.commands.len() as i32),
        )),
        // Cancelling a command in progress completes it (COMMAND_RESULT_INVALID), which is
        // how the AI notices it has nothing to do. Provisional.
        // DAOrigins.exe 0x65bf50 -> 0x73c730(-2, 0, 1, 0): the command being carried out
        // (the front one, once started) ends with COMMAND_FAILED_COMMAND_CLEARED (-2) and
        // the object is told (COMMAND_COMPLETE); the queue behind it stays. A command not yet
        // started is not the current one, so nothing happens. Provisional: the exe also
        // leaves a current command its queue marks as not to be interrupted (+4 of the
        // queue's head); that mark is not modelled.
        "ClearCurrentCommand" => {
            let who = obj(a, 0);
            let running = h.sim.world.get_mut(who).and_then(|o| {
                if o.commands.first().is_some_and(|c| c.due.is_some()) {
                    Some(o.commands.remove(0).ty)
                } else {
                    None
                }
            });
            if let Some(ty) = running {
                let mut ev = h.sim.new_event("COMMAND_COMPLETE", Some(who));
                ev.ints = vec![ty, constant(&h.sim.table, "COMMAND_FAILED_COMMAND_CLEARED")];
                h.sim.world.queue_event(0, who, ev, None);
            }
            void()
        }
        "ClearAllCommands" => {
            let who = obj(a, 0);
            let running = h.sim.world.get_mut(who).and_then(|o| {
                let first = o.commands.first().map(|c| c.ty);
                o.commands.clear();
                first
            });
            if let Some(ty) = running {
                let mut ev = h.sim.new_event("COMMAND_COMPLETE", Some(who));
                ev.ints = vec![ty, constant(&h.sim.table, "COMMAND_RESULT_INVALID")];
                h.sim.world.queue_event(0, who, ev, None);
            }
            ok(Value::Int(1))
        }

        // --- Conversation -----------------------------------------------------------------
        "DoAreaTransition" => {
            let area = string(a, 0);
            if area.is_empty() {
                return ok(Value::Int(0));
            }
            h.sim.world.transition = Some((area, string(a, 1)));
            h.sim.world.transition_at = Some(h.sim.world.time_ms + crate::sim::TRANSITION_DELAY_MS);
            ok(Value::Int(1))
        }
        "GetPackageAI" => ok(Value::Int(
            h.object(obj(a, 0))
                .and_then(|o| o.fields.get("PackageAI").copied())
                .unwrap_or(0),
        )),
        "SetEventScript" => {
            let script = string(a, 1).trim_end_matches(".ncs").to_string();
            if let Some(o) = h.sim.world.get_mut(obj(a, 0)) {
                o.script = (!script.is_empty()).then_some(script);
            }
            void()
        }
        "IntToHexString" => ok(Value::String(format!("0x{:08x}", int(a, 0)))),
        "GetPlotSummary" => ok(Value::String(String::new())),
        "GetItemPossessedBy" => {
            let tag = string(a, 1);
            let item = h
                .object(obj(a, 0))
                .map(|o| o.inventory.clone())
                .unwrap_or_default()
                .into_iter()
                .find(|i| {
                    h.object(*i)
                        .is_some_and(|x| x.tag.eq_ignore_ascii_case(&tag))
                });
            ok(Value::Object(item.unwrap_or(OBJECT_INVALID)))
        }
        // Followers: ACTIVE and LOCKEDACTIVE put a creature in the active party (the hero's
        // followers); any other state takes it out. Provisional: the party pool is not
        // modelled apart from the active party.
        "GetFollowerState" | "GetFollowerSubState" => {
            let field = if name == "GetFollowerState" {
                "FollowerState"
            } else {
                "FollowerSubState"
            };
            // The hero is in the active party without ever being given a state (creature_core
            // rewards XP only to party members whose state is set). Provisional: ACTIVE.
            let hero_default = if name == "GetFollowerState" && Some(obj(a, 0)) == h.sim.world.hero
            {
                constant(&h.sim.table, "FOLLOWER_STATE_ACTIVE")
            } else {
                0
            };
            ok(Value::Int(
                h.object(obj(a, 0))
                    .and_then(|o| o.fields.get(field).copied())
                    .unwrap_or(hero_default),
            ))
        }
        "SetFollowerState" => {
            let (who, state) = (obj(a, 0), int(a, 1));
            if h.object(who).is_none() {
                return void();
            }
            let active = state == constant(&h.sim.table, "FOLLOWER_STATE_ACTIVE")
                || state == constant(&h.sim.table, "FOLLOWER_STATE_LOCKEDACTIVE");
            if let Some(o) = h.sim.world.get_mut(who) {
                o.fields.insert("FollowerState".into(), state);
            }
            if let Some(hero) = h.sim.world.hero.filter(|&x| x != who) {
                let hero_group = h.object(hero).map(|o| o.group);
                // No PARTYMEMBER_ADDED/DROPPED: DAOrigins.exe's SetFollowerState (0x65e6e0 →
                // 0x63b9a0, by state through 0x63ba0c) only moves the follower between the
                // pool and the party; only the party picker raises those events (0x636f50,
                // 0x766c10; see `Sim::party_picker`). Raising them here made the party plot's
                // handlers, which set the state again, answer each other forever (the Human
                // Noble siege, leaving Flemeth's hut).
                if let Some(ho) = h.sim.world.get_mut(hero) {
                    ho.followers.retain(|&f| f != who);
                    if active {
                        ho.followers.push(who);
                    }
                }
                // Joining the active party puts a follower on the player's side
                // (GROUP_PC), whatever group it had as an NPC, and at the leader's side if
                // it was elsewhere. Provisional.
                let at = h.object(hero).map(|o| (o.area, o.position));
                if let (true, Some(g), Some(o)) = (active, hero_group, h.sim.world.get_mut(who)) {
                    o.group = g;
                    if let Some((area, pos)) = at {
                        if o.area != area {
                            o.area = area;
                            o.position = pos;
                            o.parked = None;
                            o.active = true;
                        }
                    }
                }
            }
            void()
        }
        "GetFollowerApproval" => ok(Value::Int(
            h.object(obj(a, 0))
                .and_then(|o| o.fields.get("Approval").copied())
                .unwrap_or(0),
        )),
        "AdjustFollowerApproval" => {
            if let Some(o) = h.sim.world.get_mut(obj(a, 0)) {
                *o.fields.entry("Approval".into()).or_default() += int(a, 1);
            }
            void()
        }
        "SetFollowerApprovalDescription" => {
            if let Some(o) = h.sim.world.get_mut(obj(a, 0)) {
                o.fields.insert("ApprovalDescription".into(), int(a, 1));
            }
            void()
        }
        // Heartbeats: EVENT_TYPE_HEARTBEAT2 every interval until EndHeartbeat.
        "InitHeartbeat" => {
            let who = obj(a, 0);
            let ms = ((float(a, 1).max(0.1)) * 1000.0) as i32;
            if let Some(o) = h.sim.world.get_mut(who) {
                o.fields.insert("HeartbeatMs".into(), ms);
            }
            let ev = h.sim.new_event("HEARTBEAT2", Some(who));
            h.sim.world.queue_event(ms as u64, who, ev, None);
            void()
        }
        "EndHeartbeat" => {
            if let Some(o) = h.sim.world.get_mut(obj(a, 0)) {
                o.fields.remove("HeartbeatMs");
            }
            void()
        }
        "SetMaxHealth" => {
            // PROPERTY_DEPLETABLE_HEALTH: base is the maximum, current is clamped to it.
            let health = constant(&h.sim.table, "PROPERTY_DEPLETABLE_HEALTH");
            if let Some(o) = h.sim.world.get_mut(obj(a, 0)) {
                let max = int(a, 1) as f32;
                let e = o.properties.entry(health).or_insert((max, max));
                e.0 = max;
                if e.1 <= 0.0 || e.1 > max {
                    e.1 = max;
                }
            }
            void()
        }
        // DAOrigins.exe 0x65dc00 reads the script context: conditions use the line's
        // field 12207 (0x7f04ca), actions the conversation's ambient state (0x7eee00).
        // dialog.rs sets the appropriate context before each script.
        "DialogIsAmbient" => ok(b(h.sim.dialog_ambient)),
        // DAOrigins.exe (0x6510f0) stops the ambient lines still to be spoken by an object
        // (all of them for OBJECT_INVALID) and returns TRUE.
        "ClearAmbientDialogs" => {
            let o = obj(a, 0);
            h.sim.clear_ambient(o);
            ok(Value::Int(1))
        }
        "SetTacticPresetID" => {
            h.sim.apply_tactic_preset(obj(a, 0), int(a, 1));
            void()
        }
        "GetTacticPresetID" => ok(Value::Int(
            h.object(obj(a, 0))
                .and_then(|o| o.fields.get("TacticPreset").copied())
                .unwrap_or(0),
        )),
        "SetTacticEntry" => {
            let (who, i) = (obj(a, 0), int(a, 1).clamp(0, 63) as usize);
            let tactic = crate::combat::Tactic {
                enabled: int(a, 2) != 0,
                target: int(a, 3),
                condition: int(a, 4),
                command: int(a, 5),
                param: int(a, 6),
                ..Default::default()
            };
            if let Some(o) = h.sim.world.get_mut(who) {
                if o.combat.tactics.len() <= i {
                    o.combat.tactics.resize(i + 1, Default::default());
                }
                o.combat.tactics[i] = tactic;
            }
            void()
        }
        "IsTacticEnabled"
        | "GetTacticTargetType"
        | "GetTacticCondition"
        | "GetTacticCommand"
        | "GetTacticCommandParam" => {
            let (who, i) = (obj(a, 0), int(a, 1));
            let num = h
                .object(who)
                .and_then(|o| o.fields.get("NumTactics").copied())
                .unwrap_or(i32::MAX);
            let tactics = h.sim.tactics_of(who);
            // Slots are numbered from 1 to GetNumTactics: player_core fills them from 1 and
            // rules_core reads them while the index is no more than GetNumTactics.
            let t = (i >= 0 && i <= num)
                .then(|| tactics.get(i as usize))
                .flatten();
            ok(Value::Int(match (name, t) {
                (_, None) => 0,
                ("IsTacticEnabled", Some(t)) => t.enabled as i32,
                ("GetTacticTargetType", Some(t)) => t.target,
                ("GetTacticCondition", Some(t)) => t.condition,
                ("GetTacticCommand", Some(t)) => t.command,
                (_, Some(t)) => t.param,
            }))
        }
        "SetFollowPartyLeader" => {
            let field = "FollowPartyLeader";
            if let Some(o) = h.sim.world.get_mut(obj(a, 0)) {
                o.fields.insert(field.to_string(), int(a, 1));
            }
            void()
        }
        // DAOrigins.exe (0x651710, the list's add at 0x7af4e0): a preset already in the
        // creature's list is not added again.
        "AddTacticPresetID" => {
            let (who, preset) = (obj(a, 0), int(a, 1));
            if h.sim.tactic_presets(who).contains(&preset) {
                return void();
            }
            if let Some(o) = h.sim.world.get_mut(who) {
                let n = o.fields.get("TacticPresetCount").copied().unwrap_or(0);
                o.fields.insert(format!("TacticPreset.{n}"), int(a, 1));
                o.fields.insert("TacticPresetCount".into(), n + 1);
            }
            void()
        }
        "GetOrientation" => ok(Value::Vector([
            0.0,
            0.0,
            h.object(obj(a, 0)).map_or(0.0, |o| o.facing),
        ])),
        "SetOrientation" => {
            let v = vector(a, 1);
            if let Some(o) = h.sim.world.get_mut(obj(a, 0)) {
                o.facing = v[2];
            }
            void()
        }
        "GetItemsInInventory" => {
            let tag = string(a, 3);
            if h.sim.game == thedas_vfs::Game::Origins {
                // 0x672410: creature equipment first, then ordinary and plot backpacks.
                // Placeables/stores apply only the tag filter, regardless of the options.
                let Some(owner) = h.object(obj(a, 0)) else {
                    return ok(objects(Vec::new()));
                };
                let mut items = Vec::new();
                if owner.kind == ObjectKind::Creature {
                    let options = a.get(1).map_or(1, |_| int(a, 1));
                    let worn: Vec<_> = owner
                        .equipment
                        .values()
                        .chain(owner.other_set.values())
                        .copied()
                        .collect();
                    if options > 0 {
                        items.extend(worn.iter().copied());
                    }
                    if options != 2 {
                        for plot in [false, true] {
                            if plot && int(a, 4) != 0 {
                                break;
                            }
                            items.extend(owner.inventory.iter().copied().filter(|i| {
                                !worn.contains(i) && h.object(*i).is_some_and(|x| x.plot == plot)
                            }));
                        }
                    }
                    let base = int(a, 2);
                    items.retain(|i| {
                        h.object(*i).is_some_and(|x| {
                            x.kind == ObjectKind::Item
                                && (base == 0 || x.fields.get("BaseItem").copied() == Some(base))
                        })
                    });
                } else if matches!(owner.kind, ObjectKind::Placeable | ObjectKind::Store) {
                    items.extend(owner.inventory.iter().copied());
                }
                items.retain(|i| {
                    h.object(*i).is_some_and(|x| {
                        x.kind == ObjectKind::Item && (tag.is_empty() || x.tag == tag)
                    })
                });
                return ok(objects(items));
            }
            let items: Vec<ObjectId> = h
                .object(obj(a, 0))
                .map(|o| o.inventory.clone())
                .unwrap_or_default()
                .into_iter()
                .filter(|i| {
                    tag.is_empty()
                        || h.object(*i)
                            .is_some_and(|x| x.tag.eq_ignore_ascii_case(&tag))
                })
                .collect();
            ok(objects(items))
        }
        // Presentation and platform services with no simulation state.
        // DAOrigins.exe (0x665030) clears the fog on the current area's map. The simulation
        // keeps the area's `MapRevealed` field for the map to read.
        "RevealCurrentMap" => {
            if let Some(o) = h.sim.world.area.and_then(|a| h.sim.world.get_mut(a)) {
                o.fields.insert("MapRevealed".into(), 1);
            }
            void()
        }
        // The game is saved to the autosave slot: the presentation writes it (see
        // `World::autosave_requested`), as it writes every save. Provisional: when
        // DAOrigins.exe writes it (at once, or when the update ends) and what it does in a
        // tutorial that forbids saving are not traced.
        "DoAutoSave" => {
            h.sim.world.autosave_requested = true;
            void()
        }
        "TakeScreenshot" => {
            h.sim.world.trace.push(TraceEntry::Log {
                t,
                channel: -3,
                text: format!("{name} (no effect in the simulation)"),
            });
            void()
        }
        "LoadCutscene" => {
            h.sim.world.cutscenes.push(string(a, 0));
            void()
        }
        // Dragon Age II's one call for both: `void PlayCutscene(resource rCutscene)` (its id
        // 647 is DA:O's LoadCutscene). DA:O's own `PlayCutscene()` takes no argument and is
        // left alone. Provisional: from DA2's script.ldf declaration.
        "PlayCutscene" if matches!(a.first(), Some(Value::Resource(_) | Value::String(_))) => {
            h.sim.world.cutscenes.push(string(a, 0));
            void()
        }
        "PreloadCharGen" => void(),
        "StartCharGen" => {
            h.sim.start_chargen(obj(a, 0), int(a, 1), int(a, 2));
            void()
        }
        // DAOrigins.exe (0x65d9a0 -> 0x6976b0): a target that is the owner itself means the
        // hero; the owner must be an active, living creature. Nothing here looks at the game
        // mode: a conversation may start in combat (and replaces it, see
        // `Sim::start_conversation`).
        // Dragon Age II's `int BeginConversation(object oTarget, resource rConversationFile)`
        // has no owner argument: the script's object talks to the target. Provisional: from
        // DA2's script.ldf declaration and its prologue, whose plot script starts the
        // player's own conversation (`pro000_opening_pc`) running as the player's creature.
        "BeginConversation" if matches!(a.get(1), Some(Value::Resource(_) | Value::String(_))) => {
            let target = obj(a, 0);
            let owner = if h
                .object(caller)
                .is_some_and(|o| o.kind == ObjectKind::Creature)
            {
                caller
            } else {
                target
            };
            let target = if target == owner {
                h.sim.world.hero.unwrap_or(target)
            } else {
                target
            };
            let can = h
                .object(owner)
                .is_some_and(|o| o.kind == ObjectKind::Creature && o.active && !o.dead);
            let ok_ = can && h.sim.start_conversation(target, owner, &string(a, 1));
            ok(b(ok_))
        }
        "BeginConversation" => {
            let owner = obj(a, 1);
            let mut target = obj(a, 0);
            if target == owner {
                target = h.sim.world.hero.unwrap_or(target);
            }
            let can = h
                .object(owner)
                .is_some_and(|o| o.kind == ObjectKind::Creature && o.active && !o.dead);
            let ok_ = can && h.sim.start_conversation(target, owner, &string(a, 2));
            ok(b(ok_))
        }

        // --- Template-backed getters --------------------------------------------------------
        // A placeable's type (1 door, 2 container, ...): the placeables table's (93)
        // `BaseType` at its appearance, as DAOrigins.exe reads it when loading one
        // (0x75084d) into the byte the command returns (0x65efc0). placeable_core
        // generates a container's treasure on spawn only for type 2.
        // A lock's template fields, as DAOrigins.exe's getters read them off the placeable
        // (0x65f020, 0x65f080, 0x65f0e0, 0x65f1c0): `AutoRemoveKey`, `KeyRequired`,
        // `KeyName` and `PickLock`.
        "GetPlaceableAutoRemoveKey" | "GetPlaceableKeyRequired" | "GetPlaceablePickLockLevel" => {
            let label = match name {
                "GetPlaceableAutoRemoveKey" => "AutoRemoveKey",
                "GetPlaceableKeyRequired" => "KeyRequired",
                _ => "PickLock",
            };
            ok(Value::Int(
                h.object(obj(a, 0))
                    .and_then(|o| o.fields.get(label).copied())
                    .unwrap_or(0),
            ))
        }
        "GetPlaceableKeyTag" => ok(Value::String(
            h.object(obj(a, 0))
                .and_then(|o| o.text_fields.get("KeyName").cloned())
                .unwrap_or_default(),
        )),
        "GetPlaceableBaseType" => ok(Value::Int(
            h.object(obj(a, 0))
                .filter(|o| o.kind == ObjectKind::Placeable)
                .and_then(|o| o.fields.get("Appearance").copied())
                .and_then(|ap| {
                    match h
                        .sim
                        .world
                        .tables()
                        .get(93, "")?
                        .cell("BaseType", ap as i64)?
                    {
                        Cell::Int(v) => Some(*v as i32),
                        _ => None,
                    }
                })
                .unwrap_or(0),
        )),
        "GetPackage" | "GetCreatureRacialType" | "GetCombatantType" => {
            let field = match name {
                "GetPackage" => "Package",
                "GetCreatureRacialType" => "Race",
                "GetCombatantType" => "Combatant",
                _ => "Appearance",
            };
            ok(Value::Int(
                h.object(obj(a, 0))
                    .and_then(|o| o.fields.get(field).copied())
                    .unwrap_or(0),
            ))
        }
        "GetCreatureGender" => ok(Value::Int(
            h.object(obj(a, 0))
                .and_then(|o| o.fields.get("Gender").copied())
                .unwrap_or(0),
        )),
        "SetCreatureRacialType" | "SetCreatureGender" => {
            let field = match name {
                "SetCreatureRacialType" => "Race",
                _ => "Gender",
            };
            if let Some(o) = h.sim.world.get_mut(obj(a, 0)) {
                o.fields.insert(field.to_string(), int(a, 1));
            }
            void()
        }
        // -1 restores the creature's own appearance (rules_core ends a shapeshift so).
        // Provisional: its own is taken to be its template's. A party member losing its
        // appearance is traced: it draws nothing.
        "SetAppearanceType" => {
            let who = obj(a, 0);
            let mut app = int(a, 1);
            if app == -1 {
                app = h
                    .sim
                    .original_appearance(who)
                    .or_else(|| h.object(who).map(|o| o.appearance))
                    .unwrap_or(0);
            }
            if app <= 0 && crate::engine::party_of(h.sim).contains(&who) {
                let t = h.sim.world.time_ms;
                let tag = h.object(who).map(|o| o.tag.clone()).unwrap_or_default();
                h.sim.world.trace.push(crate::trace::TraceEntry::Log {
                    t,
                    channel: -1,
                    text: format!(
                        "SetAppearanceType({tag}, {}) leaves it nothing to draw",
                        int(a, 1)
                    ),
                });
            }
            if let Some(o) = h.sim.world.get_mut(who) {
                o.appearance = app;
            }
            void()
        }
        "SetCreatureRank" => {
            if let Some(o) = h.sim.world.get_mut(obj(a, 0)) {
                o.rank = int(a, 1);
            }
            void()
        }
        "GetAbilityList" => ok(Value::Array {
            elem: Type::Int,
            items: da2_ability_list(h.sim, obj(a, 0), int(a, 1), int(a, 2) != 0)
                .into_iter()
                .map(Value::Int)
                .collect(),
        }),
        "CombatGetPercentageFromScaled" => ok(Value::Float(da2_percentage_from_scaled(
            h.sim,
            int(a, 0),
            float(a, 1),
            int(a, 2),
        ))),
        // The abilities of a type (`ABI_base` `AbilityType`, 0 for any) and GUI type
        // (`guitype`, 0 for any), as script.ldf documents.
        "GetAbilityCount" => {
            let (ty, gui) = (int(a, 1), int(a, 2));
            let abilities: Vec<i32> = h
                .object(obj(a, 0))
                .map(|o| o.abilities.iter().copied().collect())
                .unwrap_or_default();
            let n = abilities
                .into_iter()
                .filter(|&ab| ty == 0 || h.sim.abi_int(ab, "AbilityType") == ty)
                .filter(|&ab| gui == 0 || h.sim.abi_int(ab, "guitype") == gui)
                .count();
            ok(Value::Int(n as i32))
        }
        // One list (an `AbilityType`), or every list for `ABILITY_TYPE_INVALID` (0).
        "CharGen_ClearAbilityList" => {
            let (who, ty) = (obj(a, 0), int(a, 1));
            let keep: Vec<i32> = h
                .object(who)
                .map(|o| o.abilities.iter().copied().collect::<Vec<i32>>())
                .unwrap_or_default()
                .into_iter()
                .filter(|&ab| ty != 0 && h.sim.abi_int(ab, "AbilityType") != ty)
                .collect();
            if let Some(o) = h.sim.world.get_mut(who) {
                o.abilities = keep.into_iter().collect();
            }
            void()
        }
        "GetBaseItemType" => ok(Value::Int(
            h.object(obj(a, 0))
                .and_then(|o| o.fields.get("BaseItem").copied())
                .unwrap_or(0),
        )),
        // The hero, then the followers in the pool, in the active party or not (script.ldf:
        // "the array of followers belonging to the party pool").
        "GetPartyPoolList" => {
            let mut v: Vec<ObjectId> = h.sim.world.hero.into_iter().collect();
            v.extend(h.sim.party_pool());
            ok(objects(v))
        }
        "IsStringEmpty" => ok(b(string(a, 0).is_empty())),
        "GetLowResTimer" => ok(Value::Int(h.sim.world.time_ms as i32)),
        "DEBUG_PrintToScreen" | "DisplayStatusMessage" => {
            if name == "DisplayStatusMessage" {
                // For the interface's status lines (kept short when nothing takes them).
                if h.sim.world.status_messages.len() >= 64 {
                    h.sim.world.status_messages.remove(0);
                }
                h.sim.world.status_messages.push((string(a, 0), int(a, 1)));
            }
            h.sim.world.trace.push(TraceEntry::Log {
                t,
                channel: -2,
                text: string(a, 0),
            });
            void()
        }
        // The active weapon set (0 or 1). Switching swaps the weapon slots with the inactive
        // set's. Provisional: no equip events are sent for the swap.
        "GetActiveWeaponSet" => ok(Value::Int(
            h.object(obj(a, 0))
                .and_then(|o| o.fields.get("ActiveWeaponSet").copied())
                .unwrap_or(0),
        )),
        "SwitchWeaponSet" => {
            h.sim.switch_weapon_set(obj(a, 0), int(a, 1));
            void()
        }
        // The plot actions control (world state the interface shows).
        "SetPlotActionSet" => {
            h.sim.world.plot_actions.set = int(a, 0);
            void()
        }
        "SetPlotActionsEnabled" => {
            h.sim.world.plot_actions.disabled = int(a, 0) == 0;
            void()
        }
        "SetPlotActionState" => {
            h.sim.world.plot_actions.states.insert(int(a, 0), int(a, 1));
            void()
        }
        "GetPlotActionState" => ok(Value::Int(
            h.sim
                .world
                .plot_actions
                .states
                .get(&int(a, 0))
                .copied()
                .unwrap_or(0),
        )),
        "SetPlotActionCount" => {
            h.sim.world.plot_actions.counts.insert(int(a, 0), int(a, 1));
            void()
        }
        "GetPlotActionCount" => ok(Value::Int(
            h.sim
                .world
                .plot_actions
                .counts
                .get(&int(a, 0))
                .copied()
                .unwrap_or(0),
        )),
        // Pools only make creatures ahead of time; CreateObject makes them either way.
        // DAOrigins.exe (0x6598d0) builds a pool of creatures from the template in the current
        // area ahead of the scripts' CreateObject calls, and returns TRUE: a loading
        // optimisation the simulation does without (it creates objects when asked).
        "CreatePool" => ok(Value::Int(1)),
        "GetCreatureGoreLevel" => ok(Value::Float(0.0)),
        "GetCreaturePropertyType" => {
            ok(Value::Int(if h.sim.game == thedas_vfs::Game::DragonAge2 {
                property_bounds(h.sim, int(a, 1)).map_or(0, |b| b.0)
            } else {
                h.sim
                    .world
                    .tables
                    .get(0, "properties")
                    .and_then(|t| t.cell("Type", int(a, 1) as i64).cloned())
                    .map_or(0, |c| match c {
                        Cell::String(s) if clean(&s).eq_ignore_ascii_case("ATTRIBUTE") => 1,
                        Cell::String(s) if clean(&s).eq_ignore_ascii_case("SIMPLE") => 2,
                        Cell::String(s) if clean(&s).eq_ignore_ascii_case("DEPLETABLE") => 3,
                        Cell::String(s) if clean(&s).eq_ignore_ascii_case("DERIVED") => 4,
                        Cell::Int(v) => v as i32,
                        _ => 0,
                    })
            }))
        }
        "Deprecated_GetMaxHealth" => ok(Value::Int(
            h.object(obj(a, 0))
                .and_then(|o| {
                    o.fields
                        .get("MaxHealth")
                        .or_else(|| o.fields.get("Health"))
                        .copied()
                })
                .unwrap_or(0),
        )),
        "GetTeam" => {
            let team = int(a, 0);
            let mask = if a.len() > 1 { int(a, 1) } else { 32 };
            // Origins 0x66e570 obtains the current area's team registry (0x5d4fc0,
            // 0x5fe470). Its type argument selects one bucket, log2(type) + 1,
            // rather than combining the bits. -1 is the invalid team id.
            let mask = if h.sim.game == thedas_vfs::Game::Origins {
                if team == -1 || mask <= 0 || h.sim.world.area.is_none() {
                    return ok(objects(Vec::new()));
                }
                1 << (mask as u32).ilog2()
            } else if mask == 0 {
                32
            } else {
                mask
            };
            // Inactive and dead members remain in the registry; other areas do not.
            let here = h.sim.world.area;
            let v: Vec<ObjectId> = h
                .sim
                .world
                .objects
                .values()
                .filter(|o| o.team == team && o.kind.script_type() & mask != 0 && o.area == here)
                .map(|o| o.id)
                .collect();
            ok(objects(v))
        }
        "SetPartyPickerGUIStatus" => {
            if let Some(m) = h.sim.world.module.and_then(|m| h.sim.world.get_mut(m)) {
                m.fields.insert("PartyPickerGUIStatus".into(), int(a, 0));
            }
            void()
        }
        "GetPartyPickerGUIStatus" => ok(Value::Int(
            h.sim
                .world
                .module
                .and_then(|m| h.object(m))
                .and_then(|m| m.fields.get("PartyPickerGUIStatus").copied())
                .unwrap_or(0),
        )),
        "ShowPartyPickerGUI" => {
            h.sim.party_picker();
            void()
        }
        // --- World map (see `worldmap`) -----------------------------------------------------
        "SetWorldMapLocationStatus" => {
            if let Some(o) = h.sim.world.get_mut(obj(a, 0)) {
                o.fields.insert("WMStatus".into(), int(a, 1));
            }
            void()
        }
        "SetWorldMapLocationLocName" => {
            if let Some(o) = h.sim.world.get_mut(obj(a, 0)) {
                o.name_strref = Some(int(a, 1).max(0) as u32);
            }
            void()
        }
        "SetWorldMapPlayerLocation" => {
            h.sim.world.world_map.player_pin = Some(obj(a, 1));
            void()
        }
        // Scripts clear the secondary map with OBJECT_INVALID.
        "SetWorldMapPrimary" | "SetWorldMapSecondary" => {
            let map = Some(obj(a, 0)).filter(|&m| h.sim.is_world_map(m));
            if name == "SetWorldMapPrimary" {
                h.sim.world.world_map.primary = map;
            } else {
                h.sim.world.world_map.secondary = map;
            }
            void()
        }
        "SetWorldMapGuiStatus" => {
            h.sim.world.world_map.gui_status = int(a, 0);
            void()
        }
        // The Fade map: the Circle Tower's Fade scripts make it the primary map before they
        // open it (cir000ip_fade_pedistal). Provisional: taken to open the primary map.
        "OpenPrimaryWorldMap" | "OpenFadeMap" => {
            h.sim.world.world_map.open = true;
            void()
        }
        "ClosePrimaryWorldMap" => {
            h.sim.world.world_map.open = false;
            void()
        }
        // An encounter on the way stops the journey in the encounter's area; otherwise the
        // party arrives (EVENT_TYPE_WORLDMAP_PRETRANSITION). Provisional: travel is instant.
        "WorldMapStartTravelling" => {
            let area = string(a, 0);
            h.sim.world.world_map.open = false;
            if area.is_empty() {
                h.sim.finish_travel();
            } else {
                h.sim.world.world_map.encounter = true;
                h.sim.world.transition = Some((area, string(a, 1)));
                h.sim.world.transition_at =
                    Some(h.sim.world.time_ms + crate::sim::TRANSITION_DELAY_MS);
            }
            void()
        }
        "WorldMapCompleteRandomEncounter" => {
            h.sim.finish_travel();
            void()
        }
        // Projectiles fly at a fixed speed (provisional) and, if given an impact event, deliver
        // it to their event target (the firer by default) when they arrive.
        "FireProjectile" | "FireHomingProjectile" => {
            let from = vector(a, 1);
            let (to, event_target) = if name == "FireProjectile" {
                (vector(a, 2), obj(a, 5))
            } else {
                let tgt = h.object(obj(a, 2)).map_or(from, |o| o.position);
                (tgt, obj(a, 4))
            };
            const SPEED: f32 = 20.0;
            let ms = (dist(from, to) / SPEED * 1000.0).clamp(0.0, 60_000.0) as i32;
            let area = h.object(caller).and_then(|o| o.area);
            let id = h.sim.world.spawn(ObjectKind::Projectile, "");
            if let Some(o) = h.sim.world.get_mut(id) {
                o.area = area;
                o.position = from;
                o.fields.insert("FlightMs".into(), ms);
                o.fields.insert("ProjectileType".into(), int(a, 0));
                o.locals.objects.insert(
                    "EVENT_TARGET".into(),
                    if event_target == OBJECT_INVALID {
                        caller
                    } else {
                        event_target
                    },
                );
            }
            let delay = ms as u64;
            h.sim.world.queue_destroy(delay, id);
            ok(Value::Object(id))
        }
        "SetProjectileImpactEvent" => {
            let proj = obj(a, 0);
            if let (Some(p), Some(EngineData::Event(ev))) = (
                h.object(proj).cloned(),
                h.sim.world.handle(handle(a, 1)).cloned(),
            ) {
                let target = p
                    .locals
                    .objects
                    .get("EVENT_TARGET")
                    .copied()
                    .unwrap_or(caller);
                let ms = p.fields.get("FlightMs").copied().unwrap_or(0).max(0) as u64;
                h.sim.world.queue_event(ms, target, ev, None);
            }
            void()
        }
        "StoreFollowerInventory" => {
            h.sim.store_inventory(obj(a, 0), obj(a, 1), true);
            void()
        }
        "RestoreFollowerInventory" => {
            h.sim.restore_inventory(obj(a, 0), obj(a, 1));
            void()
        }
        // The party inventory is the hero's backpack (not what anyone wears). Provisional.
        "StorePartyInventory" | "RestorePartyInventory" => {
            if let Some(hero) = h.sim.world.hero {
                if name == "StorePartyInventory" {
                    h.sim.store_inventory(hero, obj(a, 0), false);
                } else {
                    h.sim.restore_inventory(hero, obj(a, 0));
                }
            }
            void()
        }
        // The action a click takes: the placeable type's state controller
        // (placeable_types.gda, e.g. StateCnt_Informational) gives each state's default
        // action. Without one, an area-transition destination means a transition, a
        // conversation a dialog, anything else a plain use (provisional fallback).
        "SetPlaceableActionResult" => {
            let (id, action, success, variation) =
                (obj(a, 0), int(a, 1), int(a, 2) != 0, int(a, 3) != 0);
            if success {
                if let Some(next) = placeable_transition(h.sim, id, action, variation) {
                    if let Some(o) = h.sim.world.get_mut(id) {
                        o.fields.insert("State".into(), next);
                    }
                }
            }
            void()
        }
        "GetPlaceableAction" => {
            let id = obj(a, 0);
            if let Some(action) = placeable_default_action(h.sim, id) {
                return ok(Value::Int(action));
            }
            let o = h.object(id);
            let dest = o.is_some_and(|o| {
                o.locals
                    .strings
                    .get("PLC_AT_DEST_AREA_TAG")
                    .is_some_and(|d| !d.is_empty() && !d.eq_ignore_ascii_case("none"))
            });
            let name = if dest {
                "PLACEABLE_ACTION_AREA_TRANSITION"
            } else if o.is_some_and(|o| o.dialog.is_some()) {
                "PLACEABLE_ACTION_DIALOG"
            } else {
                "PLACEABLE_ACTION_USE"
            };
            ok(Value::Int(constant(&h.sim.table, name)))
        }
        // The quest board screen: which board is open (jobboards.gda row) is kept for the
        // interface; accepting a job is the screen's business.
        "ShowChantersGUI" => {
            h.sim.open_chanter_board(int(a, 0));
            void()
        }
        "SetObjectInteractive" => {
            if let Some(o) = h.sim.world.get_mut(obj(a, 0)) {
                o.interactive = int(a, 1) != 0;
            }
            void()
        }
        "GetObjectInteractive" => ok(b(h.object(obj(a, 0)).is_some_and(|o| o.interactive))),
        "GetDistanceBetweenLocations" => {
            let loc = |i: usize| match h.sim.world.handle(handle(a, i)) {
                Some(EngineData::Location(l)) => Some(*l),
                _ => None,
            };
            ok(Value::Float(match (loc(0), loc(1)) {
                (Some(x), Some(y)) if x.area == y.area => dist(x.position, y.position),
                _ => -1.0,
            }))
        }
        // A message box: it waits for the player's answer (EVENT_TYPE_POPUP_RESULT to its
        // owner, see Sim::answer_popup).
        // The training tutorials (see training.rs).
        "SetTrainingMode" => {
            h.sim.set_training_mode(int(a, 0));
            void()
        }
        "BeginTrainingStep" => {
            h.sim.begin_training_step(int(a, 0), int(a, 1), int(a, 2));
            void()
        }
        "ShowPopup" => {
            h.sim.world.popup = Some(crate::world::Popup {
                strref: int(a, 0),
                kind: int(a, 1),
                owner: obj(a, 2),
                input: int(a, 3) != 0,
            });
            void()
        }
        // --- Stores (see store.rs) ----------------------------------------------------------
        "OpenStore" => {
            let st = obj(a, 0);
            h.sim.world.store = h.object(st).map(|_| st);
            void()
        }
        // A container's inventory opens for looting (see container.rs).
        "OpenInventory" => {
            h.sim.open_container(obj(a, 0));
            void()
        }
        "GetItemValue" => ok(Value::Int(h.sim.item_value(obj(a, 0)))),
        // A quickbar slot: an ability and the tag of the item it comes from; no ability
        // empties it.
        "SetQuickslot" => {
            h.sim
                .set_quickslot(obj(a, 0), int(a, 1), int(a, 2), &string(a, 3));
            void()
        }
        "GetQuickslot" => {
            let (who, slot) = (obj(a, 0), int(a, 1));
            ok(Value::Int(
                h.sim
                    .quickbar(who)
                    .into_iter()
                    .find(|(s, _)| *s == slot)
                    .map_or(0, |(_, q)| q.ability),
            ))
        }
        "GetStoreMoney" => ok(Value::Int(store_field(h, obj(a, 0), "StoreMoney", -1))),
        "SetStoreMoney" => set_store_field(h, obj(a, 0), "StoreMoney", int(a, 1)),
        "GetStoreMaxBuyPrice" => ok(Value::Int(store_field(h, obj(a, 0), "MaxBuyPrice", -1))),
        "SetStoreMaxBuyPrice" => set_store_field(h, obj(a, 0), "MaxBuyPrice", int(a, 1)),
        "GetStoreMarkUp" => ok(Value::Int(store_field(h, obj(a, 0), "MarkUp", 100))),
        "SetStoreMarkUp" => set_store_field(h, obj(a, 0), "MarkUp", int(a, 1)),
        "GetStoreMarkDown" => ok(Value::Int(store_field(h, obj(a, 0), "MarkDown", 100))),
        "SetStoreMarkDown" => set_store_field(h, obj(a, 0), "MarkDown", int(a, 1)),
        // The party's inventory size, kept on the hero (backpacks raise it). The starting
        // size is provisional.
        "GetMaxInventorySize" => {
            let who = match obj(a, 0) {
                OBJECT_INVALID => h.sim.world.hero.unwrap_or(OBJECT_INVALID),
                o => o,
            };
            ok(Value::Int(
                h.object(who)
                    .and_then(|o| o.fields.get("MaxInventorySize").copied())
                    .unwrap_or(DEFAULT_INVENTORY_SIZE),
            ))
        }
        "SetMaxInventorySize" => {
            let who = match obj(a, 1) {
                OBJECT_INVALID => h.sim.world.hero.unwrap_or(OBJECT_INVALID),
                o => o,
            };
            if let Some(o) = h.sim.world.get_mut(who) {
                o.fields.insert("MaxInventorySize".into(), int(a, 0));
            }
            void()
        }
        "HasConversation" => ok(b(h.object(obj(a, 0)).is_some_and(|o| o.dialog.is_some()))),
        // The state set by SetPlaceableState or an action's result, else the template's
        // starting state (`AnimContState`).
        "GetPlaceableState" => ok(Value::Int(
            h.object(obj(a, 0))
                .and_then(|o| {
                    o.fields
                        .get("State")
                        .or_else(|| o.fields.get("AnimContState"))
                        .copied()
                })
                .unwrap_or(0),
        )),
        "SetNumTactics" => {
            if let Some(o) = h.sim.world.get_mut(obj(a, 0)) {
                o.fields.insert("NumTactics".into(), int(a, 1));
            }
            void()
        }
        // DAOrigins.exe (0x66d580): the length of the creature's tactics list while its
        // tactics are on (the tactics screen's `Enabled`), else 0.
        "GetNumTactics" => {
            let who = obj(a, 0);
            ok(Value::Int(if h.sim.tactics_enabled(who) {
                h.object(who)
                    .and_then(|o| o.fields.get("NumTactics").copied())
                    .unwrap_or(0)
            } else {
                0
            }))
        }
        "SetPlaceableState" | "SetPlotGiver" | "SetAutoLevelUp" | "SetFollowerApprovalEnabled" => {
            let field = match name {
                "SetPlaceableState" => "State",
                "SetPlotGiver" => "PlotGiver",
                "SetAutoLevelUp" => "AutoLevelUp",
                _ => "ApprovalEnabled",
            };
            if let Some(o) = h.sim.world.get_mut(obj(a, 0)) {
                o.fields.insert(field.to_string(), int(a, 1));
            }
            void()
        }
        "IsAbilityAvailable" => ok(b(h
            .object(obj(a, 0))
            .is_some_and(|o| o.abilities.contains(&int(a, 1))))),
        "LoadItemsFromTemplate" => {
            h.sim
                .load_items_from_template(obj(a, 0), &string(a, 1), int(a, 2) != 0);
            void()
        }
        // DAOrigins.exe (0x666da0, 0x643bf0) adds an event with an id above 0 to the save's
        // "Story So Far" list, with the time. Provisional: the time kept is game time (the
        // engine records the play time and the wall clock).
        "LogStoryEvent" => {
            if int(a, 0) > 0 {
                h.sim.world.story_events.push((int(a, 0), t));
            }
            void()
        }
        "SetStoryPlot" => {
            // State the simulation does not use yet; recorded for the trace.
            h.sim.world.trace.push(TraceEntry::Log {
                t,
                channel: -3,
                text: format!("{name}{:?}", a),
            });
            void()
        }

        // --- Combat and perception (see `combat`) ------------------------------------------
        "GetCombatState" => ok(b(h.object(obj(a, 0)).is_some_and(|o| o.combat.in_combat))),
        "SetCombatState" => {
            if let Some(o) = h.sim.world.get_mut(obj(a, 0)) {
                o.combat.in_combat = int(a, 1) != 0;
            }
            void()
        }
        "GetAttackTarget" => ok(Value::Object(
            h.object(obj(a, 0))
                .and_then(|o| o.commands.first())
                .filter(|c| c.name == "CommandAttack")
                .and_then(|c| c.target)
                .unwrap_or(OBJECT_INVALID),
        )),
        "IsObjectHostile" => ok(b(h.sim.hostile(obj(a, 0), obj(a, 1)))),
        "IsPartyPerceivingHostiles" => {
            let party = crate::engine::party_of(h.sim);
            let any = party.iter().any(|&m| {
                h.object(m)
                    .is_some_and(|o| o.combat.perceived.iter().any(|&p| h.sim.hostile(m, p)))
            });
            ok(b(any))
        }
        "GetPerceivedCreatureList" => {
            let who = obj(a, 0);
            let hostile_only = int(a, 1) != 0;
            let list: Vec<ObjectId> = h
                .object(who)
                .map(|o| o.combat.perceived.clone())
                .unwrap_or_default()
                .into_iter()
                .filter(|&p| !hostile_only || h.sim.hostile(who, p))
                .collect();
            ok(objects(list))
        }
        "TriggerPerception" => {
            // DAOrigins.exe (0x65a7b0 -> 0x6b32a0 -> 0x6a2bb0) adds the creature only when both
            // are alive and in the same area, within 60 m (0xb03bf8), with no sight test.
            // Provisional: the room-visibility check is not modelled.
            let (who, seen) = (obj(a, 0), obj(a, 1));
            let reach = match (h.object(who), h.object(seen)) {
                (Some(x), Some(y)) => {
                    x.area.is_some()
                        && x.area == y.area
                        && dist(x.position, y.position) <= crate::combat::VISUAL_RANGE
                }
                _ => false,
            };
            let new = reach
                && h.sim.alive(who)
                && h.sim.alive(seen)
                && h.object(who)
                    .is_some_and(|o| !o.combat.perceived.contains(&seen));
            if new {
                if let Some(o) = h.sim.world.get_mut(who) {
                    o.combat.perceived.push(seen);
                }
                h.sim.perceive(who, seen, true);
            }
            ok(b(new))
        }
        "ClearPerceptionList" => {
            if let Some(o) = h.sim.world.get_mut(obj(a, 0)) {
                o.combat.perceived.clear();
            }
            void()
        }
        "UpdateThreatTable" => {
            if let Some(o) = h.sim.world.get_mut(obj(a, 0)) {
                crate::threat::update(&mut o.combat.threat, obj(a, 1), float(a, 2));
            }
            void()
        }
        "ClearThreatTable" => {
            if let Some(o) = h.sim.world.get_mut(obj(a, 0)) {
                o.combat.threat.clear();
            }
            void()
        }
        "IsPerceivingHostiles" => {
            let who = obj(a, 0);
            let any = h
                .object(who)
                .is_some_and(|o| o.combat.perceived.iter().any(|&p| h.sim.hostile(who, p)));
            ok(b(any))
        }
        // Loot bags and achievements have no simulation state.
        // The bag a creature's loot was left in (DAOrigins.exe 0x65f3d0 reads the creature's
        // +0x790, which SpawnBodyBag sets, 0x6a9302); OBJECT_INVALID for none or what is
        // not a creature.
        "GetCreatureBodyBag" => ok(Value::Object(
            h.object(obj(a, 0))
                .filter(|o| o.kind == ObjectKind::Creature)
                .and_then(|o| o.fields.get(crate::inventory::BODY_BAG_FIELD))
                .map_or(OBJECT_INVALID, |&b| b as ObjectId),
        )),
        // DAOrigins.exe (0x65d0c0, 0x67c230) switches the current area's sound objects with
        // that tag on or off (0x7c2190). The comparison at 0x67c2a4 calls 0x4c0de0 ->
        // 0x414170 -> 0x413800: an exact, case-sensitive UTF-16 comparison. The app's
        // audio snapshot already carries these objects' active flags to the mixer.
        "ActivateSoundByTag" => {
            let (tag, on) = (string(a, 0), int(a, 1) != 0);
            let area = h.sim.world.area;
            for o in h.sim.world.objects.values_mut() {
                if o.kind == ObjectKind::Sound && o.area.is_some() && o.area == area && o.tag == tag
                {
                    o.active = on;
                }
            }
            void()
        }
        // The loading screen's hint for the next area-list transition: a row of `n2DAReference`
        // (206 "Load Hints" by default, or 271 "LoadHintsRandom"), cleared once that
        // transition takes it (see `World::load_hint`). Provisional: when the exe clears it
        // is taken from the movie's behaviour, not traced.
        "SetLoadHint" => {
            let table = if a.len() > 1 { int(a, 1) } else { 206 };
            h.sim.world.load_hint = Some((int(a, 0), table));
            void()
        }
        "SpawnBodyBag" => {
            let c = obj(a, 0);
            h.sim.spawn_body_bag(c, int(a, 1) != 0);
            // A creature whose appearance decays at once (DecayBehaviour 3: shades, rage
            // demons) starts fading a second later (DAOrigins.exe 0x6a90d0).
            if h.sim
                .world
                .get(c)
                .is_some_and(|o| o.kind == ObjectKind::Creature)
                && h.sim.decay_behaviour(c) == 3
            {
                h.sim.force_decay(c, 1_000);
            }
            void()
        }
        // DAOrigins.exe 0x66be50 -> 0x69f360: the bag's creature fades after that long.
        "SetBodybagDecayDelay" => {
            h.sim
                .set_bodybag_decay_delay(obj(a, 0), int(a, 1).max(0) as u64);
            void()
        }
        // DAOrigins.exe 0x665630: the global time before a corpse is gone.
        "SetCreaturesGlobalMaxTimeBeforeDecay" => {
            h.sim.world.corpse_decay_max_ms = int(a, 0).max(0) as u64;
            void()
        }
        // No occlusion is simulated: two objects in the same area can see each other.
        // The same test as perception's (walls of the pathing grid and closed doors).
        "CheckLineOfSightObject" => {
            let seen = match (h.object(obj(a, 0)), h.object(obj(a, 1))) {
                (Some(x), Some(y)) if x.area == y.area => Some((x.area, x.position, y.position)),
                _ => None,
            };
            ok(b(match seen {
                Some((Some(area), p, q)) => h.sim.line_of_sight(area, p, q),
                Some((None, ..)) => true,
                None => false,
            }))
        }
        "CountItemsByTag" => {
            let tag = string(a, 1);
            let origins = h.sim.game == thedas_vfs::Game::Origins;
            // Origins 0x666210 -> 0x7615f0: exact tag match, adding stack counts only
            // for stackable items excluding default ammunition (0x753c60). DA2 is untraced.
            let n = h
                .object(obj(a, 0))
                .map(|o| o.inventory.clone())
                .unwrap_or_default()
                .into_iter()
                .filter_map(|i| {
                    let x = h.object(i)?;
                    if !origins {
                        return x.tag.eq_ignore_ascii_case(&tag).then_some(1_i32);
                    }
                    (x.kind == ObjectKind::Item && x.tag == tag).then(|| {
                        if h.sim.max_stack(i) > 1
                            && x.fields.get("DefaultAmmo").copied().unwrap_or(0) == 0
                        {
                            h.sim.stack_count(i)
                        } else {
                            1
                        }
                    })
                })
                .fold(0_i32, i32::wrapping_add);
            ok(Value::Int(n))
        }
        // DAOrigins.exe's SetCanLevelUp (0x662ea0 -> 0x6aca10) ignores its value: whether a
        // creature can level up is whether it has points to spend (attribute, skill and
        // talent points, properties 34-36).
        // The explore camera's zoom (0x666790 sets its target to 2z - 1, which its update
        // eases the zoom to): kept for the presentation, which applies each request once.
        "SetZoomLevel" => {
            let level = float(a, 0);
            let level = if level.is_finite() {
                level.clamp(0.0, 1.0)
            } else {
                1.0
            };
            let seq = h.sim.world.zoom_request.map_or(0, |r| r.1).wrapping_add(1);
            h.sim.world.zoom_request = Some((level, seq));
            void()
        }
        "SetCanLevelUp" => {
            let who = obj(a, 0);
            let points: f32 = [34, 35, 36]
                .iter()
                .map(|&p| h.sim.property_total(who, p))
                .sum();
            if let Some(o) = h.sim.world.get_mut(who) {
                o.fields.insert("CanLevelUp".into(), (points > 0.0) as i32);
            }
            void()
        }
        "SetAILevel" => {
            if let Some(o) = h.sim.world.get_mut(obj(a, 0)) {
                o.fields.insert("AILevel".into(), int(a, 1));
            }
            void()
        }
        "GetAutoLevelUp" => ok(Value::Int(
            h.object(obj(a, 0))
                .and_then(|o| o.fields.get("AutoLevelUp").copied())
                .unwrap_or(0),
        )),
        "GetCanLevelUp" => ok(Value::Int(
            h.object(obj(a, 0))
                .and_then(|o| o.fields.get("CanLevelUp").copied())
                .unwrap_or(0),
        )),
        "IsEffectValid" => ok(b(matches!(
            h.sim.world.handle(handle(a, 0)),
            Some(EngineData::Effect(e)) if e.ty != 0
        ))),
        // EffectModifyProperty is EFFECT_TYPE_MODIFY_PROPERTY (1012 in effects.gda):
        // integers are the property ids, floats their changes (provisional layout).
        // The hostile constructor makes the same effect, marked as a debuff. Provisional:
        // what else the executable does with the mark is not known.
        "EffectModifyProperty" | "EffectModifyPropertyHostile" => {
            let e = Effect {
                ty: 1012,
                creator: Some(caller),
                ints: vec![int(a, 0), int(a, 2), int(a, 4)],
                floats: vec![float(a, 1), float(a, 3), float(a, 5)],
                hostile: name == "EffectModifyPropertyHostile",
                ..Default::default()
            };
            let hd = h.sim.world.store(EngineData::Effect(e));
            ok(Value::Engine {
                ty: ty.effect,
                handle: Some(hd),
            })
        }
        // The death screen's hint: a row of `n2DAReference` (205 "Death Hints" by default,
        // 272 for the random hints), shown the next time the death screen is up (script.ldf).
        // Provisional: script.ldf says it is cleared when the death screen is hidden; here it
        // stays until another is set (a load replaces the world's).
        "SetDeathHint" => {
            let table = if a.len() > 1 { int(a, 1) } else { 205 };
            h.sim.world.death_hint = Some((int(a, 0), table));
            void()
        }
        "IsItemDroppable" => ok(b(h
            .object(obj(a, 0))
            .is_some_and(|o| o.fields.get("Droppable").copied().unwrap_or(0) != 0))),
        "SetItemDroppable" => {
            if let Some(o) = h.sim.world.get_mut(obj(a, 0)) {
                o.fields.insert("Droppable".into(), int(a, 1));
            }
            void()
        }
        "UnlockAchievementByID" | "UnlockAchievement" | "IncrementAchievementCountByID" => {
            h.sim.world.trace.push(TraceEntry::Log {
                t,
                channel: -3,
                text: format!("{name}{:?} (no effect in the simulation)", a),
            });
            void()
        }
        "ClearEnemyThreat" => {
            let enemy = obj(a, 1);
            if let Some(o) = h.sim.world.get_mut(obj(a, 0)) {
                o.combat.threat.retain(|t| t.enemy != enemy);
            }
            void()
        }
        "GetThreatTableSize" => ok(Value::Int(
            h.object(obj(a, 0))
                .map_or(0, |o| o.combat.threat.len() as i32),
        )),
        "GetThreatEnemy" => ok(Value::Object(
            h.object(obj(a, 0))
                .and_then(|o| o.combat.threat.get(int(a, 1).max(0) as usize))
                .map_or(OBJECT_INVALID, |t| t.enemy),
        )),
        "GetThreatValueByIndex" => ok(Value::Float(
            h.object(obj(a, 0))
                .and_then(|o| o.combat.threat.get(int(a, 1).max(0) as usize))
                .map_or(0.0, |t| t.total()),
        )),
        "GetThreatValueByObjectID" => {
            let enemy = obj(a, 1);
            ok(Value::Float(
                h.object(obj(a, 0))
                    .and_then(|o| o.combat.threat.iter().find(|t| t.enemy == enemy))
                    .map_or(0.0, |t| t.total()),
            ))
        }
        "SetAttackResult" => {
            let effect = |i: usize| match h.sim.world.handle(handle(a, i)) {
                Some(EngineData::Effect(e)) => Some(e.clone()),
                _ => None,
            };
            let result = crate::combat::AttackResult {
                results: [int(a, 1), int(a, 3)],
                effects: [effect(2), effect(4)],
            };
            if let Some(o) = h.sim.world.get_mut(obj(a, 0)) {
                o.combat.attack = Some(result);
            }
            void()
        }
        "SetAttackDuration" => {
            let ms = (float(a, 1).max(0.0) * 1000.0) as u64;
            if let Some(o) = h.sim.world.get_mut(obj(a, 0)) {
                o.combat.attack_ms = Some(ms);
            }
            void()
        }
        "GetAttackImpactDamageEffect" => {
            let i = int(a, 1).clamp(0, 1) as usize;
            let e = h
                .object(obj(a, 0))
                .and_then(|o| o.combat.attack.as_ref())
                .and_then(|r| r.effects[i].clone())
                .unwrap_or_default();
            let hd = h.sim.world.store(EngineData::Effect(e));
            ok(Value::Engine {
                ty: ty.effect,
                handle: Some(hd),
            })
        }
        "SetCommandResult" => {
            // A failed command is dropped; the owner hears COMMAND_COMPLETE with the result.
            let (who, result) = (obj(a, 0), int(a, 1));
            let failed = result == constant(&h.sim.table, "COMMAND_RESULT_FAILED");
            if let Some(o) = h.sim.world.get_mut(who) {
                o.fields.insert("CommandResult".into(), result);
                if failed && !o.commands.is_empty() {
                    let c = o.commands.remove(0);
                    let mut ev = h.sim.new_event("COMMAND_COMPLETE", Some(who));
                    ev.ints = vec![c.ty, result];
                    h.sim.world.queue_event(0, who, ev, None);
                }
            }
            void()
        }
        "HasDeathEffect" => ok(b(h.object(obj(a, 0)).is_some_and(|o| o.dead))),
        // Damage effects. Provisional: the types are the ids in effects.gda
        // (EFFECT_TYPE_DAMAGE 1013, EFFECT_TYPE_IMPACT 1024); the value layout is how the
        // rules scripts read them back. Float 0 is the damage. A damage effect's integers are
        // the damage type, flags and visual effect; an impact's are its arguments in order,
        // visual effect, ability and damage type (rules_core's attack impact and
        // talent_singletarget read integer 2 as the damage type), and object 0 is the weapon.
        "EffectDamage" | "EffectImpact" => {
            let e = if name == "EffectDamage" {
                Effect {
                    ty: 1013,
                    creator: Some(caller),
                    floats: vec![float(a, 0)],
                    ints: vec![int(a, 1), int(a, 2), int(a, 3)],
                    ..Default::default()
                }
            } else {
                Effect {
                    ty: 1024,
                    creator: Some(caller),
                    floats: vec![float(a, 0)],
                    objects: vec![obj(a, 1)],
                    ints: vec![int(a, 2), int(a, 3), int(a, 4)],
                    ability: int(a, 3),
                    ..Default::default()
                }
            };
            let hd = h.sim.world.store(EngineData::Effect(e));
            ok(Value::Engine {
                ty: ty.effect,
                handle: Some(hd),
            })
        }
        // Items: stats come from the item's base type in the rules 2DAs, which are not
        // read yet; scripts see 0 and fall back on attributes.
        // GetItemStat: the item's base-type row in itemstats.gda, in the column named by
        // the stat's label in itemstatdefines.gda (Attack, Damage, ArmorPenetration, ...).
        // Provisional: item properties (material, enchantments) are not added yet.
        "GetItemStat" => ok(Value::Float(h.sim.item_stat(obj(a, 0), int(a, 1)))),
        // DAOrigins.exe (0x665570, 0x7556d0) adds up the power of every one of the item's
        // properties with that id, then, with bIncludeSubItems, its sub-items' (runes) the
        // same way; 0 for what is not an item. Provisional: items hold no sub-items.
        "GetItemPropertyPower" => {
            let (item, p) = (obj(a, 0), int(a, 1));
            let power = if h.object(item).is_some_and(|o| o.kind == ObjectKind::Item) {
                h.sim
                    .item_properties(item)
                    .iter()
                    .filter(|(id, _)| *id == p)
                    .map(|(_, power)| *power)
                    .sum()
            } else {
                0
            };
            ok(Value::Int(power))
        }
        // The item's property ids, in order (DAOrigins.exe 0x674260). Provisional: the
        // order and, with bIncludeSubItems, sub-items' (items hold none yet).
        "GetItemProperties" => ok(Value::Array {
            elem: Type::Int,
            items: if h
                .object(obj(a, 0))
                .is_some_and(|o| o.kind == ObjectKind::Item)
            {
                h.sim
                    .item_properties(obj(a, 0))
                    .into_iter()
                    .map(|(id, _)| Value::Int(id))
                    .collect()
            } else {
                Vec::new()
            },
        }),
        "AddItemProperty" => {
            h.sim.add_item_property(obj(a, 0), int(a, 1), int(a, 2));
            void()
        }
        "RemoveItemProperty" => {
            h.sim.remove_item_property(obj(a, 0), int(a, 1));
            void()
        }

        // DragonAge2.exe commands 425–427; see crate::encounter.
        "GetEncounterId" => ok(Value::Int(h.sim.da2_encounter_id(obj(a, 0)))),
        "GetEncounter" => ok(objects(h.sim.da2_encounter(int(a, 0)))),
        "SetEncounterId" => {
            h.sim.set_da2_encounter_id(obj(a, 0), int(a, 1));
            void()
        }
        // Dragon Age II's additions (none is in DA:O's script.ldf). Provisional: read from
        // their script.ldf declarations and the fields DA2's templates carry, not traced in
        // DragonAge2.exe.
        "GetItemLevel"
        | "GetItemVariation"
        | "GetPlaceableStateController"
        | "GetCreatureAmbientActivityPattern"
        | "GetCreatureAmbientMovementPattern" => {
            let field = match name {
                "GetItemLevel" => "Level",
                "GetItemVariation" => "ModelVariation",
                // A placeable template's state controller: a row of
                // placeable_statecontrollers.gda (4 informational, 2 area transition, ...).
                "GetPlaceableStateController" => "StateController",
                "GetCreatureAmbientActivityPattern" => DA2_AMBIENT_ACTIVITY,
                _ => DA2_AMBIENT_MOVEMENT,
            };
            ok(Value::Int(
                h.object(obj(a, 0))
                    .and_then(|o| o.fields.get(field).copied())
                    .unwrap_or(0),
            ))
        }
        "SetItemLevel"
        | "SetCreatureAmbientActivityPattern"
        | "SetCreatureAmbientMovementPattern" => {
            let field = match name {
                "SetItemLevel" => "Level",
                "SetCreatureAmbientActivityPattern" => DA2_AMBIENT_ACTIVITY,
                _ => DA2_AMBIENT_MOVEMENT,
            };
            if let Some(o) = h.sim.world.get_mut(obj(a, 0)) {
                o.fields.insert(field.to_string(), int(a, 1));
            }
            void()
        }
        "min" => ok(Value::Int(int(a, 0).min(int(a, 1)))),
        "max" => ok(Value::Int(int(a, 0).max(int(a, 1)))),
        "minf" => ok(Value::Float(float(a, 0).min(float(a, 1)))),
        "maxf" => ok(Value::Float(float(a, 0).max(float(a, 1)))),
        // Dragon Age II: whether `nInt` has every bit of `nFlag`. Provisional: from its
        // script.ldf name and its callers' single-bit flags.
        "hasflag" => ok(b((int(a, 0) & int(a, 1)) == int(a, 1))),
        // Dragon Age II presentation calls (a sound parameter, blood decals on the ground):
        // nothing in the simulation changes.
        "SetAudioGameParameter" | "SpawnRandomDecals" => void(),
        // Dragon Age II: the creature's head morph (`.mor`, named without its extension), as
        // character creation sets the hero's (`hm_iconicmale` for the default Hawke, after
        // clearing it with ""). An empty name leaves the template's. Provisional: from
        // DA2's script.ldf declaration and sys_chargen's calls.
        "SetHeadMorphName" => {
            let name = string(a, 1);
            let name = name.trim().trim_end_matches(".mor").to_ascii_lowercase();
            if let Some(o) = h.sim.world.get_mut(obj(a, 0)) {
                o.head_morph = (!name.is_empty()).then_some(name);
            }
            void()
        }
        // The player's profile unlocks, the cross-game vault and the GUI attribute tree are
        // not kept: none is set.
        "GetGlobalUnlock" | "GetVaultInt" | "GetAttributeBool" => ok(Value::Int(0)),
        // Presentation only.
        "PreloadCreatureForConversation"
        | "SetGUIElementEnabled"
        | "SetPartyMailArea"
        | "SetNextLoadScreenMovie"
        | "ShowHelmetInPortrait"
        | "SetAudioGlobalGameParameter" => void(),
        "GetItemStackSize" => ok(Value::Int(
            h.object(obj(a, 0))
                .and_then(|o| o.fields.get("StackSize").copied())
                .unwrap_or(1)
                .max(1),
        )),
        // Ignored for an item that does not stack; clamped to the stack's maximum; an item
        // set to 0 is destroyed (0x66a8a0).
        "SetItemStackSize" => {
            let item = obj(a, 0);
            let max = h.sim.max_stack(item);
            if max > 1 && h.object(item).is_some() {
                let n = int(a, 1).clamp(0, max);
                if n == 0 {
                    let holders: Vec<ObjectId> = h
                        .sim
                        .world
                        .objects
                        .values()
                        .filter(|o| o.inventory.contains(&item))
                        .map(|o| o.id)
                        .collect();
                    for holder in holders {
                        h.sim.unequip(holder, item);
                        if let Some(o) = h.sim.world.get_mut(holder) {
                            o.inventory.retain(|&i| i != item);
                        }
                    }
                    h.sim.world.objects.remove(&item);
                } else if let Some(o) = h.sim.world.get_mut(item) {
                    o.fields.insert("StackSize".into(), n);
                }
            }
            void()
        }
        // ApplyEffectModifyProperty: the effect's integers are property ids and its floats
        // the changes (EffectModifyProperty's layout); applied to the current effect's owner.
        "ApplyEffectModifyProperty" | "RemoveEffectModifyProperty" => {
            let e = match h.sim.world.handle(handle(a, 0)) {
                Some(EngineData::Effect(e)) => e.clone(),
                _ => return void(),
            };
            let sign = if name == "ApplyEffectModifyProperty" {
                1.0
            } else {
                -1.0
            };
            // The changes are modifiers, kept apart from the base (PROPERTY_VALUE_MODIFIER).
            let target = h.sim.current_effect_target.unwrap_or(caller);
            if let Some(o) = h.sim.world.get_mut(target) {
                for (p, d) in e.ints.iter().zip(e.floats.iter()) {
                    if *p != 0 {
                        let v = o.modifiers.entry(*p).or_insert(0.0);
                        *v += sign * d;
                        if v.abs() < 1e-4 {
                            o.modifiers.remove(p);
                        }
                    }
                }
            }
            void()
        }
        // An item's material (materialtypes.gda row), which scales its stats (see
        // `Sim::item_stat`).
        "GetItemMaterialType" => ok(Value::Int(
            h.object(obj(a, 0))
                .and_then(|o| o.fields.get("Material").copied())
                .unwrap_or(0),
        )),
        "SetItemMaterialType" => {
            if let Some(o) = h.sim.world.get_mut(obj(a, 0)) {
                o.fields.insert("Material".into(), int(a, 1));
            }
            void()
        }
        "ScaleEquippedItems" => {
            h.sim.scale_equipped_items(obj(a, 0), int(a, 1));
            void()
        }
        "GetItemUnique" | "GetItemMaterialProgression" => {
            let field = if name == "GetItemUnique" {
                "Unique"
            } else {
                "MaterialProgress"
            };
            ok(Value::Int(
                h.object(obj(a, 0))
                    .and_then(|o| o.fields.get(field).copied())
                    .unwrap_or(0),
            ))
        }
        "SetAimLoopDuration" => {
            let ms = (float(a, 1).max(0.0) * 1000.0) as u64;
            if let Some(o) = h.sim.world.get_mut(obj(a, 0)) {
                o.combat.aim_ms = Some(ms);
            }
            void()
        }
        // Tactics and area-of-effect abilities are not simulated yet.
        // The creatures inside an area of effect (DAOrigins.exe 0x672cc0, 0x7e5e60: the
        // creatures of its area that are its members). See [`crate::aoe`].
        "GetCreaturesInAOE" => ok(objects(h.sim.aoe_members(obj(a, 0)))),
        // Membership of the creature (DAOrigins.exe 0x66e040, 0x6ae2b0).
        "IsInAOE" => ok(b(h
            .object(obj(a, 1))
            .and_then(|o| o.aoe.as_ref())
            .is_some_and(|s| s.members.contains(&obj(a, 0))))),
        "GetAbilitiesDueToAOEs" => ok(Value::Array {
            elem: Type::Int,
            items: Vec::new(),
        }),
        "GetItemEquipSlot" => {
            let item = obj(a, 0);
            let origins = h.sim.game == thedas_vfs::Game::Origins;
            let invalid = constant(&h.sim.table, "INVENTORY_SLOT_INVALID");
            if origins && !h.object(item).is_some_and(|i| i.kind == ObjectKind::Item) {
                return ok(Value::Int(invalid));
            }
            // Origins 0x665410 -> 0x797c80: an item's creature owner searches the
            // active slots, then every weapon set. The returned slot omits the set id.
            let slot = h
                .sim
                .world
                .objects
                .values()
                .filter(|o| !origins || o.kind == ObjectKind::Creature)
                .find_map(|o| {
                    o.equipment
                        .iter()
                        .chain(o.other_set.iter().filter(|_| origins))
                        .find(|(_, i)| **i == item)
                        .map(|(s, _)| *s)
                });
            ok(Value::Int(slot.unwrap_or(invalid)))
        }
        "GetWeaponStyle" => ok(Value::Int(h.sim.weapon_style(obj(a, 0)))),
        // Followers fight on their own; the member the player controls is the player's.
        "IsPartyAIEnabled" => ok(b(
            h.sim.world.autopilot || Some(obj(a, 0)) != h.sim.world.controlled()
        )),
        // The creatures on a creature's melee ring: within reach of it (the two bodies and
        // the melee gap, see `Sim::reach_to`) and between the two angles, measured from its
        // facing (0 to 360 or -180 to 180). Provisional geometry; only the main ring (0) is
        // kept.
        "GetCreaturesInMeleeRing" => {
            let who = obj(a, 0);
            let (start, end, hostile_only) = (float(a, 1), float(a, 2), int(a, 3) != 0);
            let Some((pos, area, facing)) = h.object(who).map(|o| (o.position, o.area, o.facing))
            else {
                return ok(objects(Vec::new()));
            };
            let mine = h.sim.body_radius(who);
            let list: Vec<ObjectId> = h
                .sim
                .world
                .objects
                .values()
                .filter(|o| {
                    o.id != who
                        && o.kind == ObjectKind::Creature
                        && !o.dead
                        && o.active
                        && o.area == area
                        && dist(o.position, pos)
                            <= mine + h.sim.body_radius(o.id) + crate::combat::MELEE_GAP + 0.25
                })
                .filter(|o| {
                    let ang =
                        crate::world::facing_toward(o.position[0] - pos[0], o.position[1] - pos[1])
                            - facing;
                    let mut rel = ang.rem_euclid(360.0);
                    if start < 0.0 && rel > 180.0 {
                        rel -= 360.0;
                    }
                    end - start >= 360.0 || (start <= rel && rel <= end)
                })
                .filter(|o| !hostile_only || h.sim.hostile(who, o.id))
                .map(|o| o.id)
                .collect();
            ok(objects(list))
        }
        // Origins formats the raw id as signed decimal, without resolving it (0x64f890).
        "ObjectToString" if h.sim.game == thedas_vfs::Game::Origins => {
            ok(Value::String((obj(a, 0) as i32).to_string()))
        }
        "ObjectToString" => ok(Value::String(obj(a, 0).to_string())),
        "GetNameStrref" => ok(Value::Int(
            h.object(obj(a, 0))
                .and_then(|o| o.name_strref)
                .map_or(0, |s| s as i32),
        )),
        "GetStealthEnabled" => ok(Value::Int(
            h.object(obj(a, 0)).is_some_and(|o| o.combat.stealth) as i32,
        )),
        "SetStealthEnabled" => {
            if let Some(o) = h.sim.world.get_mut(obj(a, 0)) {
                o.combat.stealth = int(a, 1) != 0;
            }
            void()
        }
        // DAOrigins.exe 0x6645a0: the template's `NoPermDeath` is 0 and the creature is not
        // in the party (0x6316f0 on the party list). So the scripts that eat corpses (Death
        // Magic's heartbeat) leave a fallen party member alone.
        "GetCanDiePermanently" => {
            let id = obj(a, 0);
            let in_party = party_of(h.sim).contains(&id);
            ok(b(!in_party
                && h.object(id).is_some_and(|o| {
                    o.fields.get("NoPermDeath").copied().unwrap_or(0) == 0
                })))
        }
        // A placeable's treasure category and rank, as its instance sets them (`TreasureCategory`,
        // `Rank`). Provisional: the rank is taken to be the placed object's `Rank`.
        "GetPlaceableTreasureCategory" => ok(Value::Int(
            h.object(obj(a, 0))
                .and_then(|o| o.fields.get("TreasureCategory").copied())
                .unwrap_or(0),
        )),
        // 0x65f220 resolves a placeable and reads +0x6bc. That member is Rank:
        // binary field 0x3f7d at 0x74ff4b and named Rank at 0x75098e.
        "GetPlaceableTreasureRank" => ok(Value::Int(
            h.object(obj(a, 0))
                .filter(|o| o.kind == ObjectKind::Placeable)
                .map_or(0, |o| o.rank),
        )),
        "GetCreatureTreasureCategory" => ok(Value::Int(
            h.object(obj(a, 0))
                .and_then(|o| o.fields.get("TreasureCategory").copied())
                .unwrap_or(0),
        )),
        // One target's share of an area ability: DAOrigins.exe (0x65c580) queues event 1066
        // to the calling script's object, after a random 0-149 ms plus the base delay, with
        // the ability (int 0), caster, target and the location's area (objects 0-2), its
        // position (floats 0-2) and the running script's name (string 0), so that script
        // (spell_aoe_instant and the like) applies it.
        "SetIndividualImpactAOEEvent" => {
            let l = match h.sim.world.handle(handle(a, 3)) {
                Some(EngineData::Location(l)) => Some(*l),
                _ => None,
            };
            let mut ev = Event {
                ty: 1066,
                creator: Some(caller),
                ..Default::default()
            };
            ev.ints = vec![int(a, 2)];
            ev.objects = vec![obj(a, 0), obj(a, 1), l.map_or(OBJECT_INVALID, |l| l.area)];
            ev.floats = l.map_or(vec![0.0; 3], |l| l.position.to_vec());
            let script = h.script.trim_end_matches(".ncs").to_string();
            ev.strings = vec![script.clone()];
            let delay = h.sim.world.rng.below(150).max(0) as u64 + int(a, 4).max(0) as u64;
            h.sim.world.queue_event(delay, caller, ev, Some(script));
            void()
        }
        // Presentation only.
        "PlayAdditiveAnimation" | "SignalDamage" => void(),
        // Creature switches that DAOrigins.exe sets in one handler (0x66be50), kept on the
        // creature for what reads them: SetCreatureIsGhost (0x69f4a0: a ghost walks through
        // other creatures but around other ghosts; the simulation's movement does not yet
        // collide creatures), SetQuickslotBar (0x719ae0: the bar the quickbar shows, taken
        // only below 5) and LockQuickslotBar (0x719ac0: locked when bLock > 0).
        "SetCreatureIsGhost" | "SetQuickslotBar" | "LockQuickslotBar" => {
            let (who, v) = (obj(a, 0), int(a, 1));
            let field = match name {
                "SetCreatureIsGhost" => Some(("Ghost", (v != 0) as i32)),
                "SetQuickslotBar" => (v < 5).then_some(("QuickslotBar", v)),
                _ => Some(("QuickslotBarLocked", (v > 0) as i32)),
            };
            if let Some(o) = h
                .sim
                .world
                .get_mut(who)
                .filter(|o| o.kind == ObjectKind::Creature)
            {
                if let Some((f, v)) = field {
                    o.fields.insert(f.into(), v);
                }
            }
            void()
        }
        "SetCreatureIsStatue" => {
            if let Some(o) = h.sim.world.get_mut(obj(a, 0)) {
                o.fields.insert("Statue".into(), int(a, 1));
            }
            void()
        }
        "IsPerceiving" => ok(b(h
            .object(obj(a, 0))
            .is_some_and(|o| o.combat.perceived.contains(&obj(a, 1))))),
        "GetAngleBetweenObjects" => {
            let (x, y) = (h.object(obj(a, 0)), h.object(obj(a, 1)));
            ok(Value::Float(match (x, y) {
                (Some(x), Some(y)) => {
                    let d = [y.position[0] - x.position[0], y.position[1] - x.position[1]];
                    let to = crate::world::facing_toward(d[0], d[1]);
                    let mut a = (to - x.facing).rem_euclid(360.0);
                    if a > 180.0 {
                        a = 360.0 - a;
                    }
                    a
                }
                _ => 0.0,
            }))
        }
        "SetFacingObject" => {
            let target = h.object(obj(a, 1)).map(|o| o.position);
            if let (Some(t), Some(o)) = (target, h.sim.world.get_mut(obj(a, 0))) {
                o.facing = crate::world::facing_toward(t[0] - o.position[0], t[1] - o.position[1]);
            }
            void()
        }
        // Cooldowns are kept per ability and source item tag; an item's cooldown is shared by
        // every item with its tag. Provisional.
        "GetRemainingCooldown" => {
            let key = cooldown_key(int(a, 1), &string(a, 2));
            let now = h.sim.world.time_ms;
            ok(Value::Float(
                h.object(obj(a, 0))
                    .and_then(|o| o.combat.cooldowns.get(&key))
                    .map_or(0.0, |&until| until.saturating_sub(now) as f32 / 1000.0),
            ))
        }
        "SetCooldown" => {
            let key = cooldown_key(int(a, 1), &string(a, 3));
            let until = h.sim.world.time_ms + (float(a, 2).clamp(0.0, 1.0e6) * 1000.0) as u64;
            if let Some(o) = h.sim.world.get_mut(obj(a, 0)) {
                if until > o.combat.cooldowns.get(&key).copied().unwrap_or(0) {
                    o.combat.cooldowns.insert(key, until);
                }
            }
            void()
        }
        // Removes `count` from an item's stack, destroying it when none are left.
        "RemoveItem" => {
            // A count below 0 (−1 is the default; the scripts pass it 23 times) takes the
            // whole stack, as RemoveItemsByTag documents for its count.
            let (item, count) = (obj(a, 0), int(a, 1));
            let Some(left) = h
                .object(item)
                .filter(|o| o.kind == ObjectKind::Item)
                .map(|o| {
                    let stack = o.fields.get("StackSize").copied().unwrap_or(1).max(1);
                    if count < 0 { 0 } else { stack - count }
                })
            else {
                return void();
            };
            if left > 0 {
                if let Some(o) = h.sim.world.get_mut(item) {
                    o.fields.insert("StackSize".into(), left);
                }
            } else {
                let holders: Vec<ObjectId> = h
                    .sim
                    .world
                    .objects
                    .values()
                    .filter(|o| o.inventory.contains(&item))
                    .map(|o| o.id)
                    .collect();
                for holder in holders {
                    h.sim.unequip(holder, item);
                    if let Some(o) = h.sim.world.get_mut(holder) {
                        o.inventory.retain(|&i| i != item);
                    }
                }
                h.sim.world.objects.remove(&item);
            }
            void()
        }
        // Ability results only steer animations, which the simulation does not play.
        // Reports the target's resistance roll (made by the scripts themselves) and the node a
        // projectile aims at: presentation for the simulation.
        "SetAbilityResult" => {
            *h.sim
                .world
                .trace
                .presentation
                .entry(name.to_string())
                .or_default() += 1;
            void()
        }
        "GetShowSpecialMoveFloaties" => ok(Value::Int(0)),
        "GetEventLocation" => {
            let l = match h.sim.world.handle(handle(a, 0)) {
                Some(EngineData::Event(e)) => e.locations.get(int(a, 1).max(0) as usize).copied(),
                _ => None,
            };
            let l = l.unwrap_or(Location {
                area: OBJECT_INVALID,
                position: [0.0; 3],
                facing: 0.0,
            });
            let hd = h.sim.world.store(EngineData::Location(l));
            ok(Value::Engine {
                ty: ty.location,
                handle: Some(hd),
            })
        }
        "SetEventLocation" => {
            let mut ev = match h.sim.world.handle(handle(a, 0)) {
                Some(EngineData::Event(e)) => e.clone(),
                _ => Event::default(),
            };
            let i = int(a, 1).max(0) as usize;
            if let (true, Some(EngineData::Location(l))) =
                (i < 64, h.sim.world.handle(handle(a, 2)))
            {
                let l = *l;
                if ev.locations.len() <= i {
                    ev.locations.resize(
                        i + 1,
                        Location {
                            area: OBJECT_INVALID,
                            position: [0.0; 3],
                            facing: 0.0,
                        },
                    );
                }
                ev.locations[i] = l;
            }
            let hd = h.sim.world.store(EngineData::Event(ev));
            ok(Value::Engine {
                ty: ty.event,
                handle: Some(hd),
            })
        }
        // DAOrigins.exe (0x6614f0) returns the item tag string kept in the tactic entry
        // (index - 1, entry + 0x128). SetTacticEntry (0x6615f0) takes no tag and the preset
        // tables (tacticsuserpresets.gda's TacticsTable worksheets) have no item column, so
        // only the player's tactics screen gives one (`CommandItem`): the item's tag, or
        // empty.
        "GetTacticCommandItemTag" => {
            let (who, i) = (obj(a, 0), int(a, 1));
            let item = usize::try_from(i)
                .ok()
                .and_then(|i| h.sim.tactics_of(who).get(i).and_then(|t| t.item));
            ok(Value::String(
                item.and_then(|it| h.object(it))
                    .map(|o| o.tag.clone())
                    .unwrap_or_default(),
            ))
        }
        // The party member a slot's per-follower target or condition names (the tactics
        // screen's `TargetID` and `ConditionID`), else OBJECT_INVALID. Provisional: read
        // from the screen's writes, not traced in the executable.
        "GetTacticTargetObject" | "GetTacticConditionObject" => {
            let (who, i) = (obj(a, 0), int(a, 1));
            let slot = usize::try_from(i)
                .ok()
                .and_then(|i| h.sim.tactics_of(who).get(i).cloned());
            let o = slot.and_then(|t| match name {
                "GetTacticTargetObject" => t.target_object,
                _ => t.condition_object,
            });
            ok(Value::Object(o.unwrap_or(OBJECT_INVALID)))
        }
        // The player's "auto-pause when combat starts" option: DAOrigins.exe (0x65e520)
        // reads it from the game options, where it starts off (0x5e8d57) until the player's
        // settings (`AutoPauseCombat`) say otherwise.
        "GetAutoPauseCombatStatus" => ok(b(h.sim.auto_pause_combat)),
        "EnableWeaponTrail" => void(),
        _ => None,
    }
}

/// The hero and its followers.
pub fn party_of(sim: &Sim) -> Vec<ObjectId> {
    let hero = sim.world.hero;
    let mut v: Vec<ObjectId> = hero.into_iter().collect();
    if let Some(h) = hero.and_then(|h| sim.world.get(h)) {
        v.extend(h.followers.iter().copied());
    }
    v
}

pub fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// Point-in-polygon on the XY plane (even-odd rule).
/// Whether a point is inside a trigger. The outline is relative to the trigger's position
/// and rotated by its facing (degrees about Z).
pub fn in_trigger(p: [f32; 3], trigger: &crate::world::Object) -> bool {
    if trigger.geometry.len() < 3 {
        return false;
    }
    let (dx, dy) = (p[0] - trigger.position[0], p[1] - trigger.position[1]);
    let (s, c) = (-trigger.facing.to_radians()).dsin_cos();
    let local = [dx * c - dy * s, dx * s + dy * c, p[2]];
    inside(local, &trigger.geometry)
}

pub fn inside(p: [f32; 3], poly: &[[f32; 3]]) -> bool {
    let mut inside = false;
    let n = poly.len();
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + n - 1) % n]);
        if (a[1] > p[1]) != (b[1] > p[1])
            && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0]
        {
            inside = !inside;
        }
    }
    inside
}

/// The key `SetCooldown` and `GetRemainingCooldown` share: the ability and source item tag.
pub(crate) fn cooldown_key(ability: i32, item: &str) -> String {
    format!("{ability}:{}", item.to_ascii_lowercase())
}

/// The state a placeable moves to when `action` succeeds (`SetPlaceableActionResult`): its
/// state controller's row for the current state gives the default action, the state it
/// leads to and the state its variation leads to (`statecnt_door.gda`: Closed_Unlocked opens
/// to Open, or Open_2 as the variation; Closed_Locked unlocks to Closed_Unlocked; Open closes
/// to Closed_Unlocked). Provisional: only the default action is known to move the state;
/// other actions leave it.
fn placeable_transition(sim: &Sim, id: ObjectId, action: i32, variation: bool) -> Option<i32> {
    let o = sim.world.get(id)?;
    let appearance = *o.fields.get("Appearance")? as i64;
    let state = o
        .fields
        .get("State")
        .or_else(|| o.fields.get("AnimContState"))
        .copied()
        .unwrap_or(0) as i64;
    let tables = sim.world.tables();
    let types = tables.get(93, "")?;
    let col = types
        .columns
        .iter()
        .find(|(_, i)| **i == 8)
        .map(|(h, _)| *h)?;
    let controller = match types.cell_by_hash(col, appearance)? {
        Cell::String(s) => clean(s).to_ascii_lowercase(),
        _ => return None,
    };
    let t = tables.get(0, &controller)?;
    let int = |h: u32| match t.cell_by_hash(h, state) {
        Some(Cell::Int(v)) => Some(*v as i32),
        _ => None,
    };
    if int(0x52be_3c5e)? != action {
        return None;
    }
    let next = int(0xa2b4_782c)?;
    let alt = int(0x5ad0_780d).filter(|&v| v > 0);
    Some(if variation { alt.unwrap_or(next) } else { next })
}

/// A placeable's default action in its current state, from its type's state controller:
/// `placeable_types.gda` (the ninth column names the controller table) and the controller's
/// action column (hash 0x52be3c5e) at the row of the placeable's state. Provisional.
pub(crate) fn placeable_default_action(sim: &Sim, id: ObjectId) -> Option<i32> {
    let o = sim.world.get(id)?;
    let appearance = *o.fields.get("Appearance")? as i64;
    let state = o
        .fields
        .get("State")
        .or_else(|| o.fields.get("AnimContState"))
        .copied()
        .unwrap_or(0) as i64;
    let tables = sim.world.tables();
    let types = tables.get(93, "")?;
    let col = types
        .columns
        .iter()
        .find(|(_, i)| **i == 8)
        .map(|(h, _)| *h)?;
    let controller = match types.cell_by_hash(col, appearance)? {
        Cell::String(s) => clean(s).to_ascii_lowercase(),
        _ => return None,
    };
    if controller.is_empty() {
        return None;
    }
    let t = tables.get(0, &controller)?;
    match t.cell_by_hash(0x52be_3c5e, state)? {
        Cell::Int(v) if *v > 0 => Some(*v as i32),
        _ => None,
    }
}

impl Sim {
    /// `SetCreatureProperty` (or, with `update`, `UpdateCreatureProperty`): property
    /// `property` of creature `id` set to (or moved by) `value` as `value_type` says (base,
    /// total, current, modifier), within `properties.gda`'s bounds; a depletable's current
    /// clamped to its total, a hit's health loss recorded for `caller`, and the hero's
    /// experience shared with the followers waiting in the pool (`by`: the script doing it,
    /// for the trace).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn set_creature_property(
        &mut self,
        caller: ObjectId,
        by: String,
        id: ObjectId,
        p: i32,
        value: f32,
        vt: i32,
        update: bool,
    ) {
        let base_type = constant(&self.table, "PROPERTY_VALUE_BASE");
        let total_type = constant(&self.table, "PROPERTY_VALUE_TOTAL");
        let current = constant(&self.table, "PROPERTY_VALUE_CURRENT");
        let modifier = constant(&self.table, "PROPERTY_VALUE_MODIFIER");
        let kind = property_kind(self, p);
        // Health before, to record what a hit takes.
        let health = constant(&self.table, "PROPERTY_DEPLETABLE_HEALTH");
        let health_before = (p == health && vt == current)
            .then(|| {
                self.world
                    .get(id)
                    .filter(|o| !o.dead)
                    .and_then(|o| o.properties.get(&health))
                    .map(|v| v.1)
            })
            .flatten();
        // The hero's experience before, for followers waiting in the pool.
        let xp = constant(&self.table, "PROPERTY_SIMPLE_EXPERIENCE");
        let hero_xp_before = (p == xp && Some(id) == self.world.hero).then(|| {
            self.world
                .get(id)
                .and_then(|o| o.properties.get(&xp))
                .map_or(0.0, |v| v.0)
        });
        if (vt == base_type || vt == total_type) && !update {
            if let Some(o) = self.world.get_mut(id) {
                o.properties.entry(p).or_insert((0.0, 0.0)).0 = value;
            }
        } else if vt == current && kind == PropertyKind::Depletable {
            let max = self.property_total(id, p);
            if let Some(o) = self.world.get_mut(id) {
                let e = o.properties.entry(p).or_insert((0.0, 0.0));
                let v = if update { e.1 + value } else { value };
                e.1 = v.clamp(0.0, max.max(0.0));
            }
        } else if vt == modifier && kind != PropertyKind::Simple {
            let gear = self.equipment_bonus(id, p);
            if let Some(o) = self.world.get_mut(id) {
                let m = o.modifiers.entry(p).or_insert(0.0);
                *m = if update { *m + value } else { value - gear };
            }
        }
        if kind == PropertyKind::Depletable && vt != current {
            self.clamp_current(id, p);
        }
        // Scripts bring the dead back by giving them health (module_core's
        // HandleEvent_Resurrection applies no effect). Provisional.
        if let Some(o) = self.world.get_mut(id) {
            if p == health
                && vt == current
                && o.dead
                && o.properties.get(&health).is_some_and(|v| v.1 > 0.0)
            {
                o.dead = false;
                o.died_at = None;
                // One placed dead is no longer (DAOrigins.exe 0x6bc5fb-0x6bc631: health
                // rising above 0 sets a CREATURE_SPAWN_DEAD of 1 to 0).
                if let Some(v) = o.locals.ints.get_mut("CREATURE_SPAWN_DEAD")
                    && *v == 1
                {
                    *v = 0;
                }
            }
        }
        if let Some(before) = health_before {
            let after = self
                .world
                .get(id)
                .and_then(|o| o.properties.get(&health))
                .map_or(before, |v| v.1);
            self.record_health_loss(caller, id, before - after.max(0.0));
        }
        if let Some(before) = hero_xp_before {
            let after = self
                .world
                .get(id)
                .and_then(|o| o.properties.get(&xp))
                .map_or(0.0, |v| v.0);
            if after > before {
                *self.world.trace.hero_xp.entry(by).or_default() += (after - before) as u64;
            }
            self.share_pool_xp(after - before);
        }
    }
}

/// A property's type (as `GetCreaturePropertyType`), Min and Max from `properties.gda`.
/// A property's kind (`properties.gda`'s Type), which decides which of its values exist.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PropertyKind {
    Attribute,
    Simple,
    Depletable,
    Derived,
}

/// A property's kind; when the table does not say, Health and Mana_Stamina (7, 8) are
/// depletables and the rest attributes.
pub fn property_kind(sim: &Sim, property: i32) -> PropertyKind {
    match property_bounds(sim, property).map(|b| b.0) {
        Some(2) => PropertyKind::Simple,
        Some(3) => PropertyKind::Depletable,
        Some(4) => PropertyKind::Derived,
        Some(_) => PropertyKind::Attribute,
        None if property == 7 || property == 8 => PropertyKind::Depletable,
        None => PropertyKind::Attribute,
    }
}

pub(crate) fn property_bounds(sim: &Sim, property: i32) -> Option<(i32, f32, f32)> {
    let t = if sim.game == thedas_vfs::Game::DragonAge2 {
        sim.world.tables.get(98, "")?
    } else {
        sim.world.tables.get(0, "properties")?
    };
    let num = |c: Option<&Cell>| match c {
        Some(Cell::Int(v)) => Some(*v as f32),
        Some(Cell::Float(v)) => Some(*v as f32),
        _ => None,
    };
    if sim.game == thedas_vfs::Game::DragonAge2 {
        let (ty, min, max) = (
            num(t.cell_by_hash(0xbaf5_5bcf, property as i64))? as i32,
            num(t.cell_by_hash(0x9837_7833, property as i64))?,
            num(t.cell_by_hash(0xb48b_ff62, property as i64))?,
        );
        return (min <= max).then_some((ty, min, max));
    }
    let (min, max) = (
        num(t.cell("Min", property as i64))?,
        num(t.cell("Max", property as i64))?,
    );
    let ty = match t.cell("Type", property as i64) {
        Some(Cell::String(s)) if clean(s).eq_ignore_ascii_case("ATTRIBUTE") => 1,
        Some(Cell::String(s)) if clean(s).eq_ignore_ascii_case("SIMPLE") => 2,
        Some(Cell::String(s)) if clean(s).eq_ignore_ascii_case("DEPLETABLE") => 3,
        Some(Cell::String(s)) if clean(s).eq_ignore_ascii_case("DERIVED") => 4,
        _ => 0,
    };
    (min <= max).then_some((ty, min, max))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use thedas_script::EngineTable;
    use thedas_vfs::Vfs;

    use super::*;

    const LDF: &str = "void SetCooldown(object o, int n, float f, string s) = 1;\n\
        void DoAutoSave() = 9001;\n\
        void ClearCurrentCommand(object o) = 540;\n\
        int COMMAND_FAILED_COMMAND_CLEARED = -2;\n\
        int EVENT_TYPE_COMMAND_COMPLETE = 1004;\n\
        float GetRemainingCooldown(object o, int n, string s) = 2;\n\
        void RemoveItem(object o, int n) = 3;\n\
        void SetFollowerState(object o, int n) = 4;\n\
        int FOLLOWER_STATE_ACTIVE = 1;\n\
        int FOLLOWER_STATE_LOCKEDACTIVE = 5;\n\
        void SetCreatureProperty(object o, int p, float f, int t) = 5;\n\
        void UpdateCreatureProperty(object o, int p, float f, int t) = 43;\n\
        void SetTacticEntry(object o, int i, int e, int t, int c, int m, int p) = 44;\n\
        int GetTacticCondition(object o, int i) = 45;\n\
        int PROPERTY_DEPLETABLE_HEALTH = 7;\n\
        int PROPERTY_VALUE_TOTAL = 1;\n\
        int PROPERTY_VALUE_BASE = 2;\n\
        int PROPERTY_VALUE_CURRENT = 3;\n\
        int PROPERTY_VALUE_MODIFIER = 4;\n\
        float GetCreatureProperty(object o, int p, int t) = 31;\n\
        object CreateItemOnObject(resource r, object o, int n = 1, string s = \"\", int b = FALSE, int d = TRUE) = 6;\n\
        int FALSE = 0;\n\
        int TRUE = 1;\n\
        object GetObjectByTag(string s, int n = 0) = 7;\n\
        object[] GetNearestObjectByTag(object o, string s, int t = 32, int n = 1, int a = 0, int b = 0, int c = 0) = 8;\n\
        int GetFollowerState(object o) = 9;\n\
        int IsControlled(object o) = 10;\n\
        int IsPartyAIEnabled(object o) = 11;\n\
        object[] GetTeam(int n, int t = 32) = 12;\n\
        void ShowPopup(int s, int t, object o, int b = FALSE, int d = 0) = 13;\n\
        void OpenStore(object o) = 14;\n\
        int GetMaxInventorySize(object o) = 15;\n\
        void SetMaxInventorySize(int n, object o) = 16;\n\
        void SwitchWeaponSet(object o, int n = -1) = 17;\n\
        int GetActiveWeaponSet(object o) = 18;\n\
        void SetPlotActionState(int a, int s) = 19;\n\
        int GetPlotActionState(int a) = 20;\n\
        int EquipItem(object o, object i, int s = 255, int w = -1) = 21;\n\
        object GetItemInEquipSlot(int s, object o, int w = -1) = 22;\n\
        int CountItemsByTag(object o, string s) = 380;\n\
        int GetItemEquipSlot(object o) = 358;\n\
        int INVENTORY_SLOT_INVALID = 255;\n\
        object[] GetPartyPoolList() = 23;\n\
        void SetPlaceableActionResult(object o, int a, int s, int v = FALSE) = 24;\n\
        int GetPlaceableState(object o) = 25;\n\
        int GetEffectsFlags(object o) = 26;\n\
        #define ENGINE_STRUCTURE_3 effect\n\
        effect EffectModifyPropertyHostile(int p0, float c0, int p1 = 0, float c1 = 0.0, int p2 = 0, float c2 = 0.0) = 27;\n\
        effect SetEffectEngineInteger(effect e, int i, int v) = 28;\n\
        int GetEffectType(effect e) = 29;\n\
        string GetPlotGUID(string s) = 30;\n\
        int GetPlaceableBaseType(object o) = 40;\n\
        int ABILITY_INVALID = 0;\n\
        int GetItemAbilityId(object o) = 46;\n\
        int IsConjuring(object o) = 47;\n\
        void OpenFadeMap() = 48;\n\
        int GetGraphicsDetailLevel() = 49;\n\
        effect SetEffectEngineVector(effect e, int i, vector v) = 50;\n\
        int GetHasEffects(object o, int t = 0, int a = -1) = 51;\n\
        effect[] GetEffects(object o, int t = 0, int a = 0, object c = OBJECT_INVALID, int d = 0, int i = -1) = 52;\n\
        void RemoveEffectsByParameters(object o, int t = 0, int a = 0, object c = OBJECT_INVALID) = 53;\n\
        int GetAbilityCount(object o, int t = 0, int g = 0) = 54;\n\
        int[] GetAbilityList(object o, int t = 0, int cooling = 0) = 646;\n\
        float CombatGetPercentageFromScaled(int level, float scaled, int property) = 861;\n\
        void CharGen_ClearAbilityList(object o, int t = 0) = 55;\n\
        void SetQuickslot(object o, int n, int a, string s = \"\") = 56;\n\
        int GetPlaceablePickLockLevel(object o) = 57;\n\
        int GetPlaceableKeyRequired(object o) = 58;\n\
        string GetPlaceableKeyTag(object o) = 59;\n\
        int GetPlaceableAutoRemoveKey(object o) = 60;\n\
        int GetHasAchievement(string s) = 61;\n\
        void UnlockAchievement(string s) = 62;\n\
        int GetNumTactics(object o) = 63;\n\
        void AddTacticPresetID(object o, int n) = 64;\n\
        object GetTacticTargetObject(object o, int i) = 68;\n\
        object GetTacticConditionObject(object o, int i) = 69;\n\
        string GetTacticCommandItemTag(object o, int i) = 70;\n\
        int EVENT_TYPE_PARTYMEMBER_ADDED = 66;\n\
        int EVENT_TYPE_PARTYMEMBER_DROPPED = 67;\n\
        #define ENGINE_STRUCTURE_1 location\n\
        location Location(object a, vector v, float f) = 41;\n\
        void SetIndividualImpactAOEEvent(object c, object t, int n, location l, int d = 0) = 42;\n\
        int FOLLOWER_STATE_AVAILABLE = 2;\n\
        int PROPERTY_SIMPLE_EXPERIENCE = 19;\n\
        int PROPERTY_SIMPLE_LEVEL = 15;\n\
        int GetCanDiePermanently(object o) = 184;\n\
        int GetHashedM2DAInt(int n2DA, int nColumnHash, int nRow) = 841;\n\
        int GetHashedM2DABool(int n2DA, int nColumnHash, int nRow) = 883;\n\
        float GetHashedM2DAFloat(int n2DA, int nColumnHash, int nRow) = 864;\n\
        string GetHashedM2DAString(int n2DA, int nColumnHash, int nRow) = 865;\n\
        resource GetHashedM2DAResource(int n2DA, int nColumnHash, int nRow) = 866;\n\
        int GetItemLevel( object oItem ) = 701;\n\
        void SetEncounterId(object o, int id) = 425;\n\
        int GetEncounterId(object o) = 426;\n\
        object[] GetEncounter(int id) = 427;\n\
        void SetItemLevel( object oItem, int nLevel ) = 880;\n\
        int GetPlaceableStateController( object oPlaceable ) = 134;\n\
        int GetCreatureAmbientActivityPattern(object oCreature) = 670;\n\
        void SetCreatureAmbientActivityPattern(object oCreature, int n) = 671;\n\
        int max(int a, int b) = 422;\n\
        float minf(float a, float b) = 423;\n\
        void SetLocalString(object o, int n, string s) = 990;\n\
        string GetLocalString(object o, int n) = 991;\n\
        void SetLocalInt(object o, int n, int v) = 992;\n\
        int GetLocalInt(object o, int n) = 993;\n\
        void PlayCutscene(resource r) = 994;\n\
        int hasflag(int nInt, int nFlag) = 420;\n\
        void SetHeadMorphName(object o, string s) = 887;\n\
        object[] GetItemsInInventory(object o, int n = 1, int b = 0, string s = \"\", int p = 0) = 180;\n\
        int GetUpgradableAbilityHashedM2DAInt(object o, int nColumnHash, int nAbilityId) = 875;\n\
        float GetUpgradableAbilityHashedM2DAFloat(object o, int nColumnHash, int nAbilityId) = 876;\n";

    #[test]
    fn da2_encounter_commands_read_template_case_and_update_membership() {
        let table = Arc::new(EngineTable::parse(LDF.as_bytes()).unwrap());
        let mut s = Sim::new(Arc::new(Vfs::new()), table, 1);
        s.game = thedas_vfs::Game::DragonAge2;
        let area = s.world.spawn(ObjectKind::Area, "area");
        s.world.area = Some(area);
        let npc = s.world.spawn(ObjectKind::Creature, "npc");
        let o = s.world.get_mut(npc).unwrap();
        o.area = Some(area);
        o.active = false;
        o.fields.insert("EncounterID".into(), 42);
        s.restore_da2_encounters();
        assert_eq!(
            run(&mut s, 1, "GetEncounterId", &[Value::Object(npc)]),
            Some(Value::Int(42))
        );
        let members = Some(Value::Array {
            elem: Type::Object,
            items: vec![Value::Object(npc)],
        });
        assert_eq!(run(&mut s, 1, "GetEncounter", &[Value::Int(42)]), members);
        run(
            &mut s,
            1,
            "SetEncounterId",
            &[Value::Object(npc), Value::Int(43)],
        );
        assert_eq!(
            run(&mut s, 1, "GetEncounterId", &[Value::Object(npc)]),
            Some(Value::Int(43))
        );
        assert_eq!(run(&mut s, 1, "GetEncounter", &[Value::Int(43)]), members);
        assert_eq!(
            run(&mut s, 1, "GetEncounter", &[Value::Int(42)]),
            Some(Value::Array {
                elem: Type::Object,
                items: vec![]
            })
        );
    }

    #[test]
    fn da2_scaled_ratings_follow_level_curves_and_both_sides_of_the_knee() {
        let table = Arc::new(EngineTable::parse(LDF.as_bytes()).unwrap());
        let mut s = Sim::new(Arc::new(Vfs::new()), table, 1);
        let tables = crate::m2da::Tables::empty();
        // Synthetic curves, independent of the player's game data.
        tables.insert(
            "#371",
            &["#1d78b8e3", "#2c4edea1", "#7fc1781c", "#5ee22fe7"],
            (0..4)
                .map(|id| {
                    (
                        id,
                        vec![
                            Cell::Int(id),
                            Cell::Float(0.4),
                            Cell::Float(2.0),
                            Cell::Float(1.0),
                        ],
                    )
                })
                .chain([(4, vec![Cell::Int(4)]), (5, vec![Cell::Int(-1)])])
                .collect(),
        );
        tables.insert(
            "#405",
            &["#73a86afe", "#e33746bf", "#f9fc7170", "#6e1d17ea"],
            vec![
                (
                    1,
                    vec![
                        Cell::Float(100.0),
                        Cell::Float(200.0),
                        Cell::Float(300.0),
                        Cell::Float(400.0),
                    ],
                ),
                (
                    2,
                    vec![
                        Cell::Float(200.0),
                        Cell::Float(0.0),
                        Cell::Float(-1.0),
                        Cell::Float(f64::NAN),
                    ],
                ),
            ],
        );
        s.world.tables = Arc::new(tables);
        let percentage = |s: &mut Sim, level, scaled, property| match run(
            s,
            1,
            "CombatGetPercentageFromScaled",
            &[
                Value::Int(level),
                Value::Float(scaled),
                Value::Int(property),
            ],
        ) {
            Some(Value::Float(v)) => v,
            other => panic!("expected float: {other:?}"),
        };
        for (scaled, want) in [
            (0.0, 0.0),
            (50.0, 0.1),
            (100.0, 0.4),
            (200.0, 0.7),
            (300.0, 0.85),
            (-50.0, 0.1),
        ] {
            assert!((percentage(&mut s, 1, scaled, 0) - want).abs() < 1e-6);
        }
        // Every selector chooses a different column, while a new level changes the knee.
        for property in 0..4 {
            let optimal = (property + 1) as f32 * 100.0;
            assert!((percentage(&mut s, 1, optimal, property) - 0.4).abs() < 1e-6);
        }
        assert!((percentage(&mut s, 2, 100.0, 0) - 0.1).abs() < 1e-6);
        for (level, property) in [(99, 0), (1, 99), (1, 4), (1, 5), (2, 1), (2, 2), (2, 3)] {
            assert_eq!(percentage(&mut s, level, 100.0, property), 0.0);
        }
        assert!(percentage(&mut s, 1, f32::NAN, 0).is_nan());
    }

    #[test]
    fn da2_scaled_rating_missing_curve_cells_keep_the_native_zero_defaults() {
        let table = Arc::new(EngineTable::parse(LDF.as_bytes()).unwrap());
        let mut s = Sim::new(Arc::new(Vfs::new()), table, 1);
        assert_eq!(da2_percentage_from_scaled(&s, 1, 50.0, 0), 0.0);
        let tables = crate::m2da::Tables::empty();
        tables.insert(
            "#371",
            &["#1d78b8e3", "#2c4edea1"],
            vec![
                (0, vec![Cell::Int(0), Cell::Float(0.4)]),
                (1, vec![Cell::Int(0), Cell::Empty]),
            ],
        );
        tables.insert("#405", &["#73a86afe"], vec![(1, vec![Cell::Float(100.0)])]);
        s.world.tables = Arc::new(tables);
        // Missing lower exponent is zero (power of zero), missing ceiling is zero.
        assert!((da2_percentage_from_scaled(&s, 1, 50.0, 0) - 0.4).abs() < 1e-6);
        assert!((da2_percentage_from_scaled(&s, 1, 200.0, 0) - 0.2).abs() < 1e-6);
        assert_eq!(da2_percentage_from_scaled(&s, 1, 200.0, 1), 0.0);
    }

    #[test]
    fn da2_ability_lists_exclude_items_and_enumerate_cooldowns_separately() {
        let table = Arc::new(EngineTable::parse(LDF.as_bytes()).unwrap());
        let mut s = Sim::new(Arc::new(Vfs::new()), table, 1);
        let tables = crate::m2da::Tables::empty();
        tables.insert(
            "#1",
            &["#100b492a", "AbilityType"],
            vec![
                (1, vec![Cell::Int(1), Cell::Int(4)]),
                (2, vec![Cell::Int(2), Cell::Int(4)]),
                (3, vec![Cell::Int(4), Cell::Int(1)]),
                (4, vec![Cell::Int(1), Cell::Int(4)]),
                (5, vec![Cell::Int(1), Cell::Int(4)]),
            ],
        );
        s.world.tables = Arc::new(tables);
        let hero = s.world.spawn(ObjectKind::Creature, "hero");
        let item = s.world.spawn(ObjectKind::Item, "item");
        s.world.time_ms = 1000;
        let o = s.world.get_mut(hero).unwrap();
        o.abilities = [1, 2, 3, 4, 99].into_iter().collect();
        o.combat.cooldowns = [
            ("1:".into(), 2000),
            ("2:".into(), 1000),
            ("3:potion".into(), 2000),
            ("4:".into(), 999),
            ("5:".into(), 2000),
            ("5:source".into(), 3000),
        ]
        .into_iter()
        .collect();
        let list = |s: &mut Sim, who, ty, cooling| match run(
            s,
            hero,
            "GetAbilityList",
            &[Value::Object(who), Value::Int(ty), Value::Int(cooling)],
        ) {
            Some(Value::Array {
                elem: Type::Int,
                items,
            }) => items,
            other => panic!("expected integer array: {other:?}"),
        };
        let ints = |ids: &[i32]| ids.iter().copied().map(Value::Int).collect::<Vec<_>>();
        assert_eq!(list(&mut s, hero, 0, 0), ints(&[1, 2, 4]));
        assert_eq!(list(&mut s, hero, 1, 0), ints(&[1, 4]));
        assert_eq!(list(&mut s, hero, 2, 0), ints(&[2]));
        assert_eq!(list(&mut s, hero, 0, 1), ints(&[1, 5, 5]));
        assert_eq!(list(&mut s, hero, 1, 1), ints(&[1, 5, 5]));
        for (who, ty, cooling) in [
            (hero, 4, 0),
            (hero, 4, 1),
            (hero, -1, 0),
            (item, 0, 0),
            (OBJECT_INVALID, 0, 1),
        ] {
            assert!(list(&mut s, who, ty, cooling).is_empty());
        }
        // Expired records are ignored even while still stored; the query is read-only.
        assert_eq!(s.world.get(hero).unwrap().combat.cooldowns.len(), 6);
        s.world.time_ms = 3000;
        assert!(list(&mut s, hero, 0, 1).is_empty());
    }

    #[test]
    fn dragon_age_2_functions_read_and_set_template_fields() {
        let table = Arc::new(EngineTable::parse(LDF.as_bytes()).unwrap());
        let mut s = Sim::new(Arc::new(Vfs::new()), table, 1);
        let item = s.world.spawn(ObjectKind::Item, "sword");
        s.world
            .get_mut(item)
            .unwrap()
            .fields
            .insert("Level".into(), 7);
        let door = s.world.spawn(ObjectKind::Placeable, "door");
        s.world
            .get_mut(door)
            .unwrap()
            .fields
            .insert("StateController".into(), 1);
        let npc = s.world.spawn(ObjectKind::Creature, "npc");
        let o = |id| Value::Object(id);
        assert_eq!(
            run(&mut s, 1, "GetItemLevel", &[o(item)]),
            Some(Value::Int(7))
        );
        run(&mut s, 1, "SetItemLevel", &[o(item), Value::Int(9)]);
        assert_eq!(
            run(&mut s, 1, "GetItemLevel", &[o(item)]),
            Some(Value::Int(9))
        );
        assert_eq!(
            run(&mut s, 1, "GetPlaceableStateController", &[o(door)]),
            Some(Value::Int(1))
        );
        run(
            &mut s,
            1,
            "SetCreatureAmbientActivityPattern",
            &[o(npc), Value::Int(3)],
        );
        assert_eq!(
            run(&mut s, 1, "GetCreatureAmbientActivityPattern", &[o(npc)]),
            Some(Value::Int(3))
        );
        assert_eq!(
            run(&mut s, 1, "max", &[Value::Int(2), Value::Int(5)]),
            Some(Value::Int(5))
        );
        assert_eq!(
            run(&mut s, 1, "minf", &[Value::Float(2.5), Value::Float(1.5)]),
            Some(Value::Float(1.5))
        );
        assert_eq!(
            run(
                &mut s,
                1,
                "hasflag",
                &[Value::Int(0b1010), Value::Int(0b1000)]
            ),
            Some(Value::Int(1))
        );
        assert_eq!(
            run(
                &mut s,
                1,
                "hasflag",
                &[Value::Int(0b1010), Value::Int(0b0100)]
            ),
            Some(Value::Int(0))
        );
        run(
            &mut s,
            1,
            "SetHeadMorphName",
            &[o(npc), Value::String("HM_IconicMale.mor".into())],
        );
        assert_eq!(
            s.world.get(npc).unwrap().head_morph.as_deref(),
            Some("hm_iconicmale")
        );
        run(
            &mut s,
            1,
            "SetHeadMorphName",
            &[o(npc), Value::String(String::new())],
        );
        assert_eq!(s.world.get(npc).unwrap().head_morph, None);
    }

    #[test]
    fn dragon_age_2_objects_with_another_tables_variables_number_them_in_template_order() {
        let table = Arc::new(EngineTable::parse(LDF.as_bytes()).unwrap());
        let mut s = Sim::new(Arc::new(Vfs::new()), table, 1);
        s.world.tables().insert(
            "var_trigger",
            &["Label"],
            vec![
                (0, vec![Cell::String("TRIGGER_COUNTER_1\0".into())]),
                (1, vec![Cell::String("TRIGGER_COUNTER_2\0".into())]),
            ],
        );
        // A talk trigger: its template's variables are var_trigger_talk's, in its rows' order.
        let talk = s.world.spawn(ObjectKind::Trigger, "gentr_talk");
        {
            let l = &mut s.world.get_mut(talk).unwrap().locals;
            l.order = vec!["TRIG_TALK_SPEAKER".into(), "TRIG_TALK_REPEAT".into()];
            l.strings
                .insert("TRIG_TALK_SPEAKER".into(), "gen00fl_bethany".into());
        }
        // A generic trigger keeps the kind's table.
        let plain = s.world.spawn(ObjectKind::Trigger, "gentr_counter");
        s.world.get_mut(plain).unwrap().locals.order =
            vec!["TRIGGER_COUNTER_1".into(), "TRIGGER_COUNTER_2".into()];
        let o = |id| Value::Object(id);
        assert_eq!(
            run(&mut s, 1, "GetLocalString", &[o(talk), Value::Int(0)]),
            Some(Value::String("gen00fl_bethany".into()))
        );
        run(
            &mut s,
            1,
            "SetLocalInt",
            &[o(talk), Value::Int(1), Value::Int(1)],
        );
        assert_eq!(
            s.world
                .get(talk)
                .unwrap()
                .locals
                .ints
                .get("TRIG_TALK_REPEAT"),
            Some(&1)
        );
        run(
            &mut s,
            1,
            "SetLocalInt",
            &[o(plain), Value::Int(1), Value::Int(4)],
        );
        assert_eq!(
            s.world
                .get(plain)
                .unwrap()
                .locals
                .ints
                .get("TRIGGER_COUNTER_2"),
            Some(&4)
        );
    }

    #[test]
    fn dragon_age_2_locals_are_named_by_their_variable_table_row() {
        let table = Arc::new(EngineTable::parse(LDF.as_bytes()).unwrap());
        let mut s = Sim::new(Arc::new(Vfs::new()), table, 1);
        s.world.tables().insert(
            "var_module",
            &["Label"],
            vec![
                (1014, vec![Cell::String("CUTSCENE_SET_PLOT\0".into())]),
                (1015, vec![Cell::String("CUTSCENE_SET_PLOT_FLAG\0".into())]),
            ],
        );
        s.world.tables().insert(
            "var_creature",
            &["Label"],
            vec![(1014, vec![Cell::String("AI_THING\0".into())])],
        );
        let module = s.world.spawn(ObjectKind::Module, "module");
        let npc = s.world.spawn(ObjectKind::Creature, "npc");
        let o = |id| Value::Object(id);
        run(
            &mut s,
            1,
            "SetLocalString",
            &[o(module), Value::Int(1014), Value::String("plt_x".into())],
        );
        run(
            &mut s,
            1,
            "SetLocalInt",
            &[o(module), Value::Int(1015), Value::Int(3)],
        );
        run(
            &mut s,
            1,
            "SetLocalInt",
            &[o(npc), Value::Int(1014), Value::Int(7)],
        );
        run(
            &mut s,
            1,
            "SetLocalInt",
            &[o(npc), Value::Int(55), Value::Int(8)],
        );
        let m = &s.world.get(module).unwrap().locals;
        assert_eq!(
            m.strings.get("CUTSCENE_SET_PLOT").map(String::as_str),
            Some("plt_x")
        );
        assert_eq!(m.ints.get("CUTSCENE_SET_PLOT_FLAG"), Some(&3));
        // The row is looked up in the object's own table; a row it lacks keeps its number.
        let n = &s.world.get(npc).unwrap().locals;
        assert_eq!(n.ints.get("AI_THING"), Some(&7));
        assert_eq!(n.ints.get("55"), Some(&8));
        assert_eq!(
            run(&mut s, 1, "GetLocalInt", &[o(npc), Value::Int(1014)]),
            Some(Value::Int(7))
        );
        // Dragon Age II's PlayCutscene takes the cutscene and queues it.
        run(
            &mut s,
            1,
            "PlayCutscene",
            &[Value::Resource("pro000cs_opening.cut".into())],
        );
        assert_eq!(s.world.cutscenes, vec!["pro000cs_opening.cut".to_string()]);
    }

    #[test]
    fn dragon_age_2_events_carry_vectors() {
        let ldf = "#define ENGINE_STRUCTURE_0 event\n\
            event Event(int nType) = 1;\n\
            event SetEventVector(event e, int n, vector v) = 2;\n\
            vector GetEventVector(event e, int n) = 3;\n\
            int IsVectorEmpty(vector v) = 4;\n";
        let table = Arc::new(EngineTable::parse(ldf.as_bytes()).unwrap());
        let mut s = Sim::new(Arc::new(Vfs::new()), table, 1);
        let ev = run(&mut s, 1, "Event", &[Value::Int(1001)]).unwrap();
        let ev = run(
            &mut s,
            1,
            "SetEventVector",
            &[ev, Value::Int(1), Value::Vector([1.0, 2.0, 3.0])],
        )
        .unwrap();
        assert_eq!(
            run(&mut s, 1, "GetEventVector", &[ev.clone(), Value::Int(1)]),
            Some(Value::Vector([1.0, 2.0, 3.0]))
        );
        // One never set is the zero vector, which is empty.
        let unset = run(&mut s, 1, "GetEventVector", &[ev, Value::Int(0)]).unwrap();
        assert_eq!(unset, Value::Vector([0.0; 3]));
        assert_eq!(
            run(&mut s, 1, "IsVectorEmpty", &[unset]),
            Some(Value::Int(1))
        );
        assert_eq!(
            run(
                &mut s,
                1,
                "IsVectorEmpty",
                &[Value::Vector([0.0, 0.5, 0.0])]
            ),
            Some(Value::Int(0))
        );
    }

    #[test]
    fn hashed_table_reads_find_columns_by_hash() {
        let table = Arc::new(EngineTable::parse(LDF.as_bytes()).unwrap());
        let mut s = Sim::new(Arc::new(Vfs::new()), table, 1);
        s.world.tables().insert(
            "#77",
            &["Label", "Speed", "Flag", "Model"],
            vec![(
                3,
                vec![
                    Cell::String("row".into()),
                    Cell::Float(1.5),
                    Cell::Int(2),
                    Cell::String("c_dog\0".into()),
                ],
            )],
        );
        let col = |name: &str| Value::Int(thedas_formats::gda::column_hash(name) as i32);
        let read = |s: &mut Sim, f: &str, column: &str, row: i32| {
            run(s, 1, f, &[Value::Int(77), col(column), Value::Int(row)])
        };
        assert_eq!(
            read(&mut s, "GetHashedM2DAFloat", "Speed", 3),
            Some(Value::Float(1.5))
        );
        assert_eq!(
            read(&mut s, "GetHashedM2DAInt", "Flag", 3),
            Some(Value::Int(2))
        );
        assert_eq!(
            read(&mut s, "GetHashedM2DABool", "Flag", 3),
            Some(Value::Int(1))
        );
        assert_eq!(
            read(&mut s, "GetHashedM2DAString", "Label", 3),
            Some(Value::String("row".into()))
        );
        assert_eq!(
            read(&mut s, "GetHashedM2DAResource", "Model", 3),
            Some(Value::Resource("c_dog".into()))
        );
        // A missing row or column reads as the type's empty value.
        assert_eq!(
            read(&mut s, "GetHashedM2DABool", "Flag", 9),
            Some(Value::Int(0))
        );
        assert_eq!(
            read(&mut s, "GetHashedM2DAString", "Nope", 3),
            Some(Value::String(String::new()))
        );
    }

    #[test]
    fn da2_upgradable_numbers_add_only_learned_direct_upgrades_in_marked_columns() {
        let table = Arc::new(EngineTable::parse(LDF.as_bytes()).unwrap());
        let mut s = Sim::new(Arc::new(Vfs::new()), table, 7);
        s.world.tables().insert(
            "#1",
            &[
                "Range",
                "Charges",
                "Fixed",
                "#d300f4d9",
                "#f82da71a",
                "#e136965b",
                "#ae77009c",
            ],
            vec![
                (0, vec![Cell::Float(1.0), Cell::Int(1), Cell::Int(0)]),
                (
                    100,
                    vec![
                        Cell::Float(2.5),
                        Cell::Int(3),
                        Cell::Int(9),
                        Cell::Int(101),
                        Cell::Int(102),
                        Cell::Int(103),
                        Cell::Int(104),
                    ],
                ),
                (
                    101,
                    vec![
                        Cell::Float(0.75),
                        Cell::Int(2),
                        Cell::Int(100),
                        Cell::Int(105),
                    ],
                ),
                (102, vec![Cell::Float(-0.25), Cell::Int(-1), Cell::Int(200)]),
                (103, vec![Cell::Float(0.5), Cell::Int(4), Cell::Int(300)]),
                (104, vec![Cell::Float(1.25), Cell::Int(5), Cell::Int(400)]),
                (105, vec![Cell::Float(99.0), Cell::Int(99), Cell::Int(999)]),
            ],
        );
        let npc = s.world.spawn(ObjectKind::Creature, "npc");
        let read = |s: &mut Sim, float: bool, column: &str| {
            run(
                s,
                npc,
                if float {
                    "GetUpgradableAbilityHashedM2DAFloat"
                } else {
                    "GetUpgradableAbilityHashedM2DAInt"
                },
                &[
                    Value::Object(npc),
                    Value::Int(thedas_formats::gda::column_hash(column) as i32),
                    Value::Int(100),
                ],
            )
        };
        assert_eq!(read(&mut s, true, "Range"), Some(Value::Float(2.5)));
        assert_eq!(read(&mut s, false, "Charges"), Some(Value::Int(3)));
        // The base ability need not be learned. A learned upgrade's own upgrade
        // does not recursively contribute, and an unlearned direct upgrade does not.
        s.world
            .get_mut(npc)
            .unwrap()
            .abilities
            .extend([101, 103, 104, 105]);
        let before = s.save_json().unwrap();
        assert_eq!(read(&mut s, true, "Range"), Some(Value::Float(5.0)));
        assert_eq!(read(&mut s, false, "Charges"), Some(Value::Int(14)));
        assert_eq!(read(&mut s, false, "Fixed"), Some(Value::Int(9)));
        assert_eq!(read(&mut s, false, "Missing"), Some(Value::Int(0)));
        assert_eq!(
            s.save_json().unwrap(),
            before,
            "queries change no world state or RNG"
        );
        s.world.get_mut(npc).unwrap().abilities.remove(&101);
        assert_eq!(read(&mut s, true, "Range"), Some(Value::Float(4.25)));
        assert_eq!(read(&mut s, false, "Charges"), Some(Value::Int(12)));
        s.world.get_mut(npc).unwrap().abilities.insert(102);
        assert_eq!(read(&mut s, true, "Range"), Some(Value::Float(4.0)));
        assert_eq!(read(&mut s, false, "Charges"), Some(Value::Int(11)));
    }

    #[test]
    fn da2_upgradable_numbers_require_a_creature_and_respect_missing_metadata() {
        let table = Arc::new(EngineTable::parse(LDF.as_bytes()).unwrap());
        let mut s = Sim::new(Arc::new(Vfs::new()), table, 7);
        s.world.tables().insert(
            "#1",
            &["Value", "#d300f4d9"],
            vec![
                (100, vec![Cell::Int(7), Cell::Int(101)]),
                (101, vec![Cell::Int(99)]),
            ],
        );
        let creature = s.world.spawn(ObjectKind::Creature, "creature");
        let item = s.world.spawn(ObjectKind::Item, "item");
        s.world.get_mut(creature).unwrap().abilities.insert(101);
        let column = Value::Int(thedas_formats::gda::column_hash("Value") as i32);
        for (id, expected) in [(creature, 7), (item, 0), (OBJECT_INVALID, 0)] {
            assert_eq!(
                run(
                    &mut s,
                    creature,
                    "GetUpgradableAbilityHashedM2DAInt",
                    &[Value::Object(id), column.clone(), Value::Int(100)]
                ),
                Some(Value::Int(expected))
            );
            assert_eq!(
                run(
                    &mut s,
                    creature,
                    "GetUpgradableAbilityHashedM2DAFloat",
                    &[Value::Object(id), column.clone(), Value::Int(100)]
                ),
                Some(Value::Float(expected as f32))
            );
        }
        assert_eq!(
            run(
                &mut s,
                creature,
                "GetUpgradableAbilityHashedM2DAInt",
                &[Value::Object(creature), column, Value::Int(999)]
            ),
            Some(Value::Int(0))
        );
    }

    #[test]
    fn dragon_age_2_upgradable_ability_numbers_come_from_the_ability_row() {
        let table = Arc::new(EngineTable::parse(LDF.as_bytes()).unwrap());
        let mut s = Sim::new(Arc::new(Vfs::new()), table, 1);
        s.world.tables().insert(
            "#1",
            &["Range", "Charges"],
            vec![(307001, vec![Cell::Float(2.5), Cell::Int(3)])],
        );
        let npc = s.world.spawn(ObjectKind::Creature, "npc");
        let col = |name: &str| Value::Int(thedas_formats::gda::column_hash(name) as i32);
        let o = Value::Object(npc);
        assert_eq!(
            run(
                &mut s,
                1,
                "GetUpgradableAbilityHashedM2DAFloat",
                &[o.clone(), col("Range"), Value::Int(307001)]
            ),
            Some(Value::Float(2.5))
        );
        assert_eq!(
            run(
                &mut s,
                1,
                "GetUpgradableAbilityHashedM2DAInt",
                &[o.clone(), col("Charges"), Value::Int(307001)]
            ),
            Some(Value::Int(3))
        );
        assert_eq!(
            run(
                &mut s,
                1,
                "GetUpgradableAbilityHashedM2DAInt",
                &[o, col("Charges"), Value::Int(1)]
            ),
            Some(Value::Int(0))
        );
    }

    #[test]
    fn origins_integer_conversions_share_prefix_radix_and_signed_limits() {
        let table = Arc::new(
            EngineTable::parse(
                br#"
            int StringToInt(string s) = 51;
            int HexStringToInt(string s) = 58;
        "#,
            )
            .unwrap(),
        );
        let mut s = Sim::new(Arc::new(Vfs::new()), table, 1);
        for name in ["StringToInt", "HexStringToInt"] {
            for (text, expected) in [
                ("", 0),
                ("text", 0),
                ("+", 0),
                ("- 1", 0),
                (" \t\n\u{b}\u{c}\r\u{a0}-42 trailing", -42),
                ("\u{85}42", 0),
                ("123abc", 123),
                ("1_234", 1),
                ("12\0x", 12),
                ("0779", 63),
                ("08", 0),
                ("+0X2a!", 42),
                ("-0x2a", -42),
                ("ff", 0),
                ("0x", 0),
                ("0xg", 0),
                ("0x1p2", 1),
                ("2147483647", i32::MAX),
                ("2147483648", i32::MAX),
                ("-2147483648", i32::MIN),
                ("-2147483649", i32::MIN),
                ("0xffffffff", i32::MAX),
                ("-0x80000000", i32::MIN),
                ("99999999999999999999999999999999", i32::MAX),
                ("-99999999999999999999999999999999", i32::MIN),
                ("\u{ff11}\u{ff12}rest", 12),
                ("\u{660}\u{667}8", 7),
                ("\u{660}xF", 15),
                ("\u{1d7cf}", 0),
            ] {
                assert_eq!(
                    run(&mut s, 1, name, &[Value::String(text.into())]),
                    Some(Value::Int(expected)),
                    "{name}({text:?})"
                );
            }
        }
        // DA2 keeps its earlier whole-decimal parser and unimplemented hex conversion.
        s.game = thedas_vfs::Game::DragonAge2;
        for (text, expected) in [("077", 77), ("12tail", 0), ("0x2a", 0), ("2147483648", 0)] {
            assert_eq!(
                run(&mut s, 1, "StringToInt", &[Value::String(text.into())]),
                Some(Value::Int(expected))
            );
        }
        let f = s.table.function_by_name("HexStringToInt").unwrap().clone();
        let mut host = ScriptHost {
            sim: &mut s,
            event: None,
            script: "test".into(),
        };
        assert!(call(&mut host, 1, &f, &[Value::String("0x2a".into())]).is_none());
    }

    #[test]
    fn origins_float_conversion_consumes_the_native_decimal_prefix() {
        let table = Arc::new(EngineTable::parse(b"float StringToFloat(string s) = 57;").unwrap());
        let mut s = Sim::new(Arc::new(Vfs::new()), table, 1);
        let convert = |s: &mut Sim, text: &str| {
            let Some(Value::Float(value)) =
                run(s, 1, "StringToFloat", &[Value::String(text.into())])
            else {
                panic!("StringToFloat must return a float");
            };
            value
        };
        for (text, expected) in [
            ("", 0.0f32),
            ("text", 0.0),
            ("+", 0.0),
            ("-text", 0.0),
            (".", 0.0),
            ("-.e2", 0.0),
            ("- 1", 0.0),
            (" \t\n\u{b}\u{c}\r\u{a0}-1.25tail", -1.25),
            ("\u{85}1.25", 0.0),
            ("+12.5suffix", 12.5),
            (".25rest", 0.25),
            ("1.", 1.0),
            ("1.2.3", 1.2),
            ("0779", 779.0),
            ("0x1p2", 0.0),
            ("1,5", 1.0),
            ("1_234", 1.0),
            ("12\0.5", 12.0),
            ("2e3tail", 2000.0),
            ("2E-2!", 0.02),
            ("2d+2!", 200.0),
            ("2D-2", 0.02),
            ("1.e2", 100.0),
            ("12e", 12.0),
            ("12e+tail", 12.0),
            ("12d-", 12.0),
            ("12-3", 12.0),
            ("12e 3", 12.0),
            ("12e000tail", 12.0),
            ("\u{ff11}.5", 0.0),
            ("1.\u{ff15}", 1.0),
            ("1e\u{661}", 1.0),
            ("inf", 0.0),
            ("-Infinity", 0.0),
            ("NaN", 0.0),
            ("-0tail", -0.0),
            ("-.0e+2", -0.0),
            ("1e9999", f32::INFINITY),
            ("-1e9999", f32::NEG_INFINITY),
            ("-1e-9999", -0.0),
            // Double rounding distinguishes this from directly parsing to f32.
            ("1.0000000596046448", 1.0),
        ] {
            assert_eq!(
                convert(&mut s, text).to_bits(),
                expected.to_bits(),
                "{text:?}"
            );
        }
        // DA2 retains its whole-string parser, including its special values.
        s.game = thedas_vfs::Game::DragonAge2;
        for (text, expected) in [
            ("1.5tail", 0.0),
            ("2D2", 0.0),
            ("\u{85}1.5", 1.5),
            ("inf", f32::INFINITY),
            ("1.0000000596046448", f32::from_bits(1.0f32.to_bits() + 1)),
        ] {
            assert_eq!(
                convert(&mut s, text).to_bits(),
                expected.to_bits(),
                "{text:?}"
            );
        }
        assert!(convert(&mut s, "NaN").is_nan());
    }

    #[test]
    fn origins_object_to_string_formats_raw_ids_as_signed_decimal() {
        let table = Arc::new(EngineTable::parse(b"string ObjectToString(object o) = 54;").unwrap());
        let mut s = Sim::new(Arc::new(Vfs::new()), table, 1);
        let live = s.world.spawn(ObjectKind::Creature, "not_the_id");
        s.world.get_mut(live).unwrap().template = "not_the_template".into();
        let removed = s.world.spawn(ObjectKind::Placeable, "removed");
        s.world.objects.remove(&removed);
        let convert = |s: &mut Sim, id| run(s, live, "ObjectToString", &[Value::Object(id)]);
        for (id, expected) in [
            (0, "0"),
            (live, "1"),
            (removed, "2"),
            (OBJECT_INVALID, "2130706432"),
            (0x7fff_ffff, "2147483647"),
            (0x8000_0000, "-2147483648"),
            (0xffff_fffe, "-2"),
            (0xffff_ffff, "-1"),
        ] {
            assert_eq!(convert(&mut s, id), Some(Value::String(expected.into())));
        }
        // DA2's formatting is untraced and retains its earlier unsigned result.
        s.game = thedas_vfs::Game::DragonAge2;
        for (id, expected) in [
            (live, "1"),
            (OBJECT_INVALID, "2130706432"),
            (0x8000_0000, "2147483648"),
            (0xffff_ffff, "4294967295"),
        ] {
            assert_eq!(convert(&mut s, id), Some(Value::String(expected.into())));
        }
    }

    /// Calls engine function `name` as `caller`.
    fn run(s: &mut Sim, caller: ObjectId, name: &str, a: &[Value]) -> Option<Value> {
        let f = s.table.function_by_name(name).cloned().unwrap();
        let mut h = ScriptHost {
            sim: s,
            event: None,
            script: "test".into(),
        };
        call(&mut h, caller, &f, a).unwrap().unwrap()
    }

    #[test]
    fn unnamed_constructor_types_are_observable_through_the_script_accessor() {
        let table = Arc::new(
            EngineTable::parse(
                br#"
          #define ENGINE_STRUCTURE_2 command
          command CommandMoveAwayFromObject() = 1;
          command CommandMoveToMultiLocations() = 2;
          command CommandSheatheWeapons() = 3;
          command CommandUnsheatheWeapons() = 4;
          command CommandTurn() = 5;
          int GetCommandType(command c) = 6;
        "#,
            )
            .unwrap(),
        );
        let mut s = Sim::new(Arc::new(Vfs::new()), table, 1);
        let creature = s.world.spawn(ObjectKind::Creature, "creature");
        for (name, expected) in [
            ("CommandMoveAwayFromObject", 21),
            ("CommandMoveToMultiLocations", 22),
            ("CommandSheatheWeapons", 39),
            ("CommandUnsheatheWeapons", 39),
            ("CommandTurn", 41),
        ] {
            let command = run(&mut s, creature, name, &[]).unwrap();
            assert_eq!(
                run(&mut s, creature, "GetCommandType", &[command]),
                Some(Value::Int(expected)),
                "{name}"
            );
        }
        s.game = thedas_vfs::Game::DragonAge2;
        let command = run(&mut s, creature, "CommandTurn", &[]).unwrap();
        assert_eq!(
            run(&mut s, creature, "GetCommandType", &[command]),
            Some(Value::Int(1004))
        );
    }

    #[test]
    fn treasure_rank_reads_only_a_placeables_rank() {
        let table =
            Arc::new(EngineTable::parse(b"int GetPlaceableTreasureRank(object o) = 818;").unwrap());
        let mut s = Sim::new(Arc::new(Vfs::new()), table, 1);
        let placeable = s.world.spawn(ObjectKind::Placeable, "chest");
        for rank in [0, 7, -1] {
            s.world.get_mut(placeable).unwrap().rank = rank;
            assert_eq!(
                run(
                    &mut s,
                    placeable,
                    "GetPlaceableTreasureRank",
                    &[Value::Object(placeable)]
                ),
                Some(Value::Int(rank))
            );
        }
        let creature = s.world.spawn(ObjectKind::Creature, "creature");
        s.world.get_mut(creature).unwrap().rank = 7;
        let item = s.world.spawn(ObjectKind::Item, "item");
        s.world.get_mut(item).unwrap().rank = 7;
        for target in [creature, item, OBJECT_INVALID] {
            assert_eq!(
                run(
                    &mut s,
                    placeable,
                    "GetPlaceableTreasureRank",
                    &[Value::Object(target)]
                ),
                Some(Value::Int(0))
            );
        }
    }

    #[test]
    fn humanoids_are_identified_by_the_appearances_combat_tree() {
        let table = Arc::new(EngineTable::parse(b"int IsHumanoid(object o) = 363;").unwrap());
        let mut s = Sim::new(Arc::new(Vfs::new()), table, 1);
        let tables = crate::m2da::Tables::empty();
        tables.insert(
            "#2",
            &["ID", "#de80b418"],
            [
                "combat_humanoid.anb",
                "combat_wolf.anb",
                "Combat_Humanoid.anb",
                "prefix_combat_humanoid_variant.anb",
                "",
            ]
            .into_iter()
            .enumerate()
            .map(|(i, tree)| {
                (
                    (i + 1) as i64,
                    vec![Cell::Int((i + 1) as i64), Cell::String(tree.into())],
                )
            })
            .collect(),
        );
        s.world.tables = Arc::new(tables);
        let creature = s.world.spawn(ObjectKind::Creature, "creature");
        for (appearance, expected) in [(1, 1), (2, 0), (3, 0), (4, 1), (5, 0), (6, 0), (65537, 1)] {
            s.world.get_mut(creature).unwrap().appearance = appearance;
            assert_eq!(
                run(&mut s, creature, "IsHumanoid", &[Value::Object(creature)]),
                Some(Value::Int(expected)),
                "appearance {appearance}"
            );
        }
        let placeable = s.world.spawn(ObjectKind::Placeable, "placeable");
        s.world.get_mut(placeable).unwrap().appearance = 1;
        for target in [placeable, OBJECT_INVALID] {
            assert_eq!(
                run(&mut s, creature, "IsHumanoid", &[Value::Object(target)]),
                Some(Value::Int(0))
            );
        }
        s.game = thedas_vfs::Game::DragonAge2;
        s.world.get_mut(creature).unwrap().appearance = 2;
        assert_eq!(
            run(&mut s, creature, "IsHumanoid", &[Value::Object(creature)]),
            Some(Value::Int(1))
        );
        s.game = thedas_vfs::Game::Origins;
        s.world.tables = Arc::new(crate::m2da::Tables::empty());
        assert_eq!(
            run(&mut s, creature, "IsHumanoid", &[Value::Object(creature)]),
            Some(Value::Int(0))
        );
        s.game = thedas_vfs::Game::DragonAge2;
        for (target, expected) in [(creature, 1), (placeable, 0), (OBJECT_INVALID, 0)] {
            assert_eq!(
                run(&mut s, creature, "IsHumanoid", &[Value::Object(target)]),
                Some(Value::Int(expected))
            );
        }
    }

    #[test]
    fn clear_current_command_ends_only_the_running_command() {
        let table = Arc::new(EngineTable::parse(LDF.as_bytes()).unwrap());
        let mut s = Sim::new(Arc::new(Vfs::new()), table, 1);
        let c = s.world.spawn(ObjectKind::Creature, "c");
        let cmd = |name: &str, due: Option<u64>| Command {
            ty: 7,
            name: name.into(),
            due,
            ..Default::default()
        };
        // Not started: nothing is current, nothing happens.
        s.world.get_mut(c).unwrap().commands = vec![cmd("CommandWait", None)];
        run(&mut s, c, "ClearCurrentCommand", &[Value::Object(c)]);
        assert_eq!(s.world.get(c).unwrap().commands.len(), 1);
        // Started: it goes, the queue behind it stays, and the creature hears the result.
        s.world.get_mut(c).unwrap().commands =
            vec![cmd("CommandWait", Some(500)), cmd("CommandAttack", None)];
        run(&mut s, c, "ClearCurrentCommand", &[Value::Object(c)]);
        let o = s.world.get(c).unwrap();
        assert_eq!(o.commands.len(), 1);
        assert_eq!(o.commands[0].name, "CommandAttack");
        let ev = s
            .world
            .queue
            .values()
            .find(|q| q.target == c)
            .map(|q| q.event.clone());
        assert_eq!(ev.map(|e| e.ints), Some(vec![7, -2]));
    }

    #[test]
    fn party_members_and_no_perm_death_creatures_cannot_die_permanently() {
        let table = Arc::new(EngineTable::parse(LDF.as_bytes()).unwrap());
        let mut s = Sim::new(Arc::new(Vfs::new()), table, 1);
        let hero = s.world.spawn(ObjectKind::Creature, "player");
        s.world.hero = Some(hero);
        let follower = s.world.spawn(ObjectKind::Creature, "follower");
        s.world.get_mut(hero).unwrap().followers.push(follower);
        let foe = s.world.spawn(ObjectKind::Creature, "foe");
        let kept = s.world.spawn(ObjectKind::Creature, "kept");
        s.world
            .get_mut(kept)
            .unwrap()
            .fields
            .insert("NoPermDeath".into(), 1);
        let mut can = |id| run(&mut s, hero, "GetCanDiePermanently", &[Value::Object(id)]);
        assert_eq!(can(foe), Some(Value::Int(1)));
        assert_eq!(can(follower), Some(Value::Int(0)));
        assert_eq!(can(hero), Some(Value::Int(0)));
        assert_eq!(can(kept), Some(Value::Int(0)));
        // A destroyed follower leaves the party list.
        s.world.queue_destroy(0, follower);
        s.world.destroy_due();
        assert!(s.world.get(hero).unwrap().followers.is_empty());
    }

    #[test]
    fn plot_guids_come_from_resource_names() {
        let table = Arc::new(EngineTable::parse(LDF.as_bytes()).unwrap());
        let mut s = Sim::new(Arc::new(Vfs::new()), table, 1);
        s.plots.insert(crate::plot::PlotInfo {
            guid: "41A427F27CAB4027A8B6C00BC123A0E1".into(),
            resref: "plt_lot100pt_bears".into(),
            script: None,
            name_strref: None,
            flags: Default::default(),
            rewards: Default::default(),
            journal: Default::default(),
            initial: [0; 4],
            ..Default::default()
        });
        let guid = |s: &mut Sim, name: &str| {
            run(
                s,
                OBJECT_INVALID,
                "GetPlotGUID",
                &[Value::String(name.into())],
            )
        };
        let want = Some(Value::String("41A427F27CAB4027A8B6C00BC123A0E1".into()));
        assert_eq!(guid(&mut s, "plt_lot100pt_bears"), want);
        assert_eq!(guid(&mut s, "41a427f27cab4027a8b6c00bc123a0e1"), want);
        assert_eq!(
            guid(&mut s, "plt_nothing"),
            Some(Value::String(String::new()))
        );
    }

    #[test]
    fn hostile_modifiers_and_engine_integers_keep_their_effects_valid() {
        let table = Arc::new(EngineTable::parse(LDF.as_bytes()).unwrap());
        let mut s = Sim::new(Arc::new(Vfs::new()), table, 1);
        let c = s.world.spawn(ObjectKind::Creature, "c");
        let e = run(
            &mut s,
            c,
            "EffectModifyPropertyHostile",
            &[Value::Int(10), Value::Float(-5.0)],
        )
        .unwrap();
        let effect = |s: &Sim, v: &Value| match v {
            Value::Engine {
                handle: Some(hd), ..
            } => match s.world.handle(Some(*hd)) {
                Some(EngineData::Effect(e)) => Some(e.clone()),
                _ => None,
            },
            _ => None,
        };
        let made = effect(&s, &e).expect("a valid effect");
        assert_eq!(
            (made.ty, made.hostile, made.ints[0], made.floats[0]),
            (1012, true, 10, -5.0)
        );
        // Scripts assign the result: it must be the same effect, with the integer kept.
        let back = run(
            &mut s,
            c,
            "SetEffectEngineInteger",
            &[e, Value::Int(2), Value::Int(7)],
        )
        .unwrap();
        let kept = effect(&s, &back).expect("still a valid effect");
        assert_eq!(
            (kept.ty, kept.hostile, kept.engine_ints.clone()),
            (1012, true, vec![(2, 7)])
        );
    }

    #[test]
    fn effect_modifiers_survive_a_script_setting_the_base() {
        let table = Arc::new(EngineTable::parse(LDF.as_bytes()).unwrap());
        let mut s = Sim::new(Arc::new(Vfs::new()), table, 1);
        let c = s.world.spawn(ObjectKind::Creature, "c");
        // A worn amulet's +2 to property 30 (no attribute or equipment bonus there).
        s.world.get_mut(c).unwrap().modifiers.insert(30, 2.0);
        let o = Value::Object(c);
        run(
            &mut s,
            c,
            "SetCreatureProperty",
            &[o.clone(), Value::Int(30), Value::Float(15.0), Value::Int(2)],
        );
        let get = |s: &mut Sim, t: i32| {
            run(
                s,
                c,
                "GetCreatureProperty",
                &[o.clone(), Value::Int(30), Value::Int(t)],
            )
        };
        assert_eq!(get(&mut s, 1), Some(Value::Float(17.0)));
        assert_eq!(get(&mut s, 2), Some(Value::Float(15.0)));
        assert_eq!(get(&mut s, 4), Some(Value::Float(2.0)));
    }

    #[test]
    fn effect_queries_filter_as_documented() {
        let table = Arc::new(EngineTable::parse(LDF.as_bytes()).unwrap());
        let mut s = Sim::new(Arc::new(Vfs::new()), table, 1);
        let c = s.world.spawn(ObjectKind::Creature, "c");
        let (hunter, mage) = (
            s.world.spawn(ObjectKind::Creature, "hunter"),
            s.world.spawn(ObjectKind::Creature, "mage"),
        );
        let bleed = Effect {
            ty: 1017,
            ability: 5,
            creator: Some(hunter),
            duration_type: 2,
            ..Default::default()
        };
        let curse = Effect {
            ty: 1012,
            ability: 7,
            creator: Some(mage),
            duration_type: 3,
            ..Default::default()
        };
        s.world.get_mut(c).unwrap().effects = vec![bleed, curse];
        let (o, i) = (Value::Object, Value::Int);
        let has = |s: &mut Sim, ty, ab| run(s, c, "GetHasEffects", &[o(c), i(ty), i(ab)]);
        // −1, GetHasEffects' default, is no ability filter.
        assert_eq!(has(&mut s, 1017, -1), Some(i(1)));
        assert_eq!(has(&mut s, 1017, 0), Some(i(1)));
        assert_eq!(has(&mut s, 1017, 5), Some(i(1)));
        assert_eq!(has(&mut s, 1017, 9), Some(i(0)));
        assert_eq!(has(&mut s, 1013, -1), Some(i(0)));
        let count = |s: &mut Sim, args: &[Value]| match run(s, c, "GetEffects", args) {
            Some(Value::Array { items, .. }) => items.len(),
            v => panic!("{v:?}"),
        };
        let any = o(OBJECT_INVALID);
        assert_eq!(
            count(&mut s, &[o(c), i(0), i(0), any.clone(), i(0), i(-1)]),
            2
        );
        assert_eq!(count(&mut s, &[o(c), i(0), i(0), o(mage), i(0), i(-1)]), 1);
        assert_eq!(
            count(&mut s, &[o(c), i(0), i(0), any.clone(), i(2), i(-1)]),
            1
        );
        assert_eq!(
            count(&mut s, &[o(c), i(1012), i(0), o(hunter), i(0), i(-1)]),
            0
        );
        // Removing by creator leaves the others' effects.
        run(
            &mut s,
            c,
            "RemoveEffectsByParameters",
            &[o(c), i(0), i(0), o(hunter)],
        );
        let left = &s.world.get(c).unwrap().effects;
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].creator, Some(mage));
    }

    #[test]
    fn effect_flags_combine_the_flags_of_the_effects_held() {
        use thedas_formats::gda::Cell;
        let table = Arc::new(EngineTable::parse(LDF.as_bytes()).unwrap());
        let mut s = Sim::new(Arc::new(Vfs::new()), table, 1);
        let t = crate::m2da::Tables::empty();
        t.insert(
            "effects",
            &["ID", "Label", "Flags"],
            vec![
                (
                    1014,
                    vec![
                        Cell::Int(1014),
                        Cell::String("KNOCKDOWN".into()),
                        Cell::Int(7),
                    ],
                ),
                (
                    1007,
                    vec![
                        Cell::Int(1007),
                        Cell::String("LOCK_INVENTORY".into()),
                        Cell::Int(512),
                    ],
                ),
                (
                    1010,
                    vec![Cell::Int(1010), Cell::String("HEAL".into()), Cell::Int(0)],
                ),
            ],
        );
        s.world.tables = Arc::new(t);
        let c = s.world.spawn(ObjectKind::Creature, "c");
        let flags = |s: &mut Sim| run(s, c, "GetEffectsFlags", &[Value::Object(c)]);
        assert_eq!(flags(&mut s), Some(Value::Int(0)));
        for ty in [1014, 1007, 1010] {
            s.world.get_mut(c).unwrap().effects.push(Effect {
                ty,
                ..Default::default()
            });
        }
        assert_eq!(flags(&mut s), Some(Value::Int(7 | 512)));
    }

    #[test]
    fn an_area_ability_queues_each_targets_impact_back_to_its_script() {
        let table = Arc::new(EngineTable::parse(LDF.as_bytes()).unwrap());
        let mut s = Sim::new(Arc::new(Vfs::new()), table, 1);
        let caster = s.world.spawn(ObjectKind::Creature, "mage");
        let foe = s.world.spawn(ObjectKind::Creature, "foe");
        let area = s.world.spawn(ObjectKind::Area, "area");
        let l = run(
            &mut s,
            caster,
            "Location",
            &[
                Value::Object(area),
                Value::Vector([1.0, 2.0, 3.0]),
                Value::Float(0.0),
            ],
        )
        .unwrap();
        let f = s
            .table
            .function_by_name("SetIndividualImpactAOEEvent")
            .cloned()
            .unwrap();
        let mut h = ScriptHost {
            sim: &mut s,
            event: None,
            script: "spell_aoe_instant.ncs".into(),
        };
        let args = [
            Value::Object(caster),
            Value::Object(foe),
            Value::Int(14000),
            l,
            Value::Int(500),
        ];
        call(&mut h, caster, &f, &args).unwrap().unwrap();
        let q: Vec<_> = s.world.queue.iter().collect();
        assert_eq!(q.len(), 1);
        let ((due, _), e) = q[0];
        assert!((500..650).contains(due), "{due}");
        assert_eq!(e.target, caster);
        assert_eq!(e.script.as_deref(), Some("spell_aoe_instant"));
        assert_eq!(e.event.ty, 1066);
        assert_eq!(e.event.ints, vec![14000]);
        assert_eq!(e.event.objects, vec![caster, foe, area]);
        assert_eq!(e.event.floats, vec![1.0, 2.0, 3.0]);
        assert_eq!(e.event.strings, vec!["spell_aoe_instant".to_string()]);
    }

    #[test]
    fn a_placeables_base_type_comes_from_its_appearance_row() {
        let table = Arc::new(EngineTable::parse(LDF.as_bytes()).unwrap());
        let mut s = Sim::new(Arc::new(Vfs::new()), table, 1);
        let t = crate::m2da::Tables::empty();
        // Placeables (93): appearance 5 is a container (2), 7 a door (1).
        t.insert(
            "#93",
            &["ID", "Label", "BaseType"],
            vec![
                (
                    5,
                    vec![Cell::Int(5), Cell::String("chest".into()), Cell::Int(2)],
                ),
                (
                    7,
                    vec![Cell::Int(7), Cell::String("door".into()), Cell::Int(1)],
                ),
            ],
        );
        s.world.tables = Arc::new(t);
        let spawn = |s: &mut Sim, kind, appearance| {
            let id = s.world.spawn(kind, "x");
            s.world
                .get_mut(id)
                .unwrap()
                .fields
                .insert("Appearance".into(), appearance);
            id
        };
        let chest = spawn(&mut s, ObjectKind::Placeable, 5);
        let door = spawn(&mut s, ObjectKind::Placeable, 7);
        let odd = spawn(&mut s, ObjectKind::Placeable, 99);
        let wolf = spawn(&mut s, ObjectKind::Creature, 5);
        let base = |s: &mut Sim, o| run(s, o, "GetPlaceableBaseType", &[Value::Object(o)]);
        assert_eq!(base(&mut s, chest), Some(Value::Int(2)));
        assert_eq!(base(&mut s, door), Some(Value::Int(1)));
        assert_eq!(base(&mut s, odd), Some(Value::Int(0)));
        assert_eq!(base(&mut s, wolf), Some(Value::Int(0)));
    }

    #[test]
    fn a_successful_action_moves_a_placeable_along_its_state_controller() {
        use thedas_formats::gda::Cell;
        let table = Arc::new(EngineTable::parse(LDF.as_bytes()).unwrap());
        let mut s = Sim::new(Arc::new(Vfs::new()), table, 1);
        let t = crate::m2da::Tables::empty();
        // placeable_types: the ninth column names the state controller.
        let mut cols = vec![
            "ID",
            "Label",
            "c2",
            "c3",
            "c4",
            "c5",
            "c6",
            "c7",
            "Controller",
        ];
        let row = |id: i64, ctl: &str| {
            let mut v: Vec<Cell> = (0..8).map(Cell::Int).collect();
            v[0] = Cell::Int(id);
            v.push(Cell::String(ctl.into()));
            (id, v)
        };
        t.insert("#93", &cols, vec![row(93, "StateCnt_Door")]);
        cols.clear();
        // statecnt_door: default action, next state, the variation's next state.
        let st = |id: i64, act: i64, next: i64, alt: i64| {
            (
                id,
                vec![
                    Cell::Int(id),
                    Cell::Int(act),
                    Cell::Int(next),
                    Cell::Int(alt),
                ],
            )
        };
        t.insert(
            "statecnt_door",
            &["ID", "#52be3c5e", "#a2b4782c", "#5ad0780d"],
            vec![st(0, 1, 2, 4), st(1, 3, 0, 0), st(2, 2, 0, 0)],
        );
        s.world.tables = Arc::new(t);
        let door = s.world.spawn(ObjectKind::Placeable, "door");
        s.world.get_mut(door).unwrap().fields.extend([
            ("Appearance".to_string(), 93),
            ("AnimContState".to_string(), 1),
        ]);
        let state = |s: &mut Sim| run(s, door, "GetPlaceableState", &[Value::Object(door)]);
        let act = |s: &mut Sim, a: i32, ok: i32, var: i32| {
            run(
                s,
                door,
                "SetPlaceableActionResult",
                &[
                    Value::Object(door),
                    Value::Int(a),
                    Value::Int(ok),
                    Value::Int(var),
                ],
            );
        };
        assert_eq!(state(&mut s), Some(Value::Int(1)), "locked at the start");
        act(&mut s, 1, 1, 0);
        assert_eq!(
            state(&mut s),
            Some(Value::Int(1)),
            "opening a locked door does nothing"
        );
        act(&mut s, 3, 0, 0);
        assert_eq!(
            state(&mut s),
            Some(Value::Int(1)),
            "a failed unlock leaves it"
        );
        act(&mut s, 3, 1, 0);
        assert_eq!(state(&mut s), Some(Value::Int(0)));
        act(&mut s, 1, 1, 1);
        assert_eq!(
            state(&mut s),
            Some(Value::Int(4)),
            "the variation's open state"
        );
    }

    #[test]
    fn the_pool_holds_waiting_followers_and_can_share_the_heros_experience() {
        use thedas_formats::gda::Cell;
        let table = Arc::new(EngineTable::parse(LDF.as_bytes()).unwrap());
        let mut s = Sim::new(Arc::new(Vfs::new()), table, 1);
        let t = crate::m2da::Tables::empty();
        t.insert(
            "exptable",
            &["ID", "XP"],
            (1..=3)
                .map(|l| {
                    (
                        l,
                        vec![Cell::Int(l), Cell::Int([0, 0, 2001, 4501][l as usize])],
                    )
                })
                .collect(),
        );
        s.world.tables = Arc::new(t);
        let hero = s.world.spawn(ObjectKind::Creature, "hero");
        s.world.hero = Some(hero);
        let (alistair, dog) = (
            s.world.spawn(ObjectKind::Creature, "alistair"),
            s.world.spawn(ObjectKind::Creature, "dog"),
        );
        run(
            &mut s,
            hero,
            "SetFollowerState",
            &[Value::Object(alistair), Value::Int(1)],
        );
        run(
            &mut s,
            hero,
            "SetFollowerState",
            &[Value::Object(dog), Value::Int(2)],
        );
        let pool = run(&mut s, hero, "GetPartyPoolList", &[]);
        assert_eq!(
            pool,
            Some(objects(vec![hero, alistair, dog])),
            "the dog waits in the pool, out of the active party"
        );
        let xp = |s: &Sim, o| {
            s.world
                .get(o)
                .unwrap()
                .properties
                .get(&19)
                .map_or(0.0, |v| v.0)
        };
        let give = |s: &mut Sim, v: f32| {
            run(
                s,
                hero,
                "SetCreatureProperty",
                &[
                    Value::Object(hero),
                    Value::Int(19),
                    Value::Float(v),
                    Value::Int(2),
                ],
            );
        };
        give(&mut s, 1000.0);
        assert_eq!(
            (xp(&s, alistair), xp(&s, dog)),
            (0.0, 0.0),
            "off by default"
        );
        s.world.pool_xp = true;
        give(&mut s, 2500.0);
        // Only the waiting follower shares it (the active party is paid by the scripts).
        assert_eq!((xp(&s, alistair), xp(&s, dog)), (0.0, 1500.0));
        assert!(!s.world.get(dog).unwrap().fields.contains_key("CanLevelUp"));
        give(&mut s, 3100.0);
        assert_eq!(xp(&s, dog), 2100.0);
        assert_eq!(s.world.get(dog).unwrap().fields.get("CanLevelUp"), Some(&1));
        // Followers do not pass their experience on.
        run(
            &mut s,
            hero,
            "SetCreatureProperty",
            &[
                Value::Object(alistair),
                Value::Int(19),
                Value::Float(9000.0),
                Value::Int(2),
            ],
        );
        assert_eq!(xp(&s, dog), 2100.0);
    }

    #[test]
    fn weapon_sets_hold_their_own_weapons() {
        let table = Arc::new(EngineTable::parse(LDF.as_bytes()).unwrap());
        let mut s = Sim::new(Arc::new(Vfs::new()), table, 1);
        let c = s.world.spawn(ObjectKind::Creature, "militia");
        let (dagger, bow, armour) = (
            s.world.spawn(ObjectKind::Item, "dagger"),
            s.world.spawn(ObjectKind::Item, "bow"),
            s.world.spawn(ObjectKind::Item, "armour"),
        );
        let o = Value::Object;
        run(
            &mut s,
            c,
            "EquipItem",
            &[o(c), o(armour), Value::Int(4), Value::Int(-1)],
        );
        run(
            &mut s,
            c,
            "EquipItem",
            &[o(c), o(dagger), Value::Int(0), Value::Int(0)],
        );
        run(
            &mut s,
            c,
            "EquipItem",
            &[o(c), o(bow), Value::Int(0), Value::Int(1)],
        );
        let main = |s: &mut Sim, set: i32| {
            run(
                s,
                c,
                "GetItemInEquipSlot",
                &[Value::Int(0), o(c), Value::Int(set)],
            )
        };
        // The bow went to the second set; the dagger is in hand.
        assert_eq!(main(&mut s, -1), Some(o(dagger)));
        assert_eq!(main(&mut s, 1), Some(o(bow)));
        let slot = |s: &mut Sim, item| run(s, c, "GetItemEquipSlot", &[o(item)]);
        assert_eq!(slot(&mut s, dagger), Some(Value::Int(0)));
        assert_eq!(slot(&mut s, bow), Some(Value::Int(0)));
        assert_eq!(slot(&mut s, armour), Some(Value::Int(4)));
        run(&mut s, c, "SwitchWeaponSet", &[o(c), Value::Int(1)]);
        assert_eq!(main(&mut s, -1), Some(o(bow)));
        assert_eq!(main(&mut s, 0), Some(o(dagger)));
        assert_eq!(slot(&mut s, dagger), Some(Value::Int(0)));
        assert_eq!(slot(&mut s, bow), Some(Value::Int(0)));
        assert!(s.unequip(c, dagger));
        assert_eq!(slot(&mut s, dagger), Some(Value::Int(255)));
        assert_eq!(slot(&mut s, OBJECT_INVALID), Some(Value::Int(255)));
        assert_eq!(slot(&mut s, c), Some(Value::Int(255)));
        // Preserve DA2's previous active-set-only lookup pending a DA2 trace.
        s.game = thedas_vfs::Game::DragonAge2;
        s.world.get_mut(c).unwrap().other_set.insert(0, dagger);
        assert_eq!(slot(&mut s, dagger), Some(Value::Int(255)));
        assert_eq!(
            s.world.get(c).unwrap().equipment.get(&4),
            Some(&armour),
            "armour stays"
        );
    }

    #[test]
    fn tag_counts_use_stacks_but_not_default_ammunition() {
        let table = Arc::new(EngineTable::parse(LDF.as_bytes()).unwrap());
        let mut s = Sim::new(Arc::new(Vfs::new()), table, 1);
        s.world.tables().insert(
            "bitm_base",
            &["#39974122"],
            vec![(25, vec![Cell::Int(99)]), (2, vec![Cell::Int(1)])],
        );
        let owner = s.world.spawn(ObjectKind::Creature, "owner");
        let elsewhere = s.world.spawn(ObjectKind::Placeable, "chest");
        for (holder, tag, base, size, ammo) in [
            (owner, "poultice", 25, 4, 0),
            (owner, "poultice", 25, 2, 0),
            (owner, "poultice", 2, 99, 0),
            (owner, "poultice", 25, 20, 1),
            (owner, "Poultice", 25, 3, 0),
            (elsewhere, "poultice", 25, 7, 0),
        ] {
            let item = s.world.spawn(ObjectKind::Item, tag);
            let f = &mut s.world.get_mut(item).unwrap().fields;
            f.insert("BaseItem".into(), base);
            f.insert("StackSize".into(), size);
            f.insert("DefaultAmmo".into(), ammo);
            s.world.get_mut(holder).unwrap().inventory.push(item);
        }
        let count = |s: &mut Sim, holder, tag: &str| {
            run(
                s,
                owner,
                "CountItemsByTag",
                &[Value::Object(holder), Value::String(tag.into())],
            )
        };
        assert_eq!(count(&mut s, owner, "poultice"), Some(Value::Int(8)));
        assert_eq!(count(&mut s, owner, "Poultice"), Some(Value::Int(3)));
        assert_eq!(count(&mut s, elsewhere, "poultice"), Some(Value::Int(7)));
        assert_eq!(
            count(&mut s, OBJECT_INVALID, "poultice"),
            Some(Value::Int(0))
        );
        assert_eq!(count(&mut s, owner, "missing"), Some(Value::Int(0)));
        s.game = thedas_vfs::Game::DragonAge2;
        assert_eq!(count(&mut s, owner, "poultice"), Some(Value::Int(5)));
    }

    #[test]
    fn weapon_sets_and_plot_actions_are_remembered() {
        let table = Arc::new(EngineTable::parse(LDF.as_bytes()).unwrap());
        let mut s = Sim::new(Arc::new(Vfs::new()), table, 1);
        let c = s.world.spawn(ObjectKind::Creature, "c");
        let set = |s: &mut Sim| run(s, c, "GetActiveWeaponSet", &[Value::Object(c)]);
        assert_eq!(set(&mut s), Some(Value::Int(0)));
        run(
            &mut s,
            c,
            "SwitchWeaponSet",
            &[Value::Object(c), Value::Int(-1)],
        );
        assert_eq!(
            set(&mut s),
            Some(Value::Int(1)),
            "no set given: the other one"
        );
        run(
            &mut s,
            c,
            "SwitchWeaponSet",
            &[Value::Object(c), Value::Int(1)],
        );
        assert_eq!(set(&mut s), Some(Value::Int(1)));
        run(
            &mut s,
            c,
            "SetPlotActionState",
            &[Value::Int(4), Value::Int(2)],
        );
        assert_eq!(
            run(&mut s, c, "GetPlotActionState", &[Value::Int(4)]),
            Some(Value::Int(2))
        );
    }

    #[test]
    fn inventory_queries_filter_creature_slots_and_backpacks() {
        let table = Arc::new(EngineTable::parse(LDF.as_bytes()).unwrap());
        let mut s = Sim::new(Arc::new(Vfs::new()), table, 1);
        let owner = s.world.spawn(ObjectKind::Creature, "owner");
        let item = |s: &mut Sim, tag: &str, base, plot| {
            let id = s.world.spawn(ObjectKind::Item, tag);
            let x = s.world.get_mut(id).unwrap();
            x.fields.insert("BaseItem".into(), base);
            x.plot = plot;
            id
        };
        let bag = item(&mut s, "loot", 25, false);
        let case = item(&mut s, "Loot", 25, false);
        let plot = item(&mut s, "loot", 25, true);
        let active = item(&mut s, "loot", 2, true);
        let inactive = item(&mut s, "loot", 25, false);
        let armour = item(&mut s, "loot", 2, false);
        let original = vec![plot, bag, inactive, case, armour, active];
        let x = s.world.get_mut(owner).unwrap();
        x.inventory = original.clone();
        x.equipment.insert(0, active);
        x.equipment.insert(4, armour);
        x.other_set.insert(0, inactive);
        let query = |s: &mut Sim, who, options, base, tag: &str, ignore_plot| {
            run(
                s,
                owner,
                "GetItemsInInventory",
                &[
                    Value::Object(who),
                    Value::Int(options),
                    Value::Int(base),
                    Value::String(tag.into()),
                    Value::Int(ignore_plot),
                ],
            )
        };
        let all = vec![active, armour, inactive, bag, case, plot];
        for (options, base, tag, ignore_plot, expected) in [
            (1, 0, "", 0, all.clone()),
            (0, 0, "", 0, vec![bag, case, plot]),
            (2, 0, "", 0, vec![active, armour, inactive]),
            (3, 0, "", 0, all),
            (-1, 0, "", 0, vec![bag, case, plot]),
            (1, 25, "", 0, vec![inactive, bag, case, plot]),
            (1, 999, "", 0, vec![]),
            (1, 0, "loot", 0, vec![active, armour, inactive, bag, plot]),
            (1, 0, "Loot", 0, vec![case]),
            // Plot equipment remains included; only the plot backpack is skipped.
            (1, 0, "", 1, vec![active, armour, inactive, bag, case]),
            (0, 0, "", -1, vec![bag, case]),
        ] {
            assert_eq!(
                query(&mut s, owner, options, base, tag, ignore_plot),
                Some(objects(expected)),
                "options={options}, base={base}, tag={tag:?}, ignore_plot={ignore_plot}"
            );
        }
        // The host also supplies the declared ALL default when called directly.
        assert_eq!(
            run(
                &mut s,
                owner,
                "GetItemsInInventory",
                &[Value::Object(owner)]
            ),
            query(&mut s, owner, 1, 0, "", 0)
        );
        // Containers and stores ignore creature options/base/plot filters.
        for kind in [ObjectKind::Placeable, ObjectKind::Store] {
            let holder = s.world.spawn(kind, "holder");
            let a = item(&mut s, "loot", 2, true);
            let b = item(&mut s, "Loot", 2, false);
            s.world.get_mut(holder).unwrap().inventory = vec![b, a];
            assert_eq!(
                query(&mut s, holder, 2, 999, "loot", 1),
                Some(objects(vec![a]))
            );
            assert_eq!(
                query(&mut s, holder, 2, 999, "", 1),
                Some(objects(vec![b, a]))
            );
        }
        assert_eq!(
            query(&mut s, OBJECT_INVALID, 1, 0, "", 0),
            Some(objects(vec![]))
        );
        let area = s.world.spawn(ObjectKind::Area, "area");
        assert_eq!(query(&mut s, area, 1, 0, "", 0), Some(objects(vec![])));
        // DA2 keeps its previous order and filtering pending its own trace.
        s.game = thedas_vfs::Game::DragonAge2;
        assert_eq!(
            query(&mut s, owner, 2, 999, "loot", 1),
            Some(objects(original))
        );
    }

    #[test]
    fn teams_are_counted_in_the_loaded_area_only() {
        let table = Arc::new(EngineTable::parse(LDF.as_bytes()).unwrap());
        let mut s = Sim::new(Arc::new(Vfs::new()), table, 1);
        let (here, there) = (
            s.world.spawn(ObjectKind::Area, "here"),
            s.world.spawn(ObjectKind::Area, "there"),
        );
        s.world.area = Some(here);
        let mut team = Vec::new();
        for (area, active) in [(here, true), (here, false), (there, true)] {
            let c = s.world.spawn(ObjectKind::Creature, "darkspawn");
            let o = s.world.get_mut(c).unwrap();
            o.team = 16;
            o.area = Some(area);
            o.active = active;
            team.push(c);
        }
        let r = run(&mut s, here, "GetTeam", &[Value::Int(16), Value::Int(32)]);
        assert_eq!(
            r,
            Some(objects(vec![team[0], team[1]])),
            "inactive members count, other areas' do not"
        );
        s.world.get_mut(team[0]).unwrap().dead = true;
        assert_eq!(
            run(&mut s, there, "GetTeam", &[Value::Int(16)]),
            Some(objects(vec![team[0], team[1]])),
            "the default includes dead creatures and uses the loaded area, not the caller"
        );
        let chest = s.world.spawn(ObjectKind::Placeable, "chest");
        let o = s.world.get_mut(chest).unwrap();
        o.team = 16;
        o.area = Some(here);
        // A multi-bit type selects its highest bit's bucket, rather than a union.
        for mask in [512, 512 | 32, 1023] {
            assert_eq!(
                run(&mut s, here, "GetTeam", &[Value::Int(16), Value::Int(mask)]),
                Some(objects(vec![chest]))
            );
        }
        s.world.get_mut(team[1]).unwrap().team = -1;
        for (id, mask) in [(-1, 32), (99, 32), (16, 0), (16, -1)] {
            assert_eq!(
                run(&mut s, here, "GetTeam", &[Value::Int(id), Value::Int(mask)]),
                Some(objects(vec![]))
            );
        }
        s.world.area = Some(there);
        assert_eq!(
            run(&mut s, here, "GetTeam", &[Value::Int(16)]),
            Some(objects(vec![team[2]]))
        );
        // With no loaded area, area-less objects are not a substitute registry.
        s.world.get_mut(team[2]).unwrap().area = None;
        s.world.area = None;
        assert_eq!(
            run(&mut s, here, "GetTeam", &[Value::Int(16)]),
            Some(objects(vec![]))
        );
        // DA2 retains its earlier provisional mask and area-less behavior.
        s.game = thedas_vfs::Game::DragonAge2;
        assert_eq!(
            run(&mut s, here, "GetTeam", &[Value::Int(16), Value::Int(0)]),
            Some(objects(vec![team[2]]))
        );
    }

    #[test]
    fn an_autosave_is_asked_of_the_presentation() {
        let table = Arc::new(EngineTable::parse(LDF.as_bytes()).unwrap());
        let mut s = Sim::new(Arc::new(Vfs::new()), table, 1);
        let m = s.world.spawn(ObjectKind::Module, "module");
        assert!(!s.world.autosave_requested);
        run(&mut s, m, "DoAutoSave", &[]);
        assert!(s.world.autosave_requested);
    }

    #[test]
    fn popups_wait_for_an_answer_and_stores_open() {
        let table = Arc::new(EngineTable::parse(LDF.as_bytes()).unwrap());
        let mut s = Sim::new(Arc::new(Vfs::new()), table, 1);
        let door = s.world.spawn(ObjectKind::Placeable, "exit");
        let store = s.world.spawn(ObjectKind::Store, "store");
        let hero = s.world.spawn(ObjectKind::Creature, "hero");
        s.world.hero = Some(hero);
        let module = s.world.spawn(ObjectKind::Module, "module");
        s.world.module = Some(module);
        run(
            &mut s,
            door,
            "ShowPopup",
            &[Value::Int(321), Value::Int(1), Value::Object(door)],
        );
        assert_eq!(
            s.world.popup.as_ref().map(|p| (p.strref, p.owner)),
            Some((321, door))
        );
        assert!(s.answer_popup(1));
        assert!(s.world.popup.is_none());
        // The answer goes to the module, the popup's owner as object 0 (DAOrigins.exe
        // 0x7098ef, 0x70993b).
        let q: Vec<_> = s
            .world
            .queue
            .values()
            .map(|q| (q.target, q.event.ints.clone(), q.event.objects.clone()))
            .collect();
        assert_eq!(q, vec![(module, vec![1, 1], vec![door])]);
        // Text typed into the box rides along as string 0.
        assert!(
            s.world
                .queue
                .values()
                .all(|q| q.event.strings == vec![String::new()])
        );
        assert!(!s.answer_popup(1), "nothing left to answer");

        run(&mut s, hero, "OpenStore", &[Value::Object(store)]);
        assert_eq!(s.world.store, Some(store));
        let size = |s: &mut Sim| {
            run(
                s,
                hero,
                "GetMaxInventorySize",
                &[Value::Object(OBJECT_INVALID)],
            )
        };
        assert_eq!(size(&mut s), Some(Value::Int(DEFAULT_INVENTORY_SIZE)));
        run(
            &mut s,
            hero,
            "SetMaxInventorySize",
            &[Value::Int(80), Value::Object(OBJECT_INVALID)],
        );
        assert_eq!(size(&mut s), Some(Value::Int(80)));
    }

    #[test]
    fn cooldowns_run_out_and_used_items_leave_the_stack() {
        let table = Arc::new(EngineTable::parse(LDF.as_bytes()).unwrap());
        let mut s = Sim::new(Arc::new(Vfs::new()), table, 1);
        let hero = s.world.spawn(ObjectKind::Creature, "hero");
        let potion = s.world.spawn(ObjectKind::Item, "potion");
        s.world
            .get_mut(potion)
            .unwrap()
            .fields
            .insert("StackSize".into(), 2);
        s.world.get_mut(hero).unwrap().inventory.push(potion);
        let tag = Value::String("Potion".into());
        run(
            &mut s,
            hero,
            "SetCooldown",
            &[
                Value::Object(hero),
                Value::Int(7),
                Value::Float(5.0),
                tag.clone(),
            ],
        );
        s.world.time_ms += 2000;
        let left = run(
            &mut s,
            hero,
            "GetRemainingCooldown",
            &[
                Value::Object(hero),
                Value::Int(7),
                Value::String("potion".into()),
            ],
        );
        assert_eq!(left, Some(Value::Float(3.0)));
        s.world.time_ms += 4000;
        let left = run(
            &mut s,
            hero,
            "GetRemainingCooldown",
            &[Value::Object(hero), Value::Int(7), tag],
        );
        assert_eq!(left, Some(Value::Float(0.0)));

        run(
            &mut s,
            hero,
            "RemoveItem",
            &[Value::Object(potion), Value::Int(1)],
        );
        assert_eq!(
            s.world.get(potion).unwrap().fields.get("StackSize"),
            Some(&1)
        );
        run(
            &mut s,
            hero,
            "RemoveItem",
            &[Value::Object(potion), Value::Int(1)],
        );
        assert!(s.world.get(potion).is_none());
        assert!(s.world.get(hero).unwrap().inventory.is_empty());
    }

    #[test]
    fn default_arguments_mean_what_script_ldf_says() {
        use thedas_formats::gda::Cell;
        let table = Arc::new(EngineTable::parse(LDF.as_bytes()).unwrap());
        let mut s = Sim::new(Arc::new(Vfs::new()), table, 1);
        let t = crate::m2da::Tables::empty();
        let row =
            |id: i64, ty: i64, gui: i64| (id, vec![Cell::Int(id), Cell::Int(ty), Cell::Int(gui)]);
        t.insert(
            "ABI_base",
            &["ID", "AbilityType", "guitype"],
            vec![row(1, 1, 18), row(2, 1, 3), row(3, 2, 18), row(4, 3, 0)],
        );
        s.world.tables = Arc::new(t);
        let (o, i) = (Value::Object, Value::Int);
        let hero = s.world.spawn(ObjectKind::Creature, "hero");
        s.world.get_mut(hero).unwrap().abilities = [1, 2, 3, 4].into_iter().collect();
        let count =
            |s: &mut Sim, ty, gui| run(s, hero, "GetAbilityCount", &[o(hero), i(ty), i(gui)]);
        assert_eq!(count(&mut s, 0, 0), Some(i(4)));
        assert_eq!(count(&mut s, 1, 0), Some(i(2)));
        assert_eq!(count(&mut s, 1, 18), Some(i(1)));
        assert_eq!(count(&mut s, 0, 18), Some(i(2)));
        // One list cleared, then all of them.
        run(&mut s, hero, "CharGen_ClearAbilityList", &[o(hero), i(1)]);
        let left: Vec<i32> = s
            .world
            .get(hero)
            .unwrap()
            .abilities
            .iter()
            .copied()
            .collect();
        assert_eq!(left, vec![3, 4]);
        run(&mut s, hero, "CharGen_ClearAbilityList", &[o(hero), i(0)]);
        assert!(s.world.get(hero).unwrap().abilities.is_empty());
        // Slot −1 is the first slot nothing was set in, once per ability.
        let tag = || Value::String(String::new());
        run(&mut s, hero, "SetQuickslot", &[o(hero), i(0), i(10), tag()]);
        run(
            &mut s,
            hero,
            "SetQuickslot",
            &[o(hero), i(-1), i(11), tag()],
        );
        run(
            &mut s,
            hero,
            "SetQuickslot",
            &[o(hero), i(-1), i(11), tag()],
        );
        let slots = &s.world.get(hero).unwrap().quickslots;
        assert_eq!(slots.len(), 2);
        assert_eq!(slots.get(&1).map(|q| q.ability), Some(11));
        // A count of −1 takes the whole stack.
        let potion = s.world.spawn(ObjectKind::Item, "potion");
        s.world
            .get_mut(potion)
            .unwrap()
            .fields
            .insert("StackSize".into(), 5);
        s.world.get_mut(hero).unwrap().inventory.push(potion);
        run(&mut s, hero, "RemoveItem", &[o(potion), i(-1)]);
        assert!(s.world.get(potion).is_none());
        // A worn item is taken off as it goes, not left in the equipment map.
        let sword = s.world.spawn(ObjectKind::Item, "sword");
        let h = s.world.get_mut(hero).unwrap();
        h.inventory.push(sword);
        h.equipment.insert(1, sword);
        run(&mut s, hero, "RemoveItem", &[o(sword), i(-1)]);
        assert!(s.world.get(sword).is_none());
        assert!(
            !s.world
                .get(hero)
                .unwrap()
                .equipment
                .values()
                .any(|&x| x == sword)
        );
        assert!(s.world.get(hero).unwrap().inventory.is_empty());
    }

    #[test]
    fn locks_read_their_template_fields() {
        let table = Arc::new(EngineTable::parse(LDF.as_bytes()).unwrap());
        let mut s = Sim::new(Arc::new(Vfs::new()), table, 1);
        let door = s.world.spawn(ObjectKind::Placeable, "door");
        let o = s.world.get_mut(door).unwrap();
        o.fields.insert("PickLock".into(), 10000);
        o.fields.insert("KeyRequired".into(), 1);
        o.fields.insert("AutoRemoveKey".into(), 1);
        o.text_fields.insert("KeyName".into(), "gate_key".into());
        let d = [Value::Object(door)];
        assert_eq!(
            run(&mut s, door, "GetPlaceablePickLockLevel", &d),
            Some(Value::Int(10000))
        );
        assert_eq!(
            run(&mut s, door, "GetPlaceableKeyRequired", &d),
            Some(Value::Int(1))
        );
        assert_eq!(
            run(&mut s, door, "GetPlaceableAutoRemoveKey", &d),
            Some(Value::Int(1))
        );
        assert_eq!(
            run(&mut s, door, "GetPlaceableKeyTag", &d),
            Some(Value::String("gate_key".into()))
        );
        let none = [Value::Object(OBJECT_INVALID)];
        assert_eq!(
            run(&mut s, door, "GetPlaceableKeyTag", &none),
            Some(Value::String(String::new()))
        );
    }

    #[test]
    fn the_simulations_profile_has_no_achievements() {
        let table = Arc::new(EngineTable::parse(LDF.as_bytes()).unwrap());
        let mut s = Sim::new(Arc::new(Vfs::new()), table, 1);
        let hero = s.world.spawn(ObjectKind::Creature, "hero");
        let id = || [Value::String("ACH_DALISH".into())];
        assert_eq!(run(&mut s, hero, "UnlockAchievement", &id()), None);
        assert_eq!(
            run(&mut s, hero, "GetHasAchievement", &id()),
            Some(Value::Int(0))
        );
    }

    #[test]
    fn dispatch_cleanup_preserves_hero_defaults_and_presentation_routing() {
        let table = Arc::new(
            EngineTable::parse(
                br#"
          int FOLLOWER_STATE_ACTIVE = 1;
          int GetFollowerState(object o) = 1;
          int GetFollowerSubState(object o) = 2;
          int GetPackage(object o) = 3;
          int GetCreatureRacialType(object o) = 4;
          int GetCombatantType(object o) = 5;
          int GetItemUnique(object o) = 6;
          int GetItemMaterialProgression(object o) = 7;
          void SetCreatureGoreLevel(object o, float f) = 8;
        "#,
            )
            .unwrap(),
        );
        for game in [thedas_vfs::Game::Origins, thedas_vfs::Game::DragonAge2] {
            let mut s = Sim::new(Arc::new(Vfs::new()), table.clone(), 1);
            s.game = game;
            let hero = s.world.spawn(ObjectKind::Creature, "hero");
            let other = s.world.spawn(ObjectKind::Creature, "other");
            s.world.hero = Some(hero);
            let args = [Value::Object(hero)];
            assert!(
                !s.world
                    .get(hero)
                    .unwrap()
                    .fields
                    .contains_key("FollowerState")
            );
            assert_eq!(
                run(&mut s, hero, "GetFollowerState", &args),
                Some(Value::Int(1))
            );
            for who in [other, OBJECT_INVALID] {
                assert_eq!(
                    run(&mut s, hero, "GetFollowerState", &[Value::Object(who)]),
                    Some(Value::Int(0))
                );
            }
            assert_eq!(
                run(&mut s, hero, "GetFollowerSubState", &args),
                Some(Value::Int(0))
            );
            // The removed copies could never supply ACTIVE to any of these getters.
            for (name, field) in [
                ("GetPackage", "Package"),
                ("GetCreatureRacialType", "Race"),
                ("GetCombatantType", "Combatant"),
                ("GetItemUnique", "Unique"),
                ("GetItemMaterialProgression", "MaterialProgress"),
            ] {
                assert_eq!(run(&mut s, hero, name, &args), Some(Value::Int(0)));
                s.world
                    .get_mut(hero)
                    .unwrap()
                    .fields
                    .insert(field.into(), 37);
                assert_eq!(run(&mut s, hero, name, &args), Some(Value::Int(37)));
            }
            // A stored state, including zero, takes priority over the hero fallback.
            for state in [0, 5] {
                s.world
                    .get_mut(hero)
                    .unwrap()
                    .fields
                    .insert("FollowerState".into(), state);
                assert_eq!(
                    run(&mut s, hero, "GetFollowerState", &args),
                    Some(Value::Int(state))
                );
            }
            assert_eq!(
                run(
                    &mut s,
                    hero,
                    "SetCreatureGoreLevel",
                    &[Value::Object(hero), Value::Float(1.0)]
                ),
                None
            );
            assert_eq!(
                s.world.trace.presentation.get("SetCreatureGoreLevel"),
                Some(&1)
            );
            assert!(
                s.world.trace.entries.is_empty(),
                "the later log arm is unreachable"
            );
        }
    }

    #[test]
    fn followers_joining_the_party_take_the_heros_side() {
        let table = Arc::new(EngineTable::parse(LDF.as_bytes()).unwrap());
        let mut s = Sim::new(Arc::new(Vfs::new()), table, 1);
        let hero = s.world.spawn(ObjectKind::Creature, "hero");
        let ally = s.world.spawn(ObjectKind::Creature, "ally");
        let (here, there) = (
            s.world.spawn(ObjectKind::Area, "here"),
            s.world.spawn(ObjectKind::Area, "there"),
        );
        s.world.hero = Some(hero);
        s.world.get_mut(hero).unwrap().area = Some(here);
        let a = s.world.get_mut(ally).unwrap();
        a.group = 2;
        a.area = Some(there);
        a.active = false;
        a.parked = Some(true);
        run(
            &mut s,
            hero,
            "SetFollowerState",
            &[Value::Object(ally), Value::Int(1)],
        );
        assert_eq!(s.world.get(hero).unwrap().followers, vec![ally]);
        let a = s.world.get(ally).unwrap();
        assert_eq!(a.group, 0);
        assert_eq!(
            a.area,
            Some(here),
            "hired from another area, it joins the leader"
        );
        assert!(a.active && a.parked.is_none());
        // The hero counts as an active party member without being given a state.
        let state = |s: &mut Sim, o| run(s, hero, "GetFollowerState", &[Value::Object(o)]);
        assert_eq!(state(&mut s, hero), Some(Value::Int(1)));
        assert_eq!(state(&mut s, ally), Some(Value::Int(1)));
    }

    #[test]
    fn item_abilities_casting_and_the_fade_map() {
        let table = Arc::new(EngineTable::parse(LDF.as_bytes()).unwrap());
        let mut s = Sim::new(Arc::new(Vfs::new()), table, 1);
        let mage = s.world.spawn(ObjectKind::Creature, "mage");
        let rune = s.world.spawn(ObjectKind::Item, "rune");
        let stone = s.world.spawn(ObjectKind::Item, "stone");
        s.world
            .get_mut(rune)
            .unwrap()
            .fields
            .insert("AbilityID".into(), 200011);
        let ability = |s: &mut Sim, o| run(s, mage, "GetItemAbilityId", &[Value::Object(o)]);
        assert_eq!(ability(&mut s, rune), Some(Value::Int(200011)));
        assert_eq!(
            ability(&mut s, stone),
            Some(Value::Int(0)),
            "ABILITY_INVALID"
        );
        let conjuring = |s: &mut Sim| run(s, mage, "IsConjuring", &[Value::Object(mage)]);
        assert_eq!(conjuring(&mut s), Some(Value::Int(0)));
        s.world
            .get_mut(mage)
            .unwrap()
            .commands
            .push(crate::world::Command {
                ty: 0,
                name: "CommandUseAbility".into(),
                target: None,
                location: None,
                seconds: 0.0,
                ints: vec![],
                strings: vec![],
                due: Some(5000),
                from: None,
                approach: None,
                ring_wait: false,
                after_impact_ms: None,
                impacted: false,
                arrived: false,
                path: vec![],
            });
        assert_eq!(
            conjuring(&mut s),
            Some(Value::Int(1)),
            "its cast has started"
        );
        run(&mut s, mage, "OpenFadeMap", &[]);
        assert!(s.world.world_map.open);
        assert_eq!(
            run(&mut s, mage, "GetGraphicsDetailLevel", &[]),
            Some(Value::Int(1))
        );
        let e = run(
            &mut s,
            mage,
            "SetEffectEngineVector",
            &[
                Value::Engine {
                    ty: 3,
                    handle: None,
                },
                Value::Int(0),
                Value::Vector([1.0, 2.0, 3.0]),
            ],
        );
        let Some(Value::Engine { handle, .. }) = e else {
            panic!("{e:?}")
        };
        match s.world.handle(handle) {
            Some(EngineData::Effect(e)) => {
                assert_eq!(e.engine_vectors, vec![(0, [1.0, 2.0, 3.0])])
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_follower_state_set_by_script_raises_no_party_events() {
        let table = Arc::new(EngineTable::parse(LDF.as_bytes()).unwrap());
        let mut s = Sim::new(Arc::new(Vfs::new()), table, 1);
        let hero = s.world.spawn(ObjectKind::Creature, "hero");
        let ally = s.world.spawn(ObjectKind::Creature, "ally");
        let module = s.world.spawn(ObjectKind::Module, "module");
        s.world.hero = Some(hero);
        s.world.module = Some(module);
        let set = |s: &mut Sim, state: i32| {
            run(
                s,
                hero,
                "SetFollowerState",
                &[Value::Object(ally), Value::Int(state)],
            );
        };
        let heard = |s: &mut Sim| -> Vec<(i32, Vec<ObjectId>)> {
            let q: Vec<_> = s
                .world
                .queue
                .values()
                .filter(|q| q.target == module)
                .map(|q| (q.event.ty, q.event.objects.clone()))
                .collect();
            s.world.queue.clear();
            q
        };
        // As DAOrigins.exe's SetFollowerState: the follower joins and leaves the active
        // party, and the module hears nothing (only the party picker raises
        // PARTYMEMBER_ADDED/DROPPED).
        set(&mut s, 1);
        assert!(heard(&mut s).is_empty());
        assert!(s.world.get(hero).unwrap().followers.contains(&ally));
        set(&mut s, 2);
        assert!(heard(&mut s).is_empty());
        assert!(!s.world.get(hero).unwrap().followers.contains(&ally));
    }

    #[test]
    fn tactics_slots_run_from_one_to_the_number_of_tactics() {
        let table = Arc::new(EngineTable::parse(LDF.as_bytes()).unwrap());
        let mut s = Sim::new(Arc::new(Vfs::new()), table, 1);
        let c = s.world.spawn(ObjectKind::Creature, "alistair");
        s.world
            .get_mut(c)
            .unwrap()
            .fields
            .insert("NumTactics".into(), 3);
        for i in 1..=4 {
            let a = [
                Value::Object(c),
                Value::Int(i),
                Value::Int(1),
                Value::Int(1),
                Value::Int(100 + i),
                Value::Int(4),
                Value::Int(617),
            ];
            run(&mut s, c, "SetTacticEntry", &a);
        }
        let cond = |s: &mut Sim, i: i32| {
            run(
                s,
                c,
                "GetTacticCondition",
                &[Value::Object(c), Value::Int(i)],
            )
        };
        assert_eq!(cond(&mut s, 1), Some(Value::Int(101)));
        assert_eq!(
            cond(&mut s, 3),
            Some(Value::Int(103)),
            "the last slot counts"
        );
        assert_eq!(
            cond(&mut s, 4),
            Some(Value::Int(0)),
            "past the number of tactics"
        );
        // Tactics switched off on the tactics screen: there are none to run.
        let num = |s: &mut Sim| run(s, c, "GetNumTactics", &[Value::Object(c)]);
        assert_eq!(num(&mut s), Some(Value::Int(3)));
        s.world
            .get_mut(c)
            .unwrap()
            .fields
            .insert(crate::tactics::FIELD_ENABLED.into(), 0);
        assert_eq!(num(&mut s), Some(Value::Int(0)));
        // A preset is added to the list once.
        for p in [4, 6, 4] {
            run(
                &mut s,
                c,
                "AddTacticPresetID",
                &[Value::Object(c), Value::Int(p)],
            );
        }
        assert_eq!(s.tactic_presets(c), vec![4, 6]);
        // The tactics screen's per-follower target and item.
        let dog = s.world.spawn(ObjectKind::Creature, "dog");
        let potion = s.world.spawn(ObjectKind::Item, "gen_im_qck_health_101");
        {
            let t = &mut s.world.get_mut(c).unwrap().combat.tactics[2];
            t.target_object = Some(dog);
            t.item = Some(potion);
        }
        let slot = |n| [Value::Object(c), Value::Int(n)];
        assert_eq!(
            run(&mut s, c, "GetTacticTargetObject", &slot(2)),
            Some(Value::Object(dog))
        );
        assert_eq!(
            run(&mut s, c, "GetTacticConditionObject", &slot(2)),
            Some(Value::Object(OBJECT_INVALID))
        );
        assert_eq!(
            run(&mut s, c, "GetTacticCommandItemTag", &slot(2)),
            Some(Value::String("gen_im_qck_health_101".into()))
        );
        assert_eq!(
            run(&mut s, c, "GetTacticCommandItemTag", &slot(1)),
            Some(Value::String(String::new()))
        );
    }

    #[test]
    fn mod_mode_sets_properties_as_scripts_do() {
        use crate::commands::{CommandOutcome, CommandSource, SimCommand};
        let table = Arc::new(EngineTable::parse(LDF.as_bytes()).unwrap());
        let mut s = Sim::new(Arc::new(Vfs::new()), table, 1);
        let t = crate::m2da::Tables::empty();
        let row = |id: i64, ty: &str, min: f64, max: f64| {
            (
                id,
                vec![
                    Cell::Int(id),
                    Cell::String(ty.into()),
                    Cell::Float(min),
                    Cell::Float(max),
                ],
            )
        };
        t.insert(
            "properties",
            &["ID", "Type", "Min", "Max"],
            vec![
                row(7, "DEPLETABLE", 1.0, 9999.0),
                row(40, "SIMPLE", -1000.0, 1000.0),
            ],
        );
        s.world.tables = Arc::new(t);
        let c = s.world.spawn(ObjectKind::Creature, "c");
        let set = |s: &mut Sim, property: i32, base: Option<f32>, current: Option<f32>| {
            s.apply(
                CommandSource::ModMode,
                SimCommand::SetProperty {
                    target: c,
                    property,
                    base,
                    current,
                },
            )
        };
        // A depletable: the base first, then the current value, clamped to the new total.
        let out = set(&mut s, 7, Some(50.0), Some(80.0));
        assert_eq!(
            out,
            CommandOutcome::PropertySet {
                base: 0.0,
                current: 0.0
            }
        );
        assert_eq!(s.world.get(c).unwrap().properties[&7], (50.0, 50.0));
        let out = set(&mut s, 7, None, Some(20.0));
        assert_eq!(
            out,
            CommandOutcome::PropertySet {
                base: 50.0,
                current: 50.0
            }
        );
        assert_eq!(s.world.get(c).unwrap().properties[&7], (50.0, 20.0));
        // Undo: what it held, set back.
        set(&mut s, 7, Some(50.0), Some(50.0));
        assert_eq!(s.world.get(c).unwrap().properties[&7], (50.0, 50.0));
        assert_eq!(s.world.command_journal.len(), 3);
        // A simple property has no current value; unknown properties and NaN are refused.
        let refused = |o: CommandOutcome| matches!(o, CommandOutcome::Refused { .. });
        assert!(refused(set(&mut s, 40, None, Some(1.0))));
        assert!(refused(set(&mut s, 99, Some(1.0), None)));
        assert!(refused(set(&mut s, 7, Some(f32::NAN), None)));
        assert!(!refused(set(&mut s, 40, Some(-3.0), None)));
    }

    #[test]
    fn property_values_follow_their_kind() {
        let table = Arc::new(EngineTable::parse(LDF.as_bytes()).unwrap());
        let mut s = Sim::new(Arc::new(Vfs::new()), table, 1);
        let t = crate::m2da::Tables::empty();
        let row = |id: i64, ty: &str, min: f64, max: f64| {
            (
                id,
                vec![
                    Cell::Int(id),
                    Cell::String(ty.into()),
                    Cell::Float(min),
                    Cell::Float(max),
                ],
            )
        };
        t.insert(
            "properties",
            &["ID", "Type", "Min", "Max"],
            vec![
                row(7, "DEPLETABLE", 1.0, 9999.0),
                row(11, "ATTRIBUTE", 0.0, 1000.0),
                row(40, "SIMPLE", -1000.0, 1000.0),
            ],
        );
        s.world.tables = Arc::new(t);
        let c = s.world.spawn(ObjectKind::Creature, "c");
        let call = |s: &mut Sim, f: &str, p: i32, v: f32, ty: i32| {
            let a = [
                Value::Object(c),
                Value::Int(p),
                Value::Float(v),
                Value::Int(ty),
            ];
            run(s, c, f, &a);
        };
        let get = |s: &mut Sim, p: i32, ty: i32| match run(
            s,
            c,
            "GetCreatureProperty",
            &[Value::Object(c), Value::Int(p), Value::Int(ty)],
        ) {
            Some(Value::Float(v)) => v,
            other => panic!("{other:?}"),
        };
        let (total, base, current, modifier) = (1, 2, 3, 4);
        // A simple property keeps a negative base, and has no modifier or current value.
        call(&mut s, "SetCreatureProperty", 40, -0.5, base);
        call(&mut s, "SetCreatureProperty", 40, 3.0, modifier);
        call(&mut s, "SetCreatureProperty", 40, 9.0, current);
        assert_eq!(get(&mut s, 40, total), -0.5);
        assert_eq!(get(&mut s, 40, modifier), 0.0);
        assert_eq!(get(&mut s, 40, current), -1.0);
        // An attribute: setting the total sets the base; current is not kept; modifiers add.
        call(&mut s, "SetCreatureProperty", 11, 4.0, total);
        call(&mut s, "SetCreatureProperty", 11, 99.0, current);
        call(&mut s, "SetCreatureProperty", 11, -6.0, modifier);
        assert_eq!(get(&mut s, 11, base), 4.0);
        assert_eq!(get(&mut s, 11, current), -1.0);
        assert_eq!(get(&mut s, 11, total), 0.0, "4 - 6, within the minimum 0");
        call(&mut s, "UpdateCreatureProperty", 11, 3.0, modifier);
        assert_eq!(get(&mut s, 11, total), 1.0);
        call(&mut s, "UpdateCreatureProperty", 11, 50.0, base);
        assert_eq!(get(&mut s, 11, base), 4.0, "updating the base does nothing");
        // A depletable's current value stays within its total.
        call(&mut s, "SetCreatureProperty", 7, 100.0, base);
        call(&mut s, "SetCreatureProperty", 7, 150.0, current);
        assert_eq!(get(&mut s, 7, current), 100.0);
        call(&mut s, "UpdateCreatureProperty", 7, -30.0, current);
        assert_eq!(get(&mut s, 7, current), 70.0);
        call(&mut s, "SetCreatureProperty", 7, 50.0, base);
        assert_eq!(
            get(&mut s, 7, current),
            50.0,
            "lowering the maximum lowers what is left"
        );
    }

    #[test]
    fn giving_the_dead_health_brings_them_back() {
        let table = Arc::new(EngineTable::parse(LDF.as_bytes()).unwrap());
        let mut s = Sim::new(Arc::new(Vfs::new()), table, 1);
        let hero = s.world.spawn(ObjectKind::Creature, "hero");
        let o = s.world.get_mut(hero).unwrap();
        o.dead = true;
        o.died_at = Some(10);
        o.properties.insert(7, (100.0, 0.0));
        o.locals.ints.insert("CREATURE_SPAWN_DEAD".into(), 1);
        let set = |s: &mut Sim, v: f32, t: i32| {
            let a = [
                Value::Object(hero),
                Value::Int(7),
                Value::Float(v),
                Value::Int(t),
            ];
            run(s, hero, "SetCreatureProperty", &a);
        };
        set(&mut s, 120.0, 2);
        assert!(
            s.world.get(hero).unwrap().dead,
            "raising the maximum is not enough"
        );
        set(&mut s, 25.0, 3);
        let o = s.world.get(hero).unwrap();
        assert!(!o.dead);
        assert_eq!(o.died_at, None);
        assert_eq!(o.properties[&7], (120.0, 25.0));
        assert_eq!(
            o.locals.ints["CREATURE_SPAWN_DEAD"], 0,
            "one placed dead is no longer"
        );
    }

    #[test]
    fn items_scripts_create_on_creatures_drop_unless_told_otherwise() {
        let table = Arc::new(EngineTable::parse(LDF.as_bytes()).unwrap());
        let mut s = Sim::new(Arc::new(Vfs::new()), table, 1);
        let foe = s.world.spawn(ObjectKind::Creature, "foe");
        let make = |s: &mut Sim, d: i32| {
            let a = [
                Value::Resource("blood.uti".into()),
                Value::Object(foe),
                Value::Int(2),
                Value::String(String::new()),
                Value::Int(0),
                Value::Int(d),
            ];
            match run(s, foe, "CreateItemOnObject", &a) {
                Some(Value::Object(i)) => i,
                other => panic!("{other:?}"),
            }
        };
        let (kept, dropped) = (make(&mut s, 0), make(&mut s, 1));
        // An item that does not stack (no base item here) comes as two separate items.
        assert_eq!(s.world.get(dropped).unwrap().fields.get("StackSize"), None);
        let bag = s.spawn_body_bag(foe, false).unwrap();
        assert_eq!(
            s.world.get(bag).unwrap().inventory,
            vec![dropped, dropped + 1]
        );
        assert_eq!(s.world.get(foe).unwrap().inventory, vec![kept, kept + 1]);
    }

    #[test]
    fn tag_lookups_prefer_the_current_area_and_areas_search_themselves() {
        let table = Arc::new(EngineTable::parse(LDF.as_bytes()).unwrap());
        let mut s = Sim::new(Arc::new(Vfs::new()), table, 1);
        let wilds = s.world.spawn(ObjectKind::Area, "wilds");
        let hut = s.world.spawn(ObjectKind::Area, "hut");
        let there = s.world.spawn(ObjectKind::Creature, "morrigan");
        let here = s.world.spawn(ObjectKind::Creature, "morrigan");
        s.world.get_mut(there).unwrap().area = Some(wilds);
        s.world.get_mut(here).unwrap().area = Some(hut);
        s.world.area = Some(hut);
        let tag = Value::String("morrigan".into());
        let by_tag = run(&mut s, hut, "GetObjectByTag", &[tag.clone(), Value::Int(0)]);
        assert_eq!(by_tag, Some(Value::Object(here)));
        let none = Value::String(String::new());
        let untagged = s.world.spawn(ObjectKind::Placeable, "");
        s.world.get_mut(untagged).unwrap().area = Some(hut);
        let by_tag = run(&mut s, hut, "GetObjectByTag", &[none, Value::Int(0)]);
        assert_eq!(
            by_tag,
            Some(Value::Object(OBJECT_INVALID)),
            "an empty tag names nothing"
        );
        let near = run(
            &mut s,
            hut,
            "GetNearestObjectByTag",
            &[
                Value::Object(hut),
                tag,
                Value::Int(32),
                Value::Int(1),
                Value::Int(0),
                Value::Int(0),
                Value::Int(0),
            ],
        );
        match near {
            Some(Value::Array { items, .. }) => assert_eq!(items, vec![Value::Object(here)]),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn on_autopilot_the_party_ai_drives_the_hero_too() {
        let table = Arc::new(EngineTable::parse(LDF.as_bytes()).unwrap());
        let mut s = Sim::new(Arc::new(Vfs::new()), table, 1);
        let hero = s.world.spawn(ObjectKind::Creature, "hero");
        s.world.hero = Some(hero);
        let ask = |s: &mut Sim, f| run(s, hero, f, &[Value::Object(hero)]);
        assert_eq!(ask(&mut s, "IsControlled"), Some(Value::Int(1)));
        assert_eq!(ask(&mut s, "IsPartyAIEnabled"), Some(Value::Int(0)));
        s.world.autopilot = true;
        assert_eq!(ask(&mut s, "IsControlled"), Some(Value::Int(0)));
        assert_eq!(ask(&mut s, "IsPartyAIEnabled"), Some(Value::Int(1)));
    }

    #[test]
    fn helpers() {
        assert_eq!(
            command_constant("CommandMoveToObject"),
            "COMMAND_TYPE_MOVE_TO_OBJECT"
        );
        assert_eq!(command_constant("CommandWait"), "COMMAND_TYPE_WAIT");
        assert_eq!(
            command_constant("CommandDeathBlow"),
            "COMMAND_TYPE_DEATHBLOW"
        );
        assert_eq!(
            command_constant("CommandSwitchWeaponSet"),
            "COMMAND_TYPE_SWITCH_WEAPON_SETS"
        );
        let table = thedas_script::EngineTable::default();
        assert!(UNNAMED_COMMANDS.iter().all(|(n, _)| command_type(
            &table,
            n,
            thedas_vfs::Game::DragonAge2
        ) >= 1000));
        assert_eq!(format_float(1.5, 0, 6), "1.500000");
        assert_eq!(format_float(2.0, 6, 1), "   2.0");
        let square = [
            [0.0, 0.0, 0.0],
            [2.0, 0.0, 0.0],
            [2.0, 2.0, 0.0],
            [0.0, 2.0, 0.0],
        ];
        assert!(inside([1.0, 1.0, 5.0], &square));
        assert!(!inside([3.0, 1.0, 0.0], &square));
    }
}

/// Engine functions taken off the coverage list; their own script header, so ids do not
/// collide with `tests`.
#[cfg(test)]
mod coverage_tests {
    use std::sync::Arc;

    use thedas_script::EngineTable;
    use thedas_vfs::Vfs;

    use super::*;

    const LDF: &str = "int FALSE = 0;\n\
        int TRUE = 1;\n\
        int EFFECT_DURATION_TYPE_INSTANT = 1;\n\
        int EFFECT_DURATION_TYPE_TEMPORARY = 2;\n\
        int EFFECT_DURATION_TYPE_PERMANENT = 3;\n\
        #define ENGINE_STRUCTURE_1 location\n\
        #define ENGINE_STRUCTURE_3 effect\n\
        location Location(object a, vector v, float f) = 41;\n\
        void Engine_ApplyEffectOnParty(int d, effect e, float f = 0.0, object c = OBJECT_INVALID, int n = 0, int x = FALSE) = 70;\n\
        object[] GetNearestObjectToLocation(location l, int t = 4294967295, int n = 1) = 71;\n\
        int GetCreatureMana(object o) = 72;\n\
        void SetCreatureMana(object o, int n) = 73;\n\
        int GetCreatureMaxMana(object o) = 74;\n\
        void RemoveAllEffects(object o, int i = TRUE, int d = FALSE) = 75;\n\
        void SetCreatureIsGhost(object o, int v) = 76;\n\
        void SetQuickslotBar(object o, int n) = 77;\n\
        void LockQuickslotBar(object o, int b) = 78;\n\
        int DialogIsAmbient() = 79;\n\
        int ClearAmbientDialogs(object o = OBJECT_INVALID) = 80;\n\
        void RevealCurrentMap() = 81;\n\
        int GetAutoPauseCombatStatus() = 82;\n\
        location GetClusterCenter(object c, int a, int n, int f, int b) = 83;\n\
        object[] GetObjectsInShape(int m, int s, location l, float a, float b = 0.0, float c = 0.0, int d = FALSE) = 84;\n\
        object[] FilterObjectsInShape(object[] o, int m, int s, location l, float a, float b = 0.0, float c = 0.0, int d = FALSE) = 85;\n\
        vector AngleToVector(float f) = 86;\n\
        float VectorToAngle(vector v) = 87;\n\
        string VectorToString(vector v) = 88;\n\
        location SetLocationPosition(location l, vector v) = 89;\n\
        location SetLocationFacing(location l, float f) = 90;\n\
        vector GetPositionFromLocation(location l) = 91;\n\
        float GetFacingFromLocation(location l) = 92;\n\
        void AddItemVisualEffect(object o, int n) = 93;\n\
        int GetItemPropertyPower(object o, int p, int s = FALSE) = 94;\n\
        void DisplayPortraitMessage(object o, string s, int c = 16777215) = 95;\n\
        void SetLocation(object o, location l) = 96;\n\
        resource GetCurrentScriptResource() = 97;\n\
        void SetItemIndestructible(object o, int b) = 98;\n\
        void LogStoryEvent(int n) = 99;\n\
        int CreatePool(resource r, int n) = 100;\n\
        object GetCreatureBodyBag(object o) = 101;\n\
        void ActivateSoundByTag(string s, int b) = 102;\n\
        void AddItemProperty(object o, int p, int n) = 103;\n\
        void RemoveItemProperty(object o, int p) = 104;\n\
        int[] GetItemProperties(object o, int s = FALSE) = 105;\n\
        void PlaySound(object o, string s) = 106;\n\
        void PlayMusic(string s) = 107;\n\
        void DestroyObject(object o, int n = 0) = 108;\n\
        void SetSoundSet(object o, string s) = 109;\n\
        void SetMusicIntensity(int n) = 110;\n\
        void Engine_ApplyEffectAtLocation(int d, effect e, location l, float f = 0.0, object c = OBJECT_INVALID, int n = 0) = 111;\n\
        effect[] GetEffects(object o, int t = 0, int a = 0, object c = OBJECT_INVALID, int d = 0, int i = -1) = 112;\n\
        void RemoveEffect(object o, effect e) = 113;\n\
        void SetGameMode(int n) = 114;\n\
        void SetZoomLevel(float f) = 115;\n\
        void SetLoadHint(int n, int t = 206) = 116;\n\
        void ApplyEffectVisualEffect(object c, object t, int v, int d, float f, int a = 0) = 117;\n\
        void SetDeathHint(int n, int t = 205) = 118;\n\
        void AddAbility(object o, int a, int n = FALSE) = 119;\n\
        void ShowSpecUnlockedNotification(int s) = 120;\n\
        void ShowAreaUnlockedNotification(string a, string i = \"\") = 121;\n";

    #[test]
    fn da2_area_load_visual_replays_reconcile_saved_copies() {
        let table = EngineTable::parse(
            format!("{LDF}\nint EVENT_TYPE_AREALOADSAVE_POSTLOADEXIT = 43;\n").as_bytes(),
        )
        .unwrap();
        let mut s = Sim::new(Arc::new(Vfs::new()), Arc::new(table), 7);
        s.game = thedas_vfs::Game::DragonAge2;
        let area = s.world.spawn(ObjectKind::Area, "area");
        let wall = s.world.spawn(ObjectKind::Placeable, "wall");
        s.world.get_mut(wall).unwrap().area = Some(area);
        let fire = Effect {
            ty: 4,
            creator: Some(area),
            duration_type: 3,
            ints: vec![600001],
            ..Default::default()
        };
        let other = Effect {
            ints: vec![600002],
            ..fire.clone()
        };
        // An older snapshot may already contain several copies. Only the exact
        // environmental visual reissued by the load handler is reconciled.
        s.world.get_mut(wall).unwrap().effects = vec![fire.clone(), other.clone(), fire.clone()];
        for _ in 0..3 {
            s.load_game(s.save_game()).unwrap();
            let event = s.new_event("AREALOADSAVE_POSTLOADEXIT", None);
            let event = Some(s.world.store(EngineData::Event(event)));
            let mut h = ScriptHost {
                sim: &mut s,
                event,
                script: "area".into(),
            };
            apply_visual_or_effect(&mut h, wall, fire.clone());
            assert_eq!(
                s.world.get(wall).unwrap().effects,
                [fire.clone(), other.clone()]
            );
        }
    }

    #[test]
    fn area_load_visual_reconciliation_preserves_other_effect_applications() {
        for case in 0..7 {
            let table = EngineTable::parse(
                format!(
                    "{LDF}\nint EVENT_TYPE_AREALOADSAVE_POSTLOADEXIT = 43;\n\
                     int EVENT_TYPE_AREALOAD_POSTLOADEXIT = 39;\n"
                )
                .as_bytes(),
            )
            .unwrap();
            let mut s = Sim::new(Arc::new(Vfs::new()), Arc::new(table), 7);
            s.game = thedas_vfs::Game::DragonAge2;
            let area = s.world.spawn(ObjectKind::Area, "area");
            let caster = s.world.spawn(ObjectKind::Creature, "caster");
            let target = s.world.spawn(ObjectKind::Placeable, "target");
            s.world.get_mut(target).unwrap().area = Some(area);
            let mut effect = Effect {
                ty: 4,
                creator: Some(area),
                duration_type: 3,
                ints: vec![600001],
                ..Default::default()
            };
            let mut event = s.new_event("AREALOADSAVE_POSTLOADEXIT", None);
            match case {
                0 => s.game = thedas_vfs::Game::Origins,
                1 => event = s.new_event("AREALOAD_POSTLOADEXIT", None),
                2 => effect.ability = 123,
                3 => {
                    effect.duration_type = 2;
                    effect.expires = Some(1000);
                }
                4 => effect.creator = Some(caster),
                5 => s.world.get_mut(target).unwrap().kind = ObjectKind::Creature,
                _ => s.world.get_mut(target).unwrap().area = None,
            }
            s.world
                .get_mut(target)
                .unwrap()
                .effects
                .push(effect.clone());
            let event = Some(s.world.store(EngineData::Event(event)));
            let mut h = ScriptHost {
                sim: &mut s,
                event,
                script: "area".into(),
            };
            apply_visual_or_effect(&mut h, target, effect);
            assert_eq!(s.world.get(target).unwrap().effects.len(), 2, "case {case}");
        }
    }

    #[test]
    fn visual_effects_on_objects_are_applied_as_effects() {
        let mut s = sim();
        let area = s.world.spawn(ObjectKind::Area, "a");
        let (mage, vein) = (
            s.world.spawn(ObjectKind::Creature, "mage"),
            s.world.spawn(ObjectKind::Placeable, "vein"),
        );
        for id in [mage, vein] {
            s.world.get_mut(id).unwrap().area = Some(area);
        }
        let apply = |s: &mut Sim, vfx: i32, duration_type: i32, seconds: f32| {
            run(
                s,
                mage,
                "ApplyEffectVisualEffect",
                &[
                    Value::Object(mage),
                    Value::Object(vein),
                    Value::Int(vfx),
                    Value::Int(duration_type),
                    Value::Float(seconds),
                    Value::Int(200),
                ],
            );
        };
        // A glow for three seconds, a lasting one, and an instant flash.
        apply(&mut s, 1021, 2, 3.0);
        apply(&mut s, 1022, 3, 0.0);
        apply(&mut s, 77, 1, 0.0);
        let kept: Vec<(i32, i32, Option<ObjectId>, i32)> = s
            .world
            .get(vein)
            .unwrap()
            .effects
            .iter()
            .map(|e| (e.ty, e.ints[0], e.creator, e.ability))
            .collect();
        assert_eq!(
            kept,
            vec![(4, 1021, Some(mage), 200), (4, 1022, Some(mage), 200)]
        );
        let flash = &s.world.instant_vfx;
        assert_eq!((flash.len(), flash[0].target, flash[0].vfx), (1, vein, 77));
        s.tick(3_500);
        let left: Vec<i32> = s
            .world
            .get(vein)
            .unwrap()
            .effects
            .iter()
            .map(|e| e.ints[0])
            .collect();
        assert_eq!(left, vec![1022], "the glow has run out");
        assert!(s.world.instant_vfx.is_empty(), "the flash note has gone");
    }

    #[test]
    fn abilities_map_areas_and_specializations_raise_large_alerts() {
        use crate::journal::{ALERT_ABILITY, ALERT_SPECIALIZATION, ALERT_WORLD_MAP};
        let mut s = sim();
        let hero = s.world.spawn(ObjectKind::Creature, "player");
        let stranger = s.world.spawn(ObjectKind::Creature, "stranger");
        s.world.hero = Some(hero);
        let t = s.world.tables();
        t.insert(
            "ABI_base",
            &["ID", "NameStrRef", "Icon"],
            vec![(
                10001,
                vec![
                    Cell::Int(10001),
                    Cell::Int(500),
                    Cell::String("ico_winters_grasp".into()),
                ],
            )],
        );
        t.insert(
            "CLA_base",
            &["ID", "NameStrRef", "Icon"],
            vec![(
                4,
                vec![
                    Cell::Int(4),
                    Cell::Int(501),
                    Cell::String("ico_spec_shapeshifter".into()),
                ],
            )],
        );
        let _ = s.strings.set(
            [(500, "Winter's Grasp"), (501, "Shapeshifter")]
                .into_iter()
                .map(|(k, v)| (k, v.to_string()))
                .collect(),
        );
        let add = |s: &mut Sim, who: ObjectId, notify: i32| {
            run(
                s,
                who,
                "AddAbility",
                &[Value::Object(who), Value::Int(10001), Value::Int(notify)],
            );
        };
        // Only with bSendNotification, and only for a party member.
        add(&mut s, hero, 0);
        add(&mut s, stranger, 1);
        assert!(s.world.uber_alerts.is_empty());
        add(&mut s, hero, 1);
        run(
            &mut s,
            hero,
            "ShowSpecUnlockedNotification",
            &[Value::Int(4)],
        );
        run(
            &mut s,
            hero,
            "ShowAreaUnlockedNotification",
            &[Value::String("Soldier's Peak".into())],
        );
        type Shown<'a> = (i32, Option<&'a str>, Option<&'a str>, Option<ObjectId>);
        let got: Vec<Shown> = s
            .world
            .uber_alerts
            .iter()
            .map(|a| (a.kind, a.text.as_deref(), a.icon.as_deref(), a.creature))
            .collect();
        assert_eq!(
            got,
            [
                (
                    ALERT_ABILITY,
                    Some("Winter's Grasp"),
                    Some("ico_winters_grasp"),
                    Some(hero)
                ),
                (
                    ALERT_SPECIALIZATION,
                    Some("Shapeshifter"),
                    Some("ico_spec_shapeshifter"),
                    None
                ),
                (ALERT_WORLD_MAP, Some("Soldier's Peak"), None, None),
            ]
        );
        assert!(s.world.alerts.is_empty(), "none of them is a small alert");
    }

    #[test]
    fn a_load_hint_waits_for_the_next_transition() {
        let mut s = sim();
        let hero = s.world.spawn(ObjectKind::Creature, "hero");
        run(&mut s, hero, "SetLoadHint", &[Value::Int(12)]);
        assert_eq!(s.world.load_hint, Some((12, 206)));
        run(
            &mut s,
            hero,
            "SetLoadHint",
            &[Value::Int(3), Value::Int(271)],
        );
        assert_eq!(s.world.load_hint, Some((3, 271)));
        s.area_transition("nowhere", "");
        assert_eq!(s.world.load_hint, None);
        assert_eq!(s.world.transition_hint, Some((3, 271)));
        // The death screen's hint is kept the same way, from its own tables.
        run(&mut s, hero, "SetDeathHint", &[Value::Int(4)]);
        assert_eq!(s.world.death_hint, Some((4, 205)));
        run(
            &mut s,
            hero,
            "SetDeathHint",
            &[Value::Int(2), Value::Int(272)],
        );
        assert_eq!(s.world.death_hint, Some((2, 272)));
    }

    fn sim() -> Sim {
        let table = Arc::new(EngineTable::parse(LDF.as_bytes()).unwrap());
        Sim::new(Arc::new(Vfs::new()), table, 1)
    }

    /// Calls engine function `name` as `caller`.
    fn run(s: &mut Sim, caller: ObjectId, name: &str, a: &[Value]) -> Option<Value> {
        let f = s.table.function_by_name(name).cloned().unwrap();
        let mut h = ScriptHost {
            sim: s,
            event: None,
            script: "test".into(),
        };
        call(&mut h, caller, &f, a).unwrap().unwrap()
    }

    fn effect_value(s: &mut Sim, e: Effect) -> Value {
        let hd = s.world.store(EngineData::Effect(e));
        Value::Engine {
            ty: s.types.effect,
            handle: Some(hd),
        }
    }

    #[test]
    fn visual_effects_at_places_are_kept_until_they_end_or_are_removed() {
        let mut s = sim();
        let area = s.world.spawn(ObjectKind::Area, "a");
        let hero = s.world.spawn(ObjectKind::Creature, "hero");
        s.world.get_mut(hero).unwrap().area = Some(area);
        let vfx = |s: &mut Sim, row: i32| {
            effect_value(
                s,
                Effect {
                    ty: 4,
                    ints: vec![row],
                    ..Default::default()
                },
            )
        };
        let place = |s: &mut Sim, x: f32| {
            let hd = s.world.store(EngineData::Location(crate::world::Location {
                area,
                position: [x, 0.0, 0.0],
                facing: 90.0,
            }));
            Value::Engine {
                ty: s.types.location,
                handle: Some(hd),
            }
        };
        let (crowd, flash) = (vfx(&mut s, 1234), vfx(&mut s, 77));
        let (here, there) = (place(&mut s, 1.0), place(&mut s, 2.0));
        // A permanent crowd and a two-second flash.
        run(
            &mut s,
            hero,
            "Engine_ApplyEffectAtLocation",
            &[Value::Int(3), crowd, here, Value::Float(0.0)],
        );
        run(
            &mut s,
            hero,
            "Engine_ApplyEffectAtLocation",
            &[Value::Int(2), flash, there, Value::Float(2.0)],
        );
        let v = &s.world.location_vfx;
        assert_eq!(v.len(), 2);
        assert_eq!(
            (v[0].vfx, v[0].position, v[0].facing, v[0].duration_ms),
            (1234, [1.0, 0.0, 0.0], 90.0, None)
        );
        assert_eq!((v[1].vfx, v[1].duration_ms), (77, Some(2000)));
        s.tick(2500);
        assert_eq!(s.world.location_vfx.len(), 1, "the flash has run out");
        // The area's effects list it, and removing it from the area takes it away.
        let found = match run(&mut s, hero, "GetEffects", &[Value::Object(area)]) {
            Some(Value::Array { items, .. }) => items,
            v => panic!("{v:?}"),
        };
        assert_eq!(found.len(), 1);
        run(
            &mut s,
            hero,
            "RemoveEffect",
            &[Value::Object(area), found[0].clone()],
        );
        assert!(s.world.location_vfx.is_empty());
    }

    #[test]
    fn a_mode_asked_for_during_a_conversation_waits_for_its_end() {
        let mut s = sim();
        let module = s.world.spawn(ObjectKind::Module, "m");
        s.world.module = Some(module);
        let hero = s.world.spawn(ObjectKind::Creature, "hero");
        // A conversation holds the game (mode 4), having replaced exploring (0).
        s.world.game_mode = 4;
        s.world.mode_before_talk = Some(0);
        run(&mut s, hero, "SetGameMode", &[Value::Int(1)]);
        assert_eq!(s.world.game_mode, 4, "combat waits");
        assert_eq!(s.world.mode_after_talk, Some(1));
        // No conversation is going on any more: the waiting mode comes in.
        s.tick(10);
        assert_eq!(s.world.game_mode, 1);
        assert_eq!(s.world.mode_before_talk, None);
        let changes: Vec<Vec<i32>> = s
            .world
            .queue
            .values()
            .filter(|q| q.target == module)
            .map(|q| q.event.ints.clone())
            .collect();
        assert!(
            changes.is_empty() || changes.contains(&vec![1, 4]),
            "{changes:?}"
        );
    }

    #[test]
    fn a_destroyed_item_leaves_its_holder() {
        let mut s = sim();
        let hero = s.world.spawn(ObjectKind::Creature, "hero");
        let (worn, carried) = (
            s.world.spawn(ObjectKind::Item, "clothes"),
            s.world.spawn(ObjectKind::Item, "ring"),
        );
        {
            let o = s.world.get_mut(hero).unwrap();
            o.inventory = vec![worn, carried];
            o.equipment.insert(4, worn);
        }
        for i in [worn, carried] {
            run(
                &mut s,
                hero,
                "DestroyObject",
                &[Value::Object(i), Value::Int(0)],
            );
        }
        let o = s.world.get(hero).unwrap();
        assert!(o.inventory.is_empty());
        assert!(o.equipment.is_empty());
        assert!(s.world.get(worn).is_none() && s.world.get(carried).is_none());
    }

    #[test]
    fn a_party_effect_reaches_every_member_but_an_excluded_creator() {
        let mut s = sim();
        let hero = s.world.spawn(ObjectKind::Creature, "hero");
        let follower = s.world.spawn(ObjectKind::Creature, "follower");
        let stranger = s.world.spawn(ObjectKind::Creature, "stranger");
        s.world.hero = Some(hero);
        s.world.get_mut(hero).unwrap().followers.push(follower);
        let e = effect_value(
            &mut s,
            Effect {
                ty: 1012,
                ..Default::default()
            },
        );
        let apply = |s: &mut Sim, exclude: i32| {
            run(
                s,
                hero,
                "Engine_ApplyEffectOnParty",
                &[
                    Value::Int(2),
                    e.clone(),
                    Value::Float(5.0),
                    Value::Object(hero),
                    Value::Int(300),
                    Value::Int(exclude),
                ],
            );
        };
        apply(&mut s, 1);
        let count = |s: &Sim, o: ObjectId| s.world.get(o).unwrap().effects.len();
        assert_eq!(
            (count(&s, hero), count(&s, follower), count(&s, stranger)),
            (0, 1, 0)
        );
        let got = s.world.get(follower).unwrap().effects[0].clone();
        assert_eq!(
            (got.creator, got.ability, got.duration_type),
            (Some(hero), 300, 2)
        );
        assert_eq!(got.expires, Some(s.world.time_ms + 5000));
        apply(&mut s, 0);
        assert_eq!((count(&s, hero), count(&s, follower)), (1, 2));
    }

    fn location(s: &mut Sim, area: ObjectId, p: [f32; 3]) -> Value {
        run(
            s,
            OBJECT_INVALID,
            "Location",
            &[Value::Object(area), Value::Vector(p), Value::Float(0.0)],
        )
        .unwrap()
    }

    #[test]
    fn the_nearest_objects_to_a_location_are_of_its_area_and_type() {
        let mut s = sim();
        let area = s.world.spawn(ObjectKind::Area, "area");
        let elsewhere = s.world.spawn(ObjectKind::Area, "elsewhere");
        let put = |s: &mut Sim, kind, area, x: f32, dead| {
            let o = s.world.spawn(kind, "o");
            let obj = s.world.get_mut(o).unwrap();
            obj.area = Some(area);
            obj.position = [x, 0.0, 0.0];
            obj.dead = dead;
            o
        };
        let far = put(&mut s, ObjectKind::Creature, area, 9.0, false);
        let near_dead = put(&mut s, ObjectKind::Creature, area, 1.0, true);
        let mid = put(&mut s, ObjectKind::Creature, area, 4.0, false);
        let chest = put(&mut s, ObjectKind::Placeable, area, 0.5, false);
        put(&mut s, ObjectKind::Creature, elsewhere, 0.0, false);
        let l = location(&mut s, area, [0.0; 3]);
        let nearest = |s: &mut Sim, mask: i64, n: i32| {
            run(
                s,
                OBJECT_INVALID,
                "GetNearestObjectToLocation",
                &[l.clone(), Value::Int(mask as i32), Value::Int(n)],
            )
        };
        // Creatures only (32), the dead included, nearest first.
        assert_eq!(
            nearest(&mut s, 32, 5),
            Some(objects(vec![near_dead, mid, far]))
        );
        // Fewer than one asked for means one; OBJECT_TYPE_ALL takes the placeable too.
        assert_eq!(nearest(&mut s, 4294967295, 0), Some(objects(vec![chest])));
        assert_eq!(nearest(&mut s, 0, 3), Some(objects(Vec::new())));
    }

    #[test]
    fn mana_is_the_mana_stamina_property() {
        let mut s = sim();
        let mage = s.world.spawn(ObjectKind::Creature, "mage");
        let chest = s.world.spawn(ObjectKind::Placeable, "chest");
        s.world
            .get_mut(mage)
            .unwrap()
            .properties
            .insert(PROPERTY_MANA_STAMINA, (40.0, 12.7));
        let get = |s: &mut Sim, f: &str, o: ObjectId| run(s, o, f, &[Value::Object(o)]);
        assert_eq!(get(&mut s, "GetCreatureMana", mage), Some(Value::Int(12)));
        assert_eq!(
            get(&mut s, "GetCreatureMaxMana", mage),
            Some(Value::Int(40))
        );
        assert_eq!(get(&mut s, "GetCreatureMana", chest), Some(Value::Int(-1)));
        assert_eq!(
            get(&mut s, "GetCreatureMaxMana", chest),
            Some(Value::Int(-1))
        );
        for (set, want) in [(30, 30), (90, 40), (-5, 0)] {
            run(
                &mut s,
                mage,
                "SetCreatureMana",
                &[Value::Object(mage), Value::Int(set)],
            );
            assert_eq!(get(&mut s, "GetCreatureMana", mage), Some(Value::Int(want)));
        }
        // The ceiling is the computed total, including fractional modifiers.
        s.world
            .get_mut(mage)
            .unwrap()
            .modifiers
            .insert(PROPERTY_MANA_STAMINA, 13.5);
        run(
            &mut s,
            mage,
            "SetCreatureMana",
            &[Value::Object(mage), Value::Int(90)],
        );
        assert_eq!(
            s.world.get(mage).unwrap().properties[&PROPERTY_MANA_STAMINA].1,
            53.5
        );
        assert_eq!(get(&mut s, "GetCreatureMana", mage), Some(Value::Int(53)));
        assert_eq!(
            get(&mut s, "GetCreatureMaxMana", mage),
            Some(Value::Int(53))
        );
        // Immortality's nonzero health floor does not raise the mana floor.
        s.world.get_mut(mage).unwrap().immortal = true;
        run(
            &mut s,
            mage,
            "SetCreatureMana",
            &[Value::Object(mage), Value::Int(-1)],
        );
        assert_eq!(get(&mut s, "GetCreatureMana", mage), Some(Value::Int(0)));
        let o = s.world.get_mut(mage).unwrap();
        o.modifiers.clear();
        o.properties.insert(PROPERTY_MANA_STAMINA, (-5.0, 0.0));
        run(
            &mut s,
            mage,
            "SetCreatureMana",
            &[Value::Object(mage), Value::Int(90)],
        );
        assert_eq!(get(&mut s, "GetCreatureMana", mage), Some(Value::Int(0)));
        s.world
            .get_mut(chest)
            .unwrap()
            .properties
            .insert(PROPERTY_MANA_STAMINA, (40.0, 12.0));
        for target in [chest, OBJECT_INVALID] {
            run(
                &mut s,
                mage,
                "SetCreatureMana",
                &[Value::Object(target), Value::Int(90)],
            );
        }
        assert_eq!(
            s.world.get(chest).unwrap().properties[&PROPERTY_MANA_STAMINA],
            (40.0, 12.0)
        );
    }

    #[test]
    fn removing_all_effects_spares_injuries_and_what_outlasts_death() {
        let mut s = sim();
        let t = crate::m2da::Tables::empty();
        t.insert(
            "effects",
            &["ID", "IgnoreDeath"],
            vec![
                (7, vec![Cell::Int(7), Cell::Int(1)]),
                (1012, vec![Cell::Int(1012), Cell::Int(0)]),
            ],
        );
        s.world.tables = Arc::new(t);
        let c = s.world.spawn(ObjectKind::Creature, "c");
        let give = |s: &mut Sim| {
            let o = s.world.get_mut(c).unwrap();
            o.effects.clear();
            for (ty, ability, duration_type) in [
                (1012, 10, 3),      // a buff
                (1012, 500_001, 3), // an injury
                (7, 0, 3),          // outlasts death
                (1012, 0, 6),       // duration type 6 always stays
                (1012, 0, 4),       // so do types 4 and 5
            ] {
                o.effects.push(Effect {
                    ty,
                    ability,
                    duration_type,
                    ..Default::default()
                });
            }
        };
        let left = |s: &Sim| -> Vec<(i32, i32, i32)> {
            s.world
                .get(c)
                .unwrap()
                .effects
                .iter()
                .map(|e| (e.ty, e.ability, e.duration_type))
                .collect()
        };
        give(&mut s);
        run(&mut s, c, "RemoveAllEffects", &[Value::Object(c)]);
        assert_eq!(
            left(&s),
            vec![(1012, 500_001, 3), (1012, 0, 6), (1012, 0, 4)]
        );
        give(&mut s);
        run(
            &mut s,
            c,
            "RemoveAllEffects",
            &[Value::Object(c), Value::Int(0), Value::Int(1)],
        );
        assert_eq!(left(&s), vec![(7, 0, 3), (1012, 0, 6), (1012, 0, 4)]);
    }

    #[test]
    fn ghost_and_quickbar_switches_are_kept_on_the_creature() {
        let mut s = sim();
        let c = s.world.spawn(ObjectKind::Creature, "c");
        let chest = s.world.spawn(ObjectKind::Placeable, "chest");
        let set = |s: &mut Sim, f: &str, o: ObjectId, v: i32| {
            run(s, o, f, &[Value::Object(o), Value::Int(v)]);
        };
        let field = |s: &Sim, o: ObjectId, f: &str| s.world.get(o).unwrap().fields.get(f).copied();
        set(&mut s, "SetCreatureIsGhost", c, 7);
        set(&mut s, "SetQuickslotBar", c, 3);
        set(&mut s, "SetQuickslotBar", c, 5);
        set(&mut s, "LockQuickslotBar", c, 1);
        set(&mut s, "SetCreatureIsGhost", chest, 1);
        assert_eq!(field(&s, c, "Ghost"), Some(1));
        assert_eq!(field(&s, c, "QuickslotBar"), Some(3));
        assert_eq!(field(&s, c, "QuickslotBarLocked"), Some(1));
        assert_eq!(field(&s, chest, "Ghost"), None);
        set(&mut s, "SetCreatureIsGhost", c, 0);
        set(&mut s, "LockQuickslotBar", c, -1);
        assert_eq!(field(&s, c, "Ghost"), Some(0));
        assert_eq!(field(&s, c, "QuickslotBarLocked"), Some(0));
    }

    #[test]
    fn ambient_dialog_queries_answer_from_the_conversation() {
        let mut s = sim();
        assert_eq!(run(&mut s, 1, "DialogIsAmbient", &[]), Some(Value::Int(0)));
        s.dialog_ambient = true;
        assert_eq!(run(&mut s, 1, "DialogIsAmbient", &[]), Some(Value::Int(1)));
        assert_eq!(
            run(
                &mut s,
                1,
                "ClearAmbientDialogs",
                &[Value::Object(OBJECT_INVALID)]
            ),
            Some(Value::Int(1))
        );
        assert!(s.world.trace.stubbed.is_empty());
    }

    #[test]
    fn the_current_map_is_revealed_and_auto_pause_follows_the_option() {
        let mut s = sim();
        let area = s.world.spawn(ObjectKind::Area, "area");
        s.world.area = Some(area);
        run(&mut s, 1, "RevealCurrentMap", &[]);
        assert_eq!(
            s.world.get(area).unwrap().fields.get("MapRevealed"),
            Some(&1)
        );
        assert_eq!(
            run(&mut s, 1, "GetAutoPauseCombatStatus", &[]),
            Some(Value::Int(0))
        );
        s.auto_pause_combat = true;
        assert_eq!(
            run(&mut s, 1, "GetAutoPauseCombatStatus", &[]),
            Some(Value::Int(1))
        );
        assert!(s.world.trace.stubbed.is_empty());
    }

    #[test]
    fn an_area_ability_aims_at_the_middle_of_the_best_cluster_of_foes() {
        let mut s = sim();
        let t = crate::m2da::Tables::empty();
        t.insert(
            "ABI_base",
            &["ID", "aoe_type", "aoe_param1", "#5d3e0787"],
            vec![
                // A sphere of radius 2: clusters span 4.
                (
                    1,
                    vec![Cell::Int(1), Cell::Int(1), Cell::Float(2.0), Cell::Int(0)],
                ),
                // A lasting area whose VFX Persistent radius (3) is the span.
                (
                    2,
                    vec![Cell::Int(2), Cell::Int(0), Cell::Float(0.0), Cell::Int(7)],
                ),
            ],
        );
        t.insert(
            "#96",
            &["ID", "Radius"],
            vec![(7, vec![Cell::Int(7), Cell::Float(3.0)])],
        );
        s.world.tables = Arc::new(t);
        let area = s.world.spawn(ObjectKind::Area, "area");
        s.world.hostility.insert((1, 2), true);
        let put = |s: &mut Sim, group: i32, p: [f32; 3]| {
            let c = s.world.spawn(ObjectKind::Creature, "c");
            let o = s.world.get_mut(c).unwrap();
            (o.area, o.group, o.position) = (Some(area), group, p);
            c
        };
        let mage = put(&mut s, 1, [0.0, -10.0, 0.0]);
        let lone = put(&mut s, 2, [0.0, 0.0, 0.0]);
        let a = put(&mut s, 2, [20.0, 0.0, 0.0]);
        let b = put(&mut s, 2, [22.0, 0.0, 0.0]);
        let c = put(&mut s, 2, [21.0, 3.0, 0.0]);
        let ally = put(&mut s, 1, [21.0, 1.0, 0.0]);
        let far = put(&mut s, 2, [50.0, 0.0, 0.0]);
        let center = |s: &mut Sim, seen: Vec<ObjectId>, ability: i32, size: i32, fail: i32| {
            s.world.get_mut(mage).unwrap().combat.perceived = seen;
            let v = run(
                s,
                mage,
                "GetClusterCenter",
                &[
                    Value::Object(mage),
                    Value::Int(ability),
                    Value::Int(size),
                    Value::Int(fail),
                    Value::Int(0),
                ],
            );
            match v {
                Some(Value::Engine { handle, .. }) => match s.world.handle(handle) {
                    Some(EngineData::Location(l)) => (l.area, l.position),
                    _ => panic!("no location"),
                },
                _ => panic!("no location"),
            }
        };
        // The three foes around the ally; the ally is not in the mean.
        let crowd = vec![lone, a, b, c, ally, far];
        assert_eq!(
            center(&mut s, crowd.clone(), 1, 2, 0),
            (area, [21.0, 1.0, 0.0])
        );
        // An ally in every cluster fails every roll at 100%.
        assert_eq!(center(&mut s, crowd, 1, 2, 100).0, OBJECT_INVALID);
        // The first candidate's cluster is never the answer.
        assert_eq!(center(&mut s, vec![a, ally], 1, 1, 0).0, OBJECT_INVALID);
        // A lasting area's radius is the span itself.
        assert_eq!(
            center(&mut s, vec![lone, a, b], 2, 2, 0),
            (area, [21.0, 0.0, 0.0])
        );
        assert_eq!(center(&mut s, vec![lone, a, c], 2, 2, 0).0, OBJECT_INVALID);
        // The engine's distance table: an ally first in line is never a center, so its row
        // stays zero and lets in B (5.5 from it); B, joining after the center, lets in C
        // (4.6 from B). Measured pair by pair there would be no cluster of three foes.
        let x2 = put(&mut s, 1, [100.0, 0.0, 0.0]);
        let a2 = put(&mut s, 2, [102.0, 0.0, 0.0]);
        let b2 = put(&mut s, 2, [105.5, 0.0, 0.0]);
        let c2 = put(&mut s, 2, [102.0, 3.0, 0.0]);
        let (got_area, p) = center(&mut s, vec![x2, a2, b2, c2], 1, 3, 0);
        assert_eq!(got_area, area);
        assert!((p[0] - 309.5 / 3.0).abs() < 1e-4 && (p[1] - 1.0).abs() < 1e-4);
    }

    #[test]
    fn shapes_take_what_their_ground_touches() {
        let mut s = sim();
        let area = s.world.spawn(ObjectKind::Area, "area");
        let put = |s: &mut Sim, kind, p: [f32; 3]| {
            let c = s.world.spawn(kind, "o");
            let o = s.world.get_mut(c).unwrap();
            (o.area, o.position) = (Some(area), p);
            c
        };
        // Bodies are 0.4 across ([`DEFAULT_RADIUS`]) and only their ground position counts.
        let near = put(&mut s, ObjectKind::Creature, [2.3, 0.0, 9.0]);
        let edge = put(&mut s, ObjectKind::Placeable, [2.3, 0.0, 0.0]);
        let behind = put(&mut s, ObjectKind::Creature, [0.0, -1.0, 0.0]);
        let side = put(&mut s, ObjectKind::Creature, [1.0, 1.2, 0.0]);
        let dead = put(&mut s, ObjectKind::Creature, [0.0, 1.0, 0.0]);
        s.world.get_mut(dead).unwrap().dead = true;
        // Facing 270 looks along +x.
        let l = run(
            &mut s,
            OBJECT_INVALID,
            "Location",
            &[
                Value::Object(area),
                Value::Vector([0.0; 3]),
                Value::Float(270.0),
            ],
        )
        .unwrap();
        let shape = |s: &mut Sim,
                     f: &str,
                     pre: Option<Vec<ObjectId>>,
                     mask: i32,
                     shape: i32,
                     fa: f32,
                     fb: f32,
                     d: i32| {
            let mut args: Vec<Value> = pre.map(|v| vec![objects(v)]).unwrap_or_default();
            args.extend([
                Value::Int(mask),
                Value::Int(shape),
                l.clone(),
                Value::Float(fa),
                Value::Float(fb),
                Value::Float(0.0),
                Value::Int(d),
            ]);
            let mut got = match run(s, OBJECT_INVALID, f, &args) {
                Some(Value::Array { items, .. }) => items,
                _ => panic!("no array"),
            };
            got.sort_by_key(|v| match v {
                Value::Object(o) => *o,
                _ => 0,
            });
            got
        };
        let o = |v: &[ObjectId]| v.iter().map(|&x| Value::Object(x)).collect::<Vec<_>>();
        // A sphere of 2: the creature at 2.3 touches it, the placeable (no body) does not.
        assert_eq!(
            shape(&mut s, "GetObjectsInShape", None, -1, 1, 2.0, 0.0, 1),
            o(&[near, behind, side])
        );
        assert_eq!(
            shape(&mut s, "GetObjectsInShape", None, -1, 1, 2.0, 0.0, 0),
            o(&[near, behind, side, dead])
        );
        assert_eq!(
            shape(&mut s, "GetObjectsInShape", None, 512, 1, 3.0, 0.0, 0),
            o(&[edge])
        );
        // A 90-degree cone 3 long along +x: not what is behind or well to the side.
        assert_eq!(
            shape(&mut s, "GetObjectsInShape", None, -1, 2, 90.0, 3.0, 0),
            o(&[near, edge, side])
        );
        assert_eq!(
            shape(&mut s, "GetObjectsInShape", None, -1, 2, 20.0, 3.0, 0),
            o(&[near, edge])
        );
        assert_eq!(
            shape(&mut s, "GetObjectsInShape", None, -1, 3, 9.0, 9.0, 0),
            o(&[])
        );
        // Filtering keeps only the given objects that are in the shape.
        assert_eq!(
            shape(
                &mut s,
                "FilterObjectsInShape",
                Some(vec![behind, edge, side]),
                32,
                1,
                2.0,
                0.0,
                0
            ),
            o(&[behind, side])
        );
    }

    #[test]
    fn angles_vectors_and_locations_convert_as_the_engine_does() {
        let mut s = sim();
        let f = |s: &mut Sim, n: &str, a: &[Value]| run(s, OBJECT_INVALID, n, a).unwrap();
        let close = |v: Value, want: [f32; 3]| match v {
            Value::Vector(v) => (0..3).all(|i| (v[i] - want[i]).abs() < 1e-4),
            _ => false,
        };
        // Facing 270 looks along +x, 0 along +y, 90 along -x.
        assert!(close(
            f(&mut s, "AngleToVector", &[Value::Float(270.0)]),
            [1.0, 0.0, 0.0]
        ));
        assert!(close(
            f(&mut s, "AngleToVector", &[Value::Float(0.0)]),
            [0.0, 1.0, 0.0]
        ));
        assert!(close(
            f(&mut s, "AngleToVector", &[Value::Float(90.0)]),
            [-1.0, 0.0, 0.0]
        ));
        let angle = |s: &mut Sim, v: [f32; 3]| match f(s, "VectorToAngle", &[Value::Vector(v)]) {
            Value::Float(x) => x,
            _ => f32::NAN,
        };
        assert!((angle(&mut s, [1.0, 0.0, 0.0]) + 90.0).abs() < 1e-3);
        assert!((angle(&mut s, [-2.0, 0.0, 5.0]) - 90.0).abs() < 1e-3);
        // Straight back: -x is -0, so atan2 gives -180.
        assert!((angle(&mut s, [0.0, -1.0, 0.0]) + 180.0).abs() < 1e-3);
        assert_eq!(
            f(
                &mut s,
                "VectorToString",
                &[Value::Vector([1.5, -2.0, 0.25])]
            ),
            Value::String("1.500000 -2.000000 0.250000".into())
        );
        let l = f(
            &mut s,
            "Location",
            &[
                Value::Object(5),
                Value::Vector([1.0, 2.0, 3.0]),
                Value::Float(10.0),
            ],
        );
        let moved = f(
            &mut s,
            "SetLocationPosition",
            &[l.clone(), Value::Vector([7.0, 8.0, 9.0])],
        );
        let turned = f(
            &mut s,
            "SetLocationFacing",
            &[moved.clone(), Value::Float(45.0)],
        );
        assert_eq!(
            f(
                &mut s,
                "GetPositionFromLocation",
                std::slice::from_ref(&turned)
            ),
            Value::Vector([7.0, 8.0, 9.0])
        );
        assert_eq!(
            f(&mut s, "GetFacingFromLocation", &[turned]),
            Value::Float(45.0)
        );
        // The location passed in is a copy: it keeps its position.
        assert_eq!(
            f(&mut s, "GetPositionFromLocation", &[l]),
            Value::Vector([1.0, 2.0, 3.0])
        );
    }

    #[test]
    fn an_item_keeps_its_visual_effect() {
        let mut s = sim();
        let sword = s.world.spawn(ObjectKind::Item, "sword");
        let man = s.world.spawn(ObjectKind::Creature, "man");
        for o in [sword, man] {
            run(
                &mut s,
                o,
                "AddItemVisualEffect",
                &[Value::Object(o), Value::Int(1077)],
            );
        }
        let e = &s.world.get(sword).unwrap().effects;
        assert_eq!(e.len(), 1);
        assert_eq!(
            (e[0].ty, e[0].duration_type, e[0].ints.clone()),
            (4, 3, vec![1077])
        );
        assert!(s.world.get(man).unwrap().effects.is_empty());
    }

    #[test]
    fn an_items_property_power_adds_up_its_matching_properties() {
        let mut s = sim();
        let ring = s.world.spawn(ObjectKind::Item, "ring");
        s.world.get_mut(ring).unwrap().template = "gen_im_ring".into();
        s.set_item_look(
            "gen_im_ring",
            crate::sim::ItemLook {
                properties: vec![(1005, 2), (10018, 1), (1005, 3)],
                ..Default::default()
            },
        );
        let man = s.world.spawn(ObjectKind::Creature, "man");
        let power = |s: &mut Sim, o: ObjectId, p: i32| {
            run(
                s,
                o,
                "GetItemPropertyPower",
                &[Value::Object(o), Value::Int(p), Value::Int(1)],
            )
        };
        assert_eq!(power(&mut s, ring, 1005), Some(Value::Int(5)));
        assert_eq!(power(&mut s, ring, 10018), Some(Value::Int(1)));
        assert_eq!(power(&mut s, ring, 7), Some(Value::Int(0)));
        assert_eq!(power(&mut s, man, 1005), Some(Value::Int(0)));
    }

    #[test]
    fn portrait_messages_are_logged() {
        let mut s = sim();
        s.world.trace.keep = true;
        run(
            &mut s,
            1,
            "DisplayPortraitMessage",
            &[
                Value::Object(1),
                Value::String("Approval +5".into()),
                Value::Int(0xff),
            ],
        );
        assert!(s.world.trace.entries.iter().any(|e| matches!(e,
            TraceEntry::Log { channel: -1, text, .. } if text == "Approval +5")));
        assert_eq!(
            s.world.portrait_messages,
            [crate::world::PortraitMessage {
                target: 1,
                text: "Approval +5".into(),
                colour: 0xff,
            }]
        );
    }

    #[test]
    fn set_location_moves_into_an_existing_area() {
        let mut s = sim();
        let here = s.world.spawn(ObjectKind::Area, "here");
        let there = s.world.spawn(ObjectKind::Area, "there");
        let c = s.world.spawn(ObjectKind::Creature, "c");
        s.world.get_mut(c).unwrap().area = Some(here);
        let go = |s: &mut Sim, area: ObjectId, p: [f32; 3], f: f32| {
            let l = run(
                s,
                OBJECT_INVALID,
                "Location",
                &[Value::Object(area), Value::Vector(p), Value::Float(f)],
            )
            .unwrap();
            run(s, c, "SetLocation", &[Value::Object(c), l]);
            let o = s.world.get(c).unwrap();
            (o.area, o.position, o.facing)
        };
        assert_eq!(
            go(&mut s, there, [1.0, 2.0, 3.0], 90.0),
            (Some(there), [1.0, 2.0, 3.0], 90.0)
        );
        // No such area: it stays where it is, at the new position.
        assert_eq!(
            go(&mut s, 999, [4.0, 5.0, 6.0], 0.0),
            (Some(there), [4.0, 5.0, 6.0], 0.0)
        );
    }

    #[test]
    fn the_running_script_and_indestructible_items() {
        let mut s = sim();
        assert_eq!(
            run(&mut s, 1, "GetCurrentScriptResource", &[]),
            Some(Value::Resource("test.ncs".into()))
        );
        let shield = s.world.spawn(ObjectKind::Item, "shield");
        run(
            &mut s,
            1,
            "SetItemIndestructible",
            &[Value::Object(shield), Value::Int(5)],
        );
        assert_eq!(
            s.world.get(shield).unwrap().fields.get("Indestructible"),
            Some(&1)
        );
    }

    #[test]
    fn story_events_are_kept_and_pools_need_nothing() {
        let mut s = sim();
        s.world.time_ms = 4000;
        for id in [12, 0, 7] {
            run(&mut s, 1, "LogStoryEvent", &[Value::Int(id)]);
        }
        assert_eq!(s.world.story_events, vec![(12, 4000), (7, 4000)]);
        assert_eq!(
            run(
                &mut s,
                1,
                "CreatePool",
                &[Value::Resource("c.utc".into()), Value::Int(3)]
            ),
            Some(Value::Int(1))
        );
        assert!(s.world.trace.stubbed.is_empty());
    }

    #[test]
    fn a_creature_remembers_its_body_bag_and_uses_it_again() {
        let mut s = sim();
        let foe = s.world.spawn(ObjectKind::Creature, "foe");
        let chest = s.world.spawn(ObjectKind::Placeable, "chest");
        let bag_of =
            |s: &mut Sim, o: ObjectId| run(s, 1, "GetCreatureBodyBag", &[Value::Object(o)]);
        assert_eq!(bag_of(&mut s, foe), Some(Value::Object(OBJECT_INVALID)));
        s.world.get_mut(foe).unwrap().money = 12;
        let bag = s.spawn_body_bag(foe, false).unwrap();
        assert_eq!(bag_of(&mut s, foe), Some(Value::Object(bag)));
        assert_eq!(bag_of(&mut s, chest), Some(Value::Object(OBJECT_INVALID)));
        // Later loot goes into the same bag, which moves to the creature.
        s.world.get_mut(foe).unwrap().money = 5;
        s.world.get_mut(foe).unwrap().position = [3.0, 4.0, 0.0];
        assert_eq!(s.spawn_body_bag(foe, false), Some(bag));
        let b = s.world.get(bag).unwrap();
        assert_eq!((b.money, b.position), (17, [3.0, 4.0, 0.0]));
    }

    #[test]
    fn sounds_are_switched_by_tag_in_the_current_area() {
        let mut s = sim();
        let here = s.world.spawn(ObjectKind::Area, "here");
        let there = s.world.spawn(ObjectKind::Area, "there");
        s.world.area = Some(here);
        let sound = |s: &mut Sim, area: ObjectId, tag: &str| {
            let o = s.world.spawn(ObjectKind::Sound, tag);
            s.world.get_mut(o).unwrap().area = Some(area);
            o
        };
        let bells = sound(&mut s, here, "snd_bells");
        let duplicate = sound(&mut s, here, "snd_bells");
        let upper = sound(&mut s, here, "SND_BELLS");
        let other = sound(&mut s, here, "snd_wind");
        let far = sound(&mut s, there, "snd_bells");
        run(
            &mut s,
            1,
            "ActivateSoundByTag",
            &[Value::String("snd_bells".into()), Value::Int(0)],
        );
        let active = |s: &Sim, o: ObjectId| s.world.get(o).unwrap().active;
        assert!(!active(&s, bells) && !active(&s, duplicate));
        assert!(active(&s, upper) && active(&s, other) && active(&s, far));
        s.world.get_mut(upper).unwrap().active = false;
        run(
            &mut s,
            1,
            "ActivateSoundByTag",
            &[Value::String("SND_BELLS".into()), Value::Int(1)],
        );
        assert!(active(&s, upper));
        assert!(!active(&s, bells) && !active(&s, duplicate));
        run(
            &mut s,
            1,
            "ActivateSoundByTag",
            &[Value::String("snd_bells".into()), Value::Int(-1)],
        );
        assert!(active(&s, bells) && active(&s, duplicate));
    }

    #[test]
    fn zoom_requests_count_up_and_raise_no_training_event() {
        let mut s = sim();
        assert_eq!(s.world.zoom_request, None);
        run(&mut s, 1, "SetZoomLevel", &[Value::Float(0.0)]);
        assert_eq!(s.world.zoom_request, Some((0.0, 1)));
        run(&mut s, 1, "SetZoomLevel", &[Value::Float(2.0)]);
        assert_eq!(s.world.zoom_request, Some((1.0, 2)));
        // A script's zoom never counts as the player's: nothing is queued for training.
        assert!(s.world.queue.is_empty());
    }

    #[test]
    fn script_sounds_go_to_the_presentation() {
        let mut s = sim();
        let o = s.world.spawn(ObjectKind::Placeable, "bell");
        run(
            &mut s,
            1,
            "PlaySound",
            &[Value::Object(o), Value::String("glo/bell/ring".into())],
        );
        run(&mut s, 1, "PlayMusic", &[Value::String("theme".into())]);
        run(
            &mut s,
            1,
            "SetSoundSet",
            &[Value::Object(o), Value::String("SS_PC_FHUMAN_WISE".into())],
        );
        assert_eq!(
            s.world
                .get(o)
                .unwrap()
                .text_fields
                .get(crate::sim::SOUND_SET),
            Some(&"ss_pc_fhuman_wise".to_string())
        );
        assert_eq!(s.world.music_intensity, 1);
        run(&mut s, 1, "SetMusicIntensity", &[Value::Int(0)]);
        assert_eq!(s.world.music_intensity, 0);
        // Character creation's choice goes to the creature being made.
        assert!(!s.chargen_sound_set("ss_pc_mhuman_cocky"));
        s.world.chargen = Some(o);
        assert!(s.chargen_sound_set("SS_PC_MHUMAN_COCKY"));
        assert_eq!(
            s.world
                .get(o)
                .unwrap()
                .text_fields
                .get(crate::sim::SOUND_SET),
            Some(&"ss_pc_mhuman_cocky".to_string())
        );
        assert_eq!(
            std::mem::take(&mut s.world.sounds),
            [
                crate::world::SoundRequest::Event {
                    target: o,
                    event: "glo/bell/ring".into()
                },
                crate::world::SoundRequest::Music {
                    name: "theme".into()
                }
            ]
        );
    }

    #[test]
    fn item_properties_added_in_play_are_the_items_own() {
        let mut s = sim();
        let ring = s.world.spawn(ObjectKind::Item, "ring");
        s.world.get_mut(ring).unwrap().template = "gen_im_ring".into();
        s.set_item_look(
            "gen_im_ring",
            crate::sim::ItemLook {
                properties: vec![(1005, 2)],
                ..Default::default()
            },
        );
        let props = |s: &mut Sim| run(s, 1, "GetItemProperties", &[Value::Object(ring)]);
        let ints = |v: &[i32]| {
            Some(Value::Array {
                elem: Type::Int,
                items: v.iter().map(|&x| Value::Int(x)).collect(),
            })
        };
        let add = |s: &mut Sim, p: i32, n: i32| {
            run(
                s,
                1,
                "AddItemProperty",
                &[Value::Object(ring), Value::Int(p), Value::Int(n)],
            );
        };
        add(&mut s, 10018, 3);
        // Already there, or a power or property below 1: nothing.
        add(&mut s, 1005, 9);
        add(&mut s, 7, 0);
        add(&mut s, 0, 4);
        assert_eq!(props(&mut s), ints(&[1005, 10018]));
        assert_eq!(s.item_properties(ring), vec![(1005, 2), (10018, 3)]);
        run(
            &mut s,
            1,
            "RemoveItemProperty",
            &[Value::Object(ring), Value::Int(1005)],
        );
        assert_eq!(props(&mut s), ints(&[10018]));
        // The template's list is untouched for other rings.
        let other = s.world.spawn(ObjectKind::Item, "ring2");
        s.world.get_mut(other).unwrap().template = "gen_im_ring".into();
        assert_eq!(s.item_properties(other), vec![(1005, 2)]);
    }
}
