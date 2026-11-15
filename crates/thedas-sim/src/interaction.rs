//! Origins ambient interactions (native 587, 0x65c110; executor 0x7a6ea0).
//!
//! A request changes the creature's desired interaction independently of its command.
//! An existing interaction keeps its target and role. The command progresses through
//! enter, pose and exit; a negative loop count holds the pose until another request.

use serde::{Deserialize, Serialize};
use thedas_formats::gda::Cell;
use thedas_script::vm::ObjectId;

use crate::Sim;
use crate::fmath::DetMath;
use crate::world::{Command, ObjectKind};

pub const COMMAND: &str = "CommandInteract";
pub const COMMAND_TYPE: i32 = 37;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CommandState {
    pub primary: bool,
    pub skip_reposition: bool,
    pub play_exit: bool,
    pub object_approach: bool,
    pub facing: Option<f32>,
    /// 0 admission, 1 approaching, 2 animation.
    pub stage: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    pub id: i32,
    pub pose: i32,
    pub loops: i32,
    pub play_exit: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Interaction {
    pub requested: Settings,
    pub playback: Option<Playback>,
}

/// Animation phase owned by the running interaction command, retained in saves.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Playback {
    pub applied: Settings,
    /// 0 enter, 1 pose, 2 exit.
    pub phase: u8,
    pub animation: i32,
    pub started: u64,
    pub looping: bool,
}

impl Sim {
    /// Store the request and return a command only when no interaction is active.
    /// The caller admits it through the ordinary Origins AddCommand path.
    pub(crate) fn request_interaction(
        &mut self,
        who: ObjectId,
        target: ObjectId,
        settings: Settings,
        primary: bool,
        skip_reposition: bool,
    ) -> Option<Command> {
        let now = self.world.time_ms;
        let o = self
            .world
            .get_mut(who)
            .filter(|o| o.kind == ObjectKind::Creature)?;
        let active = o
            .commands
            .first_mut()
            .filter(|c| c.ty == COMMAND_TYPE && c.due.is_some());
        if let Some(c) = active {
            let state = o.interaction.get_or_insert(Interaction {
                requested: settings,
                playback: None,
            });
            // 0x7a6fc6: id changes restart entry; pose changes replace a running pose.
            // Loop/exit changes are read when the current animation's deadline expires.
            if state.playback.as_ref().is_some_and(|p| {
                p.applied.id != settings.id
                    || (p.phase == 1 && p.applied.pose != settings.pose)
                    || (c.due == Some(u64::MAX) && state.requested != settings)
            }) {
                c.due = Some(now);
            }
            state.requested = settings;
            return None;
        }
        o.interaction = Some(Interaction {
            requested: settings,
            playback: None,
        });
        Some(Command {
            ty: COMMAND_TYPE,
            name: COMMAND.into(),
            target: Some(target),
            // Native command flags 2 and 1, respectively. These belong to the
            // command: changing a request does not retarget or reposition it anew.
            ints: vec![settings.id, settings.pose, settings.loops],
            interaction: Some(CommandState {
                primary,
                skip_reposition,
                play_exit: settings.play_exit,
                ..Default::default()
            }),
            ..Default::default()
        })
    }

    /// Start a phase or resume the saved deadline. Missing animation IDs fail like
    /// 0x7a70ca. An unavailable animation component has zero duration (0x694e50).
    pub(crate) fn interaction_deadline(&mut self, who: ObjectId) -> Option<u64> {
        let cmd = self.world.get(who)?.commands.first()?.clone();
        let metadata = cmd.interaction.as_ref()?;
        // 0x7a6f5c initializes a creature that has no earlier requested interaction
        // from the command payload. This also permits AddCommand on another actor.
        let initial = Interaction {
            requested: Settings {
                id: cmd.ints.first().copied().unwrap_or(0),
                pose: cmd.ints.get(1).copied().unwrap_or(1),
                loops: cmd.ints.get(2).copied().unwrap_or(0),
                play_exit: metadata.play_exit,
            },
            playback: None,
        };
        let mut state = self
            .world
            .get_mut(who)?
            .interaction
            .get_or_insert(initial)
            .clone();
        let target = cmd.target.and_then(|id| self.world.get(id)).map(|o| o.kind);
        let Some(target) = target else {
            self.fail_front_command(who, "COMMAND_FAILED_TARGET_DESTROYED");
            return None;
        };
        if let Some(due) = cmd.due {
            return Some(due);
        }
        let stage = metadata.stage;
        if stage == 0 {
            state.playback = None;
            self.world.get_mut(who)?.interaction.as_mut()?.playback = None;
        }
        if stage == 0 && !metadata.skip_reposition {
            return self.approach_interaction(who, &cmd, state.requested);
        }
        if stage == 1 {
            let facing = metadata.facing.or_else(|| {
                let target = self.world.get(cmd.target?)?;
                let actor = self.world.get(who)?;
                Some(crate::world::facing_toward(
                    target.position[0] - actor.position[0],
                    target.position[1] - actor.position[1],
                ))
            })?;
            self.world.get_mut(who)?.facing = facing;
        }
        let c = self.world.get_mut(who)?.commands.first_mut()?;
        c.interaction.as_mut()?.stage = 2;
        let table = if target == ObjectKind::Creature {
            124
        } else {
            123
        };
        let primary = target != ObjectKind::Creature || metadata.primary;
        let phase = state.playback.as_ref().map_or(0, |p| p.phase);
        let animation = self.interaction_animation(table, state.requested, primary, phase);
        if animation == 0 {
            self.fail_front_command(who, "COMMAND_FAILED_INVALID_DATA");
            return None;
        }
        let (length, looping) = self.interaction_animation_length(who, animation);
        let held = phase == 1 && state.requested.loops < 0;
        let count = if phase == 1 && looping {
            state.requested.loops.max(0) as u64
        } else {
            1
        };
        let ms = length.saturating_mul(count);
        let now = self.world.time_ms;
        let due = if held {
            u64::MAX
        } else {
            now.saturating_add(ms.max(1))
        };
        let o = self.world.get_mut(who)?;
        o.interaction.as_mut()?.playback = Some(Playback {
            applied: state.requested,
            phase,
            animation,
            started: now,
            looping,
        });
        let c = o.commands.first_mut()?;
        c.ints = vec![
            state.requested.id,
            state.requested.pose,
            state.requested.loops,
        ];
        c.due = Some(due);
        Some(due)
    }

    /// Called at a phase deadline. True leaves the interaction active; false lets
    /// the normal command-completion path send its success event.
    pub(crate) fn advance_interaction(&mut self, who: ObjectId) -> bool {
        let Some(o) = self.world.get_mut(who) else {
            return false;
        };
        let Some(state) = o.interaction.as_mut() else {
            return false;
        };
        let Some(p) = state.playback.as_mut() else {
            return false;
        };
        let requested = state.requested;
        let phase = if requested.id != p.applied.id {
            0
        } else if p.phase == 1 && requested.pose != p.applied.pose {
            1
        } else {
            if requested.loops < 0 && (p.phase == 1 || (p.phase == 0 && requested.pose <= 0)) {
                p.applied = requested;
                o.commands.first_mut().unwrap().due = Some(u64::MAX);
                return true;
            }
            match p.phase {
                0 if requested.loops != 0 && requested.pose > 0 => 1,
                0 | 1 if requested.play_exit => 2,
                _ => return false,
            }
        };
        p.phase = phase;
        o.commands.first_mut().unwrap().due = None;
        true
    }

    /// The native factory (0x743690) puts a move and turn ahead of executor 53.
    fn approach_interaction(
        &mut self,
        who: ObjectId,
        cmd: &Command,
        settings: Settings,
    ) -> Option<u64> {
        let target = self.world.get(cmd.target?)?;
        let actor = self.world.get(who)?;
        let (from, area) = (actor.position, actor.area);
        if area.is_none() || area != target.area {
            self.fail_front_command(who, "COMMAND_FAILED_INVALID_PATH");
            return None;
        }
        let (to, facing, tolerance) = if target.kind == ObjectKind::Placeable
            && let Some((point, facing)) = self.interaction_point(target.id, settings.id)
        {
            (point, Some(facing), self.body_radius(who))
        } else {
            // 0x743872 supplies max range 0.1 and minimum 0 to the ordinary
            // move-to-object subcommand; capsule contact can end that walk.
            (target.position, None, 0.15)
        };
        let path = if crate::engine::dist(from, to) > tolerance {
            match self.route_to(who, to) {
                crate::nav::Routed::Path(p) => p,
                crate::nav::Routed::Straight => vec![to],
                crate::nav::Routed::InvalidPath => {
                    self.fail_front_command(who, "COMMAND_FAILED_INVALID_PATH");
                    return None;
                }
                crate::nav::Routed::Door(door) => {
                    self.fail_blocked(who, door);
                    return None;
                }
            }
        } else {
            Vec::new()
        };
        let now = self.world.time_ms;
        let ms = (crate::sim::path_length(from, &path) / self.move_speed(who) * 1000.0) as u64;
        let o = self.world.get_mut(who)?;
        let c = o.commands.first_mut()?;
        let metadata = c.interaction.as_mut()?;
        metadata.stage = 1;
        metadata.object_approach = facing.is_none();
        metadata.facing = facing;
        if path.is_empty() {
            return self.interaction_deadline(who);
        }
        let due = now.saturating_add(ms.max(1));
        c.approach = Some(to);
        c.from = Some((from, now));
        c.path = path;
        c.due = Some(due);
        Some(due)
    }

    pub(crate) fn interaction_object_arrived(&self, who: ObjectId) -> bool {
        let Some(actor) = self.world.get(who) else {
            return false;
        };
        let Some(cmd) = actor.commands.first() else {
            return false;
        };
        if !cmd
            .interaction
            .as_ref()
            .is_some_and(|m| m.stage == 1 && m.object_approach)
        {
            return false;
        }
        let Some(target) = cmd.target.and_then(|id| self.world.get(id)) else {
            return false;
        };
        let Some(goal) = cmd.approach else {
            return false;
        };
        let distance = |point: [f32; 3]| {
            ((actor.position[0] - point[0]).powi(2) + (actor.position[1] - point[1]).powi(2)).sqrt()
        };
        distance(goal) <= 0.15
            || (target.kind == ObjectKind::Creature
                && self.alive(target.id)
                && distance(target.position)
                    <= self.capsule_radius(who) + self.capsule_radius(target.id) + 0.05)
    }

    /// 0x5ca860: the placeable-relative use point and facing, with angles in degrees.
    fn interaction_point(&self, target: ObjectId, interaction: i32) -> Option<([f32; 3], f32)> {
        let target = self.world.get(target)?;
        let table = self.world.tables().get(123, "")?;
        let number = |hash| match table.cell_by_hash(hash, i64::from(interaction)) {
            Some(Cell::Float(v)) => *v as f32,
            Some(Cell::Int(v)) => *v as f32,
            _ => 0.0,
        };
        if number(0x414a_699c) <= 0.0 {
            return None;
        }
        let distance = number(0x9090_85da);
        let bearing = crate::world::heading_of(target.facing - number(0x85ff_d766)).to_radians();
        Some((
            [
                target.position[0] + bearing.dcos() * distance,
                target.position[1] + bearing.dsin() * distance,
                target.position[2],
            ],
            (target.facing + 180.0 - number(0x9232_1b14)).rem_euclid(360.0),
        ))
    }

    fn interaction_animation(
        &self,
        table: i32,
        settings: Settings,
        primary: bool,
        phase: u8,
    ) -> i32 {
        let tables = self.world.tables();
        let Some(table) = tables.get(table, "") else {
            return 0;
        };
        let row = i64::from(settings.id);
        let cell = match phase {
            0 => table.cell_by_hash(if primary { 0x9067_4659 } else { 0x57bc_1d1b }, row),
            1 => table.cell(
                &format!("pose{}{}", if primary { "" } else { "sync" }, settings.pose),
                row,
            ),
            _ => table.cell_by_hash(if primary { 0x1725_fa61 } else { 0xa717_b7e2 }, row),
        };
        match cell {
            Some(Cell::Int(id)) => *id as i32,
            _ => 0,
        }
    }

    fn interaction_animation_length(&mut self, who: ObjectId, animation: i32) -> (u64, bool) {
        let Some(o) = self.world.get(who) else {
            return (0, false);
        };
        let tables = self.world.tables();
        let Some(anims) = tables.get(0, "") else {
            return (0, false);
        };
        let Some(Cell::String(action)) = anims.cell_by_hash(0x7a39_8af9, i64::from(animation))
        else {
            return (0, false);
        };
        let looping = matches!(anims.cell_by_hash(0x12a0_2374, i64::from(animation)), Some(Cell::Int(flags)) if flags & 1 != 0);
        let Some(appearance) = tables.get(0, "apr_base") else {
            return (0, looping);
        };
        let mut trees = Vec::new();
        for col in [0x5009_76ec, 0xde80_b418] {
            let Some(Cell::String(tree)) = appearance.cell_by_hash(col, i64::from(o.appearance))
            else {
                continue;
            };
            trees.push(crate::engine::clean(tree).to_ascii_lowercase());
        }
        (
            self.ability_anim(&trees, crate::engine::clean(action))
                .map_or(0, |sample| sample.len_ms),
            looping,
        )
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::{engine, sim::ScriptHost};
    use thedas_script::{
        EngineTable,
        vm::{OBJECT_INVALID, Value},
    };
    use thedas_vfs::{Game, Vfs};

    fn sim() -> (Sim, ObjectId, ObjectId) {
        let ldf = b"#define ENGINE_STRUCTURE_0 command\n\
            void InteractWithObject(object o, object t, int i, int p = 1, int n = 0, int e = 1, int s = 0) = 587;\n\
            int AddCommand(object o, command c, int f = 0, int s = 0, int b = -1) = 534;\n\
            int EVENT_TYPE_COMMAND_COMPLETE = 1004;\n\
            int COMMAND_FAILED_INVALID_DATA = -3;\n\
            int COMMAND_FAILED_INVALID_PATH = -4;\n\
            int COMMAND_FAILED_TARGET_DESTROYED = -7;\n";
        let mut s = Sim::new(
            Arc::new(Vfs::new()),
            Arc::new(EngineTable::parse(ldf).unwrap()),
            1,
        );
        let a = s.world.spawn(ObjectKind::Creature, "a");
        let b = s.world.spawn(ObjectKind::Placeable, "b");
        for table in ["#123", "#124"] {
            s.world.tables().insert(
                table,
                &[
                    "#90674659",
                    "#57bc1d1b",
                    "pose1",
                    "posesync1",
                    "pose2",
                    "posesync2",
                    "#1725fa61",
                    "#a717b7e2",
                ],
                vec![
                    (
                        1,
                        vec![11, 12, 21, 22, 31, 32, 41, 42]
                            .into_iter()
                            .map(Cell::Int)
                            .collect(),
                    ),
                    (
                        2,
                        vec![51, 52, 61, 62, 71, 72, 81, 82]
                            .into_iter()
                            .map(Cell::Int)
                            .collect(),
                    ),
                ],
            );
        }
        (s, a, b)
    }

    fn request(
        s: &mut Sim,
        who: ObjectId,
        target: ObjectId,
        id: i32,
        pose: i32,
        loops: i32,
        exit: bool,
    ) {
        let f = s
            .table
            .function_by_name("InteractWithObject")
            .unwrap()
            .clone();
        let mut host = ScriptHost {
            sim: s,
            event: None,
            script: "test".into(),
        };
        assert!(
            engine::call(
                &mut host,
                who,
                &f,
                &[
                    Value::Object(who),
                    Value::Object(target),
                    Value::Int(id),
                    Value::Int(pose),
                    Value::Int(loops),
                    Value::Int(i32::from(exit)),
                    Value::Int(1),
                ]
            )
            .unwrap()
            .unwrap()
            .is_none()
        );
    }

    fn animation(s: &Sim, who: ObjectId) -> i32 {
        s.world
            .get(who)
            .unwrap()
            .interaction
            .as_ref()
            .unwrap()
            .playback
            .as_ref()
            .unwrap()
            .animation
    }

    #[test]
    fn finite_interaction_enters_poses_and_exits_before_completing() {
        let (mut s, a, b) = sim();
        request(&mut s, a, b, 1, 1, 2, true);
        s.tick(0);
        assert_eq!(animation(&s, a), 11);
        s.tick(1);
        assert_eq!(animation(&s, a), 21);
        s.tick(1);
        assert_eq!(animation(&s, a), 41);
        s.tick(1);
        assert!(s.world.get(a).unwrap().commands.is_empty());
        assert_eq!(
            s.world
                .get(a)
                .unwrap()
                .combat
                .previous_command
                .as_ref()
                .unwrap()
                .ty,
            37
        );
        assert!(s.world.get(b).unwrap().commands.is_empty());
    }

    #[test]
    fn zero_loops_skips_pose_and_exit_flag_controls_the_last_phase() {
        for exit in [false, true] {
            let (mut s, a, b) = sim();
            request(&mut s, a, b, 1, 1, 0, exit);
            s.tick(1);
            assert_eq!(s.world.get(a).unwrap().commands.is_empty(), !exit);
            if exit {
                assert_eq!(animation(&s, a), 41);
                s.tick(1);
            }
            assert!(s.world.get(a).unwrap().commands.is_empty());
        }
    }

    #[test]
    fn infinite_pose_updates_in_place_and_retains_the_original_target_and_flags() {
        let (mut s, a, b) = sim();
        request(&mut s, a, b, 1, 1, -1, true);
        s.tick(1);
        s.tick(50_000);
        assert_eq!(animation(&s, a), 21);
        let other = s.world.spawn(ObjectKind::Placeable, "other");
        request(&mut s, a, other, 1, 2, -1, true);
        s.tick(0);
        let c = &s.world.get(a).unwrap().commands[0];
        assert_eq!(c.target, Some(b));
        assert_eq!(c.ints, vec![1, 2, -1]);
        assert!(c.interaction.as_ref().unwrap().primary);
        assert!(c.interaction.as_ref().unwrap().skip_reposition);
        assert_eq!(animation(&s, a), 31);
        assert_eq!(s.world.get(a).unwrap().commands.len(), 1);
        request(&mut s, a, other, 2, 1, -1, true);
        s.tick(0);
        assert_eq!(animation(&s, a), 51);
        s.tick(1);
        assert_eq!(animation(&s, a), 61);
        request(&mut s, a, b, 2, 1, 0, true);
        s.tick(0);
        assert_eq!(animation(&s, a), 81);
        s.tick(1);
        assert!(s.world.get(a).unwrap().commands.is_empty());
    }

    #[test]
    fn creature_partner_gets_the_reciprocal_role_but_invalid_initiator_stops_both() {
        let (mut s, a, b) = sim();
        s.world.get_mut(b).unwrap().kind = ObjectKind::Creature;
        request(&mut s, OBJECT_INVALID, b, 1, 1, -1, true);
        assert!(s.world.get(b).unwrap().commands.is_empty());
        request(&mut s, a, b, 1, 1, -1, true);
        assert_eq!(s.world.get(b).unwrap().commands[0].target, Some(a));
        let partner = &s.world.get(b).unwrap().commands[0];
        assert_eq!(partner.ints, vec![1, 1, -1]);
        assert!(!partner.interaction.as_ref().unwrap().primary);
        assert!(partner.interaction.as_ref().unwrap().skip_reposition);
        s.tick(1);
        assert_eq!(animation(&s, a), 21);
        assert_eq!(animation(&s, b), 22);
    }

    #[test]
    fn invalid_animation_and_destroyed_targets_fail_with_native_results() {
        for (target, interaction, result) in [(false, 99, -3), (true, 1, -7)] {
            let (mut s, a, b) = sim();
            request(&mut s, a, b, interaction, 1, 0, true);
            if target {
                s.world.objects.remove(&b);
            }
            s.tick(0);
            assert!(s.world.get(a).unwrap().commands.is_empty());
            assert!(
                s.world
                    .queue
                    .values()
                    .any(|q| q.target == a && q.event.ints == vec![37, result])
            );
        }
    }

    #[test]
    fn interaction_state_survives_json_and_da2_remains_unanswered() {
        let (mut s, a, b) = sim();
        request(&mut s, a, b, 1, 2, -1, true);
        s.tick(1);
        let world = serde_json::to_vec(&s.world).unwrap();
        let mut restored = sim().0;
        restored.world = serde_json::from_slice(&world).unwrap();
        restored.world.tables = s.world.tables();
        s.tick(600);
        restored.tick(600);
        assert_eq!(s.world.get(a), restored.world.get(a));
        restored.game = Game::DragonAge2;
        let f = restored
            .table
            .function_by_name("InteractWithObject")
            .unwrap()
            .clone();
        let mut h = ScriptHost {
            sim: &mut restored,
            event: None,
            script: "test".into(),
        };
        assert!(engine::call(&mut h, a, &f, &[]).is_none());
    }

    #[test]
    fn finite_pose_counts_only_multiply_a_looping_animation() {
        for looping in [false, true] {
            let (mut s, a, b) = sim();
            s.world.tables().insert(
                "#0",
                &["#7a398af9", "#12a02374"],
                vec![
                    (11, vec![Cell::String("enter".into()), Cell::Int(0)]),
                    (
                        21,
                        vec![Cell::String("pose".into()), Cell::Int(i64::from(looping))],
                    ),
                    (41, vec![Cell::String("exit".into()), Cell::Int(0)]),
                ],
            );
            s.world.tables().insert(
                "apr_base",
                &["#500976ec"],
                vec![(0, vec![Cell::String("test.anb".into())])],
            );
            for (action, ms) in [("enter", 100), ("pose", 200), ("exit", 300)] {
                s.ability_anims.insert(
                    format!("test.anb|{action}"),
                    Some(crate::combat::AbilityAnimLen {
                        raw_ms: ms,
                        len_ms: ms,
                        impact_ms: None,
                    }),
                );
            }
            request(&mut s, a, b, 1, 1, 3, true);
            s.tick(100);
            assert_eq!(animation(&s, a), 21);
            let pose_ms = if looping { 600 } else { 200 };
            s.tick(pose_ms - 1);
            assert_eq!(animation(&s, a), 21);
            s.tick(1);
            assert_eq!(animation(&s, a), 41);
            s.tick(299);
            assert!(!s.world.get(a).unwrap().commands.is_empty());
            s.tick(1);
            assert!(s.world.get(a).unwrap().commands.is_empty());
        }
    }

    #[test]
    fn approach_uses_the_placeable_point_and_skip_avoids_move_and_turn() {
        for skip in [0, 1] {
            let (mut s, a, b) = sim();
            let area = s.world.spawn(ObjectKind::Area, "area");
            let actor = s.world.get_mut(a).unwrap();
            actor.area = Some(area);
            actor.position = [10.0, 30.0, 0.0];
            actor.facing = 90.0;
            let target = s.world.get_mut(b).unwrap();
            target.area = Some(area);
            target.position = [10.0, 20.0, 0.0];
            target.facing = 0.0;
            s.world.tables().insert(
                "#123",
                &[
                    "#90674659",
                    "#1725fa61",
                    "#414a699c",
                    "#909085da",
                    "#92321b14",
                    "#85ffd766",
                ],
                vec![(
                    1,
                    vec![
                        Cell::Int(11),
                        Cell::Int(41),
                        Cell::Float(0.15),
                        Cell::Float(2.0),
                        Cell::Float(0.0),
                        Cell::Float(0.0),
                    ],
                )],
            );
            let f = s
                .table
                .function_by_name("InteractWithObject")
                .unwrap()
                .clone();
            let mut h = ScriptHost {
                sim: &mut s,
                event: None,
                script: "test".into(),
            };
            engine::call(
                &mut h,
                a,
                &f,
                &[
                    Value::Object(a),
                    Value::Object(b),
                    Value::Int(1),
                    Value::Int(1),
                    Value::Int(0),
                    Value::Int(1),
                    Value::Int(skip),
                ],
            )
            .unwrap()
            .unwrap();
            s.tick(0);
            if skip == 0 {
                let command = &s.world.get(a).unwrap().commands[0];
                assert!(command.from.is_some());
                let goal = command.approach.unwrap();
                assert!((goal[0] - 10.0).abs() < 1e-5 && (goal[1] - 22.0).abs() < 1e-5);
                let due = command.due.unwrap();
                s.tick(due);
                let actor = s.world.get(a).unwrap();
                assert!((actor.position[1] - 22.0).abs() < 1e-5);
                assert!((actor.facing - 180.0).abs() < 1e-5);
                assert_eq!(animation(&s, a), 11);
            } else {
                let actor = s.world.get(a).unwrap();
                assert_eq!(actor.position, [10.0, 30.0, 0.0]);
                assert_eq!(actor.facing, 90.0);
                assert_eq!(animation(&s, a), 11);
            }
        }
    }

    #[test]
    fn an_infinite_interaction_still_notices_a_destroyed_target() {
        let (mut s, a, b) = sim();
        request(&mut s, a, b, 1, 1, -1, true);
        s.tick(1);
        s.world.objects.remove(&b);
        s.tick(0);
        assert!(s.world.get(a).unwrap().commands.is_empty());
        assert!(
            s.world
                .queue
                .values()
                .any(|q| q.target == a && q.event.ints == vec![37, -7])
        );
    }

    #[test]
    fn a_finite_pose_can_become_infinite_and_a_nonpositive_pose_holds_the_entry() {
        for pose in [0, 1] {
            let (mut s, a, b) = sim();
            request(&mut s, a, b, 1, pose, 1, true);
            s.tick(0);
            request(&mut s, a, b, 1, pose, -1, true);
            s.tick(100);
            assert_eq!(animation(&s, a), if pose == 0 { 11 } else { 21 });
            assert_eq!(s.world.get(a).unwrap().commands[0].due, Some(u64::MAX));
            request(&mut s, a, b, 1, pose, 0, true);
            s.tick(0);
            assert_eq!(animation(&s, a), 41);
            s.tick(1);
            assert!(s.world.get(a).unwrap().commands.is_empty());
        }
    }

    #[test]
    fn an_interaction_command_can_initialize_another_creature_from_its_payload() {
        let (mut s, a, b) = sim();
        request(&mut s, a, b, 1, 2, -1, false);
        let other = s.world.spawn(ObjectKind::Creature, "other");
        let c = s.world.get(a).unwrap().commands[0].clone();
        let handle = s.world.store(crate::world::EngineData::Command(c));
        let f = s.table.function_by_name("AddCommand").unwrap().clone();
        let mut h = ScriptHost {
            sim: &mut s,
            event: None,
            script: "test".into(),
        };
        let ty = h.sim.types.command;
        engine::call(
            &mut h,
            a,
            &f,
            &[
                Value::Object(other),
                Value::Engine {
                    ty,
                    handle: Some(handle),
                },
                Value::Int(1),
                Value::Int(0),
                Value::Int(-1),
            ],
        )
        .unwrap()
        .unwrap();
        s.tick(1);
        assert_eq!(animation(&s, other), 31);
        assert_eq!(
            s.world
                .get(other)
                .unwrap()
                .interaction
                .as_ref()
                .unwrap()
                .requested,
            Settings {
                id: 1,
                pose: 2,
                loops: -1,
                play_exit: false
            }
        );
    }

    #[test]
    fn reciprocal_object_approaches_end_at_capsule_contact() {
        let (mut s, a, b) = sim();
        let area = s.world.spawn(ObjectKind::Area, "area");
        s.world.get_mut(b).unwrap().kind = ObjectKind::Creature;
        for (who, x) in [(a, 0.0), (b, 4.0)] {
            let o = s.world.get_mut(who).unwrap();
            o.area = Some(area);
            o.position = [x, 0.0, 0.0];
        }
        let f = s
            .table
            .function_by_name("InteractWithObject")
            .unwrap()
            .clone();
        let mut h = ScriptHost {
            sim: &mut s,
            event: None,
            script: "test".into(),
        };
        engine::call(
            &mut h,
            a,
            &f,
            &[
                Value::Object(a),
                Value::Object(b),
                Value::Int(1),
                Value::Int(1),
                Value::Int(-1),
                Value::Int(1),
                Value::Int(0),
            ],
        )
        .unwrap()
        .unwrap();
        for _ in 0..200 {
            s.tick(50);
        }
        for who in [a, b] {
            assert_eq!(s.world.get(who).unwrap().commands[0].due, Some(u64::MAX));
            assert!(s.world.get(who).unwrap().commands[0].from.is_none());
        }
        let p = s.world.get(a).unwrap().position;
        let q = s.world.get(b).unwrap().position;
        assert!(crate::engine::dist(p, q) >= s.capsule_radius(a) + s.capsule_radius(b) - 0.05);
    }
}
