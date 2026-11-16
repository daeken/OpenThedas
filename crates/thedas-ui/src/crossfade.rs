//! Crossfades between a creature's successive animations outside movement (an ambient
//! loop giving way to the next, an animation a script plays, an attack): the new one fades
//! in over the old, which plays on beneath it, instead of snapping in.
//!
//! DAOrigins.exe changes a creature's animation through the agent's set-state (0xa5fcc0),
//! which keeps the outgoing state and fades the incoming one in at the weight rate the
//! caller gives it (notes/movement-anim-trace.md, "Agent set-state and exactly what
//! fades"); the movement controller's own changes are [`crate::locomotion`]'s. Presentation
//! only.
//!
//! Provisional: the rate (taken as the movement controller's 4 a second, a quarter of a
//! second for the whole change) — each caller of 0xa5fcc0 passes its own, not traced here;
//! a change during a crossfade starts a new one from the animation that was coming in.

use std::collections::HashMap;

use thedas_script::vm::ObjectId;
use thedas_sim::Sim;
use thedas_sim::world::ObjectKind;

use crate::models::{Motion, ObjectModel};

/// How fast an incoming animation's weight rises, a second (provisional, see the module).
pub const RATE: f32 = 4.0;

/// What tells one animation from another: its trees, actions and whether it loops (a change
/// of time alone is the same animation playing on).
type Key = (Vec<String>, Vec<String>, bool);

fn key(m: &Motion) -> Key {
    (m.trees.clone(), m.actions.clone(), m.looping)
}

/// The crossfades under way, and each creature's animation last frame.
#[derive(Default)]
pub struct Crossfade {
    /// Per creature: the animation it showed last, and when (world ms).
    last: HashMap<ObjectId, (Motion, u64)>,
    /// Per creature: the animation fading out, as it stood when the change came, and when.
    fading: HashMap<ObjectId, (Motion, u64)>,
}

impl Crossfade {
    /// Lays each creature's previous animation under its new one while the new one fades in.
    /// Animations [`crate::locomotion`] already blends (any with layers of their own) are
    /// left as they are.
    pub fn apply(&mut self, sim: &Sim, models: &mut [ObjectModel]) {
        let now = sim.world.time_ms;
        let mut seen = Vec::with_capacity(models.len());
        for m in models.iter_mut() {
            if m.kind != ObjectKind::Creature {
                continue;
            }
            let Some(motion) = m.motion.as_mut() else {
                continue;
            };
            seen.push(m.id);
            if !motion.blend.is_empty() {
                self.fading.remove(&m.id);
                self.last.remove(&m.id);
                continue;
            }
            if let Some((was, at)) = self.last.get(&m.id)
                && key(was) != key(motion)
                && now >= *at
            {
                let mut old = was.clone();
                old.time += (now - at) as f32 / 1000.0;
                self.fading.insert(m.id, (old, now));
            }
            self.last.insert(m.id, (motion.clone(), now));
            if let Some((old, from)) = self.fading.get(&m.id) {
                let since = now.saturating_sub(*from) as f32 / 1000.0;
                let w = since * RATE;
                if w >= 1.0 || now < *from {
                    self.fading.remove(&m.id);
                } else {
                    let incoming = motion.clone();
                    *motion = Motion {
                        time: old.time + since,
                        blend: vec![(incoming, w)],
                        ..old.clone()
                    };
                }
            }
        }
        self.last.retain(|id, _| seen.contains(id));
        self.fading.retain(|id, _| seen.contains(id));
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use thedas_script::EngineTable;
    use thedas_vfs::Vfs;

    use super::*;

    fn motion(action: &str) -> Motion {
        Motion {
            trees: vec!["humanoid.anb".into()],
            actions: vec![action.into()],
            time: 0.0,
            looping: true,
            blend: Vec::new(),
        }
    }

    fn model(id: ObjectId, m: Motion) -> ObjectModel {
        ObjectModel {
            id,
            tag: String::new(),
            kind: ObjectKind::Creature,
            models: Vec::new(),
            attached: Vec::new(),
            tints: Vec::new(),
            morph: None,
            hide_hair: false,
            effects: Vec::new(),
            motion: Some(m),
            position: [0.0; 3],
            facing: 0.0,
        }
    }

    #[test]
    fn a_new_animation_fades_in_over_the_old_for_a_quarter_second() {
        let mut s = Sim::new(Arc::new(Vfs::new()), Arc::new(EngineTable::default()), 1);
        let id = s.world.spawn(ObjectKind::Creature, "bartender");
        let mut x = Crossfade::default();
        let frame = |s: &Sim, x: &mut Crossfade, m: Motion| {
            let mut models = vec![model(id, m)];
            x.apply(s, &mut models);
            models.remove(0).motion.unwrap()
        };
        assert_eq!(frame(&s, &mut x, motion("lean")).actions, ["lean"]);
        s.world.time_ms += 100;
        // The change: the old animation underneath, the new one coming in from nothing.
        let m = frame(&s, &mut x, motion("wipe"));
        assert_eq!(m.actions, ["lean"]);
        assert_eq!(m.blend[0].0.actions, ["wipe"]);
        assert_eq!(m.blend[0].1, 0.0);
        s.world.time_ms += 125;
        let m = frame(&s, &mut x, motion("wipe"));
        assert!((m.blend[0].1 - 0.5).abs() < 1e-5);
        assert!((m.time - 0.225).abs() < 1e-5, "the old one plays on");
        s.world.time_ms += 125;
        let m = frame(&s, &mut x, motion("wipe"));
        assert_eq!(m.actions, ["wipe"]);
        assert!(m.blend.is_empty());
    }
}
