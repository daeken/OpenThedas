//! Playing animations on a scene's skeletons: an action is looked up in a creature's
//! animation trees (`.anb`) to find its keyframes (`.ani`), which are sampled at the clip's
//! time into a pose. Trees, animations and rest poses are read once and kept.

use std::collections::HashMap;
use std::sync::Arc;

use glam::Mat4;
use thedas_formats::anim::{AnimTree, Animation};
use thedas_vfs::Vfs;

use crate::scene::AreaScene;

/// What to play: the first of `actions` that the first tree able to has, trying `trees` in
/// order, at `time` seconds, wrapped when `looping`, else held at the end.
#[derive(Clone, Debug, PartialEq)]
pub struct Clip {
    pub trees: Vec<String>,
    pub actions: Vec<String>,
    pub time: f32,
    pub looping: bool,
    /// Clips laid over this one, in order: each sets only the bones it animates (see
    /// [`crate::skeleton::Skeleton::pose_layers`]).
    pub layers: Vec<Clip>,
    /// As a layer, its translations are measured against the first layer's first keys (a
    /// cutscene's animations; see [`crate::skeleton::Skeleton::pose_layers`]).
    pub absolute: bool,
    /// As an additive layer, how much of it is laid on
    /// ([`crate::skeleton::Skeleton::pose_weighted`]); 1 otherwise.
    pub weight: f32,
    /// Set for a head tracking pose animation ([`crate::look`]): how far through its sweep
    /// it stands, 0 to 1, in place of `time` (`phase × (length − 1/30) + 1/30` seconds).
    pub phase: Option<f32>,
    /// An animation made in memory (a speaker's lip sync, see `thedas_formats::facefx`),
    /// played in place of one looked up by `trees` and `actions`.
    pub inline: Option<Arc<Animation>>,
}

impl Default for Clip {
    fn default() -> Self {
        Clip {
            trees: Vec::new(),
            actions: Vec::new(),
            time: 0.0,
            looping: false,
            layers: Vec::new(),
            absolute: false,
            weight: 1.0,
            phase: None,
            inline: None,
        }
    }
}

/// A tree's resource: a name with an extension (`bdccs_intro.cub`, a cutscene's tree) as
/// it is, else an `.anb`.
fn tree_resource(tree: &str) -> String {
    if tree.contains('.') {
        tree.to_string()
    } else {
        format!("{tree}.anb")
    }
}

/// Each track's node on a skeleton (see [`crate::skeleton::Skeleton::track_nodes`]).
type TrackNodes = Arc<[Option<usize>]>;

/// A skeleton's rest pose and its pose (model space).
type Posed = (Arc<Vec<Mat4>>, Vec<Mat4>);

/// The most threads the props' posing is spread over.
const POSE_THREADS: usize = 4;

#[derive(Default)]
pub struct Animator {
    trees: HashMap<String, Option<Arc<AnimTree>>>,
    animations: HashMap<String, Option<Arc<Animation>>>,
    rests: HashMap<usize, Arc<Vec<Mat4>>>,
    /// Dragon Age II animations whose bones were named for a skeleton already (see
    /// [`Animator::named_for`]): by animation and the skeleton's index.
    named: std::collections::HashSet<(String, usize)>,
    /// Animations whose tracks name their bones (Dragon Age: Origins'), by resource: no
    /// naming to do (see [`Animator::named_for`]).
    unhashed: std::collections::HashSet<String>,
    /// Each animation's track nodes on each skeleton (by the skeleton's index, then the
    /// animation's resource): see [`crate::skeleton::Skeleton::track_nodes`].
    track_nodes: HashMap<usize, HashMap<String, TrackNodes>>,
}

fn load<T>(
    cache: &mut HashMap<String, Option<Arc<T>>>,
    vfs: &Vfs,
    name: &str,
    parse: impl Fn(&[u8]) -> Option<T>,
) -> Option<Arc<T>> {
    // Read every frame: found without a copy of the name when it is lowercase already.
    if !name.bytes().any(|b| b.is_ascii_uppercase())
        && let Some(hit) = cache.get(name)
    {
        return hit.clone();
    }
    let key = name.to_ascii_lowercase();
    cache
        .entry(key.clone())
        .or_insert_with(|| {
            let loaded = vfs
                .read(&key)
                .ok()
                .and_then(|(d, _)| parse(&d))
                .map(Arc::new);
            if loaded.is_none() {
                log::warn!("{key}: not loaded");
            }
            loaded
        })
        .clone()
}

impl Animator {
    /// Forgets the skeletons posed so far (they are kept by their index in a scene), for
    /// posing another scene's; the animations read stay.
    pub fn forget_skeletons(&mut self) {
        self.rests.clear();
        self.named.clear();
        self.track_nodes.clear();
    }

    /// Animation `anim`'s (resource `ani`) track nodes on skeleton `skeleton` (`s`), worked
    /// out once.
    fn track_nodes_for(
        &mut self,
        ani: &str,
        anim: &Animation,
        skeleton: usize,
        s: &crate::skeleton::Skeleton,
    ) -> Arc<[Option<usize>]> {
        let by_ani = self.track_nodes.entry(skeleton).or_default();
        match by_ani.get(ani) {
            Some(n) if n.len() == anim.tracks.len() => n.clone(),
            _ => {
                let n: Arc<[Option<usize>]> = s.track_nodes(anim).into();
                by_ani.insert(ani.to_string(), n.clone());
                n
            }
        }
    }

    /// The `.ani` resource a clip plays (the first tree with one of its actions), and the
    /// animation itself: its events (`<stem>.evt`) are named after it.
    pub fn animation_resource(
        &mut self,
        vfs: &Vfs,
        clip: &Clip,
    ) -> Option<(String, Arc<Animation>)> {
        for tree in &clip.trees {
            let name = tree_resource(tree);
            let Some(t) = load(&mut self.trees, vfs, &name, |d| AnimTree::parse(d).ok()) else {
                continue;
            };
            let Some(ani) = clip
                .actions
                .iter()
                .find_map(|a| t.action(a).and_then(|n| n.animation.clone()))
            else {
                continue;
            };
            let anim = load(&mut self.animations, vfs, &ani, |d| {
                Animation::parse(d).ok()
            })?;
            return Some((ani, anim));
        }
        None
    }

    /// Every action of tree `tree` that plays an animation, with that animation (`.ani`),
    /// by action name.
    pub fn actions(&mut self, vfs: &Vfs, tree: &str) -> Vec<(String, String)> {
        let Some(t) = load(&mut self.trees, vfs, &tree_resource(tree), |d| {
            AnimTree::parse(d).ok()
        }) else {
            return Vec::new();
        };
        let mut v: Vec<(String, String)> = t
            .nodes
            .iter()
            .filter_map(|n| Some((n.action.clone(), n.animation.clone()?)))
            .filter(|(a, _)| !a.is_empty())
            .collect();
        v.sort_by_key(|a| a.0.to_ascii_lowercase());
        v.dedup_by(|a, b| a.0.eq_ignore_ascii_case(&b.0));
        v
    }

    /// The `.ani` resource action `action` names in tree `tree` (a name with an extension,
    /// or an `.anb`).
    pub fn action_animation(&mut self, vfs: &Vfs, tree: &str, action: &str) -> Option<String> {
        let t = load(&mut self.trees, vfs, &tree_resource(tree), |d| {
            AnimTree::parse(d).ok()
        })?;
        t.action(action)?.animation.clone()
    }

    /// Whether action `action` of tree `tree` plays its animation once (field 4027), if the
    /// tree has the action.
    pub fn action_once(&mut self, vfs: &Vfs, tree: &str, action: &str) -> Option<bool> {
        let t = load(&mut self.trees, vfs, &tree_resource(tree), |d| {
            AnimTree::parse(d).ok()
        })?;
        Some(t.action(action)?.once)
    }

    /// Whether animation `ani` plays once in the first of `trees` that has it (field 4027),
    /// if one does.
    pub fn plays_once(&mut self, vfs: &Vfs, trees: &[String], ani: &str) -> Option<bool> {
        trees.iter().find_map(|tree| {
            let t = load(&mut self.trees, vfs, &tree_resource(tree), |d| {
                AnimTree::parse(d).ok()
            })?;
            t.playing(ani).map(|n| n.once)
        })
    }

    /// The animation a clip names, if a tree has the action and its keyframes load.
    pub fn animation(&mut self, vfs: &Vfs, clip: &Clip) -> Option<Arc<Animation>> {
        self.animation_resource(vfs, clip).map(|(_, a)| a)
    }

    /// Animation `ani` with its tracks' bones named from skeleton `skeleton` of the scene
    /// (`s`). A Dragon Age II animation stores only a hash of each track's bone (see
    /// [`Animation::resolve_bones`]), which a skeleton's bone names are matched against;
    /// the named copy replaces the cached one, so each animation is named once for each
    /// skeleton it plays on (skeletons of one kind share their names). An animation with
    /// named tracks (Dragon Age: Origins') is returned as it is.
    fn named_for(
        &mut self,
        ani: &str,
        anim: Arc<Animation>,
        skeleton: usize,
        s: &crate::skeleton::Skeleton,
    ) -> Arc<Animation> {
        if self.unhashed.contains(ani) {
            return anim;
        }
        let key = ani.to_ascii_lowercase();
        let hashed = |a: &Animation| {
            a.tracks
                .iter()
                .any(|t| thedas_formats::hash_of_hashed_name(&t.bone).is_some())
        };
        if !hashed(&anim) {
            self.unhashed.insert(ani.to_string());
            return anim;
        }
        if !self.named.insert((key.clone(), skeleton)) {
            return anim;
        }
        let mut named = (*anim).clone();
        if named.resolve_bones(s.bones.iter().map(|b| b.name.as_str())) == 0 {
            return anim;
        }
        let named = Arc::new(named);
        self.animations.insert(key, Some(named.clone()));
        named
    }

    /// The rest pose and the clip's pose for one of `scene`'s skeletons; `None` when the
    /// clip has no animation.
    pub fn pose(
        &mut self,
        vfs: &Vfs,
        scene: &AreaScene,
        skeleton: usize,
        clip: &Clip,
    ) -> Option<(Arc<Vec<Mat4>>, Vec<Mat4>)> {
        self.prepare(vfs, scene, skeleton, clip).map(Prepared::pose)
    }

    /// What posing a skeleton with a clip needs, looked up (the animations, their times
    /// and track nodes, the rest pose); the posing itself ([`Prepared::pose`]) reads only
    /// this, so many can run at once.
    fn prepare(
        &mut self,
        vfs: &Vfs,
        scene: &AreaScene,
        skeleton: usize,
        clip: &Clip,
    ) -> Option<Prepared> {
        let s = scene.skeletons.get(skeleton)?.clone();
        let time = |c: &Clip, a: &Animation| {
            if let Some(phase) = c.phase {
                const FRAME: f32 = 1.0 / 30.0;
                phase * (a.length - FRAME) + FRAME
            } else if c.looping && a.length > 0.0 {
                c.time.rem_euclid(a.length)
            } else {
                c.time.min(a.length)
            }
        };
        let mut layers: Vec<PreparedLayer> = Vec::new();
        for c in std::iter::once(clip).chain(&clip.layers) {
            if let Some(anim) = &c.inline {
                // Made for this frame: its nodes are not kept.
                let nodes: Arc<[Option<usize>]> = s.track_nodes(anim).into();
                layers.push((anim.clone(), time(c, anim), c.absolute, c.weight, nodes));
            } else if let Some((ani, anim)) = self.animation_resource(vfs, c) {
                let anim = self.named_for(&ani, anim, skeleton, &s);
                let nodes = self.track_nodes_for(&ani, &anim, skeleton, &s);
                let t = time(c, &anim);
                layers.push((anim, t, c.absolute, c.weight, nodes));
            }
        }
        if layers.is_empty() {
            return None;
        }
        let rest = self
            .rests
            .entry(skeleton)
            .or_insert_with(|| Arc::new(s.rest()))
            .clone();
        Some(Prepared {
            skeleton: s,
            rest,
            layers,
        })
    }
}

/// One layer of a [`Prepared`] posing: the animation, its time, whether it is measured
/// against the first layer, its weight and its track nodes.
type PreparedLayer = (Arc<Animation>, f32, bool, f32, Arc<[Option<usize>]>);

/// A skeleton and the layers to pose it with (see [`Animator::prepare`]).
struct Prepared {
    skeleton: Arc<crate::skeleton::Skeleton>,
    rest: Arc<Vec<Mat4>>,
    layers: Vec<PreparedLayer>,
}

impl Prepared {
    /// The rest pose and the posed one.
    fn pose(self) -> (Arc<Vec<Mat4>>, Vec<Mat4>) {
        let mapped: Vec<crate::skeleton::Layer<'_>> = self
            .layers
            .iter()
            .map(|(a, t, b, w, n)| crate::skeleton::Layer {
                anim: a,
                time: *t,
                against_base: *b,
                weight: *w,
                nodes: n,
            })
            .collect();
        let pose = self.skeleton.pose_mapped(&mapped);
        (self.rest, pose)
    }
}

impl Animator {
    /// Places and poses every prop of `scene` in `renderer`: `props[i]` is prop `i`'s
    /// transform and clip (none: rest pose), with `advance` seconds added to looping clips'
    /// times (for a window that keeps playing).
    pub fn pose_props(
        &mut self,
        vfs: &Vfs,
        scene: &AreaScene,
        renderer: &mut crate::AreaRenderer,
        ctx: &crate::GpuContext,
        props: &[(Mat4, Option<Clip>)],
        advance: f32,
    ) {
        // The lookups first, in order (they fill the caches); then the posing, which reads
        // only what was looked up, spread over threads (a camp holds a hundred creatures,
        // each with several skeletons).
        let mut jobs: Vec<(usize, usize, Prepared)> = Vec::new();
        for (i, (_, clip)) in props.iter().enumerate() {
            if let Some(clip) = clip {
                let mut clip = clip.clone();
                if clip.looping {
                    clip.time += advance;
                }
                for s in scene.prop_skeletons(i) {
                    if let Some(p) = self.prepare(vfs, scene, s, &clip) {
                        jobs.push((i, s, p));
                    }
                }
            }
        }
        let threads = std::thread::available_parallelism()
            .map_or(1, |n| n.get())
            .clamp(1, POSE_THREADS);
        let mut posed: Vec<(usize, usize, Posed)> = Vec::with_capacity(jobs.len());
        if threads <= 1 || jobs.len() < 8 {
            posed.extend(jobs.into_iter().map(|(i, s, p)| (i, s, p.pose())));
        } else {
            let chunk = jobs.len().div_ceil(threads);
            let mut chunks: Vec<Vec<(usize, usize, Prepared)>> = Vec::new();
            let mut jobs = jobs.into_iter().peekable();
            while jobs.peek().is_some() {
                chunks.push(jobs.by_ref().take(chunk).collect());
            }
            std::thread::scope(|scope| {
                let handles: Vec<_> = chunks
                    .into_iter()
                    .map(|c| {
                        scope.spawn(move || {
                            c.into_iter()
                                .map(|(i, s, p)| (i, s, p.pose()))
                                .collect::<Vec<_>>()
                        })
                    })
                    .collect();
                for h in handles {
                    if let Ok(done) = h.join() {
                        posed.extend(done);
                    }
                }
            });
        }
        let mut by_prop: Vec<crate::skeleton::Poses> = vec![Default::default(); props.len()];
        for (i, s, p) in posed {
            by_prop[i].insert(s, p);
        }
        for (i, ((transform, _), poses)) in props.iter().zip(&by_prop).enumerate() {
            renderer.pose_prop(ctx, scene, i, *transform, poses);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use thedas_formats::anim::{Channel, Key, Track};
    use thedas_formats::level::Transform;
    use thedas_formats::model::{Mmh, Node};

    fn node(name: &str, bone: u32, children: Vec<Node>) -> Node {
        Node {
            kind: thedas_formats::FourCC::new(b"node"),
            name: Some(name.into()),
            transform: Transform {
                position: [0.0; 3],
                rotation: [0.0, 0.0, 0.0, 1.0],
            },
            mesh: None,
            light: None,
            emitter: None,
            irradiance: None,
            bone: Some(bone),
            children,
        }
    }

    #[test]
    fn dragon_age_ii_tracks_are_named_from_the_skeleton_they_play_on() {
        let skeleton = crate::skeleton::Skeleton::from_mmh(&Mmh {
            name: None,
            mesh_file: None,
            nodes: vec![node("Root", 0, vec![node("Elbow", 1, vec![])])],
        });
        let track = |bone: String, channel| Track {
            bone,
            channel,
            keys: vec![Key {
                time: 0.0,
                value: [0.0, 0.0, 0.0, 1.0],
            }],
        };
        let hashed =
            |target: &str| thedas_formats::hashed_name(thedas_formats::name_hash32(target));
        let anim = Arc::new(Animation {
            name: "wave".into(),
            length: 1.0,
            additive: false,
            scalars: Default::default(),
            tracks: vec![
                track(hashed("elbow_rotation"), Channel::Rotation),
                // A bone this skeleton does not have stays hashed.
                track(hashed("tail_rotation"), Channel::Rotation),
            ],
        });
        let mut a = Animator::default();
        a.animations.insert("wave.ani".into(), Some(anim.clone()));
        let named = a.named_for("wave.ani", anim.clone(), 0, &skeleton);
        assert_eq!(named.tracks[0].bone, "elbow");
        assert_eq!(named.tracks[1].bone, hashed("tail_rotation"));
        // The named copy is kept, and asked again for the same skeleton nothing is redone.
        let cached = a.animations["wave.ani"].clone().unwrap();
        assert!(Arc::ptr_eq(&cached, &named));
        assert!(Arc::ptr_eq(
            &a.named_for("wave.ani", cached.clone(), 0, &skeleton),
            &cached
        ));
        // Named tracks (Dragon Age: Origins') are left as they are.
        let plain = Arc::new(Animation {
            tracks: vec![track("elbow".into(), Channel::Rotation)],
            ..(*anim).clone()
        });
        assert!(Arc::ptr_eq(
            &a.named_for("plain.ani", plain.clone(), 0, &skeleton),
            &plain
        ));
    }
}
