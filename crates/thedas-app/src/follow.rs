//! The game camera: it follows the party leader from behind, orbiting with the right mouse
//! button and zooming with the wheel, as DAOrigins.exe's explore camera does
//! (`CameraControllerExplore::Update` 0x7714b0 sets up the shot; the game camera's update
//! 0x6c4c60 places it).
//!
//! The shot is a point on a sphere round the look point: `distance` from it, at a yaw and a
//! pitch. Zoom runs from -1 (far, 20 m) to 1 (near, 3 m); the game starts at the near end.
//! In the near half (zoom >= 0.5) the camera sweeps a 0.25 m sphere from the look point to
//! where it wants to be; whatever it meets pulls it in towards the look point at once, or at
//! 15 m/s while the line behind the new spot is clear. With nothing in the way it eases
//! back out, speeding up as it goes and holding wherever easing out would touch something.

use glam::Vec3;
use thedas_render::Camera;

/// The zoom's range (`CameraControllerBase` +0x64 and +0x68, set in its constructor
/// 0x7ee9b0 at 0x7eea24 and 0x7eea2d): -1 is the far end, 1 the near.
pub const ZOOM_MIN: f32 = -1.0;
pub const ZOOM_MAX: f32 = 1.0;
/// The zoom a new explore camera starts at: the near end (`CameraControllerExplore`'s
/// constructor 0x76f890 at 0x76f9f3).
pub const ZOOM_START: f32 = 1.0;
/// Zoom per wheel notch. Provisional: the exe scales its zoom input by an option times
/// 0.05 (0x7714c4, 0x7edbe7); the option's value and the wheel's input size are not traced.
pub const ZOOM_STEP: f32 = 0.1;
/// How fast the zoom eases to a script's target (`SetZoomLevel`), in zoom units a second.
/// Provisional: the exe's update steps the zoom (+0x48) towards the target (+0x50; -9999
/// is none) at 0x7eec23-0x7eec99 by an amount its zoom input works out (0x7edc60), which
/// is not traced.
pub const ZOOM_EASE: f32 = 2.0;
/// Where the zoom splits the near half (sweep collision, free pitch) from the far half
/// (0xc0dffc; 0x76f0b0, 0x76f1b0, 0x76f640).
pub const ZOOM_SPLIT: f32 = 0.5;
/// The camera's distance from the look point at the far and near ends of the zoom (metres):
/// `CameraControllerExplore::Update` sets them every frame (0x7714d0, 0x7714d9) and the
/// game camera takes `far + (near - far) * (zoom + 1) / 2` (0x6c2360, 0x7ee770).
pub const FAR_DISTANCE: f32 = 20.0;
pub const NEAR_DISTANCE: f32 = 3.0;
/// The camera's angle down from straight overhead at the near end, as it starts (degrees;
/// 85 is 5 degrees above the look point): `+0x94` set at 0x76f9f8 and applied on
/// activation (0x770ffd -> 0x76f550, which takes `45 + (85 - 45) * (zoom + 1) / 2`).
pub const NEAR_POLAR_DEG: f32 = 85.0;
/// The angle the far half of the zoom brings the camera to at the far end (0xc0e014; 0x76f221).
pub const FAR_POLAR_DEG: f32 = 45.0;
/// How far the player can turn the camera over or under the look point in the near half
/// (degrees from overhead; 0x76f1d7 sets the game camera's limits from 0xc0e050, 0xc0e054).
pub const MIN_POLAR_DEG: f32 = 30.0;
pub const MAX_POLAR_DEG: f32 = 150.0;
/// Pitch limits (radians; negative looks down): [`MIN_POLAR_DEG`] and [`MAX_POLAR_DEG`].
pub const MIN_PITCH: f32 = (MIN_POLAR_DEG - 90.0) * std::f32::consts::PI / 180.0;
pub const MAX_PITCH: f32 = (MAX_POLAR_DEG - 90.0) * std::f32::consts::PI / 180.0;
/// The look point's height above the leader's feet when its appearance gives none
/// (`CAppearance`'s default height, 0x748a51), and the height the far half of the zoom
/// brings it to (0xc0e028; 0x76f6c6). The leader's own is `apr_base.gda`'s `HEIGHT`
/// (`HEIGHT_F` for a female; 0x748a40), which the creature hands the camera (0x6a1a6b).
pub const DEFAULT_LOOK_HEIGHT: f32 = 1.8;
/// The look height follows a change in the leader's at `difference / 0.15` metres a
/// second (0xc11c30; 0x7eea70).
pub const LOOK_HEIGHT_EASE: f32 = 0.15;
/// The radius of the sphere swept from the look point to the camera (metres; 0xaff3e8, at
/// 0x6c3750 and 0x6c368b).
pub const SWEEP_RADIUS: f32 = 0.25;
/// The nearest the camera is pulled to the look point (metres; 0xae7fe8, at 0x6c336c).
pub const PIVOT_CLEARANCE: f32 = 0.25;
/// How fast the camera comes in when the line behind it is clear (m/s; 0xc038e4, 0x6c34f4).
pub const PULL_IN_SPEED: f32 = 15.0;
/// Easing out: the fastest it goes (m/s; 0xc038e0, 0x6c35ce), and how fast it speeds up
/// (m/s²; 0xc038d8, 0x6c36cf), thirty times faster in the far half (0xc038dc, 0x6c36db).
pub const EASE_OUT_MAX: f32 = 15.0;
pub const EASE_OUT_ACCEL: f32 = 10.0;
pub const EASE_OUT_FAST: f32 = 30.0;
/// Differences the camera ignores (0xb107fc).
const EPSILON: f32 = 1e-6;
/// A look point jump at least this long (metres) is a cut (a transition, a teleport): the
/// camera stands where it can at once. Provisional.
const CUT: f32 = 10.0;

/// What a sphere of the given radius (0: a segment) meets first moving from the first point
/// to the second: the share of the way its centre has come, as
/// [`thedas_render::collide::Collider::sweep`] says.
pub type Sweep<'a> = &'a dyn Fn(Vec3, Vec3, f32) -> Option<f32>;

/// How the camera keeps clear of what is between it and the leader, by zoom (0x76f0b0).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Collision {
    /// Near half: sweep, pull in, ease out holding on what easing out would touch.
    Sweep,
    /// Far half: no sweep; whatever pull is left eases out fast.
    Free,
    /// The far end. Provisional: the exe only checks the camera is not inside something
    /// (0x6c3180), which is not traced.
    Overhead,
}

impl Collision {
    fn of(zoom: f32) -> Self {
        if zoom <= ZOOM_MIN {
            Self::Overhead
        } else if zoom >= ZOOM_SPLIT {
            Self::Sweep
        } else {
            Self::Free
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Follow {
    /// The direction the camera looks, radians anticlockwise from +X.
    pub yaw: f32,
    /// The pitch the player has set (radians; negative looks down). The far half of the
    /// zoom blends it towards [`FAR_POLAR_DEG`].
    pub pitch: f32,
    /// From [`ZOOM_MIN`] (far) to [`ZOOM_MAX`] (near).
    pub zoom: f32,
    /// The look point last frame.
    pivot: Option<Vec3>,
    /// The look point's height above the leader's feet, as it has caught up.
    height: Option<f32>,
    /// How far the camera is pulled in from where it wants to be, towards the look point
    /// (metres; the game camera's +0x34).
    pull: f32,
    /// How fast it is easing back out (m/s; +0x38).
    ease: f32,
    /// Where it stood last frame (+0x70).
    eye: Option<Vec3>,
    /// A script's zoom target (+0x50), eased to by the update; none once reached.
    zoom_target: Option<f32>,
}

impl Follow {
    /// Behind a leader looking along `heading` (radians from +X), as the explore camera
    /// starts: at [`ZOOM_START`], [`NEAR_POLAR_DEG`] from overhead.
    pub fn behind(heading: f32) -> Self {
        let mut f = Self {
            yaw: heading,
            pitch: 0.0,
            zoom: ZOOM_START,
            pivot: None,
            height: None,
            pull: 0.0,
            ease: 0.0,
            eye: None,
            zoom_target: None,
        };
        let t = f.zoom_t();
        f.pitch = (FAR_POLAR_DEG + (NEAR_POLAR_DEG - FAR_POLAR_DEG) * t - 90.0).to_radians();
        f
    }

    /// The explore camera's activation (0x770ff0), run after an area has loaded, a cutscene
    /// has ended or a movie has played (0x6949c0's callers 0x6c8fa7, 0x69269d, 0x6dbe46;
    /// conversations do not reset it): the camera turns behind the leader (`heading`, its
    /// facing's direction; 0x7ee880), takes the starting pitch blended by zoom (0x76f550),
    /// and drops its pull-in and ease-out (0x6c2890 -> 0x6c2350). Zoom is kept: the one
    /// controller lives for the whole process (built at 0x641918).
    pub fn reset(&mut self, heading: f32) {
        let zoom = self.zoom;
        *self = Self::behind(heading);
        self.zoom = zoom;
        let t = self.zoom_t();
        self.pitch = (FAR_POLAR_DEG + (NEAR_POLAR_DEG - FAR_POLAR_DEG) * t - 90.0).to_radians();
    }

    pub fn orbit(&mut self, d_yaw: f32, d_pitch: f32) {
        self.yaw += d_yaw;
        self.pitch = (self.pitch + d_pitch).clamp(MIN_PITCH, MAX_PITCH);
    }

    /// The player zooms in for positive `steps` (wheel notches, keys, the pad), out for
    /// negative. True when this brings the zoom to the near end from below: the exe's zoom
    /// setter sends the training event for it then (0x7ee680 sends 1065 at 0x7ee72e, only
    /// from the player's zoom input, 0x76faed / 0x76fb51). A script's target is dropped.
    pub fn zoom(&mut self, steps: f32) -> bool {
        if !steps.is_finite() {
            return false;
        }
        let before = self.zoom;
        self.zoom = (self.zoom + steps * ZOOM_STEP).clamp(ZOOM_MIN, ZOOM_MAX);
        self.zoom_target = None;
        before < ZOOM_MAX && self.zoom >= ZOOM_MAX
    }

    /// A script's zoom request (`SetZoomLevel`, 0x666790): `level` 0 is fully out, 1 fully
    /// in; the target is 2 * level - 1, which the update eases the zoom to. It never counts
    /// as the player reaching the near end.
    pub fn set_zoom_level(&mut self, level: f32) {
        if level.is_finite() {
            self.zoom_target = Some((2.0 * level.clamp(0.0, 1.0) - 1.0).clamp(ZOOM_MIN, ZOOM_MAX));
        }
    }

    /// Eases the zoom towards a script's target, `dt` seconds on (at once when `dt` is 0).
    fn ease_zoom(&mut self, dt: f32) {
        let Some(target) = self.zoom_target else {
            return;
        };
        let step = if dt > 0.0 {
            ZOOM_EASE * dt
        } else {
            f32::INFINITY
        };
        let gap = target - self.zoom;
        if gap.abs() <= step {
            self.zoom = target;
            self.zoom_target = None;
        } else {
            self.zoom += step.copysign(gap);
        }
    }

    /// The zoom as a share of the way from the far end to the near (0x7ee770).
    fn zoom_t(&self) -> f32 {
        ((self.zoom - ZOOM_MIN) / (ZOOM_MAX - ZOOM_MIN)).clamp(0.0, 1.0)
    }

    /// How far into the far half the zoom is: 0 at [`ZOOM_SPLIT`] and above, 1 at the far
    /// end (0x76eab0 with its argument set).
    fn far_t(&self) -> f32 {
        if self.zoom >= ZOOM_SPLIT {
            0.0
        } else {
            (1.0 - (self.zoom - ZOOM_MIN) / (ZOOM_SPLIT - ZOOM_MIN)).clamp(0.0, 1.0)
        }
    }

    /// From the look point to where the camera wants to be (metres).
    pub fn distance(&self) -> f32 {
        FAR_DISTANCE + (NEAR_DISTANCE - FAR_DISTANCE) * self.zoom_t()
    }

    /// The pitch the camera takes: the player's, blended towards [`FAR_POLAR_DEG`] in the
    /// far half (0x76f221). Provisional: the exe blends from the pitch it had on entering
    /// the far half (+0x98, 0x76fc41) rather than the player's current one.
    fn shot_pitch(&self) -> f32 {
        let far = (FAR_POLAR_DEG - 90.0).to_radians();
        self.pitch + (far - self.pitch) * self.far_t()
    }

    /// The camera this frame, following a leader of the default height standing at `feet`,
    /// `dt` seconds after the last, with nothing in the way.
    #[cfg(test)]
    pub fn camera(&mut self, feet: Vec3, dt: f32, base: &Camera) -> Camera {
        self.camera_clear(feet, DEFAULT_LOOK_HEIGHT, dt, base, &|_, _, _| None)
    }

    /// The camera this frame, following a leader standing at `feet` whose appearance's
    /// height is `height` (see [`DEFAULT_LOOK_HEIGHT`]), `dt` seconds after the last, kept on
    /// the leader's side of what `sweep` meets between them. With no time passed (a still
    /// capture) or after a cut, it stands where it would settle at once.
    pub fn camera_clear(
        &mut self,
        feet: Vec3,
        height: f32,
        dt: f32,
        base: &Camera,
        sweep: Sweep,
    ) -> Camera {
        let dt = if dt.is_finite() { dt.max(0.0) } else { 0.0 };
        let height = if height.is_finite() {
            height
        } else {
            DEFAULT_LOOK_HEIGHT
        };
        // The far half brings the look point to the default height (0x76f6c6).
        let height = height + (DEFAULT_LOOK_HEIGHT - height) * self.far_t();
        let h = match self.height {
            Some(h) if dt > 0.0 => {
                let gap = height - h;
                h + gap.signum() * (gap.abs() * dt / LOOK_HEIGHT_EASE).min(gap.abs())
            }
            _ => height,
        };
        self.height = Some(h);
        let pivot = feet + Vec3::Z * h;
        let cut = !matches!(self.pivot, Some(p) if p.distance(pivot) < CUT);
        self.pivot = Some(pivot);
        if cut {
            self.pull = 0.0;
            self.ease = 0.0;
            self.eye = None;
        }
        let settle = cut || dt <= 0.0;
        self.ease_zoom(dt);

        let mut c = Camera::new(pivot, self.yaw, self.shot_pitch());
        c.fov_y = base.fov_y;
        c.near = base.near;
        c.far = base.far;
        let toward = c.forward();
        let wanted = pivot - toward * self.distance();
        let mode = Collision::of(self.zoom);
        if mode == Collision::Sweep
            && let Some(t) = sweep(pivot, wanted, SWEEP_RADIUS)
        {
            let centre = pivot + (wanted - pivot) * t.clamp(0.0, 1.0);
            let need = centre
                .distance(wanted)
                .min(pivot.distance(wanted) - PIVOT_CLEARANCE)
                .max(0.0);
            self.pull_in(need, wanted, toward, pivot, dt, settle, sweep);
        }
        let eye = wanted + toward * self.pull;
        self.eye = Some(eye);
        // Easing out moves the camera next frame (0x6c4e7c runs after 0x6c4e6a).
        self.ease_out(mode, wanted, toward, eye, dt, sweep);
        c.position = eye;
        c
    }

    /// Brings the camera in to `need` metres short of `wanted` (towards the look point
    /// along `toward`) when that spot is in front of where it stands (0x6c32c0): at once
    /// when it is already as far in or `settle` is set, else at [`PULL_IN_SPEED`] while the
    /// line from the new spot to the look point is clear, at once when it is not.
    #[allow(clippy::too_many_arguments)]
    fn pull_in(
        &mut self,
        need: f32,
        wanted: Vec3,
        toward: Vec3,
        pivot: Vec3,
        dt: f32,
        settle: bool,
        sweep: Sweep,
    ) {
        // Provisional: the exe tests against the game camera's own facing (+0x90); this
        // frame's is used.
        if let Some(eye) = self.eye {
            let step = wanted + toward * need - eye;
            if step.length_squared() > EPSILON && step.normalize().dot(toward) < 0.0 {
                return;
            }
        }
        if (need - self.pull).abs() <= EPSILON || self.pull >= need || settle {
            self.pull = need;
        } else {
            let closer = (self.pull + PULL_IN_SPEED * dt).min(need);
            self.pull = if sweep(wanted + toward * closer, pivot, 0.0).is_none() {
                closer
            } else {
                need
            };
        }
        self.ease = 0.0;
    }

    /// Lets the camera back out towards where it wants to be (0x6c35a0), speeding up as it
    /// goes; in the near half it holds (and stops speeding) where the move from `eye` would
    /// touch something.
    fn ease_out(
        &mut self,
        mode: Collision,
        wanted: Vec3,
        toward: Vec3,
        eye: Vec3,
        dt: f32,
        sweep: Sweep,
    ) {
        if self.pull <= EPSILON {
            self.pull = 0.0;
            self.ease = 0.0;
            return;
        }
        let pull = (self.pull - self.ease.min(EASE_OUT_MAX) * dt).max(0.0);
        if mode == Collision::Sweep
            && (pull - self.pull).abs() > EPSILON
            && sweep(eye, wanted + toward * pull, SWEEP_RADIUS).is_some()
        {
            self.ease = 0.0;
            return;
        }
        self.pull = pull;
        let accel = match mode {
            Collision::Free => EASE_OUT_ACCEL * EASE_OUT_FAST,
            _ => EASE_OUT_ACCEL,
        };
        self.ease += accel * dt;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FEET: Vec3 = Vec3::ZERO;

    fn look() -> Vec3 {
        FEET + Vec3::Z * DEFAULT_LOOK_HEIGHT
    }

    fn base() -> Camera {
        Camera::new(Vec3::ZERO, 0.0, 0.0)
    }

    /// A wall `at` metres behind the look point (the plane x = -at; the camera looks along
    /// +X) as a sweep of any radius meets it.
    fn wall(at: f32) -> impl Fn(Vec3, Vec3, f32) -> Option<f32> {
        move |a: Vec3, b: Vec3, r: f32| {
            let touch = -at + r;
            if a.x <= touch {
                Some(0.0)
            } else {
                (b.x < touch).then(|| (a.x - touch) / (a.x - b.x))
            }
        }
    }

    fn dist(c: &Camera) -> f32 {
        (c.position - look()).length()
    }

    #[test]
    fn a_reset_turns_behind_the_leader_and_keeps_the_zoom() {
        let mut f = Follow::behind(0.0);
        f.zoom(-3.0);
        f.orbit(1.0, 0.3);
        f.pull = 1.5;
        let zoom = f.zoom;
        f.reset(2.0);
        assert_eq!(f.yaw, 2.0);
        assert_eq!(f.zoom, zoom);
        assert_eq!(f.pull, 0.0);
        let mut fresh = Follow::behind(2.0);
        fresh.zoom = zoom;
        let t = fresh.zoom_t();
        let pitch = (FAR_POLAR_DEG + (NEAR_POLAR_DEG - FAR_POLAR_DEG) * t - 90.0).to_radians();
        assert!((f.pitch - pitch).abs() < 1e-6);
    }

    #[test]
    fn starts_near_and_low_behind_the_leader() {
        let mut f = Follow::behind(0.0);
        assert_eq!(f.zoom, ZOOM_START);
        assert!((f.distance() - NEAR_DISTANCE).abs() < 1e-5);
        assert!((f.pitch.to_degrees() + 5.0).abs() < 1e-4, "{}", f.pitch);
        let c = f.camera(Vec3::new(10.0, 0.0, 0.0), 0.016, &base());
        let look = Vec3::new(10.0, 0.0, DEFAULT_LOOK_HEIGHT);
        // Behind (smaller x), a little above, 3 m off, looking at the look point.
        assert!(c.position.x < 10.0 && c.position.z > DEFAULT_LOOK_HEIGHT);
        assert!(((c.position - look).length() - 3.0).abs() < 1e-4);
        assert!((c.position + c.forward() * 3.0 - look).length() < 1e-4);
    }

    #[test]
    fn zoom_spans_the_near_and_far_distances() {
        let mut f = Follow::behind(0.0);
        f.zoom(-100.0);
        assert_eq!(f.zoom, ZOOM_MIN);
        assert!((f.distance() - FAR_DISTANCE).abs() < 1e-5);
        // The far end looks down at 45 degrees and from the default height.
        assert!((f.shot_pitch().to_degrees() + 45.0).abs() < 1e-3);
        f.zoom(100.0);
        assert_eq!(f.zoom, ZOOM_MAX);
        f.zoom = 0.0;
        assert!((f.distance() - 11.5).abs() < 1e-5);
        f.zoom(f32::NAN);
        assert_eq!(f.zoom, 0.0);
    }

    #[test]
    fn looks_at_the_leaders_height_and_follows_changes_to_it() {
        let mut f = Follow::behind(0.0);
        let c = f.camera_clear(FEET, 1.43, 0.0, &base(), &|_, _, _| None);
        let at = c.position + c.forward() * f.distance();
        assert!((at.z - 1.43).abs() < 1e-4, "{at}");
        // A change eases over about 0.15 s.
        let c = f.camera_clear(FEET, 1.73, 0.05, &base(), &|_, _, _| None);
        let at = c.position + c.forward() * f.distance();
        assert!((at.z - 1.53).abs() < 1e-3, "{at}");
    }

    #[test]
    fn a_wall_brings_it_in_at_once_and_it_eases_back_out() {
        let mut f = Follow::behind(0.0);
        // The wall 2 m behind the look point (the camera wants 3 m): the sphere's centre
        // stops 0.25 m short of it.
        let w = wall(2.0);
        let c = f.camera_clear(FEET, DEFAULT_LOOK_HEIGHT, 0.0, &base(), &w);
        let want = 1.75 / (-5f32).to_radians().cos();
        assert!((dist(&c) - want).abs() < 1e-3, "{} {want}", dist(&c));
        // Held there while the wall stands.
        let c = f.camera_clear(FEET, DEFAULT_LOOK_HEIGHT, 0.1, &base(), &w);
        assert!((dist(&c) - want).abs() < 1e-3, "{}", dist(&c));
        // Clear: it eases out, slowly at first (10 m/s², at most 15 m/s).
        let mut last = dist(&c);
        let mut frames = 0;
        while last < NEAR_DISTANCE - 1e-4 && frames < 100 {
            let c = f.camera_clear(FEET, DEFAULT_LOOK_HEIGHT, 0.05, &base(), &|_, _, _| None);
            assert!(dist(&c) >= last - 1e-5);
            last = dist(&c);
            frames += 1;
        }
        // 1.25 m at 10 m/s² from rest: about half a second.
        assert!((8..=14).contains(&frames), "{frames}");
        assert!((last - NEAR_DISTANCE).abs() < 1e-4);
    }

    #[test]
    fn it_comes_in_at_pull_in_speed_while_the_line_behind_is_clear() {
        let mut f = Follow::behind(0.0);
        f.camera_clear(FEET, DEFAULT_LOOK_HEIGHT, 0.0, &base(), &|_, _, _| None);
        // A post the sphere grazes but a line misses (a sweep hit, a clear ray): it comes in
        // at 15 m/s, 0.15 m in 0.01 s.
        let post = |a: Vec3, b: Vec3, r: f32| {
            (r > 0.0 && a.x > b.x && b.x < -2.0 && a.x > -2.0).then(|| (a.x + 2.0) / (a.x - b.x))
        };
        let c = f.camera_clear(FEET, DEFAULT_LOOK_HEIGHT, 0.01, &base(), &post);
        assert!(
            (NEAR_DISTANCE - dist(&c) - 0.15).abs() < 1e-3,
            "{}",
            dist(&c)
        );
        // Something on the line too: it is at the sphere's stop at once.
        let mut g = Follow::behind(0.0);
        g.camera_clear(FEET, DEFAULT_LOOK_HEIGHT, 0.0, &base(), &|_, _, _| None);
        let c = g.camera_clear(FEET, DEFAULT_LOOK_HEIGHT, 0.01, &base(), &wall(2.0));
        let want = 1.75 / (-5f32).to_radians().cos();
        assert!((dist(&c) - want).abs() < 1e-3, "{}", dist(&c));
    }

    #[test]
    fn never_nearer_than_the_clearance_and_no_sweep_in_the_far_half() {
        let mut f = Follow::behind(0.0);
        let c = f.camera_clear(FEET, DEFAULT_LOOK_HEIGHT, 0.0, &base(), &|_, _, _| {
            Some(0.0)
        });
        assert!((dist(&c) - PIVOT_CLEARANCE).abs() < 1e-4, "{}", dist(&c));
        // Zoomed out past the split, nothing pulls it in.
        let mut g = Follow::behind(0.0);
        g.zoom = 0.0;
        let c = g.camera_clear(FEET, DEFAULT_LOOK_HEIGHT, 0.0, &base(), &|_, _, _| {
            Some(0.0)
        });
        assert!((dist(&c) - g.distance()).abs() < 1e-3, "{}", dist(&c));
    }

    #[test]
    fn a_big_jump_cuts() {
        let mut f = Follow::behind(0.0);
        f.camera_clear(FEET, DEFAULT_LOOK_HEIGHT, 0.0, &base(), &|_, _, _| {
            Some(0.0)
        });
        let far = Vec3::new(100.0, 0.0, 0.0);
        let c = f.camera_clear(far, DEFAULT_LOOK_HEIGHT, 0.016, &base(), &|_, _, _| None);
        let look = far + Vec3::Z * DEFAULT_LOOK_HEIGHT;
        assert!(((c.position - look).length() - NEAR_DISTANCE).abs() < 1e-4);
    }

    #[test]
    fn orbit_stays_in_bounds() {
        let mut f = Follow::behind(0.0);
        f.orbit(0.5, -10.0);
        assert_eq!(f.pitch, MIN_PITCH);
        assert!((MIN_PITCH.to_degrees() + 60.0).abs() < 1e-4);
        f.orbit(0.0, 10.0);
        assert_eq!(f.pitch, MAX_PITCH);
    }
}
