use glam::{Vec2, Vec3};

use crate::{bvh::toolset_normal, gather::Region, legacy_sort};

// EclipseRay.exe stores these boundaries in film pixels (RVAs 0xdd10,
// 0xc760, 0xc8d0). At a shared U, the strip on the left owns the sample.
const EDGE_EPSILON: f32 = 0.0001;

pub(crate) struct LightmapCamera {
    width: f32,
    height: f32,
    starts: Vec<f32>,
    edges: Vec<Vec<Edge>>,
    triangles: Vec<Projection>,
}

#[derive(Clone, Copy)]
struct Edge {
    slope: f32,
    intercept: f32,
    triangle: Option<usize>,
}

impl Edge {
    fn y(self, x: f32) -> f64 {
        f64::from(self.slope) * f64::from(x) + f64::from(self.intercept)
    }
}

struct Projection {
    world: [Vec3; 3],
    normal: Vec3,
    origin: Vec2,
    inverse: [f32; 4],
}

impl Projection {
    fn new(world: [Vec3; 3], uv: [Vec2; 3]) -> Self {
        let [a, b, c] = uv;
        // The inverse builder stores the UV deltas and reciprocal in
        // binary32 (EclipseRay RVAs 0xa730–0xa76d and 0xa795).
        let u = a - c;
        let v = b - c;
        let det = f64::from(u.x) * f64::from(v.y) - f64::from(u.y) * f64::from(v.x);
        let inv = (1.0 / det) as f32;
        Self {
            world,
            normal: toolset_normal(world),
            origin: uv[2],
            inverse: [v.y, c.x - b.x, c.y - a.y, u.x].map(|v| v * inv),
        }
    }

    fn sample(&self, point: Vec2) -> (Vec3, Vec3) {
        // EclipseRay stores both coordinate differences as binary32 at
        // RVAs 0xca86 and 0xca98 before applying the inverse.
        let offset = (point - self.origin).as_dvec2();
        let [xx, xy, yx, yy] = self.inverse.map(f64::from);
        let a = (offset.x * xx + offset.y * xy) as f32;
        let b = (offset.x * yx + offset.y * yy) as f32;
        let c = (1.0_f64 - f64::from(a) - f64::from(b)) as f32;
        let p = self.world[0] * a + self.world[1] * b + self.world[2] * c;
        (p, self.normal)
    }
}

impl LightmapCamera {
    pub(crate) fn new(region: &Region, texcoords: &[Vec2], width: u32, height: u32) -> Self {
        let size = Vec2::new(width as f32, height as f32);
        let mut unique = texcoords
            .iter()
            .map(|uv| uv.x * size.x)
            .filter(|&x| x.is_finite() && x < size.x)
            .collect::<Vec<_>>();
        unique.sort_unstable_by(f32::total_cmp);
        let mut last = -1.0;
        unique.retain(|&x| {
            if (x - last).abs() <= EDGE_EPSILON {
                return false;
            }
            last = x;
            true
        });
        let mut camera = Self {
            width: size.x,
            height: size.y,
            edges: vec![Vec::new(); unique.len()],
            starts: unique,
            triangles: Vec::with_capacity(region.tris.len()),
        };
        for (triangle, &(world, uv)) in region.tris.iter().enumerate() {
            let uv = uv.map(|p| p * size);
            camera.triangles.push(Projection::new(world, uv));
            for (a, b, c) in [(0, 1, 2), (0, 2, 1), (1, 2, 0)] {
                camera.insert_edge(uv[a], uv[b], uv[c], triangle);
            }
        }
        for (index, edges) in camera.edges.iter_mut().enumerate() {
            let left = camera.starts[index];
            let right = camera.starts.get(index + 1).copied().unwrap_or(size.x);
            let center = ((f64::from(left) + f64::from(right)) * 0.5) as f32;
            // A valid upper edge precedes a coincident lower edge. Two valid
            // edges can compare true in both directions, as in RVA 0xa200.
            legacy_sort::sort_by(edges, |a, b| {
                let ay = a.y(center) as f32;
                let by = b.y(center) as f32;
                if (ay - by).abs() < EDGE_EPSILON {
                    a.triangle.is_some()
                } else {
                    ay < by
                }
            });
        }
        camera
    }

    fn strip(&self, x: f32) -> Option<usize> {
        let first = *self.starts.first()?;
        if x > self.width || f64::from(x) - f64::from(first) < f64::from(EDGE_EPSILON) {
            return None;
        }
        self.starts
            .partition_point(|&start| start < x)
            .checked_sub(1)
    }

    fn insert_edge(&mut self, mut start: Vec2, mut end: Vec2, other: Vec2, triangle: usize) {
        if start.x > self.width || end.x > self.width {
            return;
        }
        if end.x < start.x {
            std::mem::swap(&mut start, &mut end);
        }
        // The edge builder at RVA 0xa0c0 stores dx and the signed vertical
        // distance to the third vertex as binary32 before testing them.
        let dx = end.x - start.x;
        if dx <= EDGE_EPSILON {
            return;
        }
        let Some(last) = self.strip(end.x) else {
            return;
        };
        let first = self.strip(start.x).map_or(0, |index| index + 1);
        if first > last {
            return;
        }
        let slope = ((f64::from(end.y) - f64::from(start.y)) / f64::from(dx)) as f32;
        let intercept = (f64::from(start.y) - f64::from(start.x) * f64::from(slope)) as f32;
        let mut edge = Edge {
            slope,
            intercept,
            triangle: None,
        };
        if ((f64::from(other.y) - edge.y(other.x)) as f32) <= 0.0 {
            edge.triangle = Some(triangle);
        }
        for strip in &mut self.edges[first..=last] {
            strip.push(edge);
        }
    }

    pub(super) fn sample(&self, pixel: Vec2) -> Option<(Vec3, Vec3)> {
        let strip = self.strip(pixel.x)?;
        let point = Vec2::new(pixel.x, self.height - pixel.y);
        // The camera query at RVA 0xc9bf rounds each evaluated edge height
        // before comparing it to the sample.
        let edge = self.edges[strip]
            .iter()
            .find(|edge| point.y <= edge.y(point.x) as f32)?;
        // A selected degenerate chart still has a camera sample. The
        // resulting invalid ray contributes background rather than a hole.
        Some(self.triangles[edge.triangle?].sample(point))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projection_rounds_the_reciprocal_before_scaling_the_inverse() {
        let projection = Projection::new(
            [Vec3::ZERO, Vec3::X, Vec3::Y],
            [Vec2::ZERO, Vec2::new(9.0, 1.0), Vec2::new(3.0, 7.0)],
        );
        // Inverse coefficients from the pinned EclipseRay routine at RVA 0xa710.
        assert_eq!(
            projection.inverse.map(f32::to_bits),
            [0xbdccccce, 0xbdccccce, 0x3deeeef0, 0xbd4cccce]
        );
    }
}
