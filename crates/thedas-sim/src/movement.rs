//! Origins' scenery collision shared by routing and locomotion. The native
//! controller tests mask 0x2011134 (0x6b6880 -> 0x838240); PWK is only its
//! pathing counterpart. A missing PWK must not make a crate or door intangible.
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use thedas_formats::{gda::Cell, level::NavGrid};
use thedas_script::vm::ObjectId;

use crate::{collision::Collision, sim::Sim, world::ObjectKind};

#[derive(Default)]
pub(crate) struct MovementCache {
    props: HashMap<ObjectId, (u64, Arc<Collision>)>,
    grids: HashMap<(ObjectId, u32), MovementGrid>,
}

struct MovementGrid {
    grid: Arc<NavGrid>,
    level: Option<Arc<Collision>>,
    props: Arc<Collision>,
    cells: Arc<HashSet<usize>>,
}

impl Sim {
    /// Provisional stand-in for 0x6a1a30(1)'s animated `head` attachment: the
    /// appearance's authored height, including its female variant. A fixed
    /// perception height would let dwarves attack over head-high crates.
    fn attack_height(&self, id: ObjectId) -> f32 {
        let Some(o) = self.world.get(id) else {
            return crate::collision::EYE_HEIGHT;
        };
        let Some(table) = self.world.tables().get(0, "apr_base") else {
            return crate::collision::EYE_HEIGHT;
        };
        let height = |column| match table.cell(column, i64::from(o.appearance)) {
            Some(Cell::Float(h)) if h.is_finite() && *h > 0.0 => Some(*h as f32),
            _ => None,
        };
        let female = o.fields.get("Gender") == Some(&2);
        female
            .then(|| height("HEIGHT_F"))
            .flatten()
            .or_else(|| height("HEIGHT"))
            .unwrap_or(crate::collision::EYE_HEIGHT)
    }

    /// Creature attack execution also checks the physics ray (0x7a8909 -> 0x67c750,
    /// mask 0x24); weapon reach does not permit a blow through a box. Native head
    /// attachments are approximated by `attack_height`, separately from perception.
    pub(crate) fn attack_sight_clear(&mut self, attacker: ObjectId, target: ObjectId) -> bool {
        let Some((area, a, b)) = self.world.get(attacker).and_then(|a| {
            let b = self.world.get(target)?;
            // Placeable bashing needs a target-shape exclusion, which this ray
            // does not model. Preserve that path rather than hit the target itself.
            (a.area == b.area && a.kind == ObjectKind::Creature && b.kind == ObjectKind::Creature)
                .then_some((a.area?, a.position, b.position))
        }) else {
            return true;
        };
        let a = [a[0], a[1], a[2] + self.attack_height(attacker)];
        let b = [b[0], b[1], b[2] + self.attack_height(target)];
        let doors = self.door_sight(area);
        !self
            .placeable_collision(area)
            .is_some_and(|c| c.blocked(a, b))
            && !self.collision(area).is_some_and(|c| c.blocked(a, b))
            && !doors.solid.iter().any(|c| c.blocked(a, b))
            && !Self::door_between(&doors.points, a, b)
    }

    fn movement_props(&mut self, area: ObjectId) -> Arc<Collision> {
        let types = self.world.tables().get(93, "");
        let actors = self.world.actor_ids();
        let mut placed = Vec::new();
        for o in actors.iter().filter_map(|&id| self.world.get(id)) {
            if o.kind != ObjectKind::Placeable
                || o.area != Some(area)
                || !o.active
                || o.placeable_group
                    .is_some_and(|g| g & thedas_formats::phy::mask::MOVEMENT == 0)
            {
                continue;
            }
            let Some(types) = types.as_ref() else {
                continue;
            };
            let row = i64::from(o.appearance as u16);
            if matches!(types.cell_by_hash(0x9a32_0a51, row), Some(Cell::String(c))
                if ["statecnt_door", "statecnt_areatransition"].iter()
                    .any(|s| crate::engine::clean(c).eq_ignore_ascii_case(s)))
            {
                // Closed doors are path actions; area-transition volumes are triggers.
                continue;
            }
            let Some(Cell::String(model)) = types.cell_by_hash(0xfd84_03d5, row) else {
                continue;
            };
            placed.push((
                crate::engine::clean(model).to_string(),
                o.position,
                o.facing,
            ));
        }
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        for (m, p, f) in &placed {
            (m, p.map(f32::to_bits), f.to_bits()).hash(&mut hash);
        }
        let key = hash.finish();
        if let Some((k, c)) = self.movement_cache.props.get(&area)
            && *k == key
        {
            return c.clone();
        }
        let c = Arc::new(crate::collision::load_movement_placeables(
            &self.vfs, &placed,
        ));
        self.movement_cache.props.insert(area, (key, c.clone()));
        c
    }

    pub(crate) fn movement_cells(
        &mut self,
        id: ObjectId,
        grid: &Arc<NavGrid>,
    ) -> Arc<HashSet<usize>> {
        if self.game != thedas_vfs::Game::Origins {
            return Arc::default();
        }
        let Some(area) = self
            .world
            .get(id)
            .filter(|o| o.kind == ObjectKind::Creature)
            .and_then(|o| o.area)
        else {
            return Arc::default();
        };
        let level = self.collision(area);
        let props = self.movement_props(area);
        let radius = self.capsule_radius(id);
        let key = (area, radius.to_bits());
        if let Some(c) = self.movement_cache.grids.get(&key)
            && Arc::ptr_eq(&c.grid, grid)
            && Arc::ptr_eq(&c.props, &props)
            && match (&c.level, &level) {
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            }
        {
            return c.cells.clone();
        }
        let mut cells = props.movement_cells(grid, radius);
        if let Some(c) = &level {
            cells.extend(c.movement_cells(grid, radius));
        }
        let cells = Arc::new(cells);
        self.movement_cache.grids.insert(
            key,
            MovementGrid {
                grid: grid.clone(),
                level,
                props,
                cells: cells.clone(),
            },
        );
        cells
    }

    /// Constrain the proposed step on the current floor and obstacle grid. This also
    /// catches doors that shut after a route was started. Provisional: the grid is a
    /// conservative stand-in for PhysX sliding; it does not push other objects.
    pub(crate) fn scenery_step(&mut self, id: ObjectId, from: [f32; 3], to: [f32; 3]) -> [f32; 3] {
        if self.game != thedas_vfs::Game::Origins {
            return to;
        }
        let Some(o) = self
            .world
            .get(id)
            .filter(|o| o.kind == ObjectKind::Creature)
        else {
            return to;
        };
        let Some(area) = o.area else { return to };
        let Some(grid) = self.nav_grid(area) else {
            return to;
        };
        let cells = self.movement_cells(id, &grid);
        let own = grid.cell(from);
        let doors = self.closed_doors(area);
        let leaves = self.door_sight(area);
        let doors = self.doors_from(area, &grid, from, doors);
        let blocked = |c: (u32, u32)| {
            let i = (c.1 * grid.columns + c.0) as usize;
            !crate::nav::walkable(&grid, c)
                || cells.contains(&i)
                || doors.iter().any(|d| d.1.contains(&i))
        };
        let mut prev = own;
        let mut last = from;
        let dx = to[0] - from[0];
        let dy = to[1] - from[1];
        let steps = (((dx * dx + dy * dy).sqrt() / (grid.cell_size * 0.5)).ceil() as usize).max(1);
        for k in 1..=steps {
            let t = k as f32 / steps as f32;
            let mut p = [
                from[0] + dx * t,
                from[1] + dy * t,
                from[2] + (to[2] - from[2]) * t,
            ];
            let Some(c) = grid.cell(p) else { break };
            if blocked(c) && Some(c) != own {
                break;
            }
            // A carved doorway cell can straddle the leaf. Its centre being on
            // our side does not permit a stale route to cross the actual shape.
            let a = [last[0], last[1], last[2] + 1.0];
            let b = [p[0], p[1], p[2] + 1.0];
            if leaves
                .solid
                .iter()
                .any(|l| !l.blocked(a, [a[0], a[1], a[2] + 0.01]) && l.blocked(a, b))
            {
                break;
            }
            if let Some(previous) = prev {
                if (crate::nav::height(&grid, c) - crate::nav::height(&grid, previous)).abs()
                    > crate::nav::MAX_STEP
                {
                    break;
                }
                if previous.0 != c.0 && previous.1 != c.1 {
                    // Two samples can straddle both grid boundaries even when
                    // the line enters the free side cell first. Check the cell
                    // actually crossed; treating every such pair as a diagonal
                    // makes the same leg pass or fail depending on tick length.
                    let boundary = |axis: usize, a: u32, b: u32| {
                        grid.base[axis] + a.max(b) as f32 * grid.cell_size
                    };
                    let tx = (boundary(0, previous.0, c.0) - last[0]) / (p[0] - last[0]);
                    let ty = (boundary(1, previous.1, c.1) - last[1]) / (p[1] - last[1]);
                    let via_x = (c.0, previous.1);
                    let via_y = (previous.0, c.1);
                    let clear = |mid| {
                        !blocked(mid)
                            && (crate::nav::height(&grid, mid)
                                - crate::nav::height(&grid, previous))
                            .abs()
                                <= crate::nav::MAX_STEP
                            && (crate::nav::height(&grid, c) - crate::nav::height(&grid, mid)).abs()
                                <= crate::nav::MAX_STEP
                    };
                    let crosses_clear = if (tx - ty).abs() < 1e-5 {
                        clear(via_x) && clear(via_y)
                    } else if tx < ty {
                        clear(via_x)
                    } else {
                        clear(via_y)
                    };
                    if !crosses_clear {
                        break;
                    }
                }
            }
            let z = crate::nav::height(&grid, c);
            if (z - last[2]).abs() <= crate::nav::MAX_STEP {
                p[2] = z;
            }
            last = p;
            prev = Some(c);
        }
        last
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use thedas_script::EngineTable;
    use thedas_vfs::Vfs;

    fn scene() -> (Sim, ObjectId, ObjectId, Arc<NavGrid>) {
        let mut sim = Sim::new(Arc::new(Vfs::new()), Arc::new(EngineTable::default()), 1);
        let area = sim.world.spawn(ObjectKind::Area, "test");
        let hero = sim.world.spawn(ObjectKind::Creature, "hero");
        sim.world.hero = Some(hero);
        let o = sim.world.get_mut(hero).unwrap();
        o.area = Some(area);
        o.position = [1.5, 3.5, 0.0];
        let grid = Arc::new(NavGrid {
            columns: 8,
            rows: 8,
            cell_size: 1.0,
            clearance: 2.0,
            base: [0.0; 3],
            normal: [0.0, 0.0, 1.0],
            data: vec![1; 64],
            heights: vec![0.0; 64],
            surfaces: Vec::new(),
        });
        sim.area_layouts.insert(area, "test".into());
        sim.nav_grids.insert("test".into(), Some(grid.clone()));
        (sim, area, hero, grid)
    }

    #[test]
    fn doors_regression_f_walkers_route_around_solid_scenery() {
        let (mut s, _, hero, _) = scene();
        let wall = Arc::new(Collision::from_triangles(&[
            [[3.0, 1.0, 0.0], [3.0, 6.0, 0.0], [3.0, 6.0, 3.0]],
            [[3.0, 1.0, 0.0], [3.0, 6.0, 3.0], [3.0, 1.0, 3.0]],
        ]));
        s.collisions.insert("test".into(), Some(wall.clone()));
        s.order_move(hero, [5.5, 3.5, 0.0]);
        s.tick(0);
        let path = &s.world.get(hero).unwrap().commands[0].path;
        assert!(
            path.iter().any(|p| p[1] < 1.0 || p[1] > 6.0),
            "route cuts through the crate: {path:?}"
        );
        let mut was = s.world.get(hero).unwrap().position;
        for _ in 0..250 {
            s.tick(33);
            let now = s.world.get(hero).unwrap().position;
            assert!(
                !wall.sight_blocked(was, now),
                "body crossed scenery: {was:?} -> {now:?}"
            );
            was = now;
        }
        assert!(
            crate::engine::dist(was, [5.5, 3.5, 0.0]) < 0.05,
            "stuck at {was:?}"
        );
    }

    #[test]
    fn doors_regression_b_body_contact_keeps_a_required_corner() {
        let (mut s, area, hero, grid) = scene();
        let mut grid = (*grid).clone();
        grid.cell_size = 0.5;
        grid.data[3 * grid.columns as usize + 3] = 9;
        s.changed_nav_grids.insert(area, Arc::new(grid));
        s.world.get_mut(hero).unwrap().position = [1.25, 1.75, 0.0];
        let body = s.world.spawn(ObjectKind::Creature, "at_corner");
        let o = s.world.get_mut(body).unwrap();
        o.area = Some(area);
        o.position = [1.25, 1.25, 0.0];
        let goal = [2.25, 1.25, 0.0];
        s.order_move(hero, goal);
        for _ in 0..10 {
            s.tick(33);
        }
        // The body leaves, as a companion or enemy does. The retry still needs
        // the corner; skipping it would cross the blocked cell diagonally.
        s.world.get_mut(body).unwrap().active = false;
        for _ in 0..150 {
            s.tick(33);
        }
        let p = s.world.get(hero).unwrap().position;
        assert!(
            crate::engine::dist(p, goal) < 0.05,
            "corner lost: {p:?}, {:?}",
            s.world.get(hero).unwrap().commands
        );
    }

    #[test]
    fn doors_regression_b_large_ticks_sweep_bodies_along_the_route() {
        let (mut s, area, hero, _) = scene();
        let start = [1.5, 1.5, 0.0];
        let goal = [5.5, 5.5, 0.0];
        s.world.get_mut(hero).unwrap().position = start;
        let body = s.world.spawn(ObjectKind::Creature, "inside_the_corner");
        let o = s.world.get_mut(body).unwrap();
        o.area = Some(area);
        o.position = [3.5, 3.5, 0.0];
        s.order_move(hero, goal);
        s.tick(0);
        let c = &mut s.world.get_mut(hero).unwrap().commands[0];
        c.path = vec![[1.5, 5.5, 0.0], goal];
        c.due = Some(2_000);
        s.tick(2_000);
        let p = s.world.get(hero).unwrap().position;
        assert!(
            crate::engine::dist(p, goal) < 0.05,
            "swept the chord: {p:?}"
        );
    }

    #[test]
    fn doors_regression_b_a_free_side_cell_is_not_a_blocked_diagonal() {
        let (mut s, area, hero, grid) = scene();
        let mut grid = (*grid).clone();
        grid.data[3 * grid.columns as usize + 3] = 9;
        s.changed_nav_grids.insert(area, Arc::new(grid));
        let a = [2.99, 3.01, 0.0];
        let b = [3.01, 2.95, 0.0];
        // The segment enters (2,2), then (3,2). A short tick must not
        // mistake its endpoint cells for a diagonal through blocked (3,3).
        assert_eq!(s.scenery_step(hero, a, b), b);
        assert_eq!(s.scenery_step(hero, b, a), a);
    }

    #[test]
    fn doors_regression_c_an_unreachable_use_slot_does_not_retry_at_the_users_feet() {
        let (mut s, area, hero, grid) = scene();
        let mut grid = (*grid).clone();
        for y in 0..grid.rows {
            for x in 2..grid.columns {
                grid.data[(y * grid.columns + x) as usize] = 9;
            }
        }
        s.changed_nav_grids.insert(area, Arc::new(grid));
        s.world.get_mut(hero).unwrap().position = [1.5, 1.5, 0.0];
        let target = s.world.spawn(ObjectKind::Placeable, "unreachable");
        let o = s.world.get_mut(target).unwrap();
        o.area = Some(area);
        o.position = [3.0, 1.5, 0.0];
        s.world.trace.keep = true;
        s.order_use(hero, target);
        s.tick(500);
        assert!(s.world.get(hero).unwrap().commands.is_empty());
        assert!(s.world.trace.entries.iter().any(|e| matches!(e,
            crate::trace::TraceEntry::Command {command, status, ..}
            if command == "CommandUseObject" && status.starts_with("failed"))));
    }

    #[test]
    fn doors_regression_g_stairs_keep_the_body_on_each_floor_sample() {
        let (mut s, area, hero, grid) = scene();
        let mut grid = (*grid).clone();
        for y in 0..grid.rows {
            for x in 0..grid.columns {
                grid.heights[(y * grid.columns + x) as usize] = x as f32 * 5.0;
            }
        }
        let grid = Arc::new(grid);
        s.changed_nav_grids.insert(area, grid.clone());
        s.world.get_mut(hero).unwrap().position = [0.5, 3.5, 0.0];
        s.order_move(hero, [6.5, 3.5, 1.875]);
        for _ in 0..100 {
            s.tick(33);
            let p = s.world.get(hero).unwrap().position;
            let floor = crate::nav::height(&grid, grid.cell(p).unwrap());
            assert!(
                (p[2] - floor).abs() < 1e-4,
                "body floats on stairs: {p:?}, floor={floor}"
            );
        }
        assert!((s.world.get(hero).unwrap().position[0] - 6.5).abs() < 0.05);
    }

    #[test]
    fn doors_regression_g_use_approach_arrives_on_the_stair_floor() {
        let (mut s, area, hero, grid) = scene();
        let mut grid = (*grid).clone();
        for y in 0..grid.rows {
            for x in 0..grid.columns {
                grid.heights[(y * grid.columns + x) as usize] = x as f32 * 5.0;
            }
        }
        let grid = Arc::new(grid);
        s.changed_nav_grids.insert(area, grid.clone());
        s.world.get_mut(hero).unwrap().position = [0.5, 3.5, 0.0];
        let target = s.world.spawn(ObjectKind::Placeable, "lever");
        let o = s.world.get_mut(target).unwrap();
        o.area = Some(area);
        o.position = [6.5, 3.5, 1.875];
        s.world.trace.keep = true;
        s.order_use(hero, target);
        for _ in 0..150 {
            s.tick(33);
            let p = s.world.get(hero).unwrap().position;
            let floor = crate::nav::height(&grid, grid.cell(p).unwrap());
            assert!(
                (p[2] - floor).abs() < 1e-4,
                "use approach left the floor: {p:?}, {floor}"
            );
        }
        assert!(s.world.trace.entries.iter().any(|e| matches!(e,
            crate::trace::TraceEntry::Command {command, status, ..}
            if command == "CommandUseObject" && status == "complete")));
    }

    #[test]
    fn doors_regression_f_scenery_blocks_a_blow_already_in_reach() {
        let (mut s, area, hero, _) = scene();
        let enemy = s.world.spawn(ObjectKind::Creature, "enemy");
        let o = s.world.get_mut(enemy).unwrap();
        o.area = Some(area);
        o.position = [3.5, 3.5, 0.0];
        let wall = Arc::new(Collision::from_triangles(&[
            [[2.5, 1.0, 0.0], [2.5, 6.0, 0.0], [2.5, 6.0, 3.0]],
            [[2.5, 1.0, 0.0], [2.5, 6.0, 3.0], [2.5, 1.0, 3.0]],
        ]));
        s.collisions.insert("test".into(), Some(wall));
        assert!(!s.attack_sight_clear(hero, enemy));
        s.world.get_mut(enemy).unwrap().position = [1.5, 5.5, 0.0];
        assert!(s.attack_sight_clear(hero, enemy));
    }

    #[test]
    fn doors_regression_f_short_creatures_cannot_swing_through_head_high_crates() {
        let (mut s, area, hero, _) = scene();
        let enemy = s.world.spawn(ObjectKind::Creature, "enemy");
        let o = s.world.get_mut(enemy).unwrap();
        o.area = Some(area);
        o.position = [3.5, 3.5, 0.0];
        let tables = crate::m2da::Tables::empty();
        tables.insert(
            "apr_base",
            &["HEIGHT", "HEIGHT_F"],
            vec![
                (3, vec![Cell::Float(1.4), Cell::Float(1.3)]),
                (4, vec![Cell::Float(1.8), Cell::Float(1.7)]),
            ],
        );
        s.world.tables = Arc::new(tables);
        let wall = Collision::from_boxes(&[([2.5, 3.5, 0.8], [0.4, 1.5, 0.8])]);
        s.collisions.insert("test".into(), Some(Arc::new(wall)));
        for id in [hero, enemy] {
            s.world.get_mut(id).unwrap().appearance = 3;
        }
        assert!(
            !s.attack_sight_clear(hero, enemy),
            "perception's 1.8 m ray misses the crate"
        );
        for id in [hero, enemy] {
            s.world.get_mut(id).unwrap().appearance = 4;
        }
        assert!(
            s.attack_sight_clear(hero, enemy),
            "taller creatures can see over this cover"
        );
    }

    #[test]
    fn doors_regression_f_solid_footprints_cannot_take_the_flat_fallback() {
        let (mut s, area, hero, grid) = scene();
        let mut grid = (*grid).clone();
        for y in 0..grid.rows {
            grid.data[(y * grid.columns + 3) as usize] = 8;
        }
        s.changed_nav_grids.insert(area, Arc::new(grid));
        let wall = Arc::new(Collision::from_triangles(&[
            [[3.25, 0.0, 0.0], [3.25, 8.0, 0.0], [3.25, 8.0, 3.0]],
            [[3.25, 0.0, 0.0], [3.25, 8.0, 3.0], [3.25, 0.0, 3.0]],
        ]));
        s.collisions.insert("test".into(), Some(wall));
        assert_eq!(
            s.route_to(hero, [5.5, 3.5, 0.0]),
            crate::nav::Routed::InvalidPath
        );
    }
}
