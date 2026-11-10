//! A free-fly camera in DA:O world space (Z up).

use glam::{Mat4, Vec3};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    pub position: Vec3,
    /// Radians around +Z; 0 looks along +X, pi/2 along +Y.
    pub yaw: f32,
    /// Radians above the horizon.
    pub pitch: f32,
    /// Vertical field of view in radians.
    pub fov_y: f32,
    pub near: f32,
    pub far: f32,
}

impl Camera {
    pub fn new(position: Vec3, yaw: f32, pitch: f32) -> Self {
        Self {
            position,
            yaw,
            pitch,
            fov_y: 60f32.to_radians(),
            near: 0.05,
            far: 2000.0,
        }
    }

    /// Parses `x,y,z,yaw,pitch` with angles in degrees.
    pub fn parse(spec: &str) -> Option<Self> {
        let v: Vec<f32> = spec
            .split(',')
            .map(|s| s.trim().parse().ok())
            .collect::<Option<_>>()?;
        let [x, y, z, yaw, pitch] = v[..] else {
            return None;
        };
        Some(Self::new(
            Vec3::new(x, y, z),
            yaw.to_radians(),
            pitch.to_radians(),
        ))
    }

    /// The `x,y,z,yaw,pitch` form [`Camera::parse`] reads.
    pub fn spec(&self) -> String {
        format!(
            "{:.3},{:.3},{:.3},{:.2},{:.2}",
            self.position.x,
            self.position.y,
            self.position.z,
            self.yaw.to_degrees(),
            self.pitch.to_degrees()
        )
    }

    pub fn forward(&self) -> Vec3 {
        Vec3::new(
            self.pitch.cos() * self.yaw.cos(),
            self.pitch.cos() * self.yaw.sin(),
            self.pitch.sin(),
        )
    }

    pub fn view(&self) -> Mat4 {
        Mat4::look_to_rh(self.position, self.forward(), Vec3::Z)
    }

    pub fn projection(&self, aspect: f32) -> Mat4 {
        Mat4::perspective_rh(self.fov_y, aspect.max(1e-3), self.near, self.far)
    }

    /// Right-handed, Z up; depth 0..1 as wgpu expects. Provisional: handedness has not been
    /// checked against the original yet.
    pub fn view_proj(&self, aspect: f32) -> Mat4 {
        self.projection(aspect) * self.view()
    }

    /// Moves relative to the view: `forward` along the view direction, `right` sideways,
    /// `up` along world Z.
    pub fn fly(&mut self, forward: f32, right: f32, up: f32) {
        let f = self.forward();
        let r = f.cross(Vec3::Z).normalize_or_zero();
        self.position += f * forward + r * right + Vec3::Z * up;
    }

    pub fn look(&mut self, d_yaw: f32, d_pitch: f32) {
        self.yaw = (self.yaw + d_yaw).rem_euclid(std::f32::consts::TAU);
        self.pitch = (self.pitch + d_pitch).clamp(-1.55, 1.55);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_prints_specs() {
        let c = Camera::parse("1, 2, 3, 90, -10").unwrap();
        assert!((c.forward() - Vec3::new(0.0, 0.985, -0.174)).length() < 1e-3);
        assert_eq!(Camera::parse(&c.spec()).unwrap().spec(), c.spec());
        assert!(Camera::parse("1,2,3").is_none());
        assert!(Camera::parse("1,2,3,x,0").is_none());
    }

    #[test]
    fn projects_points_in_front() {
        let c = Camera::new(Vec3::ZERO, 0.0, 0.0);
        let p = c.view_proj(1.0).project_point3(Vec3::new(10.0, 0.0, 0.0));
        assert!(p.x.abs() < 1e-5 && p.y.abs() < 1e-5 && (0.0..1.0).contains(&p.z));
        // +Y is to the left when looking along +X with Z up (right-handed).
        let left = c.view_proj(1.0).project_point3(Vec3::new(10.0, 1.0, 0.0));
        assert!(left.x < 0.0);
    }
}
