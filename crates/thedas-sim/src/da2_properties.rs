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
    /// DragonAge2.exe 0x55ff60 selects a combat_style record, whose id is returned
    /// by GetWeaponStyle (0x4dd1e0 -> 0x52ec60). These are DA2 ids, not DAO's enum.
    pub(crate) fn da2_combat_style(&self, id: ObjectId) -> i32 {
        let Some(o) = self
            .world
            .get(id)
            .filter(|o| o.kind == crate::world::ObjectKind::Creature)
        else {
            return 0;
        };
        let Some(styles) = self.world.tables.get(300, "") else {
            return 0;
        };
        let valid = |style: i32| style > 0 && styles.rows.contains_key(&i64::from(style));
        let package = o.fields.get("Package").copied().unwrap_or(0);
        let override_style = self.da2_stat_cell(163, 0x9ebe_3727, package).unwrap_or(0.) as i32;
        if valid(override_style) {
            return override_style;
        }
        // The native inventory query takes hand 0, then hand 1 only when absent.
        let item = ["INVENTORY_SLOT_MAIN", "INVENTORY_SLOT_OFFHAND"]
            .into_iter()
            .find_map(|slot| {
                o.equipment
                    .get(&crate::sim::constant(&self.table, slot))
                    .and_then(|id| self.world.get(*id))
            });
        if let Some(item) = item {
            let row = item.fields.get("BaseItem").copied().unwrap_or(0);
            let style = self.da2_stat_cell(12000, 0x93b9_42d7, row).unwrap_or(0.) as i32;
            if valid(style) {
                return style;
            }
        }
        styles
            .order
            .iter()
            .skip(1)
            .find_map(|&row| {
                let ability = number(styles.cell_by_hash(0x5d74_e7ea, row)).unwrap_or(0.) as i32;
                (ability != 0 && o.abilities.contains(&ability)).then_some(row as i32)
            })
            .unwrap_or(0)
    }

    fn da2_modifier_state(&self, id: ObjectId) -> i32 {
        // 0x52f7c0. Neutral/unknown 255 must never enable every conditional mask.
        match self.da2_combat_style(id) {
            1 => 8,
            2 | 5 | 10 => 16,
            3 => 4,
            4 => 2,
            _ => 255,
        }
    }

    fn da2_stat_cell(&self, table: i32, column: u32, row: i32) -> Option<f32> {
        self.world
            .tables
            .get(table, "")
            .and_then(|t| number(t.cell_by_hash(column, i64::from(row))))
    }

    /// Repair snapshots made before DA2 race/stat initialization. Derived ratings remain
    /// computed on query; do not bake their totals into bases or replay SPAWN/hire events
    /// (which also change equipment, commands, locals and plot state).
    pub(crate) fn repair_da2_stats(&mut self) {
        use crate::world::ObjectKind;
        if self.game != thedas_vfs::Game::DragonAge2 {
            return;
        }
        let ids: Vec<_> = self
            .world
            .objects
            .values()
            .filter(|o| o.kind == ObjectKind::Creature)
            .map(|o| o.id)
            .collect();
        for &id in &ids {
            let o = self.world.get(id).unwrap();
            let race = o.fields.get("Race").copied().unwrap_or(0);
            let package = o.fields.get("Package").copied().unwrap_or(0);
            let class = self.da2_stat_cell(163, 0xe02e_9b49, package).unwrap_or(0.) as i32;
            // AS_ScaleCreature's PC path requires both a PC race and a PC class. A
            // monster's zero attributes are intentional; a PC's six zero bases are not.
            if o.rank <= 0
                || o.fields.get("Combatant").copied().unwrap_or(0) != 0
                || self.da2_stat_cell(5, 0xb6bc_e25c, race) != Some(1.)
                || self.da2_stat_cell(3, 0x7612_4935, class) != Some(1.)
                || !(1..=6).all(|p| o.properties.get(&p).is_none_or(|v| v.0 <= 0.))
            {
                continue;
            }
            let fractions: Vec<_> = [7, 10]
                .into_iter()
                .map(|p| {
                    let current = o.properties.get(&p).map_or(0., |v| v.1);
                    let max = self.property_total(id, p);
                    (
                        p,
                        if max > 0. {
                            (current / max).clamp(0., 1.)
                        } else {
                            1.
                        },
                    )
                })
                .collect();
            // AS_InitProperties: defaults in merged properties (98), not its minima.
            let defaults: Vec<_> = self
                .world
                .tables
                .get(98, "")
                .map(|t| {
                    t.order
                        .iter()
                        .filter_map(|&p| {
                            Some((
                                i32::try_from(p).ok()?,
                                number(t.cell_by_hash(0xaacd_a048, p))?,
                            ))
                        })
                        .collect()
                })
                .unwrap_or_default();
            if defaults.is_empty() {
                continue;
            }
            let mut abilities = Vec::new();
            // AS_InitProperties grants the party basics only to the hero/followers.
            if Some(id) == self.world.hero
                || o.fields.get("FollowerState").copied().unwrap_or(0) != 0
            {
                abilities.extend([100, 101]);
            }
            for column in [0x233f_a0bd, 0xa19d_eb47, 0x8ab0_b884] {
                if let Some(a) = self.da2_stat_cell(5, column, race) {
                    abilities.push(a as i32);
                }
            }
            if let Some(a) = self.da2_stat_cell(3, 0x5193_4b39, class) {
                abilities.push(a as i32);
            }
            // AS_SetClass: the package may replace the class's starter ability list.
            let list = self
                .da2_stat_cell(163, 0xe4fc_3a39, package)
                .filter(|n| *n > 0.)
                .or_else(|| self.da2_stat_cell(3, 0xc9c3_0b3c, class))
                .unwrap_or(0.) as i32;
            if let Some(t) = self.world.tables.get(list, "").filter(|_| list > 0) {
                abilities.extend(t.order.iter().filter_map(|&a| i32::try_from(a).ok()));
            }
            abilities.retain(|&a| self.ability_known(a));
            let primary = self.da2_stat_cell(3, 0xb59a_e9c3, class);
            let old_xp = self.property_total(id, 35);
            let xp = if old_xp > 0. {
                old_xp
            } else {
                let fixed = self.da2_stat_cell(163, 0x6acc_4c10, package).unwrap_or(-1.);
                if fixed >= 0. {
                    fixed
                } else {
                    self.world.hero.map_or(0., |h| self.property_total(h, 35))
                        + self.da2_stat_cell(163, 0xd777_aa10, package).unwrap_or(0.)
                }
                .max(0.)
            };
            let level = self.da2_level_for_xp(xp);
            let o = self.world.get_mut(id).unwrap();
            for (p, base) in defaults {
                if (1..=6).contains(&p)
                    || [7, 10, 50].contains(&p)
                    || !o.properties.contains_key(&p)
                {
                    o.properties.insert(p, (base, 0.));
                }
            }
            o.abilities.extend(abilities);
            if let Some(primary) = primary {
                o.properties.insert(50, (primary, 0.));
            }
            if o.properties.get(&35).is_none_or(|v| v.0 <= 0.) {
                o.properties.insert(35, (xp, 0.));
            }
            if o.properties.get(&36).is_none_or(|v| v.0 <= 0.) {
                o.properties.insert(36, (level as f32, 0.));
            }
            for (p, fraction) in fractions {
                let current = fraction * self.property_total(id, p);
                if let Some(v) = self.world.get_mut(id).unwrap().properties.get_mut(&p) {
                    v.1 = current;
                }
            }
        }
        for id in ids {
            let o = self.world.get(id).unwrap();
            // Zero before first perception is the normal NPC spawn state. Only repair
            // actors whose saved perception/combat state proves that step already ran.
            if !o.active
                || o.dead
                || o.area
                    != self
                        .world
                        .controlled()
                        .and_then(|h| self.world.get(h)?.area)
                || o.rank <= 0
                || o.rank == 100
                || o.fields.get("Combatant").copied().unwrap_or(0) != 0
                || self.property_total(id, 36) > 0.
                || !(o.combat.in_combat || o.combat.perceived.iter().any(|&p| self.hostile(id, p)))
            {
                continue;
            }
            if let Some(level) = self.da2_encounter_level(id) {
                self.world
                    .get_mut(id)
                    .unwrap()
                    .properties
                    .insert(36, (level as f32, 0.));
            }
        }
    }

    fn da2_level_for_xp(&self, xp: f32) -> i32 {
        self.world
            .tables
            .get(164, "")
            .map(|t| {
                t.order
                    .iter()
                    .filter(|&&row| {
                        row > 0
                            && number(t.cell_by_hash(0xa0f0_2e99, row))
                                .is_some_and(|n| n >= 0. && n <= xp)
                    })
                    .copied()
                    .max()
                    .unwrap_or(1) as i32
            })
            .unwrap_or(1)
    }

    /// rules_core's post-spawn perception path (0x2b0c -> 0x699c -> 0x6a2e):
    /// party level rounded down below half the XP interval, area bounds, APR ceiling.
    /// Used only to repair a missing value in old saves; ordinary play runs that script.
    fn da2_encounter_level(&self, id: ObjectId) -> Option<i32> {
        let leader = self.world.controlled()?; // GetPartyLeader, as in rules_core.
        let area = self.world.get(leader)?.area?;
        let key = crate::engine::local_name(self, area, &[thedas_script::vm::Value::Int(37)], 0);
        let group = self
            .world
            .get(area)?
            .locals
            .ints
            .get(&key)
            .copied()
            .unwrap_or(0);
        let min = (self.da2_stat_cell(225, 0x7443_cc93, group)? as i32).max(1);
        let max = self.da2_stat_cell(225, 0xa43f_9ab9, group)? as i32;
        let mut level = self.property_total(leader, 36) as i32;
        let lo = self.da2_stat_cell(164, 0xa0f0_2e99, level)? as i32;
        let hi = self.da2_stat_cell(164, 0xa0f0_2e99, level.saturating_add(1))? as i32;
        if (hi - lo) / 2 > self.property_total(leader, 35) as i32 - lo {
            level -= 1;
        }
        level = level.min(max).max(min);
        if self.da2_stat_cell(225, 0x75c4_4589, group).unwrap_or(0.) == 0. {
            let cap = self
                .da2_stat_cell(2, 0xd0c4_5555, self.world.get(id)?.appearance)
                .unwrap_or(0.) as i32;
            if cap > 0 {
                level = level.min(cap);
            }
        }
        Some(level)
    }

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
        let state = self.da2_modifier_state(id);
        for &ability in &o.abilities {
            for column in columns {
                // 0x56e067 resolves the modifier row through the upgraded int getter.
                let row = crate::engine::upgradable_ability_int(self, id, column, ability);
                if row == 0 {
                    break;
                }
                let row = row as i64;
                if number(modifiers.cell_by_hash(0x5012_0e24, row)).unwrap_or(0.0) as i32
                    != property
                {
                    continue;
                }
                let flags = number(modifiers.cell_by_hash(0xca5f_bbe9, row)).unwrap_or(0.0) as i32;
                if flags == 0 || (matches!(state, 1 | 2 | 4 | 8 | 16) && flags & state != 0) {
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
                let mut bonus = attr(4) - 10.0;
                // Devious Harm (0x5bdc90): its Power multiplies cunning's contribution.
                if o.abilities.contains(&210040) {
                    let power = self
                        .world
                        .tables
                        .get(1, "")
                        .and_then(|t| number(t.cell_by_hash(0x15bd_f19c, 210040)))
                        .unwrap_or(0.0);
                    bonus *= 1.0 + power * 100.0;
                }
                bonus.max(0.0)
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
    fn combat_styles_prefer_package_then_first_hand_then_learned_style() {
        let (mut s, id) = sim();
        s.table = Arc::new(
            EngineTable::parse(b"int INVENTORY_SLOT_MAIN = 0;\nint INVENTORY_SLOT_OFFHAND = 1;\n")
                .unwrap(),
        );
        s.world.tables.insert(
            "#300",
            &["#5d74e7ea"],
            vec![
                (0, vec![Cell::Int(0)]),
                (1, vec![Cell::Int(201000)]),
                (2, vec![Cell::Int(202000)]),
                (3, vec![Cell::Int(102000)]),
                (4, vec![Cell::Int(101000)]),
                (5, vec![Cell::Int(301000)]),
                (10, vec![Cell::Int(202000)]),
                (100, vec![Cell::Int(0)]),
            ],
        );
        s.world.tables.insert(
            "#163",
            &["#9ebe3727"],
            vec![(1, vec![Cell::Int(100)]), (2, vec![Cell::Int(999)])],
        );
        s.world.tables.insert(
            "#12000",
            &["#93b942d7"],
            vec![
                (1001, vec![Cell::Int(3)]),
                (1200, vec![Cell::Int(5)]),
                (1300, vec![Cell::Int(10)]),
            ],
        );
        s.world
            .get_mut(id)
            .unwrap()
            .abilities
            .extend([201000, 202000]);
        assert_eq!(s.weapon_style(id), 1, "first learned style in table order");
        s.world.get_mut(id).unwrap().abilities.remove(&201000);
        assert_eq!(
            s.weapon_style(id),
            2,
            "the later style10 shares the bow ability"
        );
        let main = s.world.spawn(ObjectKind::Item, "greatsword");
        let off = s.world.spawn(ObjectKind::Item, "staff");
        s.world
            .get_mut(main)
            .unwrap()
            .fields
            .insert("BaseItem".into(), 1001);
        s.world
            .get_mut(off)
            .unwrap()
            .fields
            .insert("BaseItem".into(), 1200);
        s.world
            .get_mut(id)
            .unwrap()
            .equipment
            .extend([(0, main), (1, off)]);
        assert_eq!(s.weapon_style(id), 3, "the first present hand wins");
        s.world
            .get_mut(id)
            .unwrap()
            .fields
            .insert("Package".into(), 1);
        assert_eq!(s.weapon_style(id), 100, "native NPC package override");
        s.world
            .get_mut(id)
            .unwrap()
            .fields
            .insert("Package".into(), 2);
        assert_eq!(s.weapon_style(id), 3, "invalid overrides fall through");
        s.world.get_mut(id).unwrap().equipment.remove(&0);
        assert_eq!(s.weapon_style(id), 5, "offhand only when main is absent");
        s.world
            .get_mut(off)
            .unwrap()
            .fields
            .insert("BaseItem".into(), 1300);
        assert_eq!(
            s.weapon_style(id),
            10,
            "an item can select style10 directly"
        );
        s.world
            .get_mut(off)
            .unwrap()
            .fields
            .insert("BaseItem".into(), 999);
        assert_eq!(
            s.weapon_style(id),
            2,
            "unknown item style uses learned fallback"
        );
        s.world.get_mut(id).unwrap().abilities.clear();
        assert_eq!(s.weapon_style(id), 0);
        assert_eq!(s.weapon_style(0x7f000000), 0);
    }

    #[test]
    fn conditional_passives_follow_da2_styles_and_never_match_neutral_255() {
        let (mut s, id) = sim();
        s.world.tables.insert(
            "#300",
            &["#5d74e7ea"],
            [0, 1, 2, 3, 4, 5, 10, 100]
                .into_iter()
                .map(|i| (i, vec![Cell::Int(0)]))
                .collect(),
        );
        s.world.tables.insert(
            "#1",
            &[
                "#b647b38b",
                "#9d6ae048",
                "#8471d109",
                "#cb3047ce",
                "#d22b768f",
            ],
            vec![(
                100,
                vec![
                    Cell::Int(11),
                    Cell::Int(12),
                    Cell::Int(13),
                    Cell::Int(14),
                    Cell::Int(15),
                ],
            )],
        );
        s.world.tables.insert(
            "#117",
            &["#50120e24", "#28d12c72", "#ca5fbbe9"],
            vec![
                (11, vec![Cell::Int(15), Cell::Float(10.), Cell::Int(4)]),
                (12, vec![Cell::Int(15), Cell::Float(7.), Cell::Int(8)]),
                (13, vec![Cell::Int(15), Cell::Float(2.), Cell::Int(255)]),
                (14, vec![Cell::Int(15), Cell::Float(4.), Cell::Int(0)]),
                (15, vec![Cell::Int(15), Cell::Float(99.), Cell::Int(1)]),
            ],
        );
        s.world.get_mut(id).unwrap().abilities.insert(100);
        s.world.get_mut(id).unwrap().properties.insert(15, (3., 0.));
        s.world
            .get_mut(id)
            .unwrap()
            .fields
            .insert("Package".into(), 1);
        for (style, expected) in [
            (1, 16.),
            (2, 9.),
            (3, 19.),
            (4, 9.),
            (5, 9.),
            (10, 9.),
            (100, 7.),
            (0, 7.),
        ] {
            s.world
                .tables
                .insert("#163", &["#9ebe3727"], vec![(1, vec![Cell::Int(style)])]);
            let before = s.save_json().unwrap();
            assert_eq!(s.property_total(id, 15), expected, "style {style}");
            assert_eq!(
                s.save_json().unwrap(),
                before,
                "queries never bake in bonuses"
            );
        }
        s.game = Game::Origins;
        assert_eq!(s.da2_ability_property_bonus(id, 15), 0.);
    }

    #[test]
    fn learned_upgrades_select_the_passives_modifier_row() {
        let (mut s, id) = sim();
        s.world.tables.insert(
            "#1",
            &["#b647b38b", "#d300f4d9"],
            vec![
                (0, vec![Cell::Int(1)]),
                (100, vec![Cell::Int(10), Cell::Int(101)]),
                (101, vec![Cell::Int(1)]),
            ],
        );
        s.world.tables.insert(
            "#117",
            &["#50120e24", "#28d12c72", "#ca5fbbe9"],
            vec![
                (10, vec![Cell::Int(15), Cell::Float(5.), Cell::Int(0)]),
                (11, vec![Cell::Int(15), Cell::Float(9.), Cell::Int(0)]),
            ],
        );
        s.world.get_mut(id).unwrap().properties.insert(15, (3., 0.));
        s.world.get_mut(id).unwrap().abilities.insert(100);
        assert_eq!(s.property_total(id, 15), 8.);
        s.world.get_mut(id).unwrap().abilities.insert(101);
        assert_eq!(s.property_total(id, 15), 12.);
        s.world.get_mut(id).unwrap().abilities.remove(&101);
        assert_eq!(s.property_total(id, 15), 8.);
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
    fn devious_harm_power_is_a_fraction_of_each_cunning_point() {
        let (mut s, id) = sim();
        s.world.tables.insert(
            "#1",
            &["#15bdf19c"],
            vec![(210040, vec![Cell::Float(0.02)])],
        );
        s.world.get_mut(id).unwrap().properties.insert(4, (12., 0.));
        assert_eq!(s.property_total(id, 16), 2.);
        s.world.get_mut(id).unwrap().abilities.insert(210040);
        assert!((s.property_total(id, 16) - 6.).abs() < 0.00001);
        s.world.get_mut(id).unwrap().properties.insert(4, (8., 0.));
        assert_eq!(s.property_total(id, 16), 0.);
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

    #[test]
    fn old_da2_pc_stats_rebuild_from_tables_once_without_replaying_events() {
        let (mut s, hero) = sim();
        s.world.hero = Some(hero);
        s.world
            .get_mut(hero)
            .unwrap()
            .properties
            .insert(35, (400., 0.));
        s.world.tables.insert(
            "#98",
            &[
                "#baf55bcf",
                "#f00a1048",
                "#98377833",
                "#b48bff62",
                "#aacda048",
            ],
            [1, 2, 3, 4, 5, 6, 7, 10, 13, 24, 36, 50]
                .into_iter()
                .map(|p| {
                    let (ty, deriv, default) = match p {
                        1..=6 => (1, 0, 10.),
                        7 => (3, 1, 50.),
                        10 => (3, 2, 50.),
                        13 => (1, 3, 0.),
                        24 => (1, 5, 0.),
                        _ => (2, 0, 0.),
                    };
                    (
                        p,
                        vec![
                            Cell::Int(ty),
                            Cell::Int(deriv),
                            Cell::Float(0.),
                            Cell::Float(10000.),
                            Cell::Float(default),
                        ],
                    )
                })
                .collect(),
        );
        s.world.tables.insert(
            "#163",
            &["#e02e9b49", "#e4fc3a39", "#6acc4c10", "#d777aa10"],
            vec![(
                42,
                vec![Cell::Int(8), Cell::Int(555), Cell::Int(-1), Cell::Int(-100)],
            )],
        );
        s.world.tables.insert(
            "#5",
            &["#b6bce25c", "#233fa0bd"],
            vec![(7, vec![Cell::Int(1), Cell::Int(200)])],
        );
        s.world.tables.insert(
            "#3",
            &["#76124935", "#51934b39", "#b59ae9c3"],
            vec![(8, vec![Cell::Int(1), Cell::Int(300), Cell::Int(3)])],
        );
        s.world
            .tables
            .insert("#555", &["ID"], vec![(400, vec![Cell::Int(400)])]);
        s.world.tables.insert(
            "#1",
            &["#b647b38b"],
            [100, 101, 200, 300, 400]
                .into_iter()
                .map(|a| (a, vec![Cell::Int(if a == 300 { 800 } else { 0 })]))
                .collect(),
        );
        s.world.tables.insert(
            "#117",
            &["#50120e24", "#28d12c72", "#ca5fbbe9"],
            vec![(800, vec![Cell::Int(3), Cell::Float(3.), Cell::Int(0)])],
        );
        s.world.tables.insert(
            "#164",
            &["#a0f02e99"],
            vec![(1, vec![Cell::Int(0)]), (2, vec![Cell::Int(500)])],
        );
        let id = s.world.spawn(ObjectKind::Creature, "old follower");
        let o = s.world.get_mut(id).unwrap();
        o.rank = 100;
        o.fields.extend([
            ("Race".into(), 7),
            ("Package".into(), 42),
            ("FollowerState".into(), 1),
        ]);
        o.properties
            .extend([(7, (100., 40.)), (36, (0., 0.)), (51, (12., 0.))]);
        o.abilities.insert(400);
        o.locals.ints.insert("already_hired".into(), 1);
        let recruit = s.world.spawn(ObjectKind::Creature, "unhired recruit");
        let o = s.world.get_mut(recruit).unwrap();
        o.rank = 1;
        o.fields
            .extend([("Race".into(), 7), ("Package".into(), 42)]);
        let ambient = s.world.spawn(ObjectKind::Creature, "noncombatant");
        let o = s.world.get_mut(ambient).unwrap();
        o.rank = 100;
        o.fields.extend([
            ("Race".into(), 7),
            ("Package".into(), 42),
            ("Combatant".into(), 2),
        ]);
        let rng = serde_json::to_value(&s.world.rng).unwrap();
        let mut old = s.save_game();
        old.revision = 8;
        s.load_game(old).unwrap();
        assert_eq!(s.property_total(id, 3), 13.);
        assert_eq!(s.property_total(id, 13), 60.);
        assert_eq!(s.property_total(id, 35), 300.);
        assert_eq!(s.property_total(id, 36), 1.);
        assert_eq!(s.property_total(id, 51), 12.);
        assert_eq!(s.world.get(id).unwrap().properties[&7], (50., 40.));
        assert_eq!(s.world.get(id).unwrap().locals.ints["already_hired"], 1);
        assert_eq!(s.world.get(id).unwrap().abilities.len(), 5);
        assert_eq!(
            s.world.get(recruit).unwrap().abilities,
            [200, 300, 400].into_iter().collect(),
            "unhired PC-class actors do not receive party-only abilities"
        );
        assert!(s.world.get(ambient).unwrap().properties.is_empty());
        assert_eq!(serde_json::to_value(&s.world.rng).unwrap(), rng);
        let properties = s.world.get(id).unwrap().properties.clone();
        let save = s.save_json().unwrap();
        s.load_json(&save).unwrap();
        assert_eq!(s.world.get(id).unwrap().properties, properties);
        assert_eq!(s.world.get(id).unwrap().abilities.len(), 5);
        // An intact PC and Origins' saves must not be reconstructed.
        s.world.get_mut(id).unwrap().properties.insert(3, (19., 0.));
        s.repair_da2_stats();
        assert_eq!(s.world.get(id).unwrap().properties[&3].0, 19.);
        s.game = Game::Origins;
        s.world.get_mut(id).unwrap().properties.clear();
        s.repair_da2_stats();
        assert!(s.world.get(id).unwrap().properties.is_empty());
    }

    #[test]
    fn old_npc_levels_follow_area_bounds_xp_rounding_and_appearance_cap() {
        let (mut s, hero) = sim();
        let area = s.world.spawn(ObjectKind::Area, "area");
        s.world
            .get_mut(area)
            .unwrap()
            .locals
            .ints
            .insert("37".into(), 42);
        s.world.hero = Some(hero);
        let o = s.world.get_mut(hero).unwrap();
        o.area = Some(area);
        o.properties.extend([(35, (2000., 0.)), (36, (5., 0.))]);
        s.world.tables.insert(
            "#164",
            &["#a0f02e99"],
            vec![(5, vec![Cell::Int(1800)]), (6, vec![Cell::Int(2400)])],
        );
        s.world.tables.insert(
            "#225",
            &["#7443cc93", "#a43f9ab9", "#75c44589"],
            vec![(42, vec![Cell::Int(3), Cell::Int(9), Cell::Int(0)])],
        );
        s.world
            .tables
            .insert("#2", &["#d0c45555"], vec![(12, vec![Cell::Int(8)])]);
        let npc = s.world.spawn(ObjectKind::Creature, "npc");
        let o = s.world.get_mut(npc).unwrap();
        o.area = Some(area);
        o.appearance = 12;
        o.rank = 1;
        assert_eq!(s.da2_encounter_level(npc), Some(4));
        // A not-yet-perceived NPC's zero level is intentional.
        s.repair_da2_stats();
        assert_eq!(s.property_total(npc, 36), 0.);
        s.world.get_mut(npc).unwrap().combat.in_combat = true;
        s.repair_da2_stats();
        assert_eq!(s.property_total(npc, 36), 4.);
        assert_eq!(s.property_total(npc, 1), 0., "monster attributes stay zero");
        s.world
            .get_mut(hero)
            .unwrap()
            .properties
            .insert(35, (2100., 0.));
        assert_eq!(s.da2_encounter_level(npc), Some(5));
        let leader = s.world.spawn(ObjectKind::Creature, "controlled follower");
        let o = s.world.get_mut(leader).unwrap();
        o.area = Some(area);
        o.properties.extend([(35, (2000., 0.)), (36, (5., 0.))]);
        s.world.get_mut(hero).unwrap().followers.push(leader);
        s.world.controlled = Some(leader);
        assert_eq!(
            s.da2_encounter_level(npc),
            Some(4),
            "scaling follows GetPartyLeader rather than always reading the hero"
        );
        s.world.controlled = None;
        s.world
            .tables
            .insert("#2", &["#d0c45555"], vec![(12, vec![Cell::Int(2)])]);
        assert_eq!(s.da2_encounter_level(npc), Some(2));
        s.world.tables.insert(
            "#225",
            &["#7443cc93", "#a43f9ab9", "#75c44589"],
            vec![(42, vec![Cell::Int(7), Cell::Int(9), Cell::Int(1)])],
        );
        assert_eq!(s.da2_encounter_level(npc), Some(7));
        s.repair_da2_stats();
        assert_eq!(s.property_total(npc, 36), 4., "preserve an assigned level");
    }
}
