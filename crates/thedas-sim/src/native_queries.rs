//! Origins ring, weapon-controller and play-time queries. See the accompanying
//! trace note for the existing skeletal-pose and clock approximations.

use thedas_formats::gda::Cell;
use thedas_script::vm::ObjectId;

use crate::sim::Sim;
use crate::world::{Object, ObjectKind};

impl Sim {
    /// Current weapon attachments, including explicit script draw/sheath commands.
    pub fn weapon_drawn_hands(&self, o: &Object) -> (bool, bool) {
        if self.game != thedas_vfs::Game::Origins {
            return (o.combat.in_combat, o.combat.in_combat);
        }
        let drawn = |field: &str| o.fields.get(field).map_or(o.combat.in_combat, |&v| v != 0);
        (drawn("WeaponMainDrawn"), drawn("WeaponOffDrawn"))
    }

    /// 160 -> shared 0x668a20 -> controller predicate 0x7c34c0.
    pub fn weapons_unsheathed(&self, id: ObjectId) -> bool {
        let Some(o) = self
            .world
            .get(id)
            .filter(|o| o.kind == ObjectKind::Creature)
        else {
            return false;
        };
        let (main, off) = self.weapon_drawn_hands(o);
        let mode = o.fields.get("WeaponDrawMode").copied().unwrap_or_else(|| {
            // Sim already derives its controller's gear selection from equipment.
            let offhand = crate::sim::constant(&self.table, "INVENTORY_SLOT_OFFHAND");
            if o.equipment.contains_key(&offhand) {
                1
            } else {
                0
            }
        });
        match mode {
            -1 | 0 | 2 => main,
            1 | 3 => main && off,
            _ => false,
        }
    }

    pub(crate) fn set_weapons_drawn(&mut self, id: ObjectId, drawn: bool) {
        if let Some(o) = self
            .world
            .get_mut(id)
            .filter(|o| o.kind == ObjectKind::Creature)
        {
            o.fields.insert("WeaponMainDrawn".into(), i32::from(drawn));
            o.fields.insert("WeaponOffDrawn".into(), i32::from(drawn));
        }
    }

    /// 606 (0x66cac0): a ring's center, never a place on its circumference.
    pub fn melee_ring_position(&self, id: ObjectId, index: i32) -> [f32; 3] {
        let Some(o) = self
            .world
            .get(id)
            .filter(|o| o.kind == ObjectKind::Creature)
        else {
            return [0.0; 3];
        };
        if !(0..=9).contains(&index) {
            return [0.0; 3];
        }
        let appearance = i64::from(o.appearance as u16);
        if index != 0 {
            // Native construction stops at the first missing integer anchor cell.
            for ring in 1..=index {
                if !self
                    .world
                    .tables()
                    .get(0, &format!("meleering{ring}"))
                    .is_some_and(|t| {
                        matches!(t.cell_by_hash(0x0cc5_4d0c, appearance), Some(Cell::Int(_)))
                    })
                {
                    return [0.0; 3];
                }
            }
            // Bone-node centers need the animated skeleton, unavailable in Sim.
            return o.position;
        }
        let offset = self
            .world
            .tables()
            .get(2, "")
            .and_then(|t| match t.cell_by_hash(0xed58_b0c9, appearance) {
                Some(Cell::Float(v)) => Some(*v as f32),
                Some(Cell::Int(v)) => Some(*v as f32),
                _ => None,
            })
            .unwrap_or(0.0)
            .clamp(-0.5, 0.5)
            * 0.6;
        let angle = o.facing.to_radians();
        use crate::fmath::DetMath;
        [
            o.position[0] - offset * angle.dsin(),
            o.position[1] + offset * angle.dcos(),
            o.position[2],
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use thedas_script::EngineTable;
    use thedas_script::vm::OBJECT_INVALID;
    use thedas_vfs::Vfs;

    fn sim() -> Sim {
        Sim::new(
            Arc::new(Vfs::new()),
            Arc::new(EngineTable::parse(b"int INVENTORY_SLOT_OFFHAND = 2;").unwrap()),
            1,
        )
    }

    #[test]
    fn ring_weapon_queries_distinguish_drawn_hands_and_controller_modes() {
        let mut s = sim();
        let c = s.world.spawn(ObjectKind::Creature, "c");
        assert!(!s.weapons_unsheathed(c));
        s.world.get_mut(c).unwrap().combat.in_combat = true;
        assert!(s.weapons_unsheathed(c));
        s.set_weapons_drawn(c, false);
        assert!(!s.weapons_unsheathed(c));
        let o = s.world.get_mut(c).unwrap();
        o.fields.insert("WeaponMainDrawn".into(), 1);
        o.fields.insert("WeaponOffDrawn".into(), 0);
        for (mode, drawn) in [
            (-2, false),
            (-1, true),
            (0, true),
            (1, false),
            (2, true),
            (3, false),
            (4, false),
        ] {
            s.world
                .get_mut(c)
                .unwrap()
                .fields
                .insert("WeaponDrawMode".into(), mode);
            assert_eq!(s.weapons_unsheathed(c), drawn, "mode {mode}");
        }
        s.set_combat_state(c, false);
        assert!(!s.weapons_unsheathed(c));
        s.world.get_mut(c).unwrap().fields.remove("WeaponDrawMode");
        s.set_combat_state(c, true);
        assert!(s.weapons_unsheathed(c));
        assert!(!s.weapons_unsheathed(OBJECT_INVALID));
    }

    #[test]
    fn ring_weapon_queries_center_is_offset_along_facing_not_circumference() {
        let mut s = sim();
        let c = s.world.spawn(ObjectKind::Creature, "c");
        let o = s.world.get_mut(c).unwrap();
        o.appearance = 7;
        o.position = [10.0, 20.0, 3.0];
        assert_eq!(s.melee_ring_position(c, 0), [10.0, 20.0, 3.0]);
        s.world.tables.insert(
            "#2",
            &["ID", "#ed58b0c9"],
            vec![(7, vec![Cell::Int(7), Cell::Float(1.5)])],
        );
        assert_eq!(s.melee_ring_position(c, 0), [10.0, 20.3, 3.0]);
        s.world.get_mut(c).unwrap().facing = 90.0;
        let p = s.melee_ring_position(c, 0);
        assert!((p[0] - 9.7).abs() < 1e-5 && (p[1] - 20.0).abs() < 1e-5);
        assert_eq!(s.melee_ring_position(c, -1), [0.0; 3]);
        assert_eq!(s.melee_ring_position(c, 10), [0.0; 3]);
        assert_eq!(s.melee_ring_position(OBJECT_INVALID, 0), [0.0; 3]);
    }

    #[test]
    fn ring_weapon_queries_additional_rings_require_consecutive_rows() {
        let mut s = sim();
        let c = s.world.spawn(ObjectKind::Creature, "c");
        s.world.get_mut(c).unwrap().position = [10.0, 2.0, 3.0];
        s.world.tables.insert(
            "meleering2",
            &["ID", "#0cc54d0c"],
            vec![(0, vec![Cell::Int(0), Cell::Int(0)])],
        );
        assert_eq!(s.melee_ring_position(c, 2), [0.0; 3]);
        s.world.tables.insert(
            "meleering1",
            &["ID", "#0cc54d0c"],
            vec![(0, vec![Cell::Int(0), Cell::Int(0)])],
        );
        assert_eq!(s.melee_ring_position(c, 1), [10.0, 2.0, 3.0]);
        assert_eq!(s.melee_ring_position(c, 2), [10.0, 2.0, 3.0]);
        assert_eq!(s.melee_ring_position(c, 3), [0.0; 3]);
    }
}
