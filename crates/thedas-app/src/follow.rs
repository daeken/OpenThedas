//! The game camera: it follows the party leader from behind and above, orbiting with the
//! right mouse button and zooming with the wheel. Provisional: distances and speeds are
//! chosen to look right, not read from the game.

use glam::Vec3;
use thedas_render::Camera;

/// Where the camera looks: this far above the leader's feet (metres).
pub const LOOK_HEIGHT: f32 = 1.5;
pub const MIN_DISTANCE: f32 = 1.5;
pub const MAX_DISTANCE: f32 = 25.0;
/// Pitch limits (radians; negative looks down).
pub const MIN_PITCH: f32 = -1.4;
pub const MAX_PITCH: f32 = 0.3;
/// How fast the camera catches up with the leader (per second; higher is snappier).
pub const CATCH_UP: f32 = 8.0;

#[derive(Clone, Debug, PartialEq)]
pub struct Follow {
    /// The direction the camera looks, radians anticlockwise from +X.
    pub yaw: f32,
    pub pitch: f32,
    /// From the look point, metres.
    pub distance: f32,
    /// The look point the camera has caught up to.
    focus: Option<Vec3>,
}

impl Follow {
    /// Behind a leader looking along `heading` (radians from +X).
    pub fn behind(heading: f32) -> Self {
        Self {
            yaw: heading,
            pitch: -0.45,
            distance: 7.0,
            focus: None,
        }
    }

    pub fn orbit(&mut self, d_yaw: f32, d_pitch: f32) {
        self.yaw += d_yaw;
        self.pitch = (self.pitch + d_pitch).clamp(MIN_PITCH, MAX_PITCH);
    }

    /// Zooms in for positive `steps`, out for negative.
    pub fn zoom(&mut self, steps: f32) {
        self.distance = (self.distance * 0.88f32.powf(steps)).clamp(MIN_DISTANCE, MAX_DISTANCE);
    }

    /// The camera this frame, following a leader standing at `feet`, `dt` seconds after the
    /// last.
    pub fn camera(&mut self, feet: Vec3, dt: f32, base: &Camera) -> Camera {
        let target = feet + Vec3::Z * LOOK_HEIGHT;
        let focus = match self.focus {
            // A long jump (a transition, a teleport) cuts rather than glides.
            Some(f) if f.distance(target) < 10.0 => {
                f.lerp(target, (CATCH_UP * dt.max(0.0)).min(1.0))
            }
            _ => target,
        };
        self.focus = Some(focus);
        let mut c = Camera::new(focus, self.yaw, self.pitch);
        c.fov_y = base.fov_y;
        c.near = base.near;
        c.far = base.far;
        c.position = focus - c.forward() * self.distance;
        c
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sits_behind_and_above_the_leader_looking_at_it() {
        let mut f = Follow::behind(0.0);
        let base = Camera::new(Vec3::ZERO, 0.0, 0.0);
        let c = f.camera(Vec3::new(10.0, 0.0, 0.0), 0.016, &base);
        // Looking along +X from behind (smaller x) and above.
        assert!(c.position.x < 10.0 && c.position.z > LOOK_HEIGHT);
        let look = Vec3::new(10.0, 0.0, LOOK_HEIGHT);
        assert!((c.position + c.forward() * f.distance - look).length() < 1e-4);
    }

    #[test]
    fn glides_after_small_moves_and_cuts_after_big_ones() {
        let mut f = Follow::behind(0.0);
        let base = Camera::new(Vec3::ZERO, 0.0, 0.0);
        f.camera(Vec3::ZERO, 0.016, &base);
        let c1 = f.camera(Vec3::new(1.0, 0.0, 0.0), 0.016, &base);
        let moved = c1.position.x + c1.forward().x * f.distance;
        assert!(moved > 0.0 && moved < 1.0, "{moved}");
        let c2 = f.camera(Vec3::new(100.0, 0.0, 0.0), 0.016, &base);
        assert!((c2.position.x + c2.forward().x * f.distance - 100.0).abs() < 1e-3);
    }

    #[test]
    fn orbit_and_zoom_stay_in_bounds() {
        let mut f = Follow::behind(0.0);
        f.orbit(0.5, -10.0);
        assert_eq!(f.pitch, MIN_PITCH);
        f.zoom(100.0);
        assert_eq!(f.distance, MIN_DISTANCE);
        f.zoom(-100.0);
        assert_eq!(f.distance, MAX_DISTANCE);
    }
}
