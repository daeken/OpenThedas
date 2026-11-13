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

/// Whether a point is over a walkable cell of the grid.
pub fn on_walkable(g: &NavGrid, p: [f32; 3]) -> bool {
    g.cell(p).is_some_and(|c| walkable(g, c))
}

/// The middle of the walkable cell nearest a point (within six metres of it, as routes
/// look for their ends), at the cell's floor height.
pub fn nearest_walkable_point(g: &NavGrid, p: [f32; 3]) -> Option<[f32; 3]> {
    let c = nearest_walkable(g, p)?;
    let m = g.center(c);
    Some([m[0], m[1], height(g, c)])
}

/// How far a straight walk from `from` towards `to` stays on the walkable area: the last
/// point, sampled every half cell, before the line leaves the grid or meets an unwalkable
/// cell (`to` itself when it never does). `None` when `from` is not on walkable ground.
pub fn walkable_reach(g: &NavGrid, from: [f32; 3], to: [f32; 3]) -> Option<[f32; 3]> {
    let on = |p: [f32; 3]| on_walkable(g, p);
    if !on(from) {
        return None;
    }
    let (dx, dy) = (to[0] - from[0], to[1] - from[1]);
    let len = (dx * dx + dy * dy).sqrt();
    let step = (g.cell_size * 0.5).max(0.05);
    let n = (len / step).ceil() as usize;
    let mut last = from;
    for i in 1..=n {
        let t = (i as f32 * step / len).min(1.0);
        let p = [
            from[0] + dx * t,
            from[1] + dy * t,
            from[2] + (to[2] - from[2]) * t,
        ];
        if !on(p) {
            return Some(last);
        }
        last = p;
    }
    Some(to)
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

/// How far from a door its shut leaf is looked for across the pathing grid, in metres (the
/// widest doors are under 6 m).
const LEAF_REACH: f32 = 6.0;

/// The pathing-grid cells of a door's doorway: the walkable cells within
/// [`DOORWAY_RADIUS`] of the door, and those its shut leaf lies across (`leaf`: the door
/// model's collision at rest). A door stands at its hinge, so a wide one reaches further
/// from it than the radius: the Dalish ruins' mirror room door (`bed110ip_mirror_door`, a
/// 2.7 m leaf) left the far half of its doorway open, and a walker went through the shut
/// door without using it (using it is what wakes the bereskarn). A cell is under the leaf
/// when one of its edges crosses it, 0.5 or 1.5 m above the floor.
pub(crate) fn doorway_cells(
    g: &NavGrid,
    door: [f32; 3],
    leaf: Option<&crate::collision::Collision>,
) -> Vec<usize> {
    let Some((cx, cy)) = g.cell(door) else {
        return Vec::new();
    };
    let reach = if leaf.is_some() {
        LEAF_REACH
    } else {
        DOORWAY_RADIUS
    };
    let r = (reach / g.cell_size).ceil() as i32;
    let h = g.cell_size / 2.0;
    let mut cells = Vec::new();
    for dy in -r..=r {
        for dx in -r..=r {
            let (x, y) = (cx as i32 + dx, cy as i32 + dy);
            if x < 0 || y < 0 || x as u32 >= g.columns || y as u32 >= g.rows {
                continue;
            }
            let c = (x as u32, y as u32);
            if !walkable(g, c) {
                continue;
            }
            let m = g.center(c);
            let near = crate::engine::dist(m, [door[0], door[1], m[2]]) <= DOORWAY_RADIUS;
            let under = || {
                leaf.is_some_and(|l| {
                    let z = height(g, c);
                    let corners = [
                        [m[0] - h, m[1] - h],
                        [m[0] + h, m[1] - h],
                        [m[0] + h, m[1] + h],
                        [m[0] - h, m[1] + h],
                    ];
                    [0.5, 1.5].iter().any(|up| {
                        (0..4).any(|i| {
                            let (a, b) = (corners[i], corners[(i + 1) % 4]);
                            l.blocked([a[0], a[1], z + up], [b[0], b[1], z + up])
                        })
                    })
                })
            };
            if near || under() {
                cells.push((c.1 * g.columns + c.0) as usize);
            }
        }
    }
    cells
}

/// Reads a layout's collision from the install at `root` (its level archives, else the
/// `global` resources): what [`Sim::collision`] loads, for presentation to read a copy of
/// its own (on a thread) when the simulation has not loaded it.
pub fn read_layout_collision(
    root: &std::path::Path,
    global: &thedas_vfs::Vfs,
    layout: &str,
) -> Option<crate::collision::Collision> {
    let install = thedas_vfs::DaoInstall::open(root).ok()?;
    let mut level = thedas_vfs::Vfs::new();
    install.mount_level(&mut level, layout).ok()?;
    let (data, _) = level
        .read(&format!("{layout}.arl"))
        .or_else(|_| global.read(&format!("{layout}.arl")))
        .ok()?;
    let arl = thedas_formats::Arl::parse(&data).ok()?;
    Some(crate::collision::load_layout(&level, global, &arl))
}

/// An area's closed doors for sight lines (see [`Sim::door_sight`]).
#[derive(Clone, Default)]
pub struct DoorSight {
    /// Doors with collision: their shapes block a ray between eyes.
    pub solid: Vec<Arc<crate::collision::Collision>>,
    /// Doors without: their positions, tested with [`Sim::door_between`].
    pub points: Vec<[f32; 3]>,
}

impl DoorSight {
    /// Whether a closed door stands between a creature at `a` and one at `b` (feet).
    pub fn blocks(&self, a: [f32; 3], b: [f32; 3]) -> bool {
        self.solid.iter().any(|c| c.sight_blocked(a, b)) || Sim::door_between(&self.points, a, b)
    }
}

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

    /// Whether a point lies in the doorway of a closed door of the area.
    pub(crate) fn in_closed_doorway(
        &mut self,
        area: thedas_script::vm::ObjectId,
        p: [f32; 3],
    ) -> bool {
        let Some(g) = self.nav_grid(area) else {
            return false;
        };
        let Some(i) = g.cell(p).map(|c| (c.1 * g.columns + c.0) as usize) else {
            return false;
        };
        self.closed_doors(area).iter().any(|d| d.1.contains(&i))
    }

    /// Whether a placeable is a door: its appearance's state controller (`placeable_types`)
    /// is `StateCnt_Door`.
    pub fn is_door(&self, id: thedas_script::vm::ObjectId) -> bool {
        let Some(o) = self.world.get(id) else {
            return false;
        };
        if o.kind != crate::world::ObjectKind::Placeable {
            return false;
        }
        let tables = self.world.tables();
        let Some(types) = tables.get(0, "placeable_types") else {
            return false;
        };
        matches!(
            types.cell_by_hash(COL_STATE_CONTROLLER, o.appearance as i64),
            Some(thedas_formats::gda::Cell::String(s))
                if crate::engine::clean(s).eq_ignore_ascii_case("StateCnt_Door")
        )
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
            // The door's own leaf, shut (its model's collision at rest), where there is
            // game data to read it from.
            let leaf = types
                .as_ref()
                .and_then(|t| match t.cell("ModelName", o.appearance as i64)? {
                    thedas_formats::gda::Cell::String(m) => {
                        Some(crate::engine::clean(m).to_string())
                    }
                    _ => None,
                })
                .filter(|m| !m.is_empty())
                .map(|m| crate::collision::load_placeables(&self.vfs, &[(m, o.position, o.facing)]))
                .filter(|c| !c.is_empty());
            out.push((o.id, doorway_cells(&g, o.position, leaf.as_ref())));
        }
        out
    }

    /// Whether one point of an area can see another: no closed door (see
    /// [`Sim::door_sight`]) and no wall of the pathing grid on the line between them (see
    /// [`wall_between`]). Provisional.
    pub fn line_of_sight(
        &mut self,
        area: thedas_script::vm::ObjectId,
        a: [f32; 3],
        b: [f32; 3],
    ) -> bool {
        if self.door_sight(area).blocks(a, b) {
            return false;
        }
        !self.nav_grid(area).is_some_and(|g| wall_between(&g, a, b))
    }

    /// The closed doors of an area as sight blockers. A closed door blocks with its own
    /// collision (its model's `.phy` shapes in the sight mask, at rest), as DAOrigins.exe's
    /// sight ray hits doors in the `Placeables` group (see [`crate::collision`]), so a
    /// creature standing against a closed door does not see through it. A door whose model
    /// has no such shapes (or with no game data to read them from) falls back to
    /// [`Sim::door_between`]. Provisional: whether the exe moves a door's shapes as it opens
    /// is not traced; open doors are left out.
    pub fn door_sight(&mut self, area: thedas_script::vm::ObjectId) -> DoorSight {
        use std::hash::{Hash, Hasher};
        let closed: Vec<thedas_script::vm::ObjectId> = self
            .closed_doors(area)
            .into_iter()
            .map(|(d, _)| d)
            .collect();
        let tables = self.world.tables();
        let types = tables.get(0, "placeable_types");
        let mut out = DoorSight::default();
        for d in closed {
            let Some(o) = self.world.get(d) else {
                continue;
            };
            let (position, facing) = (o.position, o.facing);
            let model = types
                .as_ref()
                .and_then(|t| match t.cell("ModelName", o.appearance as i64)? {
                    thedas_formats::gda::Cell::String(m) => {
                        Some(crate::engine::clean(m).to_string())
                    }
                    _ => None,
                })
                .filter(|m| !m.is_empty());
            let mut h = std::collections::hash_map::DefaultHasher::new();
            (&model, position.map(f32::to_bits), facing.to_bits()).hash(&mut h);
            let key = h.finish();
            let solid = match self.door_collisions.get(&d) {
                Some((k, c)) if *k == key => c.clone(),
                _ => {
                    let c = model
                        .as_ref()
                        .map(|m| {
                            crate::collision::load_placeables(
                                &self.vfs,
                                &[(m.clone(), position, facing)],
                            )
                        })
                        .filter(|c| !c.is_empty())
                        .map(Arc::new);
                    let t = self.world.time_ms;
                    self.world.trace.push(crate::trace::TraceEntry::Log {
                        t,
                        channel: -2,
                        text: format!(
                            "door sight: {d} model {} {} shapes",
                            model.as_deref().unwrap_or("-"),
                            c.as_ref().map_or(0, |c| c.len())
                        ),
                    });
                    self.door_collisions.insert(d, (key, c.clone()));
                    c
                }
            };
            match solid {
                Some(c) => out.solid.push(c),
                None => out.points.push(position),
            }
        }
        out
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

    /// The pathing grid of an area if the simulation has already loaded it (read-only:
    /// for presentation, which must not load it through [`Sim::nav_grid`]).
    pub fn loaded_nav_grid(&self, area: thedas_script::vm::ObjectId) -> Option<Arc<NavGrid>> {
        let name = self.area_layouts.get(&area)?;
        self.nav_grids.get(name)?.clone()
    }

    /// An area's layout collision if the simulation has already loaded it (read-only: for
    /// presentation, which must not load it through [`Sim::collision`]).
    pub fn loaded_collision(
        &self,
        area: thedas_script::vm::ObjectId,
    ) -> Option<Arc<crate::collision::Collision>> {
        let name = self.area_layouts.get(&area)?;
        self.collisions.get(name)?.clone()
    }

    /// The area's placeables that muffle sound behind them, for a collision of their own
    /// ([`crate::collision::load_placeables`]): each one's model, position, facing and how
    /// much it occludes. DAOrigins.exe's sound occlusion gives a placeable its type's
    /// `placeable_types.gda` column 0x913e3094 (read at 0x752923 into the object's +0x674,
    /// used at 0xa2c913): doors 0.4 to 0.7, a bricked doorway 0.8, tents 0.1; most types
    /// have 0 and are left out. Read-only.
    ///
    /// Provisional: doors count only while closed (as for sight, see [`Sim::door_sight`]);
    /// whether the exe drops an open door's shapes is not traced.
    pub fn sound_occluders(
        &self,
        area: thedas_script::vm::ObjectId,
    ) -> Vec<(String, [f32; 3], f32, f32)> {
        const COL_OCCLUSION: u32 = 0x913e_3094;
        let tables = self.world.tables();
        let Some(types) = tables.get(0, "placeable_types") else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for o in self.world.objects.values() {
            if o.kind != crate::world::ObjectKind::Placeable || o.area != Some(area) || !o.active {
                continue;
            }
            let row = o.appearance as i64;
            let value = match types.cell_by_hash(COL_OCCLUSION, row) {
                Some(thedas_formats::gda::Cell::Float(f)) => *f as f32,
                Some(thedas_formats::gda::Cell::Int(i)) => *i as f32,
                _ => 0.0,
            };
            if value <= 0.0 {
                continue;
            }
            let door = matches!(
                types.cell_by_hash(COL_STATE_CONTROLLER, row),
                Some(thedas_formats::gda::Cell::String(s))
                    if crate::engine::clean(s).eq_ignore_ascii_case("StateCnt_Door")
            );
            if door {
                let state = o
                    .fields
                    .get("State")
                    .or_else(|| o.fields.get("AnimContState"))
                    .copied()
                    .unwrap_or(0);
                if state != 0 && state != 1 {
                    continue;
                }
            }
            let Some(thedas_formats::gda::Cell::String(m)) = types.cell("ModelName", row) else {
                continue;
            };
            let m = crate::engine::clean(m);
            if m.is_empty() {
                continue;
            }
            out.push((m.to_string(), o.position, o.facing, value));
        }
        // A fixed order, whatever the object map's.
        out.sort_by(|a, b| (a.1.map(f32::to_bits), &a.0).cmp(&(b.1.map(f32::to_bits), &b.0)));
        out
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

    /// An area's placeables' collision for sight lines (doors excluded: closed doors keep
    /// their own test), rebuilt when the area's active placeables change.
    pub fn placeable_collision(
        &mut self,
        area: thedas_script::vm::ObjectId,
    ) -> Option<Arc<crate::collision::Collision>> {
        use std::hash::{Hash, Hasher};
        let tables = self.world.tables();
        let types = tables.get(0, "placeable_types")?;
        let mut placed: Vec<(thedas_script::vm::ObjectId, String, [f32; 3], f32)> = Vec::new();
        for o in self.world.objects.values() {
            if o.kind != crate::world::ObjectKind::Placeable || o.area != Some(area) || !o.active {
                continue;
            }
            let door = matches!(
                types.cell_by_hash(COL_STATE_CONTROLLER, o.appearance as i64),
                Some(thedas_formats::gda::Cell::String(s))
                    if crate::engine::clean(s).eq_ignore_ascii_case("StateCnt_Door")
            );
            if door {
                continue;
            }
            let Some(thedas_formats::gda::Cell::String(m)) =
                types.cell("ModelName", o.appearance as i64)
            else {
                continue;
            };
            let m = crate::engine::clean(m);
            if m.is_empty() {
                continue;
            }
            placed.push((o.id, m.to_string(), o.position, o.facing));
        }
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for (id, m, p, f) in &placed {
            (id, m, p.map(f32::to_bits), f.to_bits()).hash(&mut h);
        }
        let key = h.finish();
        if let Some((k, c)) = self.placeable_collisions.get(&area) {
            if *k == key {
                return Some(c.clone());
            }
        }
        let list: Vec<(String, [f32; 3], f32)> =
            placed.into_iter().map(|(_, m, p, f)| (m, p, f)).collect();
        let c = Arc::new(crate::collision::load_placeables(&self.vfs, &list));
        let t = self.world.time_ms;
        self.world.trace.push(crate::trace::TraceEntry::Log {
            t,
            channel: -2,
            text: format!(
                "sight collision: {} placeables, {} shapes",
                list.len(),
                c.len()
            ),
        });
        self.placeable_collisions.insert(area, (key, c.clone()));
        Some(c)
    }

    fn load_collision(&self, layout: &str) -> Option<crate::collision::Collision> {
        read_layout_collision(self.install_root.as_ref()?, &self.vfs, layout)
    }

    fn load_nav_grid(&self, layout: &str) -> Option<NavGrid> {
        let file = format!("{layout}.arl");
        let data = match self.vfs.read(&file) {
            Ok((d, _)) => d,
            Err(_) => {
                let root = self.install_root.as_ref()?;
                let install = thedas_vfs::DaoInstall::open(root).ok()?;
                // The layout's name may differ in case from the file (see `thedas_vfs::path`).
                let path = install
                    .env_dirs()
                    .ok()?
                    .into_iter()
                    .find_map(|d| thedas_vfs::resolve_ci(&d, &file).filter(|p| p.is_file()))?;
                std::fs::read(path).ok()?
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
            surfaces: Vec::new(),
        }
    }

    #[test]
    fn a_closed_door_blocks_sight_at_any_distance() {
        // A door leaf across x = 2.5, from y 1.5 to 3.5, 3 m tall.
        let leaf = crate::collision::Collision::from_triangles(&[
            [[2.5, 1.5, 0.0], [2.5, 3.5, 0.0], [2.5, 3.5, 3.0]],
            [[2.5, 1.5, 0.0], [2.5, 3.5, 3.0], [2.5, 1.5, 3.0]],
        ]);
        let doors = DoorSight {
            solid: vec![Arc::new(leaf)],
            points: Vec::new(),
        };
        // Tower of Ishal floor 3: one creature 0.47 m from the door, the other well beyond.
        let (near, far) = ([2.03, 2.5, 0.0], [9.0, 2.5, 0.0]);
        assert!(doors.blocks(near, far));
        assert!(doors.blocks(far, near));
        // The doorway stand-in leaves out the ends of the line, so it misses this one.
        assert!(!Sim::door_between(&[[2.5, 2.5, 0.0]], near, far));
        // Two creatures on the same side see each other, however close to the door.
        assert!(!doors.blocks([0.5, 2.5, 0.0], [2.3, 2.0, 0.0]));
        // Past the leaf's end, the line is clear.
        assert!(!doors.blocks([0.5, 5.0, 0.0], [9.0, 5.0, 0.0]));
    }

    #[test]
    fn loads_and_restores_drop_caches_keyed_by_object_id() {
        use thedas_script::EngineTable;
        use thedas_vfs::Vfs;
        let mut s = Sim::new(Arc::new(Vfs::new()), Arc::new(EngineTable::default()), 1);
        let fill = |s: &mut Sim| {
            s.area_layouts.insert(7, "lvl".into());
            s.door_cells.insert(7, Arc::new(Vec::new()));
            s.placeable_collisions.insert(
                7,
                (
                    1,
                    Arc::new(crate::collision::Collision::from_triangles(&[])),
                ),
            );
            s.door_collisions.insert(8, (1, None));
            s.blocked_since.insert(9, (0, false));
        };
        let empty = |s: &Sim| {
            s.area_layouts.is_empty()
                && s.door_cells.is_empty()
                && s.placeable_collisions.is_empty()
                && s.door_collisions.is_empty()
                && s.blocked_since.is_empty()
        };
        fill(&mut s);
        let snap = s.snapshot();
        s.restore(snap);
        assert!(empty(&s), "after a restore");
        fill(&mut s);
        let save = s.save_game();
        s.load_game(save).unwrap();
        assert!(empty(&s), "after a load");
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
            surfaces: Vec::new(),
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
    fn a_walker_gets_round_a_creature_standing_on_its_route() {
        // The City Elf's alienage, the leader told to talk to Valora: the route through the
        // pathing grid, a point every half metre, passes through a villager standing still,
        // and some of its points lie inside her body. Making for the first of them, the
        // leader pressed on her for good (at (70.5, 64.8), against bec100cr_ambient_f_1).
        use crate::world::{Command, Location, ObjectKind};
        use thedas_script::EngineTable;
        use thedas_vfs::Vfs;
        let mut s = Sim::new(Arc::new(Vfs::new()), Arc::new(EngineTable::default()), 1);
        let area = s.world.spawn(ObjectKind::Area, "a");
        let (w, h) = (24u32, 24u32);
        s.area_layouts.insert(area, "open".into());
        s.nav_grids.insert(
            "open".into(),
            Some(Arc::new(NavGrid {
                columns: w,
                rows: h,
                cell_size: 0.5,
                clearance: 2.0,
                base: [0.0, 0.0, 0.0],
                normal: [0.0, 0.0, 1.0],
                data: vec![1u8; (w * h) as usize],
                heights: vec![0.0; (w * h) as usize],
                surfaces: Vec::new(),
            })),
        );
        let leader = s.world.spawn(ObjectKind::Creature, "leader");
        let villager = s.world.spawn(ObjectKind::Creature, "villager");
        let stands = [2.28, 6.15, 0.0];
        for (id, p) in [(leader, [2.25, 10.25, 0.0]), (villager, stands)] {
            let o = s.world.get_mut(id).unwrap();
            o.area = Some(area);
            o.position = p;
        }
        let to = [2.25, 2.25, 0.0];
        s.world.get_mut(leader).unwrap().commands.push(Command {
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
            arrived: false,
            path: Vec::new(),
        });
        let r = 2.0 * s.capsule_radius(leader);
        for _ in 0..400 {
            s.tick(33);
            let p = s.world.get(leader).unwrap().position;
            let d = crate::engine::dist(p, stands);
            assert!(d >= r - 0.02, "{d} < {r} at {p:?}");
        }
        let o = s.world.get(leader).unwrap();
        assert!(
            crate::engine::dist(o.position, to) < 0.05,
            "{:?}",
            o.position
        );
        assert!(o.commands.is_empty());
        assert_eq!(s.world.get(villager).unwrap().position, stands);
    }

    #[test]
    fn a_walker_does_not_go_back_to_a_waypoint_a_body_slid_it_past() {
        // The Tower of Ishal's third floor: the hero, sent at an archer 35 m to the north,
        // stood touching Alistair, who waited dead ahead of him. Pressed straight into him
        // the hero went round on the right, but the first point of his route lay where he
        // had started, so he walked back to it and into Alistair again, for as long as
        // Alistair stood there (hero (-14.730, 6.271), Alistair (-14.730, 6.691)).
        use crate::world::{Command, ObjectKind};
        use thedas_script::EngineTable;
        use thedas_vfs::Vfs;
        let mut s = Sim::new(Arc::new(Vfs::new()), Arc::new(EngineTable::default()), 1);
        let area = s.world.spawn(ObjectKind::Area, "a");
        let (w, h) = (24u32, 48u32);
        s.area_layouts.insert(area, "open".into());
        s.nav_grids.insert(
            "open".into(),
            Some(Arc::new(NavGrid {
                columns: w,
                rows: h,
                cell_size: 0.5,
                clearance: 2.0,
                base: [0.0, 0.0, 0.0],
                normal: [0.0, 0.0, 1.0],
                data: vec![1u8; (w * h) as usize],
                heights: vec![0.0; (w * h) as usize],
                surfaces: Vec::new(),
            })),
        );
        // Human bodies (personal space 0.35): capsules of 0.21, touching at 0.42.
        s.world.tables().insert(
            "apr_base",
            &["PERSPACE"],
            vec![(15, vec![thedas_formats::gda::Cell::Float(0.35)])],
        );
        let hero = s.world.spawn(ObjectKind::Creature, "hero");
        let waits = s.world.spawn(ObjectKind::Creature, "alistair");
        let stands = [6.25, 2.670_97, 0.0];
        for (id, p) in [(hero, [6.250_015, 1.75, 0.0]), (waits, stands)] {
            let o = s.world.get_mut(id).unwrap();
            o.area = Some(area);
            o.position = p;
            o.appearance = 15;
        }
        // An archer far off to the north, attacked: the hero walks up to it.
        let archer = s.world.spawn(ObjectKind::Creature, "archer");
        {
            let o = s.world.get_mut(archer).unwrap();
            o.area = Some(area);
            o.position = [6.25, 22.25, 0.0];
            o.appearance = 15;
        }
        s.world.get_mut(hero).unwrap().commands.push(Command {
            ty: 0,
            name: "CommandAttack".into(),
            target: Some(archer),
            location: None,
            seconds: 0.0,
            ints: Vec::new(),
            strings: Vec::new(),
            due: None,
            from: None,
            approach: None,
            ring_wait: false,
            after_impact_ms: None,
            impacted: false,
            arrived: false,
            path: Vec::new(),
        });
        let r = 2.0 * s.capsule_radius(hero);
        for _ in 0..150 {
            s.tick(33);
            let p = s.world.get(hero).unwrap().position;
            let d = crate::engine::dist(p, stands);
            assert!(d >= r - 0.02, "{d} < {r} at {p:?}");
        }
        let o = s.world.get(hero).unwrap();
        assert!(o.position[1] > 15.0, "{:?} {:?}", o.position, o.commands);
        assert_eq!(s.world.get(waits).unwrap().position, stands);
    }

    #[test]
    fn a_players_move_with_no_route_stops_at_the_edge_of_the_walkable_area() {
        // Lothering's highway: the stick held forward aimed the hero at points beyond the
        // walkable area, which have no route, and the straight-line fallback ran him 290 m
        // off the map.
        use crate::world::ObjectKind;
        use thedas_script::EngineTable;
        use thedas_vfs::Vfs;
        let mut s = Sim::new(Arc::new(Vfs::new()), Arc::new(EngineTable::default()), 1);
        let area = s.world.spawn(ObjectKind::Area, "a");
        // Walkable for x under 5 m; beyond it the void.
        let (w, h) = (40u32, 6u32);
        let mut data = vec![10u8; (w * h) as usize];
        for y in 0..h {
            for x in 0..10 {
                data[(y * w + x) as usize] = 1;
            }
        }
        s.area_layouts.insert(area, "edge".into());
        s.nav_grids.insert(
            "edge".into(),
            Some(Arc::new(NavGrid {
                columns: w,
                rows: h,
                cell_size: 0.5,
                clearance: 2.0,
                base: [0.0, 0.0, 0.0],
                normal: [0.0, 0.0, 1.0],
                data,
                heights: vec![0.0; (w * h) as usize],
                surfaces: Vec::new(),
            })),
        );
        let hero = s.world.spawn(ObjectKind::Creature, "player");
        {
            let o = s.world.get_mut(hero).unwrap();
            o.area = Some(area);
            o.position = [1.25, 1.25, 0.0];
        }
        // Just beyond the edge (a stick's aim point), far beyond it, and off the grid.
        for to in [[7.0, 1.25, 0.0], [18.0, 1.25, 0.0], [60.0, 1.25, 0.0]] {
            s.world.get_mut(hero).unwrap().position = [1.25, 1.25, 0.0];
            s.order_move(hero, to);
            for _ in 0..300 {
                s.tick(33);
            }
            let o = s.world.get(hero).unwrap();
            assert!(
                o.position[0] > 4.0 && o.position[0] < 5.0,
                "stops at the edge: {:?}",
                o.position
            );
            assert!(o.commands.is_empty());
        }
        // A place it can walk to is not changed.
        s.order_move(hero, [2.25, 2.25, 0.0]);
        for _ in 0..100 {
            s.tick(33);
        }
        let p = s.world.get(hero).unwrap().position;
        assert!(crate::engine::dist(p, [2.25, 2.25, 0.0]) < 0.05, "{p:?}");
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
            arrived: false,
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
            surfaces: Vec::new(),
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
    fn a_wide_doors_doorway_is_as_wide_as_its_leaf() {
        // A wall down column 10 (x 5.0 to 5.5) with a gap 3 m wide, rows 4 to 9; the door
        // stands at its hinge at the gap's top and its leaf, shut, reaches 2.7 m down it.
        let (w, h) = (20u32, 14u32);
        let mut data = vec![1u8; (w * h) as usize];
        for y in (0..h).filter(|y| !(4..10).contains(y)) {
            data[(y * w + 10) as usize] = 10;
        }
        let g = NavGrid {
            columns: w,
            rows: h,
            cell_size: 0.5,
            clearance: 2.0,
            base: [0.0, 0.0, 0.0],
            normal: [0.0, 0.0, 1.0],
            data,
            heights: vec![0.0; (w * h) as usize],
            surfaces: Vec::new(),
        };
        let door = [5.25, 4.75, 0.0];
        let gap: Vec<usize> = (4..10).map(|y| (y * w + 10) as usize).collect();
        // By the radius alone the far half of the gap is left open (rows 4 to 6).
        let by_radius = doorway_cells(&g, door, None);
        assert!(gap[3..].iter().all(|c| by_radius.contains(c)));
        assert!(gap[..3].iter().all(|c| !by_radius.contains(c)));
        // With the leaf (a wall 2.7 m wide and 3.9 m tall from the hinge) all of it shuts.
        let (top, bottom) = (4.75, 4.75 - 2.7);
        let leaf = crate::collision::Collision::from_triangles(&[
            [[5.25, bottom, 0.0], [5.25, top, 0.0], [5.25, top, 3.9]],
            [[5.25, bottom, 0.0], [5.25, top, 3.9], [5.25, bottom, 3.9]],
        ]);
        let with_leaf = doorway_cells(&g, door, Some(&leaf));
        assert!(gap.iter().all(|c| with_leaf.contains(c)), "{with_leaf:?}");
        // Nothing beyond the wall's line is taken that the radius did not take.
        assert!(
            with_leaf
                .iter()
                .all(|c| by_radius.contains(c) || (c % w as usize) == 10),
            "{with_leaf:?}"
        );
    }

    /// The grid of [`grid`] with a closed door in the wall's gap, and a walker.
    #[cfg(test)]
    fn door_in_the_gap(
        walker_at: [f32; 3],
    ) -> (
        Sim,
        thedas_script::vm::ObjectId,
        thedas_script::vm::ObjectId,
        Arc<NavGrid>,
    ) {
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
        let area = s.world.spawn(ObjectKind::Area, "a");
        s.area_layouts.insert(area, "walled".into());
        let g = Arc::new(grid());
        s.nav_grids.insert("walled".into(), Some(g.clone()));
        let door = s.world.spawn(ObjectKind::Placeable, "door");
        {
            let o = s.world.get_mut(door).unwrap();
            o.area = Some(area);
            o.appearance = 93;
            o.position = [2.5, 2.5, 0.0];
        }
        let walker = s.world.spawn(ObjectKind::Creature, "walker");
        {
            let o = s.world.get_mut(walker).unwrap();
            o.area = Some(area);
            o.position = walker_at;
        }
        (s, walker, area, g)
    }

    #[test]
    fn a_move_aimed_into_a_closed_doorway_ends_at_the_door_not_in_it() {
        // A held stick aims three metres ahead, often into a doorway or onto the door
        // itself: the walk's route stops at the door, and the walker must stay there. It was
        // set down at the aimed point when the command ended: inside a locked door's
        // doorway (Highever's hall door, where nothing then got the hero out) or past it.
        for aim in [[2.5, 2.5, 0.0], [1.5, 2.5, 0.0], [3.4, 2.6, 0.0]] {
            let (mut s, walker, area, g) = door_in_the_gap([0.5, 0.5, 0.0]);
            let doorway = s.closed_doors(area)[0].1.clone();
            s.order_move(walker, aim);
            for _ in 0..200 {
                s.tick(33);
            }
            let o = s.world.get(walker).unwrap();
            let i = g
                .cell(o.position)
                .map(|c| (c.1 * g.columns + c.0) as usize)
                .unwrap();
            assert!(
                !doorway.contains(&i),
                "aimed at {aim:?}, set down in the doorway at {:?}",
                o.position
            );
            assert!(
                o.position[0] < 2.0,
                "still on its own side: {:?}",
                o.position
            );
        }
    }

    #[test]
    fn a_walker_standing_in_a_closed_doorway_can_walk_out() {
        // Set down in the middle of the doorway (by a save made there, a jump, the door
        // shutting on it), every neighbouring cell is the door's or the wall's: no route
        // could start, and every move failed against the door.
        let (mut s, walker, area, _) = door_in_the_gap([2.5, 2.5, 0.0]);
        assert_eq!(s.closed_doors(area).len(), 1);
        let to = [0.5, 0.5, 0.0];
        assert!(matches!(s.route_to(walker, to), Routed::Path(_)));
        s.order_move(walker, to);
        for _ in 0..200 {
            s.tick(33);
        }
        let p = s.world.get(walker).unwrap().position;
        assert!(crate::engine::dist(p, to) < 0.05, "{p:?}");
        // Once out, the door is in the way again.
        assert!(matches!(
            s.route_to(walker, [4.5, 0.5, 0.0]),
            Routed::Door(_)
        ));
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
