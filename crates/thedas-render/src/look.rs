//! Head tracking: a creature turning its body, head and eyes towards what it looks at.
//!
//! The original does not turn bones for this. Each of three parts (eyes, head, body; the
//! rows of `look_at.gda`, table 229) has three pose animations (`ANIM_base` rows named by
//! the table: `lookcenter`, `lookup`, `lookdown` for the head, `lookeyes…` for the eyes,
//! `lookbody…` for the body), each a sweep from the part's furthest turn one way to its
//! furthest the other at one height. A part's turn sets the three clips' time by its yaw
//! and their weights by its pitch, and the animation system lays them over the body's
//! pose (they are additive clips: `mh_o.headmid.ani` turns `head_rotation`,
//! `neck1_rotation` and `neck_rotation`).
//!
//! Traced in DAOrigins.exe (notes: `headtracking-trace.md` beside the test data): the
//! tracker's wanted rotation (0x6ca250), its update over the parts (0x6ca990, 0x6ca150), a
//! part's update (0x7f53e0) and the table's loader (0x7f5c40). Rotations are quaternions
//! multiplied as Hamilton's product (0x4cb640); a creature looks along its +Y with +Z up;
//! a heading turns about Z, a pitch about X after it (0x4ee200).
//!
//! Provisional: the tracker's origin and base rotation (the executable reads a node of the
//! model, 0x6ca450; callers here give the head's place and the creature's facing), and how
//! the clips' weights scale their turn (see [`crate::skeleton::Skeleton`]).

use glam::{Quat, Vec3};

/// One row of `look_at.gda`: a part's animations, range and speeds. Angles in radians.
#[derive(Clone, Debug, PartialEq)]
pub struct PartConfig {
    /// The centre, up and down pose animations' actions (`ANIM_base` names).
    pub actions: [String; 3],
    pub yaw: (f32, f32),
    pub pitch: (f32, f32),
    /// The turn speeds (radians a second) used at or under `slow_angle` from where it is
    /// going and at or over `fast_angle`, in between in proportion.
    pub slow: f32,
    pub fast: f32,
    pub slow_angle: f32,
    pub fast_angle: f32,
    /// The most the part takes of a turn sideways, and of one up or down.
    pub sideways: f32,
    pub updown: f32,
}

impl PartConfig {
    /// From a row's cells in degrees, as the table has them.
    #[allow(clippy::too_many_arguments)]
    pub fn from_degrees(
        actions: [String; 3],
        yaw: (f32, f32),
        pitch: (f32, f32),
        speeds: (f32, f32),
        angles: (f32, f32),
        sideways: f32,
        updown: f32,
    ) -> Self {
        let r = f32::to_radians;
        PartConfig {
            actions,
            yaw: (r(yaw.0), r(yaw.1)),
            pitch: (r(pitch.0), r(pitch.1)),
            slow: r(speeds.0),
            fast: r(speeds.1),
            slow_angle: r(angles.0),
            fast_angle: r(angles.1),
            sideways: r(sideways),
            updown: r(updown),
        }
    }
}

/// What a part's turn asks of its three pose animations.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PartPose {
    /// The weights of the centre, up and down animations.
    pub weights: [f32; 3],
    /// How far through the animations' sweep, 0 to 1: their time is
    /// `phase × (length − 1/30) + 1/30` seconds (0x7f53c0).
    pub phase: f32,
}

/// The heading (about Z, radians) of a direction: 0x4ee570.
fn heading(d: Vec3) -> f32 {
    (-d.x).atan2(d.y)
}

/// The pitch (radians, up positive) of a direction: 0x4ee5a0.
fn pitch(d: Vec3) -> f32 {
    d.z.atan2(d.x.hypot(d.y))
}

/// The rotation of a heading and a pitch (0x4ee200 with no roll).
fn euler(heading: f32, pitch: f32) -> Quat {
    Quat::from_rotation_z(heading) * Quat::from_rotation_x(pitch)
}

/// Where a rotation points what looked along +Y (0x4cbd30).
fn forward(q: Quat) -> Vec3 {
    q * Vec3::Y
}

/// A rotation about `axis` (a zero axis gives a rotation that turns nothing, as 0x4cbc40
/// does with one).
fn about(axis: Vec3, angle: f32) -> Quat {
    let (s, c) = (angle * 0.5).sin_cos();
    Quat::from_xyzw(axis.x * s, axis.y * s, axis.z * s, c)
}

/// One part of a tracker.
#[derive(Clone, Debug)]
struct Part {
    config: PartConfig,
    /// Its centre animation exists for the creature; a part without one stays at rest and
    /// takes nothing of a turn.
    valid: bool,
    rotation: Quat,
    /// The speed of the turn under way (it never slows before arriving), if one is.
    speed: Option<f32>,
    /// The speeds, scaled ([`Tracker::set_speed_scale`]).
    slow: f32,
    fast: f32,
}

impl Part {
    fn new(config: PartConfig, valid: bool) -> Self {
        Part {
            slow: config.slow,
            fast: config.fast,
            config,
            valid,
            rotation: Quat::IDENTITY,
            speed: None,
        }
    }

    /// Turns the part towards `q` for `dt` seconds (0x7f53e0).
    fn update(&mut self, dt: f32, mut q: Quat) {
        if !self.valid {
            self.rotation = Quat::IDENTITY;
            return;
        }
        let c = &self.config;
        // No more of the turn than the part may take: its sideways limit for a turn about
        // the vertical, its up-and-down limit for one about a level axis.
        let axis = Vec3::Y.cross(forward(q)).normalize_or_zero();
        let a = axis.z.abs();
        let limit = a * c.sideways + (1.0 - a) * c.updown;
        if q.w.abs() < (limit * 0.5).cos() {
            let f = forward(about(axis, limit));
            q = euler(heading(f), pitch(f));
        }
        let near = self.rotation.dot(q).abs();
        let (slow_cos, fast_cos) = ((c.slow_angle * 0.5).cos(), (c.fast_angle * 0.5).cos());
        let t = 1.0 - (near - fast_cos) / (slow_cos - fast_cos);
        let t = if t.is_finite() { t.clamp(0.0, 1.0) } else { 1.0 };
        let mut speed = self.slow + (self.fast - self.slow) * t;
        if let Some(last) = self.speed {
            speed = speed.max(last);
        }
        self.speed = Some(speed);
        let step = speed * dt;
        if step > std::f32::consts::FRAC_PI_2 || near >= (step * 0.5).cos() {
            self.rotation = q;
            self.speed = None;
        } else {
            let turn = forward(self.rotation)
                .normalize_or_zero()
                .cross(forward(q).normalize_or_zero())
                .normalize_or_zero();
            self.rotation = (about(turn, step) * self.rotation).normalize();
        }
        let f = forward(self.rotation);
        let yaw = heading(f).clamp(c.yaw.0, c.yaw.1);
        let pitch = pitch(f).clamp(c.pitch.0, c.pitch.1);
        self.rotation = euler(yaw, pitch);
    }

    fn pose(&self) -> Option<PartPose> {
        if !self.valid {
            return None;
        }
        let c = &self.config;
        let f = forward(self.rotation);
        let (yaw, pitch) = (heading(f), pitch(f));
        let (mut up, mut down) = (0.0, 0.0);
        let centre = if pitch > 0.0 {
            up = pitch / c.pitch.1;
            1.0 - up
        } else {
            down = pitch / c.pitch.0;
            1.0 - down
        };
        let phase = (yaw - c.yaw.0) / (c.yaw.1 - c.yaw.0);
        let fin = |v: f32| if v.is_finite() { v } else { 0.0 };
        Some(PartPose {
            weights: [fin(centre), fin(up), fin(down)],
            phase: fin(phase).clamp(0.0, 1.0),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Off,
    Tracking,
    /// Going back to rest; off once there.
    Returning,
}

/// The parts, in the table's order.
pub const EYES: usize = 0;
pub const HEAD: usize = 1;
pub const BODY: usize = 2;

/// A creature's head tracker.
#[derive(Clone, Debug)]
pub struct Tracker {
    parts: [Part; 3],
    /// Which parts take part (all, as the executable starts them).
    pub on: [bool; 3],
    state: State,
    origin: Vec3,
    base: Quat,
    target: Option<Vec3>,
    /// Where it looks with no target (relative to its base).
    explicit: Quat,
}

impl Tracker {
    /// A tracker of the table's three rows (eyes, head, body); `valid[i]` says whether
    /// part `i`'s centre animation exists for the creature.
    pub fn new(config: [PartConfig; 3], valid: [bool; 3]) -> Self {
        let [e, h, b] = config;
        Tracker {
            parts: [
                Part::new(e, valid[EYES]),
                Part::new(h, valid[HEAD]),
                Part::new(b, valid[BODY]),
            ],
            on: [true; 3],
            state: State::Off,
            origin: Vec3::ZERO,
            base: Quat::IDENTITY,
            target: None,
            explicit: Quat::IDENTITY,
        }
    }

    /// Starts tracking (0x6cab00): the parts begin where they are, with no turn under way.
    pub fn start(&mut self) {
        if self.state != State::Tracking {
            self.state = State::Tracking;
            for p in &mut self.parts {
                p.speed = None;
            }
        }
    }

    /// Lets go (0x6c9e60): the parts turn back to rest, and the tracker is off once there.
    pub fn stop(&mut self) {
        if self.state != State::Off {
            self.state = State::Returning;
        }
    }

    /// Whether it is tracking or still returning to rest.
    pub fn active(&self) -> bool {
        self.state != State::Off
    }

    /// Whether it is tracking (not off, not returning).
    pub fn tracking(&self) -> bool {
        self.state == State::Tracking
    }

    /// Scales the parts' speeds from the table's (0x6c9f10): a cutscene's head tracking
    /// action gives its own scale.
    pub fn set_speed_scale(&mut self, scale: f32) {
        for p in &mut self.parts {
            p.slow = p.config.slow * scale;
            p.fast = p.config.fast * scale;
            p.speed = None;
        }
    }

    /// Where the creature looks from, and the rotation its turn is measured against (its
    /// facing).
    pub fn set_origin(&mut self, origin: Vec3, base: Quat) {
        self.origin = origin;
        self.base = base;
    }

    /// The point to look at, or none: it then looks where [`Tracker::set_explicit`] says.
    pub fn set_target(&mut self, target: Option<Vec3>) {
        self.target = target;
    }

    /// Where it looks with no target: a heading and pitch from its base (0x6c9fe0).
    pub fn set_explicit(&mut self, heading: f32, pitch: f32) {
        self.explicit = euler(heading, pitch);
    }

    /// The rotation wanted of the parts together (0x6ca250).
    fn wanted(&self) -> Quat {
        const LIMIT: f32 = std::f32::consts::FRAC_PI_2;
        match (self.state, self.target) {
            (State::Tracking, Some(target)) => {
                let d = (target - self.origin).normalize_or_zero();
                let look = euler(heading(d), pitch(d));
                let f = forward((self.base.conjugate() * look).normalize());
                euler(
                    heading(f).clamp(-LIMIT, LIMIT),
                    pitch(f).clamp(-LIMIT, LIMIT),
                )
            }
            (State::Tracking, None) => self.explicit,
            _ => Quat::IDENTITY,
        }
    }

    /// Advances the parts by `dt` seconds (0x6ca990): the body takes what it may of the
    /// wanted turn, the head what is left, the eyes the rest.
    pub fn update(&mut self, dt: f32) {
        if self.state == State::Off {
            return;
        }
        let mut q = self.wanted();
        for i in [BODY, HEAD, EYES] {
            let part = &mut self.parts[i];
            if self.on[i] {
                part.update(dt, q);
            } else if part.rotation.w.abs() <= 0.9999 {
                part.update(dt, Quat::IDENTITY);
            } else {
                continue;
            }
            q = q * part.rotation.conjugate();
        }
        if self.state == State::Returning
            && self.parts.iter().all(|p| p.rotation.w.abs() > 0.9999)
        {
            self.state = State::Off;
        }
    }

    /// What each part asks of its animations now (eyes, head, body); none for a part
    /// without animations, and none at all while the tracker is off.
    pub fn poses(&self) -> [Option<PartPose>; 3] {
        if self.state == State::Off {
            return [None; 3];
        }
        [
            self.parts[EYES].pose(),
            self.parts[HEAD].pose(),
            self.parts[BODY].pose(),
        ]
    }

    /// A part's animations' actions (centre, up, down).
    pub fn actions(&self, part: usize) -> &[String; 3] {
        &self.parts[part].config.actions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The game's table.
    fn config() -> [PartConfig; 3] {
        let names = |a: &str, b: &str, c: &str| [a.to_string(), b.to_string(), c.to_string()];
        [
            PartConfig::from_degrees(
                names("lookeyescenter", "lookeyesup", "lookeyesdown"),
                (-45.0, 45.0),
                (-45.0, 45.0),
                (360.0, 360.0),
                (10.0, 25.0),
                20.0,
                10.0,
            ),
            PartConfig::from_degrees(
                names("lookcenter", "lookup", "lookdown"),
                (-45.0, 45.0),
                (-45.0, 45.0),
                (20.0, 200.0),
                (20.0, 55.0),
                45.0,
                10.0,
            ),
            PartConfig::from_degrees(
                names("lookbodycenter", "lookbodyup", "lookbodydown"),
                (-45.0, 45.0),
                (-45.0, 45.0),
                (96.0, 96.0),
                (10.0, 25.0),
                45.0,
                30.0,
            ),
        ]
    }

    fn settle(t: &mut Tracker) {
        for _ in 0..600 {
            t.update(1.0 / 30.0);
        }
    }

    #[test]
    fn at_rest_the_centre_clip_plays_at_its_middle() {
        let mut t = Tracker::new(config(), [true, true, false]);
        t.start();
        t.set_target(Some(Vec3::new(0.0, 5.0, 0.0)));
        settle(&mut t);
        let [eyes, head, body] = t.poses();
        assert_eq!(body, None);
        for p in [eyes.unwrap(), head.unwrap()] {
            assert!((p.phase - 0.5).abs() < 1e-4, "{p:?}");
            assert!((p.weights[0] - 1.0).abs() < 1e-4 && p.weights[1].abs() < 1e-4);
        }
    }

    #[test]
    fn the_head_takes_a_sideways_turn_up_to_its_limit_and_the_eyes_the_rest() {
        let mut t = Tracker::new(config(), [true, true, false]);
        t.start();
        // 60 degrees to the left (a positive heading), level.
        let h = 60f32.to_radians();
        t.set_target(Some(Vec3::new(-h.sin(), h.cos(), 0.0) * 4.0));
        settle(&mut t);
        let [eyes, head, _] = t.poses();
        let (eyes, head) = (eyes.unwrap(), head.unwrap());
        // The head's 45 of its -45..45 range is the end of its sweep; the eyes have the
        // other 15 of their own.
        assert!((head.phase - 1.0).abs() < 1e-3, "{head:?}");
        assert!((eyes.phase - (15.0 + 45.0) / 90.0).abs() < 1e-3, "{eyes:?}");
        assert!((head.weights[0] - 1.0).abs() < 1e-3);
    }

    #[test]
    fn looking_up_weights_the_up_clip_by_the_pitch() {
        let mut t = Tracker::new(config(), [true, true, false]);
        t.start();
        // 9 degrees up: within the head's 10 up or down.
        let p = 9f32.to_radians();
        t.set_target(Some(Vec3::new(0.0, p.cos(), p.sin()) * 3.0));
        settle(&mut t);
        let head = t.poses()[HEAD].unwrap();
        assert!((head.weights[1] - 9.0 / 45.0).abs() < 1e-3, "{head:?}");
        assert!((head.weights[0] - (1.0 - 9.0 / 45.0)).abs() < 1e-3);
        assert_eq!(head.weights[2], 0.0);
        assert!((head.phase - 0.5).abs() < 1e-3);
    }

    #[test]
    fn the_head_turns_at_its_speed_and_stopping_returns_it_to_rest() {
        let mut t = Tracker::new(config(), [false, true, false]);
        t.on = [false, true, false];
        t.start();
        let h = 40f32.to_radians();
        t.set_target(Some(Vec3::new(-h.sin(), h.cos(), 0.0)));
        // 40 degrees away is between the slow (20) and fast (55) angles: under 200 degrees
        // a second, over 20, so a thirtieth of a second moves it between 0.67 and 6.7.
        t.update(1.0 / 30.0);
        let first = t.poses()[HEAD].unwrap().phase;
        let turned = (first - 0.5) * 90.0;
        assert!(turned > 0.6 && turned < 6.7, "{turned}");
        settle(&mut t);
        assert!((t.poses()[HEAD].unwrap().phase - (40.0 + 45.0) / 90.0).abs() < 1e-3);
        t.stop();
        assert!(t.active());
        settle(&mut t);
        assert!(!t.active());
        assert_eq!(t.poses(), [None; 3]);
    }

    #[test]
    fn a_turn_is_measured_against_the_base_rotation() {
        let mut t = Tracker::new(config(), [true, true, false]);
        t.start();
        // Facing +X (a heading of -90 degrees); the target is straight ahead of that.
        t.set_origin(
            Vec3::new(1.0, 1.0, 1.7),
            Quat::from_rotation_z(-std::f32::consts::FRAC_PI_2),
        );
        t.set_target(Some(Vec3::new(6.0, 1.0, 1.7)));
        settle(&mut t);
        let head = t.poses()[HEAD].unwrap();
        assert!((head.phase - 0.5).abs() < 1e-3, "{head:?}");
    }
}
