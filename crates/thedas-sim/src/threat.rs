//! Threat tables, as DAOrigins.exe keeps them: the AI scripts (`ai_threat_h`) add threat
//! with `UpdateThreatTable` and pick targets from the table, while the engine itself, every
//! creature update (0x6ab620, called from 0x6be7ee), lets the threat fall off and adds a
//! proximity term:
//!
//! - each entry's threat moves by the creature's `Threat_Decrease_Rate` total (property 40,
//!   −0.5 for every creature the saves show) a second, never below 0 (0x741ac0);
//! - each entry gets a proximity term: an enemy within 60 m (0xc0ca70) of the creature,
//!   measured across the ground (the height difference is dropped), adds 100 (0xc0ca6c)
//!   times (60 − distance) / 60, one farther away 0;
//! - the table is ordered by threat plus proximity, highest first, and `GetThreatEnemy`,
//!   `GetThreatValueByIndex` and `GetThreatValueByObjectID` see that order and that sum
//!   (0x740d10, 0x740fa0).
//!
//! `UpdateThreatTable` (0x741d80) adds to an enemy's threat (never below 0); a new enemy
//! comes in with its threat (never below 0) and no proximity yet. A table of five takes a
//! new enemy only if its threat is not below the fifth entry's total; past five, the entry
//! at the old size's last index after sorting is dropped.
//!
//! Provisional: ties in the order are broken by object id (the executable's sort is not
//! traced), and the update runs once per simulation tick rather than per rendered frame.

use thedas_script::vm::ObjectId;

use crate::sim::Sim;
use crate::world::ObjectKind;

/// Threat_Decrease_Rate.
pub const PROPERTY_THREAT_DECREASE_RATE: i32 = 40;
/// How far away an enemy still adds proximity threat (metres).
pub const PROXIMITY_RANGE: f32 = 60.0;
/// The proximity threat of an enemy standing on top of the creature.
pub const PROXIMITY_THREAT: f32 = 100.0;
/// The table size past which new enemies must outweigh the last.
pub const TABLE_SIZE: usize = 5;

/// One enemy in a creature's threat table.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Threat {
    pub enemy: ObjectId,
    /// What the scripts added, less what fell off.
    pub value: f32,
    /// The engine's proximity term.
    #[serde(default)]
    pub near: f32,
}

impl Threat {
    pub fn total(&self) -> f32 {
        self.value + self.near
    }
}

/// Reads a table saved either as entries or, by saves from before the engine's part was
/// kept, as `(enemy, threat)` pairs.
pub fn de_table<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Threat>, D::Error> {
    #[derive(serde::Deserialize)]
    #[serde(untagged)]
    enum Entry {
        Now(Threat),
        Pair(ObjectId, f32),
    }
    let v: Vec<Entry> = serde::Deserialize::deserialize(d)?;
    Ok(v.into_iter()
        .map(|e| match e {
            Entry::Now(t) => t,
            Entry::Pair(enemy, value) => Threat {
                enemy,
                value,
                near: 0.0,
            },
        })
        .collect())
}

/// Orders a table highest total first (ties by enemy id).
pub fn sort(table: &mut [Threat]) {
    table.sort_by(|a, b| b.total().total_cmp(&a.total()).then(a.enemy.cmp(&b.enemy)));
}

/// `UpdateThreatTable`: adds `delta` to `enemy`'s threat in `table`.
pub fn update(table: &mut Vec<Threat>, enemy: ObjectId, delta: f32) {
    if let Some(t) = table.iter_mut().find(|t| t.enemy == enemy) {
        t.value = (t.value + delta).max(0.0);
        sort(table);
        return;
    }
    let size = table.len();
    if size == TABLE_SIZE && table[TABLE_SIZE - 1].total() > delta {
        return;
    }
    table.push(Threat {
        enemy,
        value: delta.max(0.0),
        near: 0.0,
    });
    sort(table);
    if size > TABLE_SIZE {
        table.remove(size - 1);
    }
}

/// Ground distance's proximity threat.
pub fn proximity(distance: f32) -> f32 {
    if distance < PROXIMITY_RANGE {
        (PROXIMITY_RANGE - distance) / PROXIMITY_RANGE * PROXIMITY_THREAT
    } else {
        0.0
    }
}

impl Sim {
    /// The engine's part of every creature's threat over `ms`: see the module docs.
    pub fn update_threat(&mut self, ms: u64) {
        let ids: Vec<ObjectId> = self
            .world
            .objects
            .values()
            .filter(|o| o.kind == ObjectKind::Creature && !o.combat.threat.is_empty())
            .filter(|o| o.parked.is_none())
            .map(|o| o.id)
            .collect();
        let seconds = ms as f32 / 1000.0;
        for id in ids {
            let Some(o) = self.world.get(id) else {
                continue;
            };
            // A simple property: its base and what effects add (not the larger of base and
            // current, which a negative rate would lose).
            let rate = o
                .properties
                .get(&PROPERTY_THREAT_DECREASE_RATE)
                .map_or(0.0, |v| v.0)
                + self.effect_modifier(id, PROPERTY_THREAT_DECREASE_RATE);
            let at = o.position;
            let near: Vec<f32> = o
                .combat
                .threat
                .iter()
                .map(|t| {
                    self.world.get(t.enemy).map_or(0.0, |e| {
                        let (dx, dy) = (e.position[0] - at[0], e.position[1] - at[1]);
                        proximity((dx * dx + dy * dy).sqrt())
                    })
                })
                .collect();
            if let Some(o) = self.world.get_mut(id) {
                for (t, n) in o.combat.threat.iter_mut().zip(near) {
                    t.value = (t.value + rate * seconds).max(0.0);
                    t.near = n.max(0.0);
                }
                sort(&mut o.combat.threat);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(enemy: ObjectId, value: f32) -> Threat {
        Threat {
            enemy,
            value,
            near: 0.0,
        }
    }

    #[test]
    fn updates_add_never_below_zero_and_keep_the_order() {
        let mut table = Vec::new();
        update(&mut table, 1, 10.0);
        update(&mut table, 2, 20.0);
        assert_eq!(table, vec![t(2, 20.0), t(1, 10.0)]);
        update(&mut table, 2, -50.0);
        assert_eq!(table, vec![t(1, 10.0), t(2, 0.0)]);
        update(&mut table, 3, -5.0);
        assert_eq!(
            table[2],
            t(3, 0.0),
            "a new enemy's threat starts at no less than 0"
        );
    }

    #[test]
    fn a_full_table_takes_only_enemies_that_outweigh_the_last() {
        let mut table: Vec<Threat> = (1..=5).map(|e| t(e, 10.0 * e as f32)).collect();
        sort(&mut table);
        update(&mut table, 9, 5.0);
        assert_eq!(table.len(), 5, "5 is below the last entry's 10");
        update(&mut table, 9, 15.0);
        assert_eq!(table.len(), 6);
        assert_eq!(table[4], t(9, 15.0));
        // Past five, the entry at the old last index goes.
        update(&mut table, 10, 100.0);
        assert_eq!(table.len(), 6);
        assert_eq!(table[0], t(10, 100.0));
        assert!(!table.iter().any(|x| x.enemy == 9), "{table:?}");
    }

    #[test]
    fn proximity_falls_off_over_sixty_metres() {
        assert_eq!(proximity(0.0), 100.0);
        assert_eq!(proximity(30.0), 50.0);
        assert_eq!(proximity(60.0), 0.0);
        assert_eq!(proximity(90.0), 0.0);
    }

    #[test]
    fn each_update_lets_threat_fall_off_and_counts_who_is_near() {
        use std::sync::Arc;
        let table = Arc::new(thedas_script::EngineTable::parse(b"").unwrap());
        let mut s = Sim::new(Arc::new(thedas_vfs::Vfs::new()), table, 1);
        let me = s.world.spawn(ObjectKind::Creature, "genlock");
        let tank = s.world.spawn(ObjectKind::Creature, "tank");
        let mage = s.world.spawn(ObjectKind::Creature, "mage");
        s.world.get_mut(tank).unwrap().position = [3.0, 4.0, 10.0];
        s.world.get_mut(mage).unwrap().position = [30.0, 40.0, 0.0];
        let o = s.world.get_mut(me).unwrap();
        o.properties
            .insert(PROPERTY_THREAT_DECREASE_RATE, (-0.5, 0.0));
        o.combat.threat = vec![t(mage, 20.0), t(tank, 1.0)];
        s.update_threat(2000);
        let th = &s.world.get(me).unwrap().combat.threat;
        // The tank 5 m away across the ground (its height does not count): 1 - 1 + 91.67.
        assert_eq!(th[0].enemy, tank);
        assert_eq!(th[0].value, 0.0);
        assert!((th[0].near - 100.0 * 55.0 / 60.0).abs() < 1e-3);
        // The mage 50 m away: 20 - 1 + 16.67.
        assert_eq!(th[1].value, 19.0);
        assert!((th[1].near - 100.0 * 10.0 / 60.0).abs() < 1e-3);
    }

    #[test]
    fn older_saves_tables_load() {
        #[derive(serde::Deserialize)]
        struct C {
            #[serde(deserialize_with = "de_table")]
            threat: Vec<Threat>,
        }
        let c: C = serde_json::from_str(r#"{"threat": [[7, 2.5]]}"#).unwrap();
        assert_eq!(c.threat, vec![t(7, 2.5)]);
    }
}
