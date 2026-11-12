//! Animations (GFF4): animation trees (`.anb`, `ANBT`), the general data beside each
//! animation (`.gad`, `GAD `), its events (`.evt`, `EVT `) and the keyframes (`.ani`,
//! `ANI `; see [`Animation`]).
//!
//! Field labels, read from the files:
//! - Tree: 4021 the nodes, 4022 the tree's name (`Staff`, `Bow`, `humanoid`, ...).
//! - Node: 4024 the action (`fire`, `aim`, `attk_v1`, ...), 4025 the animation (`.ani`
//!   resource, absent for a node naming the tree itself), 4026 its playback speed, 4027
//!   whether it plays once (provisional, from the data: 1 on `humanoid.anb`'s enters,
//!   exits, twitches and deaths, 0 on its loops, pauses and the movement cycles; a
//!   cutscene's `.cub` trees have 0 throughout); 4028 to 4030 are not interpreted
//!   (provisional: 4029/4030 look like tree links).
//! - General data: 4007 the animation's name, 4009 its length in seconds, 4010 its speed.
//! - Events: 4020 the list; each 4016 its time in seconds and 4017 its kind (provisional:
//!   kind 10 sits where a ranged weapon lets go, kind 1 near a melee hit).

use crate::gff4::Gff4;
use crate::typed::{Doc, TypedResult};

/// One action of an animation tree.
#[derive(Clone, Debug, PartialEq)]
pub struct AnimNode {
    pub action: String,
    /// The `.ani` resource, if the node plays one.
    pub animation: Option<String>,
    pub speed: f32,
    /// Field 4027: the animation plays once (provisional; see the module docs).
    pub once: bool,
}

/// An animation tree (`.anb`): which animation each action plays.
#[derive(Clone, Debug, PartialEq)]
pub struct AnimTree {
    pub name: String,
    pub nodes: Vec<AnimNode>,
}

impl AnimTree {
    pub fn parse(data: &[u8]) -> TypedResult<Self> {
        Self::from_gff(&Gff4::parse(data)?)
    }

    pub fn from_gff(doc: &Gff4) -> TypedResult<Self> {
        let d = Doc::new(doc, "animation tree");
        d.expect_type("ANBT")?;
        let root = doc.root;
        let name = d.opt_str(root, 4022)?.unwrap_or_default();
        let mut nodes = Vec::new();
        for n in d.structs(root, 4021)? {
            nodes.push(AnimNode {
                action: d.opt_str(n, 4024)?.unwrap_or_default(),
                animation: d.opt_str(n, 4025)?.filter(|s| !s.is_empty()),
                speed: d.opt_f32(n, 4026)?.unwrap_or(1.0),
                once: d.opt_i64(n, 4027)?.is_some_and(|v| v != 0),
            });
        }
        Ok(Self { name, nodes })
    }

    /// The first node playing animation `ani` (`.ani` resource; names compare without case).
    pub fn playing(&self, ani: &str) -> Option<&AnimNode> {
        self.nodes.iter().find(|n| {
            n.animation
                .as_deref()
                .is_some_and(|a| a.eq_ignore_ascii_case(ani))
        })
    }

    /// The node for an action (names compare without case).
    pub fn action(&self, action: &str) -> Option<&AnimNode> {
        self.nodes
            .iter()
            .find(|n| n.action.eq_ignore_ascii_case(action))
    }
}

/// The general data beside an animation (`.gad`).
#[derive(Clone, Debug, PartialEq)]
pub struct AnimInfo {
    pub name: String,
    /// Length in seconds at speed 1.
    pub duration: f32,
    pub speed: f32,
}

impl AnimInfo {
    pub fn parse(data: &[u8]) -> TypedResult<Self> {
        Self::from_gff(&Gff4::parse(data)?)
    }

    pub fn from_gff(doc: &Gff4) -> TypedResult<Self> {
        let d = Doc::new(doc, "animation data");
        d.expect_type("GAD ")?;
        let root = doc.root;
        let duration = d.opt_f32(root, 4009)?.unwrap_or(0.0);
        if !duration.is_finite() || duration < 0.0 {
            return Err(crate::typed::TypedError::Invalid(format!(
                "animation data: length {duration} is not a duration"
            )));
        }
        Ok(Self {
            name: d.opt_str(root, 4007)?.unwrap_or_default(),
            duration,
            speed: d.opt_f32(root, 4010)?.unwrap_or(1.0),
        })
    }
}

/// One event of an animation (`.evt`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AnimEvent {
    /// Seconds from the start.
    pub time: f32,
    pub kind: i64,
}

/// The events of an animation (`.evt`), in file order.
pub fn parse_events(data: &[u8]) -> TypedResult<Vec<AnimEvent>> {
    events_from_gff(&Gff4::parse(data)?)
}

pub fn events_from_gff(doc: &Gff4) -> TypedResult<Vec<AnimEvent>> {
    let d = Doc::new(doc, "animation events");
    d.expect_type("EVT ")?;
    let mut out = Vec::new();
    for e in d.structs(doc.root, 4020)? {
        out.push(AnimEvent {
            time: d.opt_f32(e, 4016)?.unwrap_or(0.0),
            kind: d.opt_i64(e, 4017)?.unwrap_or(0),
        });
    }
    Ok(out)
}

/// One event of an animation with its text (field 4019): kind 20 names an FMOD event to
/// play (`glo_fly_wep/...`), kind 14 footstep tags (`|Left`, `|Scuff`). Provisional: read
/// from the files.
#[derive(Clone, Debug, PartialEq)]
pub struct AnimEventText {
    pub time: f32,
    pub kind: i64,
    pub text: Option<String>,
}

/// The events of an animation (`.evt`) with their texts, in file order.
pub fn parse_events_with_text(data: &[u8]) -> TypedResult<Vec<AnimEventText>> {
    events_with_text_from_gff(&Gff4::parse(data)?)
}

pub fn events_with_text_from_gff(doc: &Gff4) -> TypedResult<Vec<AnimEventText>> {
    let d = Doc::new(doc, "animation events");
    d.expect_type("EVT ")?;
    let mut out = Vec::new();
    for e in d.structs(doc.root, 4020)? {
        out.push(AnimEventText {
            time: d.opt_f32(e, 4016)?.unwrap_or(0.0),
            kind: d.opt_i64(e, 4017)?.unwrap_or(0),
            text: d
                .opt_str(e, 4019)?
                .map(|t| t.trim_end_matches('\0').to_string())
                .filter(|t| !t.is_empty()),
        });
    }
    Ok(out)
}

/// The labels of an animation (`.ani`): the root holds 4005 the tracks, 4006 the name and
/// 4009 the length in seconds. A track holds 4000 its target (`<bone>_rotation`,
/// `<bone>_translation`), 4001 its kind and 4004 a struct whose 4004 lists the keys; a key
/// holds 4035 its time (0 to 65535 across the length) and 4036 (to 4038) its value.
mod ani {
    pub const TRACKS: u32 = 4005;
    pub const NAME: u32 = 4006;
    pub const LENGTH: u32 = 4009;
    pub const TARGET: u32 = 4000;
    pub const KIND: u32 = 4001;
    pub const KEYS: u32 = 4004;
    pub const TIME: u32 = 4035;
    pub const VALUE: u32 = 4036;
    pub const Y: u32 = 4037;
    pub const Z: u32 = 4038;
}

/// Track kinds (4001).
pub mod track_kind {
    /// Rotations packed in 32 bits (see [`super::unpack_quat32`]).
    pub const QUAT32: i64 = 2;
    /// Rotations packed in three 16-bit words (see [`super::unpack_quat48`]).
    pub const QUAT48: i64 = 3;
    /// Rotations packed in 64 bits (see [`super::unpack_quat64`]).
    pub const QUAT64: i64 = 4;
    /// Translations as three floats.
    pub const VEC3: i64 = 6;
}

/// The key time that means the end of the animation.
pub const TIME_SCALE: f32 = 65535.0;

/// Which component each unpacked value lands in, by the key's two-bit index: the
/// reconstructed largest component first, then the three stored ones (DAOrigins.exe's table
/// at 0xb265f0).
const ORDER: [[usize; 4]; 4] = [[0, 1, 2, 3], [1, 0, 2, 3], [1, 2, 0, 3], [1, 2, 3, 0]];

/// A unit quaternion from three stored components (each within ±1/√2) and the index of the
/// dropped largest one, which is rebuilt as √(1 − sum of squares). `[x, y, z, w]`.
fn smallest_three(index: u64, a: f32, b: f32, c: f32) -> [f32; 4] {
    let s = 1.0 - (a * a + b * b + c * c);
    let big = if s > 0.0 { s.sqrt() } else { 0.0 };
    let vals = [big, a, b, c];
    let order = ORDER[(index & 3) as usize];
    [
        vals[order[0]],
        vals[order[1]],
        vals[order[2]],
        vals[order[3]],
    ]
}

/// A stored component: `v` in `0..=max` mapped onto ±1/√2.
fn unit(v: u64, max: f32) -> f32 {
    (v as f32 - max * 0.5) * std::f32::consts::SQRT_2 / max
}

/// Unpacks a 64-bit rotation key (DAOrigins.exe 0x811660): bits 0-1 the dropped component's
/// index, 2-21 a 20-bit component, 22-42 and 43-63 two 21-bit components.
pub fn unpack_quat64(v: u64) -> [f32; 4] {
    let a = unit((v >> 43) & 0x1f_ffff, 2_097_151.0);
    let b = unit((v >> 22) & 0x1f_ffff, 2_097_151.0);
    let c = unit((v >> 2) & 0xf_ffff, 1_048_575.0);
    smallest_three(v, a, b, c)
}

/// Unpacks a 32-bit rotation key (DAOrigins.exe 0x811370): bits 0-1 the dropped
/// component's index, then three 10-bit components at bits 22, 12 and 2.
pub fn unpack_quat32(v: u32) -> [f32; 4] {
    let v = v as u64;
    let a = unit((v >> 22) & 0x3ff, 1023.0);
    let b = unit((v >> 12) & 0x3ff, 1023.0);
    let c = unit((v >> 2) & 0x3ff, 1023.0);
    smallest_three(v, a, b, c)
}

/// Unpacks a rotation key of three 16-bit words (DAOrigins.exe 0x811500): the low bits of
/// the first two give the dropped component's index (first word's bit high), the words
/// without them (and all of the third) the components.
pub fn unpack_quat48(w: [u16; 3]) -> [f32; 4] {
    let index = (((w[0] & 1) << 1) | (w[1] & 1)) as u64;
    let a = unit((w[0] & 0xfffe) as u64, 65535.0);
    let b = unit((w[1] & 0xfffe) as u64, 65535.0);
    let c = unit(w[2] as u64, 65535.0);
    smallest_three(index, a, b, c)
}

/// One key: time in seconds and the value (`[x, y, z, w]` for rotations, `[x, y, z, 0]`
/// for translations).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Key {
    pub time: f32,
    pub value: [f32; 4],
}

/// What a track animates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    Rotation,
    Translation,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    /// The bone (lowercase), from the target name without its `_rotation` or
    /// `_translation` suffix.
    pub bone: String,
    pub channel: Channel,
    /// Keys in time order.
    pub keys: Vec<Key>,
}

/// An animation's keyframes (`.ani`).
#[derive(Clone, Debug, PartialEq)]
pub struct Animation {
    pub name: String,
    /// Seconds.
    pub length: f32,
    pub tracks: Vec<Track>,
}

/// Keys are kept to this many per track, far above any shipped animation.
const MAX_KEYS: usize = 1 << 16;

impl Animation {
    pub fn parse(data: &[u8]) -> TypedResult<Self> {
        Self::from_gff(&Gff4::parse(data)?)
    }

    pub fn from_gff(doc: &Gff4) -> TypedResult<Self> {
        use crate::typed::TypedError;
        let d = Doc::new(doc, "animation");
        d.expect_type("ANI ")?;
        let root = doc.root;
        let length = d.opt_f32(root, ani::LENGTH)?.unwrap_or(0.0);
        if !length.is_finite() || length < 0.0 {
            return Err(TypedError::Invalid(format!(
                "animation: length {length} is not a duration"
            )));
        }
        let mut tracks = Vec::new();
        for t in d.structs(root, ani::TRACKS)? {
            let target = d
                .opt_str(t, ani::TARGET)?
                .unwrap_or_default()
                .to_ascii_lowercase();
            let kind = d.opt_i64(t, ani::KIND)?.unwrap_or(-1);
            let (bone, channel) = if let Some(b) = target.strip_suffix("_rotation") {
                (b, Channel::Rotation)
            } else if let Some(b) = target.strip_suffix("_translation") {
                (b, Channel::Translation)
            } else {
                // Other targets (morph weights and the like) are not bone transforms.
                continue;
            };
            let Some(list) = d.opt_struct(t, ani::KEYS)? else {
                continue;
            };
            let raw = d.structs(list, ani::KEYS)?;
            if raw.len() > MAX_KEYS {
                return Err(TypedError::Invalid(format!(
                    "animation: {} keys in {target}",
                    raw.len()
                )));
            }
            let mut keys = Vec::with_capacity(raw.len());
            for k in raw {
                let time = d.opt_i64(k, ani::TIME)?.unwrap_or(0) as f32 / TIME_SCALE * length;
                let int = || -> TypedResult<u64> {
                    match d.get(k, ani::VALUE) {
                        Some(v) => v.as_u64().ok_or_else(|| {
                            TypedError::Invalid(format!("animation: {target} key is not packed"))
                        }),
                        None => Ok(0),
                    }
                };
                let value = match (channel, kind) {
                    (Channel::Rotation, track_kind::QUAT64) => unpack_quat64(int()?),
                    (Channel::Rotation, track_kind::QUAT32) => {
                        let v = int()?;
                        let v = u32::try_from(v).map_err(|_| {
                            TypedError::Invalid(format!("animation: {target} key {v:#x} > 32 bits"))
                        })?;
                        unpack_quat32(v)
                    }
                    (Channel::Rotation, track_kind::QUAT48) => {
                        let word = |l| -> TypedResult<u16> {
                            let v = d.opt_i64(k, l)?.unwrap_or(0);
                            u16::try_from(v).map_err(|_| {
                                TypedError::Invalid(format!("animation: {target} word {v}"))
                            })
                        };
                        unpack_quat48([word(ani::VALUE)?, word(ani::Y)?, word(ani::Z)?])
                    }
                    (Channel::Translation, track_kind::VEC3) => {
                        let f = |l| d.opt_f32(k, l).map(|v| v.unwrap_or(0.0));
                        [f(ani::VALUE)?, f(ani::Y)?, f(ani::Z)?, 0.0]
                    }
                    _ => {
                        return Err(TypedError::Invalid(format!(
                            "animation: {target} has track kind {kind}"
                        )));
                    }
                };
                keys.push(Key { time, value });
            }
            keys.sort_by(|a, b| a.time.total_cmp(&b.time));
            tracks.push(Track {
                bone: bone.to_string(),
                channel,
                keys,
            });
        }
        Ok(Self {
            name: d.opt_str(root, ani::NAME)?.unwrap_or_default(),
            length,
            tracks,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gff4::Value;
    use crate::typed::testutil::{Builder, s, structs};

    #[test]
    fn a_tree_maps_actions_to_animations() {
        let mut b = Builder::new(b"ANBT");
        let fire = b.add(
            b"node",
            vec![
                (4024, s("fire")),
                (4025, s("mh_c.stf_spl_d_cst.ani")),
                (4026, Value::F32(1.0)),
            ],
        );
        let named = b.add(
            b"node",
            vec![(4024, s("Staff")), (4025, Value::String(None))],
        );
        let root = b.add(
            b"tree",
            vec![(4021, structs(&[fire, named])), (4022, s("Staff"))],
        );
        let t = AnimTree::from_gff(&b.finish(root)).unwrap();
        assert_eq!(t.name, "Staff");
        let n = t.action("FIRE").unwrap();
        assert_eq!(n.animation.as_deref(), Some("mh_c.stf_spl_d_cst.ani"));
        assert_eq!(n.speed, 1.0);
        assert_eq!(t.action("Staff").unwrap().animation, None);
        assert!(t.action("aim").is_none());
    }

    #[test]
    fn general_data_gives_the_length_and_rejects_nonsense() {
        let mut b = Builder::new(b"GAD ");
        let root = b.add(
            b"gad ",
            vec![
                (4007, s("fire")),
                (4009, Value::F32(0.433)),
                (4010, Value::F32(1.0)),
            ],
        );
        let g = AnimInfo::from_gff(&b.finish(root)).unwrap();
        assert_eq!((g.name.as_str(), g.duration, g.speed), ("fire", 0.433, 1.0));
        let mut b = Builder::new(b"GAD ");
        let root = b.add(b"gad ", vec![(4009, Value::F32(-1.0))]);
        assert!(AnimInfo::from_gff(&b.finish(root)).is_err());
        let mut b = Builder::new(b"EVT ");
        let root = b.add(b"evtl", vec![]);
        assert!(
            AnimInfo::from_gff(&b.finish(root)).is_err(),
            "wrong file type"
        );
    }

    #[test]
    fn events_keep_their_times_and_kinds_and_garbage_is_an_error() {
        let mut b = Builder::new(b"EVT ");
        let e1 = b.add(
            b"evt ",
            vec![(4016, Value::F32(0.03)), (4017, Value::I32(10))],
        );
        let e2 = b.add(
            b"evt ",
            vec![(4016, Value::F32(0.43)), (4017, Value::I32(13))],
        );
        let root = b.add(b"evtl", vec![(4020, structs(&[e1, e2]))]);
        let ev = events_from_gff(&b.finish(root)).unwrap();
        assert_eq!(
            ev,
            vec![
                AnimEvent {
                    time: 0.03,
                    kind: 10
                },
                AnimEvent {
                    time: 0.43,
                    kind: 13
                }
            ]
        );
        assert!(parse_events(b"not a gff").is_err());

        let mut b = Builder::new(b"EVT ");
        let e = b.add(
            b"evt ",
            vec![
                (4016, Value::F32(0.63)),
                (4017, Value::I32(20)),
                (4019, s("glo_dog/dog/ss/bite\0")),
            ],
        );
        let root = b.add(b"evtl", vec![(4020, structs(&[e]))]);
        assert_eq!(
            events_with_text_from_gff(&b.finish(root)).unwrap(),
            vec![AnimEventText {
                time: 0.63,
                kind: 20,
                text: Some("glo_dog/dog/ss/bite".into())
            }]
        );
        assert!(parse_events_with_text(b"junk").is_err());
        assert!(AnimTree::parse(&[]).is_err());
        assert!(AnimInfo::parse(&[0xff; 64]).is_err());
    }

    /// Packs a unit quaternion the way the decoders read it: drop the largest component,
    /// store the other three on the given grid.
    fn pack(q: [f32; 4], max: [f32; 3]) -> (u64, [u64; 3]) {
        let big = (0..4)
            .max_by(|&a, &b| q[a].abs().total_cmp(&q[b].abs()))
            .unwrap();
        let q = if q[big] < 0.0 { q.map(|c| -c) } else { q };
        // The order table puts the reconstructed value at the index's position; the other
        // three keep their order.
        let rest: Vec<f32> = (0..4).filter(|&i| i != big).map(|i| q[i]).collect();
        let index = big as u64;
        let enc = |v: f32, m: f32| ((v / std::f32::consts::SQRT_2 * m) + m * 0.5).round() as u64;
        (
            index,
            [
                enc(rest[0], max[0]),
                enc(rest[1], max[1]),
                enc(rest[2], max[2]),
            ],
        )
    }

    fn close(a: [f32; 4], b: [f32; 4], tol: f32) -> bool {
        let dot: f32 = a.iter().zip(&b).map(|(x, y)| x * y).sum();
        1.0 - dot.abs() < tol
    }

    #[test]
    fn packed_rotations_round_trip() {
        let qs = [
            [0.0, 0.0, 0.0, 1.0],
            [0.70710677, 0.0, 0.0, 0.70710677],
            [0.5, -0.5, -0.5, 0.5],
            [0.1, 0.9, -0.3, 0.2],
            [-0.6, 0.2, 0.7, 0.3],
        ];
        for q in qs {
            let n = q.iter().map(|c| c * c).sum::<f32>().sqrt();
            let q = q.map(|c| c / n);
            let (i, [a, b, c]) = pack(q, [2_097_151.0, 2_097_151.0, 1_048_575.0]);
            let v64 = (a << 43) | (b << 22) | (c << 2) | i;
            assert!(
                close(unpack_quat64(v64), q, 1e-6),
                "{q:?} {:?}",
                unpack_quat64(v64)
            );
            let (i, [a, b, c]) = pack(q, [1023.0; 3]);
            let v32 = ((a << 22) | (b << 12) | (c << 2) | i) as u32;
            assert!(close(unpack_quat32(v32), q, 1e-5), "{q:?}");
            let (i, [a, b, c]) = pack(q, [65535.0; 3]);
            let w = [
                ((a as u16) & 0xfffe) | ((i >> 1) as u16 & 1),
                ((b as u16) & 0xfffe) | (i as u16 & 1),
                c as u16,
            ];
            assert!(close(unpack_quat48(w), q, 1e-6), "{q:?}");
        }
    }

    #[test]
    fn reads_tracks_of_each_kind() {
        let mut b = Builder::new(b"ANI ");
        // Index 3 (w rebuilt), every stored component at zero: no rotation.
        let q = 3u64 | ((2_097_151 / 2) << 43) | ((2_097_151 / 2) << 22) | ((1_048_575 / 2) << 2);
        let k1 = b.add(b"quat", vec![(4035, Value::U16(0)), (4036, Value::U64(q))]);
        let k2 = b.add(
            b"quat",
            vec![(4035, Value::U16(65535)), (4036, Value::U64(q))],
        );
        let l1 = b.add(b"list", vec![(4004, structs(&[k2, k1]))]);
        let rot = b.add(
            b"node",
            vec![
                (4000, s("Head_rotation")),
                (4001, Value::U32(4)),
                (4004, Value::Struct(Some(l1))),
            ],
        );
        let k3 = b.add(
            b"vec3",
            vec![
                (4035, Value::U16(32768)),
                (4036, Value::F32(1.0)),
                (4037, Value::F32(2.0)),
                (4038, Value::F32(3.0)),
            ],
        );
        let l2 = b.add(b"list", vec![(4004, structs(&[k3]))]);
        let tr = b.add(
            b"node",
            vec![
                (4000, s("root_translation")),
                (4001, Value::U32(6)),
                (4004, Value::Struct(Some(l2))),
            ],
        );
        let root = b.add(
            b"anim",
            vec![
                (4005, structs(&[rot, tr])),
                (4006, s("mh_m.p.ani")),
                (4009, Value::F32(2.0)),
            ],
        );
        let a = Animation::from_gff(&b.finish(root)).unwrap();
        assert_eq!(a.length, 2.0);
        assert_eq!(a.tracks[0].bone, "head");
        assert_eq!(a.tracks[0].channel, Channel::Rotation);
        assert_eq!(a.tracks[0].keys[0].time, 0.0, "sorted by time");
        assert!((a.tracks[0].keys[1].time - 2.0).abs() < 1e-6);
        assert!(close(a.tracks[0].keys[0].value, [0.0, 0.0, 0.0, 1.0], 1e-6));
        assert_eq!(a.tracks[1].keys[0].value, [1.0, 2.0, 3.0, 0.0]);
        assert!((a.tracks[1].keys[0].time - 1.0).abs() < 1e-3);
    }

    #[test]
    fn rejects_unknown_track_kinds() {
        let mut b = Builder::new(b"ANI ");
        let k = b.add(b"quat", vec![(4035, Value::U16(0)), (4036, Value::U64(0))]);
        let l = b.add(b"list", vec![(4004, structs(&[k]))]);
        let t = b.add(
            b"node",
            vec![
                (4000, s("head_rotation")),
                (4001, Value::U32(9)),
                (4004, Value::Struct(Some(l))),
            ],
        );
        let root = b.add(
            b"anim",
            vec![(4005, structs(&[t])), (4009, Value::F32(1.0))],
        );
        assert!(Animation::from_gff(&b.finish(root)).is_err());
        assert!(Animation::parse(b"nope").is_err());
    }
}
