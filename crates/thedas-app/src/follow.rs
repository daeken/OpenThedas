//! The game camera: it follows the party leader from behind and above, orbiting with the
//! right mouse button and zooming with the wheel. A wall or anything solid between it and
//! the leader brings it in to the near side, quickly; once clear it backs out again, slowly.
//! Provisional: distances and speeds are chosen to look right, not read from the game.

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
/// How far short of a surface in the way the camera stops (metres): its near plane stays
/// clear of the surface.
pub const CLEARANCE: f32 = 0.4;
/// The closest the camera comes to the look point when something is in the way (metres).
pub const MIN_CLEAR: f32 = 0.5;
/// How fast the camera comes in when something is in the way, and goes back out when it
/// clears (per second).
pub const PULL_IN: f32 = 25.0;
pub const LET_OUT: f32 = 3.0;

/// What a segment meets first: the share of the way from its start, as
/// [`thedas_render::collide::Collider::first_hit`] says.
pub type FirstHit<'a> = &'a dyn Fn(Vec3, Vec3) -> Option<f32>;

/// The distance shown after `dt` seconds, going from `shown` toward `clear` (quickly when
/// coming in, slowly going out).
pub fn settle_distance(shown: f32, clear: f32, dt: f32) -> f32 {
    let rate = if clear < shown { PULL_IN } else { LET_OUT };
    shown + (clear - shown) * (1.0 - (-rate * dt.max(0.0)).exp())
}

#[derive(Clone, Debug, PartialEq)]
pub struct Follow {
    /// The direction the camera looks, radians anticlockwise from +X.
    pub yaw: f32,
    pub pitch: f32,
    /// From the look point, metres.
    pub distance: f32,
    /// The look point the camera has caught up to.
    focus: Option<Vec3>,
    /// How far from the look point it stands now (short of `distance` while something is
    /// in the way).
    shown: Option<f32>,
}

impl Follow {
    /// Behind a leader looking along `heading` (radians from +X).
    pub fn behind(heading: f32) -> Self {
        Self {
            yaw: heading,
            pitch: -0.45,
            distance: 7.0,
            focus: None,
            shown: None,
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
    /// last, with nothing in the way.
    #[cfg(test)]
    pub fn camera(&mut self, feet: Vec3, dt: f32, base: &Camera) -> Camera {
        self.camera_clear(feet, dt, base, &|_, _| None)
    }

    /// [`Follow::camera`] kept on the leader's side of whatever `hit` finds between them.
    pub fn camera_clear(&mut self, feet: Vec3, dt: f32, base: &Camera, hit: FirstHit) -> Camera {
        let target = feet + Vec3::Z * LOOK_HEIGHT;
        let cut = !matches!(self.focus, Some(f) if f.distance(target) < 10.0);
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
        let reach = self.distance + CLEARANCE;
        let clear = match hit(focus, focus - c.forward() * reach) {
            Some(t) => (t * reach - CLEARANCE).max(MIN_CLEAR),
            None => self.distance,
        };
        // After a cut (a transition), or with no time to glide in, it stands where it can.
        let shown = match self.shown {
            Some(s) if !cut && dt > 0.0 => settle_distance(s, clear, dt),
            _ => clear,
        };
        self.shown = Some(shown);
        c.position = focus - c.forward() * shown;
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
    fn comes_in_before_a_wall_and_backs_out_slowly() {
        let mut f = Follow::behind(0.0);
        let base = Camera::new(Vec3::ZERO, 0.0, 0.0);
        // A wall 3 m behind the look point (the camera wants 7 m): anything past it is hit.
        let wall = |a: Vec3, b: Vec3| {
            let (da, db) = (
                (a - Vec3::new(0.0, 0.0, LOOK_HEIGHT)).length(),
                (b - a).length(),
            );
            (db > 3.0 - da).then(|| (3.0 - da) / db)
        };
        let dist = |c: &Camera| (c.position - Vec3::new(0.0, 0.0, LOOK_HEIGHT)).length();
        let c = f.camera_clear(Vec3::ZERO, 0.0, &base, &wall);
        assert!((dist(&c) - (3.0 - CLEARANCE)).abs() < 1e-3, "{}", dist(&c));
        // Clear again, it backs out over time rather than at once.
        let c1 = f.camera_clear(Vec3::ZERO, 0.1, &base, &|_, _| None);
        assert!(
            dist(&c1) > 3.0 - CLEARANCE && dist(&c1) < 4.0,
            "{}",
            dist(&c1)
        );
        for _ in 0..100 {
            f.camera_clear(Vec3::ZERO, 0.1, &base, &|_, _| None);
        }
        let c2 = f.camera_clear(Vec3::ZERO, 0.1, &base, &|_, _| None);
        assert!((dist(&c2) - 7.0).abs() < 1e-2);
        // Into the wall again: quickly in.
        let c3 = f.camera_clear(Vec3::ZERO, 0.2, &base, &wall);
        assert!(dist(&c3) < 3.0, "{}", dist(&c3));
        // Never nearer than the minimum, however close the wall.
        let c4 = f.camera_clear(Vec3::ZERO, 0.0, &base, &|_, _| Some(0.0));
        assert!((dist(&c4) - MIN_CLEAR).abs() < 1e-3);
        assert!((settle_distance(2.0, 6.0, 0.0) - 2.0).abs() < 1e-6);
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
