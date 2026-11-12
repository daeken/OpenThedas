//! The level's solid surfaces for casting rays and sweeping spheres on the CPU: what the
//! game camera keeps on the near side of, so a wall between it and the party leader does not
//! hide the view.
//!
//! DAOrigins.exe's camera sweeps a 0.25 m sphere (a PhysX linear capsule sweep, 0x6490b0)
//! against its physics shapes (placeables and static geometry, group mask 0x1024).
//! Provisional: these are the render triangles instead; every triangle of the level's opaque
//! draws counts as solid, draws whose material is cut out or blended (foliage, glass, fx: a
//! `Punch`, `Alpha` or `Additive` semantic or render class) do not, and both faces count.

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
        Self::from_draws_at(scene, draws, solid_only, |_| None)
    }

    /// [`Self::from_draws`] with each draw where `world` puts it (a door swung open), or
    /// where the scene placed it when `world` gives none.
    pub fn from_draws_at(
        scene: &AreaScene,
        draws: impl IntoIterator<Item = usize>,
        solid_only: bool,
        world: impl Fn(usize) -> Option<Mat4>,
    ) -> Self {
        let mut triangles = Vec::new();
        for (i, d) in draws
            .into_iter()
            .filter_map(|i| scene.draws.get(i).map(|d| (i, d)))
        {
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
                &world(i).unwrap_or(d.transform),
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
        self.nearest(from, to, 0.0, |tri| segment_triangle(from, to - from, tri))
    }

    /// The share (0..1) of the way from `from` to `to` at which a sphere of `radius` metres
    /// whose centre moves along the segment first touches a surface (its face, an edge or a
    /// corner), from either side; 0 when it touches one where it starts. `None` when it
    /// touches none. A radius of 0 (or less) is [`Collider::first_hit`].
    pub fn sweep(&self, from: Vec3, to: Vec3, radius: f32) -> Option<f32> {
        if radius.is_nan() || radius == f32::INFINITY {
            return None;
        }
        if radius <= 0.0 {
            return self.first_hit(from, to);
        }
        self.nearest(from, to, radius, |tri| {
            sphere_triangle(from, to - from, radius, tri)
        })
    }

    /// The least `hit` of the triangles near the segment from `from` to `to` (within
    /// `reach` metres of it on the ground plane).
    fn nearest(
        &self,
        from: Vec3,
        to: Vec3,
        reach: f32,
        hit: impl Fn(&[Vec3; 3]) -> Option<f32>,
    ) -> Option<f32> {
        if self.is_empty() || !from.is_finite() || !to.is_finite() {
            return None;
        }
        let d = to - from;
        let flat = d.truncate().length();
        // Walk the cells the segment crosses, in half-cell steps.
        let steps = ((flat / (self.cell * 0.5)).ceil() as usize).clamp(1, 4 * MAX_CELLS);
        // A step's neighbours too: the segment may graze a corner between steps, and a
        // sphere reaches past its centre's cell.
        let around = 1 + (reach / self.cell).ceil().clamp(0.0, 8.0) as i64;
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
            for dy in -around..=around {
                for dx in -around..=around {
                    let (x, y) = (cell.0 as i64 + dx, cell.1 as i64 + dy);
                    if x < 0 || y < 0 || x >= self.size[0] as i64 || y >= self.size[1] as i64 {
                        continue;
                    }
                    let Some(list) = self.cells.get(y as usize * self.size[0] + x as usize) else {
                        continue;
                    };
                    for &i in list {
                        if !seen.insert(i) {
                            continue;
                        }
                        let Some(tri) = self.triangles.get(i as usize) else {
                            continue;
                        };
                        if let Some(t) = hit(tri) {
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

/// When a sphere of radius `r` whose centre moves along `from + t * d` (0 <= t <= 1) first
/// touches triangle `tri`, from either side: its face, then its edges and corners. 0 when it
/// touches it at the start.
fn sphere_triangle(from: Vec3, d: Vec3, r: f32, tri: &[Vec3; 3]) -> Option<f32> {
    let [a, b, c] = *tri;
    if closest_on_triangle(from, tri).distance_squared(from) <= r * r {
        return Some(0.0);
    }
    let mut best: Option<f32> = None;
    let mut take = |t: f32| {
        if (0.0..=1.0).contains(&t) {
            best = Some(best.map_or(t, |b: f32| b.min(t)));
        }
    };
    // The face: the sphere's surface reaches the plane, at a point inside the triangle.
    let n = (b - a).cross(c - a);
    if n.length_squared() > 1e-18 {
        let face = n.normalize();
        // The side `from` is on.
        let (n, s) = match face.dot(from - a) {
            s if s < 0.0 => (-face, -s),
            s => (face, s),
        };
        let towards = -n.dot(d);
        if towards > 1e-12 {
            let t = (s - r) / towards;
            if (0.0..=1.0).contains(&t) {
                let q = from + d * t - n * r;
                if inside_triangle(q, a, b, c, face) {
                    take(t);
                }
            }
        }
    }
    // The edges: the centre reaches the cylinder of radius `r` around one, between its ends.
    for (p, q) in [(a, b), (b, c), (c, a)] {
        let e = q - p;
        let ee = e.dot(e);
        if ee < 1e-18 {
            continue;
        }
        let m = from - p;
        let (md, nd) = (m.dot(e), d.dot(e));
        let qa = ee * d.dot(d) - nd * nd;
        if qa.abs() < 1e-12 {
            continue;
        }
        let qb = ee * m.dot(d) - nd * md;
        let qc = ee * (m.dot(m) - r * r) - md * md;
        let disc = qb * qb - qa * qc;
        if disc < 0.0 {
            continue;
        }
        let t = (-qb - disc.sqrt()) / qa;
        let along = (md + t * nd) / ee;
        if (0.0..=1.0).contains(&along) {
            take(t);
        }
    }
    // The corners: the centre comes within `r` of one.
    let dd = d.dot(d);
    if dd > 1e-18 {
        for v in [a, b, c] {
            let m = from - v;
            let qb = m.dot(d);
            let qc = m.dot(m) - r * r;
            let disc = qb * qb - dd * qc;
            if disc >= 0.0 {
                take((-qb - disc.sqrt()) / dd);
            }
        }
    }
    best
}

/// Whether `q` (in the plane of triangle `a b c`, whose normal by its winding is `n`) is
/// inside it.
fn inside_triangle(q: Vec3, a: Vec3, b: Vec3, c: Vec3, n: Vec3) -> bool {
    let eps = -1e-6;
    (b - a).cross(q - a).dot(n) >= eps
        && (c - b).cross(q - b).dot(n) >= eps
        && (a - c).cross(q - c).dot(n) >= eps
}

/// The point of triangle `tri` nearest `p` (Ericson, Real-Time Collision Detection 5.1.5).
fn closest_on_triangle(p: Vec3, tri: &[Vec3; 3]) -> Vec3 {
    let [a, b, c] = *tri;
    let (ab, ac, ap) = (b - a, c - a, p - a);
    let (d1, d2) = (ab.dot(ap), ac.dot(ap));
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }
    let bp = p - b;
    let (d3, d4) = (ab.dot(bp), ac.dot(bp));
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        return a + ab * (d1 / (d1 - d3));
    }
    let cp = p - c;
    let (d5, d6) = (ab.dot(cp), ac.dot(cp));
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        return a + ac * (d2 / (d2 - d6));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        return b + (c - b) * ((d4 - d3) / ((d4 - d3) + (d5 - d6)));
    }
    let sum = va + vb + vc;
    if sum.abs() < 1e-30 {
        // Degenerate (a line or a point): its corners are as near as anything.
        return [a, b, c]
            .into_iter()
            .min_by(|x, y| x.distance_squared(p).total_cmp(&y.distance_squared(p)))
            .unwrap_or(a);
    }
    let (v, w) = (vb / sum, vc / sum);
    a + ab * v + ac * w
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
    fn a_sphere_meets_the_wall_face_a_radius_early() {
        let w = wall();
        // Along +X at the wall's middle: the centre stops 0.25 m short of x = 5.
        let t = w
            .sweep(Vec3::new(0.0, 0.0, 1.0), Vec3::new(10.0, 0.0, 1.0), 0.25)
            .unwrap();
        assert!((t - 0.475).abs() < 1e-4, "{t}");
        // From the far side too.
        let t = w
            .sweep(Vec3::new(10.0, 0.0, 1.0), Vec3::new(0.0, 0.0, 1.0), 0.25)
            .unwrap();
        assert!((t - 0.475).abs() < 1e-4, "{t}");
        // A radius of 0 is the ray.
        let t = w
            .sweep(Vec3::new(0.0, 0.0, 1.0), Vec3::new(10.0, 0.0, 1.0), 0.0)
            .unwrap();
        assert!((t - 0.5).abs() < 1e-4, "{t}");
        // Stopping 0.3 m short of the wall: the sphere does not reach it.
        assert!(
            w.sweep(Vec3::new(0.0, 0.0, 1.0), Vec3::new(4.7, 0.0, 1.0), 0.25)
                .is_none()
        );
        // Bad radii touch nothing.
        assert!(
            w.sweep(Vec3::ZERO, Vec3::X * 10.0, f32::NAN).is_none()
                && w.sweep(Vec3::ZERO, Vec3::X * 10.0, f32::INFINITY).is_none()
        );
    }

    #[test]
    fn a_sphere_grazes_edges_and_corners_a_ray_misses() {
        // One triangle in the plane z = 0: corners (0,0), (2,0), (0,2).
        let tri = Collider::new(vec![[
            Vec3::ZERO,
            Vec3::new(2.0, 0.0, 0.0),
            Vec3::new(0.0, 2.0, 0.0),
        ]]);
        // Past its edge along x = 0 (y from 0 to 2), 0.1 m off it, moving along -Z through
        // the plane: a ray misses, a 0.25 m sphere touches the edge.
        let (from, to) = (Vec3::new(-0.1, 1.0, 1.0), Vec3::new(-0.1, 1.0, -1.0));
        assert!(tri.first_hit(from, to).is_none());
        let t = tri.sweep(from, to, 0.25).unwrap();
        // The centre is 0.25 m from the edge when its height is sqrt(0.25² - 0.1²).
        let z = (0.25f32 * 0.25 - 0.01).sqrt();
        assert!((t - (1.0 - z) / 2.0).abs() < 1e-4, "{t}");
        // Past the corner (0,0) diagonally, 0.1 m out, moving along -Z: the corner.
        let off = Vec3::new(-0.1, -0.1, 0.0) / 2f32.sqrt();
        let t = tri.sweep(off + Vec3::Z, off - Vec3::Z, 0.25).unwrap();
        assert!((t - (1.0 - z) / 2.0).abs() < 1e-4, "{t}");
        // Moving along the edge's line, 0.2 m beyond its end: the end corner.
        let t = tri
            .sweep(Vec3::new(0.0, 4.0, 0.0), Vec3::new(0.0, 2.0, 0.0), 0.25)
            .unwrap();
        assert!((t - 0.875).abs() < 1e-4, "{t}");
        // Wide of everything: nothing.
        assert!(
            tri.sweep(Vec3::new(-1.0, 1.0, 1.0), Vec3::new(-1.0, 1.0, -1.0), 0.25)
                .is_none()
        );
    }

    #[test]
    fn a_sphere_already_touching_meets_at_once() {
        let w = wall();
        let t = w
            .sweep(Vec3::new(4.9, 0.0, 1.0), Vec3::new(0.0, 0.0, 1.0), 0.25)
            .unwrap();
        assert_eq!(t, 0.0);
        assert!(
            closest_on_triangle(
                Vec3::new(9.0, 0.0, 1.0),
                &[
                    Vec3::new(5.0, -10.0, 0.0),
                    Vec3::new(5.0, 10.0, 0.0),
                    Vec3::new(5.0, 10.0, 4.0),
                ]
            )
            .distance(Vec3::new(5.0, 0.0, 1.0))
                < 1e-5
        );
        // A degenerate triangle (all one point) is a corner.
        let p = Collider::new(vec![[Vec3::X; 3]]);
        let t = p
            .sweep(Vec3::new(-1.0, 0.0, 0.0), Vec3::new(3.0, 0.0, 0.0), 0.5)
            .unwrap();
        assert!((t - 0.375).abs() < 1e-4, "{t}");
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
