//! DA2's script-visible AI priority (0x4e5b10, 0x539ac0, 0x662c80).

use thedas_script::vm::ObjectId;

use crate::Sim;
use crate::world::{Object, ObjectKind};

fn locked(o: &Object) -> bool {
    o.fields
        .get("AILevelLocked")
        .copied()
        .unwrap_or_else(|| i32::from(o.fields.get("AILevel").copied().unwrap_or(-1) != -1))
        != 0
}

/// 0x60dbc0: death takes its own branch, bypassing the two living bonuses.
fn priority(base: i32, dead: bool, player_group: bool, model_state: u32) -> i32 {
    let value = if dead {
        base - 1
    } else {
        base + i32::from(player_group) + i32::from(model_state == 0)
    };
    value.clamp(0, 4)
}

impl Sim {
    /// The signed byte at native object +0x30c. A script can lock its current
    /// priority; otherwise the creature's automatic assignment supplies it.
    pub(crate) fn da2_ai_level(&self, id: ObjectId) -> i32 {
        let Some(o) = self.world.get(id) else {
            return -1;
        };
        // Older snapshots only recorded SetAILevel's value. -1 means automatic.
        if locked(o) || o.kind != ObjectKind::Creature {
            return i32::from(o.fields.get("AILevel").copied().unwrap_or(-1) as i8);
        }
        self.da2_automatic_ai_level(id)
    }

    fn da2_automatic_ai_level(&self, id: ObjectId) -> i32 {
        let Some(o) = self
            .world
            .get(id)
            .filter(|o| o.kind == ObjectKind::Creature)
        else {
            // Other CCObject kinds inherit the empty automatic-assignment virtual.
            return -1;
        };
        if self.world.controlled() == Some(id) {
            return 4;
        }
        // Simulated creatures use the ordinary loaded model state (0). Native
        // fade/appearance transitions and owned-creature group inheritance are
        // not represented; activation is independent of this model-state query.
        let adjust = |base| priority(base, o.dead, o.group == 1, 0);
        if o.combat.in_combat {
            return adjust(2).min(3);
        }
        let party = crate::engine::party_of(self);
        let escort = self
            .world
            .hero
            .and_then(|hero| self.world.get(hero))
            .is_some_and(|hero| hero.escorts.contains(&id));
        if party.contains(&id) || escort {
            return 3;
        }
        let controlled_area = self
            .world
            .controlled()
            .and_then(|who| self.world.get(who))
            .and_then(|o| o.area);
        if controlled_area.is_some() && controlled_area == o.area {
            adjust(1)
        } else {
            -1
        }
    }

    pub(crate) fn da2_set_ai_level(&mut self, id: ObjectId, level: i32) {
        if self.world.get(id).is_none() {
            return;
        }
        if level == -1 {
            let o = self.world.get_mut(id).unwrap();
            // Non-creatures inherit a no-op automatic virtual: their byte stays.
            o.fields.insert("AILevelLocked".into(), 0);
            return;
        }
        let current = self.da2_ai_level(id);
        let o = self.world.get_mut(id).unwrap();
        if !locked(o) {
            // The manager rejects values outside 0..=4, but SetAILevel still
            // sets the lock bit after that failed assignment (0x4dbe24).
            o.fields.insert(
                "AILevel".into(),
                if (0..=4).contains(&level) {
                    level
                } else {
                    current
                },
            );
        }
        o.fields.insert("AILevelLocked".into(), 1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::ScriptHost;
    use std::sync::Arc;
    use thedas_script::{
        EngineTable,
        vm::{OBJECT_INVALID, Value},
    };
    use thedas_vfs::{Game, Vfs};

    fn sim() -> (Sim, ObjectId, ObjectId, ObjectId) {
        let table = Arc::new(
            EngineTable::parse(
                b"int GetAILevel(object o) = 221;\nvoid SetAILevel(object o, int n) = 222;\n",
            )
            .unwrap(),
        );
        let mut s = Sim::new(Arc::new(Vfs::new()), table, 1);
        s.game = Game::DragonAge2;
        let area = s.world.spawn(ObjectKind::Area, "area");
        let hero = s.world.spawn(ObjectKind::Creature, "hero");
        let follower = s.world.spawn(ObjectKind::Creature, "follower");
        let npc = s.world.spawn(ObjectKind::Creature, "npc");
        s.world.hero = Some(hero);
        s.world.get_mut(hero).unwrap().followers.push(follower);
        for id in [hero, follower, npc] {
            s.world.get_mut(id).unwrap().area = Some(area);
        }
        s.world.get_mut(hero).unwrap().group = 1;
        s.world.get_mut(follower).unwrap().group = 1;
        (s, hero, follower, npc)
    }

    fn call(s: &mut Sim, id: ObjectId, level: Option<i32>) -> Option<Value> {
        let name = if level.is_some() {
            "SetAILevel"
        } else {
            "GetAILevel"
        };
        let f = s.table.function_by_name(name).cloned().unwrap();
        let mut args = vec![Value::Object(id)];
        if let Some(level) = level {
            args.push(Value::Int(level));
        }
        let mut host = ScriptHost {
            sim: s,
            event: None,
            script: "test".into(),
        };
        crate::engine::call(&mut host, id, &f, &args)
            .unwrap()
            .unwrap()
    }

    #[test]
    fn native_priority_bonuses_apply_only_to_living_creatures() {
        assert_eq!(priority(2, false, false, 0), 3);
        assert_eq!(priority(2, false, false, 255), 2);
        assert_eq!(priority(2, false, true, 0), 4);
        assert_eq!(priority(2, false, true, 1), 3);
        for group in [false, true] {
            for model in [0, 1, 255] {
                assert_eq!(priority(2, true, group, model), 1);
                assert_eq!(priority(1, true, group, model), 0);
            }
        }
    }

    #[test]
    fn da2_ai_level_uses_automatic_assignments_without_a_script_setter() {
        let (mut s, hero, follower, npc) = sim();
        assert_eq!(call(&mut s, OBJECT_INVALID, None), Some(Value::Int(-1)));
        assert_eq!(call(&mut s, hero, None), Some(Value::Int(4)));
        assert_eq!(call(&mut s, follower, None), Some(Value::Int(3)));
        assert_eq!(call(&mut s, npc, None), Some(Value::Int(2)));
        s.world.get_mut(npc).unwrap().combat.in_combat = true;
        assert_eq!(call(&mut s, npc, None), Some(Value::Int(3)));
        s.world.get_mut(npc).unwrap().group = 1;
        assert_eq!(call(&mut s, npc, None), Some(Value::Int(3)));
        s.world.get_mut(npc).unwrap().dead = true;
        assert_eq!(call(&mut s, npc, None), Some(Value::Int(1)));
        s.world.get_mut(npc).unwrap().active = false;
        assert_eq!(call(&mut s, npc, None), Some(Value::Int(1)));
        s.world.get_mut(npc).unwrap().combat.in_combat = false;
        s.world.get_mut(npc).unwrap().area = None;
        assert_eq!(call(&mut s, npc, None), Some(Value::Int(-1)));
        s.set_controlled(Some(follower));
        assert_eq!(call(&mut s, follower, None), Some(Value::Int(4)));
        assert_eq!(call(&mut s, hero, None), Some(Value::Int(3)));
        let placeable = s.world.spawn(ObjectKind::Placeable, "placeable");
        assert_eq!(call(&mut s, placeable, None), Some(Value::Int(-1)));
        s.load_json(&s.save_json().unwrap()).unwrap();
        assert_eq!(call(&mut s, follower, None), Some(Value::Int(4)));
        assert!(s.world.trace.unimplemented.is_empty());
    }

    #[test]
    fn da2_ai_level_override_locks_until_minus_one_and_survives_reload() {
        let (mut s, _, follower, npc) = sim();
        call(&mut s, follower, Some(0));
        call(&mut s, follower, Some(4));
        s.world.get_mut(follower).unwrap().combat.in_combat = true;
        assert_eq!(call(&mut s, follower, None), Some(Value::Int(0)));
        s.load_json(&s.save_json().unwrap()).unwrap();
        assert_eq!(call(&mut s, follower, None), Some(Value::Int(0)));
        call(&mut s, follower, Some(-1));
        assert_eq!(call(&mut s, follower, None), Some(Value::Int(3)));
        call(&mut s, follower, Some(4));
        assert_eq!(call(&mut s, follower, None), Some(Value::Int(4)));
        call(&mut s, npc, Some(300));
        s.world.get_mut(npc).unwrap().combat.in_combat = true;
        assert_eq!(call(&mut s, npc, None), Some(Value::Int(2)));
        call(&mut s, npc, Some(-1));
        assert_eq!(call(&mut s, npc, None), Some(Value::Int(3)));
        let placeable = s.world.spawn(ObjectKind::Placeable, "placeable");
        call(&mut s, placeable, Some(2));
        call(&mut s, placeable, Some(-1));
        assert_eq!(call(&mut s, placeable, None), Some(Value::Int(2)));
        call(&mut s, placeable, Some(3));
        assert_eq!(call(&mut s, placeable, None), Some(Value::Int(3)));
        call(&mut s, OBJECT_INVALID, Some(2));
        s.world.get_mut(npc).unwrap().fields.remove("AILevelLocked");
        // A legacy snapshot's field is read as the native signed byte.
        s.world
            .get_mut(npc)
            .unwrap()
            .fields
            .insert("AILevel".into(), 255);
        assert_eq!(call(&mut s, npc, None), Some(Value::Int(-1)));
    }
}
