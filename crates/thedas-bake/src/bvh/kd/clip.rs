use super::Bounds;

type Point = [f64; 3];

#[derive(Clone)]
pub(super) struct Polygon {
    count: usize,
    points: [Point; 10],
}
pub(super) struct DegenerateClip;

impl Polygon {
    pub(super) fn triangle(verts: [Point; 3]) -> Self {
        let mut result = Self {
            count: 3,
            points: [[0.0; 3]; 10],
        };
        result.points[..3].copy_from_slice(&verts);
        result
    }

    pub(super) fn clip(
        &self,
        axis: usize,
        plane: f64,
        lower: bool,
    ) -> Result<Option<Self>, DegenerateClip> {
        let mut result = Self {
            count: 0,
            points: [[0.0; 3]; 10],
        };
        let mut emit = |point| {
            if result.count < result.points.len() {
                result.points[result.count] = point;
            }
            result.count += 1;
        };
        let intersect = |inside: Point, outside: Point| {
            let factor = (plane - inside[axis]) / (outside[axis] - inside[axis]);
            let mut point = std::array::from_fn(|i| inside[i] + factor * (outside[i] - inside[i]));
            point[axis] = plane;
            point
        };
        let inside = |point: Point| {
            if lower {
                point[axis] >= plane
            } else {
                point[axis] <= plane
            }
        };
        for i in 0..self.count {
            let from = self.points[i];
            let to = self.points[(i + 1) % self.count];
            match (inside(from), inside(to)) {
                (true, false) => emit(intersect(from, to)),
                (false, true) => {
                    if to[axis] != plane {
                        emit(intersect(to, from));
                    }
                    emit(to);
                }
                (true, true) => emit(to),
                (false, false) => {}
            }
        }
        match result.count {
            0 => Ok(None),
            2..=9 => Ok(Some(result)),
            _ => Err(DegenerateClip),
        }
    }

    pub(super) fn clip_box(mut self, [low, high]: [[f64; 3]; 2]) -> Option<Self> {
        for (axis, (low, high)) in low.into_iter().zip(high).enumerate() {
            for (plane, lower) in [(low, true), (high, false)] {
                self = self.clip(axis, plane, lower).ok().flatten()?;
            }
        }
        Some(self)
    }

    pub(super) fn bounds(&self) -> Bounds {
        let pts = &self.points[..self.count];
        let range = |reduce: fn(f64, f64) -> f64| {
            std::array::from_fn(|i| pts[1..].iter().fold(pts[0][i], |v, p| reduce(v, p[i])) as f32)
        };
        Bounds {
            low: range(f64::min),
            high: range(f64::max),
        }
    }
}
