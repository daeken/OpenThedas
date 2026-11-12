//! Routing on the layout's pathing grid (`.arl`, see `thedas_formats::level::NavGrid`).
//!
//! Provisional, inferred from the data: a cell is walkable when bit 0 of its data byte is
//! set and bit 3 is clear (cells are 1, 9 or 10: of the creatures placed in five areas, 283
//! of 287 stand on 1s; 9s form the walls between rooms, with gaps where doors stand), its
//! height is the stored value in sixteenths of a metre, and a creature can step between
//! neighbouring cells whose heights differ by at most [`MAX_STEP`]. Routes are 8-connected
//! A* without cutting blocked corners.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};
use std::sync::Arc;

use thedas_formats::level::NavGrid;

use crate::sim::Sim;

/// The largest height change between neighbouring cells a walker can take, in metres.
pub const MAX_STEP: f32 = 0.6;

/// Longest route searched, in cells expanded.
const SEARCH_LIMIT: usize = 400_000;

pub fn walkable(g: &NavGrid, c: (u32, u32)) -> bool {
    let v = g.value(c);
    v & 1 != 0 && v & 8 == 0
}

/// A cell's floor height in metres.
pub fn height(g: &NavGrid, (x, y): (u32, u32)) -> f32 {
    g.heights
        .get((y * g.columns + x) as usize)
        .map_or(g.base[2], |h| h / 16.0)
}

/// The walkable cell nearest a position (searching outwards a few metres).
fn nearest_walkable(g: &NavGrid, p: [f32; 3]) -> Option<(u32, u32)> {
    let (cx, cy) = g.cell(p)?;
    for r in 0..12i32 {
        let mut best: Option<((u32, u32), f32)> = None;
        for dy in -r..=r {
            for dx in -r..=r {
                if dx.abs() != r && dy.abs() != r {
                    continue;
                }
                let (x, y) = (cx as i32 + dx, cy as i32 + dy);
                if x < 0 || y < 0 || x as u32 >= g.columns || y as u32 >= g.rows {
                    continue;
                }
                let c = (x as u32, y as u32);
                if !walkable(g, c) {
                    continue;
                }
                let dz = (height(g, c) - p[2]).abs();
                if best.is_none_or(|b| dz < b.1) {
                    best = Some((c, dz));
                }
            }
        }
        if let Some((c, _)) = best {
            return Some(c);
        }
    }
    None
}

/// Every cell reachable on foot from `from`'s nearest walkable cell (by the same rules as
/// [`route`]), as a flag per cell in row-major order; empty when `from` is off the grid.
pub fn region(g: &NavGrid, from: [f32; 3]) -> Vec<bool> {
    let Some(start) = nearest_walkable(g, from) else {
        return Vec::new();
    };
    let idx = |c: (u32, u32)| (c.1 * g.columns + c.0) as usize;
    let mut seen = vec![false; (g.columns * g.rows) as usize];
    seen[idx(start)] = true;
    let mut todo = vec![start];
    while let Some(c) = todo.pop() {
        let hc = height(g, c);
        for (dx, dy) in [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)] {
            let (x, y) = (c.0 as i32 + dx, c.1 as i32 + dy);
            if x < 0 || y < 0 || x as u32 >= g.columns || y as u32 >= g.rows {
                continue;
            }
            let n = (x as u32, y as u32);
            if !seen[idx(n)] && walkable(g, n) && (height(g, n) - hc).abs() <= MAX_STEP {
                seen[idx(n)] = true;
                todo.push(n);
            }
        }
    }
    seen
}

/// The route from `from` to `to` as world points (cell centres at floor height, ending at
/// `to`), or `None` when either end is off the grid or no route exists.
pub fn route(g: &NavGrid, from: [f32; 3], to: [f32; 3]) -> Option<Vec<[f32; 3]>> {
    route_avoiding(g, from, to, &std::collections::HashSet::new())
}

/// [`route`], treating the cells in `blocked` (row-major indices) as unwalkable too.
pub fn route_avoiding(
    g: &NavGrid,
    from: [f32; 3],
    to: [f32; 3],
    blocked: &std::collections::HashSet<usize>,
) -> Option<Vec<[f32; 3]>> {
    let start = nearest_walkable(g, from)?;
    let goal = nearest_walkable(g, to)?;
    let idx = |c: (u32, u32)| (c.1 * g.columns + c.0) as usize;
    let h = |c: (u32, u32)| {
        let (dx, dy) = (
            (c.0 as f32 - goal.0 as f32).abs(),
            (c.1 as f32 - goal.1 as f32).abs(),
        );
        dx.max(dy) + (std::f32::consts::SQRT_2 - 1.0) * dx.min(dy)
    };
    let mut open = BinaryHeap::new();
    let mut cost: HashMap<usize, f32> = HashMap::new();
    let mut came: HashMap<usize, (u32, u32)> = HashMap::new();
    cost.insert(idx(start), 0.0);
    open.push(Reverse(((h(start) * 1000.0) as u64, start)));
    let mut expanded = 0;
    while let Some(Reverse((_, c))) = open.pop() {
        if c == goal {
            let mut cells = vec![c];
            let mut cur = c;
            while let Some(&p) = came.get(&idx(cur)) {
                cells.push(p);
                cur = p;
            }
            cells.reverse();
            let mut pts: Vec<[f32; 3]> = cells
                .iter()
                .skip(1)
                .map(|&c| {
                    let m = g.center(c);
                    [m[0], m[1], height(g, c)]
                })
                .collect();
            pts.pop();
            pts.push(to);
            return Some(pts);
        }
        expanded += 1;
        if expanded > SEARCH_LIMIT {
            return None;
        }
        let base = cost.get(&idx(c)).copied().unwrap_or(f32::MAX);
        let hc = height(g, c);
        for (dx, dy) in [
            (-1, 0),
            (1, 0),
            (0, -1),
            (0, 1),
            (-1, -1),
            (-1, 1),
            (1, -1),
            (1, 1),
        ] {
            let (x, y) = (c.0 as i32 + dx, c.1 as i32 + dy);
            if x < 0 || y < 0 || x as u32 >= g.columns || y as u32 >= g.rows {
                continue;
            }
            let n = (x as u32, y as u32);
            let free = |c: (u32, u32)| walkable(g, c) && !blocked.contains(&idx(c));
            if !free(n) || (height(g, n) - hc).abs() > MAX_STEP {
                continue;
            }
            // No cutting corners past blocked cells.
            if dx != 0 && dy != 0 {
                let a = ((c.0 as i32 + dx) as u32, c.1);
                let b = (c.0, (c.1 as i32 + dy) as u32);
                if !free(a) || !free(b) {
                    continue;
                }
            }
            let step = if dx != 0 && dy != 0 {
                std::f32::consts::SQRT_2
            } else {
                1.0
            };
            let nc = base + step;
            if nc < cost.get(&idx(n)).copied().unwrap_or(f32::MAX) {
                cost.insert(idx(n), nc);
                came.insert(idx(n), c);
                open.push(Reverse((((nc + h(n)) * 1000.0) as u64, n)));
            }
        }
    }
    None
}

/// `placeable_types.gda`: a placeable appearance's state controller (`StateCnt_Door`,
/// `StateCnt_AreaTransition`, ...). The column's name is not known.
const COL_STATE_CONTROLLER: u32 = 0x9a32_0a51;

/// How far a closed door's doorway reaches from the door, in metres. Provisional: the gaps
/// the pathing grid leaves in walls where doors stand are about 2.5 m wide.
pub const DOORWAY_RADIUS: f32 = 1.25;

/// A door and the grid cells of its doorway (row-major indices).
pub type Doorway = (thedas_script::vm::ObjectId, Vec<usize>);

/// How a mover gets somewhere (see `Sim::route_to`).
#[derive(Clone, Debug, PartialEq)]
pub enum Routed {
    /// Along these waypoints, ending at the destination.
    Path(Vec<[f32; 3]>),
    /// Only through this closed door.
    Door(thedas_script::vm::ObjectId),
    /// No route known (no grid, an end off it, or no way at all): a straight line.
    Straight,
}

/// Whether a wall of the pathing grid lies on the straight line between two points: a
/// cell valued 9 (bits 0 and 3: the walls between rooms indoors, buildings outdoors) sampled
/// every half cell, leaving out the first and last 0.75 m (a creature against a wall still
/// sees). Cells valued 10 (terrain beyond the walkable mesh, the void around a level) and
/// other unwalkable cells do not block. Provisional: the original most likely tests sight
/// against the level geometry, which the simulation does not have; large furniture, rubble
/// and some water are marked 9 too, so they block.
pub fn wall_between(g: &NavGrid, a: [f32; 3], b: [f32; 3]) -> bool {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let len = (dx * dx + dy * dy).sqrt();
    if len <= 1.5 {
        return false;
    }
    let step = (g.cell_size * 0.5).max(0.05);
    let mut d = 0.75;
    while d < len - 0.75 {
        let t = d / len;
        let p = [a[0] + dx * t, a[1] + dy * t, a[2] + (b[2] - a[2]) * t];
        if g.cell(p).is_some_and(|c| g.value(c) == 9) {
            return true;
        }
        d += step;
    }
    false
}

impl Sim {
    /// The closed doors of an area with the pathing-grid cells of their doorways: doors are
    /// the placeables whose appearance's state controller (`placeable_types.gda`) is
    /// `StateCnt_Door`; a door is closed in states 0 and 1 (`statecnt_door.gda`:
    /// Closed_Unlocked, Closed_Locked), its state being field `State` (`SetPlaceableState`)
    /// or else the template's `AnimContState`. The doorway is the walkable cells within
    /// [`DOORWAY_RADIUS`] of the door.
    pub fn closed_doors(
        &mut self,
        area: thedas_script::vm::ObjectId,
    ) -> Vec<(thedas_script::vm::ObjectId, Vec<usize>)> {
        let doors = match self.door_cells.get(&area) {
            Some(d) => d.clone(),
            None => {
                let d = Arc::new(self.doorways(area));
                self.door_cells.insert(area, d.clone());
                d
            }
        };
        doors
            .iter()
            .filter(|(id, _)| {
                self.world.get(*id).is_some_and(|o| {
                    let state = o
                        .fields
                        .get("State")
                        .or_else(|| o.fields.get("AnimContState"))
                        .copied()
                        .unwrap_or(0);
                    o.active && (state == 0 || state == 1)
                })
            })
            .cloned()
            .collect()
    }

    /// Every door of an area with its doorway's grid cells, open or not.
    fn doorways(
        &mut self,
        area: thedas_script::vm::ObjectId,
    ) -> Vec<(thedas_script::vm::ObjectId, Vec<usize>)> {
        let Some(g) = self.nav_grid(area) else {
            return Vec::new();
        };
        let tables = self.world.tables();
        let types = tables.get(0, "placeable_types");
        let is_door = |appearance: i32| {
            types
                .as_ref()
                .and_then(
                    |t| match t.cell_by_hash(COL_STATE_CONTROLLER, appearance as i64)? {
                        thedas_formats::gda::Cell::String(s) => {
                            Some(crate::engine::clean(s).eq_ignore_ascii_case("StateCnt_Door"))
                        }
                        _ => None,
                    },
                )
                .unwrap_or(false)
        };
        let mut out = Vec::new();
        for o in self.world.objects.values() {
            if o.kind != crate::world::ObjectKind::Placeable
                || o.area != Some(area)
                || !is_door(o.appearance)
            {
                continue;
            }
            let r = (DOORWAY_RADIUS / g.cell_size).ceil() as i32;
            let Some((cx, cy)) = g.cell(o.position) else {
                continue;
            };
            let mut cells = Vec::new();
            for dy in -r..=r {
                for dx in -r..=r {
                    let (x, y) = (cx as i32 + dx, cy as i32 + dy);
                    if x < 0 || y < 0 || x as u32 >= g.columns || y as u32 >= g.rows {
                        continue;
                    }
                    let c = (x as u32, y as u32);
                    let m = g.center(c);
                    if walkable(&g, c)
                        && crate::engine::dist(m, [o.position[0], o.position[1], m[2]])
                            <= DOORWAY_RADIUS
                    {
                        cells.push((c.1 * g.columns + c.0) as usize);
                    }
                }
            }
            out.push((o.id, cells));
        }
        out
    }

    /// Whether one point of an area can see another: no closed door's doorway and no wall
    /// of the pathing grid on the line between them (see [`wall_between`]). Provisional.
    pub fn line_of_sight(
        &mut self,
        area: thedas_script::vm::ObjectId,
        a: [f32; 3],
        b: [f32; 3],
    ) -> bool {
        let doors: Vec<[f32; 3]> = self
            .closed_doors(area)
            .into_iter()
            .filter_map(|(d, _)| self.world.get(d).map(|o| o.position))
            .collect();
        if Sim::door_between(&doors, a, b) {
            return false;
        }
        !self.nav_grid(area).is_some_and(|g| wall_between(&g, a, b))
    }

    /// Whether the straight line between two points passes through a closed door's
    /// doorway: within [`DOORWAY_RADIUS`] of one of `doors` (their positions), leaving out
    /// the first and last 0.75 m (two creatures standing by a door see each other).
    pub fn door_between(doors: &[[f32; 3]], a: [f32; 3], b: [f32; 3]) -> bool {
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let len2 = dx * dx + dy * dy;
        let len = len2.sqrt();
        doors.iter().any(|p| {
            let t = if len2 > 0.0 {
                (((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / len2).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let along = t * len;
            if along < 0.75 || len - along < 0.75 {
                return false;
            }
            let (qx, qy) = (a[0] + dx * t, a[1] + dy * t);
            ((p[0] - qx).powi(2) + (p[1] - qy).powi(2)).sqrt() <= DOORWAY_RADIUS
        })
    }

    /// The pathing grid of an area (by its object id), loaded once per layout.
    pub fn nav_grid(&mut self, area: thedas_script::vm::ObjectId) -> Option<Arc<NavGrid>> {
        let name = self.area_layout(area)?;
        if let Some(g) = self.nav_grids.get(&name) {
            return g.clone();
        }
        let grid = self.load_nav_grid(&name).map(Arc::new);
        self.nav_grids.insert(name, grid.clone());
        grid
    }

    /// The layout (environment level) an area is built on, from its `.are`.
    pub fn area_layout(&mut self, area: thedas_script::vm::ObjectId) -> Option<String> {
        let name = match self.area_layouts.get(&area) {
            Some(n) => n.clone(),
            None => {
                // After a load: find the layout from the area's file again.
                let template = self.world.get(area)?.template.clone();
                let (data, _) = self.vfs.read(&format!("{template}.are")).ok()?;
                let name = thedas_formats::Area::parse(&data).ok()?.layout?;
                self.area_layouts.insert(area, name.clone());
                name
            }
        };
        Some(name)
    }

    /// An area's collision for sight lines: its layout's models' `.phy` shapes (see
    /// [`crate::collision`]). `None` without an install to read the level from, or when
    /// the layout is missing.
    pub fn collision(
        &mut self,
        area: thedas_script::vm::ObjectId,
    ) -> Option<Arc<crate::collision::Collision>> {
        let name = self.area_layout(area)?;
        if let Some(c) = self.collisions.get(&name) {
            return c.clone();
        }
        let c = self.load_collision(&name).map(Arc::new);
        self.collisions.insert(name, c.clone());
        c
    }

    fn load_collision(&self, layout: &str) -> Option<crate::collision::Collision> {
        let root = self.install_root.as_ref()?;
        let install = thedas_vfs::DaoInstall::open(root).ok()?;
        let mut level = thedas_vfs::Vfs::new();
        install.mount_level(&mut level, layout).ok()?;
        let (data, _) = level
            .read(&format!("{layout}.arl"))
            .or_else(|_| self.vfs.read(&format!("{layout}.arl")))
            .ok()?;
        let arl = thedas_formats::Arl::parse(&data).ok()?;
        Some(crate::collision::load_layout(&level, &self.vfs, &arl))
    }

    fn load_nav_grid(&self, layout: &str) -> Option<NavGrid> {
        let file = format!("{layout}.arl");
        let data = match self.vfs.read(&file) {
            Ok((d, _)) => d,
            Err(_) => {
                let root = self.install_root.as_ref()?;
                let install = thedas_vfs::DaoInstall::open(root).ok()?;
                let dir = install
                    .env_dirs()
                    .ok()?
                    .into_iter()
                    .find(|d| d.join(&file).is_file())?;
                std::fs::read(dir.join(&file)).ok()?
            }
        };
        thedas_formats::Arl::parse(&data).ok()?.grid
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 5x3 grid with a wall down column 2 except at the top row.
    fn grid() -> NavGrid {
        let (w, h) = (5u32, 3u32);
        let mut data = vec![1u8; (w * h) as usize];
        for y in 0..2 {
            data[(y * w + 2) as usize] = 10;
        }
        NavGrid {
            columns: w,
            rows: h,
            cell_size: 1.0,
            clearance: 2.0,
            base: [0.0, 0.0, 0.0],
            normal: [0.0, 0.0, 1.0],
            data,
            heights: vec![0.0; (w * h) as usize],
        }
    }

    #[test]
    fn walls_block_sight_and_the_void_does_not() {
        // A row of 9s (a wall) at x = 5 with a 10 (void) beside it at x = 7.
        let (w, h) = (10u32, 3u32);
        let mut data = vec![1u8; (w * h) as usize];
        for y in 0..h {
            data[(y * w + 5) as usize] = 9;
            data[(y * w + 7) as usize] = 10;
        }
        let g = NavGrid {
            columns: w,
            rows: h,
            cell_size: 1.0,
            clearance: 2.0,
            base: [0.0, 0.0, 0.0],
            normal: [0.0, 0.0, 1.0],
            data,
            heights: vec![0.0; (w * h) as usize],
        };
        let at = |x: f32| [x, 1.5, 0.0];
        assert!(wall_between(&g, at(1.5), at(8.5)), "through the wall");
        assert!(!wall_between(&g, at(1.5), at(4.5)), "same side");
        assert!(!wall_between(&g, at(6.5), at(9.5)), "across the void");
        assert!(
            !wall_between(&g, at(4.9), at(1.0)),
            "against the wall, along its side"
        );
        assert!(
            wall_between(&g, at(4.6), at(6.4)),
            "either side of a thin wall"
        );
    }

    #[test]
    fn routes_around_walls() {
        let g = grid();
        let r = route(&g, [0.5, 0.5, 0.0], [4.5, 0.5, 0.0]).unwrap();
        assert_eq!(*r.last().unwrap(), [4.5, 0.5, 0.0]);
        // It goes through the gap in the top row.
        assert!(r.iter().any(|p| p[1] > 2.0 && (p[0] - 2.5).abs() < 0.01));
        // No point lands in the wall.
        for p in &r {
            assert!(walkable(&g, g.cell(*p).unwrap()), "{p:?}");
        }
    }

    #[test]
    fn regions_end_at_walls() {
        let g = grid();
        let r = region(&g, [0.5, 0.5, 0.0]);
        assert_eq!(r.len(), 15);
        assert!(r[4], "the far corner is reachable through the gap");
        assert!(!r[2] && !r[7], "wall cells are not");
        let mut g = g;
        g.heights[(2 * 5 + 2) as usize] = 16.0;
        assert!(
            !region(&g, [0.5, 0.5, 0.0])[4],
            "nor is anything past a step too high"
        );
    }

    #[test]
    fn steps_too_high_block() {
        let mut g = grid();
        // Raise the gap cell by a metre (16 units per metre).
        g.heights[(2 * 5 + 2) as usize] = 16.0;
        assert!(route(&g, [0.5, 0.5, 0.0], [4.5, 0.5, 0.0]).is_none());
    }

    #[test]
    fn creatures_move_along_routes_round_walls() {
        use crate::world::{Command, Location, ObjectKind};
        use thedas_script::EngineTable;
        use thedas_vfs::Vfs;
        let mut s = Sim::new(Arc::new(Vfs::new()), Arc::new(EngineTable::default()), 1);
        let area = s.world.spawn(ObjectKind::Area, "a");
        s.area_layouts.insert(area, "walled".into());
        s.nav_grids.insert("walled".into(), Some(Arc::new(grid())));
        let c = s.world.spawn(ObjectKind::Creature, "walker");
        {
            let o = s.world.get_mut(c).unwrap();
            o.area = Some(area);
            o.position = [0.5, 0.5, 0.0];
        }
        let to = [4.5, 0.5, 0.0];
        s.world.get_mut(c).unwrap().commands.push(Command {
            ty: 0,
            name: "CommandMoveToLocation".into(),
            target: None,
            location: Some(Location {
                area,
                position: to,
                facing: 0.0,
            }),
            seconds: 0.0,
            ints: Vec::new(),
            strings: Vec::new(),
            due: None,
            from: None,
            approach: None,
            ring_wait: false,
            after_impact_ms: None,
            impacted: false,
            path: Vec::new(),
        });
        // Four metres in a straight line, but the wall sends it up through the gap: the
        // route is longer, and halfway along it the walker is in the top row, not inside
        // the wall.
        s.tick(1);
        let path = s.world.get(c).unwrap().commands[0].path.clone();
        assert!(path.len() > 2, "{path:?}");
        let mut prev = [0.5, 0.5, 0.0];
        let mut len = 0.0;
        for p in &path {
            len += crate::engine::dist(prev, *p);
            prev = *p;
        }
        assert!(len > 4.5, "{len}");
        let due = s.world.get(c).unwrap().commands[0].due.unwrap();
        let half = (due - s.world.time_ms) / 2;
        s.tick(half);
        let p = s.world.get(c).unwrap().position;
        assert!(p[1] >= 1.5, "halfway it is round the wall's end: {p:?}");
        s.tick(10_000);
        let o = s.world.get(c).unwrap();
        assert!(o.commands.is_empty());
        assert_eq!(o.position, to);
    }

    #[test]
    fn a_mover_goes_by_a_door_it_can_open_rather_than_a_locked_one() {
        use crate::world::ObjectKind;
        use thedas_formats::gda::Cell;
        use thedas_script::EngineTable;
        use thedas_vfs::Vfs;
        let mut s = Sim::new(Arc::new(Vfs::new()), Arc::new(EngineTable::default()), 1);
        let t = crate::m2da::Tables::empty();
        t.insert(
            "placeable_types",
            &["ID", "Label", "#9a320a51"],
            vec![(
                93,
                vec![
                    Cell::Int(93),
                    Cell::String("Door".into()),
                    Cell::String("StateCnt_Door".into()),
                ],
            )],
        );
        s.world.tables = Arc::new(t);
        // A wall down x = 2 with a gap at each end: the near one locked, the far one not.
        let (w, h) = (5u32, 7u32);
        let mut data = vec![1u8; (w * h) as usize];
        for y in 1..h - 1 {
            data[(y * w + 2) as usize] = 10;
        }
        let g = NavGrid {
            columns: w,
            rows: h,
            cell_size: 1.0,
            clearance: 2.0,
            base: [0.0, 0.0, 0.0],
            normal: [0.0, 0.0, 1.0],
            data,
            heights: vec![0.0; (w * h) as usize],
        };
        let area = s.world.spawn(ObjectKind::Area, "a");
        s.area_layouts.insert(area, "two_doors".into());
        s.nav_grids.insert("two_doors".into(), Some(Arc::new(g)));
        let locked = s.world.spawn(ObjectKind::Placeable, "locked");
        let unlocked = s.world.spawn(ObjectKind::Placeable, "unlocked");
        for (id, y, state) in [(locked, 0.5, 1), (unlocked, 6.5, 0)] {
            let o = s.world.get_mut(id).unwrap();
            o.area = Some(area);
            o.appearance = 93;
            o.position = [2.5, y, 0.0];
            o.fields.insert("State".into(), state);
        }
        let walker = s.world.spawn(ObjectKind::Creature, "walker");
        {
            let o = s.world.get_mut(walker).unwrap();
            o.area = Some(area);
            o.position = [0.5, 1.5, 0.0];
        }
        assert_eq!(s.closed_doors(area).len(), 2);
        assert_eq!(s.route_to(walker, [4.5, 1.5, 0.0]), Routed::Door(unlocked));
        // With both locked, the shortest way is still the one tried.
        s.world
            .get_mut(unlocked)
            .unwrap()
            .fields
            .insert("State".into(), 1);
        assert_eq!(s.route_to(walker, [4.5, 1.5, 0.0]), Routed::Door(locked));
    }

    #[test]
    fn closed_doors_block_their_doorways_until_opened() {
        use crate::world::ObjectKind;
        use thedas_formats::gda::Cell;
        use thedas_script::EngineTable;
        use thedas_vfs::Vfs;
        let mut s = Sim::new(Arc::new(Vfs::new()), Arc::new(EngineTable::default()), 1);
        let t = crate::m2da::Tables::empty();
        t.insert(
            "placeable_types",
            &["ID", "Label", "#9a320a51"],
            vec![
                (
                    93,
                    vec![
                        Cell::Int(93),
                        Cell::String("Door".into()),
                        Cell::String("StateCnt_Door".into()),
                    ],
                ),
                (
                    5,
                    vec![
                        Cell::Int(5),
                        Cell::String("Chest".into()),
                        Cell::String("StateCnt_Container".into()),
                    ],
                ),
            ],
        );
        s.world.tables = Arc::new(t);
        let area = s.world.spawn(ObjectKind::Area, "a");
        s.area_layouts.insert(area, "walled".into());
        let g = Arc::new(grid());
        s.nav_grids.insert("walled".into(), Some(g.clone()));
        // A door in the wall's only gap (the top row), and a chest that does not block.
        let door = s.world.spawn(ObjectKind::Placeable, "door");
        let chest = s.world.spawn(ObjectKind::Placeable, "chest");
        for (id, app, x) in [(door, 93, 2.5), (chest, 5, 0.5)] {
            let o = s.world.get_mut(id).unwrap();
            o.area = Some(area);
            o.appearance = app;
            o.position = [x, 2.5, 0.0];
        }
        let walker = s.world.spawn(ObjectKind::Creature, "walker");
        {
            let o = s.world.get_mut(walker).unwrap();
            o.area = Some(area);
            o.position = [0.5, 0.5, 0.0];
        }
        let doors = s.closed_doors(area);
        assert_eq!(doors.len(), 1);
        assert_eq!(doors[0].0, door);
        assert_eq!(s.route_to(walker, [4.5, 0.5, 0.0]), Routed::Door(door));
        // Walking to the door itself goes up to it and stops where it would enter it.
        match s.route_to(walker, [2.5, 2.5, 0.0]) {
            Routed::Path(p) => {
                let end = *p.last().unwrap();
                let i = g
                    .cell(end)
                    .map(|c| (c.1 * g.columns + c.0) as usize)
                    .unwrap();
                assert!(!doors[0].1.contains(&i), "stops outside the door");
                assert!(
                    crate::engine::dist(end, [2.5, 2.5, 0.0]) < 2.5,
                    "next to it"
                );
            }
            r => panic!("walking to the door: {r:?}"),
        }
        let at = [[2.5, 2.5, 0.0]];
        assert!(Sim::door_between(&at, [0.5, 2.5, 0.0], [4.5, 2.5, 0.0]));
        assert!(!Sim::door_between(&at, [0.5, 0.5, 0.0], [0.5, 2.5, 0.0]));
        assert!(
            !Sim::door_between(&at, [2.0, 2.5, 0.0], [3.0, 2.5, 0.0]),
            "both by the door"
        );
        let _ = &g;
        // Opened (SetPlaceableState to Open), the way is clear.
        s.world
            .get_mut(door)
            .unwrap()
            .fields
            .insert("State".into(), 2);
        assert!(s.closed_doors(area).is_empty());
        assert!(matches!(
            s.route_to(walker, [4.5, 0.5, 0.0]),
            Routed::Path(_)
        ));
    }
}
