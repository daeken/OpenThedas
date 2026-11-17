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
/// The inventory movie's confirmed discard action, carrying the item's object id.
pub const MSG_DROP: &str = "GUI.SelectedChar.DropItem";
/// The party screens' arrows: the next or previous party member becomes the selected one
/// (the one the player controls).
pub const MSG_NEXT_CHARACTER: &str = "GUI.SelectNextCharacterMessage";
/// The quickbar's weapon set button (quickbar.gfx's `WeaponSetToggleButton_mc`, which sends
/// `1 - ActiveWeaponSet`): the set the selected party member switches to; -1 (the key) the
/// other one. As DAOrigins.exe's handler (0x6d5db0) does: it reads the message's integer and
/// calls the creature switch (0x6a8fa0) on the controlled creature (0x5d5ca0) at once, with
/// no command queued, when a game is running.
pub const MSG_SWITCH_WEAPON_SET: &str = "GUI.SelectedChar.SwitchWeaponSet";
pub const MSG_PREVIOUS_CHARACTER: &str = "GUI.SelectPreviousCharacterMessage";

/// `itemprps.gda` columns whose names are not known: the property's kind (1 while worn, 2
/// on hit, 3 a restriction, 4 other) and the effect parameter its power scales (`Float0`).
const ITEMPRPS_KIND: u32 = 0x4fe5_e973;
/// `itemprps.gda`'s column naming a restriction property's script (DAOrigins.exe reads it
/// in 0x6ae8b0; name unknown).
const ITEMPRPS_SCRIPT: u32 = 0x7578_4cde;
const ITEMPRPS_POWER_PARAM: u32 = 0x3600_1b03;
/// An item property kind that holds while the item is worn.
const ITEMPRPS_WORN: i64 = 1;
/// `Int0` of a property that modifies every attribute (`Increase All Attributes`).
const ALL_ATTRIBUTES: i32 = -1000;

/// The creature field keeping the body bag it left (`GetCreatureBodyBag`; the creature's
/// +0x790 in DAOrigins.exe).
pub const BODY_BAG_FIELD: &str = "BodyBag";

/// The template body bags are made from (see [`Sim::spawn_body_bag`]).
pub const BODY_BAG_TEMPLATE: &str = "gen_ip_bodybag";

/// Origins `placeable_types.gda`: Sack, Small (Container), `PLC_SackSmall_01_0`.
/// Provisional presentation for recoverable player drops; the native discard destroys
/// the item instead (see `docs/notes/drop-equipped-item.md`).
const DROPPED_ITEM_APPEARANCE: i32 = 126;

/// `BITM_base.gda`'s `Money - Copper`.
const BASE_ITEM_COPPER: i32 = 17;
/// `Crafting Recipe` in `bitm_base`.
const BASE_ITEM_RECIPE: i32 = 108;
/// `BITM_base.gda`'s `StackSize` column (its hash).
const BITM_STACK_SIZE: u32 = 0x3997_4122;
/// The item fields two stacks must agree on (0x755a70; MaterialType, AbilityID, Damaged,
/// Indestructible, Irremovable and the store flag).
const STACK_FIELDS: [&str; 6] = [
    "Material",
    "AbilityID",
    "Damaged",
    "Indestructible",
    "Irremovable",
    "StoreFlag",
];
impl Sim {
    /// Origins' item flag, independent of the plot-item flag.
    pub fn item_irremovable(&self, item: ObjectId) -> bool {
        self.world.get(item).is_some_and(|o| {
            o.kind == ObjectKind::Item && o.fields.get("Irremovable").copied().unwrap_or(0) != 0
        })
    }

    /// Equipment access exposed to the Origins inventory screen.
    pub fn can_change_equipment(&self, who: ObjectId) -> bool {
        self.game != thedas_vfs::Game::Origins
            || self.world.get(who).is_some_and(|o| {
                o.kind == ObjectKind::Creature
                    && o.fields.get("CanChangeEquipment").copied().unwrap_or(1) != 0
            })
    }

    pub(crate) fn equipment_slot_locked(
        &self,
        who: ObjectId,
        item: ObjectId,
        slot: i32,
        inactive: bool,
    ) -> bool {
        self.game == thedas_vfs::Game::Origins
            && self
                .world
                .get(who)
                .and_then(|o| {
                    if inactive {
                        o.other_set.get(&slot)
                    } else {
                        o.equipment.get(&slot)
                    }
                })
                .is_some_and(|&old| old != item && self.item_irremovable(old))
    }

    /// The effects an item's properties put on whoever wears it. DA2 uses its native
    /// item_properties schema and parameter builder in da2_items. For Origins, each template
    /// property of the worn kind gives `itemprps.gda`'s effect with its `Int0`/`Int1`,
    /// `Float0`/`Float1`, the parameter the row names scaled by the property's power, the
    /// item as creator. Provisional: the kinds, the power rule and the all-attributes row
    /// are read from the table's contents, not traced in the executable; on-hit
    /// properties and restrictions are not applied here.
    pub fn property_effects(&self, item: ObjectId) -> Vec<crate::world::Effect> {
        self.effects_of_properties(item, &self.item_properties(item))
    }

    /// An item's properties (`itemprps.gda` row and power), in order: its own list once a
    /// script has changed it, else its template's.
    pub fn item_properties(&self, item: ObjectId) -> Vec<(i32, i32)> {
        match self.world.get(item).and_then(|o| o.item_properties.clone()) {
            Some(p) => p,
            None => self.item_look(item).properties,
        }
    }

    /// The creature wearing `item` in its active equipment, if one does.
    fn worn_by(&self, item: ObjectId) -> Option<ObjectId> {
        let mut who: Vec<ObjectId> = self
            .world
            .objects
            .values()
            .filter(|o| o.equipment.values().any(|&e| e == item))
            .map(|o| o.id)
            .collect();
        who.sort_unstable();
        who.first().copied()
    }

    /// `AddItemProperty`: DAOrigins.exe (0x665290, 0x756800) adds the property with its
    /// power when both are above 0 and the item does not have it yet; worn, the item's new
    /// property takes effect on its wearer (0x6ac9a0, 0x759190). Returns whether it was
    /// added.
    pub fn add_item_property(&mut self, item: ObjectId, property: i32, power: i32) -> bool {
        if property <= 0
            || power <= 0
            || !self
                .world
                .get(item)
                .is_some_and(|o| o.kind == ObjectKind::Item)
        {
            return false;
        }
        let mut props = self.item_properties(item);
        if props.iter().any(|&(id, _)| id == property) {
            return false;
        }
        props.push((property, power));
        if let Some(o) = self.world.get_mut(item) {
            o.item_properties = Some(props);
        }
        if let Some(who) = self.worn_by(item) {
            for e in self.effects_of_properties(item, &[(property, power)]) {
                self.apply_effect(who, e);
            }
        }
        true
    }

    /// `RemoveItemProperty`: DAOrigins.exe (0x665370, 0x7562e0) takes the first entry with
    /// that property off the item; worn, the effect it gave its wearer goes too (0x6b3540).
    /// Returns whether it had one.
    pub fn remove_item_property(&mut self, item: ObjectId, property: i32) -> bool {
        let mut props = self.item_properties(item);
        let Some(i) = props.iter().position(|&(id, _)| id == property) else {
            return false;
        };
        let gone = props.remove(i);
        if let Some(o) = self.world.get_mut(item) {
            o.item_properties = Some(props);
        }
        if let Some(who) = self.worn_by(item) {
            for e in self.effects_of_properties(item, &[gone]) {
                let held = self
                    .world
                    .get(who)
                    .and_then(|o| o.effects.iter().find(|x| x.same_payload(&e)).cloned());
                if let Some(held) = held {
                    self.remove_effect(who, &held);
                }
            }
        }
        true
    }

    /// Whether `creature` may use `item`, as far as the item's restriction properties say
    /// (`CanUseItem`; the inventory screen's `CreatureCanUseItem`). DAOrigins.exe's check
    /// (0x6ae8b0), for each of the item's properties whose `itemprps.gda` kind (column
    /// 0x4fe5e973) is 3: clears the creature's answer (+0xc70), runs the row's script
    /// (column 0x75784cde, the `ip_restrict_*` scripts) on the creature with
    /// `EVENT_TYPE_ITEM_ONTEST_USABLE` (70; creator the creature, objects the item and the
    /// creature, int 0 the property), and refuses the item if the answer, given with
    /// `SetCanUseItem`, is still 0. Provisional: the check's other tests are not modelled
    /// (the item's flag at +0x5e0, its attribute requirements, 0x6aceb0, and its list of
    /// allowed users, +0x6ec), nor is the property's int traced (the row is passed).
    pub fn can_use_item(&mut self, creature: ObjectId, item: ObjectId) -> bool {
        use thedas_formats::gda::Cell;
        if self.world.get(creature).is_none() || self.world.get(item).is_none() {
            return false;
        }
        let scripts: Vec<(i32, String)> = {
            let tables = self.world.tables();
            let Some(t) = tables.get(0, "itemprps") else {
                return true;
            };
            self.item_properties(item)
                .into_iter()
                .filter(|&(row, _)| {
                    matches!(
                        t.cell_by_hash(ITEMPRPS_KIND, row as i64),
                        Some(Cell::Int(3))
                    )
                })
                .map(|(row, _)| {
                    let script = match t.cell_by_hash(ITEMPRPS_SCRIPT, row as i64) {
                        Some(Cell::String(s)) => s.trim_end_matches('\0').trim().to_string(),
                        _ => String::new(),
                    };
                    (row, script)
                })
                .collect()
        };
        for (row, script) in scripts {
            self.can_use_answers.insert(creature, 0);
            if !script.is_empty() {
                let mut ev = self.new_event("ITEM_ONTEST_USABLE", Some(creature));
                ev.objects = vec![item, creature];
                ev.ints = vec![row];
                self.run_script(&script, creature, Some(ev));
            }
            if self.can_use_answers.get(&creature).copied().unwrap_or(0) == 0 {
                return false;
            }
        }
        true
    }

    /// An item's on-hit properties (`itemprps.gda` kind 2, column 0x4fe5e973) with their
    /// powers, in the item's order.
    pub(crate) fn on_hit_properties(&self, item: ObjectId) -> Vec<(i32, i32)> {
        let tables = self.world.tables();
        let Some(t) = tables.get(0, "itemprps") else {
            return Vec::new();
        };
        self.item_properties(item)
            .into_iter()
            .filter(|&(row, _)| {
                matches!(
                    t.cell_by_hash(ITEMPRPS_KIND, row as i64),
                    Some(thedas_formats::gda::Cell::Int(2))
                )
            })
            .collect()
    }

    /// The effects the worn-kind properties `props` of `item` put on its wearer.
    fn effects_of_properties(
        &self,
        item: ObjectId,
        props: &[(i32, i32)],
    ) -> Vec<crate::world::Effect> {
        use thedas_formats::gda::Cell;
        if self.game == thedas_vfs::Game::DragonAge2 {
            return self.da2_item_property_effects(item, props);
        }
        let props = props.to_vec();
        if props.is_empty() {
            return Vec::new();
        }
        let tables = self.world.tables();
        let Some(t) = tables.get(0, "itemprps") else {
            return Vec::new();
        };
        let int = |c: Option<&Cell>| match c {
            Some(Cell::Int(v)) => *v as i32,
            Some(Cell::Float(v)) => *v as i32,
            _ => 0,
        };
        let float = |c: Option<&Cell>| match c {
            Some(Cell::Float(v)) => *v as f32,
            Some(Cell::Int(v)) => *v as f32,
            _ => 0.0,
        };
        let permanent = crate::sim::constant(&self.table, "EFFECT_DURATION_TYPE_PERMANENT");
        let mut out = Vec::new();
        for (row, power) in props {
            let row = row as i64;
            if int(t.cell_by_hash(ITEMPRPS_KIND, row)) as i64 != ITEMPRPS_WORN {
                continue;
            }
            let ty = int(t.cell("Effect", row));
            if ty == 0 {
                continue;
            }
            let mut ints = vec![int(t.cell("Int0", row)), int(t.cell("Int1", row))];
            let mut floats = vec![float(t.cell("Float0", row)), float(t.cell("Float1", row))];
            let scaled = match t.cell_by_hash(ITEMPRPS_POWER_PARAM, row) {
                Some(Cell::String(s)) => s.trim_end_matches('\0').trim().to_ascii_lowercase(),
                _ => String::new(),
            };
            match scaled.as_str() {
                "float0" => floats[0] *= power as f32,
                "float1" => floats[1] *= power as f32,
                "int0" => ints[0] *= power,
                "int1" => ints[1] *= power,
                _ => {}
            }
            // Every attribute: strength to constitution (properties 1 to 6).
            if ty == 1012 && ints[0] == ALL_ATTRIBUTES {
                ints = (1..=6).collect();
                floats = vec![floats[0]; 6];
            }
            out.push(crate::world::Effect {
                ty,
                creator: Some(item),
                ints,
                floats,
                duration_type: permanent,
                ..Default::default()
            });
        }
        out
    }

    /// DA2 LoadItemsFromTemplate (0x598a10) destroys equipped items first,
    /// even when the backpack is retained. Destruction goes through the normal
    /// unequip path so item effects end before their source objects disappear.
    pub(crate) fn discard_da2_template_inventory(&mut self, owner: ObjectId, replace: bool) {
        let old = self
            .world
            .get(owner)
            .map(|o| {
                let equipped: std::collections::BTreeSet<_> = o
                    .equipment
                    .values()
                    .chain(o.other_set.values())
                    .copied()
                    .collect();
                let mut old: Vec<_> = o
                    .inventory
                    .iter()
                    .copied()
                    .filter(|i| equipped.contains(i))
                    .collect();
                // Keep malformed older snapshots from retaining an equipped item
                // merely because it is absent from the owner's inventory list.
                for &item in &equipped {
                    if !old.contains(&item) {
                        old.push(item);
                    }
                }
                if replace {
                    old.extend(
                        o.inventory
                            .iter()
                            .copied()
                            .filter(|i| !equipped.contains(i)),
                    );
                }
                old
            })
            .unwrap_or_default();
        for item in old {
            // Also unwind old snapshots whose equipment is absent from inventory;
            // destroy_object discovers holders through their inventory lists.
            self.unequip(owner, item);
            self.destroy_object(item);
        }
    }

    /// The engine's side of `who` putting on `item` (already in `slot`): the item's
    /// properties take effect through the creature's `APPLY_EFFECT` handler, and its script
    /// hears `EVENT_TYPE_EQUIP`. DAOrigins.exe sends that from one routine (0x6c16e0, type
    /// 0xb set at 0x6c190f), the equipment container's item-added hook, so every way of
    /// equipping reaches it: `EquipItem`, `LoadItemsFromTemplate` and a creature's template
    /// gear when it is created. Object 0 is the item, integer 0 the slot, integer 1 whether
    /// the player equipped it on the inventory screen (bit 0x80 of the flag word 0xc371b4,
    /// set only by the screen's handler 0x6be330). rules_core works out the main-hand
    /// damage (property 50) the record shows from it.
    pub(crate) fn wear(&mut self, who: ObjectId, item: ObjectId, slot: i32) {
        self.wear_as(who, item, slot, false);
    }

    /// [`Sim::wear`], saying whether the player did it on the inventory screen.
    pub(crate) fn wear_as(&mut self, who: ObjectId, item: ObjectId, slot: i32, by_player: bool) {
        if self.game == thedas_vfs::Game::Origins {
            // Origins' active-container add hook (0x6c16e0) resolves hand conflicts
            // before applying properties or sending EQUIP. Use the full unequip
            // routine: unlike an ordinary slot swap, this also raises training 1070.
            let conflict = if self.origins_main_hand_only(item) {
                self.world
                    .get(who)
                    .and_then(|o| o.equipment.get(&1))
                    .copied()
            } else if slot == 1 {
                self.world
                    .get(who)
                    .and_then(|o| o.equipment.get(&0))
                    .copied()
                    .filter(|&main| self.origins_main_hand_only(main))
            } else {
                None
            };
            if let Some(old) = conflict.filter(|&old| old != item) {
                self.unequip(who, old);
            }
        }
        for e in self.property_effects(item) {
            self.apply_effect(who, e);
        }
        self.equip_event("EQUIP", who, item, slot, by_player);
        // A party member's equip is what the inventory lesson waits for: DAOrigins.exe raises
        // `EVENT_TYPE_TRAINING_ITEM_EQUIPPED` (1071) from its equip routine (0x6bc7f0, at
        // 0x6bcb9d), whoever asked for the equip. Provisional: the argument is taken to be
        // the slot (the script waits on slot 4, read with GetItemInEquipSlot).
        if self.world.training.mode != 0 && crate::engine::party_of(self).contains(&who) {
            self.training_event("ITEM_EQUIPPED", slot);
        }
    }

    /// The engine's side of `who` taking off `item` (from `slot`): the effects it created on
    /// `who` go through the `REMOVE_EFFECT` handler, and the script hears
    /// `EVENT_TYPE_UNEQUIP` (type 0xc, 0x6c1b2d), laid out as [`Sim::wear`]'s event.
    /// Provisional: its integer 1 is taken to mean the same as the equip event's.
    pub(crate) fn take_off(&mut self, who: ObjectId, item: ObjectId, slot: i32) {
        let mine: Vec<crate::world::Effect> = self
            .world
            .get(who)
            .map(|o| {
                o.effects
                    .iter()
                    .filter(|e| e.creator == Some(item))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        for e in mine {
            self.remove_effect(who, &e);
        }
        self.equip_event("UNEQUIP", who, item, slot, false);
    }

    pub(crate) fn equip_event(
        &mut self,
        name: &str,
        who: ObjectId,
        item: ObjectId,
        slot: i32,
        by_player: bool,
    ) {
        let mut ev = self.new_event(name, Some(who));
        ev.objects = vec![item];
        ev.ints = vec![slot, by_player as i32];
        // Delivered at once, inside the equip or unequip: DAOrigins.exe's container hooks
        // (EQUIP 0x6c16e0, UNEQUIP 0x6c19f0) send through 0x64ad80, which runs the script in
        // the same call, not through the queue (0x64ace0).
        self.deliver(who, ev, None);
    }

    /// The slot `who` wears `item` in.
    pub fn worn_slot(&self, who: ObjectId, item: ObjectId) -> Option<i32> {
        self.world
            .get(who)?
            .equipment
            .iter()
            .find(|(_, i)| **i == item)
            .map(|(s, _)| *s)
    }

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
        if !self.can_change_equipment(who) {
            return false;
        }
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
            match fits
                .iter()
                .find(|s| {
                    !worn.contains_key(s) && (**s != 1 || !self.origins_off_hand_blocked(who))
                })
                .or(fits.first())
            {
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
        if self.game == thedas_vfs::Game::Origins
            && (self.equipment_slot_locked(who, item, slot, false)
                || (slot == 1 && self.origins_off_hand_blocked(who))
                || (self.origins_main_hand_only(item)
                    && self.equipment_slot_locked(who, item, 1, false))
                || self
                    .wearer(item)
                    .is_some_and(|m| self.item_irremovable(item) || !self.can_change_equipment(m)))
        {
            return false;
        }
        // Whoever wore it gives it up, and what `who` wore in the slot comes off: both hear
        // UNEQUIP, but neither counts as an unequip for the inventory lesson (see
        // `Sim::displace`).
        if let Some(m) = self.wearer(item) {
            self.displace(m, item);
        }
        if let Some(old) = self
            .world
            .get(who)
            .and_then(|o| o.equipment.get(&slot).copied())
        {
            self.displace(who, old);
        }
        for m in party {
            if m != who {
                if let Some(o) = self.world.get_mut(m) {
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
        self.wear_as(who, item, slot, true);
        true
    }

    /// Takes `item` off `who` (either weapon set) as the engine does: see [`Sim::take_off`].
    /// Returns whether `who` wore it.
    ///
    /// This is the exe's unequip routine (0x6b7690): after the item is off and its owner has
    /// heard UNEQUIP, a party member's unequip raises
    /// `EVENT_TYPE_TRAINING_ITEM_UNEQUIPPED` (1070, 0x6b7886) for the inventory lesson.
    /// Provisional: the argument is taken to be the slot.
    pub(crate) fn unequip(&mut self, who: ObjectId, item: ObjectId) -> bool {
        match self.displace(who, item) {
            Some(slot) => {
                if self.world.training.mode != 0 && crate::engine::party_of(self).contains(&who) {
                    self.training_event("ITEM_UNEQUIPPED", slot);
                }
                true
            }
            None => false,
        }
    }

    /// Takes `item` out of `who`'s equipment because another item takes its slot, or it
    /// moves to another wearer: its effects end and `who` hears UNEQUIP, but this is not the
    /// unequip routine, so no training event is raised (DAOrigins.exe's container Add
    /// removes the occupant itself, 0x762240, and only two-handed and ammunition conflicts
    /// go through 0x6b7690). Returns the slot it was in.
    pub(crate) fn displace(&mut self, who: ObjectId, item: ObjectId) -> Option<i32> {
        let o = self.world.get_mut(who)?;
        let slot = o
            .equipment
            .iter()
            .chain(o.other_set.iter())
            .find(|(_, i)| **i == item)
            .map(|(s, _)| *s);
        o.equipment.retain(|_, i| *i != item);
        o.other_set.retain(|_, i| *i != item);
        let slot = slot?;
        self.take_off(who, item, slot);
        Some(slot)
    }

    /// The player takes `item` off party member `who`; it stays in the backpack. Returns
    /// whether `who` wore it.
    pub fn unequip_item(&mut self, who: ObjectId, item: ObjectId) -> bool {
        if !self.can_change_equipment(who)
            || (self.game == thedas_vfs::Game::Origins && self.item_irremovable(item))
        {
            return false;
        }
        if !crate::engine::party_of(self).contains(&who) {
            return false;
        }
        self.unequip(who, item)
    }

    /// Discard an item from the party into a recoverable sack at `who`'s feet.
    ///
    /// The native `DropItemMessage` handler (0x6a72e0) removes the original item through
    /// its container (0x762100), which calls the equipment removal hook (0x797e20,
    /// 0x6c19f0) before inventory removal (0x763240). Those events are UNEQUIP (12)
    /// on the wearer and INVENTORY_REMOVED (7) on the possessor, with the same item id.
    /// The player-drop flag 0x400 supplies removal integer 0 = 1.
    ///
    /// Intentional departure from the native discard: keep the item in a new bag rather
    /// than destroy it. Provisional: the navigation floor stands in for the physical
    /// walk surface; without a floor sample, keep the creature's feet height.
    pub fn drop_item(&mut self, who: ObjectId, item: ObjectId) -> bool {
        if self.game != thedas_vfs::Game::Origins
            || !self
                .world
                .get(item)
                .is_some_and(|o| o.kind == ObjectKind::Item && !o.plot)
        {
            return false;
        }
        let party = crate::engine::party_of(self);
        if !party.contains(&who) {
            return false;
        }
        let Some(owner) = party.into_iter().find(|&m| {
            self.world.get(m).is_some_and(|o| {
                o.inventory.contains(&item)
                    || o.equipment
                        .values()
                        .chain(o.other_set.values())
                        .any(|&i| i == item)
            })
        }) else {
            return false;
        };
        let worn = self.world.get(owner).is_some_and(|o| {
            o.equipment
                .values()
                .chain(o.other_set.values())
                .any(|&i| i == item)
        });
        if worn && (self.item_irremovable(item) || !self.can_change_equipment(owner)) {
            return false;
        }
        let Some(c) = self
            .world
            .get(who)
            .filter(|o| o.kind == ObjectKind::Creature && o.active && !o.dead)
        else {
            return false;
        };
        let Some(area) = c.area else { return false };
        let (mut pos, facing) = (c.position, c.facing);
        if let Some(z) = self.floor_at(area, pos) {
            pos[2] = z;
        }
        // This also removes item-created effects and delivers UNEQUIP synchronously.
        // Do not just clear the equipment map: rules_core observes this boundary.
        self.displace(owner, item);
        // An unequip script may destroy or transfer the item. Do not resurrect it or
        // steal it back from the script's recipient.
        if self.world.get(item).is_none()
            || self.world.objects.values().any(|o| {
                (o.id != owner && o.inventory.contains(&item))
                    || o.equipment
                        .values()
                        .chain(o.other_set.values())
                        .any(|&i| i == item)
            })
        {
            return false;
        }
        let bag = self.create_from_template(
            ObjectKind::Placeable,
            BODY_BAG_TEMPLATE,
            Some(area),
            pos,
            facing,
        );
        if let Some(b) = self.world.get_mut(bag) {
            b.appearance = DROPPED_ITEM_APPEARANCE;
            b.fields
                .insert("Appearance".into(), DROPPED_ITEM_APPEARANCE);
            b.inventory.push(item);
        }
        if let Some(o) = self.world.get_mut(owner) {
            o.inventory.retain(|&i| i != item);
        }
        if let Some(i) = self.world.get_mut(item) {
            i.area = Some(area);
            i.position = pos;
        }
        let mut removed = self.new_event("INVENTORY_REMOVED", Some(owner));
        removed.objects = vec![item];
        removed.ints = vec![1];
        self.world.queue_event(0, owner, removed, None);
        true
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
            self.player_selects(party[next]);
            return true;
        }
        if name == MSG_SWITCH_WEAPON_SET {
            let set = args.first().and_then(|a| a.trim().parse::<f64>().ok());
            let (Some(who), Some(set)) = (self.world.controlled(), set) else {
                return false;
            };
            self.switch_weapon_set(who, set as i32);
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
            MSG_DROP => self.drop_item(who, item),
            _ => false,
        }
    }

    /// `ScaleEquippedItems` (action 386, handler 0x6664d0 calling 0x6a88f0): gives a
    /// creature's equipped items the material their material progression (`TS_Material`,
    /// table 199: a row per progression, columns `Material1` to `Material15`) has at the
    /// target level. As DAOrigins.exe does it:
    /// - party members are not scaled;
    /// - the tier is `level / 3`, reading column `Material{tier + 1}` (the exe's eight column
    ///   hashes at 0xb0f860 are `Material1` to `Material8`); from level 24 (tier 8) on
    ///   nothing changes;
    /// - only the slots in the exe's list at 0xb0f880 (every slot to 13 but the cloak's and
    ///   slot 3);
    /// - only items whose base item's `BITM_base.gda` column `#66f63962` is set (0x752c80
    ///   reads the column the item loader keeps at item+0x6e4, 0x754822): weapons, shields,
    ///   armour, gloves, boots and helmets; never clothing (base item 38), so a noble's
    ///   doublet keeps its `Clothing, Noble` material and tint;
    /// - with a progression (`MaterialProgress`) and a non-zero material at that tier.
    ///
    /// Provisional: a level of 0 or less makes the exe read the area's level from table
    /// 0xe1 (and skip a creature already scaled); here it reads tier 0. The unique flag is
    /// not consulted (the exe's loop does not read it).
    pub fn scale_equipped_items(&mut self, creature: ObjectId, level: i32) {
        /// Slots the exe's loop visits (0xb0f880..0xb0f8b0).
        const SLOTS: [i32; 12] = [0, 1, 2, 4, 5, 6, 7, 9, 10, 11, 12, 13];
        /// `BITM_base.gda`'s column of base items that scale (name unknown).
        const SCALES: u32 = 0x66f6_3962;
        if crate::engine::party_of(self).contains(&creature) {
            return;
        }
        let tier = level.max(0) / 3;
        if tier > 7 {
            return;
        }
        let items: Vec<ObjectId> = self
            .world
            .get(creature)
            .map(|o| {
                SLOTS
                    .iter()
                    .filter_map(|s| o.equipment.get(s).copied())
                    .collect()
            })
            .unwrap_or_default();
        let tables = self.world.tables();
        let Some(t) = tables.get(199, "") else {
            return;
        };
        let bitm = tables.get(0, "bitm_base");
        for item in items {
            let Some(o) = self.world.get(item) else {
                continue;
            };
            let scales = o.fields.get("BaseItem").is_some_and(|&b| {
                matches!(
                    bitm.as_ref().and_then(|g| g.cell_by_hash(SCALES, b as i64)),
                    Some(thedas_formats::gda::Cell::Int(v)) if *v != 0
                )
            });
            let progression = o.fields.get("MaterialProgress").copied().unwrap_or(0);
            if !scales || progression == 0 {
                continue;
            }
            let material = match t.cell(&format!("Material{}", tier + 1), progression as i64) {
                Some(thedas_formats::gda::Cell::Int(m)) if *m != 0 => *m as i32,
                _ => continue,
            };
            if let Some(o) = self.world.get_mut(item) {
                o.fields.insert("Material".into(), material);
            }
        }
    }

    /// How many of an item one stack holds: its base item's `StackSize` in `BITM_base.gda`
    /// (column hash 0x39974122, read at 0x753070); 1 when the base item does not stack.
    pub fn max_stack(&self, item: ObjectId) -> i32 {
        let Some(base) = self
            .world
            .get(item)
            .and_then(|o| o.fields.get("BaseItem").copied())
        else {
            return 1;
        };
        let tables = self.world.tables();
        match tables
            .get(0, "bitm_base")
            .and_then(|t| t.cell_by_hash(BITM_STACK_SIZE, base as i64).cloned())
        {
            Some(thedas_formats::gda::Cell::Int(n)) if n > 1 => n.min(i32::MAX as i64) as i32,
            Some(thedas_formats::gda::Cell::Float(f)) if f > 1.0 => f.min(i32::MAX as f64) as i32,
            _ => 1,
        }
    }

    /// An item's stack count (`StackSize`, at least 1).
    pub fn stack_count(&self, item: ObjectId) -> i32 {
        self.world
            .get(item)
            .and_then(|o| o.fields.get("StackSize").copied())
            .unwrap_or(1)
            .max(1)
    }

    /// Whether `item` would merge into the stack `into`, as DAOrigins.exe's container decides
    /// (0x755cb0 -> 0x755a70): both stackable, and the same tag, material, ability, damaged,
    /// indestructible, irremovable and store flags, and the same item properties in any order.
    /// The template, cost, plot and droppable flags are not compared.
    fn stacks_with(&self, item: ObjectId, into: ObjectId) -> bool {
        if item == into || self.max_stack(item) <= 1 || self.max_stack(into) <= 1 {
            return false;
        }
        let (Some(a), Some(b)) = (self.world.get(item), self.world.get(into)) else {
            return false;
        };
        if a.kind != ObjectKind::Item || b.kind != ObjectKind::Item || a.tag != b.tag {
            return false;
        }
        let same = |k: &str| {
            a.fields.get(k).copied().unwrap_or(0) == b.fields.get(k).copied().unwrap_or(0)
        };
        if !STACK_FIELDS.iter().all(|k| same(k)) {
            return false;
        }
        let mut pa = self.item_properties(item);
        let mut pb = self.item_properties(into);
        pa.sort_unstable();
        pb.sort_unstable();
        pa == pb
    }

    /// Moves an item into `to`'s inventory (from wherever it is) and tells both owners.
    ///
    /// Puts `item` in `to`'s inventory: a stackable item fills the stacks it matches first,
    /// in inventory order, up to their maximum (DAOrigins.exe's container add,
    /// 0x7623f4-0x7624c9; 0x755d10). What is left becomes a stack of its own; an item taken
    /// up entirely is destroyed (0x752c90). Returns the item that holds it now (the last
    /// stack it went into, or itself).
    pub(crate) fn stack_into(&mut self, to: ObjectId, item: ObjectId) -> ObjectId {
        // The party receiving an item raises the HUD's "Items received" notification (one
        // of its kind at a time). Provisional: seen in the original at first control, for
        // the items character creation gave; where the exe raises it is not traced.
        if self.in_party(to) {
            let kind = crate::journal::ALERT_ITEMS;
            self.world.alerts.retain(|a| a.kind != kind);
            self.world.alerts.push(crate::journal::Alert {
                kind,
                ..Default::default()
            });
        }
        let mut landed = item;
        let max = self.max_stack(item);
        if max > 1 {
            let mut left = self.stack_count(item);
            let worn: Vec<ObjectId> = self
                .world
                .get(to)
                .map(|o| {
                    o.equipment
                        .values()
                        .chain(o.other_set.values())
                        .copied()
                        .collect()
                })
                .unwrap_or_default();
            let stacks: Vec<ObjectId> = self
                .world
                .get(to)
                .map(|o| o.inventory.clone())
                .unwrap_or_default()
                .into_iter()
                .filter(|&i| !worn.contains(&i) && self.stacks_with(item, i))
                .collect();
            for stack in stacks {
                if left == 0 {
                    break;
                }
                let have = self.stack_count(stack);
                let room = (max - have).max(0);
                let moved = room.min(left);
                if moved > 0 {
                    if let Some(o) = self.world.get_mut(stack) {
                        o.fields.insert("StackSize".into(), have + moved);
                    }
                    left -= moved;
                    landed = stack;
                }
            }
            if left == 0 {
                if let Some(t) = self.world.get_mut(to) {
                    t.inventory.retain(|&i| i != item);
                }
                self.world.objects.remove(&item);
            } else if let Some(o) = self.world.get_mut(item) {
                o.fields.insert("StackSize".into(), left);
                landed = item;
            }
        }
        if landed == item {
            if let Some(t) = self.world.get_mut(to) {
                if !t.inventory.contains(&item) {
                    t.inventory.push(item);
                }
            }
        }
        landed
    }

    /// A recipe (base item 108, `Crafting Recipe`) the party receives is learned, its
    /// `CraftingRecipeID` added to the known recipes, and the item is gone, as the exe's
    /// inventory add does (0x740a58: an item of base type 0x6c adds its recipe id, item
    /// +0x670, to the inventory's recipe list (0x7408b0, the console's `AddRecipe`) and
    /// deletes the item). Provisional: "the party" is taken to be the hero and followers.
    ///
    /// Merges the stackable items `owner` carries (not wears) into stacks, in inventory
    /// order, as adding them one by one would have (see [`Sim::stack_into`]).
    pub(crate) fn restack(&mut self, owner: ObjectId) {
        let Some(o) = self.world.get(owner) else {
            return;
        };
        let worn: Vec<ObjectId> = o
            .equipment
            .values()
            .chain(o.other_set.values())
            .copied()
            .collect();
        let carried: Vec<ObjectId> = o
            .inventory
            .iter()
            .copied()
            .filter(|i| !worn.contains(i) && self.max_stack(*i) > 1)
            .collect();
        for item in carried {
            if let Some(o) = self.world.get_mut(owner) {
                o.inventory.retain(|&i| i != item);
            }
            self.stack_into(owner, item);
        }
    }

    /// Coin (base item 17, `Money - Copper`) the hero receives is counted into the hero's
    /// money and the item is gone, as DAOrigins.exe's inventory add does (0x7409fa: an item
    /// of base type 0x11 added to the player's inventory adds its stack size to the money
    /// and deletes the item). Provisional: the owner it compares with is taken to be the
    /// hero, and no inventory events are sent for the coin.
    pub fn give_item(&mut self, item: ObjectId, to: ObjectId) -> bool {
        let Some(it) = self.world.get(item) else {
            return false;
        };
        if self.world.get(to).is_none() {
            return false;
        }
        let coin = (it.fields.get("BaseItem") == Some(&BASE_ITEM_COPPER)
            && Some(to) == self.world.hero)
            .then(|| it.fields.get("StackSize").copied().unwrap_or(1).max(1));
        let recipe = (it.fields.get("BaseItem") == Some(&BASE_ITEM_RECIPE))
            .then(|| it.fields.get("CraftingRecipeID").copied().unwrap_or(0))
            .filter(|_| self.in_party(to));
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
        if let Some(r) = recipe {
            if let Some(t) = self.world.get_mut(to) {
                t.inventory.retain(|&i| i != item);
            }
            self.world.recipes.insert(r);
            self.world.objects.remove(&item);
            return true;
        }
        if let Some(coppers) = coin {
            if let Some(t) = self.world.get_mut(to) {
                t.inventory.retain(|&i| i != item);
                t.money = t.money.saturating_add(coppers);
            }
            self.world.objects.remove(&item);
            // DA2 transfers remove the item from the source inventory before adding it
            // to the destination (0x6cc8f6 -> 0x6c71b0; the coin path is 0x6cc0c6).
            // Conversion to currency must not swallow the source's removal event: the
            // authored Cavril chest script completes its task on that event.
            if self.game == thedas_vfs::Game::DragonAge2
                && let Some(f) = from
            {
                let mut removed = self.new_event("INVENTORY_REMOVED", Some(to));
                removed.objects = vec![item];
                removed.ints = vec![1];
                self.world.queue_event(0, f, removed, None);
            }
            return true;
        }
        let landed = self.stack_into(to, item);
        let mut added = self.new_event("INVENTORY_ADDED", from.or(Some(OBJECT_INVALID)));
        added.objects = vec![landed];
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

    /// Origins native 350: snapshot the source's default inventory, then perform
    /// ordinary per-item transfers. Both weapon sets remain in equipment.
    pub(crate) fn move_all_items(&mut self, from: ObjectId, to: ObjectId) {
        let inventory_kind = |k| {
            matches!(
                k,
                ObjectKind::Creature | ObjectKind::Placeable | ObjectKind::Store
            )
        };
        if from == to || !self.world.get(to).is_some_and(|o| inventory_kind(o.kind)) {
            return;
        }
        let Some(src) = self.world.get(from).filter(|o| inventory_kind(o.kind)) else {
            return;
        };
        let worn: Vec<_> = src
            .equipment
            .values()
            .chain(src.other_set.values())
            .copied()
            .collect();
        let items: Vec<_> = src
            .inventory
            .iter()
            .copied()
            .filter(|id| !worn.contains(id))
            .collect();
        for item in items {
            let Some(it) = self.world.get(item).filter(|o| o.kind == ObjectKind::Item) else {
                continue;
            };
            let target = self.world.get(to).unwrap();
            // Creature default inventories route plot items and coin outside the
            // ordinary backpack capacity (0x73f600). Backpack removal itself is
            // unconditional (0x722ae0); Irremovable restricts equipment removal.
            let special = target.kind == ObjectKind::Creature
                && (it.plot || it.fields.get("BaseItem") == Some(&BASE_ITEM_COPPER));
            let max = target
                .fields
                .get("MaxInventorySize")
                .copied()
                .unwrap_or_else(|| {
                    if target.kind == ObjectKind::Creature {
                        crate::engine::DEFAULT_INVENTORY_SIZE
                    } else {
                        -1
                    }
                });
            let ordinary: Vec<_> = target
                .inventory
                .iter()
                .copied()
                .filter(|id| {
                    !target
                        .equipment
                        .values()
                        .chain(target.other_set.values())
                        .any(|i| i == id)
                        && self
                            .world
                            .get(*id)
                            .is_some_and(|o| target.kind != ObjectKind::Creature || !o.plot)
                })
                .collect();
            let merges = ordinary
                .iter()
                .any(|&id| self.stacks_with(item, id) && self.stack_count(id) < self.max_stack(id));
            if special || max == -1 || ordinary.len() < max.max(0) as usize || merges {
                self.give_item(item, to);
            }
        }
    }

    /// Whether `id` is the hero or one of the hero's followers.
    pub(crate) fn in_party(&self, id: ObjectId) -> bool {
        let Some(hero) = self.world.hero else {
            return false;
        };
        id == hero
            || self
                .world
                .get(hero)
                .is_some_and(|h| h.followers.contains(&id))
    }

    /// Learns the recipe items the party holds and drops them (see [`Self::give_item`]):
    /// saves made before recipes were learned keep them as items.
    pub fn learn_held_recipes(&mut self) {
        let held: Vec<(ObjectId, ObjectId, i32)> = self
            .world
            .objects
            .values()
            .filter(|o| self.in_party(o.id))
            .flat_map(|o| o.inventory.iter().map(move |&i| (o.id, i)))
            .filter_map(|(owner, i)| {
                let it = self.world.get(i)?;
                (it.fields.get("BaseItem") == Some(&BASE_ITEM_RECIPE)).then(|| {
                    (
                        owner,
                        i,
                        it.fields.get("CraftingRecipeID").copied().unwrap_or(0),
                    )
                })
            })
            .collect();
        for (owner, item, r) in held {
            if let Some(o) = self.world.get_mut(owner) {
                o.inventory.retain(|&i| i != item);
            }
            self.world.recipes.insert(r);
            self.world.objects.remove(&item);
        }
    }

    /// What a creature's equipped items add to a property: for each item stat that
    /// `itemstatdefines.gda` links to the property (its fourth column), the item's value
    /// from `itemstats.gda` (by base item type), scaled by its material as `GetItemStat`
    /// scales it ([`Sim::item_stat`]). Items' own property lists (enchantments) are not
    /// part of this: they are effects on the wearer while the item is worn.
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

    /// Origins' two-handed predicate (0xa664c0 -> 0xa66440): the entire
    /// EquippableSlots mask equals MAIN, including bows and crossbows.
    fn origins_main_hand_only(&self, item: ObjectId) -> bool {
        self.item_slots(item) == [0]
    }

    fn origins_off_hand_blocked(&self, who: ObjectId) -> bool {
        self.game == thedas_vfs::Game::Origins
            && self
                .world
                .get(who)
                .and_then(|o| o.equipment.get(&0))
                .is_some_and(|&main| self.origins_main_hand_only(main))
    }

    /// Equips an item in its first free slot (or its first slot); returns the slot.
    pub fn equip_default(&mut self, who: ObjectId, item: ObjectId) -> Option<i32> {
        let slots = self.item_slots(item);
        let taken = self.world.get(who)?.equipment.clone();
        let slot = slots
            .iter()
            .copied()
            .find(|s| !taken.contains_key(s) && (*s != 1 || !self.origins_off_hand_blocked(who)))
            .or_else(|| slots.first().copied())?;
        if self.equipment_slot_locked(who, item, slot, false) {
            return None;
        }
        self.unequip(who, item);
        if let Some(old) = self.world.get(who)?.equipment.get(&slot).copied() {
            self.unequip(who, old);
        }
        let o = self.world.get_mut(who)?;
        if !o.inventory.contains(&item) {
            o.inventory.push(item);
        }
        o.equipment.insert(slot, item);
        self.wear(who, item, slot);
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
                self.item_slots(item).into_iter().find(|s| {
                    !taken.contains_key(s) && (*s != 1 || !self.origins_off_hand_blocked(who))
                })
            };
            if let (Some(slot), Some(o)) = (free, self.world.get_mut(who)) {
                o.equipment.insert(slot, item);
                self.wear(who, item, slot);
                out.push((item, slot));
            }
        }
        out
    }

    /// Takes everything out of a container, body bag or dead creature into `to`'s inventory,
    /// and its money into `to`'s. Equipped items of a creature stay unless they are
    /// droppable. Returns the items taken.
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
        let money = src.money.max(0);
        for &i in &items {
            self.give_item(i, to);
        }
        if money > 0
            && from != to
            && self.world.get(to).is_some()
            && let Some(src) = self.world.get_mut(from)
        {
            src.money = 0;
            if let Some(t) = self.world.get_mut(to) {
                t.money = t.money.saturating_add(money);
            }
        }
        items
    }

    /// The body bag a dead creature left (`GetCreatureBodyBag`), when it is still in the
    /// world and active: what a click on the corpse opens. In the original the player loots a
    /// corpse by clicking the body; the bag itself has no model to click (appearance 16).
    pub fn corpse_body_bag(&self, creature: ObjectId) -> Option<ObjectId> {
        let c = self.world.get(creature).filter(|c| c.dead)?;
        let bag = *c.fields.get(BODY_BAG_FIELD)? as ObjectId;
        self.world
            .get(bag)
            .filter(|b| b.kind == ObjectKind::Placeable && b.active && b.area == c.area)
            .map(|b| b.id)
    }

    /// `SpawnBodyBag`: leaves a container where a creature fell holding its droppable items
    /// (all of them when forced) and its money. Nothing is left when there is nothing to
    /// take: no money and no items to drop (DAOrigins.exe 0x6a90d0, the check 0x6a91e0
    /// makes, counts the creature's money before its droppable items).
    ///
    /// Provisional: the original's bag refers back to the creature and is looted from its
    /// inventory; here the money moves into the bag, as the items do.
    pub fn spawn_body_bag(&mut self, creature: ObjectId, force: bool) -> Option<ObjectId> {
        let c = self.world.get(creature)?;
        // A bag the creature already left is used again, moved to where it is now
        // (DAOrigins.exe 0x6a92b9-0x6a931e).
        let old_bag = c
            .fields
            .get(BODY_BAG_FIELD)
            .map(|&b| b as ObjectId)
            .filter(|&b| self.world.get(b).is_some());
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
        let money = c.money.max(0);
        if items.is_empty() && money == 0 && old_bag.is_none() {
            return None;
        }
        // Built from its template (`gen_ip_bodybag.utp`: `placeable_core`, appearance 16 "Body
        // Bag", useable), as `CreateObject` builds a placeable: the player's use of it goes to
        // `placeable_core`, which opens it for looting (`OpenInventory`). A bare object had
        // no script, so using it did nothing and what it held could not be taken (Oskias's
        // lyrium nuggets, the Dwarf Commoner's opening quest).
        let bag = match old_bag {
            Some(b) => b,
            None => {
                self.create_from_template(ObjectKind::Placeable, BODY_BAG_TEMPLATE, area, pos, 0.0)
            }
        };
        if let Some(b) = self.world.get_mut(bag) {
            b.position = pos;
            b.area = area;
            b.template = BODY_BAG_TEMPLATE.into();
            b.inventory.extend(items.iter().copied());
            b.money = b.money.saturating_add(money);
        }
        if let Some(c) = self.world.get_mut(creature) {
            c.fields.insert(BODY_BAG_FIELD.into(), bag as i32);
            c.money -= money;
            c.inventory.retain(|i| !items.contains(i));
            c.equipment.retain(|_, i| !items.contains(i));
            c.other_set.retain(|_, i| !items.contains(i));
        }
        Some(bag)
    }
}

#[cfg(test)]
mod body_bag_tests {
    use std::sync::Arc;

    use thedas_formats::gff3::{Field, Gff3, Struct, Value as G3};
    use thedas_script::EngineTable;
    use thedas_vfs::Vfs;

    use super::*;

    /// A `gen_ip_bodybag.utp` with the fields that matter here, in a resource stack.
    fn sim_with_bag_template() -> Sim {
        // Each test writes its own template: truncating a shared file can make the
        // other test read an empty GFF while it builds its body bag.
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "thedas-bodybag-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("gen_ip_bodybag.utp");
        let field = |label: &str, value: G3| Field {
            label: label.into(),
            value,
        };
        let g = Gff3 {
            file_type: *b"UTP ",
            file_version: *b"V3.2",
            structs: vec![Struct {
                id: 0xffff_ffff,
                fields: vec![
                    field("Tag", G3::ExoString(b"gen_ip_bodybag".to_vec())),
                    field("EventScript", G3::ResRef(b"placeable_core".to_vec())),
                    field("Appearance", G3::Dword(16)),
                    field("Useable", G3::Byte(1)),
                ],
            }],
        };
        std::fs::write(&file, g.write().unwrap()).unwrap();
        let mut vfs = Vfs::new();
        vfs.push_files(std::slice::from_ref(&file), "test");
        Sim::new(Arc::new(vfs), Arc::new(EngineTable::parse(b"").unwrap()), 1)
    }

    #[test]
    fn a_body_bag_is_built_from_its_template_so_its_use_reaches_placeable_core() {
        let mut s = sim_with_bag_template();
        let foe = s.world.spawn(ObjectKind::Creature, "foe");
        s.world.get_mut(foe).unwrap().money = 5;
        let bag = s.spawn_body_bag(foe, false).unwrap();
        let b = s.world.get(bag).unwrap();
        assert_eq!(b.script.as_deref(), Some("placeable_core"));
        assert_eq!(b.appearance, 16);
        assert!(b.interactive);
        assert_eq!(b.money, 5);
    }

    #[test]
    fn a_click_on_a_corpse_finds_the_bag_it_left() {
        let mut s = sim_with_bag_template();
        let foe = s.world.spawn(ObjectKind::Creature, "foe");
        s.world.get_mut(foe).unwrap().money = 5;
        // Alive, or dead without a bag: nothing to open.
        assert_eq!(s.corpse_body_bag(foe), None);
        s.world.get_mut(foe).unwrap().dead = true;
        assert_eq!(s.corpse_body_bag(foe), None);
        let bag = s.spawn_body_bag(foe, false).unwrap();
        assert_eq!(s.corpse_body_bag(foe), Some(bag));
        // A bag that is gone (taken away, inactive) is not offered.
        s.world.get_mut(bag).unwrap().active = false;
        assert_eq!(s.corpse_body_bag(foe), None);
    }

    #[test]
    fn a_saved_bag_without_its_template_takes_it_on_load_and_keeps_its_items() {
        let mut s = sim_with_bag_template();
        // As bags were made before: a bare object holding the items.
        let bag = s.world.spawn(ObjectKind::Placeable, BODY_BAG_TEMPLATE);
        let nugget = s.world.spawn(ObjectKind::Item, "bdc130im_lyrium_ore");
        {
            let b = s.world.get_mut(bag).unwrap();
            b.template = BODY_BAG_TEMPLATE.into();
            b.position = [1.0, 2.0, 0.0];
            b.inventory.push(nugget);
        }
        s.repair_body_bags();
        let b = s.world.get(bag).unwrap();
        assert_eq!(b.script.as_deref(), Some("placeable_core"));
        assert_eq!(b.appearance, 16);
        assert_eq!(b.inventory, vec![nugget]);
        assert_eq!(b.position, [1.0, 2.0, 0.0]);
    }

    fn equipped_helm() -> (Sim, ObjectId, ObjectId, ObjectId) {
        let mut s = sim_with_bag_template();
        s.table = Arc::new(
            EngineTable::parse(
                b"int INVENTORY_SLOT_HEAD = 5;\nint EVENT_TYPE_EQUIP = 11;\n\
              int EVENT_TYPE_UNEQUIP = 12;\nint EVENT_TYPE_INVENTORY_REMOVED = 7;\n",
            )
            .unwrap(),
        );
        s.world.tables().insert(
            "bitm_base",
            &["EquippableSlots"],
            vec![(7, vec![thedas_formats::gda::Cell::Int(1 << 5)])],
        );
        let area = s.world.spawn(ObjectKind::Area, "floor");
        s.world.area = Some(area);
        s.area_layouts.insert(area, "floor".into());
        s.nav_grids.insert(
            "floor".into(),
            Some(Arc::new(thedas_formats::level::NavGrid {
                columns: 2,
                rows: 2,
                cell_size: 1.0,
                clearance: 2.0,
                base: [0.0; 3],
                normal: [0.0, 0.0, 1.0],
                data: vec![1; 4],
                heights: vec![40.0; 4],
                surfaces: Vec::new(),
            })),
        );
        let hero = s.world.spawn(ObjectKind::Creature, "hero");
        s.world.hero = Some(hero);
        let c = s.world.get_mut(hero).unwrap();
        c.area = Some(area);
        c.position = [0.75, 1.25, 3.75];
        c.facing = 43.0;
        c.script = Some("synthetic_wearer".into());
        let helm = s.world.spawn(ObjectKind::Item, "helm");
        s.world
            .get_mut(helm)
            .unwrap()
            .fields
            .insert("BaseItem".into(), 7);
        s.world.get_mut(hero).unwrap().inventory.push(helm);
        assert!(s.equip_item(hero, helm, 5));
        s.world.trace.keep = true;
        (s, area, hero, helm)
    }

    fn drop_from_screen(s: &mut Sim, item: ObjectId) -> bool {
        s.input(
            crate::commands::CommandSource::Player,
            crate::commands::SimCommand::GuiMessage {
                name: MSG_DROP.into(),
                args: vec![item.to_string()],
            },
        )
    }

    #[test]
    fn dropping_equipped_helm_moves_one_id_and_sends_unequip_before_loss() {
        let (mut s, _, hero, helm) = equipped_helm();
        s.world
            .get_mut(hero)
            .unwrap()
            .effects
            .push(crate::world::Effect {
                creator: Some(helm),
                ty: 1012,
                ..Default::default()
            });
        let before = s.world.objects.len();
        assert!(drop_from_screen(&mut s, helm));
        let h = s.world.get(hero).unwrap();
        assert!(h.equipment.is_empty() && h.other_set.is_empty() && h.inventory.is_empty());
        assert!(h.effects.iter().all(|e| e.creator != Some(helm)));
        assert_eq!(s.world.objects.len(), before + 1, "only the bag is new");
        assert_eq!(
            s.world
                .objects
                .values()
                .filter(|o| o.kind == ObjectKind::Item)
                .count(),
            1
        );
        let bags: Vec<_> = s
            .world
            .objects
            .values()
            .filter(|o| o.inventory.contains(&helm))
            .collect();
        assert_eq!(bags.len(), 1);
        let bag = bags[0];
        assert_eq!(bag.kind, ObjectKind::Placeable);
        assert_eq!(bag.script.as_deref(), Some("placeable_core"));
        assert!(bag.interactive);
        assert_eq!(bag.inventory, vec![helm]);
        let bag = bag.id;
        let events: Vec<_> = s
            .world
            .trace
            .entries
            .iter()
            .filter_map(|e| match e {
                crate::trace::TraceEntry::EventDelivered { target, ty, .. } => Some((*target, *ty)),
                _ => None,
            })
            .collect();
        assert!(
            events.contains(&(hero, 12)),
            "UNEQUIP is synchronous on the wearer"
        );
        let unequipped = s.world.trace.entries.iter().position(|e| matches!(
            e, crate::trace::TraceEntry::EventDelivered { target, ty: 12, .. } if *target == hero
        )).unwrap();
        let lost = s.world.trace.entries.iter().position(|e| matches!(
            e, crate::trace::TraceEntry::EventQueued { target, ty: 7, .. } if *target == hero
        )).unwrap();
        assert!(
            unequipped < lost,
            "the wearer hears UNEQUIP before inventory loss is queued"
        );
        let removed = s
            .world
            .queue
            .values()
            .find(|q| q.target == hero && q.event.ty == 7)
            .unwrap();
        assert_eq!(removed.event.creator, Some(hero));
        assert_eq!(removed.event.objects, vec![helm]);
        assert_eq!(removed.event.ints, vec![1]);
        assert!(
            !drop_from_screen(&mut s, helm),
            "cannot drop the bag's item twice"
        );
        s.open_container(bag);
        assert!(s.take_from_container(helm));
        assert_eq!(s.world.get(hero).unwrap().inventory, vec![helm]);
        assert!(s.world.get(hero).unwrap().equipment.is_empty());
    }

    #[test]
    fn dropping_equipped_helm_places_bag_on_nonzero_floor_above_or_below_feet() {
        for feet_z in [3.75, 1.25] {
            let (mut s, area, hero, helm) = equipped_helm();
            s.world.get_mut(hero).unwrap().position[2] = feet_z;
            assert!(drop_from_screen(&mut s, helm));
            let bag = s
                .world
                .objects
                .values()
                .find(|o| o.inventory.contains(&helm))
                .unwrap();
            assert_eq!(bag.position, [0.75, 1.25, 2.5]);
            assert_eq!(bag.area, Some(area));
            assert_eq!(bag.facing, 43.0);
            assert_eq!(bag.appearance, DROPPED_ITEM_APPEARANCE);
            assert_eq!(bag.fields.get("Appearance"), Some(&DROPPED_ITEM_APPEARANCE));
            assert_eq!(s.world.get(helm).unwrap().position, bag.position);
            assert!(
                !s.world
                    .get(hero)
                    .unwrap()
                    .fields
                    .contains_key(BODY_BAG_FIELD)
            );
        }
    }

    #[test]
    fn dropping_equipped_helm_refuses_locked_plot_foreign_and_da2_items() {
        for refusal in ["plot", "locked", "disabled", "foreign", "no_area", "da2"] {
            let (mut s, _, hero, helm) = equipped_helm();
            match refusal {
                "plot" => s.world.get_mut(helm).unwrap().plot = true,
                "locked" => {
                    s.world
                        .get_mut(helm)
                        .unwrap()
                        .fields
                        .insert("Irremovable".into(), 1);
                }
                "disabled" => {
                    s.world
                        .get_mut(hero)
                        .unwrap()
                        .fields
                        .insert("CanChangeEquipment".into(), 0);
                }
                "foreign" => s.world.hero = None,
                "no_area" => s.world.get_mut(hero).unwrap().area = None,
                "da2" => s.game = thedas_vfs::Game::DragonAge2,
                _ => unreachable!(),
            }
            let before = serde_json::to_value(&s.world).unwrap();
            assert!(!drop_from_screen(&mut s, helm), "{refusal}");
            // The apply journal records rejected input; ownership and objects do not change.
            assert_eq!(
                serde_json::to_value(&s.world.objects).unwrap(),
                before["objects"],
                "{refusal}"
            );
        }
    }

    #[test]
    fn dropping_equipped_helm_from_another_member_clears_inactive_equipment_too() {
        let (mut s, area, hero, helm) = equipped_helm();
        let ally = s.world.spawn(ObjectKind::Creature, "ally");
        let h = s.world.get_mut(hero).unwrap();
        h.followers.push(ally);
        h.equipment.clear();
        h.inventory.clear();
        let a = s.world.get_mut(ally).unwrap();
        a.area = Some(area);
        a.inventory.push(helm);
        a.other_set.insert(5, helm);
        assert!(drop_from_screen(&mut s, helm));
        let a = s.world.get(ally).unwrap();
        assert!(a.inventory.is_empty() && a.equipment.is_empty() && a.other_set.is_empty());
        assert!(
            s.world
                .queue
                .values()
                .any(|q| q.target == ally && q.event.ty == 7 && q.event.objects == vec![helm])
        );
        let bag = s
            .world
            .objects
            .values()
            .find(|o| o.inventory.contains(&helm))
            .unwrap();
        assert_eq!(bag.position, [0.75, 1.25, 2.5]);
    }
}

#[cfg(test)]
mod screen_tests {
    use std::sync::Arc;

    use thedas_formats::gda::Cell;
    use thedas_script::EngineTable;
    use thedas_vfs::Vfs;

    use super::*;

    #[test]
    fn the_weapon_set_button_switches_the_controlled_members_set() {
        let mut s = Sim::new(
            Arc::new(Vfs::new()),
            Arc::new(EngineTable::parse(b"").unwrap()),
            1,
        );
        let hero = s.world.spawn(ObjectKind::Creature, "hero");
        s.world.hero = Some(hero);
        let (sword, bow) = (
            s.world.spawn(ObjectKind::Item, "sword"),
            s.world.spawn(ObjectKind::Item, "bow"),
        );
        let h = s.world.get_mut(hero).unwrap();
        h.equipment.insert(0, sword);
        h.other_set.insert(0, bow);
        let set = |s: &Sim| {
            s.world
                .get(hero)
                .unwrap()
                .fields
                .get("ActiveWeaponSet")
                .copied()
        };
        // The button sends the set it switches to.
        assert!(s.gui_message(MSG_SWITCH_WEAPON_SET, &["1".into()]));
        assert_eq!(set(&s), Some(1));
        assert_eq!(s.world.get(hero).unwrap().equipment.get(&0), Some(&bow));
        // The key sends -1: the other set.
        assert!(s.gui_message(MSG_SWITCH_WEAPON_SET, &["-1".into()]));
        assert_eq!(set(&s), Some(0));
        assert_eq!(s.world.get(hero).unwrap().equipment.get(&0), Some(&sword));
        assert!(!s.gui_message(MSG_SWITCH_WEAPON_SET, &[]));
    }

    #[test]
    fn stackable_items_fill_matching_stacks_up_to_their_maximum() {
        let mut s = Sim::new(
            Arc::new(Vfs::new()),
            Arc::new(EngineTable::parse(b"").unwrap()),
            1,
        );
        // Poultices (base item 25) stack to 3 here; swords (2) do not stack.
        s.world.tables().insert(
            "bitm_base",
            &["#39974122"],
            vec![(25, vec![Cell::Int(3)]), (2, vec![Cell::Int(1)])],
        );
        let hero = s.world.spawn(ObjectKind::Creature, "hero");
        let item = |s: &mut Sim, tag: &str, base: i32, count: i32| {
            let i = s.world.spawn(ObjectKind::Item, tag);
            let f = &mut s.world.get_mut(i).unwrap().fields;
            f.insert("BaseItem".into(), base);
            f.insert("StackSize".into(), count);
            i
        };
        let first = item(&mut s, "poultice", 25, 1);
        assert!(s.give_item(first, hero));
        for _ in 0..2 {
            let more = item(&mut s, "poultice", 25, 1);
            assert!(s.give_item(more, hero));
            assert!(s.world.get(more).is_none(), "merged and gone");
        }
        assert_eq!(s.world.get(hero).unwrap().inventory, vec![first]);
        assert_eq!(s.stack_count(first), 3);
        // A full stack: two more start a new one.
        let extra = item(&mut s, "poultice", 25, 2);
        assert!(s.give_item(extra, hero));
        assert_eq!(s.world.get(hero).unwrap().inventory, vec![first, extra]);
        assert_eq!(s.stack_count(extra), 2);
        // A different tag, or a different material, does not merge.
        let other = item(&mut s, "lyrium", 25, 1);
        let rusty = item(&mut s, "poultice", 25, 1);
        s.world
            .get_mut(rusty)
            .unwrap()
            .fields
            .insert("Material".into(), 9);
        let (sword, sword2) = (item(&mut s, "sword", 2, 1), item(&mut s, "sword", 2, 1));
        for i in [other, rusty, sword, sword2] {
            assert!(s.give_item(i, hero));
        }
        assert_eq!(s.world.get(hero).unwrap().inventory.len(), 6);
        assert_eq!(s.stack_count(extra), 2);
    }

    /// A hero and a follower, a sword and a spare sword (base type 2, main hand: slot 0),
    /// and a pelt that fits no slot.
    fn party() -> (Sim, ObjectId, ObjectId, [ObjectId; 3]) {
        let table = EngineTable::parse(
            b"int INVENTORY_SLOT_INVALID = 255;\nint EVENT_TYPE_TRAINING_ITEM_UNEQUIPPED = 1070;\n",
        )
        .unwrap();
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

    fn equipment_flags_sim() -> (Sim, ObjectId, ObjectId, [ObjectId; 3]) {
        let (mut s, hero, ally, items) = party();
        s.table = Arc::new(
            EngineTable::parse(
                b"\
            int INVENTORY_SLOT_INVALID = 255;\n\
            void SetItemIrremovable(object o, int b) = 375;\n\
            int IsItemIrremovable(object o) = 376;\n\
            void SetCanChangeEquipment(object o, int b) = 391;\n\
            int EquipItem(object o, object i, int s = 255, int w = -1) = 392;\n\
            int UnequipItem(object o, object i) = 393;\n",
            )
            .unwrap(),
        );
        (s, hero, ally, items)
    }

    fn equipment_flags_call(
        s: &mut Sim,
        name: &str,
        args: &[thedas_script::vm::Value],
    ) -> Option<thedas_script::vm::Value> {
        let f = s.table.function_by_name(name).unwrap().clone();
        let mut h = crate::sim::ScriptHost {
            sim: s,
            event: None,
            script: "test".into(),
        };
        crate::engine::call(&mut h, OBJECT_INVALID, &f, args)
            .unwrap()
            .unwrap()
    }

    fn two_hander_party() -> (Sim, ObjectId, ObjectId, ObjectId) {
        let (mut s, hero, _, items) = equipment_flags_sim();
        s.table = Arc::new(
            EngineTable::parse(
                b"int INVENTORY_SLOT_INVALID = 255;\n\
              int EVENT_TYPE_EQUIP = 11;\n\
              int EVENT_TYPE_UNEQUIP = 12;\n\
              int EVENT_TYPE_TRAINING_ITEM_UNEQUIPPED = 1070;\n\
              int EquipItem(object o, object i, int s = 255, int w = -1) = 392;\n",
            )
            .unwrap(),
        );
        s.world.tables().insert(
            "bitm_base",
            &["EquippableSlots"],
            vec![(2, vec![Cell::Int(3)]), (5, vec![Cell::Int(1)])],
        );
        let (dagger, greatsword) = (items[0], items[1]);
        s.world
            .get_mut(greatsword)
            .unwrap()
            .fields
            .insert("BaseItem".into(), 5);
        let h = s.world.get_mut(hero).unwrap();
        h.equipment.clear();
        h.equipment.insert(1, dagger);
        h.script = Some("test".into());
        h.effects.push(crate::world::Effect {
            creator: Some(dagger),
            ..Default::default()
        });
        s.world.trace.keep = true;
        (s, hero, greatsword, dagger)
    }

    #[test]
    fn two_hander_gui_equip_unequips_off_hand_before_equip_and_removes_its_effects() {
        let (mut s, hero, greatsword, dagger) = two_hander_party();
        s.world.training.mode = 1;
        s.world.training.expected = 1070;
        s.world.training.arg = 1;
        assert!(s.gui_message(MSG_EQUIP, &[greatsword.to_string(), "0".into()]));
        let h = s.world.get(hero).unwrap();
        assert_eq!(h.equipment.get(&0), Some(&greatsword));
        assert!(!h.equipment.contains_key(&1));
        assert!(h.inventory.contains(&dagger));
        assert!(!h.effects.iter().any(|e| e.creator == Some(dagger)));
        assert_eq!(s.world.training.expected, 0);
        let events: Vec<_> = s
            .world
            .trace
            .entries
            .iter()
            .filter_map(|e| match e {
                crate::trace::TraceEntry::EventDelivered { target, ty, .. }
                    if *target == hero && matches!(ty, 11 | 12) =>
                {
                    Some(*ty)
                }
                _ => None,
            })
            .collect();
        assert_eq!(events, [12, 11]);
    }

    #[test]
    fn two_hander_gui_refuses_off_hand_drop_without_mutating_equipment() {
        let (mut s, hero, greatsword, dagger) = two_hander_party();
        assert!(s.equip_item(hero, greatsword, 0));
        assert!(s.equip_free_slots(hero).is_empty());
        let before = s.world.get(hero).unwrap().clone();
        assert!(!s.gui_message(MSG_EQUIP, &[dagger.to_string(), "1".into()]));
        let after = s.world.get(hero).unwrap();
        assert_eq!(after.equipment, before.equipment);
        assert_eq!(after.inventory, before.inventory);
        assert_eq!(after.effects, before.effects);
        // The item can still be equipped in the main hand, freeing the off hand.
        assert!(s.equip_item(hero, dagger, 255));
        let spare = s.world.spawn(ObjectKind::Item, "second_dagger");
        s.world
            .get_mut(spare)
            .unwrap()
            .fields
            .insert("BaseItem".into(), 2);
        s.world.get_mut(hero).unwrap().inventory.push(spare);
        assert!(s.equip_item(hero, spare, 1));
        assert_eq!(s.world.get(hero).unwrap().equipment.get(&0), Some(&dagger));
    }

    #[test]
    fn two_hander_script_explicit_and_default_slots_use_the_shared_hook() {
        use thedas_script::vm::Value::{Int, Object};
        for slot in [0, 255] {
            let (mut s, hero, greatsword, dagger) = two_hander_party();
            assert_eq!(
                equipment_flags_call(
                    &mut s,
                    "EquipItem",
                    &[Object(hero), Object(greatsword), Int(slot), Int(-1)]
                ),
                Some(Int(1))
            );
            assert!(!s.world.get(hero).unwrap().equipment.contains_key(&1));
            // The native off-hand insertion takes off the main-only weapon.
            // The inventory screen's drop refusal is a separate restriction.
            assert_eq!(
                equipment_flags_call(
                    &mut s,
                    "EquipItem",
                    &[Object(hero), Object(dagger), Int(1), Int(-1)]
                ),
                Some(Int(1))
            );
            let h = s.world.get(hero).unwrap();
            assert!(!h.equipment.contains_key(&0));
            assert_eq!(h.equipment.get(&1), Some(&dagger));
        }
    }

    #[test]
    fn two_hander_player_preserves_locked_off_hand_and_da2_behavior() {
        let (mut s, hero, greatsword, dagger) = two_hander_party();
        s.world
            .get_mut(dagger)
            .unwrap()
            .fields
            .insert("Irremovable".into(), 1);
        assert!(!s.equip_item(hero, greatsword, 0));
        assert_eq!(s.world.get(hero).unwrap().equipment.get(&1), Some(&dagger));
        s.game = thedas_vfs::Game::DragonAge2;
        assert!(s.equip_item(hero, greatsword, 0));
        assert_eq!(s.world.get(hero).unwrap().equipment.get(&1), Some(&dagger));
    }

    #[test]
    fn equipment_flags_native_round_trip_is_independent_of_plot_and_normalizes_booleans() {
        use thedas_script::vm::Value::{Int, Object};
        let (mut s, hero, _, items) = equipment_flags_sim();
        s.world.get_mut(items[0]).unwrap().plot = true;
        assert_eq!(
            equipment_flags_call(&mut s, "IsItemIrremovable", &[Object(items[0])]),
            Some(Int(0))
        );
        for (value, expected) in [(-3, 1), (0, 0), (19, 1)] {
            equipment_flags_call(
                &mut s,
                "SetItemIrremovable",
                &[Object(items[0]), Int(value)],
            );
            assert_eq!(
                equipment_flags_call(&mut s, "IsItemIrremovable", &[Object(items[0])]),
                Some(Int(expected))
            );
            assert!(s.world.get(items[0]).unwrap().plot);
        }
        equipment_flags_call(&mut s, "SetCanChangeEquipment", &[Object(hero), Int(0)]);
        assert!(!s.can_change_equipment(hero));
        equipment_flags_call(&mut s, "SetCanChangeEquipment", &[Object(hero), Int(-1)]);
        assert!(s.can_change_equipment(hero));
        let saved: crate::World =
            serde_json::from_slice(&serde_json::to_vec(&s.world).unwrap()).unwrap();
        assert_eq!(
            saved.get(items[0]).unwrap().fields.get("Irremovable"),
            Some(&1)
        );
        assert_eq!(
            saved.get(hero).unwrap().fields.get("CanChangeEquipment"),
            Some(&1)
        );
    }

    #[test]
    fn equipment_flags_locked_occupant_blocks_script_and_player_replacement() {
        use thedas_script::vm::Value::{Int, Object};
        let (mut s, hero, _, items) = equipment_flags_sim();
        equipment_flags_call(&mut s, "SetItemIrremovable", &[Object(items[0]), Int(1)]);
        assert!(!s.equip_item(hero, items[1], 0));
        assert!(!s.unequip_item(hero, items[0]));
        assert_eq!(
            equipment_flags_call(
                &mut s,
                "EquipItem",
                &[Object(hero), Object(items[1]), Int(0), Int(-1)]
            ),
            Some(Int(0))
        );
        assert_eq!(
            s.world.get(hero).unwrap().equipment.get(&0),
            Some(&items[0])
        );
        equipment_flags_call(&mut s, "SetItemIrremovable", &[Object(items[0]), Int(0)]);
        assert!(s.equip_item(hero, items[1], 0));
    }

    #[test]
    fn equipment_flags_player_cannot_take_locked_gear_from_another_member_or_change_disabled_gear()
    {
        use thedas_script::vm::Value::{Int, Object};
        let (mut s, hero, ally, items) = equipment_flags_sim();
        equipment_flags_call(&mut s, "SetItemIrremovable", &[Object(items[0]), Int(1)]);
        assert!(!s.equip_item(ally, items[0], 0));
        equipment_flags_call(&mut s, "SetItemIrremovable", &[Object(items[0]), Int(0)]);
        equipment_flags_call(&mut s, "SetCanChangeEquipment", &[Object(hero), Int(0)]);
        assert!(!s.equip_item(ally, items[0], 0));
        assert!(!s.equip_item(hero, items[1], 0));
        assert!(!s.unequip_item(hero, items[0]));
        // The native unequip path performs the transfer directly, independently
        // of the inventory screen's access and item-removal controls.
        equipment_flags_call(&mut s, "SetItemIrremovable", &[Object(items[0]), Int(1)]);
        assert_eq!(
            equipment_flags_call(&mut s, "UnequipItem", &[Object(hero), Object(items[0])]),
            Some(Int(1))
        );
    }

    #[test]
    fn equipment_flags_cover_inactive_equipment_and_leave_invalid_kinds_alone() {
        use thedas_script::vm::Value::{Int, Object};
        let (mut s, hero, _, items) = equipment_flags_sim();
        let h = s.world.get_mut(hero).unwrap();
        h.equipment.clear();
        h.other_set.insert(0, items[0]);
        equipment_flags_call(&mut s, "SetItemIrremovable", &[Object(items[0]), Int(1)]);
        assert_eq!(
            equipment_flags_call(
                &mut s,
                "EquipItem",
                &[Object(hero), Object(items[1]), Int(0), Int(1)]
            ),
            Some(Int(0))
        );
        for id in [hero, OBJECT_INVALID] {
            equipment_flags_call(&mut s, "SetItemIrremovable", &[Object(id), Int(1)]);
            assert_eq!(
                equipment_flags_call(&mut s, "IsItemIrremovable", &[Object(id)]),
                Some(Int(0))
            );
        }
        equipment_flags_call(&mut s, "SetCanChangeEquipment", &[Object(items[0]), Int(0)]);
        assert!(
            !s.world
                .get(items[0])
                .unwrap()
                .fields
                .contains_key("CanChangeEquipment")
        );
    }

    #[test]
    fn equipment_flags_da2_dispatch_and_equipment_access_are_unchanged() {
        use thedas_script::vm::Value::{Int, Object};
        let (mut s, hero, _, items) = equipment_flags_sim();
        s.game = thedas_vfs::Game::DragonAge2;
        for name in [
            "SetItemIrremovable",
            "IsItemIrremovable",
            "SetCanChangeEquipment",
        ] {
            let f = s.table.function_by_name(name).unwrap().clone();
            let mut h = crate::sim::ScriptHost {
                sim: &mut s,
                event: None,
                script: "test".into(),
            };
            assert!(crate::engine::call(&mut h, hero, &f, &[Object(items[0]), Int(1)]).is_none());
        }
        s.world
            .get_mut(hero)
            .unwrap()
            .fields
            .insert("CanChangeEquipment".into(), 0);
        s.world
            .get_mut(items[0])
            .unwrap()
            .fields
            .insert("Irremovable".into(), 1);
        assert!(s.equip_item(hero, items[1], 0));
    }

    #[test]
    fn recipes_the_party_receives_are_learned_not_kept() {
        let (mut s, hero, ally, _) = party();
        let recipe = |s: &mut Sim, id: i32| {
            let r = s.world.spawn(ObjectKind::Item, "gen_im_cft_hrb_101");
            let f = &mut s.world.get_mut(r).unwrap().fields;
            f.insert("BaseItem".into(), BASE_ITEM_RECIPE);
            f.insert("CraftingRecipeID".into(), id);
            r
        };
        let a = recipe(&mut s, 1);
        assert!(s.give_item(a, ally));
        assert!(s.world.get(a).is_none());
        assert!(!s.world.get(ally).unwrap().inventory.contains(&a));
        // Someone outside the party (a merchant) keeps it as an item.
        let merchant = s.world.spawn(ObjectKind::Creature, "merchant");
        let b = recipe(&mut s, 2);
        assert!(s.give_item(b, merchant));
        assert!(s.world.get(merchant).unwrap().inventory.contains(&b));
        // One already held (an older save) is learned on load.
        let c = recipe(&mut s, 3);
        s.world.get_mut(hero).unwrap().inventory.push(c);
        s.learn_held_recipes();
        assert!(s.world.get(c).is_none());
        assert_eq!(s.world.recipes.iter().copied().collect::<Vec<_>>(), [1, 3]);
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
        // Into the occupied main hand: the sword goes to the backpack. The item displaced
        // does not count as unequipped for the inventory lesson; taking one off does.
        let m = s.world.spawn(ObjectKind::Module, "module");
        s.world.module = Some(m);
        s.world.training.mode = 5;
        s.world.training.expected = 1070;
        s.world.training.arg = 0;
        s.world.trace.keep = true;
        assert!(s.equip_item(hero, spare, 0));
        assert_eq!(
            s.world.training.expected, 1070,
            "a swap leaves the step armed"
        );
        assert!(s.unequip_item(hero, spare));
        // Heard, and delivered to genev_tutorial at once (see `Sim::raise_training_event`).
        assert!(
            s.world
                .trace
                .entries
                .iter()
                .any(|e| matches!(e, crate::trace::TraceEntry::EventDelivered { ty: 1070, .. }))
        );
        assert_eq!(s.world.training.expected, 0);
        assert!(s.equip_item(hero, spare, 0));
        s.world.training = Default::default();
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

    /// A script answering `SetCanUseItem(OBJECT_SELF, answer)`.
    fn answers(answer: i32) -> thedas_script::ncs::Ncs {
        let mut b: Vec<u8> = vec![0x04, 0x03];
        b.extend(answer.to_be_bytes());
        b.extend([0x04, 0x06, 0, 0, 0, 0, 0x05, 0x00]);
        b.extend(813u16.to_be_bytes());
        b.extend([0x02, 0x20, 0x00]);
        let mut v = b"NCS V1.0B".to_vec();
        v.extend(((13 + b.len()) as u32).to_be_bytes());
        v.extend(b);
        thedas_script::ncs::Ncs::parse(&v).unwrap()
    }

    #[test]
    fn restriction_properties_ask_their_scripts_whether_a_creature_may_use_an_item() {
        let table = EngineTable::parse(
            b"int EVENT_TYPE_ITEM_ONTEST_USABLE = 70;\n\
              void SetCanUseItem(object oCreature, int value) = 813;\n",
        )
        .unwrap();
        let mut s = Sim::new(Arc::new(Vfs::new()), Arc::new(table), 1);
        s.world.tables().insert(
            "itemprps",
            &["#4fe5e973", "#75784cde"],
            vec![
                (1, vec![Cell::Int(1), Cell::String(String::new())]),
                (6027, vec![Cell::Int(3), Cell::String("ip_no".into())]),
                (6030, vec![Cell::Int(3), Cell::String("ip_yes".into())]),
                (6031, vec![Cell::Int(3), Cell::String(String::new())]),
            ],
        );
        s.add_script("ip_no", answers(0));
        s.add_script("ip_yes", answers(1));
        let who = s.world.spawn(ObjectKind::Creature, "who");
        let item = |s: &mut Sim, props: Vec<(i32, i32)>| {
            let i = s.world.spawn(ObjectKind::Item, "item");
            s.world.get_mut(i).unwrap().item_properties = Some(props);
            i
        };
        let plain = item(&mut s, vec![(1, 1)]);
        let refused = item(&mut s, vec![(6030, 0), (6027, 0)]);
        let allowed = item(&mut s, vec![(6030, 0)]);
        let no_script = item(&mut s, vec![(6031, 0)]);
        assert!(s.can_use_item(who, plain));
        assert!(!s.can_use_item(who, refused));
        assert!(s.can_use_item(who, allowed));
        // A restriction without a script leaves the cleared answer: refused.
        assert!(!s.can_use_item(who, no_script));
        assert!(!s.can_use_item(who, 9999));
    }

    #[test]
    fn wearing_applies_the_items_properties_and_tells_the_script() {
        let table = EngineTable::parse(
            b"int INVENTORY_SLOT_INVALID = 255;\nint EVENT_TYPE_EQUIP = 11;\n\
              int EVENT_TYPE_UNEQUIP = 12;\nint EFFECT_DURATION_TYPE_INSTANT = 1;\n\
              int EFFECT_DURATION_TYPE_PERMANENT = 3;\n",
        )
        .unwrap();
        let mut s = Sim::new(Arc::new(Vfs::new()), Arc::new(table), 1);
        let tables = s.world.tables();
        tables.insert(
            "bitm_base",
            &["EquippableSlots"],
            vec![(29, vec![Cell::Int(1 << 11)])],
        );
        // Increase Constitution: EFFECT_TYPE_MODIFY_PROPERTY on property 6, Float0 per
        // power; a restriction, which is not an effect.
        tables.insert(
            "itemprps",
            &[
                "#4fe5e973",
                "Effect",
                "Int0",
                "Int1",
                "Float0",
                "Float1",
                "#36001b03",
            ],
            vec![
                (
                    1005,
                    vec![
                        Cell::Int(1),
                        Cell::Int(1012),
                        Cell::Int(6),
                        Cell::Int(0),
                        Cell::Float(1.0),
                        Cell::Float(0.0),
                        Cell::String("Float0".into()),
                    ],
                ),
                (
                    10018,
                    vec![
                        Cell::Int(3),
                        Cell::Int(0),
                        Cell::Int(0),
                        Cell::Int(0),
                        Cell::Float(0.0),
                        Cell::Float(0.0),
                        Cell::String(String::new()),
                    ],
                ),
            ],
        );
        let hero = s.world.spawn(ObjectKind::Creature, "hero");
        s.world.hero = Some(hero);
        let amulet = s.world.spawn(ObjectKind::Item, "amulet");
        let o = s.world.get_mut(amulet).unwrap();
        o.template = "amulet".into();
        o.fields.insert("BaseItem".into(), 29);
        s.set_item_look(
            "amulet",
            crate::sim::ItemLook {
                properties: vec![(1005, 2), (10018, 1)],
                ..Default::default()
            },
        );
        s.world.get_mut(hero).unwrap().inventory.push(amulet);
        s.world.get_mut(hero).unwrap().script = Some("hero".into());
        s.world.trace.keep = true;
        assert!(s.equip_item(hero, amulet, 255));
        let worn = &s.world.get(hero).unwrap().effects;
        assert_eq!(worn.len(), 1);
        assert_eq!((worn[0].ty, worn[0].creator), (1012, Some(amulet)));
        assert_eq!((worn[0].ints[0], worn[0].floats[0]), (6, 2.0));
        // The wearer hears EQUIP (11) at once, inside the equip, not from the queue.
        let events = |s: &Sim| -> Vec<i32> {
            s.world
                .trace
                .entries
                .iter()
                .filter_map(|e| match e {
                    crate::trace::TraceEntry::EventDelivered { target, ty, .. }
                        if *target == hero =>
                    {
                        Some(*ty)
                    }
                    _ => None,
                })
                .collect()
        };
        assert_eq!(events(&s), vec![11]);
        assert!(!s.world.queue.values().any(|q| q.target == hero));
        // Changing a worn item's properties changes what it gives its wearer.
        assert!(s.remove_item_property(amulet, 1005));
        assert!(s.world.get(hero).unwrap().effects.is_empty());
        assert!(s.add_item_property(amulet, 1005, 3));
        let worn = &s.world.get(hero).unwrap().effects;
        assert_eq!((worn.len(), worn[0].floats[0]), (1, 3.0));
        assert!(s.unequip_item(hero, amulet));
        assert!(s.world.get(hero).unwrap().effects.is_empty());
        assert_eq!(events(&s), vec![11, 12]);
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

    fn bulk_sim() -> (Sim, ObjectId, ObjectId) {
        let table = EngineTable::parse(
            format!("{LDF}void MoveAllItems(object a, object b) = 350;\n").as_bytes(),
        )
        .unwrap();
        let mut s = Sim::new(Arc::new(Vfs::new()), Arc::new(table), 1);
        let from = s.world.spawn(ObjectKind::Creature, "from");
        let to = s.world.spawn(ObjectKind::Placeable, "to");
        (s, from, to)
    }

    fn bulk_call(s: &mut Sim, from: ObjectId, to: ObjectId) {
        let f = s.table.function_by_name("MoveAllItems").unwrap().clone();
        let mut h = crate::sim::ScriptHost {
            sim: s,
            event: None,
            script: "test".into(),
        };
        assert!(
            crate::engine::call(
                &mut h,
                from,
                &f,
                &[
                    thedas_script::vm::Value::Object(from),
                    thedas_script::vm::Value::Object(to)
                ]
            )
            .unwrap()
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn bulk_transfer_moves_backpack_items_in_order_and_keeps_both_equipment_sets_and_money() {
        let (mut s, from, to) = bulk_sim();
        let items: [_; 4] = std::array::from_fn(|_| s.world.spawn(ObjectKind::Item, "item"));
        let source = s.world.get_mut(from).unwrap();
        source.inventory = items.to_vec();
        source.equipment.insert(0, items[0]);
        source.other_set.insert(0, items[2]);
        source.money = 27;
        let f = &mut s.world.get_mut(items[1]).unwrap().fields;
        f.insert("Irremovable".into(), 1);
        f.insert("Droppable".into(), 0);
        s.world.get_mut(items[3]).unwrap().plot = true;
        bulk_call(&mut s, from, to);
        assert_eq!(
            s.world.get(from).unwrap().inventory,
            vec![items[0], items[2]]
        );
        assert_eq!(s.world.get(to).unwrap().inventory, vec![items[1], items[3]]);
        let source = s.world.get(from).unwrap();
        assert_eq!(source.equipment.get(&0), Some(&items[0]));
        assert_eq!(source.other_set.get(&0), Some(&items[2]));
        assert_eq!(source.money, 27);
        assert_eq!(s.world.get(to).unwrap().money, 0);
        assert_eq!(
            s.world.queue.len(),
            4,
            "each transfer tells both inventories"
        );
    }

    #[test]
    fn bulk_transfer_merges_stacks_and_leaves_items_that_exceed_capacity() {
        use thedas_formats::gda::Cell;
        let (mut s, from, _) = bulk_sim();
        let to = s.world.spawn(ObjectKind::Creature, "to");
        s.world
            .get_mut(to)
            .unwrap()
            .fields
            .insert("MaxInventorySize".into(), 1);
        s.world
            .tables()
            .insert("bitm_base", &["#39974122"], vec![(25, vec![Cell::Int(3)])]);
        let existing = s.world.spawn(ObjectKind::Item, "stack");
        let incoming = s.world.spawn(ObjectKind::Item, "stack");
        let blocked = s.world.spawn(ObjectKind::Item, "blocked");
        for id in [existing, incoming] {
            let f = &mut s.world.get_mut(id).unwrap().fields;
            f.insert("BaseItem".into(), 25);
            f.insert("StackSize".into(), 1);
        }
        s.world.get_mut(from).unwrap().inventory = vec![blocked, incoming];
        s.world.get_mut(to).unwrap().inventory = vec![existing];
        bulk_call(&mut s, from, to);
        assert_eq!(s.world.get(from).unwrap().inventory, vec![blocked]);
        assert_eq!(s.world.get(to).unwrap().inventory, vec![existing]);
        assert!(s.world.get(incoming).is_none());
        assert_eq!(s.stack_count(existing), 2);
        assert_eq!(s.world.queue.len(), 2);
    }

    #[test]
    fn bulk_transfer_routes_plot_items_and_coin_past_creature_backpack_capacity() {
        let (mut s, from, _) = bulk_sim();
        let hero = s.world.spawn(ObjectKind::Creature, "hero");
        s.world.hero = Some(hero);
        s.world
            .get_mut(hero)
            .unwrap()
            .fields
            .insert("MaxInventorySize".into(), 0);
        let plot = s.world.spawn(ObjectKind::Item, "plot");
        s.world.get_mut(plot).unwrap().plot = true;
        let coin = s.world.spawn(ObjectKind::Item, "coin");
        let f = &mut s.world.get_mut(coin).unwrap().fields;
        f.insert("BaseItem".into(), BASE_ITEM_COPPER);
        f.insert("StackSize".into(), 19);
        let blocked = s.world.spawn(ObjectKind::Item, "ordinary");
        s.world.get_mut(from).unwrap().inventory = vec![plot, coin, blocked];
        bulk_call(&mut s, from, hero);
        assert_eq!(s.world.get(from).unwrap().inventory, vec![blocked]);
        assert_eq!(s.world.get(hero).unwrap().inventory, vec![plot]);
        assert_eq!(s.world.get(hero).unwrap().money, 19);
        assert!(s.world.get(coin).is_none());
    }

    #[test]
    fn bulk_transfer_rejects_invalid_possessors_and_self_transfer_without_events() {
        let (mut s, from, to) = bulk_sim();
        let item = s.world.spawn(ObjectKind::Item, "item");
        s.world.get_mut(from).unwrap().inventory = vec![item];
        let invalid = s.world.spawn(ObjectKind::Trigger, "not_an_inventory");
        for (source, target) in [
            (from, from),
            (from, invalid),
            (invalid, to),
            (from, OBJECT_INVALID),
            (OBJECT_INVALID, to),
        ] {
            bulk_call(&mut s, source, target);
        }
        assert_eq!(s.world.get(from).unwrap().inventory, vec![item]);
        assert!(s.world.get(to).unwrap().inventory.is_empty());
        assert!(s.world.queue.is_empty());
    }

    #[test]
    fn bulk_transfer_da2_dispatch_remains_unchanged() {
        let (mut s, from, to) = bulk_sim();
        s.game = thedas_vfs::Game::DragonAge2;
        let f = s.table.function_by_name("MoveAllItems").unwrap().clone();
        let mut h = crate::sim::ScriptHost {
            sim: &mut s,
            event: None,
            script: "test".into(),
        };
        assert!(
            crate::engine::call(
                &mut h,
                from,
                &f,
                &[
                    thedas_script::vm::Value::Object(from),
                    thedas_script::vm::Value::Object(to)
                ]
            )
            .is_none()
        );
    }

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
        assert_eq!(s.world.get(hero).unwrap().money, 0);
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

    #[test]
    fn coin_the_hero_picks_up_becomes_money() {
        let table = Arc::new(EngineTable::parse(LDF.as_bytes()).unwrap());
        let mut s = Sim::new(Arc::new(Vfs::new()), table, 1);
        let hero = s.world.spawn(ObjectKind::Creature, "hero");
        s.world.hero = Some(hero);
        let ally = s.world.spawn(ObjectKind::Creature, "ally");
        let chest = s.world.spawn(ObjectKind::Placeable, "chest");
        let coin = |s: &mut Sim, n: i32| {
            let c = s.world.spawn(ObjectKind::Item, "gen_im_copper");
            let f = &mut s.world.get_mut(c).unwrap().fields;
            f.insert("BaseItem".into(), 17);
            f.insert("StackSize".into(), n);
            c
        };
        let (a, b) = (coin(&mut s, 37), coin(&mut s, 5));
        let gem = s.world.spawn(ObjectKind::Item, "gem");
        s.world.get_mut(chest).unwrap().inventory = vec![a, gem];
        assert_eq!(s.loot(chest, hero), vec![a, gem]);
        let h = s.world.get(hero).unwrap();
        assert_eq!(h.money, 37);
        assert_eq!(h.inventory, vec![gem]);
        assert!(s.world.get(a).is_none());
        assert!(s.world.get(chest).unwrap().inventory.is_empty());
        // Anyone else keeps coin as an item.
        assert!(s.give_item(b, ally));
        assert_eq!(s.world.get(ally).unwrap().inventory, vec![b]);
        assert_eq!(s.world.get(ally).unwrap().money, 0);
    }

    #[test]
    fn da2_currency_conversion_still_tells_the_source_inventory() {
        for game in [thedas_vfs::Game::DragonAge2, thedas_vfs::Game::Origins] {
            let table = Arc::new(EngineTable::parse(LDF.as_bytes()).unwrap());
            let mut s = Sim::new(Arc::new(Vfs::new()), table, 1);
            s.game = game;
            let hero = s.world.spawn(ObjectKind::Creature, "hero");
            s.world.hero = Some(hero);
            let chest = s.world.spawn(ObjectKind::Placeable, "chest");
            let coin = s.world.spawn(ObjectKind::Item, "coin");
            let fields = &mut s.world.get_mut(coin).unwrap().fields;
            fields.insert("BaseItem".into(), 17);
            fields.insert("StackSize".into(), 37);
            s.world.get_mut(chest).unwrap().inventory.push(coin);
            assert_eq!(s.loot(chest, hero), [coin]);
            assert!(s.loot(chest, hero).is_empty());
            assert_eq!(s.world.get(hero).unwrap().money, 37);
            assert!(s.world.get(hero).unwrap().inventory.is_empty());
            assert!(s.world.get(coin).is_none());
            let events: Vec<_> = s.world.queue.values().collect();
            if game == thedas_vfs::Game::DragonAge2 {
                assert_eq!(events.len(), 1);
                let event = &events[0];
                assert_eq!(event.target, chest);
                assert_eq!(event.event.ty, 7);
                assert_eq!(event.event.creator, Some(hero));
                assert_eq!(event.event.objects, [coin]);
                assert_eq!(event.event.ints, [1]);
            } else {
                assert!(events.is_empty());
            }
        }
    }

    #[test]
    fn a_fallen_creatures_money_goes_into_its_bag_and_to_the_looter() {
        let table = Arc::new(EngineTable::parse(LDF.as_bytes()).unwrap());
        let mut s = Sim::new(Arc::new(Vfs::new()), table, 1);
        let hero = s.world.spawn(ObjectKind::Creature, "hero");
        let foe = s.world.spawn(ObjectKind::Creature, "foe");
        let sword = s.world.spawn(ObjectKind::Item, "sword");
        s.world.get_mut(hero).unwrap().money = 50;
        let f = s.world.get_mut(foe).unwrap();
        f.inventory = vec![sword];
        f.money = 181;
        // Nothing droppable, but money: a bag is still left.
        let bag = s.spawn_body_bag(foe, false).unwrap();
        assert_eq!(s.world.get(bag).unwrap().money, 181);
        assert!(s.world.get(bag).unwrap().inventory.is_empty());
        assert_eq!(s.world.get(foe).unwrap().money, 0);
        assert!(s.loot(bag, hero).is_empty());
        assert_eq!(s.world.get(hero).unwrap().money, 231);
        assert_eq!(s.world.get(bag).unwrap().money, 0);
        // A creature with neither leaves nothing.
        let poor = s.world.spawn(ObjectKind::Creature, "poor");
        assert_eq!(s.spawn_body_bag(poor, false), None);
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
        // Boots (1) scale; clothing (38) does not. The column is known only by its hash.
        t.insert(
            "bitm_base",
            &["ID", "Label", "#66f63962"],
            vec![
                (
                    1,
                    vec![Cell::Int(1), Cell::String("Boots".into()), Cell::Int(1)],
                ),
                (
                    38,
                    vec![Cell::Int(38), Cell::String("Clothing".into()), Cell::Int(0)],
                ),
            ],
        );
        s.world.tables = Arc::new(t);
        let foe = s.world.spawn(ObjectKind::Creature, "hurlock");
        let hero = s.world.spawn(ObjectKind::Creature, "hero");
        s.world.hero = Some(hero);
        let give = |s: &mut Sim, who, slot, base| {
            let i = s.world.spawn(ObjectKind::Item, "item");
            s.world.get_mut(i).unwrap().fields.extend([
                ("MaterialProgress".to_string(), 46),
                ("Material".to_string(), 76),
                ("BaseItem".to_string(), base),
            ]);
            s.world.get_mut(who).unwrap().equipment.insert(slot, i);
            i
        };
        let (boots, clothes, cloak, heros) = (
            give(&mut s, foe, 6, 1),
            give(&mut s, foe, 4, 38),
            give(&mut s, foe, 8, 1),
            give(&mut s, hero, 6, 1),
        );
        let material = |s: &Sim, i| s.world.get(i).unwrap().fields["Material"];
        // Level 7: tier 7 / 3 = 2, Material3.
        s.scale_equipped_items(foe, 7);
        assert_eq!(material(&s, boots), 8);
        assert_eq!(material(&s, clothes), 76, "clothing keeps its material");
        assert_eq!(material(&s, cloak), 76, "the cloak slot is not visited");
        s.scale_equipped_items(hero, 7);
        assert_eq!(material(&s, heros), 76, "the party is not scaled");
        s.scale_equipped_items(foe, 3);
        assert_eq!(material(&s, boots), 2);
        s.scale_equipped_items(foe, 24);
        assert_eq!(material(&s, boots), 2, "past tier 7 nothing changes");
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
