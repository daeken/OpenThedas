//! What a cutscene does to its actors over time: the curves that move and turn them, the
//! places they jump to, their animations, the camera actors and which of them is the view.
//! The presentation plays these on the cutscene's clock
//! ([`crate::cutscene::Cutscene::elapsed_ms`]); the simulation reads only which objects
//! play the actors and where the actors end ([`crate::cutactors`]).
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
//!   of the cutscene's `.cub` animation tree (5401), whose node names the `.ani` file; see
//!   [`AnimationTrack`].
//! - Kind 25, `ActionJumpToPlace`: see [`PlaceTrack`]. Kind 38, `ActionSnapToWalkmesh`
//!   (vtable 0xb23584): the actor keeps to the ground.
//!
//! An actor's world placement goes through the cutscene's own placement (0x7d0660, 0x7cf6b0:
//! cutscene +0xa0 and +0xb0), which only a cutscene played at another object sets
//! (0x7cdc40); a cutscene loaded by name plays where it was made.
//!
//! - Kind 24, `ActionJumpToCamera` (`AJSC`; built by 0x7dc450, update 0x7df2f0): see
//!   [`CameraJump`].
//! - Kind 19, the frame buffer effect action (`AFBE`): see [`FrameEffectTrack`].
//! - Kind 27, `ActionLinkToActor` (built by 0x7db900, update 0x7d3cf0): see [`ActorLink`].
//! - Kind 28, `ActionHeadtracking` (built by 0x7da820, update 0x7d0d00): see [`HeadTrack`].
//! - Kind 32, `ActionChangeVisibility` (built by 0x7dba50, update 0x7d44c0): from its start
//!   the actor is shown or hidden as its field 5680 says (0x7d4220); when the cutscene lets
//!   go of the actor it is as the world has it again (0x7d44d0). See [`Actor::visible_at`].
//! - Kind 41, `ActionDrawWeapon` (built by 0x7dce50, update 0x7d1210): see [`WeaponDraw`].
//!
//! Provisional: which way a camera actor looks (taken as its local +Y, as a stage camera's;
//! up is +Z); whether a field of view is vertical; tracks outside their span (a track is
//! taken to apply from its start and to hold its value at its end after it); an actor with
//! no motion track stands at its base; a camera with no field-of-view track sees 45°; a
//! missing transition is linear.

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
const FIELD_ACTOR_POSE: u32 = 5211;
const FIELD_ACTOR_TAG: u32 = 5216;
const FIELD_ACTOR_END_POSITION: u32 = 5221;
const FIELD_ACTOR_END_ROTATION: u32 = 5222;
const FIELD_CUT_TREE: u32 = 5007;
const FIELD_CUT_STAGE: u32 = 5017;
const FIELD_CUT_SECONDS: u32 = 5000;
const FIELD_PLACE_NAME: u32 = 1;
const FIELD_PLACE_LOOK_AT: u32 = 5580;
const FIELD_TRACK_KIND: u32 = 5300;
const FIELD_TRACK_START: u32 = 5301;
const FIELD_TRACK_END: u32 = 5302;
const FIELD_TRACK_CURVES: u32 = 5303;
const FIELD_CAMERA_ACTOR: u32 = 5610;
const FIELD_EFFECT_NAME: u32 = 5521;
const FIELD_EFFECT_PARAMETERS: u32 = 5522;
const FIELD_PARAMETER_NAME: u32 = 5523;
const FIELD_PARAMETER_VALUE: u32 = 5524;
const FIELD_PARAMETER_CURVE: u32 = 5525;
const FIELD_PARAMETER_COMPONENT: u32 = 5526;
const FIELD_JUMP_BY_PLACE: u32 = 5570;
const FIELD_VISIBLE: u32 = 5680;
const FIELD_DRAW_MAIN: u32 = 5730;
const FIELD_DRAW_OFF: u32 = 5731;
const FIELD_LOOK_TARGET: u32 = 5620;
const FIELD_LOOK_SPEED: u32 = 5621;
const FIELD_LINK_TARGET: u32 = 5630;
const FIELD_LINK_NODE: u32 = 5631;
const FIELD_LINK_BY_BONE: u32 = 5636;
const FIELD_LINK_KEEP_OFFSET: u32 = 5637;
const FIELD_ANIMATION: u32 = 5400;
const FIELD_ANIMATION_TREE: u32 = 5401;
const FIELD_ANIMATION_SPEED: u32 = 5402;
const FIELD_ANIMATION_OFFSET: u32 = 5403;
const FIELD_ANIMATION_ROOT_POSITIONS: u32 = 5408;
const FIELD_ANIMATION_ROOT_ROTATIONS: u32 = 5409;
const FIELD_ANIMATION_ROOT_HELD: u32 = 5410;
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
pub const TRACK_FRAME_EFFECT: u64 = 19;
pub const TRACK_LINK_TO_ACTOR: u64 = 27;
pub const TRACK_HEAD_TRACKING: u64 = 28;
pub const TRACK_CHANGE_VISIBILITY: u64 = 32;
pub const TRACK_DRAW_WEAPON: u64 = 41;
pub const TRACK_JUMP_TO_CAMERA: u64 = 24;
pub const TRACK_JUMP_TO_PLACE: u64 = 25;
pub const TRACK_SNAP_TO_WALKMESH: u64 = 38;

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

/// An animation an actor plays (an `AANI` track, `ActionPlayAnimation`; its fields are read
/// by 0x7dad20 and it updates at 0x7db400).
#[derive(Clone, Debug, PartialEq)]
pub struct AnimationTrack {
    pub start: f32,
    pub end: f32,
    /// The node of the animation tree that names the animation (`mh.dg_f_r_5p_en_v2__1`).
    pub node: String,
    /// The animation tree (`bdccs_intro.cub`, an `ANBT` like the `.anb` trees); the
    /// cutscene's own (5007) when the track names none.
    pub tree: String,
    /// Field 5402 (+0x60): how fast the animation runs.
    pub speed: f32,
    /// Field 5403 (+0x64): seconds into the animation it starts at.
    pub offset: f32,
    /// Field 5408 (+0xb4): the animation's root motion as the cutscene file bakes it, keys of
    /// (seconds into the animation, x, y, z) in the actor's own frame. Empty for a pose.
    pub root_positions: Vec<[f32; 4]>,
    /// Field 5409 (+0xbc): the root's turn, keys of (seconds, x, y, z, w).
    pub root_rotations: Vec<[f32; 5]>,
    /// Field 5410 (+0x81): the root motion does not move the actor. Provisional: the
    /// executable takes another path for such a track (0x7d8180), which is not traced, so
    /// what the boolean means is inferred from the branch alone.
    pub root_held: bool,
}

impl AnimationTrack {
    /// Seconds into the animation `t` seconds into the cutscene (0x7db400): the time since
    /// the track's start, held to its span, times its speed, plus its offset. 0x7db400
    /// stops a hair (0xae7118) short of the span's end; that is not followed.
    pub fn time_at(&self, t: f32) -> f32 {
        let since = (t - self.start).clamp(0.0, (self.end - self.start).max(0.0));
        self.speed * since + self.offset
    }

    /// How far the animation's root motion has moved and turned the actor `t` seconds into
    /// the cutscene: the root keys at the animation's time, in the actor's own frame.
    /// `None` before the track starts, for a track without root keys and for one that
    /// holds its actor ([`AnimationTrack::root_held`]).
    ///
    /// The executable's track update (0x7d92e0) samples the two key lists (0xa38de0,
    /// 0xa38d20), turns the position into the actor's frame (0x7d3c80) and sets the actor's
    /// position to its own plus that, and its rotation to its own times the turn (0x4cb640);
    /// the track's end (0x7d9450) leaves the last keys in place, so the actor stays where
    /// the animation took it (a quarter turn keeps the actor turned).
    ///
    /// Provisional: the keys are taken as linear between samples, and what fields 5405,
    /// 5411 and 5413 switch is not traced.
    pub fn root_at(&self, t: f32) -> Option<([f32; 3], [f32; 4])> {
        if self.root_held
            || t < self.start
            || (self.root_positions.is_empty() && self.root_rotations.is_empty())
        {
            return None;
        }
        let at = self.time_at(t);
        // The keys either side of `at` and how far between them it is.
        fn span<const N: usize>(keys: &[[f32; N]], at: f32) -> Option<(usize, usize, f32)> {
            let last = keys.len().checked_sub(1)?;
            let next = keys.iter().position(|k| k[0] >= at).unwrap_or(last);
            let prev = next.saturating_sub(1);
            let (a, b) = (keys[prev][0], keys[next][0]);
            let f = if b > a && at > a {
                ((at - a) / (b - a)).clamp(0.0, 1.0)
            } else if at >= b {
                1.0
            } else {
                0.0
            };
            Some((prev, next, f))
        }
        let position = match span(&self.root_positions, at) {
            Some((a, b, f)) => {
                let (a, b) = (self.root_positions[a], self.root_positions[b]);
                [1, 2, 3].map(|i| a[i] + (b[i] - a[i]) * f)
            }
            None => [0.0; 3],
        };
        let rotation = match span(&self.root_rotations, at) {
            Some((a, b, f)) => {
                let (a, b) = (self.root_rotations[a], self.root_rotations[b]);
                let (qa, mut qb) = ([a[1], a[2], a[3], a[4]], [b[1], b[2], b[3], b[4]]);
                // The short way round between the two keys.
                if qa.iter().zip(&qb).map(|(x, y)| x * y).sum::<f32>() < 0.0 {
                    qb = qb.map(|x| -x);
                }
                normalize([0, 1, 2, 3].map(|i| qa[i] + (qb[i] - qa[i]) * f))
            }
            None => [0.0, 0.0, 0.0, 1.0],
        };
        Some((position, rotation))
    }
}

/// An `ActionJumpToPlace` track (kind 25, `AJSP`; update 0x7dc720): while it plays, the
/// actor stands at a place of the cutscene's stage (field 1), turned towards another place
/// (5580) when it names one, else as the place is turned. The place is looked up once
/// (+0x60), and the actor is set there on every update.
#[derive(Clone, Debug, PartialEq)]
pub struct PlaceTrack {
    pub start: f32,
    pub end: f32,
    pub place: String,
    pub look_at: Option<String>,
}

/// An `ActionJumpToCamera` track (kind 24, `AJSC`): while it plays, the actor (a camera)
/// is a camera of the cutscene's stage file (cutscene +0xc4), placed as the cutscene is.
///
/// Traced: the constructor 0x7dc450 reads the name (field 1, +0x28) and two flags, 5570
/// (+0x44) and 5571 (+0x45). The first update (0x7df2f0, once: +0x46) resolves the camera:
/// with 5570 the name is a place's and the camera is that place's default camera
/// (0x7d1e10); with neither flag it is the camera of that name. 0x7deaf0 then reads the
/// stage camera (0x7d1ea0: found by exact name among the stage's cameras, 11001, by
/// 0x7d1bf0; position 4, rotation 5, field of view 11003) and places it as the cutscene is
/// (0x4cb6d0 turns the position by the cutscene's rotation +0xb0 and adds its position
/// +0xa0; 0x4cb640 multiplies the rotations). Every update sets the actor's world position
/// (0x7cf450), rotation (0x7cdde0) and field of view (+0x64) to it; it runs after the
/// actor's curve tracks, so it overrides them.
///
/// Provisional: 5571 (a camera named after the place of a jump-to-place track of another
/// actor, 0x7df369) is not followed: no cutscene or conversation line of the game sets it.
/// Stage camera kinds 1 to 3 (11009; 0x7de060 and 0x7de220 aim them) are taken as plain
/// cameras. A camera the stage does not have gives no pose here (the executable leaves the
/// actor at the world's origin).
#[derive(Clone, Debug, PartialEq)]
pub struct CameraJump {
    pub start: f32,
    pub end: f32,
    /// The stage camera's name, or with [`CameraJump::by_place`] the place's.
    pub camera: String,
    /// Field 5570: `camera` names a place, whose default camera this is.
    pub by_place: bool,
}

/// What a link names on the actor it links to: fields 5631 and 5636 of an
/// `ActionLinkToActor` (see [`ActorLink`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LinkNode {
    /// 5631 is -1: the root of the actor's model (0x7d3df0, its vfunc +0x68), which stands
    /// and turns as the actor does.
    Root,
    /// 5636 set: the bone with this index in the actor's skeleton (0x7d3d53 looks the
    /// index up in a map of the model's, 0x562e00, and takes the name of what it finds),
    /// found by that name among the model's nodes (vfunc +0x10, 0x7d3dfe).
    Bone(u32),
    /// 5636 clear: row 5631 of an engine table (0x7d3d8d: table 0x5b, column hash
    /// 0x90028895) names the node. Only props use it (a dagger in a hand); no camera does.
    Table(i64),
}

/// An `ActionLinkToActor` track (kind 27): while it plays, the actor (the camera, a prop, an
/// effect) is carried by a node of another actor, as a child carried by its parent.
///
/// Traced: the constructor 0x7db900 reads the actor linked to (field 5630, +0x28: an actor
/// id, 5201, found by 0x7d0470), the node (5631, +0x4c), whether that is a bone index
/// (5636, +0x49; see [`LinkNode`]) and whether the actor keeps its own place relative to
/// the node (5637, +0x48). Each update (0x7d3cf0) finds the node of the linked actor's model
/// and attaches the actor's model to it (vfunc +0xf0) with an offset: with 5637, the
/// actor's own position and rotation in the cutscene's space (0x7cf640, 0x7cf6f0: as its
/// motion and rotation tracks set them this update) taken back from its base (5214, 5215,
/// actor +0xe0 and +0xf0): `offset = base⁻¹·(own − base)`, `turn = base⁻¹·own`, which are
/// the motion and rotation curves' own values; without it, no offset (the node's own
/// place). The actor is then where the node is, offset in the node's frame: position
/// `node + node_rotation·offset`, rotation `node_rotation·turn` (worked out explicitly,
/// when the cutscene's +0x94 is 5, at 0x7d3f56 -> 0x7cf450 and 0x7d3f98 -> 0x7cdde0;
/// otherwise the scene graph composes the attachment). Its field of view stays its own. An
/// update at or past the track's end (time since its start ≥ end − start) detaches it
/// again (vfunc +0xf8, 0x7d4027), as does the actor's reset (0x7d1150, vtable slot 2): the
/// actor is then where its own tracks put it. The action dispatcher (0x7da2ff) updates a
/// track from 1e-4 s before its start to 1e-4 s after its end. A linked actor the cutscene
/// cannot find, or one without a model, links nothing (0x7d3d19, 0x7d3e08).
///
/// The toolset keys a linked camera's curves as offsets from the node while the link plays
/// and back in the cutscene's space after it (`den600cs_loghains_duel_qun`'s Camera 2: x
/// 15.08 until 12.23 s, 0.15 at the link's start, 15.08 again after its end at 14.9 s).
///
/// Provisional: every link follows the linked actor's root. The offsets of the game's
/// camera links to bones fit the root's frame and not the bone's: `ntb100cs_attackondalish`'s
/// Camera 8 (linked to bone 7, the model hierarchy's `Chest2` on an elf) sits 1.5 m in
/// front of the elf, 1.45 m up and turned back towards it (a close-up of the face from the
/// root; from `Chest2`, whose frame is turned a quarter about two axes, it would sit 0.4 m
/// off the ground looking past the elf), and `124383`'s Camera 2 (bone 7 of Bhelen) 0.78 m
/// in front of the dwarf at 1.31 m, looking back. Which node the bone index names, and
/// table nodes, are not followed. A linked actor the world does not draw is taken where
/// its own tracks put it. The link is taken to end at its end: the executable detaches
/// only on an update within 1e-4 s of the end, and that the toolset keys the curves back
/// in the cutscene's space there says it is meant to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ActorLink {
    pub start: f32,
    pub end: f32,
    /// The id (5201) of the actor linked to.
    pub target: i64,
    pub node: LinkNode,
    /// Field 5637: the actor keeps its own tracks' place as an offset from the node.
    pub keep_offset: bool,
}

impl ActorLink {
    /// Whether the link holds `t` seconds into the cutscene: from its start (a hair early,
    /// as the dispatcher updates it) until its end, where it detaches.
    pub fn holds(&self, t: f32) -> bool {
        self.start <= t + KEY_SNAP && t < self.end
    }
}

/// An `ActionDrawWeapon` track (kind 41; `ADW_`): where the actor's creature carries its
/// weapons from the action's start.
///
/// Traced (update 0x7d1210, through 0x7cfdb0): the main hand's item (inventory slot 3) goes
/// to its drawn place when field 5730 is set, else to its sheathed place (the item's
/// `BITM_base.gda` parts, columns 0x48859b70 and 0x7083f7d4); when field 5731 is set the
/// off hand's item (slot 4) goes to its drawn place (column 0x9cc4a4af) and is otherwise
/// left as it is. When the cutscene lets go of the actor both are sheathed (0x7d1260).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WeaponDraw {
    pub start: f32,
    /// Field 5730: the main hand's weapon is drawn (else sheathed).
    pub main: bool,
    /// Field 5731: the off hand's is drawn too (else left as it is).
    pub off: bool,
}

/// An `ActionHeadtracking` track (kind 28; `ACHT`): what the actor's head tracker looks at.
///
/// Traced (update 0x7d0d00): as the action starts, the actor's creature's head tracker is
/// started (0x6cab00) and its turn speeds are scaled by field 5621 (0x6c9f10); each update
/// it is given the place to look at on the actor whose id (5201) is field 5620, and with
/// no such actor, or the actor itself, it looks straight ahead. Fields 5628 and 5629
/// (booleans) are not followed.
///
/// Provisional: nothing was seen to stop the tracker when the action ends, so the actor is
/// taken to go on looking as its last started action said ([`Actor::head_track_at`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HeadTrack {
    pub start: f32,
    pub end: f32,
    /// The id (5201) of the actor looked at; `None` when the field is absent.
    pub target: Option<i64>,
    /// Field 5621: what the tracker's speeds are multiplied by.
    pub speed: f32,
}

/// Where a cutscene's actor (an index into [`Tracks::actors`]) is drawn in the world: its
/// model's root's position and rotation (x, y, z, w). `None` when it is not drawn.
pub type RootLookup<'a> = dyn Fn(usize) -> Option<([f32; 3], [f32; 4])> + 'a;

/// How deep links of links are followed (a camera linked to a camera linked to a creature).
const MAX_LINK_DEPTH: u32 = 8;

/// A stage camera's pose in the cutscene's space, by its name (or, when the flag is set,
/// by the name of the place whose default camera it is); see [`CameraJump`].
pub type StageCameraLookup<'a> = dyn Fn(&str, bool) -> Option<Pose> + 'a;

/// A stage place's position and rotation by name (see [`Actor::stage_pose_at`]).
pub type PlaceLookup<'a> = dyn Fn(&str) -> Option<([f32; 3], [f32; 4])> + 'a;

/// A cutscene actor (`ACTR`) and what moves it.
#[derive(Clone, Debug, PartialEq)]
pub struct Actor {
    /// Field 5201, which camera cuts name.
    pub id: i64,
    pub name: String,
    /// The creature template it stands for (field 5208), when it is one: a creature the
    /// cutscene shows by itself when no object of the world stands in for the actor.
    pub creature: Option<String>,
    /// Field 5216 (actor +0x44): the object of the world that plays the actor, found by
    /// this name when the cutscene starts (0x7e36b0): `OWNER`, `PLAYER`, `HENCH...` or a
    /// tag.
    pub tag: Option<String>,
    /// Fields 5221 and 5222 (actor +0x100, +0x110): where the actor ends the cutscene
    /// (position, rotation x y z w) in the cutscene's space, as the toolset worked it out
    /// from its tracks. A creature that plays the actor is put there when a
    /// conversation's cutscene ends (see [`crate::cutactors`]).
    pub end_position: [f32; 3],
    pub end_rotation: [f32; 4],
    /// The actor has an `ActionSnapToWalkmesh` track (kind 38): it is kept on the ground.
    pub snaps: bool,
    /// Field 5211 (+0x454): the base pose the actor holds through the cutscene, the node
    /// `pose_<n>` of the cutscene's animation tree (17 is a sitting loop; see
    /// [`Actor::pose_node`]). `None` when the file gives none (the executable's -1).
    pub pose: Option<i64>,
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
    /// Kind-25 tracks, in the file's order.
    pub places: Vec<PlaceTrack>,
    /// Kind-24 tracks, in the file's order.
    pub camera_jumps: Vec<CameraJump>,
    /// Kind-27 tracks, in the file's order.
    pub links: Vec<ActorLink>,
    /// Kind-28 tracks, in the file's order.
    pub head_tracks: Vec<HeadTrack>,
    /// Kind-32 tracks, in the file's order: when, and whether shown from then.
    pub visibility: Vec<(f32, bool)>,
    /// Kind-41 tracks, in the file's order.
    pub weapon_draws: Vec<WeaponDraw>,
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
    /// Whether the cutscene shows or hides the actor `t` seconds in: what its last started
    /// visibility action says; `None` before the first (it is as the world has it).
    pub fn visible_at(&self, t: f32) -> Option<bool> {
        self.visibility
            .iter()
            .filter(|(start, _)| *start <= t + KEY_SNAP)
            .max_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, shown)| *shown)
    }

    /// Where the cutscene has the actor carry its weapons `t` seconds in: whether the main
    /// hand's is drawn (as its last started draw action says) and, when any action started
    /// so far drew it, that the off hand's is. `None` before the first action.
    pub fn weapons_at(&self, t: f32) -> Option<(bool, Option<bool>)> {
        let started = || self.weapon_draws.iter().filter(|w| w.start <= t + KEY_SNAP);
        let last = started().max_by(|a, b| a.start.total_cmp(&b.start))?;
        Some((last.main, started().any(|w| w.off).then_some(true)))
    }

    /// The head tracking action that says where the actor looks `t` seconds in: the last
    /// started (see [`HeadTrack`]).
    pub fn head_track_at(&self, t: f32) -> Option<&HeadTrack> {
        self.head_tracks
            .iter()
            .filter(|h| h.start <= t + KEY_SNAP)
            .max_by(|a, b| a.start.total_cmp(&b.start))
    }

    /// The node of the cutscene's animation tree that is the actor's base pose: `pose_<n>`
    /// for its pose number (field 5211). The executable makes the name with `pose_%d`
    /// (0x7ce490, the string at 0xb23164) and plays it under the actor's animation tracks
    /// (the actor update, 0x7d6240 on, through 0x7ce330); the trees hold such nodes beside
    /// the animations they stand for (`pose_17` with `mh.wi_sit_lp`).
    ///
    /// Provisional: the `transition_<from>_<to>` nodes the executable plays between two
    /// poses (the same function, with field 5219 as the other number) are not followed; the
    /// actor is in its pose from the start.
    pub fn pose_node(&self) -> Option<String> {
        self.pose.map(|n| format!("pose_{n}"))
    }

    /// The animations the actor plays `t` seconds into a cutscene `length` seconds long,
    /// in the file's order, each with seconds into it: those whose span holds `t`, and the
    /// last one started if it is a pose (no span) or runs past the cutscene's end, which
    /// holds its last frame. Provisional: what the executable does with a track's pose at
    /// its end is not followed (its root motion is: 0x7d92e0 and 0x7d9450, see
    /// [`AnimationTrack::root_at`]; what the booleans 5405, 5411 and 5413 switch is not
    /// traced).
    pub fn animations_at(&self, t: f32, length: f32) -> Vec<(&AnimationTrack, f32)> {
        let latest = self
            .animations
            .iter()
            .filter(|a| a.start <= t)
            .max_by(|a, b| a.start.total_cmp(&b.start));
        self.animations
            .iter()
            .filter(|a| {
                a.start <= t
                    && (t <= a.end
                        || latest.is_some_and(|l| {
                            std::ptr::eq(l, *a) && (a.end <= a.start || a.end >= length)
                        }))
            })
            .map(|a| (a, a.time_at(t)))
            .collect()
    }

    /// Where the actor stands `t` seconds into the cutscene, in the cutscene's space, with
    /// its stage's places (`place` gives a place's position and rotation by name). An
    /// `ActionJumpToPlace` playing puts it on its place; otherwise its motion tracks do
    /// ([`Actor::pose_at`]). Provisional: the jump is taken to win over the motion tracks
    /// while it plays (as the actors' end poses, 5221, say: they are the places), and an
    /// actor whose motion tracks have no keys is taken to stay on the last place it jumped
    /// to.
    pub fn stage_pose_at(&self, t: f32, place: &PlaceLookup) -> Pose {
        let keyed = self
            .motion
            .iter()
            .chain(&self.turn)
            .any(|tr| tr.curves.iter().any(|c| !c.keys.is_empty()));
        let jump = self
            .places
            .iter()
            .rev()
            .find(|p| p.start <= t && t <= p.end)
            .or_else(|| {
                (!keyed)
                    .then(|| {
                        self.places
                            .iter()
                            .filter(|p| p.start <= t)
                            .max_by(|a, b| a.end.total_cmp(&b.end))
                    })
                    .flatten()
            });
        let mut pose = self.pose_at(t);
        if let Some(j) = jump
            && let Some((at, rot)) = place(&j.place)
        {
            pose.position = at;
            pose.rotation = match j.look_at.as_deref().and_then(place) {
                Some((to, _)) if (to[0] - at[0]).hypot(to[1] - at[1]) > 1e-6 => {
                    let f = crate::world::facing_toward(to[0] - at[0], to[1] - at[1]);
                    Placement::facing([0.0; 3], f).rotation
                }
                _ => normalize(rot),
            };
        }
        // Each animation's root motion moves the actor on from there, in the file's order,
        // and stays once its track has ended (see [`AnimationTrack::root_at`]).
        for (step, turn) in self.animations.iter().filter_map(|a| a.root_at(t)) {
            let moved = rotate(pose.rotation, step);
            pose.position = [0, 1, 2].map(|i| pose.position[i] + moved[i]);
            pose.rotation = normalize(mul(pose.rotation, turn));
        }
        pose
    }

    /// The link that holds the actor `t` seconds into the cutscene: the last in the file's
    /// order, as the actions update in that order.
    pub fn link_at(&self, t: f32) -> Option<&ActorLink> {
        self.links.iter().rev().find(|l| l.holds(t))
    }

    /// The offset a link that keeps it gives the actor whose own pose (in the cutscene's
    /// space) is `own`: that pose taken back from the actor's base (see [`ActorLink`]).
    pub fn link_offset(&self, own: &Pose) -> ([f32; 3], [f32; 4]) {
        let back = conjugate(normalize(self.rotation));
        let d = [
            own.position[0] - self.position[0],
            own.position[1] - self.position[1],
            own.position[2] - self.position[2],
        ];
        (rotate(back, d), mul(back, normalize(own.rotation)))
    }

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

/// Where a cutscene plays: its space's origin and turn in the world (cutscene +0xa0 and
/// +0xb0). A cutscene a script loads plays where it was made (no placement); a
/// conversation's plays on its stage (0x7d2aa0).
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Placement {
    pub position: [f32; 3],
    /// x, y, z, w.
    pub rotation: [f32; 4],
}

impl Placement {
    pub const NONE: Placement = Placement {
        position: [0.0; 3],
        rotation: [0.0, 0.0, 0.0, 1.0],
    };

    /// Placed at `position`, turned `facing` degrees about Z (0x4cbc40's quaternion of the
    /// angle 0x4ee570 takes from a direction).
    pub fn facing(position: [f32; 3], facing: f32) -> Placement {
        let (s, w) = (facing.to_radians() * 0.5).sin_cos();
        Placement {
            position,
            rotation: [0.0, 0.0, s, w],
        }
    }

    /// A point of the cutscene's space in the world.
    pub fn point(&self, p: [f32; 3]) -> [f32; 3] {
        let r = rotate(normalize(self.rotation), p);
        [
            self.position[0] + r[0],
            self.position[1] + r[1],
            self.position[2] + r[2],
        ]
    }

    /// A rotation of the cutscene's space in the world.
    pub fn turn(&self, q: [f32; 4]) -> [f32; 4] {
        mul(normalize(self.rotation), normalize(q))
    }

    /// A pose in the cutscene's space in the world.
    pub fn pose(&self, p: Pose) -> Pose {
        Pose {
            position: self.point(p.position),
            rotation: self.turn(p.rotation),
            fov: p.fov,
        }
    }
}

/// The facing (degrees about Z, as objects face; see [`crate::world::heading_of`]) of a
/// rotation: where it turns +Y, flattened (0x7e1485 then 0x451a30, which the end of a
/// cutscene hands an object as its orientation). `None` when it looks straight up or down.
pub fn facing_of(q: [f32; 4]) -> Option<f32> {
    let f = rotate(normalize(q), [0.0, 1.0, 0.0]);
    let len = (f[0] * f[0] + f[1] * f[1]).sqrt();
    (len > 1e-6 && len.is_finite()).then(|| crate::world::facing_toward(f[0], f[1]))
}

/// A frame buffer effect a cutscene runs over its span (`AFBE`, kind 19): the effect
/// (`da_dof`, `da_colorfade`, ...; 5521) and the values it gives the effect's exposed
/// parameters (5522: `FBEP` structs).
///
/// Traced: the action's constructor (0x7dbbe6-0x7dbf49) looks each parameter up by name
/// (5523) on the effect and by its type keeps a float's value (5524) and curve index (5525),
/// or one component (5526, 0 to 3; others are dropped) of a vector's value and that
/// component's curve index. The curve index counts the action's own curves (5303); -1 is none.
///
/// Provisional (the action's update is not traced): a parameter with a curve takes the
/// curve's value at the cutscene's time, else its own; the effect is on from the action's
/// start to its end, and its clock (`effect_time`, which `da_colorfade` fades by) runs from
/// the action's start. An action carried by a camera actor (one a cut makes the view) is
/// taken to run only while that camera is the view: the files give each camera its own
/// `da_dof` over the whole cutscene (25796.cut's two cameras focus at 4 and at 7), and keep
/// the timed ones (the opening and closing fades) on the `MASTER` actor.
#[derive(Clone, Debug, PartialEq)]
pub struct FrameEffectTrack {
    /// The id (5201) of the actor that carries it.
    pub actor: i64,
    pub start: f32,
    pub end: f32,
    pub effect: String,
    pub parameters: Vec<FrameEffectParameter>,
}

/// One value a [`FrameEffectTrack`] sets.
#[derive(Clone, Debug, PartialEq)]
pub struct FrameEffectParameter {
    pub name: String,
    /// Which component of a vector parameter (0 to 3); `None` for a float.
    pub component: Option<u8>,
    /// Its value over the cutscene's time: constant without a curve.
    pub value: Curve,
}

/// A frame buffer effect as it stands at a time of the cutscene.
#[derive(Clone, Debug, PartialEq)]
pub struct FrameEffectState {
    pub effect: String,
    /// Seconds since the effect's action started.
    pub time: f32,
    /// Each parameter set, by name (as the file spells it); a float is in `[0]`.
    pub parameters: Vec<(String, [f32; 4])>,
}

impl FrameEffectState {
    /// Parameter `name` (names compare without case).
    pub fn parameter(&self, name: &str) -> Option<[f32; 4]> {
        self.parameters
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| *v)
    }
}

impl FrameEffectTrack {
    /// The effect `t` seconds into the cutscene, while the action runs.
    pub fn at(&self, t: f32) -> Option<FrameEffectState> {
        if !(self.start..=self.end).contains(&t) {
            return None;
        }
        let mut parameters: Vec<(String, [f32; 4])> = Vec::new();
        for p in &self.parameters {
            let i = match parameters
                .iter()
                .position(|(n, _)| n.eq_ignore_ascii_case(&p.name))
            {
                Some(i) => i,
                None => {
                    parameters.push((p.name.clone(), [0.0; 4]));
                    parameters.len() - 1
                }
            };
            parameters[i].1[usize::from(p.component.unwrap_or(0))] = p.value.at(t);
        }
        Some(FrameEffectState {
            effect: self.effect.clone(),
            time: t - self.start,
            parameters,
        })
    }
}

/// What a cutscene does to its actors.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tracks {
    pub actors: Vec<Actor>,
    /// The cutscene's animation tree (5007, `bdccs_intro.cub`).
    pub tree: Option<String>,
    /// The stage file it was made on (5017, cutscene +0x48): only a cutscene with one is
    /// placed on a conversation's stage (0x7d2adf, 0x7cdc46).
    pub stage_file: Option<String>,
    /// Its length (5000, seconds; 0 when the file gives none).
    pub seconds: f32,
    /// Camera cuts (kind-11 tracks): from this time (seconds), the actor with this id is the
    /// camera. By time; on a tie, the file's order.
    pub cuts: Vec<(f32, i64)>,
    /// The frame buffer effects it runs (kind-19 tracks, whichever actor carries them), in
    /// the file's order.
    pub frame_effects: Vec<FrameEffectTrack>,
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
        Self::from_cut(g, g.root)
    }

    /// The tracks of a cutscene struct (`CUT `): a `.cut` file's root, or a conversation
    /// line's own cutscene (12211).
    pub fn from_cut(g: &Gff4, root: StructId) -> Self {
        let text = |l| {
            g.field(root, l)
                .and_then(Value::as_str)
                .map(|s| s.trim_end_matches('\0').trim().to_string())
                .filter(|s| !s.is_empty())
        };
        let mut out = Self {
            tree: text(FIELD_CUT_TREE),
            stage_file: text(FIELD_CUT_STAGE),
            seconds: f32_field(g, root, FIELD_CUT_SECONDS)
                .filter(|s| *s >= 0.0)
                .unwrap_or(0.0)
                .min(crate::cutscene::MAX_SECONDS),
            ..Self::default()
        };
        for a in structs(g, root, FIELD_ACTORS) {
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
                tag: text(FIELD_ACTOR_TAG),
                end_position: match g.field(a, FIELD_ACTOR_END_POSITION) {
                    Some(Value::Vec3(v)) if v.iter().all(|x| x.is_finite()) => *v,
                    _ => [0.0; 3],
                },
                end_rotation: match g.field(a, FIELD_ACTOR_END_ROTATION) {
                    Some(Value::Quat(q) | Value::Vec4(q)) if q.iter().all(|x| x.is_finite()) => *q,
                    _ => [0.0, 0.0, 0.0, 1.0],
                },
                snaps: false,
                pose: g
                    .field(a, FIELD_ACTOR_POSE)
                    .and_then(Value::as_i64)
                    .filter(|&p| p >= 0),
                motion: Vec::new(),
                turn: Vec::new(),
                fov: Vec::new(),
                animations: Vec::new(),
                places: Vec::new(),
                camera_jumps: Vec::new(),
                links: Vec::new(),
                head_tracks: Vec::new(),
                visibility: Vec::new(),
                weapon_draws: Vec::new(),
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
                    TRACK_JUMP_TO_CAMERA => {
                        let camera = g
                            .field(tr, FIELD_PLACE_NAME)
                            .and_then(Value::as_str)
                            .map(|s| s.trim_end_matches('\0').to_string())
                            .unwrap_or_default();
                        let by_place = g
                            .field(tr, FIELD_JUMP_BY_PLACE)
                            .and_then(Value::as_u64)
                            .is_some_and(|v| v != 0);
                        actor.camera_jumps.push(CameraJump {
                            start,
                            end,
                            camera,
                            by_place,
                        });
                    }
                    TRACK_LINK_TO_ACTOR => {
                        let int = |l| g.field(tr, l).and_then(Value::as_i64);
                        let flag = |l| {
                            g.field(tr, l)
                                .and_then(Value::as_u64)
                                .is_some_and(|v| v != 0)
                        };
                        if let Some(target) = int(FIELD_LINK_TARGET) {
                            let index = int(FIELD_LINK_NODE).unwrap_or(-1);
                            let node = match index {
                                i if i < 0 => LinkNode::Root,
                                i if flag(FIELD_LINK_BY_BONE) => {
                                    u32::try_from(i).map_or(LinkNode::Root, LinkNode::Bone)
                                }
                                i => LinkNode::Table(i),
                            };
                            actor.links.push(ActorLink {
                                start,
                                end,
                                target,
                                node,
                                keep_offset: flag(FIELD_LINK_KEEP_OFFSET),
                            });
                        }
                    }
                    TRACK_CHANGE_VISIBILITY => {
                        let shown = g
                            .field(tr, FIELD_VISIBLE)
                            .and_then(Value::as_u64)
                            .is_some_and(|v| v != 0);
                        actor.visibility.push((start, shown));
                    }
                    TRACK_DRAW_WEAPON => {
                        let flag = |l| {
                            g.field(tr, l)
                                .and_then(Value::as_u64)
                                .is_some_and(|v| v != 0)
                        };
                        actor.weapon_draws.push(WeaponDraw {
                            start,
                            main: flag(FIELD_DRAW_MAIN),
                            off: flag(FIELD_DRAW_OFF),
                        });
                    }
                    TRACK_HEAD_TRACKING => {
                        actor.head_tracks.push(HeadTrack {
                            start,
                            end,
                            target: g.field(tr, FIELD_LOOK_TARGET).and_then(Value::as_i64),
                            speed: g
                                .field(tr, FIELD_LOOK_SPEED)
                                .and_then(Value::as_f64)
                                .map_or(1.0, |v| v as f32),
                        });
                    }
                    TRACK_FRAME_EFFECT => {
                        let effect = g
                            .field(tr, FIELD_EFFECT_NAME)
                            .and_then(Value::as_str)
                            .map(|s| s.trim_end_matches('\0').trim().to_string())
                            .unwrap_or_default();
                        if effect.is_empty() {
                            continue;
                        }
                        let own = curves().curves;
                        let mut parameters = Vec::new();
                        for p in structs(g, tr, FIELD_EFFECT_PARAMETERS) {
                            let Some(name) = g
                                .field(p, FIELD_PARAMETER_NAME)
                                .and_then(Value::as_str)
                                .map(|s| s.trim_end_matches('\0').trim().to_string())
                                .filter(|s| !s.is_empty())
                            else {
                                continue;
                            };
                            let int = |l| g.field(p, l).and_then(Value::as_i64).unwrap_or(-1);
                            let component = match int(FIELD_PARAMETER_COMPONENT) {
                                c @ 0..=3 => Some(c as u8),
                                c if c < 0 => None,
                                _ => continue,
                            };
                            let value = usize::try_from(int(FIELD_PARAMETER_CURVE))
                                .ok()
                                .and_then(|i| own.get(i).cloned())
                                .unwrap_or_else(|| {
                                    Curve::constant(
                                        f32_field(g, p, FIELD_PARAMETER_VALUE).unwrap_or(0.0),
                                    )
                                });
                            parameters.push(FrameEffectParameter {
                                name,
                                component,
                                value,
                            });
                        }
                        out.frame_effects.push(FrameEffectTrack {
                            actor: actor.id,
                            start,
                            end,
                            effect,
                            parameters,
                        });
                    }
                    TRACK_SNAP_TO_WALKMESH => actor.snaps = true,
                    TRACK_JUMP_TO_PLACE => {
                        let text = |l| {
                            g.field(tr, l)
                                .and_then(Value::as_str)
                                .map(|s| s.trim_end_matches('\0').trim().to_string())
                                .filter(|s| !s.is_empty())
                        };
                        if let Some(place) = text(FIELD_PLACE_NAME) {
                            actor.places.push(PlaceTrack {
                                start,
                                end,
                                place,
                                look_at: text(FIELD_PLACE_LOOK_AT),
                            });
                        }
                    }
                    TRACK_PLAY_ANIMATION => {
                        let text = |l| {
                            g.field(tr, l)
                                .and_then(Value::as_str)
                                .map(|s| s.trim_end_matches('\0').trim().to_string())
                                .unwrap_or_default()
                        };
                        let node = text(FIELD_ANIMATION);
                        let tree = Some(text(FIELD_ANIMATION_TREE))
                            .filter(|t| !t.is_empty())
                            .or_else(|| out.tree.clone())
                            .unwrap_or_default();
                        if !node.is_empty() {
                            actor.animations.push(AnimationTrack {
                                start,
                                end,
                                node,
                                tree,
                                speed: f32_field(g, tr, FIELD_ANIMATION_SPEED).unwrap_or(1.0),
                                offset: f32_field(g, tr, FIELD_ANIMATION_OFFSET).unwrap_or(0.0),
                                root_positions: float_keys(g, tr, FIELD_ANIMATION_ROOT_POSITIONS),
                                root_rotations: float_keys(g, tr, FIELD_ANIMATION_ROOT_ROTATIONS),
                                root_held: g
                                    .field(tr, FIELD_ANIMATION_ROOT_HELD)
                                    .and_then(Value::as_i64)
                                    .is_some_and(|v| v != 0),
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

    /// The frame buffer effects running `t` seconds into the cutscene: those of actors that
    /// are never the camera, in the file's order, then the camera's own (see
    /// [`FrameEffectTrack`]).
    pub fn frame_effects_at(&self, t: f32) -> Vec<FrameEffectState> {
        let view = self.cuts.iter().rev().find(|(at, _)| *at <= t).map(|c| c.1);
        let is_camera = |id: i64| self.cuts.iter().any(|c| c.1 == id);
        let running = |own: bool| {
            self.frame_effects
                .iter()
                .filter(move |e| {
                    if own {
                        Some(e.actor) == view
                    } else {
                        !is_camera(e.actor)
                    }
                })
                .filter_map(move |e| e.at(t))
        };
        running(false).chain(running(true)).collect()
    }

    /// The camera actor `t` seconds into the cutscene: the one the latest cut that has
    /// happened names, if there is one and it is among the actors.
    pub fn camera_actor(&self, t: f32) -> Option<&Actor> {
        let id = self.cuts.iter().rev().find(|(at, _)| *at <= t)?.1;
        self.actors.iter().find(|a| a.id == id)
    }

    /// The camera's world pose `t` seconds into the cutscene, once a cut has made one: the
    /// camera actor's world pose (see [`Tracks::world_pose`]; the camera reads the actor's
    /// world position and rotation, 0x7dd6b8 -> 0x7cdd30 and 0x7cdea0). `None` while it
    /// jumps to a camera the stage does not have.
    pub fn camera_at(&self, t: f32, placement: Placement, find: &Finders) -> Option<Pose> {
        let id = self.cuts.iter().rev().find(|(at, _)| *at <= t)?.1;
        let i = self.actors.iter().position(|a| a.id == id)?;
        self.world_pose(i, t, placement, find)
    }

    /// Where actor `i` is in the world `t` seconds into the cutscene: on the stage camera a
    /// jump puts it on (see [`CameraJump`]); carried by the node of another actor a link
    /// holds it to (see [`ActorLink`]; `find.root` gives where a drawn actor is, and an
    /// actor not drawn is where its own tracks put it); else where its own tracks put it
    /// ([`Actor::stage_pose_at`]) placed as the cutscene is (its tracks set its world pose
    /// through the cutscene's placement, 0x7d0660 and 0x7cf6b0). `None` for an actor it does
    /// not have or a jump to a camera the stage does not have.
    pub fn world_pose(
        &self,
        i: usize,
        t: f32,
        placement: Placement,
        find: &Finders,
    ) -> Option<Pose> {
        self.world_pose_within(i, t, placement, find, MAX_LINK_DEPTH)
    }

    fn world_pose_within(
        &self,
        i: usize,
        t: f32,
        placement: Placement,
        find: &Finders,
        depth: u32,
    ) -> Option<Pose> {
        let actor = self.actors.get(i)?;
        if let Some(j) = actor
            .camera_jumps
            .iter()
            .rev()
            .find(|j| j.start <= t && t <= j.end)
        {
            return (find.camera)(&j.camera, j.by_place).map(|p| placement.pose(p));
        }
        let own = actor.stage_pose_at(t, find.place);
        if let Some(link) = actor.link_at(t)
            && depth > 0
            && let Some(target) = self.actors.iter().position(|a| a.id == link.target)
            && target != i
        {
            let node = (find.root)(target).or_else(|| {
                self.world_pose_within(target, t, placement, find, depth - 1)
                    .map(|p| (p.position, p.rotation))
            });
            if let Some((at, turn)) = node {
                let turn = normalize(turn);
                let (offset, own_turn) = if link.keep_offset {
                    actor.link_offset(&own)
                } else {
                    ([0.0; 3], [0.0, 0.0, 0.0, 1.0])
                };
                let o = rotate(turn, offset);
                return Some(Pose {
                    position: [at[0] + o[0], at[1] + o[1], at[2] + o[2]],
                    rotation: mul(turn, own_turn),
                    fov: own.fov,
                });
            }
        }
        Some(placement.pose(own))
    }
}

/// What places a cutscene's actors besides their tracks: its stage's cameras and places,
/// and where its actors are drawn.
pub struct Finders<'a> {
    pub camera: &'a StageCameraLookup<'a>,
    pub place: &'a PlaceLookup<'a>,
    pub root: &'a RootLookup<'a>,
}

impl Finders<'_> {
    /// Nothing besides the tracks: no stage, nothing drawn.
    pub const NONE: Finders<'static> = Finders {
        camera: &|_, _| None,
        place: &|_| None,
        root: &|_| None,
    };
}

fn f32_field(g: &Gff4, id: StructId, label: u32) -> Option<f32> {
    g.field(id, label)
        .and_then(Value::as_f64)
        .map(|v| v as f32)
        .filter(|v| v.is_finite())
}

/// A list of floats read as keys of `N` floats each (a root motion list of an animation
/// track); keys with a value that is not finite, and a ragged tail, are left out.
fn float_keys<const N: usize>(g: &Gff4, id: StructId, label: u32) -> Vec<[f32; N]> {
    let list = match g.field(id, label) {
        Some(Value::List(Some(l))) => Some(l),
        Some(Value::Generic(slot)) => match &slot.value {
            Some(Value::List(Some(l))) => Some(l),
            _ => None,
        },
        _ => None,
    };
    let Some(thedas_formats::gff4::List::F32(v)) = list else {
        return Vec::new();
    };
    v.chunks_exact(N)
        .filter(|k| k.iter().all(|x| x.is_finite()))
        .map(|k| std::array::from_fn(|i| k[i]))
        .collect()
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

/// The inverse of the unit quaternion `q` (0x4cbcb0).
fn conjugate(q: [f32; 4]) -> [f32; 4] {
    [-q[0], -q[1], -q[2], q[3]]
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

    /// A cutscene whose `MASTER` actor (id 0) fades in from black over its first 3 s (a
    /// timed action, 0 to 3) and whose two cameras (ids 7 and 9; the view from 0 s and from
    /// 2 s) each carry a `da_dof` for the whole cutscene: camera 7's focus distance follows
    /// a curve from 1 to 5 over 4 s, camera 9's is 7.
    fn cut_with_effects() -> Gff4 {
        let text = |s: &str| Value::String(Some(format!("{s}\0")));
        let actor = |id: i32, tracks: &[usize]| StructData {
            template: 1,
            values: vec![
                Value::I32(id),
                Value::List(Some(List::Generic(
                    tracks.iter().map(|&t| slot(t)).collect(),
                ))),
            ],
        };
        let effect = |name: &str, end: f32, curves: &[usize], parameters: &[usize]| StructData {
            template: 2,
            values: vec![
                Value::U8(19),
                Value::F32(0.0),
                Value::F32(end),
                if curves.is_empty() {
                    Value::List(None)
                } else {
                    list(curves)
                },
                Value::Absent,
                text(name),
                list(parameters),
            ],
        };
        let cut = |at: f32, camera: i32| StructData {
            template: 2,
            values: vec![
                Value::U8(11),
                Value::F32(at),
                Value::F32(at),
                Value::List(None),
                Value::I32(camera),
                Value::Absent,
                Value::Absent,
            ],
        };
        let parameter = |name: &str, value: f32, curve: i8, component: i8| StructData {
            template: 3,
            values: vec![
                text(name),
                Value::F32(value),
                Value::I8(curve),
                Value::I8(component),
            ],
        };
        let vert = |t: f32, v: f32| StructData {
            template: 5,
            values: vec![Value::F32(t), Value::F32(v)],
        };
        Gff4 {
            version: Version::V40,
            platform: FourCC::new(b"PC  "),
            file_type: FourCC::new(b"CUT "),
            file_version: FourCC::new(b"V1.0"),
            templates: vec![
                template(b"CUT ", &[FIELD_ACTORS]),
                template(b"ACTR", &[FIELD_ACTOR_ID, FIELD_ACTOR_TRACKS]),
                template(
                    b"AFBE",
                    &[
                        FIELD_TRACK_KIND,
                        FIELD_TRACK_START,
                        FIELD_TRACK_END,
                        FIELD_TRACK_CURVES,
                        FIELD_CAMERA_ACTOR,
                        FIELD_EFFECT_NAME,
                        FIELD_EFFECT_PARAMETERS,
                    ],
                ),
                template(
                    b"FBEP",
                    &[
                        FIELD_PARAMETER_NAME,
                        FIELD_PARAMETER_VALUE,
                        FIELD_PARAMETER_CURVE,
                        FIELD_PARAMETER_COMPONENT,
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
                // 0: root; 1-3: the master and the two cameras
                StructData {
                    template: 0,
                    values: vec![list(&[1, 2, 3])],
                },
                actor(0, &[4, 5, 6]),
                actor(7, &[7]),
                actor(9, &[8]),
                // 4: the fade; 5, 6: the cuts
                effect("da_colorfade", 3.0, &[], &[9, 10, 11]),
                cut(0.0, 7),
                cut(2.0, 9),
                // 7, 8: the cameras' depth of field
                effect("da_dof", 1e10, &[14], &[12]),
                effect("da_dof", 1e10, &[], &[13]),
                // 9-11: the fade's parameters (a vector's alpha, a float)
                parameter("StartColour", 1.0, -1, 3),
                parameter("EndColour", 0.0, -1, 3),
                parameter("TimeToFade", 1.2, -1, -1),
                // 12, 13: the focus distances
                parameter("FocusDistance", 2.0, 0, -1),
                parameter("FocusDistance", 7.0, -1, -1),
                // 14: the curve, 1 to 5 over 4 s
                StructData {
                    template: 4,
                    values: vec![Value::F32(2.0), list(&[15, 16]), Value::List(None)],
                },
                vert(0.0, 1.0),
                vert(4.0, 5.0),
            ],
            root: StructId(0),
            shared_strings: None,
        }
    }

    #[test]
    fn frame_effects_run_by_their_span_or_with_their_camera() {
        let t = Tracks::from_gff4(&cut_with_effects());
        assert_eq!(t.frame_effects.len(), 3);
        // 1 s in: the master's fade, then camera 7's depth of field on its curve.
        let at = t.frame_effects_at(1.0);
        let names: Vec<&str> = at.iter().map(|e| e.effect.as_str()).collect();
        assert_eq!(names, ["da_colorfade", "da_dof"]);
        assert_eq!(at[0].time, 1.0);
        assert_eq!(at[0].parameter("startcolour"), Some([0.0, 0.0, 0.0, 1.0]));
        assert_eq!(at[0].parameter("TimeToFade"), Some([1.2, 0.0, 0.0, 0.0]));
        assert_eq!(at[1].parameter("FocusDistance"), Some([2.0, 0.0, 0.0, 0.0]));
        // 2.5 s in: camera 9 is the view, with its own.
        let at = t.frame_effects_at(2.5);
        assert_eq!(at.len(), 2);
        assert_eq!(at[1].parameter("FocusDistance"), Some([7.0, 0.0, 0.0, 0.0]));
        // 3.5 s in: the fade's action is over.
        let at = t.frame_effects_at(3.5);
        assert_eq!(at.len(), 1);
        assert_eq!(at[0].effect, "da_dof");
    }

    #[test]
    fn plays_the_camera_actor_the_cuts_name() {
        let t = Tracks::from_gff4(&cut());
        assert_eq!(t.cuts, [(0.5, 7)]);
        assert_eq!(t.actors.len(), 1);
        let none = Finders::NONE;
        assert!(t.camera_at(0.25, Placement::NONE, &none).is_none());
        let p = t.camera_at(1.0, Placement::NONE, &none).unwrap();
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
        // Placed as the cutscene is: half a turn about Z, then moved.
        let half = Placement {
            position: [1.0, 2.0, 3.0],
            rotation: [0.0, 0.0, 1.0, 0.0],
        };
        let q = t.camera_at(1.0, half, &none).unwrap();
        assert!(
            (q.position[0] + 99.0).abs() < 1e-3
                && (q.position[1] + 3.0).abs() < 1e-3
                && (q.position[2] - 5.0).abs() < 1e-3,
            "{:?}",
            q.position
        );
        // A jump to a stage camera wins over the curves while it plays, placed as the
        // cutscene is; a camera the stage lacks gives none.
        let mut jumps = t.clone();
        jumps.actors[0].camera_jumps.push(CameraJump {
            start: 0.75,
            end: 1.5,
            camera: "wide".into(),
            by_place: false,
        });
        let cameras = |name: &str, by_place: bool| {
            (name == "wide" && !by_place).then_some(Pose {
                position: [0.0, 1.0, 10.0],
                rotation: [0.0, 0.0, 0.0, 1.0],
                fov: 45.0,
            })
        };
        let stage = Finders {
            camera: &cameras,
            ..Finders::NONE
        };
        let j = jumps.camera_at(1.0, half, &stage).unwrap();
        assert!(
            (j.position[0] - 1.0).abs() < 1e-4
                && (j.position[1] - 1.0).abs() < 1e-4
                && (j.position[2] - 13.0).abs() < 1e-4,
            "{:?}",
            j.position
        );
        assert_eq!(j.rotation, half.rotation);
        assert_eq!(j.fov, 45.0);
        assert!(jumps.camera_at(1.0, half, &none).is_none());
        assert_eq!(
            jumps.camera_at(1.6, half, &stage),
            t.camera_at(1.6, half, &none)
        );
        // Other files have none.
        let mut other = cut();
        other.file_type = FourCC::new(b"CONV");
        assert_eq!(Tracks::from_gff4(&other), Tracks::default());
        assert_eq!(Tracks::parse(b"junk"), Tracks::default());
    }

    /// `cut()` with link tracks (kind 27) on its camera: (target 5630, node 5631, bone 5636,
    /// keep 5637) each.
    fn cut_with_links(links: &[(i32, i32, u8, u8)]) -> Gff4 {
        let mut g = cut();
        let tid = g.templates.len() as u32;
        g.templates.push(template(
            b"ALTA",
            &[
                FIELD_TRACK_KIND,
                FIELD_TRACK_START,
                FIELD_TRACK_END,
                FIELD_LINK_TARGET,
                FIELD_LINK_NODE,
                FIELD_LINK_BY_BONE,
                FIELD_LINK_KEEP_OFFSET,
            ],
        ));
        for &(target, node, bone, keep) in links {
            let id = g.structs.len();
            g.structs.push(StructData {
                template: tid,
                values: vec![
                    Value::U8(27),
                    Value::F32(0.75),
                    Value::F32(1.5),
                    Value::I32(target),
                    Value::I32(node),
                    Value::U8(bone),
                    Value::U8(keep),
                ],
            });
            let Value::List(Some(List::Generic(tracks))) = &mut g.structs[1].values[4] else {
                unreachable!()
            };
            tracks.push(slot(id));
        }
        g
    }

    fn near(a: [f32; 3], b: [f32; 3]) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-4)
    }

    #[test]
    fn reads_links_to_actors() {
        let t = Tracks::from_gff4(&cut_with_links(&[
            (9, -1, 0, 255),
            (9, 4, 255, 0),
            (9, 14, 0, 0),
        ]));
        let links = &t.actors[0].links;
        assert_eq!(links.len(), 3);
        assert_eq!(
            links[0],
            ActorLink {
                start: 0.75,
                end: 1.5,
                target: 9,
                node: LinkNode::Root,
                keep_offset: true,
            }
        );
        assert_eq!(links[1].node, LinkNode::Bone(4));
        assert!(!links[1].keep_offset);
        assert_eq!(links[2].node, LinkNode::Table(14));
        // The last link holding wins; it holds a hair before its start and not at its end.
        assert_eq!(t.actors[0].link_at(1.0), Some(&links[2]));
        assert!(t.actors[0].link_at(0.75 - 5e-5).is_some());
        assert!(t.actors[0].link_at(0.7).is_none());
        assert!(t.actors[0].link_at(1.5).is_none());
    }

    /// The camera of `cut()` (id 7) linked to an actor (id 9) standing at (10, 20, 0),
    /// unturned, from 0.75 s to 1.5 s.
    fn linked(node: LinkNode, keep_offset: bool) -> Tracks {
        let mut t = Tracks::from_gff4(&cut());
        let mut target = t.actors[0].clone();
        target.id = 9;
        target.name = "OWNER".into();
        target.position = [10.0, 20.0, 0.0];
        target.rotation = [0.0, 0.0, 0.0, 1.0];
        target.motion.clear();
        target.turn.clear();
        t.actors.push(target);
        t.actors[0].links.push(ActorLink {
            start: 0.75,
            end: 1.5,
            target: 9,
            node,
            keep_offset,
        });
        t
    }

    #[test]
    fn carries_a_linked_camera_on_its_node() {
        let t = linked(LinkNode::Root, true);
        let none = Finders::NONE;
        let h = std::f32::consts::FRAC_1_SQRT_2;
        let quarter = [0.0, 0.0, h, h];
        // At 1 s the camera's own curves are x 5, z 2, turned 90° about Z: from its base
        // (100, 0, 0) turned a quarter, it stands at (100, 5, 2). Linked, those curves are
        // its offset from the node, in the node's frame: (10, 20, 0) + (5, 0, 2).
        let p = t.camera_at(1.0, Placement::NONE, &none).unwrap();
        assert!(near(p.position, [15.0, 20.0, 2.0]), "{:?}", p.position);
        assert!(near(rotate(p.rotation, [0.0, 1.0, 0.0]), [-1.0, 0.0, 0.0]));
        assert_eq!(p.fov, 30.0);
        // The node is placed as the cutscene is, and the offset turns with it.
        let half = Placement {
            position: [1.0, 2.0, 3.0],
            rotation: [0.0, 0.0, 1.0, 0.0],
        };
        let q = t.camera_at(1.0, half, &none).unwrap();
        assert!(near(q.position, [-14.0, -18.0, 5.0]), "{:?}", q.position);
        assert!(near(rotate(q.rotation, [0.0, 1.0, 0.0]), [1.0, 0.0, 0.0]));
        // Before the link and after its end, the camera's own tracks place it.
        let own = Tracks::from_gff4(&cut());
        for at in [0.6, 1.5, 1.6] {
            assert_eq!(
                t.camera_at(at, half, &none),
                own.camera_at(at, half, &none),
                "{at}"
            );
        }
        // A drawn actor: carried by its root as drawn, whatever node the link names.
        let drawn = |i: usize| (i == 1).then_some(([1.0, 1.0, 1.0], quarter));
        let find = Finders {
            root: &drawn,
            ..Finders::NONE
        };
        let b = t.camera_at(1.0, half, &find).unwrap();
        assert!(near(b.position, [1.0, 6.0, 3.0]), "{:?}", b.position);
        for node in [LinkNode::Bone(4), LinkNode::Table(14)] {
            assert_eq!(linked(node, true).camera_at(1.0, half, &find), Some(b));
        }
        // Without keeping its offset, the camera is the node's place and turn.
        let snap = linked(LinkNode::Bone(4), false);
        let s = snap.camera_at(1.0, half, &find).unwrap();
        assert_eq!(s.position, [1.0, 1.0, 1.0]);
        assert!(near(rotate(s.rotation, [0.0, 1.0, 0.0]), [-1.0, 0.0, 0.0]));
        assert_eq!(s.fov, 30.0);
    }

    #[test]
    fn follows_links_of_links_and_ignores_bad_ones() {
        // A camera (id 3, unturned at the origin, offset (0, -2, 1)) linked to the camera
        // linked to the actor: carried along both.
        let mut t = linked(LinkNode::Root, true);
        let mut outer = t.actors[1].clone();
        outer.id = 3;
        outer.position = [0.0; 3];
        outer.motion = vec![CurveTrack {
            start: 0.0,
            end: f32::INFINITY,
            curves: vec![
                Curve::constant(0.0),
                Curve::constant(-2.0),
                Curve::constant(1.0),
            ],
        }];
        outer.links.push(ActorLink {
            start: 0.0,
            end: 10.0,
            target: 7,
            node: LinkNode::Root,
            keep_offset: true,
        });
        t.actors.push(outer);
        t.cuts = vec![(0.0, 3)];
        // The inner camera stands at (15, 20, 2) looking along -X; (0, -2, 1) in its
        // frame is (2, 0, 1).
        let p = t.camera_at(1.0, Placement::NONE, &Finders::NONE).unwrap();
        assert!(near(p.position, [17.0, 20.0, 3.0]), "{:?}", p.position);
        // Links in a ring end where the depth runs out; a link to itself or to an actor
        // the cutscene lacks is ignored.
        let mut ring = t.clone();
        ring.actors[1].links.push(ActorLink {
            start: 0.0,
            end: 10.0,
            target: 3,
            node: LinkNode::Root,
            keep_offset: true,
        });
        assert!(
            ring.camera_at(1.0, Placement::NONE, &Finders::NONE)
                .is_some()
        );
        for target in [3, 42] {
            let mut bad = t.clone();
            bad.actors[2].links[0].target = target;
            let own = bad.actors[2].pose_at(1.0);
            assert_eq!(
                bad.camera_at(1.0, Placement::NONE, &Finders::NONE),
                Some(own),
                "{target}"
            );
        }
    }

    /// An actor's pose number names its base pose's node in the cutscene's tree.
    #[test]
    fn an_actor_looks_as_its_last_started_head_tracking_action_says() {
        let mut a = Actor {
            id: 1,
            name: "a".into(),
            creature: None,
            tag: None,
            end_position: [0.0; 3],
            end_rotation: [0.0, 0.0, 0.0, 1.0],
            snaps: false,
            pose: None,
            position: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            motion: Vec::new(),
            turn: Vec::new(),
            fov: Vec::new(),
            animations: Vec::new(),
            places: Vec::new(),
            camera_jumps: Vec::new(),
            links: Vec::new(),
            head_tracks: Vec::new(),
            visibility: Vec::new(),
            weapon_draws: Vec::new(),
        };
        assert_eq!(a.head_track_at(1.0), None);
        let track = |start: f32, end: f32, target: i64| HeadTrack {
            start,
            end,
            target: Some(target),
            speed: 0.5,
        };
        a.head_tracks = vec![track(4.0, 6.0, 3), track(1.0, 2.0, 2)];
        assert_eq!(a.head_track_at(0.5), None);
        assert_eq!(a.head_track_at(1.0).and_then(|h| h.target), Some(2));
        // Held after its end, until the next starts.
        assert_eq!(a.head_track_at(3.0).and_then(|h| h.target), Some(2));
        assert_eq!(a.head_track_at(5.0).and_then(|h| h.target), Some(3));
        assert_eq!(a.head_track_at(9.0).and_then(|h| h.target), Some(3));
    }

    #[test]
    fn visibility_and_weapon_draws_hold_from_their_start() {
        let mut a = Actor {
            id: 1,
            name: "a".into(),
            creature: None,
            tag: None,
            end_position: [0.0; 3],
            end_rotation: [0.0, 0.0, 0.0, 1.0],
            snaps: false,
            pose: None,
            position: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            motion: Vec::new(),
            turn: Vec::new(),
            fov: Vec::new(),
            animations: Vec::new(),
            places: Vec::new(),
            camera_jumps: Vec::new(),
            links: Vec::new(),
            head_tracks: Vec::new(),
            visibility: vec![(32.2, true), (0.0, false)],
            weapon_draws: Vec::new(),
        };
        assert_eq!(a.visible_at(1.0), Some(false));
        assert_eq!(a.visible_at(40.0), Some(true));
        assert_eq!(a.weapons_at(40.0), None);
        let draw = |start: f32, main: bool, off: bool| WeaponDraw { start, main, off };
        a.weapon_draws = vec![draw(5.0, true, true), draw(9.0, false, false)];
        assert_eq!(a.weapons_at(1.0), None);
        assert_eq!(a.weapons_at(6.0), Some((true, Some(true))));
        // Sheathing the main hand's leaves the off hand's where an earlier action put it.
        assert_eq!(a.weapons_at(10.0), Some((false, Some(true))));
        a.visibility.clear();
        assert_eq!(a.visible_at(1.0), None);
    }

    #[test]
    fn an_actors_pose_number_names_its_base_pose() {
        let mut a = Actor {
            id: 1,
            name: "a".into(),
            creature: None,
            tag: None,
            end_position: [0.0; 3],
            end_rotation: [0.0, 0.0, 0.0, 1.0],
            snaps: false,
            pose: Some(17),
            position: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
            motion: Vec::new(),
            turn: Vec::new(),
            fov: Vec::new(),
            animations: Vec::new(),
            places: Vec::new(),
            camera_jumps: Vec::new(),
            links: Vec::new(),
            head_tracks: Vec::new(),
            visibility: Vec::new(),
            weapon_draws: Vec::new(),
        };
        assert_eq!(a.pose_node().as_deref(), Some("pose_17"));
        a.pose = Some(0);
        assert_eq!(a.pose_node().as_deref(), Some("pose_0"));
        a.pose = None;
        assert_eq!(a.pose_node(), None);
    }

    /// An animation track's root motion moves and turns its actor, and stays after the
    /// track ends (a cutscene's quarter turn keeps the actor turned).
    #[test]
    fn an_animations_root_motion_moves_the_actor_and_stays() {
        let h = std::f32::consts::FRAC_1_SQRT_2;
        let turn = AnimationTrack {
            start: 2.0,
            end: 4.0,
            node: "turn".into(),
            tree: "x.cub".into(),
            speed: 1.0,
            offset: 0.0,
            // One metre along the actor's own +Y over two seconds, and a quarter turn left.
            root_positions: vec![[0.0, 0.0, 0.0, 0.0], [2.0, 0.0, 1.0, 0.0]],
            root_rotations: vec![[0.0, 0.0, 0.0, 0.0, 1.0], [2.0, 0.0, 0.0, h, h]],
            root_held: false,
        };
        assert_eq!(turn.root_at(1.9), None);
        let (p, r) = turn.root_at(3.0).unwrap();
        assert!((p[1] - 0.5).abs() < 1e-5);
        assert!((facing_of(r).unwrap() - facing_of([0.0, 0.0, 0.0, 1.0]).unwrap()).abs() > 1.0);
        // Past its end the last keys hold.
        let (p, r) = turn.root_at(60.0).unwrap();
        assert!((p[1] - 1.0).abs() < 1e-5);
        assert!((r[2] - h).abs() < 1e-5 && (r[3] - h).abs() < 1e-5);
        // A track that holds its actor, and a pose without keys, move nothing.
        let held = AnimationTrack {
            root_held: true,
            ..turn.clone()
        };
        assert_eq!(held.root_at(3.0), None);
        let pose = AnimationTrack {
            root_positions: Vec::new(),
            root_rotations: Vec::new(),
            ..turn.clone()
        };
        assert_eq!(pose.root_at(3.0), None);

        // On an actor that stands at (10, 0, 0) turned a quarter left (its +Y is world -X):
        // the step goes along world -X, and two such tracks turn it half way round.
        let mut a = Actor {
            id: 1,
            name: "a".into(),
            creature: None,
            tag: None,
            end_position: [0.0; 3],
            end_rotation: [0.0, 0.0, 0.0, 1.0],
            snaps: false,
            pose: None,
            position: [10.0, 0.0, 0.0],
            rotation: [0.0, 0.0, h, h],
            motion: Vec::new(),
            turn: Vec::new(),
            fov: Vec::new(),
            animations: vec![turn.clone()],
            places: Vec::new(),
            camera_jumps: Vec::new(),
            links: Vec::new(),
            head_tracks: Vec::new(),
            visibility: Vec::new(),
            weapon_draws: Vec::new(),
        };
        let none: &PlaceLookup = &|_| None;
        let before = a.stage_pose_at(1.0, none);
        assert_eq!(before.position, [10.0, 0.0, 0.0]);
        let after = a.stage_pose_at(9.0, none);
        assert!((after.position[0] - 9.0).abs() < 1e-4 && after.position[1].abs() < 1e-4);
        // A half turn about Z: (0, 0, 1, 0) up to sign.
        assert!(after.rotation[2].abs() > 0.999 && after.rotation[3].abs() < 1e-3);
        a.animations.push(AnimationTrack {
            start: 5.0,
            end: 7.0,
            ..turn
        });
        let twice = a.stage_pose_at(9.0, none);
        // The second step is taken in the frame the first left: along world -Y.
        assert!((twice.position[0] - 9.0).abs() < 1e-4 && (twice.position[1] + 1.0).abs() < 1e-4);
        assert!((twice.rotation[2].abs() - h).abs() < 1e-3);
    }

    #[test]
    fn plays_jumps_and_animations_as_the_tracks_say() {
        let t = Tracks::from_gff4(&cut());
        let mut a = t.actors[0].clone();
        // No motion keys but x's: a jump wins while it plays.
        a.places.push(PlaceTrack {
            start: 0.0,
            end: 1.0,
            place: "spot".into(),
            look_at: Some("there".into()),
        });
        let place = |n: &str| match n {
            "spot" => Some(([1.0, 2.0, 0.0], [0.0, 0.0, 0.0, 1.0])),
            "there" => Some(([1.0, 5.0, 0.0], [0.0, 0.0, 0.0, 1.0])),
            _ => None,
        };
        let p = a.stage_pose_at(0.5, &place);
        assert_eq!(p.position, [1.0, 2.0, 0.0]);
        // Turned towards the other place: +Y, facing 0.
        assert!(facing_of(p.rotation).unwrap().abs() < 1e-3);
        // After it the motion tracks (keyed) place it again.
        assert!((a.stage_pose_at(1.5, &place).position[0] - 100.0).abs() < 1e-3);
        // Placed on a stage turned a quarter: +Y is world -X.
        let at = Placement::facing([10.0, 0.0, 0.0], 90.0).point([0.0, 2.0, 0.0]);
        assert!((at[0] - 8.0).abs() < 1e-4 && at[1].abs() < 1e-4);
        // Animations: a pose holds until the next starts; one running past the end holds.
        let anim = |node: &str, start: f32, end: f32| AnimationTrack {
            start,
            end,
            node: node.into(),
            tree: "x.cub".into(),
            speed: 2.0,
            offset: 1.0,
            root_positions: Vec::new(),
            root_rotations: Vec::new(),
            root_held: false,
        };
        a.animations = vec![
            anim("pose", 0.0, 0.0),
            anim("walk", 2.0, 3.0),
            anim("sit", 4.0, 9.0),
        ];
        let names = |t: f32| -> Vec<&str> {
            a.animations_at(t, 5.0)
                .iter()
                .map(|(l, _)| l.node.as_str())
                .collect()
        };
        assert_eq!(names(1.0), ["pose"]);
        assert_eq!(names(2.5), ["walk"]);
        assert!(names(3.5).is_empty());
        assert_eq!(names(6.0), ["sit"]);
        // Speed and offset; held to the span.
        assert_eq!(a.animations[1].time_at(2.5), 2.0);
        assert_eq!(a.animations[1].time_at(9.0), 3.0);
    }
}
