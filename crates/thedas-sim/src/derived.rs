//! What attributes add to other properties, which no script applies: the rules scripts read
//! the totals of Health, Mana_Stamina, Attack, Defense and SpellPower (`GetCreatureProperty`
//! with `PROPERTY_VALUE_TOTAL`) and expect the attributes to be in them.
//!
//! The amounts are the game's own attribute descriptions (talk table strings 396047-396052):
//! - Strength: melee attack +0.5 a point.
//! - Dexterity: melee attack +0.5 a point, ranged attack +1 a point, defense +1 a point.
//! - Constitution: health +5 a point.
//! - Willpower: mana or stamina +5 a point.
//! - Magic: spellpower +1 a point.
//!
//! Provisional: the descriptions count "each point purchased"; we count the points above
//! 10, the score every attribute starts from in character generation and at spawn
//! (`creature_core` sets all six to 10 before adding race, class and package points). Fewer
//! than 10 subtract. Whether the executable counts from 10, from the score at character
//! creation, or rounds the half points, is not recovered.

use thedas_script::vm::ObjectId;

use crate::sim::{Sim, constant};

/// The score attributes are counted from.
pub const ATTRIBUTE_BASE: f32 = 10.0;

impl Sim {
    /// A depletable property's maximum (its total): the base, equipment and attributes.
    pub fn depletable_max(&self, id: ObjectId, property: i32) -> f32 {
        let base = self
            .world
            .get(id)
            .and_then(|o| o.properties.get(&property))
            .map_or(0.0, |v| v.0);
        base + self.equipment_bonus(id, property)
            + self.attribute_bonus(id, property)
            + self.effect_modifier(id, property)
    }

    /// What effects add to a property (worn items' properties, buffs): see
    /// [`crate::world::Object::modifiers`].
    pub fn effect_modifier(&self, id: ObjectId, property: i32) -> f32 {
        self.world
            .get(id)
            .and_then(|o| o.modifiers.get(&property).copied())
            .unwrap_or(0.0)
    }

    /// What a creature's attributes add to `property`'s total (0 for the other properties).
    pub fn attribute_bonus(&self, id: ObjectId, property: i32) -> f32 {
        let c = |n: &str| constant(&self.table, n);
        let Some(o) = self.world.get(id) else {
            return 0.0;
        };
        let attr = |n: &str| {
            o.properties
                .get(&c(n))
                .map_or(ATTRIBUTE_BASE, |v| v.0.max(v.1))
                + o.modifiers.get(&c(n)).copied().unwrap_or(0.0)
                - ATTRIBUTE_BASE
        };
        if property == c("PROPERTY_DEPLETABLE_HEALTH") {
            5.0 * attr("PROPERTY_ATTRIBUTE_CONSTITUTION")
        } else if property == c("PROPERTY_DEPLETABLE_MANA_STAMINA") {
            5.0 * attr("PROPERTY_ATTRIBUTE_WILLPOWER")
        } else if property == c("PROPERTY_ATTRIBUTE_ATTACK") {
            let dex = attr("PROPERTY_ATTRIBUTE_DEXTERITY");
            if self.attack_reach(id) > crate::combat::MELEE_REACH {
                dex
            } else {
                0.5 * (attr("PROPERTY_ATTRIBUTE_STRENGTH") + dex)
            }
        } else if property == c("PROPERTY_ATTRIBUTE_DEFENSE") {
            attr("PROPERTY_ATTRIBUTE_DEXTERITY")
        } else if property == c("PROPERTY_ATTRIBUTE_SPELLPOWER") {
            attr("PROPERTY_ATTRIBUTE_MAGIC")
        } else {
            0.0
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use thedas_script::EngineTable;
    use thedas_vfs::Vfs;

    use crate::world::ObjectKind;

    use super::*;

    const LDF: &str = "int PROPERTY_DEPLETABLE_HEALTH = 7;\n\
        int PROPERTY_DEPLETABLE_MANA_STAMINA = 8;\n\
        int PROPERTY_ATTRIBUTE_STRENGTH = 1;\n\
        int PROPERTY_ATTRIBUTE_DEXTERITY = 2;\n\
        int PROPERTY_ATTRIBUTE_WILLPOWER = 3;\n\
        int PROPERTY_ATTRIBUTE_MAGIC = 4;\n\
        int PROPERTY_ATTRIBUTE_CONSTITUTION = 6;\n\
        int PROPERTY_ATTRIBUTE_ATTACK = 9;\n\
        int PROPERTY_ATTRIBUTE_DEFENSE = 10;\n\
        int PROPERTY_ATTRIBUTE_SPELLPOWER = 13;\n";

    #[test]
    fn attributes_above_ten_add_to_their_properties() {
        let table = Arc::new(EngineTable::parse(LDF.as_bytes()).unwrap());
        let mut s = Sim::new(Arc::new(Vfs::new()), table, 1);
        let c = s.world.spawn(ObjectKind::Creature, "warrior");
        for (p, v) in [(1, 22.0), (2, 19.0), (3, 11.0), (4, 8.0), (6, 16.0)] {
            s.world.get_mut(c).unwrap().properties.insert(p, (v, 0.0));
        }
        let bonus = |p| s.attribute_bonus(c, p);
        assert_eq!(bonus(7), 30.0, "constitution 16: +5 a point");
        assert_eq!(bonus(8), 5.0);
        assert_eq!(bonus(9), 10.5, "melee: half of strength's and dexterity's");
        assert_eq!(bonus(10), 9.0);
        assert_eq!(bonus(13), -2.0, "magic 8 takes away");
        assert_eq!(bonus(11), 0.0, "armour comes from gear only");
    }
}
