//! Lasting areas of effect: the objects `EffectAreaOfEffect` leaves on the ground or around a
//! creature (Grease, Blizzard, Inferno, auras, traps), as DAOrigins.exe keeps them
//! (`CAreaOfEffectObject`, vtable 0xb23854).
//!
//! - Applying an area-of-effect effect (type 5) at a location or to an object (0x7a0230)
//!   makes the object: its `VFX Persistent` row (M2DA 96, the effect's integer 0) gives the
//!   shape (0x7e4da0): `C` a circle of `Radius`, `R` a rectangle `Width` across by `Length`
//!   along its facing. The effect's string 0 is its script; its creator (if a creature) and
//!   ability are the object's. Applied at a location it stays there; applied to an object
//!   it goes with that object (0x7e5d80).
//! - Its first update sends it one `EVENT_TYPE_AOE_HEARTBEAT` (0x7e4b63), with the creator,
//!   the ability (integer 0) and itself (object 0); the scripts send the later ones
//!   themselves (`spell_aoe_duration` re-delays the event it is handling).
//! - Living, active creatures in its area that come inside it join it and it gets
//!   `EVENT_TYPE_ENTER`; members that are outside leave and it gets `EVENT_TYPE_EXIT`
//!   (0x7e5950, events from 0x698850: the creator, the creature as target, the ability as
//!   integer 0, itself as object 0). The dead are passed over and stay members. Inside a
//!   circle is within its radius in 3D (0x451990).
//! - It goes when its effect does (0x7a0520): a temporary one at the end of its duration,
//!   or when a script removes it or destroys the object. Going, it sends EXIT at once to the
//!   members still inside (0x7e5560 from 0x7e5670), and its effect is removed (0x7e4a50).
//!
//! Provisional: membership is brought up to date once a tick (the engine does it as objects
//! move); creatures are taken in id order (the engine walks its area's list sorted by x);
//! the rectangle's axes (width across the facing, length along it) are taken from the
//! corner offsets (0x7e60d0); a row's script when the effect names none is the row's script
//! column (hash 0x53e76f70); the engine's gate on ENTER for those inside when it appears
//! (event switch 8 at 0x7a043d) is taken to leave ENTER on; and an ENTER is not held back in
//! conversations (0x6a2960).

use serde::{Deserialize, Serialize};
use thedas_formats::gda::Cell;
use thedas_script::vm::{OBJECT_INVALID, ObjectId};

use crate::sim::Sim;
use crate::world::{Effect, ObjectKind};

/// The M2DA id of `VFX Persistent`, whose rows describe areas of effect.
pub const VFX_PERSISTENT_TABLE: i32 = 96;
/// `VFX Persistent`'s script column (name unknown).
const COL_SCRIPT: u32 = 0x53e7_6f70;
/// The effect type of `EffectAreaOfEffect` (`Area of Effect (AOE)` in effects.gda).
pub const EFFECT_TYPE_AOE: i32 = 5;

/// The ground an area of effect covers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum AoeShape {
    /// A row with no shape the engine knows: nothing is inside.
    #[default]
    None,
    /// A circle (sphere) of this radius, metres.
    Circle(f32),
    /// A rectangle `width` across by `length` along the facing, centred on the position.
    Rect { width: f32, length: f32 },
}

/// What an area-of-effect object keeps (see the module docs).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AoeState {
    /// The `VFX Persistent` row.
    pub row: i32,
    pub shape: AoeShape,
    /// The effect's creator, if a creature (`OBJECT_INVALID` otherwise).
    pub creator: ObjectId,
    pub ability: i32,
    /// The object it goes with, when applied to one rather than at a location.
    pub follows: Option<ObjectId>,
    /// When it goes (simulation ms), for one at a location with a temporary effect; one on
    /// an object goes with its effect on that object.
    pub expires: Option<u64>,
    /// The creatures inside it, in the order they came in.
    pub members: Vec<ObjectId>,
}

impl AoeShape {
    /// Whether `p` is inside the shape placed at `center` facing `facing` (degrees).
    pub fn contains(&self, center: [f32; 3], facing: f32, p: [f32; 3]) -> bool {
        match *self {
            AoeShape::None => false,
            AoeShape::Circle(r) => crate::engine::dist(center, p) <= r,
            AoeShape::Rect { width, length } => {
                let (dx, dy) = (p[0] - center[0], p[1] - center[1]);
                let h = crate::world::heading_of(facing).to_radians();
                let (along, across) = (dx * h.cos() + dy * h.sin(), dx * h.sin() - dy * h.cos());
                along.abs() <= length * 0.5 && across.abs() <= width * 0.5
            }
        }
    }
}

impl Sim {
    /// Makes the area of effect an effect of type 5 leaves at `position` in `area` (facing
    /// `facing`), or on `follows`; returns the object. `expires` is when a location's
    /// temporary effect runs out.
    pub fn create_aoe(
        &mut self,
        effect: &Effect,
        area: ObjectId,
        position: [f32; 3],
        facing: f32,
        follows: Option<ObjectId>,
        expires: Option<u64>,
    ) -> ObjectId {
        let row = effect.ints.first().copied().unwrap_or(0);
        let table = self.world.tables.get(VFX_PERSISTENT_TABLE, "");
        let float = |c: &str| match table.as_ref().and_then(|t| t.cell(c, row as i64)) {
            Some(Cell::Float(v)) => *v as f32,
            Some(Cell::Int(v)) => *v as f32,
            _ => 0.0,
        };
        let text = |c: Option<&Cell>| match c {
            Some(Cell::String(s)) => s
                .trim_matches(|c: char| c == '\0' || c.is_whitespace())
                .to_string(),
            _ => String::new(),
        };
        let shape = match text(table.as_ref().and_then(|t| t.cell("Shape", row as i64))).as_str() {
            "C" => AoeShape::Circle(float("Radius")),
            "R" => AoeShape::Rect {
                width: float("Width"),
                length: float("Length"),
            },
            _ => AoeShape::None,
        };
        let label = text(table.as_ref().and_then(|t| t.cell("Label", row as i64)));
        let script = effect
            .strings
            .first()
            .filter(|s| !s.is_empty())
            .cloned()
            .or_else(|| {
                Some(text(
                    table
                        .as_ref()
                        .and_then(|t| t.cell_by_hash(COL_SCRIPT, row as i64)),
                ))
                .filter(|s| !s.is_empty())
            })
            .map(|s| s.trim_end_matches(".ncs").to_string());
        let creator = effect
            .creator
            .filter(|&c| {
                self.world
                    .get(c)
                    .is_some_and(|o| o.kind == ObjectKind::Creature)
            })
            .unwrap_or(OBJECT_INVALID);
        let id = self.world.spawn(ObjectKind::AreaOfEffect, &label);
        if let Some(o) = self.world.get_mut(id) {
            o.area = Some(area);
            o.position = position;
            o.facing = facing;
            o.script = script.clone();
            o.aoe = Some(AoeState {
                row,
                shape,
                creator,
                ability: effect.ability,
                follows,
                expires,
                members: Vec::new(),
            });
        }
        // The first update's heartbeat.
        let mut ev = self.new_event("AOE_HEARTBEAT", Some(creator));
        ev.ints = vec![effect.ability];
        ev.objects = vec![id];
        self.world.queue_event(0, id, ev, script);
        self.scan_aoe(id);
        id
    }

    /// Brings every area of effect up to date: those whose time is up go, those on an object
    /// move with it, and creatures come in and go out.
    pub fn update_aoes(&mut self) {
        let now = self.world.time_ms;
        let ids: Vec<ObjectId> = self
            .world
            .objects
            .values()
            .filter(|o| o.kind == ObjectKind::AreaOfEffect)
            .map(|o| o.id)
            .collect();
        for id in ids {
            let Some(state) = self.world.get(id).and_then(|o| o.aoe.clone()) else {
                continue;
            };
            if state.expires.is_some_and(|t| t <= now) {
                self.destroy_aoe(id);
                continue;
            }
            if let Some(owner) = state.follows {
                let at = self.world.get(owner).map(|o| (o.position, o.area));
                if let (Some((p, Some(area))), Some(o)) = (at, self.world.get_mut(id)) {
                    if o.area == Some(area) {
                        o.position = p;
                    }
                }
            }
            self.scan_aoe(id);
        }
    }

    /// Sends ENTER for living, active creatures newly inside an area of effect and EXIT for
    /// members now outside (queued, as the engine queues them).
    fn scan_aoe(&mut self, id: ObjectId) {
        let Some(o) = self.world.get(id) else { return };
        let Some(state) = o.aoe.clone() else { return };
        let (area, center, facing) = (o.area, o.position, o.facing);
        let script = o.script.clone();
        let mut members = state.members.clone();
        let mut changes: Vec<(ObjectId, bool)> = Vec::new();
        for c in self.world.objects.values() {
            if c.kind != ObjectKind::Creature || c.area != area || !c.active || c.dead {
                continue;
            }
            let inside = state.shape.contains(center, facing, c.position);
            let member = members.contains(&c.id);
            if inside && !member {
                members.push(c.id);
                changes.push((c.id, true));
            } else if !inside && member {
                members.retain(|&m| m != c.id);
                changes.push((c.id, false));
            }
        }
        if let Some(s) = self.world.get_mut(id).and_then(|o| o.aoe.as_mut()) {
            s.members = members;
        }
        for (c, enter) in changes {
            let ev = self.aoe_event(id, &state, c, enter);
            self.world.queue_event(0, id, ev, script.clone());
        }
    }

    fn aoe_event(
        &self,
        id: ObjectId,
        state: &AoeState,
        creature: ObjectId,
        enter: bool,
    ) -> crate::world::Event {
        let mut ev = self.new_event(if enter { "ENTER" } else { "EXIT" }, Some(state.creator));
        ev.target = Some(creature);
        ev.ints = vec![state.ability];
        ev.objects = vec![id];
        ev
    }

    /// Removes an area of effect: members still inside get EXIT at once, then the object
    /// goes, and with it its effect on the object it was on.
    pub fn destroy_aoe(&mut self, id: ObjectId) {
        let Some(o) = self.world.get_mut(id) else {
            return;
        };
        let (center, facing, script) = (o.position, o.facing, o.script.clone());
        // Taken out first, so a script that destroys it again while it goes finds nothing.
        let Some(state) = o.aoe.take() else { return };
        for &c in &state.members {
            let inside = self
                .world
                .get(c)
                .is_some_and(|m| state.shape.contains(center, facing, m.position));
            if inside && self.world.get(id).is_some() {
                let ev = self.aoe_event(id, &state, c, false);
                self.deliver(id, ev, script.clone());
            }
        }
        self.world.objects.remove(&id);
        if let Some(owner) = state.follows {
            let linked: Vec<Effect> = self
                .world
                .get(owner)
                .map(|o| {
                    o.effects
                        .iter()
                        .filter(|e| e.ty == EFFECT_TYPE_AOE && e.objects.first() == Some(&id))
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
            for e in linked {
                self.remove_effect(owner, &e);
            }
        }
    }

    /// The creatures inside an area of effect (`GetCreaturesInAOE`), in id order.
    pub fn aoe_members(&self, id: ObjectId) -> Vec<ObjectId> {
        let Some(o) = self.world.get(id) else {
            return Vec::new();
        };
        let mut v: Vec<ObjectId> = o
            .aoe
            .as_ref()
            .map(|s| s.members.clone())
            .unwrap_or_default()
            .into_iter()
            .filter(|&m| self.world.get(m).is_some_and(|c| c.area == o.area))
            .collect();
        v.sort_unstable();
        v
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use thedas_script::EngineTable;
    use thedas_script::vm::Value;
    use thedas_vfs::Vfs;

    use super::*;
    use crate::sim::ScriptHost;

    const LDF: &str = "int FALSE = 0;\n\
        int TRUE = 1;\n\
        int EFFECT_DURATION_TYPE_INSTANT = 1;\n\
        int EFFECT_DURATION_TYPE_TEMPORARY = 2;\n\
        int EFFECT_DURATION_TYPE_PERMANENT = 3;\n\
        int EVENT_TYPE_ENTER = 8;\n\
        int EVENT_TYPE_EXIT = 9;\n\
        int EVENT_TYPE_AOE_HEARTBEAT = 74;\n\
        #define ENGINE_STRUCTURE_1 location\n\
        #define ENGINE_STRUCTURE_3 effect\n\
        location Location(object a, vector v, float f) = 41;\n\
        effect EffectAreaOfEffect(int n, resource r, int v = 0) = 70;\n\
        void Engine_ApplyEffectAtLocation(int d, effect e, location l, float f = 0.0, object c = OBJECT_INVALID, int n = 0) = 71;\n\
        void Engine_ApplyEffectOnObject(int d, effect e, object o, float f = 0.0, object c = OBJECT_INVALID, int n = 0) = 72;\n\
        object[] GetCreaturesInAOE(object o) = 73;\n\
        int IsInAOE(object c, object o) = 74;\n\
        void DestroyObject(object o, int n = 0) = 75;\n";

    fn sim() -> Sim {
        let table = Arc::new(EngineTable::parse(LDF.as_bytes()).unwrap());
        let mut s = Sim::new(Arc::new(Vfs::new()), table, 1);
        let t = crate::m2da::Tables::empty();
        let row = |id: i64, shape: &str, r: f64, w: f64, l: f64| {
            (
                id,
                vec![
                    Cell::Int(id),
                    Cell::String(format!("AOE_{id}\0")),
                    Cell::String(format!("{shape}\0")),
                    Cell::Float(r),
                    Cell::Float(w),
                    Cell::Float(l),
                    Cell::String("aoe_core\0".into()),
                ],
            )
        };
        t.insert(
            "#96",
            &[
                "ID",
                "Label",
                "Shape",
                "Radius",
                "Width",
                "Length",
                "#53e76f70",
            ],
            vec![row(203, "C", 2.0, 0.0, 0.0), row(16, "R", 0.0, 1.5, 9.0)],
        );
        s.world.tables = Arc::new(t);
        s
    }

    fn run(s: &mut Sim, caller: ObjectId, name: &str, a: &[Value]) -> Option<Value> {
        let f = s.table.function_by_name(name).cloned().unwrap();
        let mut h = ScriptHost {
            sim: s,
            event: None,
            script: "test".into(),
        };
        crate::engine::call(&mut h, caller, &f, a).unwrap().unwrap()
    }

    fn creature(s: &mut Sim, area: ObjectId, p: [f32; 3]) -> ObjectId {
        let c = s.world.spawn(ObjectKind::Creature, "c");
        let o = s.world.get_mut(c).unwrap();
        (o.area, o.position) = (Some(area), p);
        c
    }

    /// The events queued for `aoe`: (type, target, creator, integer 0, object 0).
    fn queued(
        s: &Sim,
        aoe: ObjectId,
    ) -> Vec<(i32, Option<ObjectId>, Option<ObjectId>, i32, ObjectId)> {
        s.world
            .queue
            .values()
            .filter(|q| q.target == aoe)
            .map(|q| {
                let e = &q.event;
                (
                    e.ty,
                    e.target,
                    e.creator,
                    e.ints.first().copied().unwrap_or(-1),
                    e.objects.first().copied().unwrap_or(OBJECT_INVALID),
                )
            })
            .collect()
    }

    fn aoes(s: &Sim) -> Vec<ObjectId> {
        s.world
            .objects
            .values()
            .filter(|o| o.kind == ObjectKind::AreaOfEffect)
            .map(|o| o.id)
            .collect()
    }

    #[test]
    fn an_area_of_effect_at_a_location_tracks_who_is_inside_and_runs_out() {
        let mut s = sim();
        let area = s.world.spawn(ObjectKind::Area, "area");
        let mage = creature(&mut s, area, [0.0, -20.0, 0.0]);
        let a = creature(&mut s, area, [1.0, 0.0, 0.0]);
        let b = creature(&mut s, area, [5.0, 0.0, 0.0]);
        let dead = creature(&mut s, area, [0.0, 1.0, 0.0]);
        s.world.get_mut(dead).unwrap().dead = true;
        let e = run(
            &mut s,
            mage,
            "EffectAreaOfEffect",
            &[
                Value::Int(203),
                Value::Resource("spell_aoe_duration.ncs".into()),
                Value::Int(0),
            ],
        )
        .unwrap();
        let l = run(
            &mut s,
            mage,
            "Location",
            &[
                Value::Object(area),
                Value::Vector([0.0; 3]),
                Value::Float(0.0),
            ],
        )
        .unwrap();
        s.world.queue.clear();
        run(
            &mut s,
            mage,
            "Engine_ApplyEffectAtLocation",
            &[
                Value::Int(2),
                e,
                l,
                Value::Float(5.0),
                Value::Object(mage),
                Value::Int(11113),
            ],
        );
        let aoe = aoes(&s)[0];
        let o = s.world.get(aoe).unwrap();
        assert_eq!(o.script.as_deref(), Some("spell_aoe_duration"));
        assert_eq!(o.tag, "AOE_203");
        assert_eq!(o.aoe.as_ref().unwrap().shape, AoeShape::Circle(2.0));
        // Its first heartbeat, then ENTER for the living creature inside.
        assert_eq!(
            queued(&s, aoe),
            vec![
                (74, None, Some(mage), 11113, aoe),
                (8, Some(a), Some(mage), 11113, aoe),
            ]
        );
        let members = |s: &mut Sim| run(s, mage, "GetCreaturesInAOE", &[Value::Object(aoe)]);
        assert_eq!(
            members(&mut s),
            Some(Value::Array {
                elem: thedas_script::ldf::Type::Object,
                items: vec![Value::Object(a)],
            })
        );
        // B walks in, A walks out.
        s.world.queue.clear();
        s.world.get_mut(b).unwrap().position = [0.0, -1.5, 0.0];
        s.world.get_mut(a).unwrap().position = [3.0, 0.0, 0.0];
        s.update_aoes();
        assert_eq!(
            queued(&s, aoe),
            vec![
                (9, Some(a), Some(mage), 11113, aoe),
                (8, Some(b), Some(mage), 11113, aoe),
            ]
        );
        assert_eq!(
            run(
                &mut s,
                mage,
                "IsInAOE",
                &[Value::Object(b), Value::Object(aoe)]
            ),
            Some(Value::Int(1))
        );
        // The effect's five seconds run out.
        s.world.time_ms += 5000;
        s.update_aoes();
        assert!(aoes(&s).is_empty());
    }

    #[test]
    fn an_area_of_effect_on_a_creature_goes_with_it_and_with_its_effect() {
        let mut s = sim();
        let area = s.world.spawn(ObjectKind::Area, "area");
        let healer = creature(&mut s, area, [0.0; 3]);
        let ally = creature(&mut s, area, [1.0, 0.0, 0.0]);
        let e = run(
            &mut s,
            healer,
            "EffectAreaOfEffect",
            &[
                Value::Int(203),
                Value::Resource(String::new()),
                Value::Int(0),
            ],
        )
        .unwrap();
        run(
            &mut s,
            healer,
            "Engine_ApplyEffectOnObject",
            &[
                Value::Int(3),
                e,
                Value::Object(healer),
                Value::Float(0.0),
                Value::Object(healer),
                Value::Int(10507),
            ],
        );
        let aoe = aoes(&s)[0];
        // No script named: the row's.
        assert_eq!(
            s.world.get(aoe).unwrap().script.as_deref(),
            Some("aoe_core")
        );
        let linked = s.world.get(healer).unwrap().effects[0].clone();
        assert_eq!(linked.objects, vec![aoe]);
        assert_eq!(s.aoe_members(aoe), vec![healer, ally]);
        // It moves with the healer: the ally is left behind.
        s.world.get_mut(healer).unwrap().position = [10.0, 0.0, 0.0];
        s.update_aoes();
        assert_eq!(s.world.get(aoe).unwrap().position, [10.0, 0.0, 0.0]);
        assert_eq!(s.aoe_members(aoe), vec![healer]);
        // Destroying it takes its effect off the healer.
        run(&mut s, healer, "DestroyObject", &[Value::Object(aoe)]);
        assert!(aoes(&s).is_empty());
        assert!(s.world.get(healer).unwrap().effects.is_empty());
        // And removing the effect takes the area away.
        let e = run(
            &mut s,
            healer,
            "EffectAreaOfEffect",
            &[
                Value::Int(203),
                Value::Resource(String::new()),
                Value::Int(0),
            ],
        )
        .unwrap();
        run(
            &mut s,
            healer,
            "Engine_ApplyEffectOnObject",
            &[Value::Int(3), e, Value::Object(healer)],
        );
        let effect = s.world.get(healer).unwrap().effects[0].clone();
        s.remove_effect(healer, &effect);
        assert!(aoes(&s).is_empty());
    }

    #[test]
    fn rectangles_lie_along_their_facing() {
        let r = AoeShape::Rect {
            width: 1.5,
            length: 9.0,
        };
        // Facing 270 looks along +x: 4.5 m each way along x, 0.75 across.
        assert!(r.contains([0.0; 3], 270.0, [4.4, 0.7, 0.0]));
        assert!(!r.contains([0.0; 3], 270.0, [4.6, 0.0, 0.0]));
        assert!(!r.contains([0.0; 3], 270.0, [0.0, 0.8, 0.0]));
        // Facing 0 looks along +y.
        assert!(r.contains([0.0; 3], 0.0, [0.7, -4.4, 0.0]));
        assert!(!AoeShape::None.contains([0.0; 3], 0.0, [0.0; 3]));
        assert!(AoeShape::Circle(2.0).contains([0.0; 3], 0.0, [0.0, 0.0, 2.0]));
        assert!(!AoeShape::Circle(2.0).contains([0.0; 3], 0.0, [0.0, 1.5, 1.5]));
    }
}
