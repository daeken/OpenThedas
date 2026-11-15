//! Starting and stopping: how a creature's body goes from standing to running and back, as
//! DAOrigins.exe's movement controller (0xa69a00–0xa6c2b0) blends it. Presentation only:
//! it follows each creature's motion as [`crate::models::motion`] reads it from the
//! simulation (running while a move is under way, standing otherwise) and keeps, per
//! creature, the weights the exe's faders would have.
//!
//! - The movement tree (`movement.anb`) has two branches: `Motion` (the running cycle,
//!   `runf`) and `Transition` (one-shot clips: `pauserunf` to start, `runpausefr` /
//!   `runpausefl` to stop on the right or left foot, `shuffle`).
//! - **Starting** (controller state 4, 0xa6ae30 → 0xa6a300): the start clip plays from its
//!   start at speed 1; the movement layer and `Transition` fade to 1 and `Motion` to 0 at
//!   4/s (0xadef1c), while the standing pose fades out at 2.5/s (the agent's state 0 at
//!   0xb03b08, 0xa6a417). When the clip ends (events 11/13, 0xa6bffb) `Motion` fades in
//!   at 4/s and the running cycle starts there, at the offset the start clip's event 26
//!   gave (its payload / 30 seconds, 0xa6c274).
//! - **Stopping** (state 8, 0xa6b760 → 0xa6ad50): the stop clip the last event 25 passed
//!   named (0xa6c0b9: the running cycle's markers, `runpausefr` at 0.07 s and 0.80 s,
//!   `runpausefl` at 0.40 s in the core clip; before the first, the start clip's) plays
//!   from its start under `Transition`; at its end the movement layer fades to 0 at 4/s
//!   and the standing pose back in. No clip named (or `pause`): straight to standing.
//!   While the start clip is between an event 23 and the next 24 (0xa6bce3), a stop
//!   waits.
//! - A fader moves its weight toward its target by its rate per second (0x815130,
//!   0x8151f0), so a whole 0 → 1 change takes 0.25 s at 4/s and 0.4 s at 2.5/s.
//!
//! Provisional (not mirrored): starting with a turn (`pauseturn{l,r}{90,180}runf`, chosen
//! by the angle to the new heading, 45° and 157.5°), turning in place (`turnl90` /
//! `turnr90`), walking and the run/walk weights by speed, leaning into turns, the playback
//! rate following speed, authored transition curves (`.tbi`), and the agent's time scale.
//! Weights blend the poses in order rather than by the exe's tree evaluation (0x814c22).

use std::collections::HashMap;
use std::sync::Arc;

use thedas_formats::anim::{AnimInfo, AnimTree, parse_events_with_text};
use thedas_script::vm::ObjectId;
use thedas_sim::Sim;
use thedas_sim::world::ObjectKind;

use crate::models::{Motion, ObjectModel, action, movement_trees, standing_motion};

/// How long a creature may stand still between moves and still be moving (ms).
pub const MOVE_GAP_MS: u64 = 150;
/// The movement layer's fade rate (weight per second; 0xadef1c).
pub const MOVE_RATE: f32 = 4.0;
/// The standing pose's fade-out rate as a start begins (0xb03b08).
pub const CLEAR_RATE: f32 = 2.5;

/// Event kinds of the movement clips (`.evt`, field 4017).
mod event {
    /// Protects a start from being stopped, until the next [`UNPROTECT`].
    pub const PROTECT: i64 = 23;
    pub const UNPROTECT: i64 = 24;
    /// Names the clip a stop from here plays.
    pub const NEXT: i64 = 25;
    /// The running cycle's start offset after this clip, in frames of 1/30 s.
    pub const MOTION_OFFSET: i64 = 26;
}

/// The start and stop clips' and the running cycle's action names.
const START: &str = "pauserunf";

/// A weight moving linearly toward a target.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Fader {
    value: f32,
    target: f32,
    rate: f32,
}

impl Fader {
    fn at(value: f32) -> Self {
        Fader {
            value,
            target: value,
            rate: MOVE_RATE,
        }
    }

    fn to(&mut self, target: f32, rate: f32) {
        self.target = target;
        self.rate = rate;
    }

    fn step(&mut self, dt: f32) {
        let d = self.target - self.value;
        let max = self.rate * dt;
        self.value = if d.abs() <= max {
            self.target
        } else {
            self.value + max * d.signum()
        };
    }
}

/// What a movement clip's files say.
#[derive(Clone, Debug, Default)]
struct ClipInfo {
    /// Seconds at speed 1.
    length: f32,
    /// Event 25 markers: (time, the clip a stop from here plays).
    next: Vec<(f32, String)>,
    /// Events 23 and 24: (time, protected from then on).
    protect: Vec<(f32, bool)>,
    /// Event 26: the running cycle's start offset (seconds).
    offset: Option<f32>,
}

impl ClipInfo {
    /// The stop clip named by the last marker at or before `t` (`None` when none is).
    fn next_at(&self, t: f32) -> Option<&str> {
        self.next
            .iter()
            .filter(|(at, _)| *at <= t)
            .last()
            .map(|(_, n)| n.as_str())
    }

    fn protected_at(&self, t: f32) -> bool {
        self.protect
            .iter()
            .filter(|(at, _)| *at <= t)
            .last()
            .is_some_and(|(_, p)| *p)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Idle,
    Starting,
    Running,
    Stopping,
}

/// One creature's movement controller.
#[derive(Clone, Debug)]
struct Gait {
    phase: Phase,
    /// The standing pose's weight.
    standing: Fader,
    /// The movement layer's weight.
    layer: Fader,
    /// `Motion`'s weight within the layer (`Transition` takes the rest).
    motion: Fader,
    /// The one-shot playing under `Transition`: its action and when it started (ms).
    clip: Option<(String, u64)>,
    /// The one-shot it replaced, fading out.
    previous: Option<(String, u64, Fader)>,
    /// When the running cycle started (ms) and its offset (seconds).
    cycle: (u64, f32),
    /// The stop clip the start clip's markers named, for a stop before the cycle's first.
    start_next: Option<String>,
    /// The movement trees the run came from.
    trees: Vec<String>,
    /// Where the creature stood last frame, and when it last moved (ms).
    last: Option<([f32; 3], u64)>,
}

impl Gait {
    fn idle() -> Self {
        Gait {
            phase: Phase::Idle,
            standing: Fader::at(1.0),
            layer: Fader::at(0.0),
            motion: Fader::at(0.0),
            clip: None,
            previous: None,
            cycle: (0, 0.0),
            start_next: None,
            trees: Vec::new(),
            last: None,
        }
    }

    /// Already running (first seen moving): the cycle at full weight.
    fn running(now: u64, trees: Vec<String>) -> Self {
        Gait {
            phase: Phase::Running,
            standing: Fader::at(0.0),
            layer: Fader::at(1.0),
            motion: Fader::at(1.0),
            cycle: (now, 0.0),
            trees,
            ..Gait::idle()
        }
    }

    /// Plays one-shot `action` from its start under `Transition` (0xa6a300).
    fn play(&mut self, action: &str, now: u64) {
        if let Some((a, at)) = self.clip.take() {
            let mut f = Fader::at(1.0 - self.motion.value);
            f.to(0.0, MOVE_RATE);
            self.previous = Some((a, at, f));
        }
        self.clip = Some((action.to_string(), now));
        self.layer.to(1.0, MOVE_RATE);
        self.motion.to(0.0, MOVE_RATE);
    }

    fn clip_time(&self, now: u64) -> f32 {
        self.clip
            .as_ref()
            .map_or(0.0, |(_, at)| now.saturating_sub(*at) as f32 / 1000.0)
    }

    fn cycle_time(&self, now: u64) -> f32 {
        now.saturating_sub(self.cycle.0) as f32 / 1000.0 + self.cycle.1
    }
}

/// The movement controllers of the creatures drawn, kept between frames.
#[derive(Default)]
pub struct Locomotion {
    gaits: HashMap<ObjectId, Gait>,
    /// By (tree, action): the clip's files, read once.
    clips: HashMap<(String, String), Option<Arc<ClipInfo>>>,
    last_ms: Option<u64>,
}

impl Locomotion {
    /// Advances every creature's controller to the simulation's time and gives each running
    /// or standing creature's motion its blend.
    pub fn apply(&mut self, sim: &Sim, models: &mut [ObjectModel]) {
        let now = sim.world.time_ms;
        let dt = self
            .last_ms
            .map_or(0.0, |l| now.saturating_sub(l) as f32 / 1000.0);
        // A load or a jump back in time starts everyone afresh.
        if self.last_ms.is_some_and(|l| now < l) {
            self.gaits.clear();
        }
        self.last_ms = Some(now);
        let mut seen = Vec::with_capacity(models.len());
        for m in models.iter_mut() {
            if m.kind != ObjectKind::Creature {
                continue;
            }
            let Some(motion) = m.motion.as_ref() else {
                continue;
            };
            let first = motion.actions.first().map(String::as_str);
            let running = match first {
                Some(action::RUN) => true,
                Some(action::PAUSE) if motion.looping => false,
                // Anything else (an attack, a death, a played animation) takes the body
                // whole; the controller starts again from standing.
                _ => {
                    self.gaits.remove(&m.id);
                    continue;
                }
            };
            seen.push(m.id);
            let Some(o) = sim.world.get(m.id) else {
                continue;
            };
            let gait = self.gaits.entry(m.id).or_insert_with(|| {
                if running {
                    Gait::running(now, motion.trees.clone())
                } else {
                    Gait::idle()
                }
            });
            // Moving: running, or carried along. The exe's controller follows the creature's
            // speed (C+0x134); the simulation stands a moment between one move and the
            // next (a held stick's moves are issued again and again), which is not a stop.
            // Provisional: the gap allowed.
            let moved = gait.last.is_some_and(|(p, _)| {
                let d = [p[0] - m.position[0], p[1] - m.position[1]];
                d[0] * d[0] + d[1] * d[1] > 1e-6
            });
            let last_moved = match gait.last {
                Some((_, at)) if !(running || moved) => at,
                _ => now,
            };
            gait.last = Some((m.position, last_moved));
            let going = matches!(gait.phase, Phase::Starting | Phase::Running);
            let moving = running || (going && now.saturating_sub(last_moved) < MOVE_GAP_MS);
            if running {
                gait.trees = motion.trees.clone();
            } else if gait.trees.is_empty() {
                gait.trees = movement_trees(sim, o);
            }
            let standing = if running {
                standing_motion(sim, o)
            } else {
                Some(motion.clone())
            };
            let trees = gait.trees.clone();
            step(gait, &mut self.clips, sim, &trees, moving, now, dt);
            log::trace!(
                "gait {} {}: {:?} moving {moving} standing {:.2} layer {:.2} motion {:.2} clip {:?} cycle {:.2}",
                m.id,
                m.tag,
                gait.phase,
                gait.standing.value,
                gait.layer.value,
                gait.motion.value,
                gait.clip
                    .as_ref()
                    .map(|(a, at)| (a.clone(), now.saturating_sub(*at))),
                gait.cycle_time(now)
            );
            if let Some(blended) = compose(gait, standing, &self.clips, now) {
                m.motion = Some(blended);
            }
        }
        self.gaits.retain(|id, _| seen.contains(id));
    }
}

/// A clip's files: its `.gad` length and `.evt` events, through the tree naming it.
fn clip_info(
    clips: &mut HashMap<(String, String), Option<Arc<ClipInfo>>>,
    sim: &Sim,
    trees: &[String],
    action: &str,
) -> Option<Arc<ClipInfo>> {
    let tree = trees.first()?.clone();
    clips
        .entry((tree.clone(), action.to_string()))
        .or_insert_with(|| {
            let (data, _) = sim.vfs.read(&tree).ok()?;
            let t = AnimTree::parse(&data).ok()?;
            let ani = t.action(action)?.animation.clone()?;
            let stem = ani.trim_end_matches('\0');
            let stem = stem
                .rsplit_once('.')
                .filter(|(_, e)| e.eq_ignore_ascii_case("ani"))
                .map_or(stem, |(s, _)| s);
            let length = sim
                .vfs
                .read(&format!("{stem}.gad"))
                .ok()
                .and_then(|(d, _)| AnimInfo::parse(&d).ok())
                .map_or(0.0, |g| g.duration);
            let mut info = ClipInfo {
                length,
                ..Default::default()
            };
            if let Some(events) = sim
                .vfs
                .read(&format!("{stem}.evt"))
                .ok()
                .and_then(|(d, _)| parse_events_with_text(&d).ok())
            {
                for e in events {
                    let text = e
                        .text
                        .as_deref()
                        .unwrap_or_default()
                        .trim_end_matches('\0')
                        .trim()
                        .to_ascii_lowercase();
                    match e.kind {
                        event::NEXT if !text.is_empty() => info.next.push((e.time, text)),
                        event::PROTECT => info.protect.push((e.time, true)),
                        event::UNPROTECT => info.protect.push((e.time, false)),
                        event::MOTION_OFFSET => {
                            info.offset = text.parse::<f32>().ok().map(|f| f / 30.0)
                        }
                        _ => {}
                    }
                }
            }
            Some(Arc::new(info))
        })
        .clone()
}

/// The stop clip a stop now plays: the running cycle's last marker passed, else the start
/// clip's (0xa69ba0; `pause` means none).
fn stop_clip(
    gait: &Gait,
    clips: &mut HashMap<(String, String), Option<Arc<ClipInfo>>>,
    sim: &Sim,
    trees: &[String],
    now: u64,
) -> Option<String> {
    let named = match gait.phase {
        Phase::Running => {
            let run = clip_info(clips, sim, trees, action::RUN);
            let t = gait.cycle_time(now);
            run.as_ref()
                .filter(|r| r.length > 0.0 && !r.next.is_empty())
                .and_then(|r| {
                    let laps = (t / r.length).floor();
                    let at = t - laps * r.length;
                    r.next_at(at)
                        .or_else(|| {
                            (laps >= 1.0).then(|| r.next.last().map(|(_, n)| n.as_str()))?
                        })
                        .map(str::to_string)
                })
                .or_else(|| gait.start_next.clone())
        }
        Phase::Starting => {
            let (a, _) = gait.clip.as_ref()?;
            let info = clip_info(clips, sim, trees, a)?;
            info.next_at(gait.clip_time(now)).map(str::to_string)
        }
        _ => None,
    };
    named.filter(|n| n != action::PAUSE)
}

fn step(
    gait: &mut Gait,
    clips: &mut HashMap<(String, String), Option<Arc<ClipInfo>>>,
    sim: &Sim,
    trees: &[String],
    moving: bool,
    now: u64,
    dt: f32,
) {
    let playing = |gait: &Gait, clips: &mut HashMap<_, _>| {
        gait.clip
            .as_ref()
            .and_then(|(a, _)| clip_info(clips, sim, trees, a))
    };
    match (gait.phase, moving) {
        (Phase::Idle | Phase::Stopping, true) => {
            if clip_info(clips, sim, trees, START).is_some_and(|c| c.length > 0.0) {
                gait.phase = Phase::Starting;
                gait.play(START, now);
                gait.standing.to(0.0, CLEAR_RATE);
                gait.start_next = None;
            } else {
                // No start clip: straight into the cycle.
                gait.phase = Phase::Running;
                gait.cycle = (now, 0.0);
                gait.layer.to(1.0, MOVE_RATE);
                gait.motion.to(1.0, MOVE_RATE);
                gait.standing.to(0.0, CLEAR_RATE);
            }
        }
        (Phase::Starting, true) => {
            let info = playing(gait, clips);
            let t = gait.clip_time(now);
            if let Some(n) = info.as_ref().and_then(|i| i.next_at(t)) {
                gait.start_next = Some(n.to_string());
            }
            if info.as_ref().is_none_or(|i| t >= i.length) {
                gait.phase = Phase::Running;
                gait.cycle = (now, info.and_then(|i| i.offset).unwrap_or(0.0));
                gait.motion.to(1.0, MOVE_RATE);
            }
        }
        (Phase::Starting | Phase::Running, false) => {
            // A start between its events 23 and 24 is not stopped until its end (0xa6bce3;
            // its end clears the protection, 0xa6bffb).
            let t = gait.clip_time(now);
            let protected = gait.phase == Phase::Starting
                && playing(gait, clips).is_some_and(|i| t < i.length && i.protected_at(t));
            if !protected {
                match stop_clip(gait, clips, sim, trees, now)
                    .filter(|s| clip_info(clips, sim, trees, s).is_some_and(|c| c.length > 0.0))
                {
                    Some(stop) => {
                        gait.phase = Phase::Stopping;
                        gait.play(&stop, now);
                    }
                    None => to_idle(gait),
                }
            }
        }
        (Phase::Stopping, false) => {
            let t = gait.clip_time(now);
            if playing(gait, clips).is_none_or(|i| t >= i.length) {
                to_idle(gait);
            }
        }
        (Phase::Running, true) | (Phase::Idle, false) => {}
    }
    gait.standing.step(dt);
    gait.layer.step(dt);
    gait.motion.step(dt);
    if let Some((_, _, f)) = gait.previous.as_mut() {
        f.step(dt);
    }
    if gait
        .previous
        .as_ref()
        .is_some_and(|(_, _, f)| f.value <= 0.0)
    {
        gait.previous = None;
    }
}

/// Back to standing (0xa6ac22): the movement layer fades out, the standing pose in.
fn to_idle(gait: &mut Gait) {
    gait.phase = Phase::Idle;
    gait.layer.to(0.0, MOVE_RATE);
    gait.standing.to(1.0, MOVE_RATE);
}

/// The creature's motion: the standing pose, then the movement layer's one-shots and
/// running cycle at their weights, each blended into what comes before it.
fn compose(
    gait: &Gait,
    standing: Option<Motion>,
    clips: &HashMap<(String, String), Option<Arc<ClipInfo>>>,
    now: u64,
) -> Option<Motion> {
    let layer = gait.layer.value;
    let motion = gait.motion.value;
    let one_shot = |a: &str, at: u64| Motion {
        trees: gait.trees.clone(),
        actions: vec![a.to_string()],
        time: now.saturating_sub(at) as f32 / 1000.0,
        looping: false,
        blend: Vec::new(),
    };
    let mut parts: Vec<(Motion, f32)> = Vec::new();
    if let Some(s) = standing {
        parts.push((s, gait.standing.value));
    }
    if let Some((a, at, f)) = &gait.previous {
        parts.push((one_shot(a, *at), layer * f.value));
    }
    if let Some((a, at)) = &gait.clip {
        let previous = gait.previous.as_ref().map_or(0.0, |(_, _, f)| f.value);
        parts.push((one_shot(a, *at), layer * (1.0 - motion - previous).max(0.0)));
    }
    let cycle_known = clips
        .get(&(
            gait.trees.first().cloned().unwrap_or_default(),
            action::RUN.to_string(),
        ))
        .is_none_or(|c| c.is_some());
    if cycle_known {
        parts.push((
            Motion {
                trees: gait.trees.clone(),
                actions: vec![action::RUN.into()],
                time: gait.cycle_time(now),
                looping: true,
                blend: Vec::new(),
            },
            layer * motion,
        ));
    }
    blend(parts)
}

/// Lays weighted motions over each other in order: the first with weight above zero is the
/// base, and each after it is blended in by its weight over the total so far, so that the
/// result weighs each by its share.
fn blend(parts: Vec<(Motion, f32)>) -> Option<Motion> {
    let mut parts = parts.into_iter().filter(|(_, w)| *w > 1e-4);
    let (mut base, mut total) = parts.next()?;
    for (m, w) in parts {
        total += w;
        base.blend.push((m, w / total));
    }
    Some(base)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(a: &str) -> Motion {
        Motion {
            trees: vec!["t".into()],
            actions: vec![a.into()],
            time: 0.0,
            looping: true,
            blend: Vec::new(),
        }
    }

    #[test]
    fn a_fader_moves_by_its_rate_and_stops_at_its_target() {
        let mut f = Fader::at(0.0);
        f.to(1.0, MOVE_RATE);
        f.step(0.1);
        assert!((f.value - 0.4).abs() < 1e-6);
        f.step(0.2);
        assert_eq!(f.value, 1.0);
        f.to(0.0, CLEAR_RATE);
        f.step(0.2);
        assert!((f.value - 0.5).abs() < 1e-6);
    }

    #[test]
    fn blending_weighs_each_part_by_its_share() {
        let out = blend(vec![(m("a"), 0.0), (m("b"), 0.5), (m("c"), 0.5)]).unwrap();
        assert_eq!(out.actions, ["b"]);
        assert_eq!(out.blend.len(), 1);
        assert_eq!(out.blend[0].0.actions, ["c"]);
        assert!((out.blend[0].1 - 0.5).abs() < 1e-6);
        let out = blend(vec![(m("a"), 1.0), (m("b"), 1.0), (m("c"), 1.0)]).unwrap();
        let shares: Vec<f32> = out.blend.iter().map(|(_, w)| *w).collect();
        assert!((shares[0] - 0.5).abs() < 1e-6 && (shares[1] - 1.0 / 3.0).abs() < 1e-6);
        assert!(blend(vec![(m("a"), 0.0)]).is_none());
    }

    #[test]
    fn a_run_starts_with_its_clip_and_stops_on_the_foot_last_marked() {
        let sim = Sim::new(
            Arc::new(thedas_vfs::Vfs::new()),
            Arc::new(thedas_script::EngineTable::default()),
            1,
        );
        let trees = vec!["movement.anb".to_string()];
        let mut clips = HashMap::new();
        let mut put = |a: &str, info: ClipInfo| {
            clips.insert((trees[0].clone(), a.to_string()), Some(Arc::new(info)));
        };
        put(
            START,
            ClipInfo {
                length: 0.2,
                next: vec![(0.0, "shuffle".into()), (0.1, "runpausefr".into())],
                protect: vec![(0.1, true)],
                offset: None,
            },
        );
        put(
            action::RUN,
            ClipInfo {
                length: 1.16667,
                next: vec![
                    (0.07, "runpausefr".into()),
                    (0.4, "runpausefl".into()),
                    (0.8, "runpausefr".into()),
                ],
                ..Default::default()
            },
        );
        for stop in ["runpausefr", "runpausefl", "shuffle"] {
            put(
                stop,
                ClipInfo {
                    length: 2.26667,
                    ..Default::default()
                },
            );
        }
        let mut g = Gait::idle();
        g.trees = trees.clone();
        let mut t = 1000;
        let mut run = |g: &mut Gait, moving: bool, ms: u64, t: &mut u64| {
            let mut left = ms;
            while left > 0 {
                let d = left.min(10);
                *t += d;
                left -= d;
                step(g, &mut clips, &sim, &trees, moving, *t, d as f32 / 1000.0);
            }
        };
        run(&mut g, true, 10, &mut t);
        assert_eq!(g.phase, Phase::Starting);
        assert_eq!(g.clip.as_ref().unwrap().0, START);
        // Stopped at once: the start's first marker names the shuffle.
        run(&mut g, false, 10, &mut t);
        assert_eq!(g.phase, Phase::Stopping);
        assert_eq!(g.clip.as_ref().unwrap().0, "shuffle");
        // Started again and stopped in its protected tail: held off until its end, then
        // the right foot's stop.
        run(&mut g, true, 130, &mut t);
        assert_eq!(g.phase, Phase::Starting);
        run(&mut g, false, 40, &mut t);
        assert_eq!(g.phase, Phase::Starting);
        run(&mut g, false, 60, &mut t);
        assert_eq!(g.phase, Phase::Stopping);
        assert_eq!(g.clip.as_ref().unwrap().0, "runpausefr");
        run(&mut g, true, 300, &mut t);
        assert_eq!(g.phase, Phase::Running);
        assert!((g.layer.value - 1.0).abs() < 1e-6);
        // Half a second into the cycle: the left foot's marker was the last.
        let since = t - g.cycle.0;
        run(&mut g, true, 500 - since, &mut t);
        run(&mut g, false, 10, &mut t);
        assert_eq!(g.phase, Phase::Stopping);
        assert_eq!(g.clip.as_ref().unwrap().0, "runpausefl");
        run(&mut g, false, 2300, &mut t);
        assert_eq!(g.phase, Phase::Idle);
        run(&mut g, false, 300, &mut t);
        assert_eq!((g.standing.value, g.layer.value), (1.0, 0.0));
    }

    #[test]
    fn the_stop_clip_is_the_last_marker_passed() {
        let info = ClipInfo {
            length: 1.0,
            next: vec![(0.07, "runpausefr".into()), (0.4, "runpausefl".into())],
            protect: vec![(0.0, true), (0.07, false)],
            offset: None,
        };
        assert_eq!(info.next_at(0.05), None);
        assert_eq!(info.next_at(0.07), Some("runpausefr"));
        assert_eq!(info.next_at(0.5), Some("runpausefl"));
        assert!(info.protected_at(0.03));
        assert!(!info.protected_at(0.08));
    }
}
