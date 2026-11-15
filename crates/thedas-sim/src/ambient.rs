//! Origins' engine kick for the scripted ambient system (`ambient_h`): the creature update
//! (0x6be4b0, before the scripted heartbeat at 0x6be889) starts a creature's ambient
//! animations and movement once the party comes near.
//!
//! The ambient scripts keep a creature's state in its `AMBIENT_SYSTEM_STATE` local
//! (`AMBIENT_SYSTEM_ENABLED` 0x01; 0x40 while the system runs, cleared again by
//! `Ambient_Stop`). A creature whose template enables the system without starting it at
//! spawn (the dwarf commoner's mother in her chair: state 9, pattern 38) waits for the
//! engine:
//!
//! - While the state has the enabled bit and the creature is not a party member, its clock
//!   (creature +0xc84, a float of milliseconds) runs with the update's time.
//! - When the clock passes 3000 (0xb106ac; strictly greater) it goes back to 0. Unless the
//!   state has the 0x40 bit, the party (0x6a2cc0) is searched in order (0x630120) for the
//!   first member within 50 metres on the ground plane (squared x/y distance below 2500,
//!   0xb0fc58; height ignored).
//! - If one is, the creature is sent `EVENT_TYPE_AMBIENT_CONTINUE` (1026; `events_h`), made
//!   by the creature itself, with that member as object 0 (0xa5b950). `creature_core`
//!   answers with `Ambient_DoSomething`, which queues the ambient pattern's animation and
//!   sets the 0x40 bit.
//!
//! The clock keeps its value while the state is not enabled. Provisional: the exe reads the
//! state through a variable key (0x702e90 with key {0, 1}) taken here to be
//! `AMBIENT_SYSTEM_STATE` because its bits are the ambient system's; the update's other
//! guards (creature +0x2c5, the conversation check at 0x68cc30) are those of the
//! scripted heartbeat, whose active and parked conditions are used.

use crate::Sim;
use crate::world::ObjectKind;

/// `EVENT_TYPE_AMBIENT_CONTINUE` (`events_h`; not an engine constant).
pub const EVENT_TYPE_AMBIENT_CONTINUE: i32 = 1026;
/// The local holding a creature's ambient system state.
pub const STATE: &str = "AMBIENT_SYSTEM_STATE";
pub const ENABLED: i32 = 0x01;
/// Set by the ambient scripts while the system runs.
pub const RUNNING: i32 = 0x40;
/// How long the clock runs before the engine looks for the party (milliseconds).
pub const INTERVAL_MS: f32 = 3000.0;
/// The party's reach, squared (metres on the ground plane).
pub const REACH_SQUARED: f32 = 2500.0;

pub(crate) fn is_zero(v: &f32) -> bool {
    *v == 0.0
}

impl Sim {
    /// Runs the ambient clocks `ms` on and sends the creatures whose clock ran out with the
    /// party in reach `EVENT_TYPE_AMBIENT_CONTINUE`, delivered by the next pump.
    pub(crate) fn kick_ambient_creatures(&mut self, ms: u64) {
        if self.game != thedas_vfs::Game::Origins {
            return;
        }
        let party = crate::engine::party_of(self);
        let members: Vec<(thedas_script::vm::ObjectId, [f32; 3])> = party
            .iter()
            .filter_map(|&m| self.world.get(m).map(|o| (m, o.position)))
            .collect();
        let mut kicks = Vec::new();
        for o in self.world.objects.values_mut() {
            if o.kind != ObjectKind::Creature || !o.active || o.parked.is_some() {
                continue;
            }
            let state = o.locals.ints.get(STATE).copied().unwrap_or(0);
            if state & ENABLED == 0 || party.contains(&o.id) {
                continue;
            }
            o.ambient_ms += ms as f32;
            if o.ambient_ms <= INTERVAL_MS {
                continue;
            }
            o.ambient_ms = 0.0;
            if state & RUNNING != 0 {
                continue;
            }
            let near = members.iter().find(|(_, p)| {
                let (dx, dy) = (p[0] - o.position[0], p[1] - o.position[1]);
                dx * dx + dy * dy < REACH_SQUARED
            });
            if let Some(&(member, _)) = near {
                kicks.push((o.id, member));
            }
        }
        for (target, member) in kicks {
            let mut ev = crate::world::Event {
                ty: EVENT_TYPE_AMBIENT_CONTINUE,
                creator: Some(target),
                ..Default::default()
            };
            ev.objects.push(member);
            self.world.queue_event(0, target, ev, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use thedas_script::EngineTable;
    use thedas_script::vm::ObjectId;
    use thedas_vfs::Vfs;

    use super::*;
    use crate::world::Event;

    fn sim() -> (Sim, ObjectId, ObjectId) {
        let table = EngineTable::parse(b"int EVENT_TYPE_HEARTBEAT2 = 81;\n").unwrap();
        let mut s = Sim::new(Arc::new(Vfs::new()), Arc::new(table), 1);
        let hero = s.world.spawn(ObjectKind::Creature, "hero");
        s.world.hero = Some(hero);
        let mother = s.world.spawn(ObjectKind::Creature, "mother");
        let o = s.world.get_mut(mother).unwrap();
        o.position = [30.0, 39.9, 5.0];
        o.locals.ints.insert(STATE.into(), 9);
        (s, hero, mother)
    }

    fn kicks(s: &Sim) -> Vec<(u64, Event)> {
        s.world
            .queue
            .iter()
            .filter(|(_, q)| q.event.ty == EVENT_TYPE_AMBIENT_CONTINUE)
            .map(|(k, q)| (k.0, q.event.clone()))
            .collect()
    }

    #[test]
    fn an_enabled_creature_is_kicked_every_three_seconds_with_the_party_near() {
        let (mut s, hero, mother) = sim();
        s.tick(3000);
        assert!(kicks(&s).is_empty()); // Equality is not enough.
        s.tick(1);
        let k = kicks(&s);
        assert_eq!(k.len(), 1);
        assert_eq!(k[0].1.creator, Some(mother));
        assert_eq!(k[0].1.objects, vec![hero]);
        assert_eq!(s.world.get(mother).unwrap().ambient_ms, 0.0);
    }

    #[test]
    fn running_disabled_far_or_party_creatures_are_not_kicked() {
        let (mut s, hero, mother) = sim();
        // Running: the clock still runs out and restarts, but nothing is sent.
        let o = s.world.get_mut(mother).unwrap();
        o.locals.ints.insert(STATE.into(), 9 | RUNNING);
        s.kick_ambient_creatures(3001);
        assert!(kicks(&s).is_empty());
        assert_eq!(s.world.get(mother).unwrap().ambient_ms, 0.0);
        // Disabled: the clock holds.
        let o = s.world.get_mut(mother).unwrap();
        o.locals.ints.insert(STATE.into(), 8);
        o.ambient_ms = 1000.0;
        s.kick_ambient_creatures(5000);
        assert_eq!(s.world.get(mother).unwrap().ambient_ms, 1000.0);
        // Out of reach on the ground plane (height ignored).
        let o = s.world.get_mut(mother).unwrap();
        o.locals.ints.insert(STATE.into(), 9);
        o.position = [50.0, 0.0, 0.0];
        s.kick_ambient_creatures(3001);
        assert!(kicks(&s).is_empty());
        s.world.get_mut(mother).unwrap().position = [0.0, 49.9, 100.0];
        s.kick_ambient_creatures(3001);
        assert_eq!(kicks(&s).len(), 1);
        // A party member is never kicked.
        let h = s.world.get_mut(hero).unwrap();
        h.locals.ints.insert(STATE.into(), 9);
        h.ambient_ms = 2999.0;
        s.kick_ambient_creatures(5);
        assert_eq!(s.world.get(hero).unwrap().ambient_ms, 2999.0);
    }
}
