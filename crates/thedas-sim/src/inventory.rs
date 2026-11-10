//! Items changing hands: giving, looting and body bags.
//!
//! Provisional: the owner that receives an item gets `EVENT_TYPE_INVENTORY_ADDED` (creator:
//! the previous owner; object 0: the item), and the previous owner
//! `EVENT_TYPE_INVENTORY_REMOVED`, as `player_core` reads them; items flagged in their
//! template's `VarTable` with `ITEM_SEND_ACQUIRED_EVENT` then tell the module, which is how
//! quest items (darkspawn blood, keys) advance plots.

use thedas_script::vm::{OBJECT_INVALID, ObjectId};

use crate::sim::Sim;
use crate::world::ObjectKind;

impl Sim {
    /// Moves an item into `to`'s inventory (from wherever it is) and tells both owners.
    pub fn give_item(&mut self, item: ObjectId, to: ObjectId) -> bool {
        if self.world.get(item).is_none() || self.world.get(to).is_none() {
            return false;
        }
        let from = self
            .world
            .objects
            .values()
            .find(|o| o.id != to && o.inventory.contains(&item))
            .map(|o| o.id);
        if let Some(f) = from.and_then(|f| self.world.get_mut(f)) {
            f.inventory.retain(|&i| i != item);
            f.equipment.retain(|_, i| *i != item);
        }
        if let Some(t) = self.world.get_mut(to) {
            if !t.inventory.contains(&item) {
                t.inventory.push(item);
            }
        }
        let mut added = self.new_event("INVENTORY_ADDED", from.or(Some(OBJECT_INVALID)));
        added.objects = vec![item];
        added.ints = vec![1];
        self.world.queue_event(0, to, added, None);
        if let Some(f) = from {
            let mut removed = self.new_event("INVENTORY_REMOVED", Some(to));
            removed.objects = vec![item];
            removed.ints = vec![1];
            self.world.queue_event(0, f, removed, None);
        }
        true
    }

    /// Takes everything out of a container, body bag or dead creature into `to`'s inventory.
    /// Equipped items of a creature stay unless they are droppable. Returns the items taken.
    pub fn loot(&mut self, from: ObjectId, to: ObjectId) -> Vec<ObjectId> {
        let Some(src) = self.world.get(from) else {
            return Vec::new();
        };
        let creature = src.kind == ObjectKind::Creature;
        let items: Vec<ObjectId> = src
            .inventory
            .iter()
            .copied()
            .filter(|i| {
                !creature
                    || self
                        .world
                        .get(*i)
                        .is_some_and(|x| x.fields.get("Droppable").copied().unwrap_or(0) != 0)
            })
            .collect();
        for &i in &items {
            self.give_item(i, to);
        }
        items
    }

    /// `SpawnBodyBag`: leaves a container where a creature fell holding its droppable items
    /// (all of them when forced). Nothing is left when there is nothing to take.
    pub fn spawn_body_bag(&mut self, creature: ObjectId, force: bool) -> Option<ObjectId> {
        let c = self.world.get(creature)?;
        let (pos, area) = (c.position, c.area);
        let items: Vec<ObjectId> = c
            .inventory
            .iter()
            .copied()
            .filter(|i| {
                force
                    || self
                        .world
                        .get(*i)
                        .is_some_and(|x| x.fields.get("Droppable").copied().unwrap_or(0) != 0)
            })
            .collect();
        if items.is_empty() {
            return None;
        }
        let bag = self.world.spawn(ObjectKind::Placeable, "gen_ip_bodybag");
        if let Some(b) = self.world.get_mut(bag) {
            b.position = pos;
            b.area = area;
            b.template = "gen_ip_bodybag".into();
            b.inventory = items.clone();
        }
        if let Some(c) = self.world.get_mut(creature) {
            c.inventory.retain(|i| !items.contains(i));
            c.equipment.retain(|_, i| !items.contains(i));
        }
        Some(bag)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use thedas_script::EngineTable;
    use thedas_vfs::Vfs;

    use super::*;

    const LDF: &str = "int EVENT_TYPE_INVENTORY_ADDED = 6;\n\
        int EVENT_TYPE_INVENTORY_REMOVED = 7;\n";

    #[test]
    fn looting_moves_droppable_items_and_tells_both_sides() {
        let table = Arc::new(EngineTable::parse(LDF.as_bytes()).unwrap());
        let mut s = Sim::new(Arc::new(Vfs::new()), table, 1);
        let hero = s.world.spawn(ObjectKind::Creature, "hero");
        let foe = s.world.spawn(ObjectKind::Creature, "foe");
        let blood = s.world.spawn(ObjectKind::Item, "blood");
        let sword = s.world.spawn(ObjectKind::Item, "sword");
        s.world
            .get_mut(blood)
            .unwrap()
            .fields
            .insert("Droppable".into(), 1);
        s.world.get_mut(foe).unwrap().inventory = vec![blood, sword];
        let bag = s.spawn_body_bag(foe, false).unwrap();
        assert_eq!(s.world.get(bag).unwrap().inventory, vec![blood]);
        assert_eq!(s.world.get(foe).unwrap().inventory, vec![sword]);
        assert_eq!(s.loot(bag, hero), vec![blood]);
        assert_eq!(s.world.get(hero).unwrap().inventory, vec![blood]);
        let evs: Vec<_> = s
            .world
            .queue
            .values()
            .map(|q| {
                (
                    q.target,
                    q.event.ty,
                    q.event.creator,
                    q.event.objects.clone(),
                )
            })
            .collect();
        assert_eq!(
            evs,
            vec![
                (hero, 6, Some(bag), vec![blood]),
                (bag, 7, Some(hero), vec![blood])
            ]
        );
    }
}
