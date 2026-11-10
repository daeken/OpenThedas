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

/// The inventory screen's equip action: the item and the `INVENTORY_SLOT_*` to wear it in,
/// for the party member the screen shows (the one the player controls).
pub const MSG_EQUIP: &str = "GUI.SelectedChar.EquipItem";
/// The inventory screen's unequip action: the item.
pub const MSG_UNEQUIP: &str = "GUI.SelectedChar.UnequipItem";
/// The party screens' arrows: the next or previous party member becomes the selected one
/// (the one the player controls).
pub const MSG_NEXT_CHARACTER: &str = "GUI.SelectNextCharacterMessage";
pub const MSG_PREVIOUS_CHARACTER: &str = "GUI.SelectPreviousCharacterMessage";

impl Sim {
    /// The party's backpack: what the party members carry and do not wear, the hero's first.
    /// Provisional: the party shares one pack, as the game's inventory screen shows it.
    pub fn party_backpack(&self) -> Vec<ObjectId> {
        let mut out = Vec::new();
        for m in crate::engine::party_of(self) {
            let Some(o) = self.world.get(m) else { continue };
            for &i in &o.inventory {
                let worn =
                    o.equipment.values().any(|&e| e == i) || o.other_set.values().any(|&e| e == i);
                if !worn && !out.contains(&i) && self.world.get(i).is_some() {
                    out.push(i);
                }
            }
        }
        out
    }

    /// The party member wearing `item`, if one does.
    fn wearer(&self, item: ObjectId) -> Option<ObjectId> {
        crate::engine::party_of(self).into_iter().find(|&m| {
            self.world.get(m).is_some_and(|o| {
                o.equipment.values().any(|&e| e == item) || o.other_set.values().any(|&e| e == item)
            })
        })
    }

    /// The player puts `item` (from the party's backpack, or off another party member) on
    /// party member `who` in `slot`, which must be one the item fits, or
    /// `INVENTORY_SLOT_INVALID` for wherever it fits. What `who` wore there goes to the
    /// backpack. Returns whether it was equipped.
    pub fn equip_item(&mut self, who: ObjectId, item: ObjectId, slot: i32) -> bool {
        let party = crate::engine::party_of(self);
        let fits = self.item_slots(item);
        // No slot given (INVENTORY_SLOT_INVALID, as a double click sends): the first the item
        // fits that is free, else the first it fits.
        let slot = if fits.contains(&slot) {
            slot
        } else if slot == crate::sim::constant(&self.table, "INVENTORY_SLOT_INVALID") {
            let worn = self
                .world
                .get(who)
                .map(|o| o.equipment.clone())
                .unwrap_or_default();
            match fits.iter().find(|s| !worn.contains_key(s)).or(fits.first()) {
                Some(&s) => s,
                None => return false,
            }
        } else {
            return false;
        };
        if !party.contains(&who) {
            return false;
        }
        if !self.party_backpack().contains(&item) && self.wearer(item).is_none() {
            return false;
        }
        for m in party {
            if let Some(o) = self.world.get_mut(m) {
                o.equipment.retain(|_, i| *i != item);
                o.other_set.retain(|_, i| *i != item);
                if m != who {
                    o.inventory.retain(|i| *i != item);
                }
            }
        }
        let Some(o) = self.world.get_mut(who) else {
            return false;
        };
        if !o.inventory.contains(&item) {
            o.inventory.push(item);
        }
        o.equipment.insert(slot, item);
        true
    }

    /// The player takes `item` off party member `who`; it stays in the backpack. Returns
    /// whether `who` wore it.
    pub fn unequip_item(&mut self, who: ObjectId, item: ObjectId) -> bool {
        if !crate::engine::party_of(self).contains(&who) {
            return false;
        }
        let Some(o) = self.world.get_mut(who) else {
            return false;
        };
        let n = o.equipment.len() + o.other_set.len();
        o.equipment.retain(|_, i| *i != item);
        o.other_set.retain(|_, i| *i != item);
        n != o.equipment.len() + o.other_set.len()
    }

    /// The inventory screen's messages ([`MSG_EQUIP`], [`MSG_UNEQUIP`]), for the party member
    /// the player controls, and the arrows choosing that member.
    pub(crate) fn inventory_message(&mut self, name: &str, args: &[String]) -> bool {
        if name == MSG_NEXT_CHARACTER || name == MSG_PREVIOUS_CHARACTER {
            let party = crate::engine::party_of(self);
            let Some(at) = self
                .world
                .controlled()
                .and_then(|c| party.iter().position(|&m| m == c))
            else {
                return false;
            };
            let n = party.len();
            let next = if name == MSG_NEXT_CHARACTER {
                (at + 1) % n
            } else {
                (at + n - 1) % n
            };
            self.set_controlled(Some(party[next]));
            return true;
        }
        let int = |i: usize| {
            args.get(i)
                .and_then(|a| a.trim().parse::<f64>().ok())
                .filter(|v| v.fract() == 0.0 && *v >= 0.0 && *v < u32::MAX as f64)
        };
        let (Some(who), Some(item)) = (self.world.controlled(), int(0).map(|v| v as ObjectId))
        else {
            return false;
        };
        match name {
            MSG_EQUIP => match int(1) {
                Some(slot) => self.equip_item(who, item, slot as i32),
                None => false,
            },
            MSG_UNEQUIP => self.unequip_item(who, item),
            _ => false,
        }
    }

    /// `ScaleEquippedItems`: gives a creature's equipped items the material their material
    /// progression (`TS_Material`, table 199: a row per progression, columns `Material1` to
    /// `Material15`) has at the target level, as the treasure scripts pick an item's
    /// material: tier (level - 1) / 3 + 1 with the level between 1 and 45 and the tier
    /// between 1 and 15. Unique items and items without a progression keep theirs; party
    /// members are not scaled (script.ldf). Provisional: the treasure scripts' rule taken for
    /// the engine's.
    pub fn scale_equipped_items(&mut self, creature: ObjectId, level: i32) {
        if crate::engine::party_of(self).contains(&creature) {
            return;
        }
        let tier = ((level.clamp(1, 45) - 1) / 3 + 1).clamp(1, 15);
        let items: Vec<ObjectId> = self
            .world
            .get(creature)
            .map(|o| o.equipment.values().copied().collect())
            .unwrap_or_default();
        let tables = self.world.tables();
        let Some(t) = tables.get(199, "") else {
            return;
        };
        for item in items {
            let Some(o) = self.world.get(item) else {
                continue;
            };
            let progression = o.fields.get("MaterialProgress").copied().unwrap_or(0);
            if progression <= 0 || o.fields.get("Unique").copied().unwrap_or(0) != 0 {
                continue;
            }
            let material = match t.cell(&format!("Material{tier}"), progression as i64) {
                Some(thedas_formats::gda::Cell::Int(m)) if *m > 0 => *m as i32,
                _ => continue,
            };
            if let Some(o) = self.world.get_mut(item) {
                o.fields.insert("Material".into(), material);
            }
        }
    }

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
            f.other_set.retain(|_, i| *i != item);
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

    /// What a creature's equipped items add to a property: for each item stat that
    /// `itemstatdefines.gda` links to the property (its fourth column), the item's value
    /// from `itemstats.gda` (by base item type). Provisional: material and quality do not
    /// scale it, and items' own property lists are not applied yet.
    pub fn equipment_bonus(&self, id: ObjectId, property: i32) -> f32 {
        const COL_PROPERTY: u32 = 0xd650_5aa1;
        let Some(o) = self.world.get(id) else {
            return 0.0;
        };
        if o.equipment.is_empty() || property <= 0 {
            return 0.0;
        }
        let tables = self.world.tables();
        let Some(defs) = tables.get(0, "itemstatdefines") else {
            return 0.0;
        };
        let stats: Vec<i64> = defs
            .order
            .iter()
            .copied()
            .filter(|&r| match defs.cell_by_hash(COL_PROPERTY, r) {
                Some(thedas_formats::gda::Cell::Int(p)) => *p == property as i64,
                _ => false,
            })
            .collect();
        o.equipment
            .values()
            .flat_map(|&item| stats.iter().map(move |&r| (item, r)))
            .map(|(item, r)| self.item_stat(item, r as i32))
            .sum()
    }

    /// `GetItemStat`: an item's stat (a row of `itemstatdefines.gda`) from `itemstats.gda` by
    /// its base item type, times its material's multiplier for that stat. The item's
    /// `Material` is a row of `materialtypes.gda`, whose `Material` column is a tier in
    /// `materialrules.gda` (Iron, Grey Iron, Steel, ... with Damage, Armor,
    /// CritChanceModifier, ArmorPenetration, Spellpower and MissileDeflection multipliers;
    /// script.ldf says GetItemStat includes the material modifier). Provisional: inferred
    /// from the tables; stats without a material column are not scaled.
    pub fn item_stat(&self, item: ObjectId, stat: i32) -> f32 {
        use crate::engine::{cell_float, cell_string, clean};
        let Some(it) = self.world.get(item) else {
            return 0.0;
        };
        let Some(base) = it.fields.get("BaseItem").copied() else {
            return 0.0;
        };
        let tables = self.world.tables();
        let Some(label) = tables
            .get(0, "itemstatdefines")
            .map(|t| cell_string(t.cell("Label", stat as i64)))
            .filter(|l| !l.is_empty())
        else {
            return 0.0;
        };
        let label = clean(&label).to_string();
        let v = tables
            .get(0, "itemstats")
            .and_then(|t| t.cell(&label, base as i64).map(|c| cell_float(Some(c))))
            .unwrap_or(0.0);
        if v == 0.0 {
            return 0.0;
        }
        let tier = it.fields.get("Material").copied().and_then(|m| {
            match tables.get(0, "materialtypes")?.cell("Material", m as i64)? {
                thedas_formats::gda::Cell::Int(t) if *t > 0 => Some(*t),
                _ => None,
            }
        });
        let mult = tier
            .and_then(|t| {
                tables
                    .get(0, "materialrules")?
                    .cell(&label, t)
                    .map(|c| cell_float(Some(c)))
            })
            .filter(|m| *m > 0.0)
            .unwrap_or(1.0);
        v * mult
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
        o.other_set.retain(|_, i| *i != item);
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
            c.other_set.retain(|_, i| !items.contains(i));
        }
        Some(bag)
    }
}

#[cfg(test)]
mod screen_tests {
    use std::sync::Arc;

    use thedas_formats::gda::Cell;
    use thedas_script::EngineTable;
    use thedas_vfs::Vfs;

    use super::*;

    /// A hero and a follower, a sword and a spare sword (base type 2, main hand: slot 0),
    /// and a pelt that fits no slot.
    fn party() -> (Sim, ObjectId, ObjectId, [ObjectId; 3]) {
        let table = EngineTable::parse(b"int INVENTORY_SLOT_INVALID = 255;\n").unwrap();
        let mut s = Sim::new(Arc::new(Vfs::new()), Arc::new(table), 1);
        s.world.tables().insert(
            "bitm_base",
            &["EquippableSlots"],
            vec![(2, vec![Cell::Int(0b11)])],
        );
        let hero = s.world.spawn(ObjectKind::Creature, "hero");
        let ally = s.world.spawn(ObjectKind::Creature, "ally");
        s.world.hero = Some(hero);
        s.world.get_mut(hero).unwrap().followers.push(ally);
        let mut items = [0; 3];
        for (i, tag) in ["sword", "spare", "pelt"].iter().enumerate() {
            items[i] = s.world.spawn(ObjectKind::Item, tag);
            if i < 2 {
                s.world
                    .get_mut(items[i])
                    .unwrap()
                    .fields
                    .insert("BaseItem".into(), 2);
            }
        }
        let h = s.world.get_mut(hero).unwrap();
        h.inventory = items.to_vec();
        h.equipment.insert(0, items[0]);
        (s, hero, ally, items)
    }

    #[test]
    fn the_backpack_is_what_nobody_wears() {
        let (s, _, _, [_, spare, pelt]) = party();
        assert_eq!(s.party_backpack(), vec![spare, pelt]);
    }

    #[test]
    fn equipping_swaps_and_moves_between_members() {
        let (mut s, hero, ally, [sword, spare, pelt]) = party();
        // Not a slot the pelt fits.
        assert!(!s.equip_item(hero, pelt, 0));
        // Into the occupied main hand: the sword goes to the backpack.
        assert!(s.equip_item(hero, spare, 0));
        assert_eq!(s.world.get(hero).unwrap().equipment.get(&0), Some(&spare));
        assert!(s.party_backpack().contains(&sword));
        // Onto the ally, wherever it fits (slot 0 is free there).
        assert!(s.equip_item(ally, sword, 255));
        assert_eq!(s.world.get(ally).unwrap().equipment.get(&0), Some(&sword));
        assert!(!s.world.get(hero).unwrap().inventory.contains(&sword));
        // Off again: back in the backpack, carried by the ally.
        assert!(s.unequip_item(ally, sword));
        assert!(!s.unequip_item(ally, sword));
        assert!(s.party_backpack().contains(&sword));
    }

    #[test]
    fn the_screen_equips_for_the_controlled_member_and_its_arrows_cycle_the_party() {
        let (mut s, hero, ally, [_, spare, _]) = party();
        assert!(s.gui_message(MSG_NEXT_CHARACTER, &[]));
        assert_eq!(s.world.controlled(), Some(ally));
        assert!(s.gui_message(MSG_EQUIP, &[spare.to_string(), "255".into()]));
        assert_eq!(s.world.get(ally).unwrap().equipment.get(&0), Some(&spare));
        assert!(s.gui_message(MSG_PREVIOUS_CHARACTER, &[]));
        assert_eq!(s.world.controlled(), Some(hero));
        assert!(!s.gui_message(MSG_UNEQUIP, &[spare.to_string()]));
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

#[cfg(test)]
mod stat_tests {
    use std::sync::Arc;

    use thedas_formats::gda::Cell;
    use thedas_script::EngineTable;
    use thedas_vfs::Vfs;

    use crate::sim::Sim;
    use crate::world::ObjectKind;

    #[test]
    fn equipped_items_take_their_progressions_material_at_a_level() {
        let mut s = Sim::new(Arc::new(Vfs::new()), Arc::new(EngineTable::default()), 1);
        let t = crate::m2da::Tables::empty();
        t.insert(
            "#199",
            &["ID", "Label", "Material1", "Material2", "Material3"],
            vec![(
                46,
                vec![
                    Cell::Int(46),
                    Cell::String("Armor, All Metal".into()),
                    Cell::Int(1),
                    Cell::Int(2),
                    Cell::Int(8),
                ],
            )],
        );
        s.world.tables = Arc::new(t);
        let foe = s.world.spawn(ObjectKind::Creature, "hurlock");
        let hero = s.world.spawn(ObjectKind::Creature, "hero");
        s.world.hero = Some(hero);
        let give = |s: &mut Sim, who, unique| {
            let i = s.world.spawn(ObjectKind::Item, "boots");
            s.world.get_mut(i).unwrap().fields.extend([
                ("MaterialProgress".to_string(), 46),
                ("Material".to_string(), 1),
                ("Unique".to_string(), unique),
            ]);
            s.world.get_mut(who).unwrap().equipment.insert(unique, i);
            i
        };
        let (plain, unique, heros) = (
            give(&mut s, foe, 0),
            give(&mut s, foe, 1),
            give(&mut s, hero, 0),
        );
        let material = |s: &Sim, i| s.world.get(i).unwrap().fields["Material"];
        // Level 7: tier (7 - 1) / 3 + 1 = 3.
        s.scale_equipped_items(foe, 7);
        assert_eq!(material(&s, plain), 8);
        assert_eq!(material(&s, unique), 1, "unique items keep theirs");
        s.scale_equipped_items(hero, 7);
        assert_eq!(material(&s, heros), 1, "the party is not scaled");
        s.scale_equipped_items(foe, 4);
        assert_eq!(material(&s, plain), 2);
    }

    #[test]
    fn item_stats_scale_by_material_and_add_to_linked_properties() {
        let mut s = Sim::new(Arc::new(Vfs::new()), Arc::new(EngineTable::default()), 1);
        let t = crate::m2da::Tables::empty();
        let (i, f, st) = (Cell::Int, Cell::Float, |x: &str| Cell::String(x.into()));
        // Armor (stat 1) is linked to property 11; Damage (2) to nothing.
        let defs_cols = ["ID", "Label", "NameStrRef", "#d6505aa1"];
        let mut defs = vec![
            (1, vec![i(1), st("Armor"), i(0), i(11)]),
            (2, vec![i(2), st("Damage"), i(0), i(0)]),
        ];
        // The linked property column is known only by its hash.
        let defs_cols: Vec<&str> = defs_cols.to_vec();
        t.insert("itemstatdefines", &defs_cols, std::mem::take(&mut defs));
        t.insert(
            "itemstats",
            &["ID", "Label", "Armor", "Damage"],
            vec![(9, vec![i(9), st("Chest"), f(2.0), f(0.0)])],
        );
        t.insert(
            "materialtypes",
            &["ID", "Label", "Material"],
            vec![(3, vec![i(3), st("Armor, Steel"), i(3)])],
        );
        t.insert(
            "materialrules",
            &["ID", "Label", "Armor"],
            vec![(3, vec![i(3), st("Steel"), f(1.5)])],
        );
        s.world.tables = Arc::new(t);
        let c = s.world.spawn(ObjectKind::Creature, "c");
        let chest = s.world.spawn(ObjectKind::Item, "chest");
        s.world
            .get_mut(chest)
            .unwrap()
            .fields
            .extend([("BaseItem".to_string(), 9), ("Material".to_string(), 3)]);
        assert_eq!(s.item_stat(chest, 1), 3.0, "2 armour, steel x1.5");
        assert_eq!(s.equipment_bonus(c, 11), 0.0, "not worn yet");
        s.world.get_mut(c).unwrap().equipment.insert(4, chest);
        assert_eq!(s.equipment_bonus(c, 11), 3.0);
        assert_eq!(s.equipment_bonus(c, 10), 0.0);
    }
}
