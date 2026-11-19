//! Origins paired death animation preparation and its separate script-death deadline.
//! Static trace and remaining selection/placement approximations are in
//! `docs/notes/deathblow-executor.md`.

use serde::{Deserialize, Serialize};
use thedas_formats::anim::{AnimInfo, AnimTree, parse_events_with_text};
use thedas_script::vm::ObjectId;

use crate::Sim;
use crate::nav::{MAX_STEP, height, walkable};
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
    /// Present only when the pair's own placement entered a nonwalkable cell.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placement_repair: Option<PairPlacement>,
}

/// The stationary in-flight placement recorded for an Origins pair's finish.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PairPlacement {
    area: ObjectId,
    before: [f32; 3],
    placed: [f32; 3],
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
        let mut placement_repair = None;
        if !(19..=23).contains(&row) {
            let from = self.world.get(id)?.position;
            let target = self.world.get(victim)?.position;
            let (dx, dy) = (target[0] - from[0], target[1] - from[1]);
            let length = (dx * dx + dy * dy).sqrt();
            if length > 1e-6 {
                let distance = self.capsule_radius(id) + self.capsule_radius(victim) + 1.2;
                let placed = [
                    target[0] - dx / length * distance,
                    target[1] - dy / length * distance,
                    from[2],
                ];
                if let Some(area) = self.world.get(id)?.area
                    && let Some(g) = self.nav_grid(area)
                    && let Some(cell) = g.cell(placed)
                    && g.cell(from).is_some_and(|before| before != cell)
                    && !walkable(&g, cell)
                {
                    placement_repair = Some(PairPlacement {
                        area,
                        before: from,
                        placed,
                    });
                }
                self.move_object(id, placed, None);
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
            placement_repair,
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

    /// Origins type 38 attempts safe placement at public completion (0x747447 calls
    /// 0x73e370 with a 5 m radius; a changed result is written by 0x695540).
    /// Provisional: only repair the pair's own baked/patched wall placement, prefer the
    /// saved pre-pair point within the normal 1.5-cell start repair bound, and use Sim's
    /// floor, body and obstacle grid in place of the native scene queries.
    pub(crate) fn finish_death_blow_placement(&mut self, id: ObjectId) {
        let Some(p) = self.world.get(id).and_then(|o| {
            let p = o.combat.death_blow.as_ref()?.placement_repair?;
            (o.active
                && !o.dead
                && o.parked.is_none()
                && o.area == Some(p.area)
                && o.position == p.placed)
                .then_some(p)
        }) else {
            return;
        };
        let Some(g) = self.nav_grid(p.area) else {
            return;
        };
        let Some(own) = g.cell(p.placed) else { return };
        if walkable(&g, own) || !(g.cell_size > 0.0 && g.cell_size.is_finite()) {
            return;
        }
        let scenery = self.movement_cells(id, &g);
        let doors = self.closed_doors(p.area);
        let leaves = self.door_sight(p.area);
        let free = |c: (u32, u32)| {
            let index = (c.1 * g.columns + c.0) as usize;
            walkable(&g, c)
                && !scenery.contains(&index)
                && !doors.iter().any(|d| d.1.contains(&index))
        };
        let feet = p.placed;
        let radius = self.capsule_radius(id);
        let candidate = |q: [f32; 3], bound: f32| {
            if !q.iter().all(|x| x.is_finite()) {
                return None;
            }
            let (dx, dy) = (q[0] - feet[0], q[1] - feet[1]);
            let distance = dx.hypot(dy);
            if distance > bound {
                return None;
            }
            let cell = g.cell(q)?;
            let z = height(&g, cell);
            if !free(cell) || (z - q[2]).abs() > MAX_STEP || (z - feet[2]).abs() > MAX_STEP {
                return None;
            }
            let q = [q[0], q[1], z];
            if self.body_in_the_way(p.area, q, radius, Some(id)) {
                return None;
            }
            // Permit leaving only the invalid cell the pair put us into. Its
            // stored height is not a floor; use the retained feet until we exit.
            let mut prev = own;
            let mut floor = feet[2];
            let n = (distance / (g.cell_size * 0.5)).ceil().max(1.0) as u32;
            for k in 1..=n {
                let t = k as f32 / n as f32;
                let c = g.cell([feet[0] + t * dx, feet[1] + t * dy, z])?;
                if c == prev {
                    continue;
                }
                let level = |c| free(c) && (height(&g, c) - feet[2]).abs() <= MAX_STEP;
                if !level(c)
                    || (height(&g, c) - floor).abs() > MAX_STEP
                    || (c.0 != prev.0
                        && c.1 != prev.1
                        && !(level((c.0, prev.1)) && level((prev.0, c.1))))
                {
                    return None;
                }
                prev = c;
                floor = height(&g, c);
            }
            // Grid doorway masks can miss a straddling leaf. Do not snap through one.
            if leaves.solid.iter().any(|leaf| {
                leaf.blocked([feet[0], feet[1], feet[2] + 1.0], [q[0], q[1], q[2] + 1.0])
            }) {
                return None;
            }
            Some(q)
        };
        const REACH: f32 = 5.0;
        let repaired = candidate(p.before, (g.cell_size * 1.5).min(REACH)).or_else(|| {
            // As in 0x73e370, advance in half-cell square rings, bottom/top then
            // left/right. The extra ring-count cap bounds work on very fine grids.
            let step = g.cell_size * 0.5;
            let rings = ((REACH / step) as i32).min(40);
            for r in 1..=rings {
                for (xs, ys, swap) in [(-r..=r, [-r, r], false), (1 - r..=r - 1, [-r, r], true)] {
                    for i in xs {
                        for j in ys {
                            let (x, y) = if swap { (j, i) } else { (i, j) };
                            if let Some(q) = candidate(
                                [
                                    feet[0] + x as f32 * step,
                                    feet[1] + y as f32 * step,
                                    feet[2],
                                ],
                                REACH,
                            ) {
                                return Some(q);
                            }
                        }
                    }
                }
            }
            None
        });
        if let Some(q) = repaired {
            self.move_object(id, q, None);
        }
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

    /// Provisional unload policy: complete paired deaths while their area and its
    /// scripts are still available. Native in-flight death-task unload is untraced.
    pub(crate) fn finish_area_death_blow_reactions(&mut self, area: ObjectId) {
        if self.game != thedas_vfs::Game::Origins {
            return;
        }
        let death = crate::sim::constant(&self.table, "EVENT_TYPE_DEATH");
        let pending = |q: &crate::world::QueuedEvent| {
            self.world.get(q.target).is_some_and(|o| {
                o.area == Some(area)
                    && o.dead
                    && o.combat
                        .death_blow_reaction
                        .as_ref()
                        .is_some_and(|r| q.event.ty == death && q.event.creator == Some(r.attacker))
            })
        };
        // Preserve queue order, and remove notifications before invoking scripts:
        // a reentrant departure must not deliver the same death twice.
        let keys: Vec<_> = self
            .world
            .queue
            .iter()
            .filter(|(_, q)| pending(q))
            .map(|(key, _)| *key)
            .collect();
        let held: Vec<_> = self
            .world
            .held
            .iter()
            .enumerate()
            .filter(|(_, q)| pending(q))
            .map(|(index, _)| index)
            .collect();
        let mut events: Vec<_> = keys
            .into_iter()
            .filter_map(|key| self.world.queue.remove(&key))
            .collect();
        let mut held_events: Vec<_> = held
            .into_iter()
            .rev()
            .map(|index| self.world.held.remove(index))
            .collect();
        held_events.reverse();
        events.extend(held_events);
        if events.is_empty() {
            return;
        }
        let now = self.world.time_ms;
        let previous_sequence = self.world.queue.keys().map(|&(_, seq)| seq).max();
        for q in events {
            // Complete the reaction too, retaining its final pose for presentation.
            if let Some(r) = self
                .world
                .get_mut(q.target)
                .and_then(|o| o.combat.death_blow_reaction.as_mut())
            {
                r.ends_ms = r.ends_ms.min(now);
            }
            self.deliver(q.target, q.event, q.script);
        }
        // DEATH handlers can signal their area asynchronously (the Tower ogre
        // does). Finish that immediate notification chain before parking, without
        // advancing time or draining events that preceded these completions.
        while let Some(key) = self
            .world
            .queue
            .keys()
            .find(|&&(at, seq)| {
                at <= now && previous_sequence.is_none_or(|previous| seq > previous)
            })
            .copied()
        {
            let q = self
                .world
                .queue
                .remove(&key)
                .expect("selected queued event");
            if self
                .world
                .get(q.target)
                .is_some_and(|o| o.parked.is_some() && o.id != area && o.area != Some(area))
            {
                self.world.held.push(q);
                continue;
            }
            self.deliver(q.target, q.event, q.script);
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
        let table = EngineTable::parse(b"#define ENGINE_STRUCTURE_0 event\nevent GetCurrentEvent() = 0;\nvoid SignalEvent(object target, event e, int immediate) = 1;\nint PROPERTY_DEPLETABLE_HEALTH = 7;\nint EVENT_TYPE_DEATH = 4;\nint EVENT_TYPE_ATTACK_IMPACT = 29;\n").unwrap();
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

    fn placement_fixture() -> (Fixture, [f32; 3], thedas_formats::level::NavGrid) {
        let mut f = fixture(false);
        let before = [2.75, 2.75, -1.0];
        let separation = f.sim.capsule_radius(f.attacker) + f.sim.capsule_radius(f.victim) + 1.2;
        let a = f.sim.world.get_mut(f.attacker).unwrap();
        a.position = before;
        let area = a.area.unwrap();
        f.sim.world.get_mut(f.victim).unwrap().position =
            [before[0] + separation - 0.7, before[1], before[2]];
        f.sim.area_layouts.insert(area, "pair_grid".into());
        let mut g = thedas_formats::level::NavGrid {
            columns: 12,
            rows: 12,
            cell_size: 0.5,
            clearance: 2.0,
            base: [0.0; 3],
            normal: [0.0, 0.0, 1.0],
            data: vec![1; 144],
            heights: vec![-16.0; 144],
            surfaces: Vec::new(),
        };
        g.data[5 * 12 + 4] = 9;
        g.heights[5 * 12 + 4] = 32.0; // The wall's sample is not the mover's floor.
        (f, before, g)
    }

    #[test]
    fn pair_wall_repairs_to_pre_pair_feet_at_finish_after_resume() {
        let (mut f, before, g) = placement_fixture();
        f.sim
            .nav_grids
            .insert("pair_grid".into(), Some(Arc::new(g)));
        f.sim.tick(0);
        let a = f.sim.world.get(f.attacker).unwrap();
        let placed = a.position;
        let facing = a.facing;
        let pair = a.combat.death_blow.as_ref().unwrap();
        assert_eq!(pair.placement_repair.unwrap().before, before);
        let saved: crate::save::SaveGame =
            serde_json::from_slice(&f.sim.save_json().unwrap()).unwrap();
        let ends = pair.ends_ms;
        // Load normally, then remount the synthetic grid (no installed layout).
        let grid = f.sim.nav_grids["pair_grid"].clone();
        let area = a.area.unwrap();
        f.sim.load_game(saved).unwrap();
        f.sim.area_layouts.insert(area, "pair_grid".into());
        f.sim.nav_grids.insert("pair_grid".into(), grid);
        f.sim.tick(ends - 1);
        assert_eq!(f.sim.world.get(f.attacker).unwrap().position, placed);
        f.sim.tick(1);
        let a = f.sim.world.get(f.attacker).unwrap();
        assert_eq!((a.position, a.facing), (before, facing));
        assert!(a.combat.death_blow.is_none());
        assert!(f.sim.world.get(f.victim).unwrap().dead);
        f.sim.order_move(f.attacker, [3.75, 2.75, -1.0]);
        f.sim.tick(250);
        assert!(f.sim.world.get(f.attacker).unwrap().position[0] > before[0]);
    }

    #[test]
    fn pair_wall_rejects_unsafe_original_point_before_trying_a_local_alternate() {
        let (mut f, before, g) = placement_fixture();
        f.sim
            .nav_grids
            .insert("pair_grid".into(), Some(Arc::new(g)));
        f.sim.tick(0);
        let a = f.sim.world.get(f.attacker).unwrap();
        let placed = a.position;
        let area = a.area;
        let ends = a.combat.death_blow.as_ref().unwrap().ends_ms;
        let other = f.sim.world.spawn(ObjectKind::Creature, "other");
        let o = f.sim.world.get_mut(other).unwrap();
        o.area = area;
        o.position = before;
        f.sim.tick(ends);
        let q = f.sim.world.get(f.attacker).unwrap().position;
        assert_ne!(q, before);
        assert_ne!(q, placed);
        assert_eq!(q[2], before[2]);
        assert!(crate::engine::dist(q, placed) < 1.0);
    }

    #[test]
    fn pair_wall_keeps_placement_when_the_only_exit_is_on_another_floor() {
        let (mut f, _, mut g) = placement_fixture();
        for (i, v) in g.data.iter_mut().enumerate() {
            if i != 5 * 12 + 5 {
                *v = 9;
            }
        }
        g.heights[5 * 12 + 5] = 16.0;
        f.sim
            .nav_grids
            .insert("pair_grid".into(), Some(Arc::new(g)));
        f.sim.tick(0);
        let a = f.sim.world.get(f.attacker).unwrap();
        let placed = a.position;
        let ends = a.combat.death_blow.as_ref().unwrap().ends_ms;
        f.sim.tick(ends);
        assert_eq!(f.sim.world.get(f.attacker).unwrap().position, placed);
    }

    #[test]
    fn pair_wall_does_not_cross_a_newly_blocked_exit_to_a_free_island() {
        let (mut f, _, mut g) = placement_fixture();
        let exit = 5 * 12 + 5;
        let island = 5 * 12 + 7;
        for (i, v) in g.data.iter_mut().enumerate() {
            if i != exit && i != island {
                *v = 9;
            }
        }
        f.sim
            .nav_grids
            .insert("pair_grid".into(), Some(Arc::new(g)));
        f.sim.tick(0);
        let a = f.sim.world.get(f.attacker).unwrap();
        let placed = a.position;
        let ends = a.combat.death_blow.as_ref().unwrap().ends_ms;
        let area = a.area.unwrap();
        f.sim
            .world
            .nav_patches
            .entry(area)
            .or_default()
            .insert(exit as u32, 8);
        f.sim.tick(ends);
        assert_eq!(f.sim.world.get(f.attacker).unwrap().position, placed);
    }

    #[test]
    fn pair_wall_never_rewrites_an_externally_displaced_attacker() {
        let (mut f, _, g) = placement_fixture();
        f.sim
            .nav_grids
            .insert("pair_grid".into(), Some(Arc::new(g)));
        f.sim.tick(0);
        let a = f.sim.world.get_mut(f.attacker).unwrap();
        let placed = a.position;
        let ends = a.combat.death_blow.as_ref().unwrap().ends_ms;
        let external = [placed[0] + 0.05, placed[1], placed[2]];
        a.position = external;
        f.sim.tick(ends);
        assert_eq!(f.sim.world.get(f.attacker).unwrap().position, external);
    }

    #[test]
    fn a_legal_pair_placement_keeps_its_position_and_needs_no_repair_record() {
        let (mut f, _, mut g) = placement_fixture();
        g.data[5 * 12 + 4] = 1;
        g.heights[5 * 12 + 4] = -16.0;
        f.sim
            .nav_grids
            .insert("pair_grid".into(), Some(Arc::new(g)));
        f.sim.tick(0);
        let a = f.sim.world.get(f.attacker).unwrap();
        let placed = a.position;
        let pair = a.combat.death_blow.as_ref().unwrap();
        assert!(pair.placement_repair.is_none());
        f.sim.tick(pair.ends_ms);
        assert_eq!(f.sim.world.get(f.attacker).unwrap().position, placed);
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

    #[test]
    fn departure_completes_paired_death_before_exit_and_never_repeats_it() {
        let mut f = fixture(false);
        let old = f.sim.world.get(f.victim).unwrap().area.unwrap();
        f.sim.world.area = Some(old);
        f.sim.world.hero = Some(f.attacker);
        f.sim.world.get_mut(old).unwrap().template = "a".into();
        f.sim.world.get_mut(old).unwrap().script = Some("area_probe".into());
        let next = f.sim.world.spawn(ObjectKind::Area, "next");
        f.sim.world.get_mut(next).unwrap().template = "next".into();
        f.sim.tick(100);
        f.sim.world.trace.entries.clear();
        // Ordinary delayed events, even DEATH, must retain their own deadlines.
        let ordinary = f.sim.world.spawn(ObjectKind::Creature, "ordinary");
        f.sim.world.get_mut(ordinary).unwrap().area = Some(old);
        f.sim.kill_with_event_delay(ordinary, f.attacker, 3000);
        let unrelated = f.sim.new_event("ATTACK_IMPACT", Some(f.attacker));
        f.sim.world.queue_event(3000, f.victim, unrelated, None);
        f.sim.area_transition("next", "");
        assert_eq!(f.sim.world.area, Some(next));
        let victim = f.sim.world.get(f.victim).unwrap();
        assert!(victim.parked.is_some());
        assert_eq!(
            victim.combat.death_blow_reaction.as_ref().unwrap().ends_ms,
            100
        );
        let events: Vec<_> = f
            .sim
            .world
            .trace
            .entries
            .iter()
            .filter_map(|entry| match entry {
                TraceEntry::EventDelivered { t, target, ty, .. } => Some((*t, *target, *ty)),
                _ => None,
            })
            .collect();
        assert_eq!(events, [(100, f.victim, 4), (100, old, 0)]);
        assert!(
            f.sim
                .world
                .queue
                .values()
                .any(|q| q.target == ordinary && q.event.ty == 4)
        );
        assert!(
            f.sim
                .world
                .queue
                .values()
                .any(|q| q.target == f.victim && q.event.ty == 29)
        );
        f.sim.tick(4000);
        f.sim.area_transition("a", "");
        f.sim.tick(0);
        assert_eq!(f.sim.world.trace.entries.iter().filter(|e| {
            matches!(e, TraceEntry::EventDelivered { target, ty: 4, .. } if *target == f.victim)
        }).count(), 1);
    }

    #[test]
    fn direct_departure_flushes_saved_held_pair_but_spares_other_areas() {
        for held in [false, true] {
            let mut f = fixture(false);
            let old = f.sim.world.get(f.victim).unwrap().area.unwrap();
            f.sim.world.area = Some(old);
            let next = f.sim.world.spawn(ObjectKind::Area, "next");
            f.sim.world.get_mut(next).unwrap().template = "next".into();
            f.sim.tick(100);
            if held {
                let key = *f.sim.world.queue.keys().next().unwrap();
                let q = f.sim.world.queue.remove(&key).unwrap();
                assert_eq!(q.event.ty, 4);
                f.sim.world.held.push(q);
                f.sim.world.get_mut(f.victim).unwrap().parked = Some(true);
            }
            let saved = f.sim.save_json().unwrap();
            f.sim.load_json(&saved).unwrap();
            f.sim.finish_area_death_blow_reactions(next);
            assert!(!f.sim.world.queue.is_empty() || !f.sim.world.held.is_empty());
            f.sim.enter_area("next", None).unwrap();
            assert_eq!(
                f.sim
                    .world
                    .get(f.victim)
                    .unwrap()
                    .combat
                    .death_blow_reaction
                    .as_ref()
                    .unwrap()
                    .ends_ms,
                100
            );
            assert!(
                f.sim
                    .world
                    .queue
                    .values()
                    .all(|q| q.target != f.victim || q.event.ty != 4)
            );
            assert!(f.sim.world.held.is_empty());
            assert_eq!(f.sim.world.trace.entries.iter().filter(|e| {
                matches!(e, TraceEntry::EventDelivered { t: 100, target, ty: 4, .. } if *target == f.victim)
            }).count(), 1);
        }
    }

    #[test]
    fn departure_runs_immediate_death_completion_chain_but_not_existing_events() {
        use thedas_script::ncs::Ncs;

        let mut f = fixture(false);
        // Literal object 1 in NCS denotes OBJECT_INVALID, so use later ids for
        // the explicitly addressed notification receivers.
        let area = f.sim.world.spawn(ObjectKind::Area, "completion_area");
        for id in [f.attacker, f.victim] {
            f.sim.world.get_mut(id).unwrap().area = Some(area);
        }
        let terminal = f.sim.world.spawn(ObjectKind::Creature, "terminal");
        let o = f.sim.world.get_mut(terminal).unwrap();
        o.area = Some(area);
        o.script = Some("terminal_probe".into());
        f.sim.world.get_mut(area).unwrap().script = Some("area_probe".into());
        let forward = |target: ObjectId| {
            // SignalEvent(target, GetCurrentEvent(), FALSE).
            let mut body = vec![0x04, 0x03, 0, 0, 0, 0, 0x05, 0, 0, 0, 0, 0x04, 0x06];
            body.extend(target.to_be_bytes());
            body.extend([0x05, 0, 0, 1, 3, 0x20, 0]);
            let mut bytes = b"NCS V1.0B".to_vec();
            bytes.extend(((13 + body.len()) as u32).to_be_bytes());
            bytes.extend(body);
            Ncs::parse(&bytes).unwrap()
        };
        f.sim.add_script("death_probe", forward(area));
        f.sim.add_script("area_probe", forward(terminal));
        f.sim.tick(100);
        f.sim.world.trace.entries.clear();
        let unrelated = f.sim.new_event("ATTACK_IMPACT", Some(f.attacker));
        f.sim
            .world
            .queue_event(0, terminal, unrelated.clone(), None);
        f.sim.world.queue_event(3000, terminal, unrelated, None);
        f.sim.finish_area_death_blow_reactions(area);
        let delivered: Vec<_> = f
            .sim
            .world
            .trace
            .entries
            .iter()
            .filter_map(|e| match e {
                TraceEntry::EventDelivered { t, target, ty, .. } => Some((*t, *target, *ty)),
                _ => None,
            })
            .collect();
        assert_eq!(
            delivered,
            [(100, f.victim, 4), (100, area, 4), (100, terminal, 4)],
            "{:?}",
            f.sim.world.trace.entries
        );
        assert_eq!(f.sim.world.queue.len(), 2);
        assert!(f.sim.world.queue.values().all(|q| q.event.ty == 29));
    }
}
