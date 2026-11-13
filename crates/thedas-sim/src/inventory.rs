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

/// `itemprps.gda` columns whose names are not known: the property's kind (1 while worn, 2
/// on hit, 3 a restriction, 4 other) and the effect parameter its power scales (`Float0`).
const ITEMPRPS_KIND: u32 = 0x4fe5_e973;
const ITEMPRPS_POWER_PARAM: u32 = 0x3600_1b03;
/// An item property kind that holds while the item is worn.
const ITEMPRPS_WORN: i64 = 1;
/// `Int0` of a property that modifies every attribute (`Increase All Attributes`).
const ALL_ATTRIBUTES: i32 = -1000;

/// The creature field keeping the body bag it left (`GetCreatureBodyBag`; the creature's
/// +0x790 in DAOrigins.exe).
pub const BODY_BAG_FIELD: &str = "BodyBag";

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
    /// The effects an item's properties put on whoever wears it: for each of its template's
    /// properties of the worn kind, `itemprps.gda`'s effect with its `Int0`/`Int1`,
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
                    .and_then(|o| o.effects.iter().find(|x| **x == e).cloned());
                if let Some(held) = held {
                    self.remove_effect(who, &held);
                }
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
        if !crate::engine::party_of(self).contains(&who) {
            return false;
        }
        self.unequip(who, item)
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
                self.item_slots(item)
                    .into_iter()
                    .find(|s| !taken.contains_key(s))
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
        let bag = match old_bag {
            Some(b) => b,
            None => self.world.spawn(ObjectKind::Placeable, "gen_ip_bodybag"),
        };
        if let Some(b) = self.world.get_mut(bag) {
            b.position = pos;
            b.area = area;
            b.template = "gen_ip_bodybag".into();
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
mod screen_tests {
    use std::sync::Arc;

    use thedas_formats::gda::Cell;
    use thedas_script::EngineTable;
    use thedas_vfs::Vfs;

    use super::*;

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
