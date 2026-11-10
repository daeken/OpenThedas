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

    /// `StoreFollowerInventory`: moves a creature's items (equipped ones too, remembering
    /// their slots) into a container. `equipped` false leaves equipped items where they are
    /// (the party inventory). Provisional.
    pub fn store_inventory(&mut self, owner: ObjectId, container: ObjectId, equipped: bool) {
        if self.world.get(container).is_none() {
            return;
        }
        let Some(o) = self.world.get_mut(owner) else {
            return;
        };
        let slot_of = |o: &crate::world::Object, i: ObjectId| {
            o.equipment
                .iter()
                .find(|(_, x)| **x == i)
                .map_or(-1, |(s, _)| *s)
        };
        let items: Vec<(ObjectId, i32)> = o
            .inventory
            .iter()
            .map(|&i| (i, slot_of(o, i)))
            .filter(|(_, s)| equipped || *s < 0)
            .collect();
        o.inventory.retain(|i| !items.iter().any(|(x, _)| x == i));
        o.equipment
            .retain(|_, i| !items.iter().any(|(x, _)| x == i));
        if let Some(c) = self.world.get_mut(container) {
            c.inventory.extend(items.iter().map(|(i, _)| *i));
            c.stored.entry(owner).or_default().extend(items);
            c.active = true;
        }
    }

    /// `RestoreFollowerInventory`: gives back what `store_inventory` put in the container
    /// (whatever is still there), re-equipping it.
    pub fn restore_inventory(&mut self, owner: ObjectId, container: ObjectId) {
        if self.world.get(owner).is_none() {
            return;
        }
        let Some(c) = self.world.get_mut(container) else {
            return;
        };
        let items = c.stored.remove(&owner).unwrap_or_default();
        let items: Vec<(ObjectId, i32)> = items
            .into_iter()
            .filter(|(i, _)| c.inventory.contains(i))
            .collect();
        c.inventory.retain(|i| !items.iter().any(|(x, _)| x == i));
        if let Some(o) = self.world.get_mut(owner) {
            for (i, slot) in items {
                if !o.inventory.contains(&i) {
                    o.inventory.push(i);
                }
                if slot >= 0 {
                    o.equipment.insert(slot, i);
                }
            }
        }
    }

    /// The equipment slots an item fits (`EquippableSlots` bitmask of its base type in
    /// `bitm_base.gda`), lowest first.
    pub fn item_slots(&self, item: ObjectId) -> Vec<i32> {
        let Some(base) = self
            .world
            .get(item)
            .and_then(|o| o.fields.get("BaseItem").copied())
        else {
            return Vec::new();
        };
        let mask = self
            .world
            .tables
            .get(0, "bitm_base")
            .and_then(|t| match t.cell("EquippableSlots", base as i64) {
                Some(thedas_formats::gda::Cell::Int(v)) => Some(*v),
                _ => None,
            })
            .unwrap_or(0);
        (0..32).filter(|b| mask & (1 << b) != 0).collect()
    }

    /// Equips an item in its first free slot (or its first slot); returns the slot.
    pub fn equip_default(&mut self, who: ObjectId, item: ObjectId) -> Option<i32> {
        let slots = self.item_slots(item);
        let taken = self.world.get(who)?.equipment.clone();
        let slot = slots
            .iter()
            .copied()
            .find(|s| !taken.contains_key(s))
            .or_else(|| slots.first().copied())?;
        let o = self.world.get_mut(who)?;
        if !o.inventory.contains(&item) {
            o.inventory.push(item);
        }
        o.equipment.retain(|_, i| *i != item);
        o.equipment.insert(slot, item);
        Some(slot)
    }

    /// Equips whatever unequipped items fit free slots, as a player would after picking up
    /// their gear. Returns (item, slot) pairs.
    pub fn equip_free_slots(&mut self, who: ObjectId) -> Vec<(ObjectId, i32)> {
        let Some(o) = self.world.get(who) else {
            return Vec::new();
        };
        let equipped: Vec<ObjectId> = o.equipment.values().copied().collect();
        let loose: Vec<ObjectId> = o
            .inventory
            .iter()
            .copied()
            .filter(|i| !equipped.contains(i))
            .collect();
        let mut out = Vec::new();
        for item in loose {
            let free = {
                let taken = &self
                    .world
                    .get(who)
                    .map(|o| o.equipment.clone())
                    .unwrap_or_default();
                self.item_slots(item)
                    .into_iter()
                    .find(|s| !taken.contains_key(s))
            };
            if let (Some(slot), Some(o)) = (free, self.world.get_mut(who)) {
                o.equipment.insert(slot, item);
                out.push((item, slot));
            }
        }
        out
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
mod store_tests {
    use std::sync::Arc;

    use thedas_script::EngineTable;
    use thedas_vfs::Vfs;

    use super::*;

    #[test]
    fn stored_inventories_come_back_equipped() {
        let mut s = Sim::new(
            Arc::new(Vfs::new()),
            Arc::new(EngineTable::parse(b"").unwrap()),
            1,
        );
        let who = s.world.spawn(ObjectKind::Creature, "alistair");
        let chest = s.world.spawn(ObjectKind::Placeable, "footlocker");
        let sword = s.world.spawn(ObjectKind::Item, "sword");
        let pelt = s.world.spawn(ObjectKind::Item, "pelt");
        let o = s.world.get_mut(who).unwrap();
        o.inventory = vec![sword, pelt];
        o.equipment.insert(0, sword);
        s.store_inventory(who, chest, true);
        assert!(s.world.get(who).unwrap().inventory.is_empty());
        assert!(s.world.get(who).unwrap().equipment.is_empty());
        assert_eq!(s.world.get(chest).unwrap().inventory, vec![sword, pelt]);
        s.restore_inventory(who, chest);
        let o = s.world.get(who).unwrap();
        assert_eq!(o.inventory, vec![sword, pelt]);
        assert_eq!(o.equipment.get(&0), Some(&sword));
        assert!(s.world.get(chest).unwrap().inventory.is_empty());
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
