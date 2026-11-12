//! The level's solid surfaces for casting rays on the CPU: what the game camera keeps on
//! the near side of, so a wall between it and the party leader does not hide the view.
//!
//! Provisional: every triangle of the level's opaque draws counts as solid; draws whose
//! material is cut out or blended (foliage, glass, fx: a `Punch`, `Alpha` or `Additive`
//! semantic or render class) do not. The game's own camera collision is not traced.

use glam::{Mat4, Vec3};

use crate::scene::AreaScene;

/// The grid's cells (metres, on the ground plane).
const CELL: f32 = 2.0;
/// No grid is built past this many cells a side; larger levels use coarser cells.
const MAX_CELLS: usize = 1024;

/// Triangles in a grid over the ground plane: each cell lists those whose bounds reach it.
#[derive(Default)]
pub struct Collider {
    triangles: Vec<[Vec3; 3]>,
    min: [f32; 2],
    cell: f32,
    size: [usize; 2],
    cells: Vec<Vec<u32>>,
}

impl Collider {
    /// The opaque surfaces of `scene` (see the module docs).
    pub fn from_scene(scene: &AreaScene) -> Self {
        Self::from_draws(scene, 0..scene.draws.len(), true)
    }

    /// The surfaces of `scene`'s draws `draws` (one object's, to pick it by its shape), only
    /// the opaque ones when `solid_only`.
    pub fn from_draws(
        scene: &AreaScene,
        draws: impl IntoIterator<Item = usize>,
        solid_only: bool,
    ) -> Self {
        let mut triangles = Vec::new();
        for d in draws.into_iter().filter_map(|i| scene.draws.get(i)) {
            if d.skin.is_some() {
                continue;
            }
            let solid = !solid_only
                || scene.materials.get(d.material).is_none_or(|m| {
                    let kind = format!(
                        "{} {}",
                        m.semantic,
                        m.render_class.as_deref().unwrap_or_default()
                    )
                    .to_ascii_lowercase();
                    !["punch", "alpha", "additive"]
                        .iter()
                        .any(|k| kind.contains(k))
                });
            let Some(mesh) = scene.meshes.get(d.mesh).filter(|_| solid) else {
                continue;
            };
            triangles.extend(world_triangles(
                &d.transform,
                mesh.vertices.iter().map(|v| v.position),
                &mesh.indices,
            ));
        }
        Self::new(triangles)
    }

    /// A collider over `triangles` (world space).
    pub fn new(triangles: Vec<[Vec3; 3]>) -> Self {
        let finite = |t: &[Vec3; 3]| t.iter().all(|v| v.is_finite());
        let triangles: Vec<[Vec3; 3]> = triangles.into_iter().filter(finite).collect();
        if triangles.is_empty() {
            return Self::default();
        }
        let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for t in &triangles {
            for v in t {
                lo = lo.min(*v);
                hi = hi.max(*v);
            }
        }
        let span = (hi - lo).max(Vec3::splat(0.01));
        let cell = CELL.max(span.x.max(span.y) / MAX_CELLS as f32);
        let size = [
            ((span.x / cell).ceil() as usize).clamp(1, MAX_CELLS),
            ((span.y / cell).ceil() as usize).clamp(1, MAX_CELLS),
        ];
        let mut c = Self {
            triangles,
            min: [lo.x, lo.y],
            cell,
            size,
            cells: vec![Vec::new(); size[0] * size[1]],
        };
        for (i, t) in c.triangles.iter().enumerate() {
            let tlo = t[0].min(t[1]).min(t[2]);
            let thi = t[0].max(t[1]).max(t[2]);
            let (x0, y0) = c.cell_of(tlo.x, tlo.y);
            let (x1, y1) = c.cell_of(thi.x, thi.y);
            for y in y0..=y1 {
                for x in x0..=x1 {
                    c.cells[y * size[0] + x].push(i as u32);
                }
            }
        }
        c
    }

    pub fn is_empty(&self) -> bool {
        self.triangles.is_empty()
    }

    fn cell_of(&self, x: f32, y: f32) -> (usize, usize) {
        let f = |v: f32, min: f32, n: usize| {
            (((v - min) / self.cell).floor().max(0.0) as usize).min(n.saturating_sub(1))
        };
        (
            f(x, self.min[0], self.size[0]),
            f(y, self.min[1], self.size[1]),
        )
    }

    /// The share (0..1) of the way from `from` to `to` at which the segment first meets a
    /// surface, from either side; `None` when it meets none.
    pub fn first_hit(&self, from: Vec3, to: Vec3) -> Option<f32> {
        if self.is_empty() || !from.is_finite() || !to.is_finite() {
            return None;
        }
        let d = to - from;
        let flat = d.truncate().length();
        // Walk the cells the segment crosses, in half-cell steps.
        let steps = ((flat / (self.cell * 0.5)).ceil() as usize).clamp(1, 4 * MAX_CELLS);
        let mut seen = std::collections::HashSet::new();
        let mut best: Option<f32> = None;
        let mut last = None;
        for s in 0..=steps {
            let p = from + d * (s as f32 / steps as f32);
            let cell = self.cell_of(p.x, p.y);
            if last == Some(cell) {
                continue;
            }
            last = Some(cell);
            // A step's neighbours too: the segment may graze a corner between steps.
            for dy in -1i64..=1 {
                for dx in -1i64..=1 {
                    let (x, y) = (cell.0 as i64 + dx, cell.1 as i64 + dy);
                    if x < 0 || y < 0 || x >= self.size[0] as i64 || y >= self.size[1] as i64 {
                        continue;
                    }
                    for &i in &self.cells[y as usize * self.size[0] + x as usize] {
                        if !seen.insert(i) {
                            continue;
                        }
                        if let Some(t) = segment_triangle(from, d, &self.triangles[i as usize]) {
                            best = Some(best.map_or(t, |b: f32| b.min(t)));
                        }
                    }
                }
            }
        }
        best
    }
}

/// A mesh's triangles placed by `transform`; indices out of range are skipped.
fn world_triangles(
    transform: &Mat4,
    positions: impl Iterator<Item = [f32; 3]>,
    indices: &[u32],
) -> Vec<[Vec3; 3]> {
    let world: Vec<Vec3> = positions
        .map(|p| transform.transform_point3(Vec3::from_array(p)))
        .collect();
    indices
        .chunks_exact(3)
        .filter_map(|t| {
            Some([
                *world.get(t[0] as usize)?,
                *world.get(t[1] as usize)?,
                *world.get(t[2] as usize)?,
            ])
        })
        .collect()
}

/// Where segment `from + t * d` (0 <= t <= 1) meets triangle `tri`, either side
/// (Möller-Trumbore).
fn segment_triangle(from: Vec3, d: Vec3, tri: &[Vec3; 3]) -> Option<f32> {
    let (e1, e2) = (tri[1] - tri[0], tri[2] - tri[0]);
    let p = d.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-9 {
        return None;
    }
    let inv = 1.0 / det;
    let s = from - tri[0];
    let u = s.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = d.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = e2.dot(q) * inv;
    (0.0..=1.0).contains(&t).then_some(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A wall in the plane x = 5, from y -10 to 10 and z 0 to 4 (two triangles).
    fn wall() -> Collider {
        let (a, b, c, d) = (
            Vec3::new(5.0, -10.0, 0.0),
            Vec3::new(5.0, 10.0, 0.0),
            Vec3::new(5.0, 10.0, 4.0),
            Vec3::new(5.0, -10.0, 4.0),
        );
        Collider::new(vec![[a, b, c], [a, c, d]])
    }

    #[test]
    fn segments_meet_the_wall_from_either_side_and_miss_around_it() {
        let w = wall();
        let t = w
            .first_hit(Vec3::new(0.0, 0.0, 1.0), Vec3::new(10.0, 0.0, 1.0))
            .unwrap();
        assert!((t - 0.5).abs() < 1e-4, "{t}");
        let back = w
            .first_hit(Vec3::new(10.0, 0.0, 1.0), Vec3::new(0.0, 0.0, 1.0))
            .unwrap();
        assert!((back - 0.5).abs() < 1e-4);
        // Short of the wall, over it, past its end: no hit.
        assert!(
            w.first_hit(Vec3::new(0.0, 0.0, 1.0), Vec3::new(4.0, 0.0, 1.0))
                .is_none()
        );
        assert!(
            w.first_hit(Vec3::new(0.0, 0.0, 6.0), Vec3::new(10.0, 0.0, 6.0))
                .is_none()
        );
        assert!(
            w.first_hit(Vec3::new(0.0, 20.0, 1.0), Vec3::new(10.0, 20.0, 1.0))
                .is_none()
        );
        // A vertical segment through nothing, and degenerate input.
        assert!(
            w.first_hit(Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 9.0))
                .is_none()
        );
        assert!(
            w.first_hit(Vec3::splat(f32::NAN), Vec3::ZERO).is_none()
                && Collider::default().first_hit(Vec3::ZERO, Vec3::X).is_none()
        );
    }

    #[test]
    fn one_objects_draws_make_its_shape() {
        use crate::scene::{DrawItem, SceneMesh, Vertex};
        let vertex = |x: f32, y: f32, z: f32| Vertex {
            position: [x, y, z],
            normal: [1.0, 0.0, 0.0],
            uv0: [0.0; 2],
            uv1: [0.0; 2],
        };
        // A 2 m square in the plane x = 0.
        let mesh = SceneMesh {
            key: "quad".into(),
            vertices: vec![
                vertex(0.0, -1.0, 0.0),
                vertex(0.0, 1.0, 0.0),
                vertex(0.0, 1.0, 2.0),
                vertex(0.0, -1.0, 2.0),
            ],
            indices: vec![0, 1, 2, 0, 2, 3],
            chunk: thedas_formats::mesh::MeshChunk {
                name: "quad".into(),
                vertex_size: 0,
                vertex_count: 4,
                index_count: 6,
                primitive: 0,
                index_format: thedas_formats::mesh::IndexFormat::U16,
                base_vertex: 0,
                vertex_offset: 0,
                start_index: 0,
                elements: vec![],
                bounds: None,
            },
            raw_vertices: vec![],
            positions: None,
        };
        let draw = |x: f32| DrawItem {
            mesh: 0,
            material: 0,
            transform: Mat4::from_translation(Vec3::new(x, 0.0, 0.0)),
            lightmap: None,
            sphere: (Vec3::new(x, 0.0, 1.0), 1.5),
            model: "quad".into(),
            room: String::new(),
            room_id: None,
            probe: None,
            skin: None,
            cast_shadow: None,
        };
        let scene = AreaScene {
            meshes: vec![mesh],
            draws: vec![draw(2.0), draw(6.0)],
            ..Default::default()
        };
        // The second object alone: a ray along +X meets it at x = 6, not the first at 2.
        let second = Collider::from_draws(&scene, [1], false);
        let t = second
            .first_hit(Vec3::new(0.0, 0.0, 1.0), Vec3::new(10.0, 0.0, 1.0))
            .unwrap();
        assert!((t - 0.6).abs() < 1e-4, "{t}");
        // Past its edge it is not touched, though its bounds would be.
        assert!(
            second
                .first_hit(Vec3::new(0.0, 1.4, 1.0), Vec3::new(10.0, 1.4, 1.0))
                .is_none()
        );
        // Both: the first is in front.
        let both = Collider::from_scene(&scene);
        let t = both
            .first_hit(Vec3::new(0.0, 0.0, 1.0), Vec3::new(10.0, 0.0, 1.0))
            .unwrap();
        assert!((t - 0.2).abs() < 1e-4);
    }

    #[test]
    fn places_mesh_triangles_and_skips_bad_indices() {
        let t = world_triangles(
            &Mat4::from_translation(Vec3::new(1.0, 2.0, 3.0)),
            [[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]].into_iter(),
            &[0, 1, 2, 0, 1, 9],
        );
        assert_eq!(t.len(), 1);
        assert_eq!(t[0][1], Vec3::new(2.0, 2.0, 3.0));
    }
}
