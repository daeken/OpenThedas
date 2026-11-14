//! DA2's modern animation executor and its cancellable notification.
//!
//! Native trace and the limits of the headless controller are recorded in
//! `docs/notes/da2-animation-cadence.md`. This path is separate from Origins.

use thedas_formats::anim::{AnimInfo, AnimTree, parse_events};
use thedas_formats::gda::Cell;
use thedas_script::vm::ObjectId;

use crate::combat::AbilityTiming;
use crate::sim::Sim;
use crate::world::Command;

#[derive(Clone, Copy, Debug)]
pub(crate) struct AnimationSample {
    pub seconds: f32,
    pub speed: f32,
    pub impact: Option<f32>,
    pub cancel: Option<f32>,
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
    sample: AnimationSample,
    tune: [Option<f32>; 8],
    rate: f32,
    random: f32,
) -> AbilityTiming {
    let tuning_rate = match (tune[2], tune[3]) {
        (Some(low), Some(high)) if low > 0. && high > 0. => low + (high - low) * random,
        (Some(low), _) if low > 0. => low,
        _ => 1.,
    };
    let speed = sample.speed * tuning_rate * rate;
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
        let info = AnimInfo::parse(&self.vfs.read(&format!("{base}.gad")).ok()?.0).ok()?;
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
                Some(sample) => *sample,
                None => {
                    let sample = self.read_da2_animation(&tree, &action);
                    self.da2_animation_data.insert(key, sample);
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
        Some(schedule(sample, tune, rate, random))
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

    #[test]
    fn impact_chain_notification_and_unqueued_tail_have_distinct_deadlines() {
        let sample = AnimationSample {
            seconds: 2.0,
            speed: 1.,
            impact: Some(0.25),
            cancel: Some(0.625),
        };
        let mut tuning = [None; 8];
        tuning[2] = Some(1.25);
        tuning[4] = Some(0.25);
        let timing = schedule(sample, tuning, 1., 0.);
        assert_eq!(timing.impact_ms, 200);
        assert_eq!(timing.cancel_ms, Some(600));
        assert_eq!(timing.end_ms, 1400);
        let fast = schedule(sample, tuning, 2., 0.);
        assert_eq!(fast.impact_ms, 100);
        assert_eq!(fast.cancel_ms, Some(350));
        assert_eq!(fast.end_ms, 700);
        tuning[6] = Some(0.8);
        assert_eq!(schedule(sample, tuning, 2., 0.).cancel_ms, Some(500));
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
        };
        sim.da2_animation_data
            .insert("style.anb|attk_v1".into(), Some(sample));
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
