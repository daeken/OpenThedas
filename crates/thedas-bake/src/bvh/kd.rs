//! The spatial tree built by the Toolset's YafaRay core. The ordered leaves
//! and the rounded segment endpoints affect which triangles a ray tests.

mod clip;
mod split;

use std::rc::Rc;

use glam::Vec3;

use super::{Hit, Tri, intersect_toolset};
use clip::Polygon;
use split::EdgeKind;

const CLIP_THRESHOLD: usize = 32;
const BUCKET_THRESHOLD: usize = 128;
const MAX_DEPTH: usize = 64;

#[derive(Clone, Copy)]
struct Bounds {
    low: [f32; 3],
    high: [f32; 3],
}

enum Node {
    Branch {
        axis: usize,
        plane: f32,
        right: usize,
    },
    Leaf {
        start: usize,
        end: usize,
    },
}

pub(super) struct Tree {
    bounds: Bounds,
    nodes: Vec<Node>,
    primitives: Vec<u32>,
}

#[derive(Clone)]
struct Candidate {
    primitive: u32,
    polygon: Option<Rc<Polygon>>,
}

struct Builder<'a> {
    tree: Tree,
    triangles: &'a [Tri],
    primitive_bounds: Vec<Bounds>,
    max_depth: usize,
    cost_ratio: f32,
}

fn triangle_points(tri: &Tri) -> [[f64; 3]; 3] {
    [tri.a, tri.b, tri.c].map(|p| p.as_dvec3().to_array())
}

impl Tree {
    pub(super) fn new(triangles: &[Tri]) -> Option<Self> {
        let primitive_bounds: Vec<_> = triangles
            .iter()
            .map(|triangle| Polygon::triangle(triangle_points(triangle)).bounds())
            .collect();
        let mut root = *primitive_bounds.first()?;
        for bounds in &primitive_bounds[1..] {
            for axis in 0..3 {
                root.low[axis] = root.low[axis].min(bounds.low[axis]);
                root.high[axis] = root.high[axis].max(bounds.high[axis]);
            }
        }
        // The constructor at yafraycore.dll RVA 0x1b3e0 expands the root
        // and sets the automatic depth from the number of primitives.
        for axis in 0..3 {
            let low = f64::from(root.low[axis]);
            let high = f64::from(root.high[axis]);
            let pad = (high - low) * 0.001;
            root.low[axis] = (low - pad) as f32;
            root.high[axis] = (high + pad) as f32;
        }
        let log_count = (triangles.len() as f64).ln();
        let max_depth = ((7.0 + f64::from(1.66f32) * log_count) as usize).min(MAX_DEPTH);
        let log2_leaves = log_count * f64::from(std::f32::consts::LOG2_E);
        let cost_ratio = (f64::from(0.8f32) + 0.25 * (log2_leaves - 16.0).max(0.0)) as f32;

        let mut builder = Builder {
            tree: Self {
                bounds: root,
                nodes: Vec::new(),
                primitives: Vec::new(),
            },
            triangles,
            primitive_bounds,
            max_depth,
            cost_ratio,
        };
        let all = (0..triangles.len())
            .map(|i| Candidate {
                primitive: i as u32,
                polygon: None,
            })
            .collect();
        builder.build(root, all, 0, 0, None);
        Some(builder.tree)
    }

    pub(super) fn walk(
        &self,
        triangles: &[Tri],
        ray: (Vec3, Vec3),
        tmin: f32,
        limit: &mut f32,
        casters_only: bool,
        on_hit: &mut dyn FnMut(Hit, &mut f32) -> bool,
    ) {
        let (from, direction) = ray;
        let maximum = *limit;
        let Some([enter, exit]) = self.bounds.cross(from, direction, maximum) else {
            return;
        };
        let mut near = Endpoint {
            point: if enter >= 0.0 {
                from + direction * enter
            } else {
                from
            },
            t: enter,
        };
        let mut far = Endpoint {
            point: from + direction * exit,
            t: exit,
        };
        let inv = Vec3::ONE / direction;
        let wide_from = from.as_dvec3();
        let wide_direction = direction.as_dvec3();
        let mut node = 0;
        let mut pending = [(0, far); MAX_DEPTH];
        let mut depth = 0;
        // The original tests the ray at the rounded entry and exit
        // coordinates (RVA 0x193c0). An endpoint on the plane is on the left.
        while near.t <= maximum {
            match self.nodes[node] {
                Node::Branch { axis, plane, right } => {
                    let enter_left = near.point[axis] <= plane;
                    let exit_left = far.point[axis] <= plane;
                    if enter_left == exit_left {
                        node = if enter_left { node + 1 } else { right };
                        continue;
                    }
                    let t = ((f64::from(plane) - wide_from[axis]) * f64::from(inv[axis])) as f32;
                    let mut point = (wide_from + wide_direction * f64::from(t)).as_vec3();
                    point[axis] = plane;
                    let left = node + 1;
                    let (child, other) = if enter_left {
                        (left, right)
                    } else {
                        (right, left)
                    };
                    pending[depth] = (other, far);
                    depth += 1;
                    node = child;
                    far = Endpoint { point, t };
                }
                Node::Leaf { start, end } => {
                    for &i in &self.primitives[start..end] {
                        let triangle = &triangles[i as usize];
                        if casters_only && !triangle.casts {
                            continue;
                        }
                        if let Some((t, u, v)) = intersect_toolset(triangle, from, direction)
                            && t > tmin
                            && t < *limit
                            && on_hit(Hit { t, tri: i, u, v }, limit)
                        {
                            return;
                        }
                    }
                    if *limit <= far.t && *limit < maximum || depth == 0 {
                        return;
                    }
                    near = far;
                    depth -= 1;
                    (node, far) = pending[depth];
                }
            }
        }
    }
}

#[derive(Clone, Copy)]
struct Endpoint {
    point: Vec3,
    t: f32,
}

impl Bounds {
    // bound_t::cross at RVA 0x2060 first stores the origin relative to
    // the low corner, then rounds each divided interval endpoint.
    fn cross(self, from: Vec3, direction: Vec3, limit: f32) -> Option<[f32; 2]> {
        let mut entry = -1.0f32;
        let mut exit = -1.0f32;
        for axis in 0..3 {
            let d = f64::from(direction[axis]);
            if d == 0.0 {
                continue;
            }
            let low = f64::from(self.low[axis]);
            let high = f64::from(self.high[axis]);
            let relative = f64::from(from[axis] - self.low[axis]);
            let to_low = (-relative / d) as f32;
            let to_high = ((high - low - relative) / d) as f32;
            let (near, far) = if to_high < to_low {
                (to_high, to_low)
            } else {
                (to_low, to_high)
            };
            entry = if axis == 0 { near } else { entry.max(near) };
            exit = if exit < 0.0 { far } else { exit.min(far) };
            if exit < 0.0 || entry > limit {
                return None;
            }
        }
        (entry <= exit && exit >= 0.0 && entry <= limit).then_some([entry, exit])
    }
}

impl Builder<'_> {
    fn expanded_limits(&self, bounds: Bounds) -> [[f64; 3]; 2] {
        let low = bounds.low.map(f64::from);
        let high = bounds.high.map(f64::from);
        let root = self.tree.bounds;
        let padding: [f64; 3] =
            std::array::from_fn(|i| (f64::from(root.high[i]) - f64::from(root.low[i])) * 0.00001);
        [
            std::array::from_fn(|i| low[i] - (high[i] - low[i]) * 0.021 - padding[i]),
            std::array::from_fn(|i| high[i] + (high[i] - low[i]) * 0.021 + padding[i]),
        ]
    }

    fn clip_candidates(
        &self,
        bounds: Bounds,
        candidates: &mut Vec<Candidate>,
        plane: Option<(usize, bool)>,
    ) {
        let expanded = self.expanded_limits(bounds);
        candidates.retain_mut(|candidate| {
            let source = &self.triangles[candidate.primitive as usize];
            let clip_box = || Polygon::triangle(triangle_points(source)).clip_box(expanded);
            let polygon = match (plane, &candidate.polygon) {
                (Some((axis, lower)), Some(previous)) => previous
                    .clip(axis, expanded[usize::from(!lower)][axis], lower)
                    .unwrap_or_else(|_| clip_box()),
                _ => clip_box(),
            };
            candidate.polygon = polygon.map(Rc::new);
            candidate.polygon.is_some()
        });
    }

    fn leaf(&mut self, candidates: Vec<Candidate>) {
        let start = self.tree.primitives.len();
        self.tree
            .primitives
            .extend(candidates.into_iter().map(|c| c.primitive));
        self.tree.nodes.push(Node::Leaf {
            start,
            end: self.tree.primitives.len(),
        });
    }

    // triKdTree_t::buildTree (RVA 0x1a210) clips small nodes, preserves the
    // ordered edge partition, and stores the left subtree before the right.
    fn build(
        &mut self,
        bounds: Bounds,
        mut candidates: Vec<Candidate>,
        depth: usize,
        mut bad_splits: usize,
        plane: Option<(usize, bool)>,
    ) {
        let small = candidates.len() <= CLIP_THRESHOLD;
        if small {
            self.clip_candidates(bounds, &mut candidates, plane);
        }
        let count = candidates.len();
        if count <= 1 || depth >= self.max_depth {
            self.leaf(candidates);
            return;
        }

        let cost_bounds: Vec<_> = candidates
            .iter()
            .map(|c| {
                c.polygon
                    .as_ref()
                    .map_or(self.primitive_bounds[c.primitive as usize], |p| p.bounds())
            })
            .collect();
        let bonus = ((1.1 - depth as f64 / self.max_depth as f64) * f64::from(0.33f32)) as f32;
        let optimal = if count > BUCKET_THRESHOLD {
            split::bucketed(bounds, &cost_bounds, self.cost_ratio, bonus)
        } else {
            split::sweep(bounds, &cost_bounds, self.cost_ratio, bonus)
        };
        let Some(optimal) = optimal else {
            self.leaf(candidates);
            return;
        };
        bad_splits += usize::from(optimal.cost > count as f32);
        if bad_splits == 2
            || count < 16 && f64::from(optimal.cost) > f64::from(1.6f32) * count as f64
        {
            self.leaf(candidates);
            return;
        }

        let axis = optimal.axis;
        let position = optimal.position;
        let mut left = Vec::new();
        let mut right = Vec::new();
        if count > BUCKET_THRESHOLD {
            for (candidate, bound) in candidates.into_iter().zip(cost_bounds) {
                if bound.low[axis] >= position {
                    right.push(candidate);
                } else if bound.high[axis] > position {
                    left.push(candidate.clone());
                    right.push(candidate);
                } else {
                    left.push(candidate);
                }
            }
        } else {
            let mut edges = Vec::with_capacity(count * 2);
            split::edges_for_axis(&cost_bounds, axis, &mut edges);
            let (left_edges, right_edges) = edges.split_at(optimal.offset);
            left.extend(
                left_edges
                    .iter()
                    .filter(|e| e.kind != EdgeKind::Upper)
                    .map(|e| candidates[e.primitive].clone()),
            );
            if right_edges[0].kind == EdgeKind::Both {
                right.push(candidates[right_edges[0].primitive].clone());
            }
            right.extend(
                right_edges[1..]
                    .iter()
                    .filter(|e| e.kind != EdgeKind::Lower)
                    .map(|e| candidates[e.primitive].clone()),
            );
        }

        let index = self.tree.nodes.len();
        self.tree.nodes.push(Node::Branch {
            axis,
            plane: position,
            right: 0,
        });
        let mut left_bounds = bounds;
        left_bounds.high[axis] = position;
        self.build(
            left_bounds,
            left,
            depth + 1,
            bad_splits,
            small.then_some((axis, false)),
        );
        let right_index = self.tree.nodes.len();
        self.tree.nodes[index] = Node::Branch {
            axis,
            plane: position,
            right: right_index,
        };
        let mut right_bounds = bounds;
        right_bounds.low[axis] = position;
        self.build(
            right_bounds,
            right,
            depth + 1,
            bad_splits,
            small.then_some((axis, true)),
        );
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, path::Path};

    use super::*;
    use crate::bvh::Bvh;

    fn words(path: &Path) -> Vec<u32> {
        let data = fs::read(path).unwrap_or_else(|err| panic!("{}: {err}", path.display()));
        assert_eq!(data.len() % 4, 0, "{}", path.display());
        data.chunks_exact(4)
            .map(|part| u32::from_le_bytes(part.try_into().unwrap()))
            .collect()
    }

    fn point(bits: &[u32]) -> Vec3 {
        Vec3::from_array(<[u32; 3]>::try_from(bits).unwrap().map(f32::from_bits))
    }

    #[test]
    #[ignore = "requires local tree and ray captures from the original Toolset"]
    fn stock_toolset_tree_and_primary_hits_match() {
        let fixture = std::env::var_os("THEDAS_BAKE_TOOLSET_TREE_REFERENCE")
            .expect("set THEDAS_BAKE_TOOLSET_TREE_REFERENCE to the original capture directory");
        let fixture = Path::new(&fixture);
        let vertices = words(&fixture.join("original-builder-ordered.bin"));
        assert_eq!(vertices.len(), 48_664 * 9);
        let triangles = vertices
            .chunks_exact(9)
            .map(|p| Tri {
                a: point(&p[..3]),
                b: point(&p[3..6]),
                c: point(&p[6..]),
                draw: 0,
                casts: true,
            })
            .collect();
        let scene = Bvh::new_toolset(triangles);
        let tree = scene.toolset.as_ref().unwrap();
        let expected = words(&fixture.join("original-builder-tree-normalized.bin"));
        assert_eq!(expected[0] as usize, tree.nodes.len());
        assert_eq!(expected[1..4], tree.bounds.low.map(f32::to_bits));
        assert_eq!(expected[4..7], tree.bounds.high.map(f32::to_bits));
        let mut cursor = 7;
        for (index, node) in tree.nodes.iter().enumerate() {
            let data = expected[cursor];
            let flags = expected[cursor + 1];
            cursor += 2;
            match *node {
                Node::Branch { axis, plane, right } => {
                    assert_eq!(data, plane.to_bits(), "split plane at node {index}");
                    assert_eq!(flags, (right as u32) << 2 | axis as u32, "node {index}");
                }
                Node::Leaf { start, end } => {
                    let count = end - start;
                    assert_eq!(flags, (count as u32) << 2 | 3, "node {index}");
                    assert_eq!(
                        expected[cursor..cursor + count],
                        tree.primitives[start..end],
                        "primitive order at node {index}"
                    );
                    cursor += count;
                }
            }
        }
        assert_eq!(cursor, expected.len());

        for (part, count) in [("3h_0_1zi", 49_284), ("4e_0_ck", 23_896)] {
            let captured = words(&fixture.join(format!("original-{part}-ordered-hits.bin")));
            assert_eq!(captured.len(), count * 15);
            for (index, ray) in captured.chunks_exact(15).enumerate() {
                let hit = scene.closest(point(&ray[..3]), point(&ray[3..6]), 0.0, f32::INFINITY);
                assert_eq!(
                    hit.map(|hit| (hit.t.to_bits(), hit.tri)),
                    (ray[6] != 0).then_some((ray[7], ray[14])),
                    "{part} ray {index}"
                );
            }
        }
    }
}
