//! The level's collision, for sight lines: the shapes of every model a layout's rooms place
//! (`.phy`, see [`thedas_formats::phy`]) in world space, behind a bounding-volume tree for
//! segment queries.
//!
//! DAOrigins.exe's perception tests a hostile's sight line with a ray through the PhysX scene
//! (0x67c530 -> 0x82deb0) from 1.8 m above each creature's feet (0xb004d8), against shapes in
//! the `Placeables | StaticGeometry` mask ([`thedas_formats::phy::mask::SIGHT`]). Creature
//! shapes, items and the invisible terrain walls at cliff edges are not in it. A shape's world
//! placement is room × placement (with its scale) × the `.mmh` node the shape hangs under ×
//! the shape's own pose.
//!
//! An area's placeables are added with [`load_placeables`], all but doors: a closed door's
//! shapes are tested on their own, so that opening it clears them
//! ([`crate::sim::Sim::door_sight`]). Provisional: whether the exe changes a placeable's
//! mask at runtime (opened, destroyed) is not traced; triangles are two-sided (whether the
//! exe's mesh raycast culls back faces is not traced).

use std::sync::Arc;

use thedas_formats::Rml;
use thedas_formats::level::Transform;
use thedas_formats::model::Mmh;
use thedas_formats::phy::{Geometry, Phy, mask};
use thedas_vfs::Vfs;

type V = [f32; 3];

/// A model's collision and the hierarchy its shapes hang under.
type LoadedModel = (Arc<Phy>, Option<Arc<Mmh>>);

/// How far above a creature's feet its eyes are, for sight lines (0xb004d8).
pub const EYE_HEIGHT: f32 = 1.8;

/// How far inside a solid a standing creature's eye may lie and still be taken to be just
/// outside it (see [`Collision::sight_blocked_standing`]): about a body's radius, less
/// than half the thickness of the walls the levels are built from, so that the way out is
/// never through to a wall's far side. Provisional.
pub const EYE_INSIDE: f32 = 0.25;

fn add(a: V, b: V) -> V {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn sub(a: V, b: V) -> V {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn scale(a: V, s: f32) -> V {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn dot(a: V, b: V) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: V, b: V) -> V {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// A similarity transform: `p' = t + s · R(q) p`, with `q` as `[x, y, z, w]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Xf {
    pub t: V,
    pub q: [f32; 4],
    pub s: f32,
}

impl Default for Xf {
    fn default() -> Self {
        Self {
            t: [0.0; 3],
            q: [0.0, 0.0, 0.0, 1.0],
            s: 1.0,
        }
    }
}

fn qmul(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    let [ax, ay, az, aw] = a;
    let [bx, by, bz, bw] = b;
    [
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
        aw * bw - ax * bx - ay * by - az * bz,
    ]
}

fn qrot(q: [f32; 4], v: V) -> V {
    let [x, y, z, w] = q;
    let t = scale(cross([x, y, z], v), 2.0);
    add(add(v, scale(t, w)), cross([x, y, z], t))
}

impl Xf {
    pub fn from_transform(t: &Transform, s: f32) -> Self {
        let n = t.rotation.iter().map(|c| c * c).sum::<f32>().sqrt();
        let q = if n > 1e-6 && n.is_finite() {
            t.rotation.map(|c| c / n)
        } else {
            [0.0, 0.0, 0.0, 1.0]
        };
        Self {
            t: t.position,
            q,
            s,
        }
    }

    pub fn then(&self, o: &Xf) -> Xf {
        Xf {
            t: add(self.t, scale(qrot(self.q, o.t), self.s)),
            q: qmul(self.q, o.q),
            s: self.s * o.s,
        }
    }

    pub fn apply(&self, p: V) -> V {
        add(self.t, scale(qrot(self.q, p), self.s))
    }

    /// A direction (no translation, scaled).
    fn rotate(&self, v: V) -> V {
        scale(qrot(self.q, v), self.s)
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Prim {
    Tri([V; 3]),
    Sphere { c: V, r: f32 },
    Capsule { a: V, b: V, r: f32 },
    Obb { c: V, axes: [V; 3], half: [f32; 3] },
}

impl Prim {
    fn bounds(&self) -> (V, V) {
        match self {
            Prim::Tri(t) => {
                let mut lo = t[0];
                let mut hi = t[0];
                for p in &t[1..] {
                    for i in 0..3 {
                        lo[i] = lo[i].min(p[i]);
                        hi[i] = hi[i].max(p[i]);
                    }
                }
                (lo, hi)
            }
            Prim::Sphere { c, r } => (sub(*c, [*r; 3]), add(*c, [*r; 3])),
            Prim::Capsule { a, b, r } => {
                let lo = [a[0].min(b[0]), a[1].min(b[1]), a[2].min(b[2])];
                let hi = [a[0].max(b[0]), a[1].max(b[1]), a[2].max(b[2])];
                (sub(lo, [*r; 3]), add(hi, [*r; 3]))
            }
            Prim::Obb { c, axes, half } => {
                let mut e = [0.0f32; 3];
                for (axis, h) in axes.iter().zip(half) {
                    for i in 0..3 {
                        e[i] += (axis[i] * h).abs();
                    }
                }
                (sub(*c, e), add(*c, e))
            }
        }
    }

    /// For a solid that holds `p`: the nearest point just outside it and how deep `p` lies
    /// in it. `None` for a triangle and for a solid `p` is not in.
    fn pushed_out(&self, p: V) -> Option<(V, f32)> {
        const SKIN: f32 = 0.01;
        match self {
            Prim::Tri(_) => None,
            Prim::Sphere { c, r } => {
                let v = sub(p, *c);
                let d = dot(v, v).sqrt();
                if d >= *r {
                    return None;
                }
                let dir = if d > 1e-6 {
                    scale(v, 1.0 / d)
                } else {
                    [1.0, 0.0, 0.0]
                };
                Some((add(*c, scale(dir, r + SKIN)), r - d))
            }
            Prim::Capsule { a, b, r } => {
                let e = sub(*b, *a);
                let len = dot(e, e);
                let t = if len > 0.0 {
                    (dot(sub(p, *a), e) / len).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let q = add(*a, scale(e, t));
                let v = sub(p, q);
                let d = dot(v, v).sqrt();
                if d >= *r {
                    return None;
                }
                let dir = if d > 1e-6 {
                    scale(v, 1.0 / d)
                } else {
                    [1.0, 0.0, 0.0]
                };
                Some((add(q, scale(dir, r + SKIN)), r - d))
            }
            Prim::Obb { c, axes, half } => {
                let lp = sub(p, *c);
                let local = [dot(lp, axes[0]), dot(lp, axes[1]), dot(lp, axes[2])];
                let mut best: Option<(usize, f32)> = None;
                for i in 0..3 {
                    let depth = half[i] - local[i].abs();
                    if depth <= 0.0 {
                        return None;
                    }
                    if best.is_none_or(|(_, d)| depth < d) {
                        best = Some((i, depth));
                    }
                }
                let (i, depth) = best?;
                let side = if local[i] < 0.0 { -1.0 } else { 1.0 };
                Some((add(p, scale(axes[i], side * (depth + SKIN))), depth))
            }
        }
    }

    /// Whether the segment from `p` to `p + d` touches the primitive.
    fn hits(&self, p: V, d: V) -> bool {
        match self {
            Prim::Tri(t) => segment_triangle(p, d, t),
            Prim::Sphere { c, r } => segment_point_distance_sq(p, d, *c) <= r * r,
            Prim::Capsule { a, b, r } => {
                segment_segment_distance_sq(p, d, *a, sub(*b, *a)) <= r * r
            }
            Prim::Obb { c, axes, half } => {
                let lp = sub(p, *c);
                let local_p = [dot(lp, axes[0]), dot(lp, axes[1]), dot(lp, axes[2])];
                let local_d = [dot(d, axes[0]), dot(d, axes[1]), dot(d, axes[2])];
                segment_box(local_p, local_d, *half)
            }
        }
    }
}

fn segment_triangle(p: V, d: V, t: &[V; 3]) -> bool {
    let e1 = sub(t[1], t[0]);
    let e2 = sub(t[2], t[0]);
    let h = cross(d, e2);
    let a = dot(e1, h);
    if a.abs() < 1e-9 {
        return false;
    }
    let f = 1.0 / a;
    let s = sub(p, t[0]);
    let u = f * dot(s, h);
    if !(0.0..=1.0).contains(&u) {
        return false;
    }
    let q = cross(s, e1);
    let v = f * dot(d, q);
    if v < 0.0 || u + v > 1.0 {
        return false;
    }
    let along = f * dot(e2, q);
    (0.0..=1.0).contains(&along)
}

fn segment_point_distance_sq(p: V, d: V, c: V) -> f32 {
    let len = dot(d, d);
    let t = if len > 0.0 {
        (dot(sub(c, p), d) / len).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let x = sub(add(p, scale(d, t)), c);
    dot(x, x)
}

/// The squared distance between the segments `p + s·d` and `q + t·e` (s, t in [0, 1]).
fn segment_segment_distance_sq(p: V, d: V, q: V, e: V) -> f32 {
    let r = sub(p, q);
    let (a, b, c) = (dot(d, d), dot(d, e), dot(e, e));
    let (f, g) = (dot(d, r), dot(e, r));
    let denom = a * c - b * b;
    let mut s = if denom > 1e-12 {
        ((b * g - c * f) / denom).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let mut t = if c > 1e-12 { (b * s + g) / c } else { 0.0 };
    if t < 0.0 {
        t = 0.0;
        s = if a > 1e-12 {
            (-f / a).clamp(0.0, 1.0)
        } else {
            0.0
        };
    } else if t > 1.0 {
        t = 1.0;
        s = if a > 1e-12 {
            ((b - f) / a).clamp(0.0, 1.0)
        } else {
            0.0
        };
    }
    let x = sub(add(p, scale(d, s)), add(q, scale(e, t)));
    dot(x, x)
}

/// Slab test of a segment against a box centred on the origin.
fn segment_box(p: V, d: V, half: [f32; 3]) -> bool {
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    for i in 0..3 {
        if d[i].abs() < 1e-12 {
            if p[i].abs() > half[i] {
                return false;
            }
            continue;
        }
        let inv = 1.0 / d[i];
        let (mut t0, mut t1) = ((-half[i] - p[i]) * inv, (half[i] - p[i]) * inv);
        if t0 > t1 {
            std::mem::swap(&mut t0, &mut t1);
        }
        lo = lo.max(t0);
        hi = hi.min(t1);
        if lo > hi {
            return false;
        }
    }
    true
}

fn segment_aabb(p: V, d: V, lo: V, hi: V) -> bool {
    let c = scale(add(lo, hi), 0.5);
    let half = scale(sub(hi, lo), 0.5);
    segment_box(sub(p, c), d, half)
}

#[derive(Clone, Debug)]
struct Node {
    lo: V,
    hi: V,
    /// A leaf's primitives `first..first + count`, else its children `first` and `first + 1`.
    first: u32,
    count: u32,
}

/// World-space collision with a bounding-volume tree.
#[derive(Clone, Debug, Default)]
pub struct Collision {
    prims: Vec<Prim>,
    nodes: Vec<Node>,
}

const LEAF: usize = 4;

impl Collision {
    /// Collision made of triangles (tests).
    #[cfg(test)]
    pub(crate) fn from_triangles(tris: &[[V; 3]]) -> Self {
        Self::build(tris.iter().map(|t| Prim::Tri(*t)).collect())
    }

    fn build(mut prims: Vec<Prim>) -> Self {
        let mut c = Collision {
            prims: Vec::new(),
            nodes: Vec::new(),
        };
        if prims.is_empty() {
            return c;
        }
        let mut items: Vec<(V, V, V)> = prims
            .iter()
            .map(|p| {
                let (lo, hi) = p.bounds();
                (lo, hi, scale(add(lo, hi), 0.5))
            })
            .collect();
        let mut order: Vec<usize> = (0..prims.len()).collect();
        c.nodes.push(Node {
            lo: [0.0; 3],
            hi: [0.0; 3],
            first: 0,
            count: 0,
        });
        let mut stack = vec![(0usize, 0usize, order.len())];
        while let Some((node, start, end)) = stack.pop() {
            let mut lo = [f32::MAX; 3];
            let mut hi = [f32::MIN; 3];
            let mut clo = [f32::MAX; 3];
            let mut chi = [f32::MIN; 3];
            for &i in &order[start..end] {
                let (l, h, m) = items[i];
                for k in 0..3 {
                    lo[k] = lo[k].min(l[k]);
                    hi[k] = hi[k].max(h[k]);
                    clo[k] = clo[k].min(m[k]);
                    chi[k] = chi[k].max(m[k]);
                }
            }
            c.nodes[node].lo = lo;
            c.nodes[node].hi = hi;
            if end - start <= LEAF {
                c.nodes[node].first = start as u32;
                c.nodes[node].count = (end - start) as u32;
                continue;
            }
            let ext = sub(chi, clo);
            let axis = if ext[0] >= ext[1] && ext[0] >= ext[2] {
                0
            } else if ext[1] >= ext[2] {
                1
            } else {
                2
            };
            let mid = (start + end) / 2;
            order[start..end].select_nth_unstable_by(mid - start, |&a, &b| {
                items[a].2[axis].total_cmp(&items[b].2[axis])
            });
            let left = c.nodes.len();
            for _ in 0..2 {
                c.nodes.push(Node {
                    lo: [0.0; 3],
                    hi: [0.0; 3],
                    first: 0,
                    count: 0,
                });
            }
            c.nodes[node].first = left as u32;
            c.nodes[node].count = 0;
            stack.push((left, start, mid));
            stack.push((left + 1, mid, end));
        }
        let mut sorted: Vec<Option<Prim>> = prims.drain(..).map(Some).collect();
        c.prims = order
            .iter()
            .map(|&i| sorted[i].take().expect("each primitive once"))
            .collect();
        items.clear();
        c
    }

    pub fn is_empty(&self) -> bool {
        self.prims.is_empty()
    }

    pub fn len(&self) -> usize {
        self.prims.len()
    }

    /// Whether anything lies on the segment from `a` to `b`.
    pub fn blocked(&self, a: V, b: V) -> bool {
        if self.nodes.is_empty() {
            return false;
        }
        let d = sub(b, a);
        let mut stack = vec![0usize];
        while let Some(n) = stack.pop() {
            let node = &self.nodes[n];
            if !segment_aabb(a, d, node.lo, node.hi) {
                continue;
            }
            if node.count > 0 {
                let s = node.first as usize;
                if self.prims[s..s + node.count as usize]
                    .iter()
                    .any(|p| p.hits(a, d))
                {
                    return true;
                }
            } else {
                stack.push(node.first as usize);
                stack.push(node.first as usize + 1);
            }
        }
        false
    }

    /// Whether a creature standing at `a` could see one at `b` (eyes [`EYE_HEIGHT`] above
    /// their feet).
    pub fn sight_blocked(&self, a: V, b: V) -> bool {
        self.blocked(
            add(a, [0.0, 0.0, EYE_HEIGHT]),
            add(b, [0.0, 0.0, EYE_HEIGHT]),
        )
    }

    /// Collision made of boxes (tests): each a centre and half extents, axis-aligned.
    #[cfg(test)]
    pub(crate) fn from_boxes(boxes: &[(V, V)]) -> Self {
        Self::build(
            boxes
                .iter()
                .map(|&(c, half)| Prim::Obb {
                    c,
                    axes: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
                    half,
                })
                .collect(),
        )
    }

    /// Whether the eye of a creature standing at `p` lies inside a solid (a box, sphere or
    /// capsule).
    pub fn eye_inside(&self, p: V) -> bool {
        let e = add(p, [0.0, 0.0, EYE_HEIGHT]);
        let mut stack = if self.nodes.is_empty() {
            Vec::new()
        } else {
            vec![0usize]
        };
        while let Some(n) = stack.pop() {
            let node = &self.nodes[n];
            if (0..3).any(|i| e[i] < node.lo[i] || e[i] > node.hi[i]) {
                continue;
            }
            if node.count > 0 {
                let s = node.first as usize;
                if self.prims[s..s + node.count as usize]
                    .iter()
                    .any(|prim| prim.pushed_out(e).is_some())
                {
                    return true;
                }
            } else {
                stack.push(node.first as usize);
                stack.push(node.first as usize + 1);
            }
        }
        false
    }

    /// Where an eye at `p` is taken to be when it lies a little way inside a solid (a box,
    /// sphere or capsule): the nearest point just outside, if that is within
    /// [`EYE_INSIDE`]. See [`Collision::sight_blocked_standing`].
    fn eye_clear(&self, mut p: V) -> V {
        let mut moved = 0.0;
        for _ in 0..3 {
            let mut best: Option<(V, f32)> = None;
            let mut stack = if self.nodes.is_empty() {
                Vec::new()
            } else {
                vec![0usize]
            };
            while let Some(n) = stack.pop() {
                let node = &self.nodes[n];
                if (0..3).any(|i| p[i] < node.lo[i] || p[i] > node.hi[i]) {
                    continue;
                }
                if node.count > 0 {
                    let s = node.first as usize;
                    for prim in &self.prims[s..s + node.count as usize] {
                        if let Some((q, depth)) = prim.pushed_out(p) {
                            if best.is_none_or(|(_, d)| depth < d) {
                                best = Some((q, depth));
                            }
                        }
                    }
                } else {
                    stack.push(node.first as usize);
                    stack.push(node.first as usize + 1);
                }
            }
            match best {
                Some((q, depth)) if moved + depth <= EYE_INSIDE => {
                    moved += depth;
                    p = q;
                }
                _ => break,
            }
        }
        p
    }

    /// [`Collision::sight_blocked`] for creatures standing in the level: an eye that lies
    /// a little way inside a solid (up to [`EYE_INSIDE`]) is taken to be at the nearest
    /// point outside it.
    ///
    /// DAOrigins.exe's creatures are capsules the level's geometry keeps out, so a
    /// creature's eye is never inside a wall. Ours follow the pathing grid, whose half-metre
    /// cells straddle wall ends: a creature in such a cell can stand inside the wall's
    /// box, where every sight ray from its eye starts inside the box and is blocked (the
    /// Dalish ruins' mirror room: Tamlen 16 cm inside the end of the wall beside the
    /// doorway saw no enemy and no enemy saw him, so the fight ended whenever the hero
    /// fell). Perception first moves such a creature's standing point to the middle of
    /// its pathing cell (`Sim::update_perception`); this covers what is left. Provisional:
    /// both stand in for keeping creatures out of geometry as they move; triangle meshes
    /// have no inside and are not treated.
    pub fn sight_blocked_standing(&self, a: V, b: V) -> bool {
        self.blocked(
            self.eye_clear(add(a, [0.0, 0.0, EYE_HEIGHT])),
            self.eye_clear(add(b, [0.0, 0.0, EYE_HEIGHT])),
        )
    }
}

/// Collects a model's sight-blocking shapes into `out`, placed by `at`.
fn add_model(phy: &Phy, mmh: Option<&Mmh>, at: &Xf, out: &mut Vec<Prim>) {
    for shape in &phy.shapes {
        if shape.mask & mask::SIGHT == 0 {
            continue;
        }
        let mut xf = *at;
        if let (Some(m), Some(parent)) = (mmh, shape.parent.as_deref()) {
            if let Some(chain) = m.node_chain(parent) {
                for t in &chain {
                    xf = xf.then(&Xf::from_transform(t, 1.0));
                }
            }
        }
        let xf = xf.then(&Xf::from_transform(&shape.pose, 1.0));
        match &shape.geometry {
            Geometry::Mesh {
                vertices,
                triangles,
            } => {
                let world: Vec<V> = vertices.iter().map(|&v| xf.apply(v)).collect();
                for t in triangles {
                    out.push(Prim::Tri([
                        world[t[0] as usize],
                        world[t[1] as usize],
                        world[t[2] as usize],
                    ]));
                }
            }
            Geometry::Sphere { radius } => out.push(Prim::Sphere {
                c: xf.apply([0.0; 3]),
                r: radius * xf.s,
            }),
            Geometry::Capsule { radius, height } => out.push(Prim::Capsule {
                a: xf.apply([0.0, 0.0, *radius]),
                b: xf.apply([0.0, 0.0, radius + height]),
                r: radius * xf.s,
            }),
            Geometry::Box { half } => {
                let axes = [
                    xf.rotate([1.0, 0.0, 0.0]),
                    xf.rotate([0.0, 1.0, 0.0]),
                    xf.rotate([0.0, 0.0, 1.0]),
                ];
                let unit = axes.map(|a| {
                    let n = dot(a, a).sqrt();
                    if n > 0.0 { scale(a, 1.0 / n) } else { a }
                });
                out.push(Prim::Obb {
                    c: xf.apply([0.0; 3]),
                    axes: unit,
                    half: half.map(|h| h * xf.s),
                });
            }
        }
    }
}

/// Builds a layout's collision from its rooms (`<room>.rml`, else `<room>_0.rml`, the most
/// detailed), reading each placed model's `.phy` and `.mmh` from `level` (the level's own
/// archives) or else `global`. Models without collision are skipped.
pub fn load_layout(level: &Vfs, global: &Vfs, arl: &thedas_formats::Arl) -> Collision {
    let read = |name: &str| -> Option<Vec<u8>> {
        level
            .read(name)
            .or_else(|_| global.read(name))
            .ok()
            .map(|(d, _)| d)
    };
    let mut models: std::collections::HashMap<String, Option<LoadedModel>> =
        std::collections::HashMap::new();
    let mut prims = Vec::new();
    for room in &arl.rooms {
        let rml = [format!("{}.rml", room.name), format!("{}_0.rml", room.name)]
            .iter()
            .find_map(|n| read(n))
            .and_then(|d| Rml::parse(&d).ok());
        let Some(rml) = rml else { continue };
        let room_xf = Xf::from_transform(&rml.transform, 1.0);
        for p in &rml.placements {
            let stem = p.model.to_ascii_lowercase();
            let entry = models.entry(stem.clone()).or_insert_with(|| {
                let phy = read(&format!("{stem}.phy")).and_then(|d| Phy::parse(&d).ok())?;
                let mmh = read(&format!("{stem}.mmh")).and_then(|d| Mmh::parse(&d).ok());
                Some((Arc::new(phy), mmh.map(Arc::new)))
            });
            let Some((phy, mmh)) = entry.clone() else {
                continue;
            };
            let scale = if p.scale.is_finite() && p.scale > 0.0 {
                p.scale
            } else {
                1.0
            };
            let at = room_xf.then(&Xf::from_transform(&p.transform, scale));
            add_model(&phy, mmh.as_deref(), &at, &mut prims);
        }
    }
    Collision::build(prims)
}

/// Builds the collision of an area's placeables: each one's model (`placeable_types.gda`
/// `ModelName`) `.phy`, placed at the object's position and turned by its facing (degrees
/// about +Z, as the renderer places it).
pub fn load_placeables(global: &Vfs, placed: &[(String, V, f32)]) -> Collision {
    let mut models: std::collections::HashMap<String, Option<LoadedModel>> =
        std::collections::HashMap::new();
    let mut prims = Vec::new();
    for (model, position, facing) in placed {
        let stem = model.to_ascii_lowercase();
        let entry = models.entry(stem.clone()).or_insert_with(|| {
            let read = |n: &str| global.read(n).ok().map(|(d, _)| d);
            let phy = read(&format!("{stem}.phy")).and_then(|d| Phy::parse(&d).ok())?;
            let mmh = read(&format!("{stem}.mmh")).and_then(|d| Mmh::parse(&d).ok());
            Some((Arc::new(phy), mmh.map(Arc::new)))
        });
        let Some((phy, mmh)) = entry.clone() else {
            continue;
        };
        let half = facing.to_radians() * 0.5;
        let at = Xf {
            t: *position,
            q: [0.0, 0.0, half.sin(), half.cos()],
            s: 1.0,
        };
        add_model(&phy, mmh.as_deref(), &at, &mut prims);
    }
    Collision::build(prims)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segments_hit_each_kind_of_shape() {
        let c = Collision::build(vec![
            Prim::Tri([[5.0, -1.0, -1.0], [5.0, 1.0, -1.0], [5.0, 0.0, 2.0]]),
            Prim::Sphere {
                c: [0.0, 10.0, 0.0],
                r: 1.0,
            },
            Prim::Capsule {
                a: [10.0, 0.0, 0.0],
                b: [10.0, 0.0, 3.0],
                r: 0.5,
            },
            Prim::Obb {
                c: [0.0, -10.0, 0.0],
                axes: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
                half: [1.0, 1.0, 1.0],
            },
        ]);
        // Through the triangle, past it above, and short of it.
        assert!(c.blocked([0.0, 0.0, 0.0], [8.0, 0.0, 0.0]));
        assert!(!c.blocked([0.0, 0.0, 3.0], [8.0, 0.0, 3.0]));
        assert!(!c.blocked([0.0, 0.0, 0.0], [4.0, 0.0, 0.0]));
        assert!(c.blocked([-5.0, 10.0, 0.5], [5.0, 10.0, 0.5]), "sphere");
        assert!(c.blocked([9.6, -5.0, 2.0], [9.6, 5.0, 2.0]), "capsule");
        assert!(
            !c.blocked([9.0, -5.0, 4.0], [9.0, 5.0, 4.0]),
            "above the capsule"
        );
        assert!(c.blocked([-5.0, -10.0, 0.0], [5.0, -10.0, 0.0]), "box");
        assert!(
            !c.blocked([-5.0, -12.0, 0.0], [5.0, -12.0, 0.0]),
            "beside the box"
        );
    }

    #[test]
    fn a_tree_of_many_triangles_finds_the_one_in_the_way() {
        // A row of small walls along x; only the one at x = 50 crosses y = 0.
        let mut prims = Vec::new();
        for i in 0..200 {
            let x = i as f32;
            let y = if i == 50 { 0.0 } else { 5.0 };
            prims.push(Prim::Tri([
                [x, y - 0.5, -1.0],
                [x, y + 0.5, -1.0],
                [x, y, 3.0],
            ]));
        }
        let c = Collision::build(prims);
        assert_eq!(c.len(), 200);
        assert!(c.blocked([0.5, 0.0, 0.0], [99.5, 0.0, 0.0]));
        assert!(!c.blocked([0.5, 0.0, 0.0], [49.5, 0.0, 0.0]));
        // Eyes 1.8 m up clear a wall 1 m high.
        let low = Collision::build(vec![Prim::Obb {
            c: [5.0, 0.0, 0.5],
            axes: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            half: [0.2, 3.0, 0.5],
        }]);
        assert!(!low.sight_blocked([0.0, 0.0, 0.0], [10.0, 0.0, 0.0]));
        assert!(low.blocked([0.0, 0.0, 0.5], [10.0, 0.0, 0.5]));
    }

    #[test]
    fn an_eye_just_inside_a_wall_sees_from_outside_it() {
        // The Dalish ruins' mirror room: a wall 0.71 m thick ending at y = -1.327 beside the
        // doorway, and Tamlen standing 16 cm inside its end; the bereskarn 3.8 m east.
        let wall = Collision::build(vec![Prim::Obb {
            c: [-76.655, -2.730_181_5, 2.750_15],
            axes: [[0.0, 1.0, 0.0], [-1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
            half: [1.403_52, 0.353, 2.75],
        }]);
        let tamlen = [-76.777_05, -1.483_174_6, 0.014_3];
        let bear = [-72.983_46, -1.283_926_5, 0.0];
        assert!(
            wall.sight_blocked(tamlen, bear),
            "the plain ray starts inside"
        );
        assert!(!wall.sight_blocked_standing(tamlen, bear));
        assert!(!wall.sight_blocked_standing(bear, tamlen));
        // The wall still hides what is behind it: someone west of it, further south.
        let behind = [-78.5, -3.5, 0.0];
        assert!(wall.sight_blocked_standing(bear, behind));
        assert!(wall.sight_blocked_standing([-75.0, -3.0, 0.0], behind));
        // An eye deep inside it is not helped out, nor one nearer the far side of a wall.
        let deep = [-76.655, -3.5, 0.0];
        assert!(wall.sight_blocked_standing(deep, bear));
        assert!(wall.eye_inside(tamlen) && wall.eye_inside(deep) && !wall.eye_inside(bear));
        let west_side = [-76.95, -3.0, 0.0];
        assert!(
            wall.sight_blocked_standing(west_side, bear),
            "5 cm in from the west"
        );
    }

    #[test]
    fn transforms_compose_as_placement_then_node_then_pose() {
        // A quarter turn about z then a metre along the new x.
        let s = std::f32::consts::FRAC_1_SQRT_2;
        let turn = Xf {
            t: [10.0, 0.0, 0.0],
            q: [0.0, 0.0, s, s],
            s: 2.0,
        };
        let step = Xf {
            t: [1.0, 0.0, 0.0],
            ..Default::default()
        };
        let p = turn.then(&step).apply([0.0; 3]);
        assert!(
            (p[0] - 10.0).abs() < 1e-5 && (p[1] - 2.0).abs() < 1e-5,
            "{p:?}"
        );
    }
}
