//! Origins placeable state initialization, transitions and walkmesh writes.
//! Animation completion/removal is a separate path (0x74d200).

use std::collections::BTreeMap;
use std::sync::Arc;

use thedas_formats::gda::Cell;
use thedas_formats::level::NavGrid;
use thedas_script::vm::ObjectId;

use crate::fmath::DetMath;
use crate::sim::Sim;
use crate::world::ObjectKind;

impl Sim {
    /// The ASCII instance reader writes InitialState directly to the state word
    /// (0x7506ba..0x7506df); model setup then forces that state (0x752a88).
    /// Saved/script-assigned State takes precedence, including on legacy saves.
    pub(crate) fn init_placeable_state(&mut self, id: ObjectId) {
        if self.game != thedas_vfs::Game::Origins {
            return;
        }
        let Some(o) = self
            .world
            .get_mut(id)
            .filter(|o| o.kind == ObjectKind::Placeable && !o.fields.contains_key("State"))
        else {
            return;
        };
        let Some(&state) = o.fields.get("InitialState") else {
            return;
        };
        let state = i32::from(state as u16);
        o.fields.insert("State".into(), state);
        // Area registration also installs the state grid without requiring a
        // controller (0x750caa), subject to the same walkmesh gates.
        if !self.change_placeable_state(id, state, true) {
            self.write_placeable_walkmesh(id, state as u16, false);
        }
        // Model setup subsequently applies the loaded active flag through
        // vfunc +0x140 (0x752096 / 0x752aa3 -> 0x74f200).
        if !self.world.get(id).unwrap().active {
            let state = self.world.get(id).unwrap().fields["State"] as u16;
            self.write_placeable_walkmesh(id, state, true);
        }
    }

    /// CCPlaceable's active setter (0x74f200) also installs/removes its PWK.
    pub(crate) fn set_object_active(&mut self, id: ObjectId, active: bool) {
        let Some(o) = self.world.get_mut(id) else {
            return;
        };
        o.active = active;
        if self.game == thedas_vfs::Game::Origins && o.kind == ObjectKind::Placeable {
            let state = o
                .fields
                .get("State")
                .or_else(|| o.fields.get("AnimContState"))
                .copied()
                .unwrap_or(0) as u16;
            self.write_placeable_walkmesh(id, state, !active);
        }
    }

    /// The ordinary state path (0x74f9d0, force=0), shared by the script setter
    /// and successful actions. Forced initialization shows the destination pose
    /// immediately; the renderer already chooses it on an object's first sighting.
    pub(crate) fn set_placeable_state(&mut self, id: ObjectId, state: i32) {
        if self.game == thedas_vfs::Game::DragonAge2 {
            if let Some(o) = self.world.get_mut(id) {
                o.fields.insert("State".into(), state);
            }
            return;
        }
        self.change_placeable_state(id, state, false);
    }

    fn change_placeable_state(&mut self, id: ObjectId, state: i32, force: bool) -> bool {
        let Some(o) = self
            .world
            .get(id)
            .filter(|o| o.kind == ObjectKind::Placeable)
        else {
            return false;
        };
        let tables = self.world.tables();
        let Some(types) = tables.get(93, "") else {
            return false;
        };
        let Some(Cell::String(controller)) =
            types.cell_by_hash(0x9a32_0a51, i64::from(o.appearance as u16))
        else {
            return false;
        };
        let controller = crate::engine::clean(controller);
        if controller.is_empty() {
            return false;
        }
        let Some(controller) = tables.get(0, controller).filter(|t| !t.is_empty()) else {
            return false;
        };
        let current = o
            .fields
            .get("State")
            .or_else(|| o.fields.get("AnimContState"))
            .copied()
            .unwrap_or(0) as u16;
        let state = state as u16;
        if current == state && !force {
            return false;
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
        // 0x74fb27 removes the old footprint, then 0x74fb4e installs the new
        // state's grid. A later transition must not leave its initial PWK behind.
        self.write_placeable_walkmesh(id, current, true);
        // Door physics restores its original group in blocking controller states
        // and uses group 1 otherwise (0x74e850 -> 0x74bcc0 / 0x74d350).
        let row = controller.order[state as usize];
        let appearance_row = i64::from(self.world.get(id).unwrap().appearance as u16);
        if matches!(types.cell_by_hash(0x5010_edc3, appearance_row), Some(Cell::Int(v)) if *v as u8 == 1)
            && let Some(Cell::String(model)) = types.cell_by_hash(0xfd84_03d5, appearance_row)
            && let Ok((data, _)) = self
                .vfs
                .read(&format!("{}.phy", crate::engine::clean(model)))
            && let Ok(physics) = thedas_formats::phy::Phy::parse(&data)
        {
            let o = self.world.get_mut(id).unwrap();
            let blocking = o.active
                && matches!(controller.cell_by_hash(0x86a2_7132, row), Some(Cell::Int(v)) if *v != 0);
            o.placeable_group = Some(if blocking {
                physics
                    .actor_masks
                    .iter()
                    .copied()
                    .find(|&m| m != 0)
                    .unwrap_or(0)
            } else {
                1
            });
        }
        self.write_placeable_walkmesh(id, state as u16, false);
        // The forced path, like an ordinary transition, disables interaction if
        // neither controller action exists (0x74fb61..0x74fba1). It never enables
        // a Useable=0 object. These are action IDs, not controller events.
        let cells = &controller.rows[&row];
        let first_action = controller.order.len() + 2;
        // 0xa3bccf..0xa3bcfe: up to two action/target/variation triples
        // between the state-animation columns and the blocking column.
        let actions = controller
            .columns
            .get(&0x86a2_7132)
            .and_then(|column| column.checked_sub(first_action))
            .map_or(0, |columns| (columns / 3).min(2));
        if (0..actions).all(|slot| {
            !matches!(cells.get(first_action + slot * 3), Some(Cell::Int(v)) if *v as u16 != 0)
        }) {
            self.world.get_mut(id).unwrap().interactive = false;
        }
        true
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
        let state = o
            .fields
            .get("State")
            .or_else(|| o.fields.get("AnimContState"))
            .copied()
            .unwrap_or(0) as u16;
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
        self.write_placeable_walkmesh(id, state, true);
    }

    /// 0x74dba0, operations 0 (install) and 1 (clear). Uses the original PHY
    /// group for eligibility, independently of the runtime group override.
    fn write_placeable_walkmesh(&mut self, id: ObjectId, state: u16, clear: bool) {
        let Some(o) = self.world.get(id) else { return };
        let row = i64::from(o.appearance as u16);
        let appearance = o.appearance;
        let Some(types) = self.world.tables().get(93, "") else {
            return;
        };
        let destructive =
            matches!(types.cell_by_hash(0x1dea_a1ea, row), Some(Cell::Int(v)) if *v != 0);
        if destructive && crate::engine::origins_placeable_health(o) <= 0 {
            return;
        }
        let Some(Cell::String(model)) = types.cell_by_hash(0xfd84_03d5, row) else {
            return;
        };
        let model = crate::engine::clean(model);
        let original_group = self
            .vfs
            .read(&format!("{model}.phy"))
            .ok()
            .and_then(|(d, _)| thedas_formats::phy::Phy::parse(&d).ok())
            .map_or(u32::MAX, |p| {
                p.actor_masks.iter().copied().find(|&m| m != 0).unwrap_or(0)
            });
        let type_one =
            matches!(types.cell_by_hash(0x5010_edc3, row), Some(Cell::Int(v)) if *v as u8 == 1);
        if original_group & 1 != 0 && !type_one {
            return;
        }
        let area = o.area;
        let position = o.position;
        let facing = o.facing;
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
        let writes = write_walkmesh_cells(&mut changed, &source, position, facing, clear);
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
#[cfg(test)]
fn clear_walkmesh_cells(
    destination: &mut NavGrid,
    source: &NavGrid,
    position: [f32; 3],
    facing: f32,
) -> BTreeMap<u32, u8> {
    write_walkmesh_cells(destination, source, position, facing, true)
}

/// Operation 0 copies source kinds only over destination kinds below 8;
/// operation 1 changes destination kinds 7/8 to 1, regardless of source kind.
fn write_walkmesh_cells(
    destination: &mut NavGrid,
    source: &NavGrid,
    position: [f32; 3],
    facing: f32,
    clear: bool,
) -> BTreeMap<u32, u8> {
    let angle = facing.to_radians();
    let (sin, cos) = (angle.dsin(), angle.dcos());
    let mut writes = BTreeMap::new();
    for x in 0..source.columns {
        for y in 0..source.rows {
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
            if let Some(v) = destination.data.get_mut(index as usize) {
                let next = if clear {
                    matches!(*v, 7 | 8).then_some(1)
                } else {
                    (*v < 8).then(|| source.data[(y * source.columns + x) as usize])
                };
                if let Some(next) = next.filter(|&next| next != *v) {
                    *v = next;
                    writes.insert(index, next);
                }
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
    fn walkmesh_install_copies_source_kinds_and_protects_destination_eight_and_above() {
        let source = grid(4, 1, vec![7, 1, 8, 6]);
        let mut destination = grid(4, 2, vec![1, 7, 8, 9, 0, 1, 6, 7]);
        let writes = write_walkmesh_cells(&mut destination, &source, [0.0; 3], 0.0, false);
        assert_eq!(writes, BTreeMap::from([(0, 7), (1, 1)]));
        assert_eq!(&destination.data[..4], &[7, 1, 8, 9]);
        let writes = write_walkmesh_cells(&mut destination, &source, [0.0, 1.0, 0.0], 0.0, false);
        assert_eq!(writes, BTreeMap::from([(4, 7), (6, 8), (7, 6)]));
        assert!(destination.heights.is_empty() && destination.surfaces.is_empty());
    }

    #[test]
    fn legacy_initialization_keeps_explicit_states_and_new_saves_keep_later_changes() {
        use thedas_script::EngineTable;
        use thedas_vfs::{Game, Vfs};
        for game in [Game::Origins, Game::DragonAge2] {
            let mut s = Sim::new(Arc::new(Vfs::new()), Arc::new(EngineTable::default()), 7);
            s.game = game;
            let untouched = s.world.spawn(ObjectKind::Placeable, "initial_open");
            let changed = s.world.spawn(ObjectKind::Placeable, "saved_closed");
            let creature = s.world.spawn(ObjectKind::Creature, "not_placeable");
            for id in [untouched, changed, creature] {
                s.world
                    .get_mut(id)
                    .unwrap()
                    .fields
                    .extend([("InitialState".into(), 2), ("AnimContState".into(), 0)]);
            }
            s.world
                .get_mut(changed)
                .unwrap()
                .fields
                .insert("State".into(), 1);
            let mut save = s.save_game();
            save.revision = 9;
            s.load_game(save).unwrap();
            assert_eq!(
                s.world.get(untouched).unwrap().fields.get("State"),
                if game == Game::Origins {
                    Some(&2)
                } else {
                    None
                }
            );
            assert_eq!(s.world.get(changed).unwrap().fields["State"], 1);
            assert!(!s.world.get(creature).unwrap().fields.contains_key("State"));
            s.world
                .get_mut(untouched)
                .unwrap()
                .fields
                .insert("State".into(), 0);
            let json = s.save_json().unwrap();
            s.load_json(&json).unwrap();
            assert_eq!(s.world.get(untouched).unwrap().fields["State"], 0);
            assert!(s.world.queue.is_empty());
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
