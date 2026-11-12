//! The quickbar: the abilities a party member can use at a click, and how they are aimed.
//!
//! Each creature's bar has a fixed layout ([`crate::world::Object::quickslots`], saved):
//! scripts fill slots with `SetQuickslot`, the player drags icons between slots (the
//! movie's `.SetQuickslot` message), and the engine places what the bar does not have yet.
//! Provisional: the engine is taken to put a party member's usable abilities (talents,
//! spells and skills that are activated or sustained, `ABI_base` `UseType` 4 or 2) in the
//! free slots, lowest ability first, then the party's quick items, as a new game's
//! quickbar shows them; each once, so what the player takes off stays off; and to drop an
//! item whose last one is gone and an ability the creature lost. The executable's own rule
//! is not traced.

use thedas_formats::gda::Cell;
use thedas_script::vm::ObjectId;

use crate::sim::Sim;
use crate::world::Quickslot;

/// The slots the quickbar has (`GUI.SelectedChar.Stats.QuickSlots.0` to `.39`).
pub const QUICKSLOTS: i32 = 40;

/// The message quickbar.gfx sends (after the selected character's token) when the player
/// puts something in a slot or takes it out: creature, slot, ability (0 empties it), the
/// item's tag and its template.
pub const MSG_SET_QUICKSLOT: &str = ".SetQuickslot";

/// What a slot holds, as [`crate::world::Object::quick_placed`] records it.
pub fn quick_key(q: &Quickslot) -> String {
    if q.item.is_empty() {
        format!("ability:{}", q.ability)
    } else {
        format!("item:{}", q.item.to_ascii_lowercase())
    }
}

/// `ABI_base` `UseType`s an ability can be used with from the quickbar: sustained (2) and
/// activated (4).
const USE_SUSTAINED: i32 = 2;
const USE_ACTIVATED: i32 = 4;
/// `ABI_base` `AbilityType`s of a creature's own abilities: talent, spell, skill.
const OWN_ABILITY_TYPES: [i32; 3] = [1, 2, 3];
/// `ABI_base` `TargetType` bit for the user itself. Provisional: the bits are read from
/// the table's contents (1 self, 2 friend, 4 hostile, the rest places).
pub const TARGET_SELF: i32 = 1;
/// `ABI_base` `TargetType` bit for a place (fireball, cones, the ogre's hurl). Provisional.
pub const TARGET_PLACE: i32 = 32;

/// The ground an ability covers (`ABI_base` `aoe_type`, `aoe_param1`, `aoe_param2`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AbilityArea {
    /// A single target.
    None,
    /// A circle of this radius (metres), type 1.
    Sphere(f32),
    /// A cone of this angle (degrees) and length (metres) from the user, type 2.
    Cone(f32, f32),
}

impl Sim {
    /// Whether an ability's `ABI_base` icon is `hidden`: the class abilities (4022 Warrior,
    /// 4023 Mage, ...) and other bookkeeping abilities the screens never show. The quickbar
    /// leaves them off (its movie has no `hidden.dds` to draw). Provisional: the exe's own
    /// placement rule is not traced; the icon is the tell the data gives.
    pub fn ability_hidden(&self, ability: i32) -> bool {
        matches!(
            self.world
                .tables()
                .get(0, "ABI_base")
                .and_then(|t| t.cell("Icon", ability as i64).cloned()),
            Some(Cell::String(s)) if s.trim_end_matches('\0').trim().eq_ignore_ascii_case("hidden")
        )
    }

    pub(crate) fn abi_int(&self, ability: i32, column: &str) -> i32 {
        match self
            .world
            .tables()
            .get(0, "ABI_base")
            .and_then(|t| t.cell(column, ability as i64).cloned())
        {
            Some(Cell::Int(v)) => v as i32,
            Some(Cell::Float(v)) => v as i32,
            _ => 0,
        }
    }

    /// An ability's `ABI_base` `TargetType` (a bitmask; 0 needs no target).
    pub fn ability_target_type(&self, ability: i32) -> i32 {
        self.abi_int(ability, "TargetType")
    }

    /// Whether an ability is used on its user alone (no target to pick).
    pub fn ability_on_self(&self, ability: i32) -> bool {
        let t = self.ability_target_type(ability);
        t == 0 || t == TARGET_SELF
    }

    /// Whether an ability may be aimed at a place on the ground.
    pub fn ability_targets_place(&self, ability: i32) -> bool {
        self.ability_target_type(ability) & TARGET_PLACE != 0
    }

    /// The ground an ability covers. Provisional: types other than 1 and 2 are taken to
    /// cover no area.
    pub fn ability_area(&self, ability: i32) -> AbilityArea {
        let float = |c: &str| match self
            .world
            .tables()
            .get(0, "ABI_base")
            .and_then(|t| t.cell(c, ability as i64).cloned())
        {
            Some(Cell::Float(v)) => v as f32,
            Some(Cell::Int(v)) => v as f32,
            _ => 0.0,
        };
        match self.abi_int(ability, "aoe_type") {
            1 if float("aoe_param1") > 0.0 => AbilityArea::Sphere(float("aoe_param1")),
            2 if float("aoe_param2") > 0.0 => {
                AbilityArea::Cone(float("aoe_param1"), float("aoe_param2"))
            }
            _ => AbilityArea::None,
        }
    }

    /// How far from its target an ability can be used (metres): the `PrimaryRange` of the
    /// ranges.gda row its `ABI_base` `Range` names (`SpellRngTouch` 2.25, `SpellRngShrt` 8,
    /// `SpellRngMed` 25, `SpellRngLng` 35, `SpellRngXLong` 60). `None` for one used where its
    /// user stands (`SpellRngPers` and `SpellRngParty` are 0, and most talents name row 0).
    pub fn ability_range(&self, ability: i32) -> Option<f32> {
        let row = self.abi_int(ability, "Range");
        if row <= 0 {
            return None;
        }
        self.world
            .tables()
            .get(crate::combat::RANGES_TABLE, "")
            .and_then(|t| match t.cell("PrimaryRange", row as i64) {
                Some(Cell::Float(v)) => Some(*v as f32),
                Some(Cell::Int(v)) => Some(*v as f32),
                _ => None,
            })
            .filter(|r| r.is_finite() && *r > 0.0)
    }

    /// An ability's `ABI_base` `UseType` (2 sustained, 3 passive, 4 activated).
    pub fn ability_use_type(&self, ability: i32) -> i32 {
        self.abi_int(ability, "UseType")
    }

    /// The share of an ability's cooldown still to run for `who` (0 ready, 1 just used),
    /// against `ABI_base` `cooldown`. Provisional: an item's cooldown is keyed by its tag,
    /// which the quickbar passes.
    pub fn cooldown_left(&self, who: ObjectId, ability: i32, item: &str) -> f32 {
        let total = match self
            .world
            .tables()
            .get(0, "ABI_base")
            .and_then(|t| t.cell("cooldown", ability as i64).cloned())
        {
            Some(Cell::Float(v)) => v as f32,
            Some(Cell::Int(v)) => v as f32,
            _ => 0.0,
        };
        let key = crate::engine::cooldown_key(ability, item);
        let left = self
            .world
            .get(who)
            .and_then(|o| o.combat.cooldowns.get(&key))
            .map_or(0.0, |&until| {
                until.saturating_sub(self.world.time_ms) as f32 / 1000.0
            });
        if total <= 0.0 {
            return (left > 0.0) as u8 as f32;
        }
        (left / total).clamp(0.0, 1.0)
    }

    /// The items the party can use from the quickbar (potions, poultices, grenades: items with
    /// an activated ability), one entry per tag, in the order the pack holds them: the tag,
    /// the ability and how many there are. Provisional: grouped by tag.
    pub fn quick_items(&self) -> Vec<(String, i32, i32)> {
        let mut out: Vec<(String, i32, i32)> = Vec::new();
        for item in self.party_backpack() {
            let Some(o) = self.world.get(item) else {
                continue;
            };
            let ability = o.fields.get("AbilityID").copied().unwrap_or(0);
            if ability <= 0 || self.ability_use_type(ability) != USE_ACTIVATED {
                continue;
            }
            let count = o.fields.get("StackSize").copied().unwrap_or(1).max(1);
            match out.iter_mut().find(|q| q.0.eq_ignore_ascii_case(&o.tag)) {
                Some(q) => q.2 += count,
                None => out.push((o.tag.clone(), ability, count)),
            }
        }
        out
    }

    /// How many of the items tagged `tag` the party carries.
    pub fn item_count(&self, tag: &str) -> i32 {
        self.party_backpack()
            .into_iter()
            .filter_map(|i| self.world.get(i))
            .filter(|o| o.tag.eq_ignore_ascii_case(tag))
            .map(|o| o.fields.get("StackSize").copied().unwrap_or(1).max(1))
            .sum()
    }

    /// The item tagged `tag` that `who` would use: its own first, else one in the party's
    /// pack (the party shares its items).
    pub fn party_item(&self, who: ObjectId, tag: &str) -> Option<ObjectId> {
        let is = |i: &ObjectId| {
            self.world
                .get(*i)
                .is_some_and(|x| x.tag.eq_ignore_ascii_case(tag))
        };
        self.world
            .get(who)
            .and_then(|o| o.inventory.iter().copied().find(is))
            .or_else(|| self.party_backpack().into_iter().find(is))
    }

    /// Puts `ability` (from the item tagged `item`, if any) in `who`'s quickbar slot `slot`, or
    /// empties the slot when `ability` is 0: a script's `SetQuickslot` or the player's drag.
    /// `SetQuickslot`: a slot of −1 is the first slot nothing was set in, as script.ldf
    /// documents (the game's scripts pass −1 2000 times). Provisional: an ability or item
    /// already set in a slot is not set again in another.
    pub fn set_quickslot(&mut self, who: ObjectId, slot: i32, ability: i32, item: &str) {
        // A hidden ability (a class's, see [`Sim::ability_hidden`]) is not put on the bar:
        // the scripts add the class ability "to the quickbar" with the rest, and the
        // original's bar shows no gap for it. Provisional.
        if ability > 0 && item.is_empty() && self.ability_hidden(ability) {
            return;
        }
        let Some(o) = self.world.get_mut(who) else {
            return;
        };
        let slot = if slot == -1 && ability > 0 {
            let q = Quickslot {
                ability,
                item: item.to_string(),
            };
            if o.quickslots.values().any(|x| quick_key(x) == quick_key(&q)) {
                return;
            }
            match (0..QUICKSLOTS).find(|s| !o.quickslots.contains_key(s)) {
                Some(s) => s,
                None => return,
            }
        } else {
            slot
        };
        if !(0..QUICKSLOTS).contains(&slot) {
            return;
        }
        if ability > 0 {
            let q = Quickslot {
                ability,
                item: item.to_string(),
            };
            o.quick_placed.insert(quick_key(&q));
            o.quickslots.insert(slot, q);
        } else {
            o.quickslots.remove(&slot);
        }
    }

    /// The player put something in a quickbar slot or took it out (quickbar.gfx's
    /// `.SetQuickslot`). Returns whether `name` was that message.
    pub fn quickbar_message(&mut self, name: &str, args: &[String]) -> bool {
        if !name.ends_with(MSG_SET_QUICKSLOT) {
            return false;
        }
        let int = |i: usize| {
            args.get(i)
                .and_then(|a| a.trim().parse::<f64>().ok())
                .filter(|v| v.is_finite())
                .map(|v| v as i64)
        };
        let (Some(who), Some(slot)) = (int(0), int(1)) else {
            return true;
        };
        let ability = int(2).unwrap_or(0).clamp(0, i32::MAX as i64) as i32;
        let item = args.get(3).map(|s| s.trim()).unwrap_or_default();
        self.set_quickslot(who as ObjectId, slot as i32, ability, item);
        true
    }

    /// Settles the party's quickbars into their layout: an item whose last one is gone and
    /// an ability its owner lost leave the bar, and what a member can use that has never been
    /// on it takes a free slot, to stay there.
    pub fn settle_quickbars(&mut self) {
        for who in crate::engine::party_of(self) {
            let Some(o) = self.world.get(who) else {
                continue;
            };
            let gone: Vec<i32> = o
                .quickslots
                .iter()
                .filter(|(_, q)| {
                    if q.item.is_empty() {
                        !o.abilities.contains(&q.ability)
                    } else {
                        self.item_count(&q.item) == 0
                    }
                })
                .map(|(s, _)| *s)
                .collect();
            if let Some(o) = self.world.get_mut(who) {
                for s in gone {
                    if let Some(q) = o.quickslots.remove(&s) {
                        // Gained again later, it comes back.
                        o.quick_placed.remove(&quick_key(&q));
                    }
                }
            }
            let bar = self.quickbar(who);
            let Some(o) = self.world.get_mut(who) else {
                continue;
            };
            for (slot, q) in bar {
                if o.quickslots.contains_key(&slot) || o.quick_placed.contains(&quick_key(&q)) {
                    continue;
                }
                o.quick_placed.insert(quick_key(&q));
                o.quickslots.insert(slot, q);
            }
        }
    }

    /// `who`'s quickbar, by slot: its layout, then (for what was never placed) its other usable
    /// abilities in the free slots, then the party's quick items.
    pub fn quickbar(&self, who: ObjectId) -> Vec<(i32, Quickslot)> {
        let Some(o) = self.world.get(who) else {
            return Vec::new();
        };
        let mut bar: Vec<(i32, Quickslot)> = o
            .quickslots
            .iter()
            .filter(|(s, _)| (0..QUICKSLOTS).contains(*s))
            // A hidden ability placed by an earlier build (saves made before the rule)
            // leaves its slot empty.
            .filter(|(_, q)| !(q.item.is_empty() && self.ability_hidden(q.ability)))
            .map(|(s, q)| (*s, q.clone()))
            .collect();
        let placed: Vec<i32> = bar.iter().map(|(_, q)| q.ability).collect();
        let was_placed = |q: &Quickslot| o.quick_placed.contains(&quick_key(q));
        let taken: Vec<i32> = bar.iter().map(|(s, _)| *s).collect();
        let mut free = (0..QUICKSLOTS).filter(|s| !taken.contains(s));
        for &ability in &o.abilities {
            if placed.contains(&ability)
                || self.ability_hidden(ability)
                || !OWN_ABILITY_TYPES.contains(&self.abi_int(ability, "AbilityType"))
                || !matches!(
                    self.ability_use_type(ability),
                    USE_SUSTAINED | USE_ACTIVATED
                )
            {
                continue;
            }
            let q = Quickslot {
                ability,
                item: String::new(),
            };
            if was_placed(&q) {
                continue;
            }
            let Some(slot) = free.next() else { break };
            bar.push((slot, q));
        }
        for (tag, ability, _) in self.quick_items() {
            let q = Quickslot { ability, item: tag };
            if bar
                .iter()
                .any(|(_, b)| b.item.eq_ignore_ascii_case(&q.item))
                || was_placed(&q)
            {
                continue;
            }
            let Some(slot) = free.next() else { break };
            bar.push((slot, q));
        }
        bar.sort_by_key(|(s, _)| *s);
        bar
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use thedas_script::EngineTable;
    use thedas_vfs::Vfs;

    use super::*;
    use crate::world::ObjectKind;

    /// ABI_base rows for fireball (a circle, medium range), a cone (short range), pommel
    /// strike (no range) and heal (on a friend, medium range), and ranges.gda's rows.
    fn ranged_sim() -> Sim {
        let s = Sim::new(Arc::new(Vfs::new()), Arc::new(EngineTable::default()), 1);
        let (i, f) = (|v: i64| Cell::Int(v), |v: f64| Cell::Float(v));
        s.world.tables().insert(
            "ABI_base",
            &[
                "TargetType",
                "aoe_type",
                "aoe_param1",
                "aoe_param2",
                "Range",
            ],
            vec![
                (10003, vec![i(127), i(1), f(7.5), f(0.0), i(3)]),
                (10201, vec![i(126), i(2), i(35), i(10), i(2)]),
                (3024, vec![i(4), i(0), i(0), i(0), i(0)]),
                (10104, vec![i(3), i(0), i(0), i(0), i(3)]),
            ],
        );
        s.world.tables().insert(
            "#134",
            &["PrimaryRange"],
            vec![(0, vec![f(0.0)]), (2, vec![f(8.0)]), (3, vec![f(25.0)])],
        );
        s
    }

    #[test]
    fn ground_abilities_are_ordered_at_a_place_and_cast_within_range() {
        let mut s = ranged_sim();
        assert!(s.ability_targets_place(10003) && s.ability_targets_place(10201));
        assert!(!s.ability_targets_place(3024));
        assert_eq!(s.ability_area(10003), AbilityArea::Sphere(7.5));
        assert_eq!(s.ability_area(10201), AbilityArea::Cone(35.0, 10.0));
        assert_eq!(s.ability_area(3024), AbilityArea::None);
        assert_eq!(s.ability_range(10003), Some(25.0));
        assert_eq!(s.ability_range(10201), Some(8.0));
        assert_eq!(s.ability_range(3024), None);
        let area = s.world.spawn(ObjectKind::Area, "a");
        let mage = s.world.spawn(ObjectKind::Creature, "mage");
        s.world.get_mut(mage).unwrap().area = Some(area);
        s.order_ability_at(mage, 10003, [10.0, 0.0, 0.0], "");
        let c = s.world.get(mage).unwrap().commands[0].clone();
        assert_eq!((c.target, c.ints.clone()), (None, vec![10003]));
        assert_eq!(c.location.unwrap().position, [10.0, 0.0, 0.0]);
        // In range: its place is where it lands, not where the mage goes.
        s.tick(5_000);
        assert_eq!(s.world.get(mage).unwrap().position, [0.0, 0.0, 0.0]);
        // Out of range: the mage walks until the place is 25 m off, then casts.
        s.order_ability_at(mage, 10003, [40.0, 0.0, 0.0], "");
        s.tick(10_000);
        let o = s.world.get(mage).unwrap();
        assert!((o.position[0] - 15.0).abs() < 0.01, "{:?}", o.position);
        // A cone aimed at a place only takes its direction there: no walk.
        s.world.get_mut(mage).unwrap().position = [0.0; 3];
        s.order_ability_at(mage, 10201, [40.0, 0.0, 0.0], "");
        s.tick(5_000);
        assert_eq!(s.world.get(mage).unwrap().position, [0.0, 0.0, 0.0]);
    }

    #[test]
    fn personal_range_abilities_walk_into_melee_reach_of_a_creature() {
        let mut s = ranged_sim();
        s.world.trace.keep = true;
        let area = s.world.spawn(ObjectKind::Area, "a");
        let warrior = s.world.spawn(ObjectKind::Creature, "warrior");
        let archer = s.world.spawn(ObjectKind::Creature, "archer");
        for (id, x) in [(warrior, 0.0), (archer, 20.0)] {
            let o = s.world.get_mut(id).unwrap();
            o.area = Some(area);
            o.position = [x, 0.0, 0.0];
        }
        let casts = |s: &Sim| {
            s.world
                .trace
                .entries
                .iter()
                .filter(|e| {
                    matches!(e, crate::trace::TraceEntry::Log { text, .. }
                        if text.starts_with("ABILITY_CAST_START"))
                })
                .count()
        };
        // 3024 has personal range (row 0): 20 m off, the bash is not cast from there; the
        // warrior walks until the archer is within the radii and 1.2 m (2.4 m for two
        // default bodies), then casts.
        s.order_ability(warrior, 3024, archer, "");
        s.tick(1);
        assert_eq!(casts(&s), 0);
        s.tick(10_000);
        let o = s.world.get(warrior).unwrap();
        assert!((o.position[0] - 17.6).abs() < 0.01, "{:?}", o.position);
        assert_eq!(casts(&s), 1);
    }

    #[test]
    fn abilities_on_a_creature_walk_into_range_of_its_body() {
        let mut s = ranged_sim();
        let area = s.world.spawn(ObjectKind::Area, "a");
        let mage = s.world.spawn(ObjectKind::Creature, "mage");
        let friend = s.world.spawn(ObjectKind::Creature, "friend");
        for (id, x) in [(mage, 0.0), (friend, 40.0)] {
            let o = s.world.get_mut(id).unwrap();
            o.area = Some(area);
            o.position = [x, 0.0, 0.0];
        }
        s.order_ability(mage, 10104, friend, "");
        // At 5 m/s, a second in: walking.
        s.tick(1_000);
        let x = s.world.get(mage).unwrap().position[0];
        assert!((x - 5.0).abs() < 0.01, "{x}");
        s.tick(10_000);
        let reach = 25.0 + crate::combat::DEFAULT_RADIUS;
        let x = s.world.get(mage).unwrap().position[0];
        assert!((x - (40.0 - reach)).abs() < 0.01, "{x}");
        // An ability with no range used on oneself is used where its user stands (on
        // another creature it walks into melee reach: see the next test).
        s.world.get_mut(mage).unwrap().position = [0.0; 3];
        s.order_ability(mage, 3024, mage, "");
        s.tick(5_000);
        assert_eq!(s.world.get(mage).unwrap().position, [0.0, 0.0, 0.0]);
    }

    #[test]
    fn the_bar_keeps_its_layout_and_the_players_changes() {
        let mut s = Sim::new(Arc::new(Vfs::new()), Arc::new(EngineTable::default()), 1);
        let i = |v: i64| Cell::Int(v);
        s.world.tables().insert(
            "ABI_base",
            &["AbilityType", "UseType", "TargetType"],
            vec![
                (700, vec![i(1), i(2), i(1)]),
                (3024, vec![i(1), i(4), i(4)]),
                (14000, vec![i(2), i(4), i(4)]),
                (200011, vec![i(4), i(4), i(1)]),
            ],
        );
        let hero = s.world.spawn(ObjectKind::Creature, "hero");
        s.world.hero = Some(hero);
        s.world.get_mut(hero).unwrap().abilities = [700, 3024].into_iter().collect();
        let potion = s.world.spawn(ObjectKind::Item, "poultice");
        s.world
            .get_mut(potion)
            .unwrap()
            .fields
            .insert("AbilityID".into(), 200011);
        s.world.get_mut(hero).unwrap().inventory = vec![potion];
        let layout = |s: &Sim| -> Vec<(i32, i32)> {
            s.world
                .get(hero)
                .unwrap()
                .quickslots
                .iter()
                .map(|(k, q)| (*k, q.ability))
                .collect()
        };
        s.settle_quickbars();
        assert_eq!(layout(&s), [(0, 700), (1, 3024), (2, 200011)]);
        // A new ability takes a free slot; nothing moves.
        s.world.get_mut(hero).unwrap().abilities.insert(14000);
        s.settle_quickbars();
        assert_eq!(layout(&s), [(0, 700), (1, 3024), (2, 200011), (3, 14000)]);
        // The player drags pommel strike off the bar, and the sustained one to slot 9.
        let msg = |s: &mut Sim, args: &[&str]| {
            let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
            assert!(s.gui_message("GUI.SelectedChar.SetQuickslot", &args));
        };
        let h = hero.to_string();
        msg(&mut s, &[&h, "1", "0", "", ""]);
        msg(&mut s, &[&h, "0", "0", "", ""]);
        msg(&mut s, &[&h, "9", "700", "", ""]);
        s.settle_quickbars();
        assert_eq!(layout(&s), [(2, 200011), (3, 14000), (9, 700)]);
        // The last potion gone, its slot empties; found again, it takes the first free one.
        s.world.get_mut(hero).unwrap().inventory.clear();
        s.settle_quickbars();
        assert_eq!(layout(&s), [(3, 14000), (9, 700)]);
        s.world.get_mut(hero).unwrap().inventory = vec![potion];
        s.settle_quickbars();
        assert_eq!(layout(&s), [(0, 200011), (3, 14000), (9, 700)]);
        // A slot off the bar's end or a creature that does not exist changes nothing.
        msg(&mut s, &[&h, "99", "3024", "", ""]);
        msg(&mut s, &["123456", "4", "3024", "", ""]);
        assert_eq!(layout(&s), [(0, 200011), (3, 14000), (9, 700)]);
        assert_eq!(s.quickbar(hero).len(), 3);
        // The layout, and what the player took off, survive a save.
        let saved = s.save_json().unwrap();
        let mut t = Sim::new(Arc::new(Vfs::new()), Arc::new(EngineTable::default()), 1);
        t.load_json(&saved).unwrap();
        t.world.tables().insert(
            "ABI_base",
            &["AbilityType", "UseType", "TargetType"],
            vec![(3024, vec![i(1), i(4), i(4)])],
        );
        t.settle_quickbars();
        assert_eq!(layout(&t), [(0, 200011), (3, 14000), (9, 700)]);
    }

    #[test]
    fn the_party_shares_its_quick_items() {
        let mut s = Sim::new(Arc::new(Vfs::new()), Arc::new(EngineTable::default()), 1);
        let i = |v: i64| Cell::Int(v);
        s.world.tables().insert(
            "ABI_base",
            &["AbilityType", "UseType", "TargetType"],
            vec![
                (700, vec![i(1), i(2), i(1)]),
                (200011, vec![i(4), i(4), i(1)]),
                (200111, vec![i(4), i(3), i(1)]), // passive: not usable
            ],
        );
        let hero = s.world.spawn(ObjectKind::Creature, "hero");
        let mage = s.world.spawn(ObjectKind::Creature, "mage");
        s.world.hero = Some(hero);
        s.world.get_mut(hero).unwrap().followers.push(mage);
        s.world.get_mut(mage).unwrap().abilities.insert(700);
        let item = |s: &mut Sim, tag: &str, ability: i32, stack: i32| {
            let it = s.world.spawn(ObjectKind::Item, tag);
            let o = s.world.get_mut(it).unwrap();
            o.fields.insert("AbilityID".into(), ability);
            o.fields.insert("StackSize".into(), stack);
            it
        };
        let a = item(&mut s, "poultice", 200011, 2);
        let b = item(&mut s, "poultice", 200011, 1);
        let c = item(&mut s, "coating", 200111, 1);
        s.world.get_mut(hero).unwrap().inventory = vec![a, b, c];
        assert_eq!(s.quick_items(), vec![("poultice".to_string(), 200011, 3)]);
        assert_eq!(s.item_count("POULTICE"), 3);
        // The mage carries none but uses the hero's.
        assert_eq!(s.party_item(mage, "poultice"), Some(a));
        let bar: Vec<(i32, i32, String)> = s
            .quickbar(mage)
            .into_iter()
            .map(|(slot, q)| (slot, q.ability, q.item))
            .collect();
        assert_eq!(
            bar,
            vec![(0, 700, String::new()), (1, 200011, "poultice".to_string())]
        );
    }

    #[test]
    fn the_bar_holds_set_slots_then_usable_abilities() {
        let mut s = Sim::new(Arc::new(Vfs::new()), Arc::new(EngineTable::default()), 1);
        let i = |v: i64| Cell::Int(v);
        s.world.tables().insert(
            "ABI_base",
            &["AbilityType", "UseType", "TargetType", "cooldown"],
            vec![
                (14, vec![i(1), i(3), i(0), i(0)]),     // passive talent
                (700, vec![i(1), i(2), i(1), i(0)]),    // sustained, on self
                (3024, vec![i(1), i(4), i(4), i(10)]),  // activated, on a foe
                (200010, vec![i(4), i(4), i(1), i(0)]), // an item's ability
            ],
        );
        let who = s.world.spawn(ObjectKind::Creature, "w");
        let o = s.world.get_mut(who).unwrap();
        o.abilities = [14, 700, 3024, 200010].into_iter().collect();
        o.quickslots.insert(
            0,
            Quickslot {
                ability: 200010,
                item: "potion".into(),
            },
        );
        let bar: Vec<(i32, i32)> = s
            .quickbar(who)
            .into_iter()
            .map(|(slot, q)| (slot, q.ability))
            .collect();
        assert_eq!(bar, vec![(0, 200010), (1, 700), (2, 3024)]);
        assert!(s.ability_on_self(700));
        assert!(!s.ability_on_self(3024));
        // Half of a 10 s cooldown left.
        let key = crate::engine::cooldown_key(3024, "");
        s.world
            .get_mut(who)
            .unwrap()
            .combat
            .cooldowns
            .insert(key, 5_000);
        assert!((s.cooldown_left(who, 3024, "") - 0.5).abs() < 1e-6);
    }
}
