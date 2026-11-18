//! A bounding volume hierarchy over the bake scene's triangles, for the camera, shadow and
//! occlusion rays.
//!
//! The toolset's ray tracer (a YafaRay 0.1 core) tests triangles from both sides, so this does
//! too; see `docs/lightmaps/baker-spec.md` §2.

use glam::Vec3;

mod kd;

/// One world-space triangle and what it belongs to.
#[derive(Clone, Copy, Debug)]
pub struct Tri {
    pub a: Vec3,
    pub b: Vec3,
    pub c: Vec3,
    /// The draw ([`thedas_render::AreaScene::draws`]) it came from.
    pub draw: u32,
    /// Whether it blocks shadow and occlusion rays ([`Bvh::occluded`]).
    pub casts: bool,
}

impl Tri {
    /// The geometric normal by winding, unit length (zero for a degenerate triangle).
    pub fn normal(&self) -> Vec3 {
        (self.b - self.a).cross(self.c - self.a).normalize_or_zero()
    }
}

// yafraycore.dll recNormal (RVA 0x14180) stores the edges, cross product
// and squared length as floats. The products in the cross and dot
// calculations stay in x87 registers until the result is stored.
pub(crate) fn toolset_normal([a, b, c]: [Vec3; 3]) -> Vec3 {
    let n = (b - a).as_dvec3().cross((c - a).as_dvec3()).as_vec3();
    let length_squared = n.as_dvec3().length_squared() as f32;
    if length_squared == 0.0 {
        return n;
    }
    n * (1.0 / length_squared.sqrt())
}

/// A ray hit: distance and triangle index.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    pub t: f32,
    pub tri: u32,
    /// Barycentric weights of `b` and `c`.
    pub u: f32,
    pub v: f32,
}

#[derive(Clone, Copy, Debug)]
struct Node {
    min: Vec3,
    max: Vec3,
    /// Leaf: first triangle index into `order`; inner: index of the left child (right = +1).
    start: u32,
    /// Leaf: triangle count; inner: 0.
    count: u32,
}

/// The hierarchy. Build once, then query from many threads.
pub struct Bvh {
    pub tris: Vec<Tri>,
    order: Vec<u32>,
    nodes: Vec<Node>,
    toolset: Option<kd::Tree>,
}

const LEAF: usize = 4;

impl Bvh {
    pub fn new(tris: Vec<Tri>) -> Bvh {
        let mut order: Vec<u32> = (0..tris.len() as u32).collect();
        let centres: Vec<Vec3> = tris.iter().map(|t| (t.a + t.b + t.c) / 3.0).collect();
        let mut nodes = Vec::with_capacity(tris.len() / 2 + 1);
        nodes.push(Node {
            min: Vec3::ZERO,
            max: Vec3::ZERO,
            start: 0,
            count: 0,
        });
        if !tris.is_empty() {
            build(&tris, &centres, &mut order, &mut nodes, 0, 0, tris.len());
        }
        Bvh {
            tris,
            order,
            nodes,
            toolset: None,
        }
    }

    pub(crate) fn new_toolset(tris: Vec<Tri>) -> Self {
        let toolset = kd::Tree::new(&tris);
        Self {
            tris,
            order: Vec::new(),
            nodes: Vec::new(),
            toolset,
        }
    }

    pub(crate) fn normal(&self, triangle: u32) -> Vec3 {
        let tri = &self.tris[triangle as usize];
        if self.toolset.is_some() {
            toolset_normal([tri.a, tri.b, tri.c])
        } else {
            tri.normal()
        }
    }

    /// The nearest hit with `tmin < t < tmax`.
    pub fn closest(&self, from: Vec3, dir: Vec3, tmin: f32, tmax: f32) -> Option<Hit> {
        let mut best: Option<Hit> = None;
        let mut limit = tmax;
        self.walk(from, dir, tmin, &mut limit, false, &mut |hit, limit| {
            *limit = hit.t;
            best = Some(hit);
            false
        });
        best
    }

    /// The nearest hit with `tmin < t < tmax` among triangles that cast shadows.
    pub fn closest_casting(&self, from: Vec3, dir: Vec3, tmin: f32, tmax: f32) -> Option<Hit> {
        let mut best: Option<Hit> = None;
        let mut limit = tmax;
        self.walk(from, dir, tmin, &mut limit, true, &mut |hit, limit| {
            *limit = hit.t;
            best = Some(hit);
            false
        });
        best
    }

    /// Whether anything is hit with `tmin < t < tmax`.
    pub fn occluded(&self, from: Vec3, dir: Vec3, tmin: f32, tmax: f32) -> bool {
        let mut any = false;
        let mut limit = tmax;
        self.walk(from, dir, tmin, &mut limit, true, &mut |_, _| {
            any = true;
            true
        });
        any
    }

    fn walk(
        &self,
        from: Vec3,
        dir: Vec3,
        tmin: f32,
        limit: &mut f32,
        casters_only: bool,
        on_hit: &mut dyn FnMut(Hit, &mut f32) -> bool,
    ) {
        if let Some(tree) = &self.toolset {
            tree.walk(&self.tris, (from, dir), tmin, limit, casters_only, on_hit);
            return;
        }
        if self.tris.is_empty() {
            return;
        }
        let inv = Vec3::ONE / dir;
        let mut stack = [0u32; 64];
        let mut top = 1;
        while top > 0 {
            top -= 1;
            let node = self.nodes[stack[top] as usize];
            if !slab(node.min, node.max, from, inv, tmin, *limit) {
                continue;
            }
            if node.count > 0 {
                for &i in &self.order[node.start as usize..(node.start + node.count) as usize] {
                    let t = &self.tris[i as usize];
                    if casters_only && !t.casts {
                        continue;
                    }
                    if let Some((d, u, v)) = intersect(t, from, dir)
                        && d > tmin
                        && d < *limit
                        && on_hit(Hit { t: d, tri: i, u, v }, limit)
                    {
                        return;
                    }
                }
            } else if top + 2 <= stack.len() {
                stack[top] = node.start;
                stack[top + 1] = node.start + 1;
                top += 2;
            }
        }
    }
}

fn build(
    tris: &[Tri],
    centres: &[Vec3],
    order: &mut [u32],
    nodes: &mut Vec<Node>,
    at: usize,
    start: usize,
    end: usize,
) {
    let (mut min, mut max) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    let (mut cmin, mut cmax) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for &i in &order[start..end] {
        let t = &tris[i as usize];
        min = min.min(t.a).min(t.b).min(t.c);
        max = max.max(t.a).max(t.b).max(t.c);
        cmin = cmin.min(centres[i as usize]);
        cmax = cmax.max(centres[i as usize]);
    }
    let count = end - start;
    let extent = cmax - cmin;
    let axis = if extent.x >= extent.y && extent.x >= extent.z {
        0
    } else if extent.y >= extent.z {
        1
    } else {
        2
    };
    if count <= LEAF || extent[axis] <= 0.0 {
        nodes[at] = Node {
            min,
            max,
            start: start as u32,
            count: count as u32,
        };
        return;
    }
    let mid = start + count / 2;
    order[start..end].select_nth_unstable_by(count / 2, |&a, &b| {
        centres[a as usize][axis].total_cmp(&centres[b as usize][axis])
    });
    let left = nodes.len();
    nodes.push(nodes[at]);
    nodes.push(nodes[at]);
    nodes[at] = Node {
        min,
        max,
        start: left as u32,
        count: 0,
    };
    build(tris, centres, order, nodes, left, start, mid);
    build(tris, centres, order, nodes, left + 1, mid, end);
}

fn slab(min: Vec3, max: Vec3, from: Vec3, inv: Vec3, tmin: f32, tmax: f32) -> bool {
    let t0 = (min - from) * inv;
    let t1 = (max - from) * inv;
    let near = t0.min(t1).max_element().max(tmin);
    let far = t0.max(t1).min_element().min(tmax);
    near <= far
}

/// Möller–Trumbore, both sides.
fn intersect(t: &Tri, from: Vec3, dir: Vec3) -> Option<(f32, f32, f32)> {
    let e1 = t.b - t.a;
    let e2 = t.c - t.a;
    let p = dir.cross(e2);
    let det = e1.dot(p);
    if det == 0.0 {
        return None;
    }
    let inv = 1.0 / det;
    let s = from - t.a;
    let u = s.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = dir.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    Some((e2.dot(q) * inv, u, v))
}

// yafraycore.dll RVA 0x13ce0 rounds each cross product and dot product to
// binary32. The distance dot product spills in the helper at RVA 0x3590.
// The final u + v comparison stays in x87 registers.
fn intersect_toolset(t: &Tri, from: Vec3, dir: Vec3) -> Option<(f32, f32, f32)> {
    let e1 = (t.b - t.a).as_dvec3();
    let e2 = (t.c - t.a).as_dvec3();
    let direction = dir.as_dvec3();
    let p = direction.cross(e2).as_vec3().as_dvec3();
    let det = e1.dot(p) as f32;
    if det == 0.0 {
        return None;
    }
    let inv = 1.0 / det;
    let from_vertex = (from - t.a).as_dvec3();
    let u = from_vertex.dot(p) as f32 * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = from_vertex.cross(e1).as_vec3().as_dvec3();
    let v = direction.dot(q) as f32 * inv;
    if v < 0.0 || f64::from(u) + f64::from(v) > 1.0 {
        return None;
    }
    Some((e2.dot(q) as f32 * inv, u, v))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toolset_intersection_rounds_the_dot_product_before_scaling_distance() {
        let triangle = Tri {
            a: Vec3::new(1.75, 0.25, -1.0),
            b: Vec3::new(2.5, -2.25, 1.75),
            c: Vec3::new(-2.0, -0.75, -2.0),
            draw: 0,
            casts: true,
        };
        let from = Vec3::new(0.85, -0.81666666, -0.31666666);
        let (t, u, v) = intersect_toolset(&triangle, from, -Vec3::ONE).unwrap();
        // Distance and barycentrics from the pinned yafraycore.dll at RVA 0x13ce0.
        assert_eq!(
            [t, u, v].map(f32::to_bits),
            [0x3dccccd4, 0x3eaaaaab, 0x3eaaaaab]
        );
    }

    #[test]
    fn toolset_normal_rounds_after_the_cross_product() {
        let normal = toolset_normal([
            Vec3::new(1.25, 2.5, 3.75),
            Vec3::new(1.89, 1.43, 3.21),
            Vec3::new(1.89, 1.43, 3.2),
        ]);
        // The pinned yafraycore.dll produces these bits for the thin triangle.
        assert_eq!(
            normal.to_array().map(f32::to_bits),
            [0x3f5bb2fb, 0x3f0368a9, 0]
        );
    }

    fn quad(z: f32, draw: u32) -> [Tri; 2] {
        let (a, b, c, d) = (
            Vec3::new(-1.0, -1.0, z),
            Vec3::new(1.0, -1.0, z),
            Vec3::new(1.0, 1.0, z),
            Vec3::new(-1.0, 1.0, z),
        );
        [
            Tri {
                a,
                b,
                c,
                draw,
                casts: true,
            },
            Tri {
                a,
                b: c,
                c: d,
                draw,
                casts: true,
            },
        ]
    }

    #[test]
    fn finds_the_nearest_of_stacked_quads_from_either_side() {
        let mut tris = Vec::new();
        for i in 0..20 {
            tris.extend(quad(i as f32, i));
        }
        let bvh = Bvh::new(tris);
        let hit = bvh
            .closest(Vec3::new(0.2, 0.1, 5.5), Vec3::NEG_Z, 0.0, f32::MAX)
            .unwrap();
        assert_eq!(bvh.tris[hit.tri as usize].draw, 5);
        assert!((hit.t - 0.5).abs() < 1e-5);
        let up = bvh
            .closest(Vec3::new(0.2, 0.1, 5.5), Vec3::Z, 0.0, f32::MAX)
            .unwrap();
        assert_eq!(bvh.tris[up.tri as usize].draw, 6);
        assert!(bvh.occluded(Vec3::new(0.0, 0.0, -1.0), Vec3::Z, 0.0, 1.5));
        assert!(!bvh.occluded(Vec3::new(0.0, 0.0, -1.0), Vec3::Z, 0.0, 0.5));
        assert!(!bvh.occluded(Vec3::new(3.0, 0.0, -1.0), Vec3::Z, 0.0, 100.0));
    }
}
