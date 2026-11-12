//! Corpses decaying, as DAOrigins.exe does it (the per-update step 0x6bc670, the hand-off
//! 0x6b98e0, `SetBodybagDecayDelay` 0x69f360).
//!
//! A dead creature that may decay counts the milliseconds it lies dead. With the global
//! maximum `max` (60 s, `SetCreaturesGlobalMaxTimeBeforeDecay`), it fades over the last two
//! seconds; 1.5 s before the end its body bag takes its place (the bag turns from hidden to
//! the appearance's `CorpseModel`, a bone pile by default) and the creature is destroyed 1.5 s
//! later. A corpse with no bag is destroyed when its time is up. Nothing tests whether the
//! player can see it.
//!
//! A corpse may decay when forced (`SetBodybagDecayDelay`) or when its appearance's
//! `DecayBehaviour` (apr_base.gda) is 0 or 3; 2 (the Broodmother, the High Dragon) keeps it.
//! `SpawnBodyBag` on a creature with `DecayBehaviour` 3 (shades, rage demons, ash wraiths)
//! starts its fade a second later, and a body bag emptied by the player starts it at once.
//! When more corpses lie waiting than the graphics detail allows (5, 10, 15 or 20), the
//! ones over start fading.
//!
//! Corpses that never decay unless forced: party members, non-combatants, creatures that
//! cannot die permanently (`NoPermDeath`) and creatures placed dead.
//!
//! Provisional: the clock runs from death (the exe waits for the death animation to settle);
//! the corpse cap uses the highest detail level (20); the bag's model swap is left
//! to the presentation; bags that die when empty are not removed.

use thedas_script::vm::ObjectId;

use crate::sim::Sim;
use crate::world::ObjectKind;

/// The global time before a corpse is gone (DAOrigins.exe 0xc021c4), milliseconds.
pub const DEFAULT_MAX_MS: u64 = 60_000;
/// The fade at the end of it (0xc021c8).
pub const FADE_MS: u64 = 2_000;
/// How long before the end the bag takes the corpse's place and the creature's destruction
/// is queued (0xc021cc).
pub const HAND_OFF_MS: u64 = 1_500;
/// Corpses allowed to lie waiting at the highest graphics detail (0x7c4bd0: 5, 10, 15, 20).
pub const CORPSE_CAP: usize = 20;

/// The field a body bag gets when it takes its corpse's place: the creature's appearance
/// (`apr_base.gda` row), whose `CorpseModel` the bag shows from then on. 0 or missing: the
/// bag is still hidden.
pub const CORPSE_APPEARANCE: &str = "CorpseAppearance";

pub fn default_max_ms() -> u64 {
    DEFAULT_MAX_MS
}

impl Sim {
    /// The corpses fading out now and their opacity, for the presentation: the exe's alpha
    /// over the last [`FADE_MS`] of the clock (0x6bc670: remaining / 2000, through 0x695060).
    pub fn corpse_fades(&self) -> Vec<(ObjectId, f32)> {
        let max = self.world.corpse_decay_max_ms;
        self.world
            .objects
            .values()
            .filter(|o| o.kind == ObjectKind::Creature && o.dead && o.decay_ms > 0)
            .filter_map(|o| {
                let left = max.saturating_sub(o.decay_ms);
                (left < FADE_MS).then(|| (o.id, left as f32 / FADE_MS as f32))
            })
            .collect()
    }

    /// Whether a dead creature may decay (DAOrigins.exe 0x6a3ad0).
    fn may_decay(&self, id: ObjectId, party: &[ObjectId]) -> bool {
        let Some(o) = self.world.get(id) else {
            return false;
        };
        if o.decay_forced {
            return true;
        }
        // Party members, non-combatants (template `Combatant` 1), creatures that cannot die
        // permanently (`NoPermDeath`) and creatures placed dead (`CREATURE_SPAWN_DEAD`) lie
        // where they fell (0x6a3ad0: +0xbd4, +0xbd8, the party list 0x5d57c0, the local
        // int with reserved id 4). Pool creatures (`CreatePool`) are not modelled.
        if party.contains(&id)
            || Some(id) == self.world.hero
            || o.fields.get("Combatant").copied().unwrap_or(0) != 0
            || o.fields.get("NoPermDeath").copied().unwrap_or(0) != 0
            || o.locals
                .ints
                .get("CREATURE_SPAWN_DEAD")
                .copied()
                .unwrap_or(0)
                > 0
        {
            return false;
        }
        matches!(self.decay_behaviour(id), 0 | 3)
    }

    /// The creature's appearance's `DecayBehaviour` (apr_base.gda; 0 when not given).
    pub(crate) fn decay_behaviour(&self, id: ObjectId) -> i32 {
        let Some(app) = self.world.get(id).map(|o| o.appearance).filter(|&a| a > 0) else {
            return 0;
        };
        self.world
            .tables()
            .get(0, "apr_base")
            .and_then(|t| match t.cell("DecayBehaviour", app as i64)? {
                thedas_formats::gda::Cell::Int(v) => Some(*v as i32),
                thedas_formats::gda::Cell::Float(v) => Some(*v as i32),
                _ => None,
            })
            .unwrap_or(0)
    }

    /// Starts `id`'s fade `ms` from now unless it is fading already, and lets it decay
    /// whatever its appearance says (DAOrigins.exe 0x69f360).
    pub(crate) fn force_decay(&mut self, id: ObjectId, ms: u64) {
        let max = self.world.corpse_decay_max_ms;
        if let Some(o) = self.world.get_mut(id) {
            o.decay_forced = true;
            if max.saturating_sub(o.decay_ms) >= FADE_MS {
                o.decay_ms = max.saturating_sub(FADE_MS + ms + 1);
            }
        }
    }

    /// `SetBodybagDecayDelay`: the creature that left `bag` starts its fade `ms` from now.
    pub(crate) fn set_bodybag_decay_delay(&mut self, bag: ObjectId, ms: u64) {
        let owner = self.body_bag_owner(bag);
        if let Some(c) = owner {
            self.force_decay(c, ms);
        }
    }

    /// The creature whose body bag `bag` is.
    fn body_bag_owner(&self, bag: ObjectId) -> Option<ObjectId> {
        self.world
            .objects
            .values()
            .find(|o| {
                o.kind == ObjectKind::Creature
                    && o.fields.get(crate::inventory::BODY_BAG_FIELD).copied() == Some(bag as i32)
            })
            .map(|o| o.id)
    }

    /// The body bag takes the corpse's place (DAOrigins.exe 0x7526f0): it stands where the
    /// creature lies, facing as it did, and turns from hidden to the appearance's
    /// `CorpseModel` (apr_base.gda). [`CORPSE_APPEARANCE`] on the bag keeps the appearance
    /// for the presentation, as the creature is gone 1.5 s later.
    fn hand_off_to_bag(&mut self, id: ObjectId) {
        let Some((bag, pos, facing, app)) = self.world.get(id).and_then(|o| {
            let bag = *o.fields.get(crate::inventory::BODY_BAG_FIELD)? as ObjectId;
            Some((bag, o.position, o.facing, o.appearance))
        }) else {
            return;
        };
        if let Some(b) = self.world.get_mut(bag) {
            b.position = pos;
            b.facing = facing;
            b.fields.insert(CORPSE_APPEARANCE.into(), app.max(0));
        }
    }

    /// Runs the corpses' decay clocks `ms` on (DAOrigins.exe 0x6bc670, per creature update).
    pub(crate) fn decay_corpses(&mut self, ms: u64) {
        let max = self.world.corpse_decay_max_ms;
        let party = crate::engine::party_of(self);
        let mut corpses: Vec<(u64, ObjectId)> = Vec::new();
        let mut revived = Vec::new();
        for o in self.world.objects.values() {
            if o.kind != ObjectKind::Creature {
                continue;
            }
            if !o.dead {
                if o.decay_ms != 0 || o.decay_forced {
                    revived.push(o.id);
                }
                continue;
            }
            // Objects in an area the party left do not update.
            if o.parked.is_some() || self.world.destroy_at.iter().any(|&(_, d)| d == o.id) {
                continue;
            }
            corpses.push((o.died_at.unwrap_or(0), o.id));
        }
        for id in revived {
            if let Some(o) = self.world.get_mut(id) {
                o.decay_ms = 0;
                o.decay_forced = false;
            }
        }
        corpses.sort();
        let corpses: Vec<ObjectId> = corpses
            .into_iter()
            .map(|(_, id)| id)
            .filter(|&id| self.may_decay(id, &party))
            .collect();
        // A body bag the player has emptied: its corpse starts fading at once (0x74e710).
        for &id in &corpses {
            let bag = self
                .world
                .get(id)
                .and_then(|o| o.fields.get(crate::inventory::BODY_BAG_FIELD).copied())
                .and_then(|b| self.world.get(b as ObjectId));
            if bag.is_some_and(|b| b.inventory.is_empty() && b.money <= 0) {
                self.force_decay(id, 0);
            }
        }
        // Too many corpses lying waiting: the oldest over the cap start fading.
        let waiting: Vec<ObjectId> = corpses
            .iter()
            .copied()
            .filter(|&id| {
                self.world
                    .get(id)
                    .is_some_and(|o| max.saturating_sub(o.decay_ms) >= FADE_MS)
            })
            .collect();
        if waiting.len() > CORPSE_CAP {
            for &id in &waiting[..waiting.len() - CORPSE_CAP] {
                self.force_decay(id, 0);
            }
        }
        for id in corpses {
            let has_bag = self
                .world
                .get(id)
                .and_then(|o| o.fields.get(crate::inventory::BODY_BAG_FIELD).copied())
                .is_some_and(|b| self.world.objects.contains_key(&(b as ObjectId)));
            let Some(o) = self.world.get_mut(id) else {
                continue;
            };
            o.decay_ms = o.decay_ms.saturating_add(ms);
            let after = o.decay_ms;
            let hand_off = max.saturating_sub(HAND_OFF_MS);
            if after >= max {
                // Time is up: gone now (a bag would have taken over at the hand-off).
                self.world.queue_destroy(0, id);
            } else if has_bag && after >= hand_off {
                // The bag takes the corpse's place; the creature goes 1.5 s on.
                self.world.queue_destroy(HAND_OFF_MS, id);
                self.hand_off_to_bag(id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sim() -> Sim {
        let table = std::sync::Arc::new(thedas_script::EngineTable::parse(b"").unwrap());
        Sim::new(std::sync::Arc::new(thedas_vfs::Vfs::new()), table, 3)
    }

    fn corpse(s: &mut Sim) -> ObjectId {
        let area = s.world.spawn(ObjectKind::Area, "a");
        let c = s.world.spawn(ObjectKind::Creature, "c");
        let o = s.world.get_mut(c).unwrap();
        o.area = Some(area);
        o.dead = true;
        o.died_at = Some(0);
        c
    }

    #[test]
    fn non_combatants_and_creatures_placed_dead_stay() {
        let mut s = sim();
        let a = corpse(&mut s);
        s.world
            .get_mut(a)
            .unwrap()
            .fields
            .insert("Combatant".into(), 1);
        let b = corpse(&mut s);
        s.world
            .get_mut(b)
            .unwrap()
            .locals
            .ints
            .insert("CREATURE_SPAWN_DEAD".into(), 1);
        let c = corpse(&mut s);
        s.world
            .get_mut(c)
            .unwrap()
            .fields
            .insert("NoPermDeath".into(), 1);
        s.tick(120_000);
        for id in [a, b, c] {
            assert!(s.world.get(id).is_some());
        }
    }

    #[test]
    fn a_corpse_without_a_bag_is_gone_after_the_global_time() {
        let mut s = sim();
        let c = corpse(&mut s);
        s.tick(59_000);
        assert!(s.world.get(c).is_some());
        s.tick(1_100);
        assert!(s.world.get(c).is_none());
    }

    #[test]
    fn a_bag_takes_over_and_the_corpse_goes_at_the_end() {
        let mut s = sim();
        let c = corpse(&mut s);
        s.world.get_mut(c).unwrap().money = 5;
        let bag = s.spawn_body_bag(c, false).unwrap();
        // Taking the money empties the bag: the corpse fades at once (2 s).
        s.world.get_mut(bag).unwrap().money = 0;
        s.tick(100);
        assert!(s.world.get(c).is_some());
        s.tick(2_100);
        assert!(s.world.get(c).is_none());
        assert!(s.world.get(bag).is_some());
    }

    #[test]
    fn set_bodybag_decay_delay_starts_the_fade_after_that_long() {
        let mut s = sim();
        let c = corpse(&mut s);
        s.world.get_mut(c).unwrap().money = 5;
        let bag = s.spawn_body_bag(c, false).unwrap();
        s.world.get_mut(c).unwrap().appearance = 7;
        s.world.get_mut(c).unwrap().position = [3.0, 4.0, 0.0];
        s.set_bodybag_decay_delay(bag, 3_000);
        // Fade in 3 s, then the 0.5 s to the hand-off and 1.5 s to the destruction.
        s.tick(3_400);
        assert!(s.world.get(c).is_some());
        assert_eq!(
            s.world.get(bag).unwrap().fields.get(CORPSE_APPEARANCE),
            None
        );
        s.tick(200);
        let b = s.world.get(bag).unwrap();
        assert_eq!(b.fields.get(CORPSE_APPEARANCE), Some(&7));
        assert_eq!(b.position, [3.0, 4.0, 0.0]);
        s.tick(1_700);
        assert!(s.world.get(c).is_none());
    }

    #[test]
    fn a_fading_corpse_reports_its_opacity() {
        let mut s = sim();
        let c = corpse(&mut s);
        s.tick(58_500);
        let f = s.corpse_fades();
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].0, c);
        assert!((f[0].1 - 0.75).abs() < 1e-3, "{f:?}");
    }

    #[test]
    fn party_members_and_the_living_do_not_decay() {
        let mut s = sim();
        let c = corpse(&mut s);
        s.world.hero = Some(c);
        s.tick(120_000);
        assert!(s.world.get(c).is_some());
        assert_eq!(s.world.get(c).unwrap().decay_ms, 0);
    }
}
