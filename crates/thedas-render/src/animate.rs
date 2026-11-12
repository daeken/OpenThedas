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
    /// The `.ani` resource a clip plays (the first tree with one of its actions), and the
    /// animation itself: its events (`<stem>.evt`) are named after it.
    pub fn animation_resource(
        &mut self,
        vfs: &Vfs,
        clip: &Clip,
    ) -> Option<(String, Arc<Animation>)> {
        for tree in &clip.trees {
            let name = if tree.ends_with(".anb") {
                tree.clone()
            } else {
                format!("{tree}.anb")
            };
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

    /// The animation a clip names, if a tree has the action and its keyframes load.
    pub fn animation(&mut self, vfs: &Vfs, clip: &Clip) -> Option<Arc<Animation>> {
        for tree in &clip.trees {
            let name = if tree.ends_with(".anb") {
                tree.clone()
            } else {
                format!("{tree}.anb")
            };
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
        let anim = self.animation(vfs, clip)?;
        let rest = self
            .rests
            .entry(skeleton)
            .or_insert_with(|| Arc::new(s.rest()))
            .clone();
        let t = if clip.looping && anim.length > 0.0 {
            clip.time.rem_euclid(anim.length)
        } else {
            clip.time.min(anim.length)
        };
        Some((rest, s.pose(&anim, t)))
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
