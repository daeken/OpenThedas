//! DA2 property derivations (DragonAge2.exe 0x5ae3b0, function table 0xd2df68).
//! The shipped scripts set bases and levels. Native totals use numeric property metadata
//! and a different attribute order and rating curve from Origins.

use thedas_formats::gda::Cell;
use thedas_script::vm::ObjectId;

use crate::sim::Sim;

fn number(cell: Option<&Cell>) -> Option<f32> {
    let n = match cell? {
        Cell::Float(v) => *v as f32,
        Cell::Int(v) => *v as f32,
        _ => return None,
    };
    n.is_finite().then_some(n)
}

impl Sim {
    /// Native AddAbility (0x5909c0 -> 0x56dfe0) applies up to ten stat modifiers
    /// from table 117. Compute this contribution from known abilities, so grants,
    /// removals and older saves share one source and never apply it twice.
    pub(crate) fn da2_ability_property_bonus(&self, id: ObjectId, property: i32) -> f32 {
        if self.game != thedas_vfs::Game::DragonAge2 {
            return 0.0;
        }
        let Some(o) = self.world.get(id) else {
            return 0.0;
        };
        let Some(abilities) = self.world.tables.get(1, "") else {
            return 0.0;
        };
        let Some(modifiers) = self.world.tables.get(117, "") else {
            return 0.0;
        };
        let columns = [
            0xb647_b38b,
            0x9d6a_e048,
            0x8471_d109,
            0xcb30_47ce,
            0xd22b_768f,
            0xf906_254c,
            0xe01d_140d,
            0x6785_08c2,
            0x7e9e_3983,
            0x1480_31fd,
        ];
        let mut bonus = 0.0;
        for &ability in &o.abilities {
            for column in columns {
                let Some(row) =
                    number(abilities.cell_by_hash(column, ability as i64)).filter(|n| *n != 0.0)
                else {
                    break;
                };
                let row = row as i64;
                if number(modifiers.cell_by_hash(0x5012_0e24, row)).unwrap_or(0.0) as i32
                    != property
                {
                    continue;
                }
                let flags = number(modifiers.cell_by_hash(0xca5f_bbe9, row)).unwrap_or(0.0) as i32;
                // Class/race/spawn bonuses have no equipment condition. Conditional
                // rows need the DA2 weapon-mode mask (0x52f7c0), which is separate
                // from the simulator's existing weapon-style enum.
                if flags == 0 {
                    bonus += number(modifiers.cell_by_hash(0x28d1_2c72, row)).unwrap_or(0.0);
                }
            }
        }
        bonus
    }

    /// Native template load 0x58aa24 reads APR race, then initializes the brain's race.
    pub(crate) fn init_da2_race(&mut self, id: ObjectId) {
        if self.game != thedas_vfs::Game::DragonAge2 {
            return;
        }
        let Some(o) = self.world.get(id).filter(|o| {
            o.kind == crate::world::ObjectKind::Creature && !o.fields.contains_key("Race")
        }) else {
            return;
        };
        let race = self
            .world
            .tables
            .get(2, "")
            .and_then(|t| number(t.cell_by_hash(0x19fb_e813, o.appearance as i64)));
        if let Some(race) = race {
            self.world
                .get_mut(id)
                .unwrap()
                .fields
                .insert("Race".into(), race as i32);
        }
    }

    pub(crate) fn da2_attribute_bonus(&self, id: ObjectId, property: i32) -> f32 {
        let Some(o) = self.world.get(id) else {
            return 0.0;
        };
        let index = self
            .world
            .tables
            .get(98, "")
            .and_then(|t| number(t.cell_by_hash(0xf00a_1048, property as i64)))
            .unwrap_or(0.0) as i32;
        let attr = |p| self.property_total(id, p);
        match index {
            1 => 5.0 * attr(6),
            2 => 5.0 * attr(5),
            3 => {
                let primary = self.property_total(id, 50) as i32;
                self.da2_rating_from_attribute(
                    id,
                    13,
                    if (1..=6).contains(&primary) {
                        primary
                    } else {
                        0
                    },
                )
            }
            4 => (attr(2) - 10.0).max(0.0),
            5 => self.da2_rating_from_attribute(id, 24, 4),
            6 => (attr(3) - 10.0).max(0.0),
            7 => (attr(1) - 10.0).max(0.0),
            8 => {
                let mut bonus = (attr(4) - 10.0).max(0.0);
                // Devious Harm (0x5bdc90): its Power multiplies cunning's contribution.
                if o.abilities.contains(&210040) {
                    let power = self
                        .world
                        .tables
                        .get(1, "")
                        .and_then(|t| number(t.cell_by_hash(0x15bd_f19c, 210040)))
                        .unwrap_or(0.0);
                    bonus *= 1.0 + power * 0.01;
                }
                bonus
            }
            _ => 0.0,
        }
    }

    /// 0x5bda30: interpolate adjacent optimal-rating rows using attribute points above
    /// ten divided by property_functions' points-per-row. Missing curve rows give zero.
    fn da2_rating_from_attribute(&self, id: ObjectId, property: i32, attribute: i32) -> f32 {
        let Some(functions) = self.world.tables.get(371, "") else {
            return 0.0;
        };
        let Some(points) =
            number(functions.cell_by_hash(0x3d9e_dcd2, property as i64)).filter(|p| *p > 0.0)
        else {
            return 0.0;
        };
        let Some(selector) = number(functions.cell_by_hash(0x1d78_b8e3, property as i64)) else {
            return 0.0;
        };
        let columns = [0x73a8_6afe, 0xe337_46bf, 0xf9fc_7170, 0x6e1d_17ea];
        let Some(&column) = usize::try_from(selector as i32)
            .ok()
            .and_then(|i| columns.get(i))
        else {
            return 0.0;
        };
        let Some(data) = self.world.tables.get(405, "") else {
            return 0.0;
        };
        let x = (self.property_total(id, attribute) - 10.0).max(0.0) / points;
        let row = x.floor() as i64;
        let lo = number(data.cell_by_hash(column, row)).unwrap_or(0.0);
        let hi = number(data.cell_by_hash(column, row + 1)).unwrap_or(0.0);
        lo + (hi - lo) * (x - row as f32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{m2da::Tables, world::ObjectKind};
    use std::sync::Arc;
    use thedas_script::EngineTable;
    use thedas_vfs::{Game, Vfs};

    fn sim() -> (Sim, ObjectId) {
        let mut s = Sim::new(
            Arc::new(Vfs::new()),
            Arc::new(EngineTable::parse(b"").unwrap()),
            1,
        );
        s.game = Game::DragonAge2;
        let t = Tables::empty();
        let rows = [
            (1, 1, 0, 0., 100.),
            (2, 1, 0, 0., 100.),
            (3, 1, 0, 0., 100.),
            (4, 1, 0, 0., 100.),
            (5, 1, 0, 0., 100.),
            (6, 1, 0, 0., 100.),
            (7, 3, 1, 1., 1000.),
            (10, 3, 2, 0., 1000.),
            (13, 1, 3, 0., 1000.),
            (15, 1, 4, 0., 100.),
            (16, 1, 8, 0., 100.),
            (24, 1, 5, 0., 1000.),
            (33, 1, 6, 0., 100.),
            (34, 1, 7, 0., 100.),
            (36, 2, 0, 0., 100.),
            (50, 2, 0, 1., 6.),
        ];
        let columns = ["#baf55bcf", "#f00a1048", "#98377833", "#b48bff62"];
        let props: Vec<_> = rows
            .into_iter()
            .map(|(id, ty, d, min, max)| {
                (
                    id,
                    vec![
                        Cell::Int(ty),
                        Cell::Int(d),
                        Cell::Float(min),
                        Cell::Float(max),
                    ],
                )
            })
            .collect();
        t.insert("properties", &columns, props.clone());
        t.insert("#98", &columns, props);
        t.insert(
            "#371",
            &["#3d9edcd2", "#1d78b8e3"],
            vec![
                (13, vec![Cell::Float(2.), Cell::Int(0)]),
                (24, vec![Cell::Float(2.), Cell::Int(1)]),
            ],
        );
        t.insert(
            "#405",
            &["#73a86afe", "#e33746bf"],
            vec![
                (1, vec![Cell::Float(40.), Cell::Float(30.)]),
                (2, vec![Cell::Float(80.), Cell::Float(50.)]),
            ],
        );
        s.world.tables = Arc::new(t);
        let id = s.world.spawn(ObjectKind::Creature, "synthetic");
        for p in 1..=6 {
            s.world.get_mut(id).unwrap().properties.insert(p, (10., 0.));
        }
        s.world.get_mut(id).unwrap().properties.insert(50, (3., 0.));
        (s, id)
    }

    #[test]
    fn numeric_metadata_drives_da2_totals_and_depletables() {
        let (mut s, id) = sim();
        let o = s.world.get_mut(id).unwrap();
        o.properties.insert(3, (13., 0.)); // primary magic: curve position 1.5
        o.properties.insert(4, (12., 0.));
        o.properties.insert(5, (11., 0.));
        o.properties.insert(6, (14., 0.));
        o.properties.insert(7, (50., 30.));
        o.properties.insert(10, (50., 20.));
        o.modifiers.insert(3, 1.); // totals, including modifiers, feed derivations
        assert_eq!(s.property_total(id, 13), 80.);
        s.world.get_mut(id).unwrap().modifiers.clear();
        assert_eq!(s.property_total(id, 13), 60.);
        assert_eq!(s.property_total(id, 24), 30.);
        assert_eq!(s.property_total(id, 7), 120.);
        assert_eq!(s.depletable_max(id, 10), 105.);
        assert_eq!(s.property_total(id, 33), 3.);
        assert_eq!(s.world.get(id).unwrap().properties[&7].1, 30.);
        s.world
            .get_mut(id)
            .unwrap()
            .properties
            .insert(36, (1000., 0.));
        assert_eq!(s.property_total(id, 36), 100.);
    }

    #[test]
    fn known_ability_modifiers_follow_grants_removals_and_reload() {
        let (mut s, id) = sim();
        s.world.tables.insert(
            "#1",
            &["#b647b38b", "#9d6ae048"],
            vec![
                (123, vec![Cell::Int(801), Cell::Int(801)]), // each slot contributes
            ],
        );
        s.world.tables.insert(
            "#117",
            &["#50120e24", "#28d12c72", "#ca5fbbe9"],
            vec![(801, vec![Cell::Int(3), Cell::Float(1.5), Cell::Int(0)])],
        );
        assert_eq!(s.property_total(id, 13), 0.);
        s.world.get_mut(id).unwrap().abilities.insert(123);
        assert_eq!(s.property_total(id, 3), 13.);
        assert_eq!(s.property_total(id, 13), 60.);
        let save = s.save_json().unwrap();
        s.load_json(&save).unwrap();
        assert_eq!(
            s.property_total(id, 3),
            13.,
            "reload does not stack ability modifiers"
        );
        s.world.get_mut(id).unwrap().abilities.remove(&123);
        assert_eq!(s.property_total(id, 13), 0.);
    }

    #[test]
    fn race_initializes_from_appearance_and_repairs_only_missing_save_fields() {
        let (mut s, id) = sim();
        s.world
            .tables
            .insert("#2", &["#19fbe813"], vec![(42, vec![Cell::Int(7)])]);
        s.world.get_mut(id).unwrap().appearance = 42;
        let save = s.save_json().unwrap();
        s.load_json(&save).unwrap();
        assert_eq!(s.world.get(id).unwrap().fields["Race"], 7);
        s.world.get_mut(id).unwrap().fields.insert("Race".into(), 3);
        s.init_da2_race(id);
        assert_eq!(s.world.get(id).unwrap().fields["Race"], 3);
        s.game = Game::Origins;
        s.world.get_mut(id).unwrap().fields.remove("Race");
        s.init_da2_race(id);
        assert!(!s.world.get(id).unwrap().fields.contains_key("Race"));
        s.world.get_mut(id).unwrap().abilities.insert(123);
        assert_eq!(s.da2_ability_property_bonus(id, 3), 0.);
    }
}
