//! DA2 encounter membership (DragonAge2.exe commands 425–427).
//! The area's registry retains inactive/dead creatures. Registration appends; setting
//! even the same encounter id unregisters and registers the creature again.

use thedas_script::vm::ObjectId;

use crate::{sim::Sim, world::ObjectKind};

const ENCOUNTER: &str = "EncounterID";
const LEGACY_ENCOUNTER: &str = "EncounterId";

impl Sim {
    /// Command 426 (0x4e7300): a creature's stored id, zero for an invalid creature.
    pub(crate) fn da2_encounter_id(&self, id: ObjectId) -> i32 {
        self.world
            .get(id)
            .filter(|o| o.kind == ObjectKind::Creature)
            .and_then(|o| {
                o.fields
                    .get(LEGACY_ENCOUNTER)
                    .or_else(|| o.fields.get(ENCOUNTER))
            })
            .copied()
            .unwrap_or(0)
    }

    /// Command 425 (0x4e7260): without an area the setter does nothing. Otherwise it
    /// removes old membership (0x55a060), appends new membership (0x55e050), then stores.
    pub(crate) fn set_da2_encounter_id(&mut self, id: ObjectId, encounter: i32) {
        if !self
            .world
            .get(id)
            .is_some_and(|o| o.kind == ObjectKind::Creature && o.area.is_some())
        {
            return;
        }
        let o = self.world.get_mut(id).unwrap();
        o.fields.remove(LEGACY_ENCOUNTER);
        o.fields.insert(ENCOUNTER.into(), encounter);
        self.register_da2_encounter(id);
    }

    pub(crate) fn register_da2_encounter(&mut self, id: ObjectId) {
        if self.game != thedas_vfs::Game::DragonAge2 {
            return;
        }
        self.world.da2_encounter_order.retain(|&c| c != id);
        if self.da2_encounter_id(id) > 0 && self.world.get(id).is_some_and(|o| o.area.is_some()) {
            self.world.da2_encounter_order.push(id);
        }
    }

    pub(crate) fn da2_encounter_area_changed(&mut self, id: ObjectId, before: Option<ObjectId>) {
        if self.game == thedas_vfs::Game::DragonAge2
            && self.world.get(id).and_then(|o| o.area) != before
        {
            self.register_da2_encounter(id);
        }
    }

    /// Old snapshots did not retain registration order. Their object ids supply creation
    /// order (provisional recovery); subsequent registrations and saves retain exact order.
    /// The legacy spelling was used by the old setter and overrides the template's value.
    pub(crate) fn restore_da2_encounters(&mut self) {
        if self.game != thedas_vfs::Game::DragonAge2 {
            return;
        }
        for o in self
            .world
            .objects
            .values_mut()
            .filter(|o| o.kind == ObjectKind::Creature)
        {
            if let Some(id) = o.fields.remove(LEGACY_ENCOUNTER) {
                o.fields.insert(ENCOUNTER.into(), id);
            }
        }
        let ids: Vec<_> = self
            .world
            .objects
            .keys()
            .copied()
            .filter(|&id| {
                self.da2_encounter_id(id) > 0
                    && self.world.get(id).is_some_and(|o| o.area.is_some())
            })
            .collect();
        let mut seen = std::collections::BTreeSet::new();
        self.world
            .da2_encounter_order
            .retain(|id| ids.contains(id) && seen.insert(*id));
        for id in ids {
            if seen.insert(id) {
                self.world.da2_encounter_order.push(id);
            }
        }
    }

    /// Command 427 (0x518f00 -> 0x5559f0): positive-id members in the current area,
    /// in registration order, without an activity, death or hostility filter.
    pub(crate) fn da2_encounter(&self, encounter: i32) -> Vec<ObjectId> {
        let Some(area) = self.world.area.filter(|_| encounter > 0) else {
            return Vec::new();
        };
        self.world
            .da2_encounter_order
            .iter()
            .copied()
            .filter(|&id| {
                self.da2_encounter_id(id) == encounter
                    && self.world.get(id).is_some_and(|o| o.area == Some(area))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use thedas_script::EngineTable;
    use thedas_vfs::{Game, Vfs};

    #[test]
    fn encounter_membership_retains_registration_order_and_survives_reload() {
        let mut s = Sim::new(
            Arc::new(Vfs::new()),
            Arc::new(EngineTable::parse(b"").unwrap()),
            7,
        );
        s.game = Game::DragonAge2;
        let a = s.world.spawn(ObjectKind::Area, "a");
        let b = s.world.spawn(ObjectKind::Area, "b");
        s.world.area = Some(a);
        let first = s.world.spawn(ObjectKind::Creature, "first");
        let second = s.world.spawn(ObjectKind::Creature, "second");
        for id in [first, second] {
            let o = s.world.get_mut(id).unwrap();
            o.area = Some(a);
            o.fields.insert(ENCOUNTER.into(), 42);
        }
        s.world.get_mut(first).unwrap().active = false;
        s.world.get_mut(second).unwrap().dead = true;
        s.restore_da2_encounters();
        assert_eq!(s.da2_encounter(42), vec![first, second]);
        s.set_da2_encounter_id(first, 42);
        assert_eq!(s.da2_encounter(42), vec![second, first]);
        let saved = s.save_json().unwrap();
        s.load_json(&saved).unwrap();
        assert_eq!(s.da2_encounter(42), vec![second, first]);
        s.move_object(second, [0.; 3], Some(b));
        assert_eq!(s.da2_encounter(42), vec![first]);
        s.world.area = Some(b);
        assert_eq!(s.da2_encounter(42), vec![second]);
        s.move_object(first, [0.; 3], Some(b));
        assert_eq!(s.da2_encounter(42), vec![second, first]);
        s.set_da2_encounter_id(second, -1);
        assert_eq!(s.da2_encounter_id(second), -1);
        assert_eq!(s.da2_encounter(42), vec![first]);
        assert!(s.da2_encounter(0).is_empty());
        assert!(s.da2_encounter(-1).is_empty());
        s.destroy_object(first);
        assert!(s.da2_encounter(42).is_empty());
    }

    #[test]
    fn legacy_setter_values_win_and_noncreatures_or_unplaced_setters_are_ignored() {
        let mut s = Sim::new(
            Arc::new(Vfs::new()),
            Arc::new(EngineTable::parse(b"").unwrap()),
            7,
        );
        s.game = Game::DragonAge2;
        let area = s.world.spawn(ObjectKind::Area, "area");
        s.world.area = Some(area);
        let npc = s.world.spawn(ObjectKind::Creature, "npc");
        s.set_da2_encounter_id(npc, 99);
        assert_eq!(s.da2_encounter_id(npc), 0, "native setter needs an area");
        let o = s.world.get_mut(npc).unwrap();
        o.area = Some(area);
        o.fields
            .extend([(ENCOUNTER.into(), 42), (LEGACY_ENCOUNTER.into(), 43)]);
        s.restore_da2_encounters();
        assert_eq!(s.da2_encounter(43), vec![npc]);
        assert!(
            !s.world
                .get(npc)
                .unwrap()
                .fields
                .contains_key(LEGACY_ENCOUNTER)
        );
        let prop = s.world.spawn(ObjectKind::Placeable, "prop");
        s.world.get_mut(prop).unwrap().area = Some(area);
        s.set_da2_encounter_id(prop, 43);
        assert_eq!(s.da2_encounter_id(prop), 0);
        assert_eq!(s.da2_encounter(43), vec![npc]);
        assert_eq!(s.da2_encounter_id(u32::MAX), 0);
        s.world.area = None;
        assert!(s.da2_encounter(43).is_empty());
        s.game = Game::Origins;
        s.world.da2_encounter_order.clear();
        s.restore_da2_encounters();
        let json = serde_json::to_value(&s.world).unwrap();
        assert!(json.get("da2_encounter_order").is_none());
    }
}
