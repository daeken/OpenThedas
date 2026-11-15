//! DA2 item-property queries: 0x56f210 (types) and 0x54dbd0 (power).
//! The existing item model has no socketed sub-items and keeps integer base powers.

use thedas_formats::gda::Cell;
use thedas_script::vm::ObjectId;

use crate::{sim::Sim, world::ObjectKind};

fn number(cell: Option<&Cell>) -> Option<f32> {
    match cell? {
        Cell::Float(v) => Some(*v as f32),
        Cell::Int(v) => Some(*v as f32),
        _ => None,
    }
}

impl Sim {
    /// Native item-effect initialization (0x57c8f0 -> 0x56ef50): passive type 1
    /// and types above 4 build the row's effect; 0/3/4 do not, and type 2 uses
    /// a separate on-hit wrapper. Parameters are filled by 0x54d7d0, with the
    /// selected parameter scaled exactly as the item-power query does.
    pub(crate) fn da2_item_property_effects(
        &self,
        item: ObjectId,
        properties: &[(i32, i32)],
    ) -> Vec<crate::world::Effect> {
        let Some(table) = self.world.tables.get(12001, "") else {
            return Vec::new();
        };
        properties
            .iter()
            .filter_map(|&(property, power)| {
                let row = i64::from(property);
                let get = |column| number(table.cell_by_hash(column, row)).unwrap_or(0.);
                let int = |column| crate::engine::cell_int(table.cell_by_hash(column, row));
                let kind = int(0x12a0_2374);
                if kind != 1 && kind <= 4 {
                    return None;
                }
                let ty = int(0x3ba0_7a8a);
                if ty == 0 {
                    return None;
                }
                let mut ints = vec![int(0xa363_0beb), int(0xba78_3aaa)];
                let mut floats = vec![get(0x8619_84a9), get(0x9f02_b5e8)];
                // Use this entry's power, including when removing a property that
                // has already been taken out of the item's live property list.
                match int(0x3600_1b03) {
                    selected @ (1 | 2) => {
                        let base = self.da2_item_power_before_parameter(
                            item,
                            property,
                            power as f32,
                            &mut vec![property],
                        );
                        let parameter = &mut ints[(selected - 1) as usize];
                        // 0x54dad7/0x54daf4 multiply the stored float base by the
                        // integer field, add 0.5, then truncate. Keep that product
                        // wider until conversion; the float query rounds it earlier.
                        *parameter = (f64::from(base) * f64::from(*parameter) + 0.5) as i32;
                    }
                    selected @ (3 | 4) => {
                        floats[(selected - 3) as usize] = self.da2_scale_item_power(
                            item,
                            property,
                            power as f32,
                            &mut vec![property],
                        );
                    }
                    _ => {}
                }
                Some(crate::world::Effect {
                    ty,
                    creator: Some(item),
                    ints,
                    floats,
                    // Native 0x56f0a2: the internal lifetime for an equipped
                    // item's effect is 4, distinct from script permanent (3).
                    duration_type: 4,
                    ..Default::default()
                })
            })
            .collect()
    }

    pub(crate) fn da2_item_property_ids(&self, item: ObjectId, desired: i32) -> Vec<i32> {
        if !self
            .world
            .get(item)
            .is_some_and(|o| o.kind == ObjectKind::Item)
        {
            return Vec::new();
        }
        let mut ids = Vec::new();
        for (id, _) in self.item_properties(item) {
            if (desired == -1 || self.da2_item_property_type(id) == desired) && !ids.contains(&id) {
                ids.push(id);
            }
        }
        ids
    }

    pub(crate) fn da2_item_property_type(&self, property: i32) -> i32 {
        self.world.tables.get(12001, "").map_or(0, |t| {
            crate::engine::cell_int(t.cell_by_hash(0x12a0_2374, i64::from(property)))
        })
    }

    pub(crate) fn da2_item_property_power(
        &self,
        item: ObjectId,
        property: i32,
        scaled: bool,
    ) -> f32 {
        if !self
            .world
            .get(item)
            .is_some_and(|o| o.kind == ObjectKind::Item)
        {
            return 0.;
        }
        self.da2_item_power(item, property, scaled, &mut Vec::new())
    }

    fn da2_item_power(
        &self,
        item: ObjectId,
        property: i32,
        scaled: bool,
        stack: &mut Vec<i32>,
    ) -> f32 {
        let Some((_, power)) = self
            .item_properties(item)
            .into_iter()
            .find(|(p, _)| *p == property)
        else {
            return 0.;
        };
        if !scaled {
            return power as f32;
        }
        // Malformed metadata must not recurse forever through the damage-power mode.
        if stack.contains(&property) {
            return 0.;
        }
        stack.push(property);
        let result = self.da2_scale_item_power(item, property, power as f32, stack);
        stack.pop();
        result
    }

    fn da2_item_power_before_parameter(
        &self,
        item: ObjectId,
        property: i32,
        power: f32,
        stack: &mut Vec<i32>,
    ) -> f32 {
        let Some(table) = self.world.tables.get(12001, "") else {
            return 0.;
        };
        if !table.rows.contains_key(&i64::from(property)) {
            return 0.;
        }
        let row = i64::from(property);
        let get = |hash| number(table.cell_by_hash(hash, row)).unwrap_or(0.);
        let o = self.world.get(item).unwrap();
        let field = |name| o.fields.get(name).copied().unwrap_or(0);
        let level = field("Level");
        let mut value = match get(0xe78b_4e2e) as i32 {
            1 => {
                // 0x697b90: select a column in optimal_values through property_functions.
                let selector = self
                    .world
                    .tables
                    .get(371, "")
                    .and_then(|t| number(t.cell_by_hash(0x1d78_b8e3, get(0x4c58_33f7) as i64)))
                    .unwrap_or(-1.) as i32;
                let columns = [0x73a8_6afe, 0xe337_46bf, 0xf9fc_7170, 0x6e1d_17ea];
                usize::try_from(selector)
                    .ok()
                    .and_then(|i| columns.get(i))
                    .and_then(|&column| {
                        self.world
                            .tables
                            .get(405, "")
                            .and_then(|t| number(t.cell_by_hash(column, i64::from(level))))
                    })
                    .unwrap_or(0.)
            }
            2 => {
                // 0x545aa0: first property of damage type 5, evaluated through
                // 0x54d7d0 with the same item's level and type/material factors.
                self.item_properties(item)
                    .into_iter()
                    .find(|(p, _)| self.da2_item_property_type(*p) == 5)
                    .map_or(0., |(p, _)| self.da2_item_power(item, p, true, stack))
                    * get(0x0326_f210)
            }
            _ => {
                (get(0x5daa_9dea) * level as f32 + get(0x7687_ce29)) * level as f32
                    + get(0x6f9c_ff68)
            }
        } * power;
        if get(0x3da7_34c4) == 0. {
            let armor = get(0x12a0_2374) as i32 == 3;
            let column = if armor { 0x42f2_09a3 } else { 0xdafb_8bab };
            let base = self
                .world
                .tables
                .get(12000, "")
                .and_then(|t| number(t.cell_by_hash(column, i64::from(field("BaseItem")))))
                .unwrap_or(1.);
            let material = if armor {
                self.world
                    .tables
                    .get(12002, "")
                    .and_then(|t| number(t.cell_by_hash(0xea24_6395, i64::from(field("Material")))))
                    .unwrap_or(1.)
            } else {
                1.
            };
            value *= base * material;
        }
        value
    }

    fn da2_scale_item_power(
        &self,
        item: ObjectId,
        property: i32,
        power: f32,
        stack: &mut Vec<i32>,
    ) -> f32 {
        let Some(table) = self.world.tables.get(12001, "") else {
            return 0.;
        };
        let row = i64::from(property);
        if !table.rows.contains_key(&row) {
            return 0.;
        }
        let get = |hash| number(table.cell_by_hash(hash, row)).unwrap_or(0.);
        let mut value = self.da2_item_power_before_parameter(item, property, power, stack);
        let kind = get(0x3600_1b03) as i32;
        value = match kind {
            1 | 2 => {
                let column = if kind == 1 { 0xa363_0beb } else { 0xba78_3aaa };
                let parameter = crate::engine::cell_int(table.cell_by_hash(column, row));
                (f64::from(value) * f64::from(parameter)) as f32
            }
            3 => value * get(0x8619_84a9),
            4 => value * get(0x9f02_b5e8),
            _ => value,
        };
        if matches!(kind, 3 | 4) && get(0x6e49_06c5) != 0. {
            // 0x52fb40 rounds absolute magnitude, with a minimum magnitude of one.
            let rounded = if value.abs() < 1. {
                1.
            } else {
                (value.abs() + 0.5).floor()
            };
            value = if value < 0. { -rounded } else { rounded };
        }
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::m2da::Tables;
    use std::sync::Arc;
    use thedas_script::EngineTable;
    use thedas_vfs::Vfs;

    fn sim() -> (Sim, ObjectId) {
        let mut s = Sim::new(
            Arc::new(Vfs::new()),
            Arc::new(EngineTable::parse(b"").unwrap()),
            1,
        );
        let tables = Tables::empty();
        let cols = [
            "#12a02374",
            "#e78b4e2e",
            "#5daa9dea",
            "#7687ce29",
            "#6f9cff68",
            "#36001b03",
            "#861984a9",
            "#6e4906c5",
            "#3da734c4",
            "#4c5833f7",
            "#0326f210",
            "#a3630beb",
        ];
        let row = |ty, mode, a, b, c, kind, coeff, rounded, skip, curve, stat, int0| {
            vec![
                Cell::Int(ty),
                Cell::Int(mode),
                Cell::Float(a),
                Cell::Float(b),
                Cell::Float(c),
                Cell::Int(kind),
                Cell::Float(coeff),
                Cell::Int(rounded),
                Cell::Int(skip),
                Cell::Int(curve),
                Cell::Float(stat),
                Cell::Int(int0),
            ]
        };
        tables.insert(
            "#12001",
            &cols,
            vec![
                (100, row(10, 0, 0., 0., 0., 0, 0., 0, 0, 0, 0., 0)),
                (101, row(5, 0, 1., 2., 3., 3, 0.5, 1, 0, 0, 0., 0)),
                (102, row(3, 0, 0., 0., 2., 1, 0., 0, 0, 0, 0., 4)),
                (104, row(7, 0, 0., 0., -0.2, 3, 1., 1, 1, 0, 0., 0)),
                (105, row(7, 1, 0., 0., 0., 0, 0., 0, 1, 33, 0., 0)),
                (106, row(7, 2, 0., 0., 0., 0, 0., 0, 1, 0, 0.25, 0)),
                (107, row(5, 2, 0., 0., 0., 0, 0., 0, 1, 0, 1., 0)),
            ],
        );
        tables.insert(
            "#12000",
            &["#dafb8bab", "#42f209a3"],
            vec![(10, vec![Cell::Float(1.), Cell::Float(3.)])],
        );
        tables.insert("#12002", &["#ea246395"], vec![(20, vec![Cell::Float(0.5)])]);
        tables.insert("#371", &["#1d78b8e3"], vec![(33, vec![Cell::Int(2)])]);
        tables.insert("#405", &["#f9fc7170"], vec![(4, vec![Cell::Float(7.)])]);
        s.world.tables = Arc::new(tables);
        let item = s.world.spawn(ObjectKind::Item, "weapon");
        let o = s.world.get_mut(item).unwrap();
        o.fields.extend([
            ("Level".into(), 4),
            ("BaseItem".into(), 10),
            ("Material".into(), 20),
        ]);
        o.item_properties = Some(vec![
            (100, 1),
            (101, 2),
            (102, 1),
            (103, 7),
            (104, 1),
            (105, 2),
            (106, 3),
        ]);
        (s, item)
    }

    #[test]
    fn scaled_power_uses_item_level_type_material_and_data_parameter() {
        let (s, item) = sim();
        assert_eq!(s.da2_item_property_type(100), 10);
        assert_eq!(s.da2_item_property_type(101), 5);
        assert_eq!(s.da2_item_property_power(item, 101, false), 2.);
        assert_eq!(s.da2_item_property_power(item, 101, true), 27.);
        assert_eq!(s.da2_item_property_power(item, 102, true), 12.);
        assert_eq!(s.da2_item_property_power(item, 104, true), -1.);
        assert_eq!(s.da2_item_property_power(item, 103, true), 0.);
        assert_eq!(s.da2_item_property_power(item, 999, true), 0.);
        assert_eq!(s.da2_item_property_power(999, 101, true), 0.);
    }

    #[test]
    fn power_can_use_a_rating_row_or_the_first_damage_property() {
        let (mut s, item) = sim();
        assert_eq!(s.da2_item_property_power(item, 105, true), 14.);
        assert_eq!(s.da2_item_property_power(item, 106, true), 20.25);
        // A malformed damage property referring to its own damage type terminates.
        s.world.get_mut(item).unwrap().item_properties = Some(vec![(107, 1)]);
        assert_eq!(s.da2_item_property_power(item, 107, true), 0.);
    }

    fn worn_sim() -> (Sim, ObjectId) {
        let table = EngineTable::parse(
            b"int PROPERTY_DEPLETABLE_HEALTH = 7;\nint EFFECT_DURATION_TYPE_INSTANT = 1;\n",
        )
        .unwrap();
        let mut s = Sim::new(Arc::new(Vfs::new()), Arc::new(table), 1);
        s.game = thedas_vfs::Game::DragonAge2;
        let cols = [
            "#12a02374",
            "#3ba07a8a",
            "#a3630beb",
            "#ba783aaa",
            "#861984a9",
            "#9f02b5e8",
            "#36001b03",
            "#7687ce29",
            "#6f9cff68",
            "#6e4906c5",
            "#3da734c4",
        ];
        let row = |kind, effect, property| {
            vec![
                Cell::Int(kind),
                Cell::Int(effect),
                Cell::Int(property),
                Cell::Int(0),
                Cell::Float(0.5),
                Cell::Float(0.25),
                Cell::Int(3),
                Cell::Float(1.),
                Cell::Float(2.),
                Cell::Int(1),
                Cell::Int(1),
            ]
        };
        s.world.tables().insert(
            "#12001",
            &cols,
            vec![
                (700, row(1, 104, 7)),
                (701, row(5, 104, 26)),
                (702, row(2, 104, 7)),
                (703, row(3, 104, 7)),
                (704, row(4, 104, 7)),
                (705, row(6, 0, 7)),
                (
                    706,
                    vec![
                        Cell::Int(1),
                        Cell::Int(104),
                        Cell::Int(2),
                        Cell::Int(16_777_217),
                        Cell::Float(0.5),
                        Cell::Float(0.25),
                        Cell::Int(1),
                        Cell::Float(0.),
                        Cell::Float(1.2),
                        Cell::Int(0),
                        Cell::Int(1),
                    ],
                ),
                (
                    707,
                    vec![
                        Cell::Int(1),
                        Cell::Int(104),
                        Cell::Int(16_777_217),
                        Cell::Int(0),
                        Cell::Float(0.5),
                        Cell::Float(0.25),
                        Cell::Int(1),
                        Cell::Float(0.),
                        Cell::Float(1.000_000_119_209_289_6),
                        Cell::Int(0),
                        Cell::Int(1),
                    ],
                ),
            ],
        );
        let item = s.world.spawn(ObjectKind::Item, "armor");
        let o = s.world.get_mut(item).unwrap();
        o.fields.insert("Level".into(), 4);
        o.item_properties = Some(vec![
            (700, 2),
            (701, 1),
            (702, 1),
            (703, 1),
            (704, 1),
            (705, 1),
        ]);
        (s, item)
    }

    #[test]
    fn worn_da2_properties_build_scaled_effects_from_the_da2_table() {
        let (mut s, item) = worn_sim();
        let effects = s.property_effects(item);
        assert_eq!(effects.len(), 2, "passive and base-item effects only");
        assert_eq!(effects[0].ty, 104);
        assert_eq!(effects[0].ints, [7, 0]);
        assert_eq!(effects[0].floats, [6., 0.25]);
        assert_eq!(effects[1].ints, [26, 0]);
        assert_eq!(effects[1].floats, [3., 0.25]);
        for e in &effects {
            assert_eq!(e.creator, Some(item));
            assert_eq!(e.duration_type, 4);
        }
        // The native effect builder rounds selected integer fields even though
        // the corresponding GetItemPropertyPower query returns the raw float.
        s.world.get_mut(item).unwrap().item_properties = Some(vec![(706, 2)]);
        let raw = s.da2_item_property_power(item, 706, true);
        assert!((raw - 4.8).abs() < 0.00001);
        let e = &s.property_effects(item)[0];
        assert_eq!(e.ints, [5, 16_777_217]);
        assert_eq!(e.floats, [0.5, 0.25]);
        // Native integer-field conversion precedes the query's f32 return.
        s.world.get_mut(item).unwrap().item_properties = Some(vec![(707, 1)]);
        assert_eq!(s.da2_item_property_power(item, 707, true), 16_777_220.);
        assert_eq!(s.property_effects(item)[0].ints, [16_777_219, 0]);
    }

    #[test]
    fn worn_da2_properties_follow_add_remove_and_unequip_without_duplicating() {
        let (mut s, item) = worn_sim();
        s.world.get_mut(item).unwrap().item_properties = Some(vec![(700, 2)]);
        let wearer = s.world.spawn(ObjectKind::Creature, "wearer");
        let o = s.world.get_mut(wearer).unwrap();
        o.properties.insert(7, (100., 100.));
        o.equipment.insert(0, item);
        s.wear(wearer, item, 0);
        assert_eq!(s.world.get(wearer).unwrap().effects.len(), 1);
        assert!(s.add_item_property(item, 701, 1));
        assert!(!s.add_item_property(item, 701, 1));
        assert_eq!(s.world.get(wearer).unwrap().effects.len(), 2);
        // Removal has already edited the live item list before it constructs
        // the old effect payload, so its original entry power must still work.
        assert!(s.remove_item_property(item, 700));
        let effects = &s.world.get(wearer).unwrap().effects;
        assert_eq!(effects.len(), 1);
        assert_eq!(effects[0].ints[0], 26);
        s.take_off(wearer, item, 0);
        assert!(s.world.get(wearer).unwrap().effects.is_empty());
    }

    #[test]
    fn replacing_da2_template_equipment_discards_its_effects_and_optional_backpack() {
        let (mut s, item) = worn_sim();
        let wearer = s.world.spawn(ObjectKind::Creature, "wearer");
        let spare = s.world.spawn(ObjectKind::Item, "spare");
        let o = s.world.get_mut(wearer).unwrap();
        o.properties.insert(7, (100., 100.));
        // An older malformed snapshot can wear an item absent from inventory.
        o.inventory = vec![spare];
        o.equipment.insert(0, item);
        s.wear(wearer, item, 0);
        assert_eq!(s.world.get(wearer).unwrap().effects.len(), 2);
        s.discard_da2_template_inventory(wearer, false);
        let o = s.world.get(wearer).unwrap();
        assert_eq!(o.inventory, [spare]);
        assert!(o.equipment.is_empty());
        assert!(o.effects.is_empty());
        assert!(s.world.get(item).is_none());
        assert!(s.world.get(spare).is_some());
        s.discard_da2_template_inventory(wearer, true);
        assert!(s.world.get(wearer).unwrap().inventory.is_empty());
        assert!(s.world.get(spare).is_none());
    }
}
