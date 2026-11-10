//! The quickbar: the abilities a party member can use at a click, and how they are aimed.
//!
//! Scripts fill slots with `SetQuickslot`. Provisional: the engine is taken to put the
//! creature's other usable abilities (talents, spells and skills that are activated or
//! sustained, `ABI_base` `UseType` 4 or 2) in the free slots, lowest ability first, as a
//! new game's quickbar shows them; the executable's own rule is not traced.

use thedas_formats::gda::Cell;
use thedas_script::vm::ObjectId;

use crate::sim::Sim;
use crate::world::Quickslot;

/// The slots the quickbar has (`GUI.SelectedChar.Stats.QuickSlots.0` to `.39`).
pub const QUICKSLOTS: i32 = 40;

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
    fn abi_int(&self, ability: i32, column: &str) -> i32 {
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

    /// `who`'s quickbar, by slot: the slots scripts filled, and its other usable abilities
    /// in the free slots.
    pub fn quickbar(&self, who: ObjectId) -> Vec<(i32, Quickslot)> {
        let Some(o) = self.world.get(who) else {
            return Vec::new();
        };
        let mut bar: Vec<(i32, Quickslot)> = o
            .quickslots
            .iter()
            .filter(|(s, _)| (0..QUICKSLOTS).contains(*s))
            .map(|(s, q)| (*s, q.clone()))
            .collect();
        let placed: Vec<i32> = bar.iter().map(|(_, q)| q.ability).collect();
        let mut free = (0..QUICKSLOTS).filter(|s| !o.quickslots.contains_key(s));
        for &ability in &o.abilities {
            if placed.contains(&ability)
                || !OWN_ABILITY_TYPES.contains(&self.abi_int(ability, "AbilityType"))
                || !matches!(
                    self.ability_use_type(ability),
                    USE_SUSTAINED | USE_ACTIVATED
                )
            {
                continue;
            }
            let Some(slot) = free.next() else { break };
            bar.push((
                slot,
                Quickslot {
                    ability,
                    item: String::new(),
                },
            ));
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

    #[test]
    fn ground_abilities_are_ordered_at_a_place_and_cast_from_where_the_user_stands() {
        let mut s = Sim::new(Arc::new(Vfs::new()), Arc::new(EngineTable::default()), 1);
        let (i, f) = (|v: i64| Cell::Int(v), |v: f64| Cell::Float(v));
        s.world.tables().insert(
            "ABI_base",
            &["TargetType", "aoe_type", "aoe_param1", "aoe_param2"],
            vec![
                (10003, vec![i(127), i(1), f(7.5), f(0.0)]),
                (10201, vec![i(126), i(2), i(35), i(10)]),
                (3024, vec![i(4), i(0), i(0), i(0)]),
            ],
        );
        assert!(s.ability_targets_place(10003) && s.ability_targets_place(10201));
        assert!(!s.ability_targets_place(3024));
        assert_eq!(s.ability_area(10003), AbilityArea::Sphere(7.5));
        assert_eq!(s.ability_area(10201), AbilityArea::Cone(35.0, 10.0));
        assert_eq!(s.ability_area(3024), AbilityArea::None);
        let area = s.world.spawn(ObjectKind::Area, "a");
        let mage = s.world.spawn(ObjectKind::Creature, "mage");
        s.world.get_mut(mage).unwrap().area = Some(area);
        s.order_ability_at(mage, 10003, [10.0, 0.0, 0.0], "");
        let c = s.world.get(mage).unwrap().commands[0].clone();
        assert_eq!((c.target, c.ints.clone()), (None, vec![10003]));
        assert_eq!(c.location.unwrap().position, [10.0, 0.0, 0.0]);
        // Its place is where it lands, not where the mage goes.
        s.tick(5_000);
        assert_eq!(s.world.get(mage).unwrap().position, [0.0, 0.0, 0.0]);
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
