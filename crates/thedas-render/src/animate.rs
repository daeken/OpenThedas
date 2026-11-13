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

#[derive(Default)]
pub struct Animator {
    trees: HashMap<String, Option<Arc<AnimTree>>>,
    animations: HashMap<String, Option<Arc<Animation>>>,
    rests: HashMap<usize, Arc<Vec<Mat4>>>,
}

fn load<T>(
    cache: &mut HashMap<String, Option<Arc<T>>>,
    vfs: &Vfs,
    name: &str,
    parse: impl Fn(&[u8]) -> Option<T>,
) -> Option<Arc<T>> {
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

    /// The `.ani` resource action `action` names in tree `tree` (a name with an extension,
    /// or an `.anb`).
    pub fn action_animation(&mut self, vfs: &Vfs, tree: &str, action: &str) -> Option<String> {
        let t = load(&mut self.trees, vfs, &tree_resource(tree), |d| {
            AnimTree::parse(d).ok()
        })?;
        t.action(action)?.animation.clone()
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
            return load(&mut self.animations, vfs, &ani, |d| {
                Animation::parse(d).ok()
            });
        }
        None
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
        let mut layers: Vec<(Arc<Animation>, f32, bool, f32)> = Vec::new();
        if let Some(anim) = self.animation(vfs, clip) {
            let t = time(clip, &anim);
            layers.push((anim, t, clip.absolute, clip.weight));
        }
        for l in &clip.layers {
            if let Some(anim) = self.animation(vfs, l) {
                let t = time(l, &anim);
                layers.push((anim, t, l.absolute, l.weight));
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
        let refs: Vec<(&Animation, f32, bool, f32)> = layers
            .iter()
            .map(|(a, t, b, w)| (a.as_ref(), *t, *b, *w))
            .collect();
        Some((rest, s.pose_weighted(&refs)))
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
        for (i, (transform, clip)) in props.iter().enumerate() {
            let mut poses = crate::skeleton::Poses::new();
            if let Some(clip) = clip {
                let mut clip = clip.clone();
                if clip.looping {
                    clip.time += advance;
                }
                for s in scene.prop_skeletons(i) {
                    if let Some(p) = self.pose(vfs, scene, s, &clip) {
                        poses.insert(s, p);
                    }
                }
            }
            renderer.pose_prop(ctx, scene, i, *transform, &poses);
        }
    }
}
