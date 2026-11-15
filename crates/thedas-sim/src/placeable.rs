//! Origins placeable destruction's pathing and physics writes, before current health
//! changes (0x74e8e0). Animation completion/removal is a separate path (0x74d200).

use std::collections::BTreeMap;
use std::sync::Arc;

use thedas_formats::gda::Cell;
use thedas_formats::level::NavGrid;
use thedas_script::vm::ObjectId;

use crate::fmath::DetMath;
use crate::sim::Sim;
use crate::world::ObjectKind;

impl Sim {
    /// Origins 356 (0x65f2e0): resolve the placeable's localized popup string.
    pub fn placeable_popup_text(&self, id: ObjectId) -> String {
        let Some(o) = self
            .world
            .get(id)
            .filter(|o| o.kind == ObjectKind::Placeable)
        else {
            return String::new();
        };
        o.fields
            .get("LocPopupText")
            .and_then(|&r| self.string(r as u32))
            .or_else(|| o.text_fields.get("LocPopupText").cloned())
            .unwrap_or_default()
    }

    /// The independent creature flag written by Origins 364 (0x6665c0).
    pub fn show_as_ally_on_map(&self, id: ObjectId) -> bool {
        self.world.get(id).is_some_and(|o| {
            o.kind == ObjectKind::Creature
                && o.fields.get("ShowAsAllyOnMap").copied().unwrap_or(0) != 0
        })
    }

    /// The ordinary state path (0x74f9d0, force=0), shared by the script setter
    /// and successful actions. Animation playback and PWK installation remain separate.
    pub(crate) fn set_placeable_state(&mut self, id: ObjectId, state: i32) {
        if self.game == thedas_vfs::Game::DragonAge2 {
            if let Some(o) = self.world.get_mut(id) {
                o.fields.insert("State".into(), state);
            }
            return;
        }
        let Some(o) = self
            .world
            .get(id)
            .filter(|o| o.kind == ObjectKind::Placeable)
        else {
            return;
        };
        let tables = self.world.tables();
        let Some(types) = tables.get(93, "") else {
            return;
        };
        let Some(Cell::String(controller)) =
            types.cell_by_hash(0x9a32_0a51, i64::from(o.appearance as u16))
        else {
            return;
        };
        let controller = crate::engine::clean(controller);
        if controller.is_empty() {
            return;
        }
        let Some(controller) = tables.get(0, controller).filter(|t| !t.is_empty()) else {
            return;
        };
        let current = o
            .fields
            .get("State")
            .or_else(|| o.fields.get("AnimContState"))
            .copied()
            .unwrap_or(0) as u16;
        let state = state as u16;
        if current == state {
            return;
        }
        // The controller loader stores one record per row position (0xa3bc73).
        // The setter bounds the unsigned-short request by that record count.
        let state = if usize::from(state) < controller.order.len() {
            i32::from(state)
        } else {
            0
        };
        self.world
            .get_mut(id)
            .unwrap()
            .fields
            .insert("State".into(), state);
    }

    pub(crate) fn set_placeable_health(&mut self, id: ObjectId, current: i32) {
        if self.game != thedas_vfs::Game::Origins
            || !self
                .world
                .get(id)
                .is_some_and(|o| o.kind == ObjectKind::Placeable)
        {
            return;
        }
        let current = i32::from(current as i16);
        if current <= 0 {
            self.clear_destroyed_placeable_walkmesh(id);
        }
        let o = self.world.get_mut(id).unwrap();
        o.fields.insert("Health".into(), current);
        if let Some(p) = o.properties.get_mut(&7) {
            p.1 = current as f32;
        }
        if current <= 0 && o.inventory.is_empty() {
            o.interactive = false;
        }
    }

    fn clear_destroyed_placeable_walkmesh(&mut self, id: ObjectId) {
        let o = self.world.get(id).unwrap();
        let row = i64::from(o.appearance as u16);
        let appearance = o.appearance;
        let Some(types) = self.world.tables().get(93, "") else {
            return;
        };
        if !matches!(types.cell_by_hash(0x1dea_a1ea, row), Some(Cell::Int(v)) if *v != 0) {
            return;
        }
        let Some(Cell::String(model)) = types.cell_by_hash(0xfd84_03d5, row) else {
            return;
        };
        let model = crate::engine::clean(model);
        let physics = self
            .vfs
            .read(&format!("{model}.phy"))
            .ok()
            .and_then(|(d, _)| thedas_formats::phy::Phy::parse(&d).ok());
        let original_group = physics.as_ref().map_or(u32::MAX, |p| {
            p.actor_masks.iter().copied().find(|&m| m != 0).unwrap_or(0)
        });
        let area = o.area;
        let state = o
            .fields
            .get("State")
            .or_else(|| o.fields.get("AnimContState"))
            .copied()
            .unwrap_or(0) as u16;
        let position = o.position;
        let facing = o.facing;
        let was_alive = crate::engine::origins_placeable_health(o) > 0;
        let type_one =
            matches!(types.cell_by_hash(0x5010_edc3, row), Some(Cell::Int(v)) if *v as u8 == 1);
        // 0x74d350(0) writes group 1 on every nonpositive setter, even when
        // 0x74dba0 rejects the walkmesh operation. It does not change Active.
        if physics.is_some() {
            self.world.get_mut(id).unwrap().placeable_group = Some(1);
        }
        if !was_alive || (original_group & 1 != 0 && !type_one) {
            return;
        }
        let Some(area) = area else { return };
        let Some(source) = self
            .vfs
            .read(&format!("{model}_{appearance}.pwk"))
            .ok()
            .and_then(|(d, _)| thedas_formats::pwk::Pwk::parse(&d).ok())
            .and_then(|p| p.states.get(state as usize).cloned().flatten())
        else {
            return;
        };
        let Some(grid) = self.nav_grid(area) else {
            return;
        };
        let mut changed = (*grid).clone();
        let writes = clear_walkmesh_cells(&mut changed, &source, position, facing);
        if !writes.is_empty() {
            self.world
                .nav_patches
                .entry(area)
                .or_default()
                .extend(writes);
            self.changed_nav_grids.insert(area, Arc::new(changed));
        }
    }
}

/// Operation 1 (0xa46cc0): visit EVERY source cell, transformed into the area's
/// grid. Only destination kinds 7/8 become 1; source kinds are not a mask.
fn clear_walkmesh_cells(
    destination: &mut NavGrid,
    source: &NavGrid,
    position: [f32; 3],
    facing: f32,
) -> BTreeMap<u32, u8> {
    let angle = facing.to_radians();
    let (sin, cos) = (angle.dsin(), angle.dcos());
    let mut writes = BTreeMap::new();
    for y in 0..source.rows {
        for x in 0..source.columns {
            let px = source.base[0] + (x as f32 + 0.5) * source.cell_size;
            let py = source.base[1] + (y as f32 + 0.5) * source.cell_size;
            let world = [
                position[0] + px * cos - py * sin,
                position[1] + px * sin + py * cos,
            ];
            let cell = |i: usize| {
                ((world[i] - destination.base[i] - destination.cell_size * 0.5)
                    / destination.cell_size
                    + 0.5)
                    .trunc()
            };
            let (cx, cy) = (cell(0), cell(1));
            if !cx.is_finite()
                || !cy.is_finite()
                || cx < 0.0
                || cy < 0.0
                || cx >= destination.columns as f32
                || cy >= destination.rows as f32
            {
                continue;
            }
            let index = cy as u32 * destination.columns + cx as u32;
            if let Some(v) = destination.data.get_mut(index as usize)
                && matches!(*v, 7 | 8)
            {
                *v = 1;
                writes.insert(index, 1);
            }
        }
    }
    writes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(columns: u32, rows: u32, data: Vec<u8>) -> NavGrid {
        NavGrid {
            columns,
            rows,
            cell_size: 1.0,
            clearance: 1.0,
            base: [0.0; 3],
            normal: [0.0, 0.0, 1.0],
            data,
            heights: vec![],
            surfaces: vec![],
        }
    }

    #[test]
    fn walkmesh_clear_transforms_cells_and_keeps_other_kinds() {
        let source = grid(3, 1, vec![0, 0, 0]);
        let mut destination = grid(4, 4, vec![7, 8, 9, 1, 8, 6, 7, 1, 7, 8, 7, 1, 1, 1, 1, 1]);
        let writes = clear_walkmesh_cells(&mut destination, &source, [0.0; 3], 0.0);
        assert_eq!(writes, BTreeMap::from([(0, 1), (1, 1)]));
        assert_eq!(&destination.data[..4], &[1, 1, 9, 1]);
        // A quarter turn of the same three local cells hits column 2, rows 0..2.
        let writes = clear_walkmesh_cells(&mut destination, &source, [3.0, 0.0, 0.0], 90.0);
        assert_eq!(writes, BTreeMap::from([(6, 1), (10, 1)]));
        assert!(clear_walkmesh_cells(&mut destination, &source, [100.0; 3], 0.0).is_empty());
        assert_eq!(destination.data[5], 6);
    }

    #[test]
    #[ignore = "requires an Origins THEDAS_INSTALL"]
    fn installed_destructible_walkmeshes_clear_once_and_survive_reload() {
        use thedas_script::EngineTable;
        let install =
            thedas_vfs::DaoInstall::open(std::env::var_os("THEDAS_INSTALL").unwrap()).unwrap();
        let vfs = Arc::new(
            install
                .global_vfs(thedas_vfs::install::TextureQuality::High)
                .unwrap(),
        );
        let table = Arc::new(EngineTable::parse(&vfs.read("script.ldf").unwrap().0).unwrap());
        let mut s = Sim::new(vfs, table, 7);
        for appearance in [227, 367, 368, 404] {
            let area = s.world.spawn(ObjectKind::Area, "walkmesh_area");
            let untouched = s.world.spawn(ObjectKind::Area, "other_instance");
            let layout = format!("fixture_{appearance}");
            for id in [area, untouched] {
                s.area_layouts.insert(id, layout.clone());
            }
            let base = Arc::new(grid(100, 100, vec![7; 10_000]));
            s.nav_grids.insert(layout.clone(), Some(base.clone()));
            let id = s.world.spawn(ObjectKind::Placeable, "barricade");
            let o = s.world.get_mut(id).unwrap();
            o.appearance = appearance;
            o.area = Some(area);
            o.position = [20.0, 20.0, 0.0];
            o.fields.insert("Health".into(), 5);
            o.fields.insert("State".into(), 65_536); // Native state indexes use u16.
            let door = s.is_door(id);
            // Populate the sight cache before destruction, so a stale cache cannot
            // keep the old physics group blocking after the health write.
            if !door {
                assert!(!s.placeable_collision(area).unwrap().is_empty());
            }
            s.set_placeable_health(id, 1);
            assert!(s.world.nav_patches.is_empty());
            assert_eq!(s.world.get(id).unwrap().placeable_group, None);
            s.set_placeable_health(id, 0);
            assert_eq!(s.world.get(id).unwrap().placeable_group, Some(1));
            assert!(s.placeable_collision(area).unwrap().is_empty());
            assert!(s.closed_doors(area).is_empty());
            assert!(s.door_sight(area).solid.is_empty());
            let patches = s
                .world
                .nav_patches
                .get(&area)
                .expect("real PWK must change grid cells");
            assert!(!patches.is_empty());
            let index = *patches.keys().next().unwrap() as usize;
            assert_eq!(s.nav_grid(area).unwrap().data[index], 1);
            assert_eq!(s.nav_grid(untouched).unwrap().data, base.data);
            // A later grid owner can write the cell again. Repeated dead health
            // writes must not replay the first old-state removal over its write.
            let mut replaced = (*s.nav_grid(area).unwrap()).clone();
            replaced.data[index] = 8;
            s.changed_nav_grids.insert(area, Arc::new(replaced));
            s.world
                .nav_patches
                .get_mut(&area)
                .unwrap()
                .insert(index as u32, 8);
            let json = s.save_json().unwrap();
            s.load_json(&json).unwrap();
            s.area_layouts.insert(area, layout.clone());
            s.area_layouts.insert(untouched, layout);
            s.set_placeable_health(id, -1);
            assert_eq!(s.nav_grid(area).unwrap().data[index], 8);
            assert_eq!(s.nav_grid(untouched).unwrap().data, base.data);
            assert_eq!(s.world.get(id).unwrap().placeable_group, Some(1));
            assert!(s.world.destroy_at.is_empty()); // Health alone never starts removal.
            s.world.nav_patches.clear();
            s.changed_nav_grids.clear();
            // Table lookup truncates appearance, but the PWK loader formats the
            // original ASCII Appearance integer. No such suffixed asset exists.
            let o = s.world.get_mut(id).unwrap();
            o.appearance += 65_536;
            o.fields.insert("Health".into(), 5);
            s.set_placeable_health(id, 0);
            assert_eq!(s.world.get(id).unwrap().placeable_group, Some(1));
            assert!(s.world.nav_patches.is_empty());
        }
    }
}
