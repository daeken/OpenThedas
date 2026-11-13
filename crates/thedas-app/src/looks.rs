//! Who looks at whom: the creatures' head trackers ([`thedas_render::look`]) and what
//! drives them.
//!
//! Traced: at a conversation line's start the conversation mode has the line's speaker and
//! its listener look at each other (DAOrigins.exe 0x68e620 to 0x68e6e4: each is given the
//! other as its look target, 0x69eaa0, and its tracker is started, 0x6cab00). The trackers'
//! table is `look_at.gda` (table 229), whose animations are rows of `ANIM_base`.
//!
//! A cutscene's actors, and those of a line with its own cinematics, look where its head
//! tracking actions say instead ([`thedas_sim::cuttracks::HeadTrack`]).
//!
//! Provisional: the tracker looks from the creature's head at rest height to the other's,
//! measured against the creature's facing (the executable reads nodes of the models,
//! 0x6ca450); a creature is taken to have the head and eye pose animations when its own
//! trees are the humanoid ones (they are `humanoid_overlay.anb`'s; which overlay tree a
//! creature plays is not traced), and no creature here has the body's. Not followed: the
//! glances creatures give the party while exploring (0x6cad80) and `SetLookAtEnabled`.

use std::collections::HashMap;

use glam::{Quat, Vec3};
use thedas_render::Clip;
use thedas_render::look::{BODY, EYES, HEAD, PartConfig, Tracker};
use thedas_script::vm::ObjectId;
use thedas_sim::Sim;

/// The tree with the humanoids' head and eye pose animations.
const OVERLAY: &str = "humanoid_overlay";

/// `look_at.gda`'s columns (names not known; see [`thedas_render::look::PartConfig`]).
mod col {
    pub const ANIMATIONS: [u32; 3] = [0xe7e6_a1ac, 0xbdcc_8db6, 0x0351_a7c8];
    pub const YAW: [u32; 2] = [0x926b_199e, 0x0a48_77bf];
    pub const PITCH: [u32; 2] = [0xdaf7_6ae3, 0xc04d_2885];
    pub const SPEEDS: [u32; 2] = [0xdb26_d384, 0x0b5a_85ae];
    pub const ANGLES: [u32; 2] = [0xd7c2_bef0, 0x2f08_bee7];
    pub const SIDEWAYS: u32 = 0x7e06_fd39;
    pub const UPDOWN: u32 = 0xdd4f_889a;
    /// `ANIM_base`'s action name.
    pub const ACTION: u32 = 0x7a39_8af9;
}

/// The trackers' table: rows 0 to 2 of table 229 (eyes, head, body), `None` when the game's
/// data does not have them all.
fn config(sim: &Sim) -> Option<[PartConfig; 3]> {
    use thedas_formats::gda::Cell;
    let tables = sim.world.tables();
    let look = tables.get(229, "")?;
    let anims = tables.get(0, "")?;
    let number = |hash: u32, row: i64| match look.cell_by_hash(hash, row)? {
        Cell::Float(v) => Some(*v as f32),
        Cell::Int(v) => Some(*v as f32),
        _ => None,
    };
    let action = |hash: u32, row: i64| {
        let id = number(hash, row)? as i64;
        match anims.cell_by_hash(col::ACTION, id)? {
            Cell::String(s) => Some(s.trim_end_matches('\0').trim().to_ascii_lowercase()),
            _ => None,
        }
    };
    let part = |row: i64| {
        let [a, b, c] = col::ANIMATIONS;
        Some(PartConfig::from_degrees(
            [action(a, row)?, action(b, row)?, action(c, row)?],
            (number(col::YAW[0], row)?, number(col::YAW[1], row)?),
            (number(col::PITCH[0], row)?, number(col::PITCH[1], row)?),
            (number(col::SPEEDS[0], row)?, number(col::SPEEDS[1], row)?),
            (number(col::ANGLES[0], row)?, number(col::ANGLES[1], row)?),
            number(col::SIDEWAYS, row)?,
            number(col::UPDOWN, row)?,
        ))
    };
    Some([part(0)?, part(1)?, part(2)?])
}

/// Who looks at whom now: each creature with the one it looks at.
pub fn pairs(sim: &Sim) -> Vec<(ObjectId, ObjectId)> {
    match crate::talkcam::line_speaker_and_listener(sim) {
        Some((speaker, listener)) if speaker != listener => {
            vec![(speaker, listener), (listener, speaker)]
        }
        _ => Vec::new(),
    }
}

/// Where a creature looks from and what at.
#[derive(Clone, Copy, Debug)]
pub struct Sight {
    pub origin: Vec3,
    /// The creature's facing.
    pub base: Quat,
    /// None: straight ahead.
    pub target: Option<Vec3>,
    /// What the tracker's speeds are multiplied by (1 but for a cutscene's actions).
    pub speed: f32,
}

#[derive(Default)]
pub struct Looks {
    /// The table, read at first use (`Some(None)`: the data has none).
    config: Option<Option<[PartConfig; 3]>>,
    trackers: HashMap<ObjectId, Tracker>,
}

impl Looks {
    /// Reads the table if it has not been.
    pub fn prepare(&mut self, sim: &Sim) {
        if self.config.is_none() {
            let c = config(sim);
            if c.is_none() {
                log::warn!("look_at (table 229) or its animations not found: no head tracking");
            }
            self.config = Some(c);
        }
    }

    /// Whether `id` has a tracker running (tracking, or still turning back to rest).
    pub fn has(&self, id: ObjectId) -> bool {
        self.trackers.contains_key(&id)
    }

    /// Advances creature `id`'s tracker by `dt` seconds towards `sight` (none: it lets go
    /// and turns back to rest), and gives the pose animations to lay over its clip.
    /// `valid(action)` says whether the creature has that pose animation; it is asked when
    /// the tracker is made.
    pub fn step(
        &mut self,
        id: ObjectId,
        sight: Option<Sight>,
        dt: f32,
        mut valid: impl FnMut(&str) -> bool,
    ) -> Vec<Clip> {
        let Some(Some(config)) = self.config.as_ref() else {
            return Vec::new();
        };
        if sight.is_some() && !self.trackers.contains_key(&id) {
            let has = [EYES, HEAD, BODY].map(|i| valid(&config[i].actions[0]));
            if has == [false; 3] {
                return Vec::new();
            }
            log::debug!("creature {id}: head tracking starts (eyes, head, body: {has:?})");
            self.trackers.insert(id, Tracker::new(config.clone(), has));
        }
        let Some(tracker) = self.trackers.get_mut(&id) else {
            return Vec::new();
        };
        match sight {
            Some(s) => {
                tracker.start();
                tracker.set_speed_scale(s.speed);
                tracker.set_origin(s.origin, s.base);
                tracker.set_target(s.target);
            }
            None => tracker.stop(),
        }
        tracker.update(dt);
        if !tracker.active() {
            self.trackers.remove(&id);
            return Vec::new();
        }
        let poses = tracker.poses();
        let mut out = Vec::new();
        for part in [BODY, HEAD, EYES] {
            let Some(pose) = poses[part] else {
                continue;
            };
            for (action, weight) in tracker.actions(part).iter().zip(pose.weights) {
                if weight <= 0.0 {
                    continue;
                }
                out.push(Clip {
                    trees: vec![OVERLAY.to_string()],
                    actions: vec![action.clone()],
                    weight,
                    phase: Some(pose.phase),
                    ..Default::default()
                });
            }
        }
        out
    }
}

/// Whether a creature playing `trees` is taken to have the humanoids' pose animations.
pub fn humanoid(trees: &[String]) -> bool {
    trees
        .iter()
        .any(|t| t.to_ascii_lowercase().starts_with("humanoid"))
}
