//! What a cutscene does to its actors over time: the curves that move and turn them, the
//! camera actors and which of them is the view. Nothing in the simulation reads these; they
//! are for the presentation, which plays them on the cutscene's clock
//! ([`crate::cutscene::Cutscene::elapsed_ms`]).
//!
//! Traced in the executable (`EC::Cutscene`):
//!
//! - A curve (`CURV`; loaded by 0x7d2350, evaluated by 0x7d2440) has a default value
//!   (field 5350), keys (5351: `VERT`, time 5370 and value 5371, in seconds of the
//!   cutscene) and one transition per key to the next (5352: `TRAN`, kind 5380). A curve
//!   without keys is its default; before its first key it is the first key's value and
//!   after its last the last's. A time within 1e-4 s of a key is that key's value. Between
//!   keys a transition is a Bézier (kind 0) with the absolute (time, value) control points
//!   5381 and 5382, solved for the time by 20 halvings (0x7ccdf0) and then evaluated
//!   (0x7ccef0); linear (1); held (2, the earlier key's value); any other kind gives the
//!   default.
//! - A track (`TRAK` and the `A???` actions) has a kind (field 5300; the factory 0x7ddaa0
//!   switches on it), a start and an end (5301 and 5302) and its curves (5303). Its update
//!   is given the time since its start and evaluates its curves at that plus the start
//!   (0x7d30b0): the curves are keyed in the cutscene's time.
//! - Kind 17, `ActionMoveSpline` (update 0x7d32f0): three curves, x y z; the actor stands at
//!   its base position (field 5214) plus the curves' point turned by its base rotation
//!   (5215, a quaternion).
//! - Kind 18, `ActionRotate` (update 0x7d3460): three curves of degrees; the actor's
//!   rotation is its base rotation times the turn the curves make (0x4ee200): about Z by the
//!   first, then about X by the second, then about Y by the third (Z·X·Y; quaternions
//!   multiply as 0x4cb640 does, Hamilton's product).
//! - Kind 13, `ActionCameraFOV` (update 0x7d3500): one curve, the actor's field of view in
//!   degrees (actor +0x64).
//! - Kind 11, `ActionMakeActiveCamera` (update 0x7d11e0): from its start the actor whose id
//!   (field 5201) is its field 5610 is the cutscene's camera (cutscene +0x1d8). Each tick the
//!   cutscene hands the camera that actor's position, rotation and field of view (0x7dd6b8).
//! - Kind 4, `ActionPlayAnimation` (vtable 0xb23240): the animation (field 5400) names a node
//!   of the cutscene's `.cub` animation tree (5401), whose node names the `.ani` file.
//!
//! An actor's world placement goes through the cutscene's own placement (0x7d0660, 0x7cf6b0:
//! cutscene +0xa0 and +0xb0), which only a cutscene played at another object sets
//! (0x7cdc40); a cutscene loaded by name plays where it was made.
//!
//! Provisional: which way a camera actor looks (taken as its local +Y, as a stage camera's;
//! up is +Z); whether a field of view is vertical; tracks outside their span (a track is
//! taken to apply from its start and to hold its value at its end after it); an actor with
//! no motion track stands at its base; a camera with no field-of-view track sees 45°; a
//! missing transition is linear. Not followed yet: a camera actor that jumps to a camera of
//! the cutscene's stage (`ActionJumpToCamera`, most conversation cutscenes) or is linked to
//! another actor; while one is, [`Tracks::camera_at`] gives no camera.

use thedas_formats::Gff4;
use thedas_formats::gff4::{StructId, Value};

use crate::cutscene::structs;

const FIELD_ACTORS: u32 = 5200;
const FIELD_ACTOR_NAME: u32 = 2;
const FIELD_ACTOR_ID: u32 = 5201;
const FIELD_ACTOR_TRACKS: u32 = 5206;
const FIELD_ACTOR_CREATURE: u32 = 5208;
const FIELD_ACTOR_POSITION: u32 = 5214;
const FIELD_ACTOR_ROTATION: u32 = 5215;
const FIELD_TRACK_KIND: u32 = 5300;
const FIELD_TRACK_START: u32 = 5301;
const FIELD_TRACK_END: u32 = 5302;
const FIELD_TRACK_CURVES: u32 = 5303;
const FIELD_CAMERA_ACTOR: u32 = 5610;
const FIELD_ANIMATION: u32 = 5400;
const FIELD_ANIMATION_TREE: u32 = 5401;
const FIELD_CURVE_DEFAULT: u32 = 5350;
const FIELD_CURVE_KEYS: u32 = 5351;
const FIELD_CURVE_TRANSITIONS: u32 = 5352;
const FIELD_KEY_TIME: u32 = 5370;
const FIELD_KEY_VALUE: u32 = 5371;
const FIELD_TRANSITION_KIND: u32 = 5380;
const FIELD_TRANSITION_OUT: u32 = 5381;
const FIELD_TRANSITION_IN: u32 = 5382;

/// Track kinds (field 5300), as the action factory 0x7ddaa0 makes them.
pub const TRACK_PLAY_ANIMATION: u64 = 4;
pub const TRACK_MAKE_ACTIVE_CAMERA: u64 = 11;
pub const TRACK_CAMERA_FOV: u64 = 13;
pub const TRACK_MOVE_SPLINE: u64 = 17;
pub const TRACK_ROTATE: u64 = 18;
pub const TRACK_LINK_TO_ACTOR: u64 = 27;
pub const TRACK_JUMP_TO_CAMERA: u64 = 24;

/// The field of view of a camera actor without a field-of-view track (degrees).
/// Provisional: the actor's initial +0x64 is not traced.
pub const DEFAULT_FOV: f32 = 45.0;

/// A key is taken as reached within this many seconds of it (0x7d2440's 1e-4).
const KEY_SNAP: f32 = 1e-4;

/// How a curve runs from a key to the next (`TRAN`, field 5380).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Transition {
    /// Kind 0: a cubic Bézier through the keys, with these (time, value) control points.
    Bezier { out: [f32; 2], into: [f32; 2] },
    /// Kind 1.
    Linear,
    /// Kind 2: the earlier key's value until the next key.
    Hold,
    /// Any other kind: the curve's default value.
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Key {
    /// Seconds into the cutscene.
    pub time: f32,
    pub value: f32,
    /// How the curve runs on to the next key.
    pub next: Transition,
}

/// A cutscene curve (`CURV`).
#[derive(Clone, Debug, PartialEq)]
pub struct Curve {
    pub default: f32,
    /// By time, as the file lists them.
    pub keys: Vec<Key>,
}

impl Curve {
    /// A curve that is `value` throughout.
    pub fn constant(value: f32) -> Self {
        Self {
            default: value,
            keys: Vec::new(),
        }
    }

    /// The curve's value `t` seconds into the cutscene (0x7d2440).
    pub fn at(&self, t: f32) -> f32 {
        let (Some(first), Some(last)) = (self.keys.first(), self.keys.last()) else {
            return self.default;
        };
        if t.is_nan() || t <= first.time {
            return first.value;
        }
        if last.time <= t {
            return last.value;
        }
        let mut segment = 0;
        for (i, k) in self.keys.iter().enumerate().skip(1) {
            if k.time <= t - KEY_SNAP {
                continue;
            }
            if t + KEY_SNAP > k.time {
                return k.value;
            }
            if t < k.time {
                segment = i - 1;
                break;
            }
        }
        let (a, b) = (
            self.keys[segment],
            self.keys[(segment + 1).min(self.keys.len() - 1)],
        );
        match a.next {
            Transition::Hold => a.value,
            Transition::Linear => {
                if b.time <= a.time {
                    a.value
                } else {
                    a.value + (b.value - a.value) * ((t - a.time) / (b.time - a.time))
                }
            }
            Transition::Bezier { out, into } => {
                bezier([[a.time, a.value], out, into, [b.time, b.value]], t)
            }
            Transition::Other => self.default,
        }
    }

    fn from_gff4(g: &Gff4, id: StructId) -> Self {
        let default = f32_field(g, id, FIELD_CURVE_DEFAULT).unwrap_or(0.0);
        let transitions = structs(g, id, FIELD_CURVE_TRANSITIONS);
        let point = |s: Option<StructId>| -> [f32; 2] {
            s.map_or([0.0; 2], |s| {
                [
                    f32_field(g, s, FIELD_KEY_TIME).unwrap_or(0.0),
                    f32_field(g, s, FIELD_KEY_VALUE).unwrap_or(0.0),
                ]
            })
        };
        let keys = structs(g, id, FIELD_CURVE_KEYS)
            .into_iter()
            .enumerate()
            .map(|(i, k)| Key {
                time: f32_field(g, k, FIELD_KEY_TIME).unwrap_or(0.0),
                value: f32_field(g, k, FIELD_KEY_VALUE).unwrap_or(0.0),
                next: match transitions.get(i) {
                    None => Transition::Linear,
                    Some(&t) => match g.field(t, FIELD_TRANSITION_KIND).and_then(Value::as_u64) {
                        Some(0) => Transition::Bezier {
                            out: point(g.field(t, FIELD_TRANSITION_OUT).and_then(Value::as_struct)),
                            into: point(g.field(t, FIELD_TRANSITION_IN).and_then(Value::as_struct)),
                        },
                        Some(1) => Transition::Linear,
                        Some(2) => Transition::Hold,
                        _ => Transition::Other,
                    },
                },
            })
            .collect();
        Self { default, keys }
    }
}

/// The value at time `t` of the cubic Bézier through `p` ((time, value) points): the
/// parameter whose time is `t` is found by halving 20 times (0x7ccdf0, which stops early
/// within 1e-5 s), and the curve's value there is taken (0x7ccef0).
fn bezier(p: [[f32; 2]; 4], t: f32) -> f32 {
    let at = |axis: usize, s: f32| {
        let u = 1.0 - s;
        u * u * u * p[0][axis]
            + 3.0 * u * u * s * p[1][axis]
            + 3.0 * u * s * s * p[2][axis]
            + s * s * s * p[3][axis]
    };
    let (mut lo, mut hi, mut s) = (0.0f32, 1.0f32, 0.0f32);
    for _ in 0..20 {
        s = lo + (hi - lo) * 0.5;
        let off = at(0, s) - t;
        if off.abs() <= 1e-5 {
            break;
        }
        if off > 0.0 {
            hi = s;
        } else {
            lo = s;
        }
    }
    at(1, s)
}

/// One of an actor's tracks with curves: when it plays and its curves.
#[derive(Clone, Debug, PartialEq)]
pub struct CurveTrack {
    pub start: f32,
    pub end: f32,
    pub curves: Vec<Curve>,
}

impl CurveTrack {
    /// Its curves' values at `t` (seconds into the cutscene), once it has started; a track
    /// past its end holds its values there. Provisional (see the module docs).
    fn at(&self, t: f32) -> Option<Vec<f32>> {
        if t < self.start {
            return None;
        }
        let t = t.min(self.end);
        Some(self.curves.iter().map(|c| c.at(t)).collect())
    }
}

/// An animation an actor plays (an `AANI` track).
#[derive(Clone, Debug, PartialEq)]
pub struct AnimationTrack {
    pub start: f32,
    pub end: f32,
    /// The node of the animation tree that names the animation (`mh.dg_f_r_5p_en_v2__1`).
    pub node: String,
    /// The animation tree (`bdccs_intro.cub`).
    pub tree: String,
}

/// A cutscene actor (`ACTR`) and what moves it.
#[derive(Clone, Debug, PartialEq)]
pub struct Actor {
    /// Field 5201, which camera cuts name.
    pub id: i64,
    pub name: String,
    /// The creature it stands for (field 5208), when it is one.
    pub creature: Option<String>,
    /// Field 5214: the base the motion curves are placed from.
    pub position: [f32; 3],
    /// Field 5215: the base rotation (x, y, z, w).
    pub rotation: [f32; 4],
    /// Kind-17 tracks (three curves: x, y, z), in the file's order.
    pub motion: Vec<CurveTrack>,
    /// Kind-18 tracks (three curves of degrees), in the file's order.
    pub turn: Vec<CurveTrack>,
    /// Kind-13 tracks (one curve of degrees), in the file's order.
    pub fov: Vec<CurveTrack>,
    /// Kind-4 tracks, in the file's order.
    pub animations: Vec<AnimationTrack>,
    /// Spans (start, end) of tracks that place the actor by other means, which are not
    /// followed: `ActionJumpToCamera` (kind 24, onto a camera of the cutscene's stage;
    /// update 0x7df2f0, which sets the actor's world placement and field of view each tick
    /// and so overrides the curves) and `ActionLinkToActor` (kind 27).
    pub placed_elsewhere: Vec<(f32, f32)>,
}

/// Where an actor is at a moment of the cutscene.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    /// World position.
    pub position: [f32; 3],
    /// World rotation (x, y, z, w).
    pub rotation: [f32; 4],
    /// Field of view in degrees.
    pub fov: f32,
}

impl Actor {
    /// Where the actor is `t` seconds into the cutscene: the last of each kind of track
    /// that has started wins, as the actions update in the file's order.
    pub fn pose_at(&self, t: f32) -> Pose {
        let last = |tracks: &[CurveTrack]| {
            tracks
                .iter()
                .rev()
                .filter_map(|tr| tr.at(t))
                .find(|v| !v.is_empty())
        };
        let base = normalize(self.rotation);
        let position = match last(&self.motion) {
            Some(v) if v.len() >= 3 => {
                let p = rotate(base, [v[0], v[1], v[2]]);
                [
                    self.position[0] + p[0],
                    self.position[1] + p[1],
                    self.position[2] + p[2],
                ]
            }
            _ => self.position,
        };
        let rotation = match last(&self.turn) {
            Some(v) if v.len() >= 3 => mul(base, euler_zxy(v[0], v[1], v[2])),
            _ => base,
        };
        let fov = last(&self.fov).map_or(DEFAULT_FOV, |v| v[0]);
        Pose {
            position,
            rotation,
            fov,
        }
    }
}

/// What a cutscene does to its actors.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tracks {
    pub actors: Vec<Actor>,
    /// Camera cuts (kind-11 tracks): from this time (seconds), the actor with this id is the
    /// camera. By time; on a tie, the file's order.
    pub cuts: Vec<(f32, i64)>,
}

impl Tracks {
    /// A cutscene file's tracks (none when it is not a cutscene).
    pub fn parse(data: &[u8]) -> Self {
        Gff4::parse(data)
            .map(|g| Self::from_gff4(&g))
            .unwrap_or_default()
    }

    pub fn from_gff4(g: &Gff4) -> Self {
        if g.file_type.0 != *b"CUT " {
            return Self::default();
        }
        let mut out = Self::default();
        for a in structs(g, g.root, FIELD_ACTORS) {
            let text = |l| {
                g.field(a, l)
                    .and_then(Value::as_str)
                    .map(|s| s.trim_end_matches('\0').trim().to_string())
                    .filter(|s| !s.is_empty())
            };
            let mut actor = Actor {
                id: g
                    .field(a, FIELD_ACTOR_ID)
                    .and_then(Value::as_i64)
                    .unwrap_or(-1),
                name: text(FIELD_ACTOR_NAME).unwrap_or_default(),
                creature: text(FIELD_ACTOR_CREATURE),
                position: match g.field(a, FIELD_ACTOR_POSITION) {
                    Some(Value::Vec3(v)) if v.iter().all(|x| x.is_finite()) => *v,
                    _ => [0.0; 3],
                },
                rotation: match g.field(a, FIELD_ACTOR_ROTATION) {
                    Some(Value::Quat(q) | Value::Vec4(q)) if q.iter().all(|x| x.is_finite()) => *q,
                    _ => [0.0, 0.0, 0.0, 1.0],
                },
                motion: Vec::new(),
                turn: Vec::new(),
                fov: Vec::new(),
                animations: Vec::new(),
                placed_elsewhere: Vec::new(),
            };
            for tr in structs(g, a, FIELD_ACTOR_TRACKS) {
                let Some(kind) = g.field(tr, FIELD_TRACK_KIND).and_then(Value::as_u64) else {
                    continue;
                };
                let time = |l| {
                    g.field(tr, l)
                        .and_then(Value::as_f64)
                        .map(|v| v as f32)
                        .filter(|v| !v.is_nan())
                };
                let start = time(FIELD_TRACK_START).unwrap_or(0.0);
                let end = time(FIELD_TRACK_END).unwrap_or(f32::INFINITY).max(start);
                let curves = || CurveTrack {
                    start,
                    end,
                    curves: structs(g, tr, FIELD_TRACK_CURVES)
                        .into_iter()
                        .map(|c| Curve::from_gff4(g, c))
                        .collect(),
                };
                match kind {
                    TRACK_MOVE_SPLINE => actor.motion.push(curves()),
                    TRACK_ROTATE => actor.turn.push(curves()),
                    TRACK_CAMERA_FOV => actor.fov.push(curves()),
                    TRACK_MAKE_ACTIVE_CAMERA => {
                        if let Some(id) = g.field(tr, FIELD_CAMERA_ACTOR).and_then(Value::as_i64) {
                            out.cuts.push((start, id));
                        }
                    }
                    TRACK_JUMP_TO_CAMERA | TRACK_LINK_TO_ACTOR => {
                        actor.placed_elsewhere.push((start, end))
                    }
                    TRACK_PLAY_ANIMATION => {
                        let text = |l| {
                            g.field(tr, l)
                                .and_then(Value::as_str)
                                .map(|s| s.trim_end_matches('\0').trim().to_string())
                                .unwrap_or_default()
                        };
                        let node = text(FIELD_ANIMATION);
                        if !node.is_empty() {
                            actor.animations.push(AnimationTrack {
                                start,
                                end,
                                node,
                                tree: text(FIELD_ANIMATION_TREE),
                            });
                        }
                    }
                    _ => {}
                }
            }
            out.actors.push(actor);
        }
        out.cuts.sort_by(|a, b| a.0.total_cmp(&b.0));
        out
    }

    /// The camera actor `t` seconds into the cutscene: the one the latest cut that has
    /// happened names, if there is one and it is among the actors.
    pub fn camera_actor(&self, t: f32) -> Option<&Actor> {
        let id = self.cuts.iter().rev().find(|(at, _)| *at <= t)?.1;
        self.actors.iter().find(|a| a.id == id)
    }

    /// The camera's pose `t` seconds into the cutscene, once a cut has made one; `None`
    /// while the camera actor is placed by a track not followed (see
    /// [`Actor::placed_elsewhere`]).
    pub fn camera_at(&self, t: f32) -> Option<Pose> {
        let actor = self.camera_actor(t)?;
        if actor
            .placed_elsewhere
            .iter()
            .any(|&(start, end)| start <= t && t <= end)
        {
            return None;
        }
        Some(actor.pose_at(t))
    }
}

fn f32_field(g: &Gff4, id: StructId, label: u32) -> Option<f32> {
    g.field(id, label)
        .and_then(Value::as_f64)
        .map(|v| v as f32)
        .filter(|v| v.is_finite())
}

/// Hamilton's product `a·b` of (x, y, z, w) quaternions (0x4cb640).
pub fn mul(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    let [ax, ay, az, aw] = a;
    let [bx, by, bz, bw] = b;
    [
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
        aw * bw - ax * bx - ay * by - az * bz,
    ]
}

/// `v` turned by the unit quaternion `q` (0x4cb6d0).
pub fn rotate(q: [f32; 4], v: [f32; 3]) -> [f32; 3] {
    let p = mul(mul(q, [v[0], v[1], v[2], 0.0]), [-q[0], -q[1], -q[2], q[3]]);
    [p[0], p[1], p[2]]
}

fn normalize(q: [f32; 4]) -> [f32; 4] {
    let n = q.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > 1e-6 && n.is_finite() {
        q.map(|x| x / n)
    } else {
        [0.0, 0.0, 0.0, 1.0]
    }
}

/// The turn an `ActionRotate` makes of its three curves' degrees (0x4ee200): about Z by `a`,
/// then X by `b`, then Y by `c`.
pub fn euler_zxy(a: f32, b: f32, c: f32) -> [f32; 4] {
    let about = |axis: [f32; 3], degrees: f32| {
        let (s, w) = (degrees.to_radians() * 0.5).sin_cos();
        [axis[0] * s, axis[1] * s, axis[2] * s, w]
    };
    mul(
        mul(about([0.0, 0.0, 1.0], a), about([1.0, 0.0, 0.0], b)),
        about([0.0, 1.0, 0.0], c),
    )
}

#[cfg(test)]
mod tests {
    use thedas_formats::gff4::{
        FieldDef, FourCC, GenericSlot, List, StructData, StructTemplate, Version,
    };

    use super::*;

    fn key(time: f32, value: f32, next: Transition) -> Key {
        Key { time, value, next }
    }

    #[test]
    fn evaluates_curves_as_the_executable_does() {
        assert_eq!(Curve::constant(45.0).at(3.0), 45.0);
        let c = Curve {
            default: 7.0,
            keys: vec![
                key(1.0, 10.0, Transition::Linear),
                key(3.0, 20.0, Transition::Hold),
                key(5.0, 30.0, Transition::Other),
                key(6.0, 0.0, Transition::Linear),
            ],
        };
        // Held before the first key and after the last.
        assert_eq!(c.at(0.0), 10.0);
        assert_eq!(c.at(9.0), 0.0);
        assert_eq!(c.at(2.0), 15.0);
        // A key is reached a hair early.
        assert_eq!(c.at(3.0 - 5e-5), 20.0);
        assert_eq!(c.at(4.0), 20.0);
        // An unknown transition gives the default.
        assert_eq!(c.at(5.5), 7.0);
        // A Bézier whose control points lie on the line is the line.
        let b = Curve {
            default: 0.0,
            keys: vec![
                key(
                    0.0,
                    0.0,
                    Transition::Bezier {
                        out: [1.0, 1.0],
                        into: [2.0, 2.0],
                    },
                ),
                key(3.0, 3.0, Transition::Linear),
            ],
        };
        assert!((b.at(1.2) - 1.2).abs() < 1e-3, "{}", b.at(1.2));
        // Easing in: slow at the start.
        let ease = Curve {
            default: 0.0,
            keys: vec![
                key(
                    0.0,
                    0.0,
                    Transition::Bezier {
                        out: [1.0, 0.0],
                        into: [2.0, 3.0],
                    },
                ),
                key(3.0, 3.0, Transition::Linear),
            ],
        };
        assert!(ease.at(0.5) < 0.25, "{}", ease.at(0.5));
        assert!((ease.at(1.5) - 1.5).abs() < 1e-3, "{}", ease.at(1.5));
    }

    #[test]
    fn turns_about_z_then_x_then_y() {
        // A quarter turn about Z takes +Y to -X.
        let q = euler_zxy(90.0, 0.0, 0.0);
        let v = rotate(q, [0.0, 1.0, 0.0]);
        assert!((v[0] + 1.0).abs() < 1e-5 && v[1].abs() < 1e-5, "{v:?}");
        // Pitching up 30° about X, then turning: the X turn is applied in the Z-turned frame.
        let v = rotate(euler_zxy(90.0, 30.0, 0.0), [0.0, 1.0, 0.0]);
        let s = 30f32.to_radians();
        assert!(
            (v[0] + s.cos()).abs() < 1e-5 && (v[2] - s.sin()).abs() < 1e-5,
            "{v:?}"
        );
    }

    fn template(label: &[u8; 4], fields: &[u32]) -> StructTemplate {
        StructTemplate {
            label: FourCC::new(label),
            fields: fields
                .iter()
                .map(|&label| FieldDef {
                    label,
                    type_code: 0,
                    flags: 0,
                    offset: 0,
                })
                .collect(),
            size: 0,
        }
    }

    fn slot(id: usize) -> GenericSlot {
        GenericSlot {
            type_code: 0,
            flags: 0,
            value: Some(Value::Struct(Some(StructId(id)))),
        }
    }

    fn list(ids: &[usize]) -> Value {
        Value::List(Some(List::Struct(
            ids.iter().map(|&i| Some(StructId(i))).collect(),
        )))
    }

    /// A cutscene with one camera actor (id 7) whose x moves 0→10 over two seconds from a
    /// base of (100, 0, 0) turned a quarter about Z, looking turned 90° about Z, with a
    /// field of view of 30°; a cut makes it the camera at 0.5 s.
    fn cut() -> Gff4 {
        let curve = |default: f32, keys: &[usize]| StructData {
            template: 3,
            values: vec![
                Value::F32(default),
                if keys.is_empty() {
                    Value::List(None)
                } else {
                    list(keys)
                },
                Value::List(None),
            ],
        };
        let vert = |t: f32, v: f32| StructData {
            template: 4,
            values: vec![Value::F32(t), Value::F32(v)],
        };
        let track = |kind: u8, curves: &[usize]| StructData {
            template: 2,
            values: vec![
                Value::U8(kind),
                Value::F32(0.0),
                Value::F32(1e10),
                list(curves),
                Value::Absent,
            ],
        };
        let h = std::f32::consts::FRAC_1_SQRT_2;
        Gff4 {
            version: Version::V40,
            platform: FourCC::new(b"PC  "),
            file_type: FourCC::new(b"CUT "),
            file_version: FourCC::new(b"V1.0"),
            templates: vec![
                template(b"CUT ", &[FIELD_ACTORS]),
                template(
                    b"ACTR",
                    &[
                        FIELD_ACTOR_NAME,
                        FIELD_ACTOR_ID,
                        FIELD_ACTOR_POSITION,
                        FIELD_ACTOR_ROTATION,
                        FIELD_ACTOR_TRACKS,
                    ],
                ),
                template(
                    b"TRAK",
                    &[
                        FIELD_TRACK_KIND,
                        FIELD_TRACK_START,
                        FIELD_TRACK_END,
                        FIELD_TRACK_CURVES,
                        FIELD_CAMERA_ACTOR,
                    ],
                ),
                template(
                    b"CURV",
                    &[
                        FIELD_CURVE_DEFAULT,
                        FIELD_CURVE_KEYS,
                        FIELD_CURVE_TRANSITIONS,
                    ],
                ),
                template(b"VERT", &[FIELD_KEY_TIME, FIELD_KEY_VALUE]),
            ],
            structs: vec![
                // 0: root
                StructData {
                    template: 0,
                    values: vec![list(&[1])],
                },
                // 1: the camera actor
                StructData {
                    template: 1,
                    values: vec![
                        Value::String(Some("Camera 1\0".into())),
                        Value::I32(7),
                        Value::Vec3([100.0, 0.0, 0.0]),
                        Value::Quat([0.0, 0.0, h, h]),
                        Value::List(Some(List::Generic(vec![
                            slot(2),
                            slot(3),
                            slot(4),
                            slot(5),
                        ]))),
                    ],
                },
                // 2: motion; 3: turn; 4: field of view
                track(17, &[6, 7, 8]),
                track(18, &[9, 10, 11]),
                track(13, &[12]),
                // 5: the cut
                StructData {
                    template: 2,
                    values: vec![
                        Value::U8(11),
                        Value::F32(0.5),
                        Value::F32(0.5),
                        Value::List(None),
                        Value::I32(7),
                    ],
                },
                // 6-8: x 0→10 (linear: no transitions), y 0, z 2
                curve(0.0, &[13, 14]),
                curve(0.0, &[]),
                curve(2.0, &[]),
                // 9-11: 90° about Z
                curve(90.0, &[]),
                curve(0.0, &[]),
                curve(0.0, &[]),
                // 12: field of view
                curve(30.0, &[]),
                vert(0.0, 0.0),
                vert(2.0, 10.0),
            ],
            root: StructId(0),
            shared_strings: None,
        }
    }

    #[test]
    fn plays_the_camera_actor_the_cuts_name() {
        let t = Tracks::from_gff4(&cut());
        assert_eq!(t.cuts, [(0.5, 7)]);
        assert_eq!(t.actors.len(), 1);
        assert!(t.camera_at(0.25).is_none());
        let p = t.camera_at(1.0).unwrap();
        // x 5 turned a quarter about Z is y 5, from the base at x 100.
        assert!(
            (p.position[0] - 100.0).abs() < 1e-4
                && (p.position[1] - 5.0).abs() < 1e-4
                && (p.position[2] - 2.0).abs() < 1e-4,
            "{:?}",
            p.position
        );
        // Base quarter turn then the curves' quarter turn: +Y looks along -Y.
        let f = rotate(p.rotation, [0.0, 1.0, 0.0]);
        assert!((f[1] + 1.0).abs() < 1e-4, "{f:?}");
        assert_eq!(p.fov, 30.0);
        // A camera a stage places is not followed while it is.
        let mut staged = t.clone();
        staged.actors[0].placed_elsewhere.push((0.75, 1.5));
        assert!(staged.camera_at(1.0).is_none());
        assert!(staged.camera_at(1.6).is_some());
        // Other files have none.
        let mut other = cut();
        other.file_type = FourCC::new(b"CONV");
        assert_eq!(Tracks::from_gff4(&other), Tracks::default());
        assert_eq!(Tracks::parse(b"junk"), Tracks::default());
    }
}
