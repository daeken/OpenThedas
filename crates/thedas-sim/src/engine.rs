//! Engine functions, dispatched by name (names and signatures come from `script.ldf` at run
//! time). Implemented in order of how often the shipped scripts call them; see
//! `thedas script stats`. Anything not here is traced as unimplemented and returns the default
//! for its return type.
//!
//! Behaviour that is inferred rather than verified against the original is noted per function.
//! Functions that only affect presentation (visual effects, sounds, floaty text, UI hints)
//! are accepted as no-ops: the simulation is headless and presentation reads it, never the
//! other way round.

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

fn b(v: bool) -> Value {
    Value::Int(v as i32)
}

fn objects(items: Vec<ObjectId>) -> Value {
    Value::Array {
        elem: Type::Object,
        items: items.into_iter().map(Value::Object).collect(),
    }
}

fn cell_int(c: Option<&Cell>) -> i32 {
    match c {
        Some(Cell::Int(v)) => *v as i32,
        Some(Cell::Float(v)) => *v as i32,
        Some(Cell::String(s)) => clean(s).parse().unwrap_or(0),
        _ => 0,
    }
}

/// GDA strings carry a trailing NUL and sometimes padding.
fn clean(s: &str) -> &str {
    s.trim_matches(|c: char| c == '\0' || c.is_whitespace())
}

fn cell_float(c: Option<&Cell>) -> f32 {
    match c {
        Some(Cell::Int(v)) => *v as f32,
        Some(Cell::Float(v)) => *v as f32,
        Some(Cell::String(s)) => clean(s).parse().unwrap_or(0.0),
        _ => 0.0,
    }
}

fn cell_string(c: Option<&Cell>) -> String {
    match c {
        Some(Cell::Int(v)) => v.to_string(),
        Some(Cell::Float(v)) => v.to_string(),
        Some(Cell::String(s)) => clean(s).to_string(),
        _ => String::new(),
    }
}

/// NWScript's float-to-string: `%<width>.<decimals>f`, right-aligned. Provisional for
/// `ToString` on floats (assumed `%f` with six decimals).
fn format_float(f: f32, width: i32, decimals: i32) -> String {
    let d = decimals.clamp(0, 9) as usize;
    let s = format!("{f:.d$}");
    format!("{s:>w$}", w = width.clamp(0, 18) as usize)
}

/// Functions that only drive presentation; accepted as no-ops.
const PRESENTATION: &[&str] = &[
    "ApplyEffectVisualEffect",
    "SetAtmosphere",
    "SetAtmosphereRGB",
    "FB_SetEffectResource",
    "FB_SetEffectEnabled",
    "PlaySoundSet",
    "PlaySound",
    "PlaySoundObject",
    "SetLoadHint",
    "LinkAreaMiniMaps",
    "SetQuickslot",
    "SendGameEventRunDatabase",
    "SetTrainingMode",
    "SetItemMaterialType",
    "SetObjectInteractive",
    "SetPlaceableHealth",
    "SetMapPinState",
    "ShowPopup",
    "SetCameraTarget",
    "SetPlaceableActionResult",
    "ApplyEffectVisualEffectOnLocation",
    "SetLookAtEnabled",
    "SetSoundSet",
    "PlayMusic",
    "SetMusicIntensity",
    "ScaleEquippedItems",
    "SendToRunDatabase",
    "SetCreaturesGlobalMaxTimeBeforeDecay",
    "SetCreatureGoreLevel",
];

/// Converts `CommandMoveToObject` to `COMMAND_TYPE_MOVE_TO_OBJECT`.
fn command_constant(name: &str) -> String {
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

/// Runs engine function `f`; `None` if it is not implemented.
#[allow(clippy::too_many_lines)]
pub fn call(
    h: &mut ScriptHost,
    caller: ObjectId,
    f: &Function,
    a: &[Value],
) -> Option<VmResult<Option<Value>>> {
    let name = f.name.as_str();
    if PRESENTATION.contains(&name) {
        return void();
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
        "StringToInt" => ok(Value::Int(string(a, 0).trim().parse().unwrap_or(0))),
        "StringToFloat" => ok(Value::Float(string(a, 0).trim().parse().unwrap_or(0.0))),
        "ResourceToString" => ok(Value::String(string(a, 0))),
        "StringToResource" => ok(Value::Resource(string(a, 0))),
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
        "FindSubString" => {
            let s = string(a, 0);
            let start = int(a, 2).max(0) as usize;
            ok(Value::Int(
                s.get(start..)
                    .and_then(|r| r.find(&string(a, 1)))
                    .map_or(-1, |i| (i + start) as i32),
            ))
        }
        "abs" => ok(Value::Int(int(a, 0).wrapping_abs())),
        "fabs" => ok(Value::Float(float(a, 0).abs())),
        "sqrt" => ok(Value::Float(float(a, 0).max(0.0).sqrt())),
        // Radians, per script.ldf; out-of-domain inputs are clamped rather than giving NaN.
        "sin" => ok(Value::Float(float(a, 0).sin())),
        "cos" => ok(Value::Float(float(a, 0).cos())),
        "tan" => ok(Value::Float(float(a, 0).tan())),
        "asin" => ok(Value::Float(float(a, 0).clamp(-1.0, 1.0).asin())),
        "acos" => ok(Value::Float(float(a, 0).clamp(-1.0, 1.0).acos())),
        "atan" => ok(Value::Float(float(a, 0).atan())),
        "log" => ok(Value::Float(float(a, 0).max(f32::MIN_POSITIVE).ln())),
        "pow" => ok(Value::Float(float(a, 0).powf(float(a, 1)))),
        "Random" => ok(Value::Int(h.sim.world.rng.below(int(a, 0)))),
        "RandomFloat" => ok(Value::Float(h.sim.world.rng.unit())),
        "Vector" => ok(Value::Vector([float(a, 0), float(a, 1), float(a, 2)])),
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
        | "PrintToLogWindow" => {
            let (channel, text) = match name {
                "LogTrace" => (int(a, 0), string(a, 1)),
                "PrintInteger" => (0, int(a, 0).to_string()),
                "PrintFloat" => (0, float(a, 0).to_string()),
                "DisplayFloatyMessage" => (-1, string(a, 1)),
                _ => (0, string(a, 0)),
            };
            h.sim.world.trace.push(TraceEntry::Log { t, channel, text });
            void()
        }

        // --- Objects ----------------------------------------------------------------------
        "GetModule" => ok(Value::Object(h.sim.world.module.unwrap_or(OBJECT_INVALID))),
        "GetHero" | "GetPartyLeader" | "GetMainControlled" => {
            ok(Value::Object(h.sim.world.hero.unwrap_or(OBJECT_INVALID)))
        }
        "IsHero" => ok(b(Some(obj(a, 0)) == h.sim.world.hero)),
        "IsControlled" => ok(b(
            !h.sim.world.autopilot && Some(obj(a, 0)) == h.sim.world.hero
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
        "GetName" => ok(Value::String(
            h.object(obj(a, 0))
                .map(|o| match o.name_strref {
                    Some(s) => h.sim.string(s).unwrap_or_else(|| o.tag.clone()),
                    None => o.tag.clone(),
                })
                .unwrap_or_default(),
        )),
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
                ok(Value::Object(
                    ids.first().copied().unwrap_or(OBJECT_INVALID),
                ))
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
        "IsHumanoid" => ok(b(h
            .object(obj(a, 0))
            .is_some_and(|o| o.kind == ObjectKind::Creature))),
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
            let id = obj(a, 0);
            if Some(id) != h.sim.world.hero {
                h.sim.world.objects.remove(&id);
            }
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
            let id = h.sim.world.spawn(kind, &template);
            if let Some(o) = h.sim.world.get_mut(id) {
                o.template = template;
                if let Some(l) = loc {
                    o.area = Some(l.area);
                    o.position = l.position;
                    o.facing = l.facing;
                }
                let script = string(a, 3);
                if !script.is_empty() {
                    o.script = Some(script);
                }
            }
            ok(Value::Object(id))
        }

        // --- Locals -----------------------------------------------------------------------
        "GetLocalInt" => ok(Value::Int(
            h.object(obj(a, 0))
                .and_then(|o| o.locals.ints.get(&string(a, 1)).copied())
                .unwrap_or(0),
        )),
        "SetLocalInt" => {
            if let Some(o) = h.sim.world.get_mut(obj(a, 0)) {
                o.locals.ints.insert(string(a, 1), int(a, 2));
            }
            void()
        }
        "GetLocalFloat" => ok(Value::Float(
            h.object(obj(a, 0))
                .and_then(|o| o.locals.floats.get(&string(a, 1)).copied())
                .unwrap_or(0.0),
        )),
        "SetLocalFloat" => {
            if let Some(o) = h.sim.world.get_mut(obj(a, 0)) {
                o.locals.floats.insert(string(a, 1), float(a, 2));
            }
            void()
        }
        "GetLocalString" => ok(Value::String(
            h.object(obj(a, 0))
                .and_then(|o| o.locals.strings.get(&string(a, 1)).cloned())
                .unwrap_or_default(),
        )),
        "SetLocalString" => {
            if let Some(o) = h.sim.world.get_mut(obj(a, 0)) {
                o.locals.strings.insert(string(a, 1), string(a, 2));
            }
            void()
        }
        "GetLocalObject" => ok(Value::Object(
            h.object(obj(a, 0))
                .and_then(|o| o.locals.objects.get(&string(a, 1)).copied())
                .unwrap_or(OBJECT_INVALID),
        )),
        "SetLocalObject" => {
            if let Some(o) = h.sim.world.get_mut(obj(a, 0)) {
                o.locals.objects.insert(string(a, 1), obj(a, 2));
            }
            void()
        }
        "GetLocalResource" => ok(Value::Resource(
            h.object(obj(a, 0))
                .and_then(|o| o.locals.resources.get(&string(a, 1)).cloned())
                .unwrap_or_default(),
        )),
        "SetLocalResource" => {
            if let Some(o) = h.sim.world.get_mut(obj(a, 0)) {
                o.locals.resources.insert(string(a, 1), string(a, 2));
            }
            void()
        }
        "GetLocalLocation" => {
            let l = h
                .object(obj(a, 0))
                .and_then(|o| o.locals.locations.get(&string(a, 1)).copied());
            let hd = l.map(|l| h.sim.world.store(EngineData::Location(l)));
            ok(Value::Engine {
                ty: ty.location,
                handle: hd,
            })
        }
        "SetLocalLocation" => {
            if let Some(EngineData::Location(l)) = h.sim.world.handle(handle(a, 2)).cloned() {
                if let Some(o) = h.sim.world.get_mut(obj(a, 0)) {
                    o.locals.locations.insert(string(a, 1), l);
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
        "GetEventType" | "GetEventCreator" | "GetEventTarget" | "GetEventInteger"
        | "GetEventFloat" | "GetEventString" | "GetEventObject" | "GetEventResource" => {
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
                _ => Value::Object(ev.objects.get(i).copied().unwrap_or(OBJECT_INVALID)),
            })
        }
        "SetEventType" | "SetEventCreator" | "SetEventInteger" | "SetEventFloat"
        | "SetEventString" | "SetEventObject" | "SetEventResource" => {
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
            ok(match name {
                "GetM2DAInt" => Value::Int(cell_int(c)),
                "GetM2DAFloat" => Value::Float(cell_float(c)),
                "GetM2DAString" => Value::String(cell_string(c)),
                _ => Value::Resource(cell_string(c)),
            })
        }
        "GetHashedM2DAInt" => {
            let table = h.sim.world.tables.get(int(a, 0), "");
            ok(Value::Int(cell_int(table.as_ref().and_then(|t| {
                t.cell_by_hash(int(a, 1) as u32, int(a, 2) as i64)
            }))))
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
        "GetCreatureProperty" => {
            let vt = int(a, 2);
            let base_type = constant(&h.sim.table, "PROPERTY_VALUE_BASE");
            let modifier = constant(&h.sim.table, "PROPERTY_VALUE_MODIFIER");
            let current = constant(&h.sim.table, "PROPERTY_VALUE_CURRENT");
            let (base, cur) = h
                .object(obj(a, 0))
                .and_then(|o| o.properties.get(&int(a, 1)).copied())
                .unwrap_or((0.0, 0.0));
            let v = if vt == base_type {
                base
            } else if vt == modifier {
                return ok(Value::Float(0.0));
            } else if vt == current {
                cur
            } else {
                base.max(cur)
            };
            // Totals and attributes stay within `properties.gda`'s Min and Max (a depletable's
            // current value may still run down to 0). Provisional: inferred from the table.
            let v = match property_bounds(h.sim, int(a, 1)) {
                Some((ty, min, max)) if !(vt == current && ty == 3) => v.clamp(min, max),
                _ => v,
            };
            ok(Value::Float(v))
        }
        "SetCreatureProperty" | "UpdateCreatureProperty" => {
            let vt = int(a, 3);
            let base_type = constant(&h.sim.table, "PROPERTY_VALUE_BASE");
            let current = constant(&h.sim.table, "PROPERTY_VALUE_CURRENT");
            if let Some(o) = h.sim.world.get_mut(obj(a, 0)) {
                let e = o.properties.entry(int(a, 1)).or_insert((0.0, 0.0));
                let v = float(a, 2);
                let v = if name == "UpdateCreatureProperty" {
                    if vt == base_type { e.0 + v } else { e.1 + v }
                } else {
                    v
                };
                if vt == base_type {
                    e.0 = v;
                } else if vt == current {
                    e.1 = v;
                } else {
                    e.0 = v;
                    e.1 = v;
                }
                // Scripts bring the dead back by giving them health (module_core's
                // HandleEvent_Resurrection applies no effect). Provisional.
                let health = constant(&h.sim.table, "PROPERTY_DEPLETABLE_HEALTH");
                if int(a, 1) == health && vt != base_type && o.dead && o.properties[&health].1 > 0.0
                {
                    o.dead = false;
                }
            }
            void()
        }

        // --- Plots ------------------------------------------------------------------------
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
            let old = h.sim.world.game_mode;
            h.sim.world.game_mode = int(a, 0);
            if let Some(m) = h.sim.world.module {
                let mut ev = h.sim.new_event("GAMEMODE_CHANGE", Some(m));
                ev.ints = vec![int(a, 0), old];
                h.sim.world.queue_event(0, m, ev, None);
            }
            void()
        }
        "GetGameDifficulty" => ok(Value::Int(1)),
        "GetMaxLevel" => ok(Value::Int(20)),
        "GetTime" => ok(Value::Int((h.sim.world.time_ms / 1000) as i32)),
        "ReadIniEntry" => ok(Value::String(String::new())),
        "GetPRCEnabled" | "GetHasAchievementByID" => ok(Value::Int(0)),
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
        "AddAbility" => {
            if let Some(o) = h.sim.world.get_mut(obj(a, 0)) {
                o.abilities.insert(int(a, 1));
            }
            void()
        }
        "RemoveAbility" => {
            if let Some(o) = h.sim.world.get_mut(obj(a, 0)) {
                o.abilities.remove(&int(a, 1));
            }
            void()
        }
        "HasAbility" => ok(b(h
            .object(obj(a, 0))
            .is_some_and(|o| o.abilities.contains(&int(a, 1))))),
        "IsModalAbilityActive" => ok(b(h.sim.modal_active(obj(a, 0), int(a, 1)))),
        "GetItemInEquipSlot" => {
            let who = obj(a, 1);
            ok(Value::Object(
                h.object(who)
                    .and_then(|o| o.equipment.get(&int(a, 0)).copied())
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
            let ok_ = h.object(item).is_some();
            if let (true, Some(o)) = (ok_, h.sim.world.get_mut(who)) {
                if !o.inventory.contains(&item) {
                    o.inventory.push(item);
                }
                o.equipment.retain(|_, &mut i| i != item);
                o.equipment.insert(slot, item);
            }
            ok(b(ok_))
        }
        "UnequipItem" => {
            let item = obj(a, 1);
            let found = match h.sim.world.get_mut(obj(a, 0)) {
                Some(o) => {
                    let n = o.equipment.len();
                    o.equipment.retain(|_, &mut i| i != item);
                    n != o.equipment.len()
                }
                None => false,
            };
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
            if let Some(o) = h.sim.world.get_mut(who) {
                o.inventory.retain(|i| !items.contains(i));
                o.equipment.retain(|_, i| !items.contains(i));
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
            let template = string(a, 0);
            let id = h.sim.create_item(&template);
            // (resource, target, stack size, tag, suppress notification, droppable)
            let stack = if a.len() > 2 { int(a, 2) } else { 1 };
            let droppable = a.len() <= 5 || int(a, 5) != 0;
            if let Some(o) = h.sim.world.get_mut(id) {
                let tag = string(a, 3);
                if !tag.is_empty() {
                    o.tag = tag;
                }
                if stack > 1 {
                    o.fields.insert("StackSize".into(), stack);
                }
                o.fields.insert("Droppable".into(), droppable as i32);
            }
            h.sim.give_item(id, obj(a, 1));
            ok(Value::Object(id))
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
                ("SetEffectFloat", i + 32)
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
        "Engine_ApplyEffectOnObject" | "ApplyEffectOnObject" => {
            if let Some(EngineData::Effect(mut e)) = h.sim.world.handle(handle(a, 1)).cloned() {
                e.duration_type = int(a, 0);
                let creator = obj(a, 4);
                if creator != OBJECT_INVALID && creator != 0 {
                    e.creator = Some(creator);
                }
                // The ability the effect comes from (`GetEffectAbilityID`).
                if a.len() > 5 && int(a, 5) != 0 {
                    e.ability = int(a, 5);
                }
                // A temporary effect lasts its duration (seconds).
                let temporary = constant(&h.sim.table, "EFFECT_DURATION_TYPE_TEMPORARY");
                if e.duration_type == temporary && float(a, 3) > 0.0 {
                    e.expires = Some(t + (float(a, 3).min(1.0e6) * 1000.0) as u64);
                }
                h.sim.apply_effect(obj(a, 2), e);
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
            let (et, ab) = (int(a, 1), int(a, 2));
            ok(b(h.object(obj(a, 0)).is_some_and(|o| {
                o.effects
                    .iter()
                    .any(|e| (et == 0 || e.ty == et) && (ab == 0 || e.ability == ab))
            })))
        }
        "GetEffects" => {
            let (et, ab) = (int(a, 1), int(a, 2));
            let list: Vec<Effect> = h
                .object(obj(a, 0))
                .map(|o| {
                    o.effects
                        .iter()
                        .filter(|e| (et == 0 || e.ty == et) && (ab == 0 || e.ability == ab))
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
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
                h.sim.remove_effect(obj(a, 0), &e);
            }
            void()
        }
        "RemoveEffectsByParameters" => {
            let (who, et, ab) = (obj(a, 0), int(a, 1), int(a, 2));
            let gone: Vec<Effect> = h
                .object(who)
                .map(|o| {
                    o.effects
                        .iter()
                        .filter(|e| (et == 0 || e.ty == et) && (ab == 0 || e.ability == ab))
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
            for e in gone {
                h.sim.remove_effect(who, &e);
            }
            void()
        }
        "GetEffectsFlags" => ok(Value::Int(0)),

        // --- Commands ---------------------------------------------------------------------
        n if n.starts_with("Command") && f.ret == Type::Engine(ty.command) => {
            let mut c = Command {
                ty: constant(&h.sim.table, &command_constant(n)),
                name: n.to_string(),
                target: None,
                location: None,
                seconds: 0.0,
                ints: Vec::new(),
                strings: Vec::new(),
                due: None,
            };
            for (p, v) in f.params.iter().zip(a) {
                match (&p.ty, v) {
                    (Type::Object, Value::Object(o)) if c.target.is_none() => c.target = Some(*o),
                    (Type::Float, Value::Float(x)) if c.seconds == 0.0 => c.seconds = *x,
                    (Type::Int, Value::Int(x)) => c.ints.push(*x),
                    (Type::String | Type::Resource, Value::String(s) | Value::Resource(s)) => {
                        c.strings.push(s.clone())
                    }
                    (Type::Engine(_), Value::Engine { handle, .. }) => {
                        if let Some(EngineData::Location(l)) = h.sim.world.handle(*handle) {
                            c.location = Some(*l);
                        }
                    }
                    _ => {}
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
            if c.name == "CommandStartConversation" {
                let other = c.target.unwrap_or(OBJECT_INVALID);
                let dlg = c.strings.first().cloned().unwrap_or_default();
                h.sim.start_conversation(target, other, &dlg);
            }
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
        "GetObjectsInShape" => {
            // Provisional: every shape is treated as a sphere of its largest dimension.
            let (mask, excl_dead) = (int(a, 0), int(a, 6) != 0);
            let radius = float(a, 3).max(float(a, 4)).max(float(a, 5));
            let l = match h.sim.world.handle(handle(a, 2)) {
                Some(EngineData::Location(l)) => Some(*l),
                _ => None,
            };
            let list: Vec<ObjectId> = match l {
                Some(l) => h
                    .sim
                    .world
                    .objects
                    .values()
                    .filter(|o| o.area == Some(l.area) && o.active)
                    .filter(|o| mask == 0 || o.kind.script_type() & mask != 0)
                    .filter(|o| !excl_dead || !o.dead)
                    .filter(|o| dist(o.position, l.position) <= radius)
                    .map(|o| o.id)
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
                if let Some(o) = h.sim.world.get_mut(from) {
                    o.inventory.retain(|&i| i != item);
                    o.equipment.retain(|_, i| *i != item);
                }
                if let Some(o) = h.sim.world.get_mut(to) {
                    o.inventory.push(item);
                }
            }
            void()
        }
        // Area effects and map notifications are not simulated yet.
        "Engine_ApplyEffectAtLocation" | "ShowAreaUnlockedNotification" => {
            h.sim.world.trace.push(TraceEntry::Log {
                t,
                channel: -3,
                text: format!("{name}{:?} (no effect in the simulation)", a),
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
                (Some(leader), Some(me)) => {
                    let i = leader.followers.iter().position(|&f| f == who).unwrap_or(0);
                    let (s, c) = leader.facing.to_radians().sin_cos();
                    let back = 1.0 + i as f32;
                    Location {
                        area: leader.area.or(me.area).unwrap_or(OBJECT_INVALID),
                        position: [
                            leader.position[0] - c * back,
                            leader.position[1] - s * back,
                            leader.position[2],
                        ],
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
                if let Some(ho) = h.sim.world.get_mut(hero) {
                    ho.followers.retain(|&f| f != who);
                    if active {
                        ho.followers.push(who);
                    }
                }
                // Joining the active party puts a follower on the player's side
                // (GROUP_PC), whatever group it had as an NPC. Provisional.
                if let (true, Some(g), Some(o)) = (active, hero_group, h.sim.world.get_mut(who)) {
                    o.group = g;
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
        // No ambient (bark) conversations are simulated.
        "DialogIsAmbient" => ok(Value::Int(0)),
        "ClearAmbientDialogs" => ok(Value::Int(0)),
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
            let t = (i >= 0 && i < num)
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
        "AddTacticPresetID" => {
            if let Some(o) = h.sim.world.get_mut(obj(a, 0)) {
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
        "DoAutoSave" | "TakeScreenshot" | "RevealCurrentMap" | "SetCreatureGoreLevel" => {
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
        "PreloadCharGen" => void(),
        "StartCharGen" => {
            h.sim.start_chargen(obj(a, 0), int(a, 1), int(a, 2));
            void()
        }
        "BeginConversation" => {
            let ok_ = h
                .sim
                .start_conversation(obj(a, 0), obj(a, 1), &string(a, 2));
            ok(b(ok_))
        }

        // --- Template-backed getters --------------------------------------------------------
        "GetPackage" | "GetCreatureRacialType" | "GetCombatantType" | "GetPlaceableBaseType" => {
            let field = match name {
                "GetPackage" => "Package",
                "GetCreatureRacialType" => "Race",
                "GetCombatantType" => "Combatant",
                _ => "Appearance",
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
        "GetCreatureGender" => ok(Value::Int(
            h.object(obj(a, 0))
                .and_then(|o| o.fields.get("Gender").copied())
                .unwrap_or(0),
        )),
        "SetCreatureRacialType" | "SetCreatureGender" | "SetMaxInventorySize" => {
            let field = match name {
                "SetCreatureRacialType" => "Race",
                "SetCreatureGender" => "Gender",
                _ => "MaxInventorySize",
            };
            if let Some(o) = h.sim.world.get_mut(obj(a, 0)) {
                o.fields.insert(field.to_string(), int(a, 1));
            }
            void()
        }
        "SetAppearanceType" => {
            if let Some(o) = h.sim.world.get_mut(obj(a, 0)) {
                o.appearance = int(a, 1);
            }
            void()
        }
        "SetCreatureRank" => {
            if let Some(o) = h.sim.world.get_mut(obj(a, 0)) {
                o.rank = int(a, 1);
            }
            void()
        }
        "GetAbilityCount" => ok(Value::Int(
            h.object(obj(a, 0)).map_or(0, |o| o.abilities.len() as i32),
        )),
        "CharGen_ClearAbilityList" => {
            if let Some(o) = h.sim.world.get_mut(obj(a, 0)) {
                o.abilities.clear();
            }
            void()
        }
        "GetBaseItemType" => ok(Value::Int(
            h.object(obj(a, 0))
                .and_then(|o| o.fields.get("BaseItem").copied())
                .unwrap_or(0),
        )),
        "GetPartyPoolList" => {
            let hero = h.sim.world.hero;
            let mut v: Vec<ObjectId> = hero.into_iter().collect();
            if let Some(x) = hero.and_then(|x| h.object(x)) {
                v.extend(x.followers.iter().copied());
            }
            ok(objects(v))
        }
        "IsStringEmpty" => ok(b(string(a, 0).is_empty())),
        "GetLowResTimer" => ok(Value::Int(h.sim.world.time_ms as i32)),
        "DEBUG_PrintToScreen" | "DisplayStatusMessage" => {
            h.sim.world.trace.push(TraceEntry::Log {
                t,
                channel: -2,
                text: string(a, 0),
            });
            void()
        }
        "GetActiveWeaponSet" => ok(Value::Int(0)),
        "GetCreatureGoreLevel" => ok(Value::Float(0.0)),
        "GetCreaturePropertyType" => ok(Value::Int(
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
                }),
        )),
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
            let (team, mask) = (int(a, 0), int(a, 1));
            let mask = if mask == 0 { 32 } else { mask };
            let v: Vec<ObjectId> = h
                .sim
                .world
                .objects
                .values()
                .filter(|o| o.team == team && o.kind.script_type() & mask != 0)
                .map(|o| o.id)
                .collect();
            ok(objects(v))
        }
        // The action a click would take. Provisional: the game derives it from the
        // placeable's state controller; here an area-transition destination makes it a
        // transition, a conversation a dialog, and anything else a plain use.
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
        "SetWorldMapPrimary" => {
            h.sim.world.world_map.primary = Some(obj(a, 0));
            void()
        }
        "SetWorldMapSecondary" => {
            h.sim.world.world_map.secondary = Some(obj(a, 0));
            void()
        }
        "SetWorldMapGuiStatus" => {
            h.sim.world.world_map.gui_status = int(a, 0);
            void()
        }
        "OpenPrimaryWorldMap" => {
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
                h.sim.world.transition = Some((area, string(a, 1)));
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
        "GetPlaceableAction" => {
            let o = h.object(obj(a, 0));
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
        "GetPlaceableState" => ok(Value::Int(
            h.object(obj(a, 0))
                .and_then(|o| o.fields.get("State").copied())
                .unwrap_or(0),
        )),
        "SetNumTactics" => {
            if let Some(o) = h.sim.world.get_mut(obj(a, 0)) {
                o.fields.insert("NumTactics".into(), int(a, 1));
            }
            void()
        }
        "GetNumTactics" => ok(Value::Int(
            h.object(obj(a, 0))
                .and_then(|o| o.fields.get("NumTactics").copied())
                .unwrap_or(0),
        )),
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
        "LogStoryEvent" => {
            h.sim.world.trace.push(TraceEntry::Log {
                t,
                channel: -3,
                text: format!("{name}{:?} (no effect in the simulation)", a),
            });
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
            let (who, seen) = (obj(a, 0), obj(a, 1));
            let new = h
                .object(who)
                .is_some_and(|o| !o.combat.perceived.contains(&seen));
            if new && h.object(seen).is_some() {
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
            let (enemy, delta) = (obj(a, 1), float(a, 2));
            if let Some(o) = h.sim.world.get_mut(obj(a, 0)) {
                match o.combat.threat.iter_mut().find(|t| t.0 == enemy) {
                    Some(t) => t.1 += delta,
                    None => o.combat.threat.push((enemy, delta)),
                }
                o.combat
                    .threat
                    .sort_by(|x, y| y.1.total_cmp(&x.1).then(x.0.cmp(&y.0)));
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
        "SpawnBodyBag" => {
            h.sim.spawn_body_bag(obj(a, 0), int(a, 1) != 0);
            void()
        }
        // No occlusion is simulated: two objects in the same area can see each other.
        "CheckLineOfSightObject" => ok(b(match (h.object(obj(a, 0)), h.object(obj(a, 1))) {
            (Some(x), Some(y)) => x.area == y.area,
            _ => false,
        })),
        "CountItemsByTag" => {
            let tag = string(a, 1);
            let n = h
                .object(obj(a, 0))
                .map(|o| o.inventory.clone())
                .unwrap_or_default()
                .into_iter()
                .filter(|i| {
                    h.object(*i)
                        .is_some_and(|x| x.tag.eq_ignore_ascii_case(&tag))
                })
                .count();
            ok(Value::Int(n as i32))
        }
        "SetCanLevelUp" | "SetAILevel" => {
            let field = if name == "SetCanLevelUp" {
                "CanLevelUp"
            } else {
                "AILevel"
            };
            if let Some(o) = h.sim.world.get_mut(obj(a, 0)) {
                o.fields.insert(field.to_string(), int(a, 1));
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
        "EffectModifyProperty" => {
            let e = Effect {
                ty: 1012,
                creator: Some(caller),
                ints: vec![int(a, 0), int(a, 2), int(a, 4)],
                floats: vec![float(a, 1), float(a, 3), float(a, 5)],
                ..Default::default()
            };
            let hd = h.sim.world.store(EngineData::Effect(e));
            ok(Value::Engine {
                ty: ty.effect,
                handle: Some(hd),
            })
        }
        "SetDeathHint" => void(),
        "IsItemDroppable" => ok(b(h
            .object(obj(a, 0))
            .is_some_and(|o| o.fields.get("Droppable").copied().unwrap_or(0) != 0))),
        "SetItemDroppable" => {
            if let Some(o) = h.sim.world.get_mut(obj(a, 0)) {
                o.fields.insert("Droppable".into(), int(a, 1));
            }
            void()
        }
        "UnlockAchievementByID" | "IncrementAchievementCountByID" => {
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
                o.combat.threat.retain(|t| t.0 != enemy);
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
                .map_or(OBJECT_INVALID, |t| t.0),
        )),
        "GetThreatValueByIndex" => ok(Value::Float(
            h.object(obj(a, 0))
                .and_then(|o| o.combat.threat.get(int(a, 1).max(0) as usize))
                .map_or(0.0, |t| t.1),
        )),
        "GetThreatValueByObjectID" => {
            let enemy = obj(a, 1);
            ok(Value::Float(
                h.object(obj(a, 0))
                    .and_then(|o| o.combat.threat.iter().find(|t| t.0 == enemy))
                    .map_or(0.0, |t| t.1),
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
        // rules scripts read them back (float 0 is the damage, integers are the damage type,
        // flags and visual effect, object 0 is the weapon).
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
                    ints: vec![int(a, 4), 0, int(a, 2)],
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
        "GetItemStat" => {
            let base = h
                .object(obj(a, 0))
                .and_then(|o| o.fields.get("BaseItem").copied());
            let stat = int(a, 1);
            let tables = h.sim.world.tables.clone();
            let label = tables
                .get(0, "itemstatdefines")
                .map(|t| cell_string(t.cell("Label", stat as i64)))
                .filter(|l| !l.is_empty());
            let v = match (base, label) {
                (Some(b), Some(l)) => tables
                    .get(0, "itemstats")
                    .and_then(|t| t.cell(clean(&l), b as i64).map(|c| cell_float(Some(c))))
                    .unwrap_or(0.0),
                _ => 0.0,
            };
            ok(Value::Float(v))
        }
        "GetItemStackSize" => ok(Value::Int(
            h.object(obj(a, 0))
                .and_then(|o| o.fields.get("StackSize").copied())
                .unwrap_or(1)
                .max(1),
        )),
        "SetItemStackSize" => {
            if let Some(o) = h.sim.world.get_mut(obj(a, 0)) {
                o.fields.insert("StackSize".into(), int(a, 1).max(1));
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
            let target = h.sim.current_effect_target.unwrap_or(caller);
            if let Some(o) = h.sim.world.get_mut(target) {
                for (p, d) in e.ints.iter().zip(e.floats.iter()) {
                    if *p != 0 {
                        let v = o.properties.entry(*p).or_insert((0.0, 0.0));
                        v.0 += sign * d;
                    }
                }
            }
            void()
        }
        "GetItemUnique" | "GetItemMaterialProgression" => {
            let field = if name == "GetItemUnique" {
                "Unique"
            } else {
                "MaterialProgress"
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
        "SetAimLoopDuration" => void(),
        // Tactics and area-of-effect abilities are not simulated yet.
        "GetAbilitiesDueToAOEs" => ok(Value::Array {
            elem: Type::Int,
            items: Vec::new(),
        }),
        "GetItemEquipSlot" => {
            let item = obj(a, 0);
            let slot = h.sim.world.objects.values().find_map(|o| {
                o.equipment
                    .iter()
                    .find(|(_, i)| **i == item)
                    .map(|(s, _)| *s)
            });
            ok(Value::Int(slot.unwrap_or_else(|| {
                constant(&h.sim.table, "INVENTORY_SLOT_INVALID")
            })))
        }
        "GetWeaponStyle" => {
            let n = h.object(obj(a, 0)).map_or(0, |o| o.equipment.len());
            ok(Value::Int(if n == 0 { 0 } else { 1 }))
        }
        // Followers fight on their own; the hero is the player's.
        "IsPartyAIEnabled" => ok(b(
            h.sim.world.autopilot || Some(obj(a, 0)) != h.sim.world.hero
        )),
        "GetCreaturesInMeleeRing" => {
            let (who, hostile_only) = (obj(a, 0), int(a, 3) != 0);
            let Some(me) = h.object(who).map(|o| (o.position, o.area)) else {
                return ok(objects(Vec::new()));
            };
            let list: Vec<ObjectId> = h
                .sim
                .world
                .objects
                .values()
                .filter(|o| {
                    o.id != who
                        && o.kind == ObjectKind::Creature
                        && !o.dead
                        && o.area == me.1
                        && dist(o.position, me.0) <= 2.5
                })
                .filter(|o| !hostile_only || h.sim.hostile(who, o.id))
                .map(|o| o.id)
                .collect();
            ok(objects(list))
        }
        "ObjectToString" => ok(Value::String(format!("{}", obj(a, 0)))),
        "GetNameStrref" => ok(Value::Int(
            h.object(obj(a, 0))
                .and_then(|o| o.name_strref)
                .map_or(0, |s| s as i32),
        )),
        "GetStealthEnabled" => ok(Value::Int(0)),
        "GetCanDiePermanently" => ok(b(h.object(obj(a, 0)).is_some_and(|o| !o.immortal))),
        "GetCreatureTreasureCategory" => ok(Value::Int(
            h.object(obj(a, 0))
                .and_then(|o| o.fields.get("TreasureCategory").copied())
                .unwrap_or(0),
        )),
        // Presentation only.
        "PlayAdditiveAnimation" | "SignalDamage" => void(),
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
                    let to = d[1].atan2(d[0]).to_degrees();
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
                o.facing = (t[1] - o.position[1])
                    .atan2(t[0] - o.position[0])
                    .to_degrees();
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
            let (item, count) = (obj(a, 0), int(a, 1).max(0));
            let Some(left) = h
                .object(item)
                .filter(|o| o.kind == ObjectKind::Item)
                .map(|o| o.fields.get("StackSize").copied().unwrap_or(1).max(1) - count)
            else {
                return void();
            };
            if left > 0 {
                if let Some(o) = h.sim.world.get_mut(item) {
                    o.fields.insert("StackSize".into(), left);
                }
            } else {
                for o in h.sim.world.objects.values_mut() {
                    o.inventory.retain(|&i| i != item);
                    o.equipment.retain(|_, i| *i != item);
                }
                h.sim.world.objects.remove(&item);
            }
            void()
        }
        // Ability results only steer animations, which the simulation does not play.
        "SetAbilityResult" => void(),
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
        "GetTacticCommandItemTag" => ok(Value::String(String::new())),
        "GetAutoPauseCombatStatus" => ok(Value::Int(0)),
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
/// and rotated by its facing (provisional: degrees about Z, as `facing` is derived).
pub fn in_trigger(p: [f32; 3], trigger: &crate::world::Object) -> bool {
    if trigger.geometry.len() < 3 {
        return false;
    }
    let (dx, dy) = (p[0] - trigger.position[0], p[1] - trigger.position[1]);
    let (s, c) = (-trigger.facing.to_radians()).sin_cos();
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
fn cooldown_key(ability: i32, item: &str) -> String {
    format!("{ability}:{}", item.to_ascii_lowercase())
}

/// A property's type (as `GetCreaturePropertyType`), Min and Max from `properties.gda`.
fn property_bounds(sim: &Sim, property: i32) -> Option<(i32, f32, f32)> {
    let t = sim.world.tables.get(0, "properties")?;
    let num = |c: Option<&Cell>| match c {
        Some(Cell::Int(v)) => Some(*v as f32),
        Some(Cell::Float(v)) => Some(*v as f32),
        _ => None,
    };
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
        float GetRemainingCooldown(object o, int n, string s) = 2;\n\
        void RemoveItem(object o, int n) = 3;\n\
        void SetFollowerState(object o, int n) = 4;\n\
        int FOLLOWER_STATE_ACTIVE = 1;\n\
        int FOLLOWER_STATE_LOCKEDACTIVE = 5;\n\
        void SetCreatureProperty(object o, int p, float f, int t) = 5;\n\
        int PROPERTY_DEPLETABLE_HEALTH = 7;\n\
        int PROPERTY_VALUE_BASE = 2;\n\
        int PROPERTY_VALUE_CURRENT = 3;\n\
        object CreateItemOnObject(resource r, object o, int n = 1, string s = \"\", int b = FALSE, int d = TRUE) = 6;\n\
        int FALSE = 0;\n\
        int TRUE = 1;\n\
        object GetObjectByTag(string s, int n = 0) = 7;\n\
        object[] GetNearestObjectByTag(object o, string s, int t = 32, int n = 1, int a = 0, int b = 0, int c = 0) = 8;\n\
        int GetFollowerState(object o) = 9;\n\
        int IsControlled(object o) = 10;\n\
        int IsPartyAIEnabled(object o) = 11;\n";

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
    fn followers_joining_the_party_take_the_heros_side() {
        let table = Arc::new(EngineTable::parse(LDF.as_bytes()).unwrap());
        let mut s = Sim::new(Arc::new(Vfs::new()), table, 1);
        let hero = s.world.spawn(ObjectKind::Creature, "hero");
        let ally = s.world.spawn(ObjectKind::Creature, "ally");
        s.world.hero = Some(hero);
        s.world.get_mut(ally).unwrap().group = 2;
        run(
            &mut s,
            hero,
            "SetFollowerState",
            &[Value::Object(ally), Value::Int(1)],
        );
        assert_eq!(s.world.get(hero).unwrap().followers, vec![ally]);
        assert_eq!(s.world.get(ally).unwrap().group, 0);
        // The hero counts as an active party member without being given a state.
        let state = |s: &mut Sim, o| run(s, hero, "GetFollowerState", &[Value::Object(o)]);
        assert_eq!(state(&mut s, hero), Some(Value::Int(1)));
        assert_eq!(state(&mut s, ally), Some(Value::Int(1)));
    }

    #[test]
    fn giving_the_dead_health_brings_them_back() {
        let table = Arc::new(EngineTable::parse(LDF.as_bytes()).unwrap());
        let mut s = Sim::new(Arc::new(Vfs::new()), table, 1);
        let hero = s.world.spawn(ObjectKind::Creature, "hero");
        let o = s.world.get_mut(hero).unwrap();
        o.dead = true;
        o.properties.insert(7, (100.0, 0.0));
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
        assert_eq!(o.properties[&7], (120.0, 25.0));
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
        assert_eq!(
            s.world.get(dropped).unwrap().fields.get("StackSize"),
            Some(&2)
        );
        let bag = s.spawn_body_bag(foe, false).unwrap();
        assert_eq!(s.world.get(bag).unwrap().inventory, vec![dropped]);
        assert_eq!(s.world.get(foe).unwrap().inventory, vec![kept]);
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
