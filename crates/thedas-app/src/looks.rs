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
//! creature plays is not traced), and no creature here has the body's.
//!
//! While exploring, creatures glance at the player's character ([`Glance`]; the tracker's
//! own update, 0x6cad80). Not followed: its other targets (what a creature's action is
//! aimed at, and what the player has selected, within 25 m) and `SetLookAtEnabled`.

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

/// How creatures glance at the player's character while exploring: `globals.gda` (table
/// 105; its value column's name is not known), rows 0 to 6 and 10.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GlanceSettings {
    /// `MIN_TIME_FOR_LOOKAT_ACTION` and `MAX_…` (milliseconds): how long before a creature
    /// thinks again about looking.
    pub think_ms: (f32, f32),
    /// `PERCENT_CHANCE_TO_LOOK_AT_PLAYER`.
    pub chance: u32,
    /// `MIN_LOOKAT_HEADTURN_SPEED` and `MAX_…`: what the tracker's speeds are multiplied
    /// by as it turns to look.
    pub look_speed: (f32, f32),
    /// `MIN_LOOKAWAY_HEADTURN_SPEED` and `MAX_…`: as it turns away.
    pub away_speed: (f32, f32),
    /// `LOOKAT_THRESHOLD_DISTANCE_NEARBYNPC` (metres): how near the player's character
    /// has to be.
    pub near: f32,
}

/// `globals.gda`'s value column.
const COL_GLOBAL_VALUE: u32 = 0x3fc1_3de1;

fn glance_settings(sim: &Sim) -> Option<GlanceSettings> {
    use thedas_formats::gda::Cell;
    let t = sim.world.tables().get(105, "")?;
    let v = |row: i64| match t.cell_by_hash(COL_GLOBAL_VALUE, row)? {
        Cell::Float(v) => Some(*v as f32),
        Cell::Int(v) => Some(*v as f32),
        _ => None,
    };
    Some(GlanceSettings {
        think_ms: (v(0)?, v(1)?),
        chance: v(2)?.max(0.0) as u32,
        look_speed: (v(3)?, v(4)?),
        away_speed: (v(5)?, v(6)?),
        near: v(10)?,
    })
}

/// One creature's glancing at the player's character while exploring.
///
/// Traced (the tracker's update, 0x6cad80): a timer runs down; when it is out the creature
/// rolls a hundred-sided die: one not looking starts to with the settings' chance, one
/// looking goes on with half that chance and otherwise stops; the timer is set anew
/// between the settings' two times. While it means to look, it tracks the player's
/// character whenever that is within the settings' distance and in front of it; as it
/// starts to and as it stops, the tracker's speeds are scaled by a random factor within
/// the settings' ranges. The player's own character does not glance.
///
/// Provisional: the die is the C runtime's generator seeded per creature here (the
/// executable's is shared), and a creature in combat is taken not to glance (the
/// executable asks 0x7c4970 of it, which is not traced).
#[derive(Clone, Copy, Debug)]
pub struct Glance {
    timer_ms: f32,
    looking: bool,
    tracking: bool,
    /// The scale the tracker's speeds have now.
    speed: f32,
    rand: u32,
}

impl Glance {
    pub fn new(seed: u32) -> Self {
        Glance {
            timer_ms: 0.0,
            looking: false,
            tracking: false,
            speed: 1.0,
            rand: seed,
        }
    }

    fn roll(&mut self) -> u32 {
        self.rand = self.rand.wrapping_mul(214_013).wrapping_add(2_531_011);
        (self.rand >> 16) & 0x7fff
    }

    fn within(&mut self, (low, high): (f32, f32)) -> f32 {
        low + self.roll() as f32 / 32767.0 * (high - low)
    }

    /// Advances it by `dt` seconds; `in_reach` says whether the player's character is near
    /// enough and in front. Gives whether it tracks now and the speed scale to track or to
    /// turn away with.
    pub fn advance(&mut self, s: &GlanceSettings, dt: f32, in_reach: bool) -> (bool, f32) {
        if self.timer_ms > 0.0 {
            self.timer_ms -= dt * 1000.0;
        } else {
            let die = self.roll() % 100;
            self.looking = if self.looking {
                die < s.chance / 2
            } else {
                die < s.chance
            };
            let span = (s.think_ms.1 - s.think_ms.0).max(1.0) as u32;
            self.timer_ms = s.think_ms.0 + (self.roll() % span) as f32;
        }
        let tracking = self.looking && in_reach;
        if tracking != self.tracking {
            self.speed = if tracking {
                self.within(s.look_speed)
            } else {
                self.within(s.away_speed)
            };
            self.tracking = tracking;
        }
        (tracking, self.speed)
    }
}

#[derive(Default)]
pub struct Looks {
    /// The table, read at first use (`Some(None)`: the data has none).
    config: Option<Option<[PartConfig; 3]>>,
    trackers: HashMap<ObjectId, Tracker>,
    /// The glances' settings, read at first use, and each creature's glancing.
    glance_settings: Option<Option<GlanceSettings>>,
    glances: HashMap<ObjectId, Glance>,
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
        if self.glance_settings.is_none() {
            self.glance_settings = Some(glance_settings(sim));
        }
    }

    /// How near the player's character has to be for a creature to glance at it (metres);
    /// none when the game's data has no settings for glances.
    pub fn glance_reach(&self) -> Option<f32> {
        Some(self.glance_settings??.near)
    }

    /// Advances creature `id`'s glancing ([`Glance::advance`]): whether it tracks the
    /// player's character now, and the speed scale for its tracker.
    pub fn glance(&mut self, id: ObjectId, dt: f32, in_reach: bool) -> (bool, f32) {
        let Some(Some(settings)) = self.glance_settings else {
            return (false, 1.0);
        };
        self.glances
            .entry(id)
            .or_insert_with(|| Glance::new(id.wrapping_mul(2_654_435_761)))
            .advance(&settings, dt, in_reach)
    }

    /// Forgets the glancing of creatures not among `kept` (those no longer drawn).
    pub fn keep_glances(&mut self, kept: &dyn Fn(ObjectId) -> bool) {
        self.glances.retain(|id, _| kept(*id));
    }

    /// Whether `id` has a tracker running (tracking, or still turning back to rest).
    pub fn has(&self, id: ObjectId) -> bool {
        self.trackers.contains_key(&id)
    }

    /// Advances creature `id`'s tracker by `dt` seconds towards `sight` (none: it lets go
    /// and turns back to rest, at the speed scale `away`), and gives the pose animations to
    /// lay over its clip.
    /// `valid(action)` says whether the creature has that pose animation; it is asked when
    /// the tracker is made.
    pub fn step(
        &mut self,
        id: ObjectId,
        sight: Option<Sight>,
        away: f32,
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
            None => {
                if tracker.tracking() {
                    tracker.set_speed_scale(away);
                }
                tracker.stop();
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The game's settings.
    const SETTINGS: GlanceSettings = GlanceSettings {
        think_ms: (1500.0, 4000.0),
        chance: 70,
        look_speed: (0.75, 1.0),
        away_speed: (0.4, 0.6),
        near: 5.0,
    };

    #[test]
    fn a_creature_thinks_about_looking_every_few_seconds_and_looks_most_of_the_time() {
        let mut g = Glance::new(7);
        // A minute of frames with the player in reach.
        let (mut looked, mut changes, mut was) = (0, 0, false);
        let frames = 60 * 30;
        for _ in 0..frames {
            let (tracking, speed) = g.advance(&SETTINGS, 1.0 / 30.0, true);
            looked += tracking as u32;
            if tracking != was {
                changes += 1;
                // Turning to look is quicker than turning away.
                if tracking {
                    assert!((0.75..=1.0).contains(&speed), "{speed}");
                } else {
                    assert!((0.4..=0.6).contains(&speed), "{speed}");
                }
            }
            was = tracking;
        }
        // It changes its mind no more often than every 1.5 s, and does change it.
        assert!((2..=40).contains(&changes), "{changes}");
        // Starting at 70 in a hundred and staying at 35: it looks roughly half the time.
        let share = looked as f32 / frames as f32;
        assert!((0.25..=0.8).contains(&share), "{share}");
    }

    #[test]
    fn a_creature_out_of_reach_does_not_track() {
        let mut g = Glance::new(3);
        for _ in 0..600 {
            assert!(!g.advance(&SETTINGS, 1.0 / 30.0, false).0);
        }
    }
}
