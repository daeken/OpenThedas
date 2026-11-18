use super::Bounds;
use crate::legacy_sort;

fn dimensions(bounds: Bounds) -> [f32; 3] {
    std::array::from_fn(|axis| bounds.high[axis] - bounds.low[axis])
}

fn inverse_area(dimensions: [f32; 3]) -> f64 {
    let [x, y, z] = dimensions.map(f64::from);
    f64::from((1.0 / (z * y + x * z + y * x)) as f32)
}

struct AxisArea {
    cap: f64,
    perimeter: f64,
    span: f64,
    inverse_area: f64,
    cost_ratio: f32,
    empty_bonus: f32,
}

impl AxisArea {
    fn new(
        dimensions: [f32; 3],
        axis: usize,
        inverse_area: f64,
        cost_ratio: f32,
        empty_bonus: f32,
    ) -> Self {
        let across = dimensions[(axis + 1) % 3];
        let up = dimensions[(axis + 2) % 3];
        Self {
            cap: f64::from(across * up),
            perimeter: f64::from(across + up),
            span: f64::from(dimensions[axis]),
            inverse_area,
            cost_ratio,
            empty_bonus,
        }
    }

    // yafraycore.dll's sweep and bucket scorer (RVAs 0x1ec90 and 0x1c360)
    // store each area and the weighted sum before applying the empty bonus.
    fn split_cost(&self, left: f64, right: f64, below: usize, above: usize) -> f32 {
        let left_area = (self.cap + left * self.perimeter) as f32;
        let right_area = (self.cap + right * self.perimeter) as f32;
        let raw =
            (f64::from(left_area) * below as f64 + f64::from(right_area) * above as f64) as f32;
        let bonus_fraction = match (below, above) {
            (_, 0) => f64::from(0.1f32) + right / self.span,
            (0, _) => f64::from(0.1f32) + left / self.span,
            _ => 0.0,
        };
        let bonus = (bonus_fraction * f64::from(self.empty_bonus) * f64::from(raw)) as f32;
        self.cost(raw, bonus)
    }

    fn cost(&self, raw: f32, bonus: f32) -> f32 {
        ((f64::from(raw) - f64::from(bonus)) * self.inverse_area + f64::from(self.cost_ratio))
            as f32
    }
}

#[derive(Clone, Copy)]
pub(super) struct Edge {
    point: f32,
    pub(super) kind: EdgeKind,
    pub(super) primitive: usize,
}

#[derive(Clone, Copy, Ord, PartialOrd, Eq, PartialEq)]
pub(super) enum EdgeKind {
    Lower,
    Both,
    Upper,
}

#[derive(Clone, Copy)]
pub(super) struct Split {
    pub(super) axis: usize,
    pub(super) offset: usize,
    pub(super) position: f32,
    pub(super) cost: f32,
}

pub(super) fn edges_for_axis(triangles: &[Bounds], axis: usize, edges: &mut Vec<Edge>) {
    edges.clear();
    for (primitive, bounds) in triangles.iter().enumerate() {
        let start = bounds.low[axis];
        let end = bounds.high[axis];
        if start == end {
            edges.push(Edge {
                point: start,
                kind: EdgeKind::Both,
                primitive,
            });
        } else {
            edges.push(Edge {
                point: start,
                kind: EdgeKind::Lower,
                primitive,
            });
            edges.push(Edge {
                point: end,
                kind: EdgeKind::Upper,
                primitive,
            });
        }
    }
    legacy_sort::sort_by(edges, |a, b| {
        a.point < b.point || (a.point == b.point && a.kind > b.kind)
    });
}

pub(super) fn sweep(
    bounds: Bounds,
    triangles: &[Bounds],
    cost_ratio: f32,
    empty_bonus: f32,
) -> Option<Split> {
    let dimensions = dimensions(bounds);
    let inverse_area = inverse_area(dimensions);
    let primitive_cost = triangles.len() as f64;
    let mut best: Option<Split> = None;
    let mut edges = Vec::with_capacity(triangles.len() * 2);
    for axis in 0..3 {
        let low = bounds.low[axis];
        let high = bounds.high[axis];
        let area = AxisArea::new(dimensions, axis, inverse_area, cost_ratio, empty_bonus);

        edges_for_axis(triangles, axis, &mut edges);
        let mut update = |offset, cost| {
            if cost < best.map_or(f32::INFINITY, |best| best.cost) {
                let edge: Edge = edges[offset];
                best = Some(Split {
                    axis,
                    offset,
                    position: edge.point,
                    cost,
                });
            }
        };

        if triangles.len() >= 6 {
            let leftmost = edges[0].point;
            let left = f64::from(leftmost - low);
            let right = f64::from(high - leftmost);
            if left > right * primitive_cost && right > 0.0 {
                let raw = ((area.cap + right * area.perimeter) * primitive_cost) as f32;
                update(0, area.cost(raw, empty_bonus));
                continue;
            }

            let last = edges.len() - 1;
            let rightmost = edges[last].point;
            let left = f64::from(rightmost - low);
            let right = f64::from(high - rightmost);
            if right > left * primitive_cost && left > 0.0 {
                let raw = ((area.cap + left * area.perimeter) * primitive_cost) as f32;
                update(last, area.cost(raw, empty_bonus));
                continue;
            }
        }

        let mut below = 0;
        let mut above = triangles.len();
        for (offset, edge) in edges.iter().enumerate() {
            if edge.kind == EdgeKind::Upper {
                above -= 1;
            }
            if edge.point > low && edge.point < high {
                let left = f64::from(edge.point - low);
                let right = f64::from(high - edge.point);
                let cost = area.split_cost(left, right, below, above);
                update(offset, cost);
            }
            if edge.kind != EdgeKind::Upper {
                below += 1;
                if edge.kind == EdgeKind::Both {
                    above -= 1;
                }
            }
        }
        assert_eq!((below, above), (triangles.len(), 0));
    }
    best
}

#[derive(Clone, Copy, Default)]
struct Bucket {
    edges: usize,
    point: f32,
    below: usize,
    above: usize,
    start: usize,
    flat: usize,
}

impl Bucket {
    fn change_point(&mut self, point: f32) {
        self.point = point;
        self.below += self.start + self.flat;
        self.above += self.flat;
        self.start = 0;
        self.flat = 0;
    }
}

pub(super) fn bucketed(
    bounds: Bounds,
    triangles: &[Bounds],
    cost_ratio: f32,
    empty_bonus: f32,
) -> Option<Split> {
    const LAST: usize = 1024;
    let dimensions = dimensions(bounds);
    let inverse_area = inverse_area(dimensions);
    let mut best: Option<Split> = None;
    for axis in 0..3 {
        let low = bounds.low[axis];
        let high = bounds.high[axis];
        let area = AxisArea::new(dimensions, axis, inverse_area, cost_ratio, empty_bonus);
        let scale = f64::from((LAST as f64 / area.span) as f32);
        let bin = |point: f32| {
            let offset = (f64::from(point) - f64::from(low)) * scale;
            (offset as i32).clamp(0, LAST as i32) as usize
        };

        let mut buckets = [Bucket::default(); LAST + 1];
        for bounds in triangles {
            let start = bounds.low[axis];
            let end = bounds.high[axis];
            let left = &mut buckets[bin(start)];
            if start == end {
                if left.edges == 0 || start >= left.point {
                    left.point = start;
                    left.flat += 1;
                } else {
                    left.below += 1;
                    left.above += 1;
                }
                left.edges += 2;
            } else {
                if left.edges == 0 || start > left.point {
                    left.change_point(start);
                    left.start = 1;
                } else if start == left.point {
                    left.start += 1;
                } else {
                    left.below += 1;
                }
                left.edges += 1;
                let right = &mut buckets[bin(end)];
                right.above += 1;
                if right.edges == 0 || end > right.point {
                    right.change_point(end);
                }
                right.edges += 1;
            }
        }
        let mut below = 0;
        let mut above = triangles.len();
        for (offset, bucket) in buckets.into_iter().enumerate() {
            if bucket.edges == 0 {
                continue;
            }
            below += bucket.below;
            above -= bucket.above;
            if bucket.point > low && bucket.point < high {
                let left = f64::from(bucket.point - low);
                let right = f64::from(high - bucket.point);
                let cost = area.split_cost(left, right, below, above);
                if cost < best.map_or(f32::INFINITY, |best| best.cost) {
                    best = Some(Split {
                        axis,
                        offset,
                        position: bucket.point,
                        cost,
                    });
                }
            }
            below += bucket.start + bucket.flat;
            above -= bucket.flat;
        }
        assert_eq!((below, above), (triangles.len(), 0));
    }
    best
}
