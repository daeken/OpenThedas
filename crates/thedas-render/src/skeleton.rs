//! Skeletons and poses: a model's node tree as bones, sampled from an animation (`.ani`)
//! into model-space matrices, and the skinning palettes the character vertex shaders read.
//!
//! Animation tracks hold each bone's local rotation (absolute, not an offset from the rest
//! pose: an idle's first keys sit a few degrees from the model's rest rotations; the
//! animations keep `GOD` unrotated and turn `Root` by the half turn the models give `GOD`,
//! which composes to the same thing). Translation keys are in a convention not yet worked
//! out (the root's keys lack the hip height, other nodes' come out doubled), so they apply
//! as offsets from the clip's first key on top of the rest translation (provisional).
//! Nodes without a track keep their rest placement.
//!
//! An additive animation ([`Animation::additive`], field 4011: conversation gestures such as
//! `mh.po_2arm_06.ani` and the face overlays `mh_o.*`) holds offsets from the identity, not
//! a pose: it turns the pose beneath rather than replacing it (see
//! [`Skeleton::pose_layers`]).

use std::collections::HashMap;

use glam::{Mat4, Quat, Vec3};
use thedas_formats::anim::{Animation, Channel, Key};
use thedas_formats::model::{Mmh, Node};

/// Poses by skeleton: (rest, posed) model-space matrices per node.
pub type Poses = HashMap<usize, (std::sync::Arc<Vec<Mat4>>, Vec<Mat4>)>;

#[derive(Clone, Debug, PartialEq)]
pub struct Bone {
    /// Lowercase node name (`head`, `rightwrist`, `prop01`).
    pub name: String,
    pub parent: Option<usize>,
    pub rest_translation: Vec3,
    pub rest_rotation: Quat,
    /// The node's bone index, which skinned meshes' palettes name.
    pub index: Option<u32>,
}

/// Every node of a model, parents before children.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Skeleton {
    pub bones: Vec<Bone>,
    by_name: HashMap<String, usize>,
    by_index: HashMap<u32, usize>,
}

fn quat(r: [f32; 4]) -> Quat {
    let q = Quat::from_array(r);
    if q.length_squared() > 0.0 {
        q.normalize()
    } else {
        Quat::IDENTITY
    }
}

impl Skeleton {
    pub fn from_mmh(m: &Mmh) -> Self {
        fn walk(n: &Node, parent: Option<usize>, s: &mut Skeleton) {
            let i = s.bones.len();
            let name = n.name.as_deref().unwrap_or("").to_ascii_lowercase();
            s.by_name.entry(name.clone()).or_insert(i);
            if let Some(b) = n.bone {
                s.by_index.entry(b).or_insert(i);
            }
            s.bones.push(Bone {
                name,
                parent,
                rest_translation: Vec3::from_array(n.transform.position),
                rest_rotation: quat(n.transform.rotation),
                index: n.bone,
            });
            for c in &n.children {
                walk(c, Some(i), s);
            }
        }
        let mut s = Skeleton::default();
        for n in &m.nodes {
            walk(n, None, &mut s);
        }
        s
    }

    pub fn node(&self, name: &str) -> Option<usize> {
        self.by_name.get(&name.to_ascii_lowercase()).copied()
    }

    pub fn node_of_bone(&self, index: u32) -> Option<usize> {
        self.by_index.get(&index).copied()
    }

    /// Model-space matrices from each node's local (translation, rotation).
    fn globals(&self, local: impl Fn(usize) -> (Vec3, Quat)) -> Vec<Mat4> {
        let mut out: Vec<Mat4> = Vec::with_capacity(self.bones.len());
        for (i, b) in self.bones.iter().enumerate() {
            let (t, r) = local(i);
            let m = Mat4::from_rotation_translation(r, t);
            // Parents precede children, so the parent's matrix is already there.
            out.push(match b.parent {
                Some(p) => out[p] * m,
                None => m,
            });
        }
        out
    }

    /// The rest pose.
    pub fn rest(&self) -> Vec<Mat4> {
        self.globals(|i| (self.bones[i].rest_translation, self.bones[i].rest_rotation))
    }

    /// The pose `anim` gives at `time` seconds (clamped to its length).
    pub fn pose(&self, anim: &Animation, time: f32) -> Vec<Mat4> {
        self.pose_layers(&[(anim, time, false)])
    }

    /// The pose of animations laid one over another: each sets the bones it has tracks for,
    /// over what the ones before it set (a cutscene's head or gesture animation over a
    /// body's). A bone's translation moves it from its rest by how far it is from the
    /// animation's first key; for a layer marked as measured against the first (a
    /// cutscene's), from the first layer's first key where that has one (a sitting
    /// animation's lowered hips stay lowered against the standing pose). Provisional: from
    /// what the animations look like; the executable's blend layers and weights are not
    /// followed.
    ///
    /// Additive layers ([`Animation::additive`]) go on last, in their order, whatever their
    /// place in `layers` (DAOrigins.exe keeps them on a list of their own and blends it after
    /// the others, 0x8149d0, 0x544620): each rotation key turns its bone's pose (key × pose,
    /// 0x8111e0 → 0x4cb640) and each translation key is added to its bone's translation
    /// (0x8110f0), on the bones the clip has tracks for. They are taken at full weight
    /// here ([`Skeleton::pose_weighted`] gives each its own), and the translation sum rests
    /// on the key convention above (provisional).
    pub fn pose_layers(&self, layers: &[(&Animation, f32, bool)]) -> Vec<Mat4> {
        let weighted: Vec<_> = layers.iter().map(|&(a, t, b)| (a, t, b, 1.0)).collect();
        self.pose_weighted(&weighted)
    }

    /// [`Skeleton::pose_layers`] with a weight for each layer, which scales an additive
    /// layer's turn and shift (the head tracking's pose animations; see [`crate::look`]):
    /// its rotation key is taken that far from no turn at all, its translation key times
    /// the weight. Provisional: the executable scales each additive clip by its instance's
    /// weight (+0xc of its blend entry), but how is not traced. Other layers ignore it.
    pub fn pose_weighted(&self, layers: &[(&Animation, f32, bool, f32)]) -> Vec<Mat4> {
        let mut rot: Vec<Option<Quat>> = vec![None; self.bones.len()];
        let mut pos: Vec<Option<Vec3>> = vec![None; self.bones.len()];
        let mut reference: Vec<Option<Vec3>> = vec![None; self.bones.len()];
        let (additive, posed): (Vec<_>, Vec<_>) =
            layers.iter().partition(|(a, _, _, _)| a.additive);
        if let Some((base, _, _, _)) = posed.first() {
            for track in &base.tracks {
                if track.channel == Channel::Translation
                    && let (Some(i), Some(k)) = (self.node(&track.bone), track.keys.first())
                    && let Some(slot) = reference.get_mut(i)
                {
                    *slot = Some(Vec3::new(k.value[0], k.value[1], k.value[2]));
                }
            }
        }
        for (n, (anim, time, against_base, _)) in posed.iter().enumerate() {
            let r = (n > 0 && *against_base).then_some(reference.as_slice());
            self.sample_into(anim, *time, r, &mut rot, &mut pos);
        }
        for (anim, time, _, weight) in additive {
            self.add_into(anim, time, weight, &mut rot, &mut pos);
        }
        self.globals(|i| {
            let b = &self.bones[i];
            (
                pos[i].unwrap_or(b.rest_translation),
                rot[i].unwrap_or(b.rest_rotation),
            )
        })
    }

    /// Lays additive animation `anim` at `time` and `weight` over the local pose in `rot`
    /// and `pos` (bones not set there are at rest): see [`Skeleton::pose_weighted`].
    fn add_into(
        &self,
        anim: &Animation,
        time: f32,
        weight: f32,
        rot: &mut [Option<Quat>],
        pos: &mut [Option<Vec3>],
    ) {
        let t = time.clamp(0.0, anim.length.max(0.0));
        for track in &anim.tracks {
            let Some(i) = self.node(&track.bone) else {
                continue;
            };
            let rest = &self.bones[i];
            match track.channel {
                Channel::Rotation => {
                    if let (Some(d), Some(slot)) = (sample_rotation(&track.keys, t), rot.get_mut(i))
                    {
                        let d = if weight == 1.0 {
                            d
                        } else {
                            Quat::IDENTITY.slerp(d, weight)
                        };
                        *slot = Some((d * slot.unwrap_or(rest.rest_rotation)).normalize());
                    }
                }
                Channel::Translation => {
                    if let (Some(d), Some(slot)) =
                        (sample_translation(&track.keys, t), pos.get_mut(i))
                    {
                        *slot = Some(slot.unwrap_or(rest.rest_translation) + d * weight);
                    }
                }
            }
        }
    }

    fn sample_into(
        &self,
        anim: &Animation,
        time: f32,
        reference: Option<&[Option<Vec3>]>,
        rot: &mut [Option<Quat>],
        pos: &mut [Option<Vec3>],
    ) {
        let t = time.clamp(0.0, anim.length.max(0.0));
        for track in &anim.tracks {
            let Some(i) = self.node(&track.bone) else {
                continue;
            };
            match track.channel {
                Channel::Rotation => {
                    if let Some(r) = sample_rotation(&track.keys, t) {
                        if let Some(slot) = rot.get_mut(i) {
                            *slot = Some(r);
                        }
                    }
                }
                Channel::Translation => {
                    let own = track
                        .keys
                        .first()
                        .map(|k| Vec3::new(k.value[0], k.value[1], k.value[2]));
                    let first = reference.and_then(|r| r.get(i).copied().flatten()).or(own);
                    let p = sample_translation(&track.keys, t)
                        .zip(first)
                        .map(|(v, f)| self.bones[i].rest_translation + (v - f));
                    if let (Some(p), Some(slot)) = (p, pos.get_mut(i)) {
                        *slot = Some(p);
                    }
                }
            }
        }
    }
}

/// The keys around `t` and how far between them it lies.
fn segment(keys: &[Key], t: f32) -> Option<(&Key, &Key, f32)> {
    let first = keys.first()?;
    if keys.len() == 1 || t <= first.time {
        return Some((first, first, 0.0));
    }
    let next = keys.partition_point(|k| k.time <= t);
    if next >= keys.len() {
        let last = keys.last()?;
        return Some((last, last, 0.0));
    }
    let (a, b) = (&keys[next - 1], &keys[next]);
    let span = b.time - a.time;
    let f = if span > 0.0 { (t - a.time) / span } else { 0.0 };
    Some((a, b, f.clamp(0.0, 1.0)))
}

fn sample_rotation(keys: &[Key], t: f32) -> Option<Quat> {
    let (a, b, f) = segment(keys, t)?;
    Some(quat(a.value).slerp(quat(b.value), f))
}

fn sample_translation(keys: &[Key], t: f32) -> Option<Vec3> {
    let (a, b, f) = segment(keys, t)?;
    let v = |k: &Key| Vec3::new(k.value[0], k.value[1], k.value[2]);
    Some(v(a).lerp(v(b), f))
}

/// The skinning palette for one mesh: per palette slot, the three rows of the 3x4 matrix
/// taking a vertex from its rest position to `pose`, in the space of the mesh node
/// (`mesh_node` is the node's rest model-space matrix). `palette` holds skeleton node
/// indices (`None` for a bone the skeleton lacks: left at rest).
pub fn palette_rows(
    rest: &[Mat4],
    pose: &[Mat4],
    palette: &[Option<usize>],
    mesh_node: Mat4,
) -> Vec<[f32; 4]> {
    let inv_node = mesh_node.inverse();
    let mut rows = Vec::with_capacity(palette.len() * 3);
    for slot in palette {
        let m = match slot {
            Some(n) if *n < rest.len() && *n < pose.len() => {
                inv_node * pose[*n] * rest[*n].inverse() * mesh_node
            }
            _ => Mat4::IDENTITY,
        };
        for r in 0..3 {
            rows.push(m.row(r).to_array());
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use thedas_formats::anim::Track;
    use thedas_formats::level::Transform;

    fn node(name: &str, bone: Option<u32>, position: [f32; 3], children: Vec<Node>) -> Node {
        Node {
            kind: thedas_formats::FourCC::new(b"node"),
            name: Some(name.into()),
            transform: Transform {
                position,
                rotation: [0.0, 0.0, 0.0, 1.0],
            },
            mesh: None,
            light: None,
            emitter: None,
            irradiance: None,
            bone,
            children,
        }
    }

    fn arm() -> Skeleton {
        // root -> shoulder (1 m up) -> elbow (1 m along x).
        let elbow = node("Elbow", Some(2), [1.0, 0.0, 0.0], vec![]);
        let shoulder = node("Shoulder", Some(1), [0.0, 0.0, 1.0], vec![elbow]);
        Skeleton::from_mmh(&Mmh {
            name: None,
            mesh_file: None,
            nodes: vec![node("Root", Some(0), [0.0; 3], vec![shoulder])],
        })
    }

    #[test]
    fn rest_and_posed_bones_compose_down_the_tree() {
        let s = arm();
        let rest = s.rest();
        let elbow = s.node("elbow").unwrap();
        assert_eq!(s.node_of_bone(2), Some(elbow));
        assert!(
            (rest[elbow].transform_point3(Vec3::ZERO) - Vec3::new(1.0, 0.0, 1.0)).length() < 1e-6
        );
        // Turn the shoulder a quarter turn about z over one second.
        let q = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2).to_array();
        let anim = Animation {
            name: "t".into(),
            length: 1.0,
            additive: false,
            scalars: Vec::new(),
            tracks: vec![Track {
                bone: "shoulder".into(),
                channel: Channel::Rotation,
                keys: vec![
                    Key {
                        time: 0.0,
                        value: [0.0, 0.0, 0.0, 1.0],
                    },
                    Key {
                        time: 1.0,
                        value: q,
                    },
                ],
            }],
        };
        let end = s.pose(&anim, 1.0);
        assert!(
            (end[elbow].transform_point3(Vec3::ZERO) - Vec3::new(0.0, 1.0, 1.0)).length() < 1e-5
        );
        let half = s.pose(&anim, 0.5);
        let p = half[elbow].transform_point3(Vec3::ZERO);
        let r = std::f32::consts::FRAC_1_SQRT_2;
        assert!((p - Vec3::new(r, r, 1.0)).length() < 1e-5, "{p:?}");
        // The skinning matrix carries a vertex at the rest elbow to the posed elbow.
        let rows = palette_rows(&rest, &end, &[Some(elbow), None], Mat4::IDENTITY);
        assert_eq!(rows.len(), 6);
        let v = [1.0, 0.0, 1.0, 1.0];
        let moved: Vec<f32> = rows[..3]
            .iter()
            .map(|r| r.iter().zip(&v).map(|(a, b)| a * b).sum())
            .collect();
        assert!((moved[0] - 0.0).abs() < 1e-5 && (moved[1] - 1.0).abs() < 1e-5);
        assert_eq!(
            rows[3],
            [1.0, 0.0, 0.0, 0.0],
            "a missing bone stays at rest"
        );
    }

    fn rotation_clip(bone: &str, additive: bool, keys: &[(f32, Quat)]) -> Animation {
        Animation {
            name: "t".into(),
            length: 1.0,
            additive,
            scalars: Vec::new(),
            tracks: vec![Track {
                bone: bone.into(),
                channel: Channel::Rotation,
                keys: keys
                    .iter()
                    .map(|(time, q)| Key {
                        time: *time,
                        value: q.to_array(),
                    })
                    .collect(),
            }],
        }
    }

    fn local_rotation(s: &Skeleton, pose: &[Mat4], bone: &str) -> Quat {
        let i = s.node(bone).unwrap();
        let parent = s.bones[i].parent.map_or(Mat4::IDENTITY, |p| pose[p]);
        let (_, r, _) = (parent.inverse() * pose[i]).to_scale_rotation_translation();
        r
    }

    #[test]
    fn an_additive_layer_turns_the_pose_beneath_it() {
        use std::f32::consts::FRAC_PI_2;
        let s = arm();
        // The body holds the shoulder a quarter turn about z.
        let held = Quat::from_rotation_z(FRAC_PI_2);
        let body = rotation_clip("shoulder", false, &[(0.0, held)]);
        // A gesture's offsets start at the identity and lift the arm about x by 0.3 rad;
        // it also has an elbow track at the identity.
        let lift = Quat::from_rotation_x(0.3);
        let mut gesture = rotation_clip("shoulder", true, &[(0.0, Quat::IDENTITY), (1.0, lift)]);
        gesture.tracks.push(Track {
            bone: "elbow".into(),
            channel: Channel::Rotation,
            keys: vec![Key {
                time: 0.0,
                value: [0.0, 0.0, 0.0, 1.0],
            }],
        });
        let close = |a: Quat, b: Quat| a.dot(b).abs() > 1.0 - 1e-5;
        // At its start it leaves the pose as the body has it.
        let start = s.pose_layers(&[(&body, 0.0, false), (&gesture, 0.0, true)]);
        assert!(close(local_rotation(&s, &start, "shoulder"), held));
        // At its end the lift turns the held shoulder (key × pose), not replacing it.
        let end = s.pose_layers(&[(&body, 0.0, false), (&gesture, 1.0, true)]);
        assert!(close(local_rotation(&s, &end, "shoulder"), lift * held));
        // An identity offset leaves a bone the body does not animate at its rest.
        assert!(close(local_rotation(&s, &end, "elbow"), Quat::IDENTITY));
        // Applied as a pose, the same gesture would have dropped the held turn.
        let wrong = s.pose_layers(&[
            (&body, 0.0, false),
            (&rotation_clip("shoulder", false, &[(0.0, lift)]), 0.0, true),
        ]);
        assert!(!close(local_rotation(&s, &wrong, "shoulder"), lift * held));
    }

    #[test]
    fn additive_layers_go_on_after_the_poses() {
        use std::f32::consts::FRAC_PI_2;
        let s = arm();
        let body = rotation_clip("shoulder", false, &[(0.0, Quat::IDENTITY)]);
        let lift = Quat::from_rotation_x(0.3);
        let gesture = rotation_clip("shoulder", true, &[(0.0, lift)]);
        // A pose layer listed after the additive one still lies beneath it.
        let turn = Quat::from_rotation_z(FRAC_PI_2);
        let later = rotation_clip("shoulder", false, &[(0.0, turn)]);
        let p = s.pose_layers(&[
            (&body, 0.0, false),
            (&gesture, 0.0, true),
            (&later, 0.0, true),
        ]);
        assert!(local_rotation(&s, &p, "shoulder").dot(lift * turn).abs() > 1.0 - 1e-5);
        // With nothing beneath, the offsets turn the rest pose.
        let alone = s.pose_layers(&[(&gesture, 0.0, true)]);
        assert!(local_rotation(&s, &alone, "shoulder").dot(lift).abs() > 1.0 - 1e-5);
    }

    #[test]
    fn an_additive_translation_is_added() {
        let s = arm();
        let shift = Animation {
            name: "t".into(),
            length: 1.0,
            additive: true,
            scalars: Vec::new(),
            tracks: vec![Track {
                bone: "elbow".into(),
                channel: Channel::Translation,
                keys: vec![Key {
                    time: 0.0,
                    value: [0.0, 0.5, 0.0, 0.0],
                }],
            }],
        };
        let p = s.pose_layers(&[(&shift, 0.0, false)]);
        let elbow = p[s.node("elbow").unwrap()].transform_point3(Vec3::ZERO);
        assert!(
            (elbow - Vec3::new(1.0, 0.5, 1.0)).length() < 1e-5,
            "{elbow:?}"
        );
    }

    #[test]
    fn sampling_clamps_to_the_ends() {
        let k = |t: f32, x: f32| Key {
            time: t,
            value: [x, 0.0, 0.0, 0.0],
        };
        let keys = [k(0.5, 1.0), k(1.0, 3.0)];
        assert_eq!(sample_translation(&keys, 0.0), Some(Vec3::X));
        assert_eq!(
            sample_translation(&keys, 0.75),
            Some(Vec3::new(2.0, 0.0, 0.0))
        );
        assert_eq!(
            sample_translation(&keys, 9.0),
            Some(Vec3::new(3.0, 0.0, 0.0))
        );
        assert_eq!(sample_translation(&[], 0.0), None);
    }
}
