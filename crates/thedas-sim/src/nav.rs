//! Routing on the layout's pathing grid (`.arl`, see `thedas_formats::level::NavGrid`).
//!
//! Provisional, inferred from the data: a cell is walkable when bit 0 of its data byte is
//! set, its height is the stored value in sixteenths of a metre, and a creature can step
//! between neighbouring cells whose heights differ by at most [`MAX_STEP`]. Routes are
//! 8-connected A* without cutting blocked corners.

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
    g.value(c) & 1 != 0
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

/// The route from `from` to `to` as world points (cell centres at floor height, ending at
/// `to`), or `None` when either end is off the grid or no route exists.
pub fn route(g: &NavGrid, from: [f32; 3], to: [f32; 3]) -> Option<Vec<[f32; 3]>> {
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
            if !walkable(g, n) || (height(g, n) - hc).abs() > MAX_STEP {
                continue;
            }
            // No cutting corners past blocked cells.
            if dx != 0 && dy != 0 {
                let a = ((c.0 as i32 + dx) as u32, c.1);
                let b = (c.0, (c.1 as i32 + dy) as u32);
                if !walkable(g, a) || !walkable(g, b) {
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

impl Sim {
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
    fn steps_too_high_block() {
        let mut g = grid();
        // Raise the gap cell by a metre (16 units per metre).
        g.heights[(2 * 5 + 2) as usize] = 16.0;
        assert!(route(&g, [0.5, 0.5, 0.0], [4.5, 0.5, 0.0]).is_none());
    }
}
