//! What attributes add to other properties, which no script applies: the rules scripts read
//! the totals of Health, Mana_Stamina, Attack, Defense, SpellPower and the resistances
//! (`GetCreatureProperty` with `PROPERTY_VALUE_TOTAL`) and expect the attributes to be in them.
//!
//! As DAOrigins.exe derives them: a property's total (0x79c760) is its base, plus what its
//! derivation gives (0x79c5b0), plus its modifiers. `properties.gda` names each property's
//! derivation by index (the column hashed `0x106ccdd5`) into a table of ten functions
//! (0xc0f604), each counting attribute totals from 10, the same for every creature:
//!
//! | index | function | used by | amount |
//! |---|---|---|---|
//! | 1 | 0x79cb30 | Attack | 0.5 (Strength − 10) + 0.5 (Dexterity − 10) |
//! | 2 | 0x79cc90 | Defense | Dexterity − 10 |
//! | 3 | 0x83a430 | Armor | 0 |
//! | 4 | 0x79cd40 | AP_BONUS | (Cunning − 10) / 7 |
//! | 5 | 0x79cdf0 | SpellPower | Magic − 10 |
//! | 6 | 0x79cea0 | Health | 5 (Constitution − 10) |
//! | 7 | 0x79cf50 | Mana_Stamina | 5 (Willpower − 10) |
//! | 8 | 0x79d000 | Resistance_Mind | 0.5 ((Willpower − 10) + (Magic − 10) + (Cunning − 10)) |
//! | 9 | 0x79d0f0 | Resistance_Physical | 0.5 ((Strength − 10) + (Constitution − 10) + (Dexterity − 10)) |
//!
//! Scores under 10 subtract. An original save agrees: a hero with Constitution 14 and
//! Willpower 11 stores Health base 100, current 120 and Mana base 100, current 105.

use thedas_script::vm::ObjectId;

use thedas_formats::gda::Cell;

use crate::sim::Sim;
use crate::world::ObjectKind;

/// The score attributes are counted from (0xae7d48).
pub const ATTRIBUTE_BASE: f32 = 10.0;

/// `properties.gda`'s derivation index column (its name is not known; its hash is).
const DERIVATION_COLUMN: u32 = 0x106c_cdd5;

impl Sim {
    /// A depletable property's maximum (its total): the base, equipment and attributes.
    pub fn depletable_max(&self, id: ObjectId, property: i32) -> f32 {
        let base = self
            .world
            .get(id)
            .and_then(|o| o.properties.get(&property))
            .map_or(0.0, |v| v.0);
        let max = base
            + self.equipment_bonus(id, property)
            + self.attribute_bonus(id, property)
            + self.effect_modifier(id, property);
        if self.game == thedas_vfs::Game::DragonAge2 {
            max + self.da2_ability_property_bonus(id, property)
        } else {
            max
        }
    }

    /// A property's total, as `GetCreatureProperty` gives `PROPERTY_VALUE_TOTAL`
    /// (DAOrigins.exe 0x79c760 for attributes and depletables, 0x79c6d0 for simple
    /// properties): its base, plus its derivation, plus its modifiers (none for a simple
    /// property), within `properties.gda`'s bounds (0x79c2d0). A depletable's total is its
    /// maximum, whatever is left of it now.
    pub fn property_total(&self, id: ObjectId, property: i32) -> f32 {
        use crate::engine::PropertyKind;
        let base = self
            .world
            .get(id)
            .and_then(|o| o.properties.get(&property))
            .map_or(0.0, |v| v.0);
        let derived = self.attribute_bonus(id, property);
        let v = match crate::engine::property_kind(self, property) {
            PropertyKind::Simple => base + derived,
            _ => base + derived + self.property_modifier(id, property),
        };
        match crate::engine::property_bounds(self, property) {
            Some((_, min, max)) => v.clamp(min, max),
            None => v,
        }
    }

    /// A property's modifiers (`PROPERTY_VALUE_MODIFIER`): what worn items' stats and
    /// effects add.
    pub fn property_modifier(&self, id: ObjectId, property: i32) -> f32 {
        let modifier = self.equipment_bonus(id, property) + self.effect_modifier(id, property);
        if self.game == thedas_vfs::Game::DragonAge2 {
            modifier + self.da2_ability_property_bonus(id, property)
        } else {
            modifier
        }
    }

    /// Origins' depletable current setter (0x79c8d0): floor first, then total,
    /// including a total below the floor or unordered. Only health's floor is
    /// changed by SetImmortal; the flag alone does not establish that floor.
    pub(crate) fn clamped_current(&self, id: ObjectId, property: i32, value: f32) -> f32 {
        let total = self.property_total(id, property);
        if self.game == thedas_vfs::Game::Origins {
            let floor = if property == 7
                && self
                    .world
                    .get(id)
                    .is_some_and(|o| o.kind == ObjectKind::Creature && o.health_floor_one)
            {
                1.0
            } else {
                0.0
            };
            let floored = if value > floor { value } else { floor };
            if total > floored { floored } else { total }
        } else {
            value.clamp(0.0, total.max(0.0))
        }
    }

    /// Reclamps current after a base or modifier changes (0x79c860, 0x79c890).
    pub fn clamp_current(&mut self, id: ObjectId, property: i32) {
        let Some(current) = self
            .world
            .get(id)
            .and_then(|o| o.properties.get(&property))
            .map(|v| v.1)
        else {
            return;
        };
        let current = self.clamped_current(id, property, current);
        if let Some(v) = self
            .world
            .get_mut(id)
            .and_then(|o| o.properties.get_mut(&property))
        {
            v.1 = current;
        }
    }

    /// What effects add to a property (worn items' properties, buffs): see
    /// [`crate::world::Object::modifiers`].
    pub fn effect_modifier(&self, id: ObjectId, property: i32) -> f32 {
        self.world
            .get(id)
            .and_then(|o| o.modifiers.get(&property).copied())
            .unwrap_or(0.0)
    }

    /// What a creature's attributes add to `property`'s total (0 for a property without a
    /// derivation): see the module docs.
    pub fn attribute_bonus(&self, id: ObjectId, property: i32) -> f32 {
        if self.game == thedas_vfs::Game::DragonAge2 {
            return self.da2_attribute_bonus(id, property);
        }
        let Some(o) = self.world.get(id) else {
            return 0.0;
        };
        let index = self
            .world
            .tables
            .get(0, "properties")
            .and_then(
                |t| match t.cell_by_hash(DERIVATION_COLUMN, property as i64)? {
                    Cell::Int(v) => Some(*v),
                    _ => None,
                },
            )
            .unwrap_or(0);
        // An attribute's total, counted from 10 (the attributes are properties 1-6).
        let attr = |p: i32| {
            o.properties
                .get(&p)
                .map_or(ATTRIBUTE_BASE, |v| v.0.max(v.1))
                + o.modifiers.get(&p).copied().unwrap_or(0.0)
                - ATTRIBUTE_BASE
        };
        let (str_, dex, wil, mag, cun, con) =
            (attr(1), attr(2), attr(3), attr(4), attr(5), attr(6));
        match index {
            1 => 0.5 * str_ + 0.5 * dex,
            2 => dex,
            4 => cun / 7.0,
            5 => mag,
            6 => 5.0 * con,
            7 => 5.0 * wil,
            8 => 0.5 * (wil + mag + cun),
            9 => 0.5 * (str_ + con + dex),
            _ => 0.0,
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
    fn attributes_add_through_each_propertys_derivation() {
        let table = Arc::new(EngineTable::parse(LDF.as_bytes()).unwrap());
        let mut s = Sim::new(Arc::new(Vfs::new()), table, 1);
        // properties.gda's derivation index for each derived property.
        let t = crate::m2da::Tables::empty();
        let rows = [
            (7, 6),
            (8, 7),
            (9, 1),
            (10, 2),
            (11, 3),
            (13, 5),
            (32, 8),
            (33, 9),
            (53, 4),
        ];
        t.insert(
            "properties",
            &["ID", "#106ccdd5"],
            rows.iter()
                .map(|&(id, d)| (id, vec![Cell::Int(id), Cell::Int(d)]))
                .collect(),
        );
        s.world.tables = Arc::new(t);
        let c = s.world.spawn(ObjectKind::Creature, "warrior");
        let attrs = [
            (1, 22.0),
            (2, 19.0),
            (3, 11.0),
            (4, 8.0),
            (5, 24.0),
            (6, 16.0),
        ];
        for (p, v) in attrs {
            s.world.get_mut(c).unwrap().properties.insert(p, (v, 0.0));
        }
        let bonus = |p| s.attribute_bonus(c, p);
        assert_eq!(bonus(7), 30.0, "health: constitution 16, five a point");
        assert_eq!(bonus(8), 5.0, "mana/stamina: willpower 11, five a point");
        assert_eq!(bonus(9), 10.5, "attack: half of strength's and dexterity's");
        assert_eq!(bonus(10), 9.0);
        assert_eq!(bonus(11), 0.0, "armour comes from gear only");
        assert_eq!(bonus(13), -2.0, "magic 8 takes away");
        assert_eq!(bonus(32), 0.5 * (1.0 - 2.0 + 14.0));
        assert_eq!(bonus(33), 0.5 * (12.0 + 6.0 + 9.0));
        assert_eq!(bonus(53), 2.0, "cunning 24: a seventh of a point");
        assert_eq!(bonus(12), 0.0, "no derivation");
        // A ranged attacker's attack is derived the same way.
        assert_eq!(bonus(9), 10.5);
    }
}
