//! DA2's modern animation executor and its cancellable notification.
//!
//! Native trace and the limits of the headless controller are recorded in
//! `docs/notes/da2-animation-cadence.md`. This path is separate from Origins.

use thedas_formats::anim::{AnimInfo, AnimTree, parse_events};
use thedas_formats::gda::Cell;
use thedas_formats::gff4::{Gff4, List};
use thedas_formats::typed::Doc;
use thedas_script::vm::ObjectId;

use crate::combat::AbilityTiming;
use crate::sim::Sim;
use crate::world::Command;

#[derive(Clone, Debug)]
pub(crate) struct AnimationSample {
    pub seconds: f32,
    pub speed: f32,
    pub impact: Option<f32>,
    pub cancel: Option<f32>,
    pub motion: Vec<[f32; 4]>,
}

/// The native GAD motion track is separate from the pose tracks in ANI. Keep its
/// clock and transform in the save; an impact or reload must not restart the lunge.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub(crate) struct AnimationMotion {
    ability: i32,
    target: Option<ObjectId>,
    started: u64,
    speed: f32,
    origin: [f32; 3],
    facing: f32,
    scale: f32,
    keys: Vec<[f32; 4]>,
}

fn motion_keys(doc: &Gff4) -> Option<Vec<[f32; 4]>> {
    let d = Doc::new(doc, "DA2 animation motion");
    d.expect_type("GAD ").ok()?;
    // 0x90f5c0: GAD 4008 admits the animation to the motion controller.
    if d.opt_i64(doc.root, 4008).ok()?? == 0 {
        return None;
    }
    let track = d.structs(doc.root, 4005).ok()?.into_iter().find(|&t| {
        d.opt_name_or_hash(t, 4000)
            .ok()
            .flatten()
            .is_some_and(|name| {
                name == "gob_translation"
                    || thedas_formats::hash_of_hashed_name(&name)
                        == Some(thedas_formats::name_hash32("gob_translation"))
            })
    })?;
    // 0x911b50 -> 0x90c900: each flat key is seconds, x, y, z.
    if d.opt_i64(track, 4001).ok()?? != 6 || d.opt_i64(track, 4003).ok()?? != 4 {
        return None;
    }
    let List::F32(values) = d.get(track, 4004)?.as_list()? else {
        return None;
    };
    if values.is_empty() || values.len() % 4 != 0 || values.len() > 4 * (1 << 16) {
        return None;
    }
    let keys: Vec<[f32; 4]> = values
        .chunks_exact(4)
        .map(|k| [k[0], k[1], k[2], k[3]])
        .collect();
    (keys.iter().flatten().all(|v| v.is_finite())
        && keys[0][0] >= 0.
        && keys.windows(2).all(|w| w[0][0] < w[1][0]))
    .then_some(keys)
}

fn motion_at(keys: &[[f32; 4]], seconds: f32) -> [f32; 3] {
    let Some(first) = keys.first() else {
        return [0.; 3];
    };
    let mut p = [first[1], first[2], first[3]];
    if seconds <= first[0] {
        return p;
    }
    for pair in keys.windows(2) {
        let [a, b] = pair else { unreachable!() };
        if seconds <= b[0] {
            let f = (seconds - a[0]) / (b[0] - a[0]);
            return std::array::from_fn(|i| a[i + 1] + (b[i + 1] - a[i + 1]) * f);
        }
        p = [b[1], b[2], b[3]];
    }
    p
}

impl AnimationMotion {
    fn position(&self, now: u64) -> [f32; 3] {
        let local = motion_at(
            &self.keys,
            now.saturating_sub(self.started) as f32 * 0.001 * self.speed,
        );
        let angle = self.facing.to_radians();
        use crate::fmath::DetMath;
        let (sin, cos) = (angle.dsin(), angle.dcos());
        [
            self.origin[0] + (cos * local[0] - sin * local[1]) * self.scale,
            self.origin[1] + (sin * local[0] + cos * local[1]) * self.scale,
            self.origin[2] + local[2] * self.scale,
        ]
    }
}

fn text(cell: Option<&Cell>) -> Option<String> {
    match cell? {
        Cell::String(s) => Some(s.trim_end_matches('\0').to_ascii_lowercase()),
        _ => None,
    }
}

fn number(cell: Option<&Cell>) -> Option<f32> {
    let value = match cell? {
        Cell::Int(n) => *n as f32,
        Cell::Float(n) => *n as f32,
        _ => return None,
    };
    value.is_finite().then_some(value)
}

// The eight floats populated by 0x8a1bb0, in native record order.
const TUNING: [u32; 8] = [
    0x4dca_dd9b,
    0x311c_2dfc,
    0xb428_3d52,
    0xdef2_da69,
    0x2173_2cb1,
    0x4d28_f736,
    0xc393_0717,
    0x82cc_8766,
];

fn schedule(
    sample: &AnimationSample,
    tune: [Option<f32>; 8],
    rate: f32,
    random: f32,
) -> AbilityTiming {
    let speed = playback_speed(sample, tune, rate, random);
    let ms = |seconds: f32| (seconds.max(0.) * 1000.) as u64;
    // 0x6d8af5 subtracts the tuned tail before converting to milliseconds.
    let tail = tune[4].unwrap_or(0.);
    let end_ms = ms((sample.seconds - tail) / speed);
    let impact_ms = sample.impact.map(|t| ms(t / speed)).unwrap_or(end_ms);
    // A table deadline is already in command time. Animation events instead run
    // at the actual playback rate. The executor waits another 100 ms in either case.
    let cancel_ms = tune[6]
        .map(|t| ms(t / rate))
        .or_else(|| sample.cancel.map(|t| ms(t / speed)))
        .map(|t| t.saturating_add(100));
    AbilityTiming {
        impact_ms,
        end_ms: end_ms.max(impact_ms),
        cancel_ms,
    }
}

fn playback_speed(sample: &AnimationSample, tune: [Option<f32>; 8], rate: f32, random: f32) -> f32 {
    let tuning_rate = match (tune[2], tune[3]) {
        (Some(low), Some(high)) if low > 0. && high > 0. => low + (high - low) * random,
        (Some(low), _) if low > 0. => low,
        _ => 1.,
    };
    sample.speed * tuning_rate * rate
}

impl Sim {
    fn da2_animation_trees(&self, id: ObjectId) -> Vec<String> {
        let tables = self.world.tables();
        let style = self.da2_combat_style(id);
        let mut trees = Vec::new();
        if style != 0
            && let Some(styles) = tables.get(300, "")
        {
            trees.extend(
                text(styles.cell_by_hash(0xb421_4584, i64::from(style))).filter(|s| !s.is_empty()),
            );
        }
        if let Some(o) = self.world.get(id)
            && let Some(appearances) = tables.get(2, "")
        {
            if let Some(tree) = text(appearances.cell_by_hash(0xde80_b418, i64::from(o.appearance)))
                .filter(|s| !s.is_empty() && !trees.contains(s))
            {
                trees.push(tree);
            }
        }
        trees
    }

    fn read_da2_animation(&self, tree: &str, action: &str) -> Option<AnimationSample> {
        let tree = AnimTree::parse(&self.vfs.read(tree).ok()?.0).ok()?;
        let node = tree.action(action)?;
        let animation = node.animation.as_deref()?;
        let base = animation.strip_suffix(".ani").unwrap_or(animation);
        let gad = Gff4::parse(&self.vfs.read(&format!("{base}.gad")).ok()?.0).ok()?;
        let info = AnimInfo::from_gff(&gad).ok()?;
        let events = self
            .vfs
            .read(&format!("{base}.evt"))
            .ok()
            .and_then(|(bytes, _)| parse_events(&bytes).ok())
            .unwrap_or_default();
        let first = |kinds: &[i64]| {
            events
                .iter()
                .filter(|e| kinds.contains(&e.kind) && e.time.is_finite() && e.time >= 0.)
                .map(|e| e.time)
                .min_by(f32::total_cmp)
        };
        let speed = node.speed * info.speed;
        (speed.is_finite() && speed > 0.).then_some(AnimationSample {
            seconds: info.duration,
            speed,
            impact: first(&[7, 9]),
            cancel: first(&[41]),
            motion: motion_keys(&gad).unwrap_or_default(),
        })
    }

    pub(crate) fn da2_ability_timing(
        &mut self,
        id: ObjectId,
        ability: i32,
    ) -> Option<AbilityTiming> {
        let tables = self.world.tables();
        let abilities = tables.get(1, "")?;
        let animation = number(abilities.cell_by_hash(0x43c8_d895, i64::from(ability)))? as i64;
        if animation <= 0 {
            return None;
        }
        let actions = tables.get(0, "")?;
        let action = text(actions.cell_by_hash(0x7a39_8af9, animation))?;
        let mut resolved = None;
        for tree in self.da2_animation_trees(id) {
            let key = format!("{tree}|{action}");
            let sample = match self.da2_animation_data.get(&key) {
                Some(sample) => sample.clone(),
                None => {
                    let sample = self.read_da2_animation(&tree, &action);
                    self.da2_animation_data.insert(key, sample.clone());
                    sample
                }
            };
            if let Some(sample) = sample {
                resolved = Some((tree, sample));
                break;
            }
        }
        let (tree, sample) = resolved?;
        let mut tune = [None; 8];
        if let Some(tuning) = tables.get(3000, "") {
            let tree = tree.strip_suffix(".anb").unwrap_or(&tree);
            // Native 0x8bf0c0 overlays generic, tree, action and exact-pair rows.
            for (want_tree, want_action) in [
                ("", ""),
                (tree, ""),
                ("", action.as_str()),
                (tree, action.as_str()),
            ] {
                for &row in &tuning.order {
                    if text(tuning.cell_by_hash(0x1109_e419, row)).as_deref() != Some(want_tree)
                        || text(tuning.cell_by_hash(0xe4a4_4daf, row)).as_deref()
                            != Some(want_action)
                    {
                        continue;
                    }
                    for (slot, hash) in tune.iter_mut().zip(TUNING) {
                        if let Some(n) = number(tuning.cell_by_hash(hash, row)).filter(|&n| n >= 0.)
                        {
                            *slot = Some(n);
                        }
                    }
                    break;
                }
            }
        }
        // Modern ability executor 0x6d8448 reads property 42 as a percentage.
        let rate = (1. + self.property_total(id, 42) * 0.01).max(0.01);
        let random = if tune[2].is_some_and(|v| v > 0.) && tune[3].is_some_and(|v| v > 0.) {
            self.world.rng.unit()
        } else {
            0.
        };
        self.start_da2_animation_motion(
            id,
            ability,
            &sample,
            playback_speed(&sample, tune, rate, random),
        );
        Some(schedule(&sample, tune, rate, random))
    }

    fn start_da2_animation_motion(
        &mut self,
        id: ObjectId,
        ability: i32,
        sample: &AnimationSample,
        speed: f32,
    ) {
        let o = self.world.get(id);
        let motion = (|| {
            let o = o?;
            let cmd = o.commands.first()?;
            let abilities = self.world.tables().get(1, "")?;
            let flags = number(abilities.cell_by_hash(0x650e_5503, i64::from(ability)))? as i32;
            // Modern preparation 0x640e43 copies bit 1 into animation task bool 8.
            if flags & 2 == 0 || sample.motion.is_empty() {
                return None;
            }
            let mut scale = 1.;
            if self.ability_range_class(ability) > 1
                && let Some(target) = cmd
                    .target
                    .and_then(|t| self.world.get(t))
                    .filter(|t| t.area == o.area && t.id != id)
                && let Some(end) = sample.motion.last().filter(|k| k[2] > 0.)
            {
                // 0x6d8f64..0x6d9003: warp the motion's forward displacement
                // to end at close-range minimum + caster space + target bound.
                let ranges = self.world.tables().get(301, "")?;
                let close = number(ranges.cell_by_hash(0x6e4f_a70c, 1))?;
                scale = (crate::engine::dist(o.position, target.position)
                    - close
                    - self.body_radius(id)
                    - self.body_radius(target.id))
                    / end[2];
            }
            (scale.is_finite()).then(|| AnimationMotion {
                ability,
                target: cmd.target,
                started: self.world.time_ms,
                speed,
                origin: o.position,
                facing: o.facing,
                scale,
                keys: sample.motion.clone(),
            })
        })();
        if let Some(o) = self.world.get_mut(id) {
            o.combat.animation_motion = motion;
        }
    }

    /// Return the animation's absolute displacement before script impacts run.
    /// Canceled/replaced commands cannot continue moving their old caster.
    pub(crate) fn da2_animation_positions(&mut self) -> Vec<(ObjectId, [f32; 3])> {
        if self.game != thedas_vfs::Game::DragonAge2 {
            return Vec::new();
        }
        let now = self.world.time_ms;
        let mut positions = Vec::new();
        for &id in self.world.actor_ids().iter() {
            let Some(o) = self.world.get_mut(id) else {
                continue;
            };
            let Some(motion) = o.combat.animation_motion.as_ref() else {
                continue;
            };
            let active = o.active
                && !o.dead
                && o.parked.is_none()
                && o.commands.first().is_some_and(|c| {
                    c.name == "CommandUseAbility"
                        && c.ints.first() == Some(&motion.ability)
                        && c.target == motion.target
                        && c.due.is_some()
                        && c.approach.is_none()
                        && c.from.is_none()
                });
            if active {
                positions.push((o.id, motion.position(now)));
            } else {
                o.combat.animation_motion = None;
            }
        }
        positions
    }

    /// A ready animation stays active until another command is requested. Notify
    /// the creature's authored rules/AI rather than treating readiness as an impact.
    pub(crate) fn da2_animation_cancellable(&mut self, id: ObjectId, cmd: &Command) {
        let mut event = self.new_event("ABILITY_ANIM_CANCELLABLE", Some(id));
        event.objects = vec![id, cmd.target.unwrap_or(thedas_script::vm::OBJECT_INVALID)];
        event.ints = vec![cmd.ty, 1, cmd.ints.first().copied().unwrap_or(0), 1];
        self.world.queue_event(0, id, event, None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn motion_fixture() -> Gff4 {
        use thedas_formats::gff4::{
            FieldDef, FourCC, StructData, StructId, StructTemplate, Value, Version,
        };
        let template = |label, fields: &[u32]| StructTemplate {
            label: FourCC::new(label),
            size: 0,
            fields: fields
                .iter()
                .map(|&label| FieldDef {
                    label,
                    type_code: 0,
                    flags: 0,
                    offset: 0,
                })
                .collect(),
        };
        Gff4 {
            version: Version::V40,
            platform: FourCC::new(b"PC  "),
            file_type: FourCC::new(b"GAD "),
            file_version: FourCC::new(b"V1.0"),
            shared_strings: None,
            root: StructId(0),
            templates: vec![
                template(b"gad ", &[4008, 4005]),
                template(b"node", &[4000, 4001, 4003, 4004]),
            ],
            structs: vec![
                StructData {
                    template: 0,
                    values: vec![
                        Value::U8(1),
                        Value::List(Some(List::Struct(vec![Some(StructId(1))]))),
                    ],
                },
                StructData {
                    template: 1,
                    values: vec![
                        Value::U32(thedas_formats::name_hash32("gob_translation")),
                        Value::U32(6),
                        Value::U32(4),
                        Value::List(Some(List::F32(vec![
                            0., 0., 0., 0., 0.25, 0., 0., 0., 0.75, 0., 4., 0., 2., 0., 4., 0.,
                        ]))),
                    ],
                },
            ],
        }
    }

    #[test]
    fn gad_motion_uses_its_own_flat_timed_track_and_admission_flag() {
        use thedas_formats::gff4::Value;
        let mut doc = motion_fixture();
        let keys = motion_keys(&doc).unwrap();
        assert_eq!(motion_at(&keys, 0.2), [0.; 3]);
        assert_eq!(motion_at(&keys, 0.5), [0., 2., 0.]);
        assert_eq!(motion_at(&keys, 3.), [0., 4., 0.]);
        doc.structs[0].values[0] = Value::U8(0);
        assert!(motion_keys(&doc).is_none());
        doc.structs[0].values[0] = Value::U8(1);
        doc.structs[1].values[3] = Value::List(Some(List::F32(vec![0., 0., 0.])));
        assert!(motion_keys(&doc).is_none());
    }

    #[test]
    fn lunge_moves_before_impact_and_keeps_its_clock_across_reload_tail_and_cancel() {
        use crate::world::ObjectKind;
        use std::sync::Arc;
        let (mut sim, who) = running();
        let target = sim.world.spawn(ObjectKind::Creature, "target");
        sim.world.get_mut(target).unwrap().position = [0., 8., 0.];
        let tables = crate::m2da::Tables::empty();
        tables.insert(
            "#1",
            &["ID", "#650e5503", "#a7fa7789"],
            vec![(71, vec![Cell::Int(71), Cell::Int(3), Cell::Int(3)])],
        );
        tables.insert(
            "#301",
            &["ID", "#6e4fa70c"],
            vec![(1, vec![Cell::Int(1), Cell::Float(1.3)])],
        );
        sim.world.tables = Arc::new(tables);
        let command = &mut sim.world.get_mut(who).unwrap().commands[0];
        command.target = Some(target);
        command.impacted = false;
        command.after_impact_ms = Some(1000);
        command.due = Some(750);
        command.cancellable_at = None;
        let sample = AnimationSample {
            seconds: 2.,
            speed: 1.,
            impact: Some(0.75),
            cancel: None,
            motion: motion_keys(&motion_fixture()).unwrap(),
        };
        sim.start_da2_animation_motion(who, 71, &sample, 1.);
        sim.tick(500);
        let midway = sim.world.get(who).unwrap().position;
        assert!(midway[1] > 2. && midway[1] < 4.);
        let saved = serde_json::to_string(&sim.world).unwrap();
        sim.tick(250);
        let at_impact = sim.world.get(who).unwrap().position;
        let gap = 8. - at_impact[1];
        assert!((gap - 1.3 - sim.body_radius(who) - sim.body_radius(target)).abs() < 0.00001);
        assert!(gap < 2.5 + sim.body_radius(target));
        assert!(sim.world.get(who).unwrap().commands[0].impacted);
        sim.world = serde_json::from_str(&saved).unwrap();
        // Runtime table resources are restored separately from a world snapshot.
        let tables = crate::m2da::Tables::empty();
        sim.world.tables = Arc::new(tables);
        sim.tick(250);
        assert_eq!(sim.world.get(who).unwrap().position, at_impact);
        sim.tick(200);
        assert_eq!(sim.world.get(who).unwrap().position, at_impact);
        sim.order_wait(who, 1.);
        sim.tick(100);
        assert!(
            sim.world
                .get(who)
                .unwrap()
                .combat
                .animation_motion
                .is_none()
        );
        assert_eq!(sim.world.get(who).unwrap().position, at_impact);
        sim.world = serde_json::from_str(&saved).unwrap();
        sim.game = thedas_vfs::Game::Origins;
        sim.tick(100);
        assert_eq!(sim.world.get(who).unwrap().position, midway);
    }

    #[test]
    fn impact_chain_notification_and_unqueued_tail_have_distinct_deadlines() {
        let sample = AnimationSample {
            seconds: 2.0,
            speed: 1.,
            impact: Some(0.25),
            cancel: Some(0.625),
            motion: Vec::new(),
        };
        let mut tuning = [None; 8];
        tuning[2] = Some(1.25);
        tuning[4] = Some(0.25);
        let timing = schedule(&sample, tuning, 1., 0.);
        assert_eq!(timing.impact_ms, 200);
        assert_eq!(timing.cancel_ms, Some(600));
        assert_eq!(timing.end_ms, 1400);
        let fast = schedule(&sample, tuning, 2., 0.);
        assert_eq!(fast.impact_ms, 100);
        assert_eq!(fast.cancel_ms, Some(350));
        assert_eq!(fast.end_ms, 700);
        tuning[6] = Some(0.8);
        assert_eq!(schedule(&sample, tuning, 2., 0.).cancel_ms, Some(500));
    }

    fn running() -> (Sim, ObjectId) {
        use std::sync::Arc;
        use thedas_script::ldf::EngineTable;
        let table = EngineTable::parse(
            b"int COMMAND_TYPE_USE_ABILITY = 25;\n\
            int EVENT_TYPE_ABILITY_ANIM_CANCELLABLE = 112;\n\
            int EVENT_TYPE_COMMAND_COMPLETE = 9;\n",
        )
        .unwrap();
        let mut sim = Sim::new(Arc::new(thedas_vfs::Vfs::new()), Arc::new(table), 3);
        sim.game = thedas_vfs::Game::DragonAge2;
        let who = sim
            .world
            .spawn(crate::world::ObjectKind::Creature, "caster");
        sim.order_ability(who, 71, who, "");
        let command = &mut sim.world.get_mut(who).unwrap().commands[0];
        command.due = Some(1500);
        command.impacted = true;
        command.cancellable_at = Some(700);
        sim.world.trace.keep = true;
        (sim, who)
    }

    #[test]
    fn readiness_notifies_once_and_survives_reload_without_finishing_an_unqueued_attack() {
        use crate::trace::TraceEntry;
        let (mut sim, who) = running();
        sim.tick(699);
        assert!(!sim.world.get(who).unwrap().commands[0].cancellable);
        sim.world = serde_json::from_str(&serde_json::to_string(&sim.world).unwrap()).unwrap();
        sim.world.trace.keep = true;
        sim.tick(1);
        assert!(sim.world.get(who).unwrap().commands[0].cancellable);
        sim.tick(799);
        assert_eq!(sim.world.get(who).unwrap().commands.len(), 1);
        let notifications = sim
            .world
            .trace
            .entries
            .iter()
            .filter(|entry| matches!(entry, TraceEntry::EventQueued { ty: 112, .. }))
            .count();
        assert_eq!(notifications, 1);
        sim.tick(1);
        assert!(sim.world.get(who).unwrap().commands.is_empty());
    }

    #[test]
    fn a_queued_command_chains_only_after_the_notification() {
        let (mut sim, who) = running();
        let mut next = sim.world.get(who).unwrap().commands[0].clone();
        next.name = "CommandWait".into();
        next.seconds = 1.;
        sim.world.get_mut(who).unwrap().commands.push(next);
        let next = &mut sim.world.get_mut(who).unwrap().commands[1];
        next.due = None;
        next.cancellable_at = None;
        next.cancellable = false;
        sim.tick(699);
        assert_eq!(
            sim.world.get(who).unwrap().commands[0].name,
            "CommandUseAbility"
        );
        sim.tick(1);
        assert_eq!(sim.world.get(who).unwrap().commands[0].name, "CommandWait");
        assert_eq!(sim.world.get(who).unwrap().commands[0].due, Some(1700));
    }

    #[test]
    fn cancellable_event_keeps_the_native_command_and_target_slots() {
        let (mut sim, who) = running();
        let command = sim.world.get(who).unwrap().commands[0].clone();
        sim.da2_animation_cancellable(who, &command);
        let queued = sim.world.queue.values().next().unwrap();
        assert_eq!(queued.target, who);
        assert_eq!(queued.event.ty, 112);
        assert_eq!(queued.event.creator, Some(who));
        assert_eq!(queued.event.objects, [who, who]);
        assert_eq!(queued.event.ints, [25, 1, 71, 1]);
    }

    #[test]
    fn modern_animation_column_style_tree_and_monster_fallback_are_resolved() {
        use std::sync::Arc;
        let (mut sim, who) = running();
        let tables = crate::m2da::Tables::empty();
        tables.insert(
            "#1",
            &["ID", "#43c8d895"],
            vec![(71, vec![Cell::Int(71), Cell::Int(107)])],
        );
        tables.insert(
            "#0",
            &["ID", "#7a398af9"],
            vec![(107, vec![Cell::Int(107), Cell::String("attk_v1\0".into())])],
        );
        tables.insert(
            "#2",
            &["ID", "#de80b418"],
            vec![(
                10,
                vec![Cell::Int(10), Cell::String("monster.anb\0".into())],
            )],
        );
        tables.insert(
            "#163",
            &["ID", "#9ebe3727"],
            vec![(9, vec![Cell::Int(9), Cell::Int(3)])],
        );
        tables.insert(
            "#300",
            &["ID", "#b4214584"],
            vec![
                (0, vec![Cell::Int(0), Cell::String(String::new())]),
                (3, vec![Cell::Int(3), Cell::String("style.anb\0".into())]),
            ],
        );
        sim.world.tables = Arc::new(tables);
        let actor = sim.world.get_mut(who).unwrap();
        actor.appearance = 10;
        actor.fields.insert("Package".into(), 9);
        let sample = AnimationSample {
            seconds: 2.,
            speed: 1.,
            impact: Some(0.25),
            cancel: Some(0.5),
            motion: Vec::new(),
        };
        sim.da2_animation_data
            .insert("style.anb|attk_v1".into(), Some(sample.clone()));
        sim.da2_animation_data.insert(
            "monster.anb|attk_v1".into(),
            Some(AnimationSample {
                seconds: 3.,
                ..sample
            }),
        );
        assert_eq!(sim.ability_timing(who, 71).unwrap().end_ms, 2000);
        sim.da2_animation_data
            .insert("style.anb|attk_v1".into(), None);
        assert_eq!(sim.ability_timing(who, 71).unwrap().end_ms, 3000);
        sim.game = thedas_vfs::Game::Origins;
        assert_eq!(sim.ability_timing(who, 71), None);
    }
}
