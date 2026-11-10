//! A minimal combat core: perception, hostility, threat, attacks and death.
//!
//! The game's rules live in its scripts (`rules_core` and the AI includes): they decide what
//! to attack, roll hits and damage from the rules 2DAs, and apply damage effects. The engine
//! supplies the plumbing those scripts expect, which is what this module does:
//!
//! - **Perception.** Creatures in the hero's area perceive each other within
//!   [`PERCEPTION_RANGE`] and get `EVENT_TYPE_PERCEPTION_APPEAR` (object 0 is the creature
//!   seen; integers are hostility changed, stealthed, hostile) and `PERCEPTION_DISAPPEAR`
//!   when it leaves the range.
//! - **Hostility.** Creatures are hostile when their groups are set hostile with
//!   `SetGroupHostility`.
//! - **Attacks.** When a `CommandAttack` reaches the front of the queue, its owner gets
//!   `EVENT_TYPE_COMMAND_PENDING` (objects: owner, target; integers: command type, id). The
//!   script answers with `SetAttackResult` and `SetAttackDuration`. When the attack's time is
//!   up, the owner gets `EVENT_TYPE_ATTACK_IMPACT` (objects: attacker, target; integer 0: the
//!   hit result), and `GetAttackImpactDamageEffect` returns the damage effect the script set.
//! - **Effects.** `ApplyEffectOnObject` runs the target's `EVENT_TYPE_APPLY_EFFECT` handler at
//!   once, with `GetCurrentEffect` returning the effect. Unless the handler calls
//!   `SetIsCurrentEffectValid(FALSE)`, an effect with a duration stays on the target.
//! - **Death.** An `EFFECT_TYPE_DEATH` that its handler accepts kills the target (the rules
//!   scripts turn a lethal hit into one). A creature whose current health (`PROPERTY_DEPLETABLE_HEALTH`) drops to zero
//!   or below and is neither immortal nor plot dies and gets `EVENT_TYPE_DEATH`, with the
//!   killer as creator and object 0.
//!
//! All of this is provisional: event layouts are read from how `rules_core` reads them, and
//! the range and default attack time are guesses, not values taken from the executable.

use serde::{Deserialize, Serialize};
use thedas_script::vm::{OBJECT_INVALID, ObjectId};

use crate::sim::Sim;
use crate::world::{Effect, ObjectKind};

/// How far creatures see each other, in metres. Provisional.
pub const PERCEPTION_RANGE: f32 = 20.0;

/// `EFFECT_TYPE_DEATH` in effects.gda.
pub const EFFECT_TYPE_DEATH: i32 = 7;

/// How long an attack takes when the script does not say (`SetAttackDuration`). Provisional.
pub const DEFAULT_ATTACK_MS: u64 = 1500;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CombatData {
    /// `GetCombatState`.
    pub in_combat: bool,
    /// Creatures this one currently perceives.
    pub perceived: Vec<ObjectId>,
    /// Threat by enemy.
    pub threat: Vec<(ObjectId, f32)>,
    /// What the script decided for the attack in progress (`SetAttackResult`).
    pub attack: Option<AttackResult>,
    /// `SetAttackDuration` for the attack in progress.
    pub attack_ms: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AttackResult {
    pub results: [i32; 2],
    pub effects: [Option<Effect>; 2],
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

impl Sim {
    /// Whether two objects are hostile: both creatures, in groups set hostile.
    pub fn hostile(&self, a: ObjectId, b: ObjectId) -> bool {
        let (Some(x), Some(y)) = (self.world.get(a), self.world.get(b)) else {
            return false;
        };
        if x.kind != ObjectKind::Creature || y.kind != ObjectKind::Creature || a == b {
            return false;
        }
        let (g, h) = (x.group, y.group);
        g != h
            && self
                .world
                .hostility
                .get(&(g.min(h), g.max(h)))
                .copied()
                .unwrap_or(false)
    }

    fn alive(&self, id: ObjectId) -> bool {
        self.world
            .get(id)
            .is_some_and(|o| o.kind == ObjectKind::Creature && o.active && !o.dead)
    }

    /// Updates who sees whom in the hero's area and queues perception events.
    pub fn update_perception(&mut self) {
        let Some(area) = self
            .world
            .hero
            .and_then(|h| self.world.get(h))
            .and_then(|h| h.area)
        else {
            return;
        };
        let creatures: Vec<(ObjectId, [f32; 3])> = self
            .world
            .objects
            .values()
            .filter(|o| o.kind == ObjectKind::Creature && o.area == Some(area))
            .map(|o| (o.id, o.position))
            .collect();
        for &(id, pos) in &creatures {
            let alive = self.alive(id);
            let now: Vec<ObjectId> = if alive {
                creatures
                    .iter()
                    .filter(|&&(o, p)| o != id && self.alive(o) && dist(pos, p) <= PERCEPTION_RANGE)
                    .map(|&(o, _)| o)
                    .collect()
            } else {
                Vec::new()
            };
            let before = self
                .world
                .get(id)
                .map(|o| o.combat.perceived.clone())
                .unwrap_or_default();
            if now == before {
                continue;
            }
            for &o in now.iter().filter(|o| !before.contains(o)) {
                self.perceive(id, o, true);
            }
            for &o in before.iter().filter(|o| !now.contains(o)) {
                self.perceive(id, o, false);
            }
            if let Some(obj) = self.world.get_mut(id) {
                obj.combat.perceived = now;
            }
        }
    }

    /// Queues `PERCEPTION_APPEAR` (or `DISAPPEAR`) for `who` about `seen`.
    pub fn perceive(&mut self, who: ObjectId, seen: ObjectId, appear: bool) {
        let name = if appear {
            "PERCEPTION_APPEAR"
        } else {
            "PERCEPTION_DISAPPEAR"
        };
        let mut ev = self.new_event(name, Some(seen));
        ev.objects = vec![seen];
        ev.ints = vec![0, 0, self.hostile(who, seen) as i32];
        self.world.queue_event(0, who, ev, None);
    }

    /// Runs a target's `APPLY_EFFECT` handler for an effect, keeps it if it lasts, and
    /// checks for death afterwards.
    pub fn apply_effect(&mut self, target: ObjectId, effect: Effect) {
        if self.world.get(target).is_none() {
            return;
        }
        let creator = effect.creator;
        let t = self.world.time_ms;
        self.world.trace.push(crate::trace::TraceEntry::Log {
            t,
            channel: -4,
            text: format!(
                "effect {} on {target} from {:?} ({:?})",
                effect.ty, creator, effect.floats
            ),
        });
        let mut ev = self.new_event("APPLY_EFFECT", creator);
        ev.objects = vec![target];
        let saved = self.current_effect.replace((effect.clone(), true));
        let script = self.world.get(target).and_then(|o| o.script.clone());
        if let Some(script) = script {
            self.run_script(&script, target, Some(ev));
        }
        let (effect, valid) =
            std::mem::replace(&mut self.current_effect, saved).unwrap_or((effect, true));
        // A lethal hit arrives as EFFECT_TYPE_DEATH (7 in effects.gda) rather than as damage
        // that empties the health bar: the engine does the killing.
        if valid && effect.ty == EFFECT_TYPE_DEATH {
            self.kill(target, creator.unwrap_or(OBJECT_INVALID));
            return;
        }
        let instant = constant_of(self, "EFFECT_DURATION_TYPE_INSTANT");
        if valid && effect.duration_type != instant {
            if let Some(o) = self.world.get_mut(target) {
                o.effects.push(effect);
            }
        }
        self.check_death(target, creator.unwrap_or(OBJECT_INVALID));
    }

    /// Kills a creature whose health ran out.
    pub fn check_death(&mut self, id: ObjectId, killer: ObjectId) {
        let health = constant_of(self, "PROPERTY_DEPLETABLE_HEALTH");
        let Some(o) = self.world.get(id) else { return };
        if o.kind != ObjectKind::Creature || o.dead || o.immortal || o.plot {
            return;
        }
        let Some(&(_, current)) = o.properties.get(&health) else {
            return;
        };
        if current > 0.0 {
            return;
        }
        self.kill(id, killer);
    }

    /// Marks a creature dead, clears what it was doing and sends it `EVENT_TYPE_DEATH`.
    pub fn kill(&mut self, id: ObjectId, killer: ObjectId) {
        if let Some(o) = self.world.get_mut(id) {
            if o.dead {
                return;
            }
            o.dead = true;
            o.commands.clear();
            o.combat.attack = None;
            o.combat.in_combat = false;
        }
        let mut ev = self.new_event("DEATH", Some(killer));
        ev.objects = vec![killer];
        self.world.queue_event(0, id, ev, None);
    }

    /// Orders a creature to attack another, as the player does by clicking an enemy.
    pub fn order_attack(&mut self, attacker: ObjectId, target: ObjectId) {
        let cmd = crate::world::Command {
            ty: constant_of(self, "COMMAND_TYPE_ATTACK"),
            name: "CommandAttack".into(),
            target: Some(target),
            location: None,
            seconds: 0.0,
            ints: Vec::new(),
            strings: Vec::new(),
            due: None,
        };
        if let Some(o) = self.world.get_mut(attacker) {
            o.commands.clear();
            o.commands.push(cmd);
            o.combat.in_combat = true;
        }
    }

    /// The front command started: an attack asks its owner's script for the result.
    pub(crate) fn start_attack(&mut self, id: ObjectId, target: ObjectId, ty: i32) {
        let mut ev = self.new_event("COMMAND_PENDING", Some(id));
        ev.objects = vec![id, target];
        ev.ints = vec![ty, 0];
        if let Some(o) = self.world.get_mut(id) {
            o.combat.attack = None;
            o.combat.attack_ms = None;
        }
        let script = self.world.get(id).and_then(|o| o.script.clone());
        if let Some(script) = script {
            self.run_script(&script, id, Some(ev));
        }
    }

    /// The attack's time is up: its owner applies the impact.
    pub(crate) fn finish_attack(&mut self, id: ObjectId, target: ObjectId) {
        let Some(result) = self.world.get(id).and_then(|o| o.combat.attack.clone()) else {
            return;
        };
        if !self.alive(id) {
            return;
        }
        let t = self.world.time_ms;
        self.world.trace.push(crate::trace::TraceEntry::Log {
            t,
            channel: -4,
            text: format!(
                "attack {id} -> {target}: results {:?}, effects {:?}",
                result.results,
                result
                    .effects
                    .iter()
                    .map(|e| e.as_ref().map(|e| (e.ty, e.floats.clone())))
                    .collect::<Vec<_>>()
            ),
        });
        let mut ev = self.new_event("ATTACK_IMPACT", Some(id));
        ev.objects = vec![id, target];
        ev.ints = vec![result.results[0], result.results[1]];
        let script = self.world.get(id).and_then(|o| o.script.clone());
        if let Some(script) = script {
            self.run_script(&script, id, Some(ev));
        }
        if let Some(o) = self.world.get_mut(id) {
            o.combat.attack = None;
        }
    }
}

fn constant_of(sim: &Sim, name: &str) -> i32 {
    crate::sim::constant(&sim.table, name)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use thedas_script::EngineTable;
    use thedas_vfs::Vfs;

    use super::*;

    const LDF: &str = "int EVENT_TYPE_PERCEPTION_APPEAR = 25;\n\
        int EVENT_TYPE_PERCEPTION_DISAPPEAR = 26;\n\
        int EVENT_TYPE_DEATH = 4;\n\
        int PROPERTY_DEPLETABLE_HEALTH = 7;\n";

    fn sim() -> Sim {
        let table = Arc::new(EngineTable::parse(LDF.as_bytes()).unwrap());
        Sim::new(Arc::new(Vfs::new()), table, 3)
    }

    #[test]
    fn creatures_perceive_hostiles_in_range_and_die_at_zero_health() {
        let mut s = sim();
        let area = s.world.spawn(ObjectKind::Area, "a");
        let hero = s.world.spawn(ObjectKind::Creature, "hero");
        let foe = s.world.spawn(ObjectKind::Creature, "foe");
        let far = s.world.spawn(ObjectKind::Creature, "far");
        s.world.hero = Some(hero);
        for (id, x, g) in [(hero, 0.0, 0), (foe, 5.0, 2), (far, 50.0, 2)] {
            let o = s.world.get_mut(id).unwrap();
            o.area = Some(area);
            o.position = [x, 0.0, 0.0];
            o.group = g;
        }
        s.world.hostility.insert((0, 2), true);
        assert!(s.hostile(hero, foe) && !s.hostile(foe, far));
        s.update_perception();
        assert_eq!(s.world.get(hero).unwrap().combat.perceived, vec![foe]);
        let appear: Vec<_> = s
            .world
            .queue
            .values()
            .map(|q| (q.target, q.event.ty, q.event.ints.clone()))
            .collect();
        assert!(appear.contains(&(hero, 25, vec![0, 0, 1])));
        assert!(appear.contains(&(foe, 25, vec![0, 0, 1])));
        s.world.queue.clear();

        s.world
            .get_mut(foe)
            .unwrap()
            .properties
            .insert(7, (10.0, 0.0));
        s.check_death(foe, hero);
        assert!(s.world.get(foe).unwrap().dead);
        let death: Vec<_> = s
            .world
            .queue
            .values()
            .map(|q| (q.target, q.event.ty))
            .collect();
        assert_eq!(death, vec![(foe, 4)]);
        // The dead drop out of perception.
        s.world.queue.clear();
        s.update_perception();
        assert!(s.world.get(hero).unwrap().combat.perceived.is_empty());
    }
}
