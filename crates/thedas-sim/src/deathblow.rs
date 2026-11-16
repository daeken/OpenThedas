//! Origins paired death animation preparation and its separate script-death deadline.
//! Static trace and remaining selection/placement approximations are in
//! `docs/notes/deathblow-executor.md`.

use serde::{Deserialize, Serialize};
use thedas_formats::anim::{AnimInfo, AnimTree, parse_events_with_text};
use thedas_script::vm::ObjectId;

use crate::Sim;
use crate::world::{Command, ObjectKind};

const SYNC_DEATH_ANIMATIONS: i32 = 102;
const ATTACK_ANIMATION: u32 = 0xf954_09f5;
const REACTION_ANIMATION: u32 = 0x2855_abec;
const ACTION: u32 = 0x7a39_8af9;

/// A victim's paired animation, available from preparation through its corpse pose.
/// Presentation should play `action` from `started_ms`, retaining its final pose at
/// `ends_ms`. A live creature restored by resurrection discards this record.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeathBlowReaction {
    pub attacker: ObjectId,
    pub started_ms: u64,
    pub ends_ms: u64,
    /// Row of the merged animation table (M2DA 0).
    pub animation: i32,
    pub action: String,
}

/// The attacker's selected animation and outstanding hit markers. This is saved so
/// resuming a paired animation neither selects again nor repeats an impact.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeathBlow {
    pub victim: ObjectId,
    pub started_ms: u64,
    pub ends_ms: u64,
    pub animation: i32,
    pub action: String,
    pub impacts: Vec<DeathBlowImpact>,
    pub next_impact: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeathBlowImpact {
    pub at_ms: u64,
    pub hand: usize,
}

struct Clip {
    animation: i32,
    action: String,
    duration_ms: u64,
    impacts: Vec<(u64, usize)>,
}

impl Sim {
    fn death_blow_clip(&self, id: ObjectId, animation: i32) -> Option<Clip> {
        let table = self.world.tables().get(0, "")?;
        let action = crate::engine::cell_string(table.cell_by_hash(ACTION, animation as i64))
            .trim_end_matches('\0')
            .to_ascii_lowercase();
        let read = |name: &str| self.vfs.read(name).ok().map(|(bytes, _)| bytes);
        // Provisional tree precedence, shared with ordinary ability playback.
        for tree in self.ability_trees(id) {
            let Some(tree) = read(&tree).and_then(|bytes| AnimTree::parse(&bytes).ok()) else {
                continue;
            };
            let Some(node) = tree.action(&action) else {
                continue;
            };
            let Some(animation_name) = node.animation.as_deref() else {
                continue;
            };
            let base = animation_name
                .strip_suffix(".ani")
                .unwrap_or(animation_name);
            let Some(info) =
                read(&format!("{base}.gad")).and_then(|bytes| AnimInfo::parse(&bytes).ok())
            else {
                continue;
            };
            let speed = (node.speed * info.speed).max(0.01);
            let events = read(&format!("{base}.evt"))
                .and_then(|bytes| parse_events_with_text(&bytes).ok())
                .unwrap_or_default();
            // Native 0x6c0c45 -> 0x6a6230 accepts kind 1. In particular, the
            // visual contact markers (kind 19) in paired clips do not apply hits.
            let mut impacts: Vec<_> = events
                .iter()
                .filter(|event| {
                    event.kind == 1
                        && event.time.is_finite()
                        && event.time >= 0.0
                        && event.time <= info.duration
                })
                .map(|event| {
                    let hand =
                        usize::from(event.text.as_deref().is_some_and(|s| s.contains("Left")));
                    ((event.time / speed * 1000.0).round() as u64, hand)
                })
                .collect();
            impacts.sort_by_key(|&(time, _)| time);
            return Some(Clip {
                animation,
                action,
                duration_ms: (info.duration / speed * 1000.0).round() as u64,
                impacts,
            });
        }
        None
    }

    /// Resolve the explicit SyncDeathAnimations row, or the default's first playable
    /// pair. Provisional default selection: the native animation-selection callback's
    /// filters and random variant choice have not been ported.
    fn death_blow_pair(
        &self,
        id: ObjectId,
        victim: ObjectId,
        row: i32,
    ) -> Option<(i64, Clip, Clip)> {
        let table = self.world.tables().get(SYNC_DEATH_ANIMATIONS, "")?;
        let rows = if row == 0 {
            table.order.clone()
        } else {
            vec![row as i64]
        };
        for row in rows {
            let attack = crate::engine::cell_int(table.cell_by_hash(ATTACK_ANIMATION, row));
            let reaction = crate::engine::cell_int(table.cell_by_hash(REACTION_ANIMATION, row));
            if attack <= 0 || reaction <= 0 {
                continue;
            }
            if let (Some(attack), Some(reaction)) = (
                self.death_blow_clip(id, attack),
                self.death_blow_clip(victim, reaction),
            ) {
                return Some((row, attack, reaction));
            }
        }
        None
    }

    /// Starts both sides together. The victim's native death task zeroes health on
    /// its first execution, then sends DEATH after its animation deadline.
    pub(crate) fn start_death_blow(&mut self, id: ObjectId, command: &Command) -> Option<u64> {
        let victim = command.target?;
        if id == victim
            || !self.alive(id)
            || !self.alive(victim)
            || self.world.get(victim)?.kind != ObjectKind::Creature
            || self.world.get(id)?.area != self.world.get(victim)?.area
        {
            return None;
        }
        let row = command.ints.first().copied().unwrap_or(0);
        let (row, attack, reaction) = self.death_blow_pair(id, victim, row)?;
        let now = self.world.time_ms;
        // Ordinary pairs use the context's negative distance, the combined body
        // radii plus 1.2 m (0x746384), along the victim-to-attacker bearing.
        // Provisional: large-creature sync-table placement, floor projection and
        // the native placement blend are not ported.
        if !(19..=23).contains(&row) {
            let from = self.world.get(id)?.position;
            let target = self.world.get(victim)?.position;
            let (dx, dy) = (target[0] - from[0], target[1] - from[1]);
            let length = (dx * dx + dy * dy).sqrt();
            if length > 1e-6 {
                let distance = self.capsule_radius(id) + self.capsule_radius(victim) + 1.2;
                self.move_object(
                    id,
                    [
                        target[0] - dx / length * distance,
                        target[1] - dy / length * distance,
                        from[2],
                    ],
                    None,
                );
                self.world.get_mut(id)?.facing = crate::world::facing_toward(dx, dy);
                self.world.get_mut(victim)?.facing = crate::world::facing_toward(-dx, -dy);
            }
        }
        let ends_ms = now.saturating_add(attack.duration_ms.max(1));
        let impacts = attack
            .impacts
            .into_iter()
            .map(|(time, hand)| DeathBlowImpact {
                at_ms: now.saturating_add(time).min(ends_ms),
                hand,
            })
            .collect::<Vec<_>>();
        let due = impacts.first().map_or(ends_ms, |impact| impact.at_ms);
        self.world.get_mut(id)?.combat.death_blow = Some(DeathBlow {
            victim,
            started_ms: now,
            ends_ms,
            animation: attack.animation,
            action: attack.action,
            impacts,
            next_impact: 0,
        });
        self.world.get_mut(victim)?.combat.death_blow_reaction = Some(DeathBlowReaction {
            attacker: id,
            started_ms: now,
            ends_ms: now.saturating_add(reaction.duration_ms.max(1)),
            animation: reaction.animation,
            action: reaction.action,
        });
        self.kill_with_event_delay(victim, id, reaction.duration_ms.max(1));
        Some(due.saturating_sub(now))
    }

    /// Keep the existing simulation's resurrection consistent with paired state.
    /// Provisional: native interruption of an active death task remains untraced.
    pub(crate) fn cancel_death_blow_reaction(&mut self, id: ObjectId) {
        let reaction = self
            .world
            .get_mut(id)
            .and_then(|o| o.combat.death_blow_reaction.take());
        if let Some(reaction) = reaction {
            let death = crate::sim::constant(&self.table, "EVENT_TYPE_DEATH");
            self.world.queue.retain(|&(at, _), queued| {
                !(at == reaction.ends_ms
                    && queued.target == id
                    && queued.event.ty == death
                    && queued.event.creator == Some(reaction.attacker))
            });
        }
    }

    /// Handles an authored hit while leaving the animation command alive to its end.
    pub(crate) fn death_blow_impact(&mut self, id: ObjectId, due: u64) -> bool {
        let Some(blow) = self
            .world
            .get_mut(id)
            .and_then(|o| o.combat.death_blow.as_mut())
        else {
            return false;
        };
        let Some(impact) = blow
            .impacts
            .get(blow.next_impact)
            .filter(|impact| impact.at_ms <= due)
        else {
            return false;
        };
        let (victim, hand) = (blow.victim, impact.hand);
        blow.next_impact += 1;
        let next = blow
            .impacts
            .get(blow.next_impact)
            .map_or(blow.ends_ms, |impact| impact.at_ms);
        if let Some(command) = self.world.get_mut(id).and_then(|o| o.commands.first_mut()) {
            command.due = Some(next.max(due));
        }
        self.attack_impact_slots(id, victim, &[hand]);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::combat::AttackResult;
    use crate::trace::TraceEntry;
    use std::path::PathBuf;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    use thedas_formats::gda::Cell;
    use thedas_formats::gff4::{
        FieldDef, FourCC, Gff4, List, StructData, StructId, StructTemplate, Value, Version,
    };
    use thedas_script::EngineTable;
    use thedas_vfs::Vfs;

    struct Fixture {
        sim: Sim,
        attacker: ObjectId,
        victim: ObjectId,
        directory: PathBuf,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.directory);
        }
    }

    fn document(kind: &[u8; 4], objects: Vec<Vec<(u32, Value)>>) -> Vec<u8> {
        let mut doc = Gff4 {
            version: Version::V40,
            platform: FourCC::new(b"PC  "),
            file_type: FourCC::new(kind),
            file_version: FourCC::new(b"V0.1"),
            templates: Vec::new(),
            structs: Vec::new(),
            root: StructId(0),
            shared_strings: None,
        };
        for fields in objects {
            let template = doc.templates.len() as u32;
            doc.templates.push(StructTemplate {
                label: FourCC::new(b"test"),
                size: fields.len() as u32 * 4,
                fields: fields
                    .iter()
                    .enumerate()
                    .map(|(index, (label, value))| {
                        let (type_code, flags) = match value {
                            Value::F32(_) => (8, 0),
                            Value::I32(_) => (5, 0),
                            Value::String(_) => (14, 0),
                            Value::List(_) => (1, 0xe000),
                            _ => unreachable!(),
                        };
                        FieldDef {
                            label: *label,
                            type_code,
                            flags,
                            offset: index as u32 * 4,
                        }
                    })
                    .collect(),
            });
            doc.structs.push(StructData {
                template,
                values: fields.into_iter().map(|(_, value)| value).collect(),
            });
        }
        doc.write().unwrap()
    }

    fn fixture(hits: bool) -> Fixture {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let directory = std::env::temp_dir().join(format!(
            "thedas-deathblow-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let string = |s: &str| Value::String(Some(s.into()));
        let list = |ids: &[usize]| {
            Value::List(Some(List::Struct(
                ids.iter().map(|&id| Some(StructId(id))).collect(),
            )))
        };
        let tree = document(
            b"ANBT",
            vec![
                vec![(4021, list(&[1, 2]))],
                vec![
                    (4024, string("blow")),
                    (4025, string("blow.ani")),
                    (4026, Value::F32(2.0)),
                ],
                vec![
                    (4024, string("reaction")),
                    (4025, string("reaction.ani")),
                    (4026, Value::F32(1.0)),
                ],
            ],
        );
        std::fs::write(directory.join("test.anb"), tree).unwrap();
        for (name, duration) in [("blow", 2.0), ("reaction", 3.0)] {
            std::fs::write(
                directory.join(format!("{name}.gad")),
                document(
                    b"GAD ",
                    vec![vec![(4009, Value::F32(duration)), (4010, Value::F32(1.0))]],
                ),
            )
            .unwrap();
        }
        let events = if hits { list(&[1, 2, 3]) } else { list(&[1]) };
        let mut event_objects = vec![
            vec![(4020, events)],
            vec![
                (4016, Value::F32(0.1)),
                (4017, Value::I32(19)),
                (4019, string("")),
            ],
        ];
        if hits {
            event_objects.extend([
                vec![
                    (4016, Value::F32(0.5)),
                    (4017, Value::I32(1)),
                    (4019, string("")),
                ],
                vec![
                    (4016, Value::F32(1.0)),
                    (4017, Value::I32(1)),
                    (4019, string("|Left")),
                ],
            ]);
        }
        std::fs::write(directory.join("blow.evt"), document(b"EVT ", event_objects)).unwrap();
        let mut vfs = Vfs::new();
        vfs.push_directory(&directory, false, "synthetic deathblow")
            .unwrap();
        let table = EngineTable::parse(b"int PROPERTY_DEPLETABLE_HEALTH = 7;\nint EVENT_TYPE_DEATH = 4;\nint EVENT_TYPE_ATTACK_IMPACT = 29;\n").unwrap();
        let mut sim = Sim::new(Arc::new(vfs), Arc::new(table), 7);
        sim.world.trace.keep = true;
        let tables = sim.world.tables();
        tables.insert(
            "#2",
            &["ID", "#de80b418"],
            vec![(1, vec![Cell::Int(1), Cell::String("test.anb".into())])],
        );
        tables.insert(
            "#0",
            &["ID", "#7a398af9"],
            vec![
                (300, vec![Cell::Int(300), Cell::String("blow".into())]),
                (306, vec![Cell::Int(306), Cell::String("reaction".into())]),
            ],
        );
        tables.insert(
            "#102",
            &["ID", "#f95409f5", "#2855abec"],
            vec![(1, vec![Cell::Int(1), Cell::Int(300), Cell::Int(306)])],
        );
        let area = sim.world.spawn(ObjectKind::Area, "a");
        let attacker = sim.world.spawn(ObjectKind::Creature, "attacker");
        let victim = sim.world.spawn(ObjectKind::Creature, "victim");
        for (id, x) in [(attacker, 0.0), (victim, 1.0)] {
            let o = sim.world.get_mut(id).unwrap();
            o.appearance = 1;
            o.area = Some(area);
            o.position = [x, 0.0, 0.0];
            o.properties.insert(7, (100.0, 100.0));
        }
        sim.world.get_mut(victim).unwrap().script = Some("death_probe".into());
        let command: Command = serde_json::from_value(serde_json::json!({
            "ty":38,"name":"CommandDeathBlow","target":victim,"location":null,
            "seconds":0.0,"ints":[1],"strings":[]
        }))
        .unwrap();
        let o = sim.world.get_mut(attacker).unwrap();
        o.combat.attack = Some(AttackResult {
            results: [4, 5],
            effects: [None, None],
        });
        o.commands.push(command);
        Fixture {
            sim,
            attacker,
            victim,
            directory,
        }
    }

    fn impacts(sim: &Sim) -> Vec<(u64, i32)> {
        sim.world
            .trace
            .entries
            .iter()
            .filter_map(|entry| match entry {
                TraceEntry::Attack { t, result, .. } => Some((*t, *result)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn reaction_and_health_start_together_death_event_waits_for_reaction_end() {
        let mut f = fixture(true);
        f.sim.tick(0);
        let victim = f.sim.world.get(f.victim).unwrap();
        let reaction = victim.combat.death_blow_reaction.as_ref().unwrap();
        assert_eq!(
            (reaction.started_ms, reaction.ends_ms, reaction.animation),
            (0, 3000, 306)
        );
        assert!(victim.dead);
        assert_eq!(victim.properties[&7].1, 0.0);
        let attacker = f.sim.world.get(f.attacker).unwrap();
        let separation = f.sim.capsule_radius(f.attacker) + f.sim.capsule_radius(f.victim) + 1.2;
        assert!((victim.position[0] - attacker.position[0] - separation).abs() < 1e-5);
        assert_eq!(attacker.facing, crate::world::facing_toward(1.0, 0.0));
        assert_eq!(victim.facing, crate::world::facing_toward(-1.0, 0.0));
        assert!(impacts(&f.sim).is_empty());
        // A later stored death effect cannot send a second, earlier notification.
        f.sim.apply_effect(
            f.victim,
            crate::world::Effect {
                ty: crate::combat::EFFECT_TYPE_DEATH,
                creator: Some(f.attacker),
                ..Default::default()
            },
        );
        let death_count = |sim: &Sim| {
            sim.world
                .trace
                .entries
                .iter()
                .filter(|entry| matches!(entry, TraceEntry::EventDelivered { ty: 4, .. }))
                .count()
        };
        f.sim.tick(2999);
        assert_eq!(death_count(&f.sim), 0);
        f.sim.tick(1);
        assert_eq!(death_count(&f.sim), 1);
        f.sim.tick(3000);
        assert_eq!(death_count(&f.sim), 1);
    }

    #[test]
    fn stored_hands_land_only_at_their_hit_markers_and_command_keeps_its_tail() {
        let mut f = fixture(true);
        f.sim.tick(249);
        assert!(impacts(&f.sim).is_empty());
        f.sim.tick(1);
        assert_eq!(impacts(&f.sim), [(250, 4)]);
        assert_eq!(
            f.sim
                .world
                .get(f.attacker)
                .unwrap()
                .combat
                .attack
                .as_ref()
                .unwrap()
                .results,
            [4, 5]
        );
        f.sim.tick(250);
        assert_eq!(impacts(&f.sim), [(250, 4), (500, 5)]);
        assert!(!f.sim.world.get(f.attacker).unwrap().commands.is_empty());
        f.sim.tick(500);
        assert!(f.sim.world.get(f.attacker).unwrap().commands.is_empty());
        assert_eq!(impacts(&f.sim).len(), 2);
    }

    #[test]
    fn visual_contacts_in_a_paired_clip_do_not_invent_attack_impacts() {
        let mut f = fixture(false);
        f.sim.tick(1000);
        assert!(impacts(&f.sim).is_empty());
        assert!(f.sim.world.get(f.victim).unwrap().dead);
        assert!(f.sim.world.get(f.attacker).unwrap().combat.attack.is_none());
    }

    #[test]
    fn selected_large_pair_keeps_its_unported_table_placement() {
        for requested_row in [0, 19] {
            let mut f = fixture(false);
            f.sim.world.tables().insert(
                "#102",
                &["ID", "#f95409f5", "#2855abec"],
                vec![(19, vec![Cell::Int(19), Cell::Int(300), Cell::Int(306)])],
            );
            f.sim.world.get_mut(f.attacker).unwrap().commands[0].ints = vec![requested_row];
            f.sim.tick(0);
            assert_eq!(f.sim.world.get(f.attacker).unwrap().position, [0.0; 3]);
            assert!(f.sim.world.get(f.victim).unwrap().dead);
        }
    }

    #[test]
    fn saved_pair_resumes_without_restarting_or_repeating_hits() {
        let mut f = fixture(true);
        f.sim.tick(300);
        let saved = f.sim.save_json().unwrap();
        let mut restored = fixture(true);
        restored.sim.load_json(&saved).unwrap();
        f.sim.tick(700);
        restored.sim.tick(700);
        assert_eq!(
            f.sim.save_json().unwrap(),
            restored.sim.save_json().unwrap()
        );
        assert_eq!(impacts(&restored.sim), [(500, 5)]);
    }

    #[test]
    fn resurrection_cancels_the_pending_death_notification_and_reaction() {
        let mut f = fixture(true);
        f.sim.tick(0);
        f.sim.apply_effect(
            f.victim,
            crate::world::Effect {
                ty: crate::combat::EFFECT_TYPE_RESURRECTION,
                ..Default::default()
            },
        );
        assert!(!f.sim.world.get(f.victim).unwrap().dead);
        f.sim.tick(3000);
        assert!(
            f.sim
                .world
                .get(f.victim)
                .unwrap()
                .combat
                .death_blow_reaction
                .is_none()
        );
        assert!(
            !f.sim
                .world
                .trace
                .entries
                .iter()
                .any(|entry| matches!(entry, TraceEntry::EventDelivered { ty: 4, .. }))
        );
    }
    #[test]
    fn old_active_commands_restart_preparation_and_new_defaults_select_once() {
        let mut f = fixture(true);
        let command = f
            .sim
            .world
            .get_mut(f.attacker)
            .unwrap()
            .commands
            .first_mut()
            .unwrap();
        command.ints = vec![0];
        command.due = Some(500);
        let mut saved = f.sim.save_game();
        saved.revision = 10;
        f.sim.load_game(saved).unwrap();
        assert!(
            f.sim.world.get(f.attacker).unwrap().commands[0]
                .due
                .is_none()
        );
        f.sim.tick(0);
        assert_eq!(
            f.sim
                .world
                .get(f.attacker)
                .unwrap()
                .combat
                .death_blow
                .as_ref()
                .unwrap()
                .animation,
            300
        );
        let started = f
            .sim
            .world
            .get(f.victim)
            .unwrap()
            .combat
            .death_blow_reaction
            .as_ref()
            .unwrap()
            .started_ms;
        f.sim.tick(100);
        assert_eq!(
            f.sim
                .world
                .get(f.victim)
                .unwrap()
                .combat
                .death_blow_reaction
                .as_ref()
                .unwrap()
                .started_ms,
            started
        );
    }

    #[test]
    fn unavailable_rows_and_da2_retain_the_stored_result_fallback() {
        for da2 in [false, true] {
            let mut f = fixture(true);
            if da2 {
                f.sim.game = thedas_vfs::Game::DragonAge2;
            } else {
                f.sim.world.get_mut(f.attacker).unwrap().commands[0].ints = vec![9999];
            }
            f.sim.tick(499);
            assert!(!f.sim.world.get(f.victim).unwrap().dead);
            assert!(
                f.sim
                    .world
                    .get(f.victim)
                    .unwrap()
                    .combat
                    .death_blow_reaction
                    .is_none()
            );
            f.sim.tick(1);
            assert_eq!(impacts(&f.sim), [(500, 4), (500, 5)]);
        }
    }
    #[test]
    fn each_authored_hit_uses_its_stored_hand_until_the_command_ends() {
        let mut f = fixture(true);
        f.sim.tick(0);
        f.sim
            .world
            .get_mut(f.attacker)
            .unwrap()
            .combat
            .death_blow
            .as_mut()
            .unwrap()
            .impacts
            .insert(
                1,
                DeathBlowImpact {
                    at_ms: 375,
                    hand: 0,
                },
            );
        f.sim.tick(1000);
        assert_eq!(impacts(&f.sim), [(250, 4), (375, 4), (500, 5)]);
        assert!(f.sim.world.get(f.attacker).unwrap().combat.attack.is_none());
    }
}
