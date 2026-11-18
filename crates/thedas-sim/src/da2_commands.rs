//! DA2 script command admission, separate from the active executor.

use thedas_script::vm::ObjectId;

use crate::{Sim, engine::cell_int, sim::constant, trace::TraceEntry, world::Command};

/// The native command data comparison excludes executor state (0x60fe40).
fn same_payload(a: &Command, b: &Command) -> bool {
    a.ty == b.ty
        // The simulation stores some native payload variants in the name
        // (for example sheathe versus unsheathe, both command type 39).
        && a.name == b.name
        && a.is_static == b.is_static
        && a.da2_player_issued == b.da2_player_issued
        && a.target == b.target
        && (matches!(
            a.name.as_str(),
            "CommandMoveToObject" | "CommandMoveAwayFromObject"
        ) || a.location == b.location)
        && (a.name == "CommandInitiateDialog" || a.seconds == b.seconds)
        && a.max_range == b.max_range
        && a.ints == b.ints
        && a.strings == b.strings
        && a.da2_basic_attack_fallback == b.da2_basic_attack_fallback
        && (a.name != "CommandMoveToMultiLocations" || a.path == b.path)
}

/// Native task protection is independent of static state and readiness.
/// Movement tasks release it after setup; wait task 36 does likewise.
fn task_protected(c: &Command) -> bool {
    c.from.is_none()
        // Placeholder selection precedes task construction.
        && !(c.name == "CommandUseAbility" && c.ints.first() == Some(&1))
        && matches!(
            c.name.as_str(),
            "CommandUseAbility"
                | "CommandAttack"
                | "CommandPlayAnimation"
                | "CommandStartConversation"
                | "CommandUseObject"
                | "CommandJumpToObject"
                | "CommandJumpToLocation"
                | "CommandTurn"
                | "CommandSwitchWeaponSet"
        )
}

fn fresh(c: &mut Command) {
    c.due = None;
    c.from = None;
    c.turned_at = None;
    c.approach = None;
    c.ring_wait = false;
    c.arrived = false;
    c.impacted = false;
    c.after_impact_ms = None;
    c.cancellable_at = None;
    c.cancellable = false;
    c.da2_target_prepared = false;
    if matches!(
        c.name.as_str(),
        "CommandMoveToObject" | "CommandMoveAwayFromObject"
    ) {
        c.location = None;
    }
    if c.name == "CommandInitiateDialog" {
        c.seconds = 0.0;
    }
    if c.name != "CommandMoveToMultiLocations" {
        c.path.clear();
    }
}

impl Sim {
    /// Native combat input reaches 0x5d2330, then ordinary admission.
    pub(crate) fn da2_player_command(&mut self, who: ObjectId, mut c: Command) {
        c.da2_player_issued = true;
        self.da2_add_command(who, c, 0, false, -1);
    }

    pub(crate) fn da2_add_command(
        &mut self,
        who: ObjectId,
        mut c: Command,
        front: i32,
        is_static: bool,
        behavior: i32,
    ) -> bool {
        let Some(o) = self.world.get(who) else {
            return false;
        };
        if c.ty == 0 {
            return false;
        }
        // 0x5cd3ef: Drive cannot precede the first waiting UseAbility command.
        let active = usize::from(o.commands.first().is_some_and(|c| c.due.is_some()));
        if c.ty == 33 && o.commands.get(active).is_some_and(|c| c.ty == 25) {
            return false;
        }
        c.is_static = is_static;
        fresh(&mut c);
        // 0x5cd403: notify before inspecting the active command/default behavior.
        // The callback may change either queue, so read them again afterwards.
        if c.da2_player_issued {
            let mut event = self.new_event("PLAYER_COMMAND_ADDED", Some(who));
            event.objects = vec![who];
            event.ints = vec![c.ty];
            if event.ty != 0
                && let Some(script) = self.world.get(who).and_then(|o| o.script.clone())
            {
                self.run_script(&script, who, Some(event));
            }
        }
        let Some(o) = self.world.get(who) else {
            return false;
        };
        let running = o.commands.first().filter(|c| c.due.is_some());
        let equal = running.is_some_and(|old| same_payload(&c, old));
        // PlayAnimation and UseAbility share the native same-type comparison.
        let kind = |ty| if ty == 14 { 25 } else { ty };
        let same_type = running.map_or(0, |old| kind(old.ty)) == kind(c.ty);
        let behavior = if behavior == -1 {
            self.world.tables().get(94, "").map_or(0, |t| {
                cell_int(t.cell_by_hash(
                    if same_type { 0x7569_547a } else { 0xed36_668a },
                    i64::from(c.ty),
                ))
            })
        } else {
            behavior
        };
        // 0x5cd4da: a player-issued active command downgrades hard clear,
        // except for incoming native type 32.
        let behavior =
            if behavior == 2 && running.is_some_and(|old| old.da2_player_issued) && c.ty != 32 {
                1
            } else {
                behavior
            };
        let active = usize::from(running.is_some());
        if matches!(behavior, 1 | 2) {
            // 0x5ca4c0: both paths prune ordinary waiting entries. The current
            // command is separate; equality prevents clearing it.
            // A pending follow move can also be pruned before it starts.
            if active == 0 && o.commands.first().is_some_and(|old| !old.is_static) {
                self.world.following.remove(&who);
            }
            let mut index = 0;
            self.world.get_mut(who).unwrap().commands.retain(|old| {
                let keep = index < active || old.is_static;
                index += 1;
                keep
            });
            let clear = self
                .world
                .get(who)
                .unwrap()
                .commands
                .first()
                .is_some_and(|old| {
                    old.due.is_some() && !equal && (!old.is_static || behavior == 2)
                    // 0x5c6274: soft and hard clears respect the running task.
                    // Successful ready handoff uses a separate override.
                    && !task_protected(old)
                });
            if clear {
                let old = self.world.get_mut(who).unwrap().commands.remove(0);
                self.world.following.remove(&who);
                if old.from.is_some() {
                    self.da2_stop_locomotion(who);
                }
                if old.name == "CommandUseAbility" {
                    self.complete_combat_target(who);
                }
                let result = constant(&self.table, "COMMAND_FAILED_COMMAND_CLEARED");
                let mut event = self.new_event("COMMAND_COMPLETE", Some(who));
                event.ints = vec![old.ty, result];
                self.world.trace.push(TraceEntry::Command {
                    t: self.world.time_ms,
                    object: who,
                    command: old.name.clone(),
                    status: format!("failed {result}"),
                });
                self.world.get_mut(who).unwrap().combat.previous_command = Some(old);
                self.world.queue_event(0, who, event, None);
            }
        }
        // 0x5cd53d: equal player-issued ability/animation commands still queue.
        if equal && !(kind(c.ty) == 25 && c.da2_player_issued) {
            return true;
        }
        self.world.trace.push(TraceEntry::Command {
            t: self.world.time_ms,
            object: who,
            command: c.name.clone(),
            status: "queued".into(),
        });
        let commands = &self.world.get(who).unwrap().commands;
        let active = usize::from(commands.first().is_some_and(|c| c.due.is_some()));
        if commands.is_empty() || (front == 1 && active == 0) {
            self.world.following.remove(&who);
        }
        let o = self.world.get_mut(who).unwrap();
        if front == 1 {
            o.commands.insert(active, c);
        } else {
            o.commands.push(c);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        sim::ScriptHost,
        world::{EngineData, ObjectKind},
    };
    use std::sync::Arc;
    use thedas_script::{EngineTable, vm::Value};
    use thedas_vfs::{Game, Vfs};

    fn sim() -> (Sim, ObjectId) {
        let table = Arc::new(EngineTable::parse(b"#define ENGINE_STRUCTURE_0 command\nint AddCommand(object o, command c, int front=0, int fixed=0, int behavior=-1)=533;\nint EVENT_TYPE_COMMAND_COMPLETE=1004;\nint COMMAND_FAILED_COMMAND_CLEARED=-2;\n").unwrap());
        let mut s = Sim::new(Arc::new(Vfs::new()), table, 1);
        s.game = Game::DragonAge2;
        let who = s.world.spawn(ObjectKind::Creature, "actor");
        (s, who)
    }

    fn wait(seconds: f32) -> Command {
        Command {
            name: "CommandWait".into(),
            ty: 13,
            seconds,
            ..Default::default()
        }
    }

    fn add(s: &mut Sim, who: ObjectId, c: Command, front: i32, fixed: bool, behavior: i32) -> bool {
        let handle = s.world.store(EngineData::Command(c));
        let f = s.table.function_by_name("AddCommand").cloned().unwrap();
        let ty = 0;
        let mut h = ScriptHost {
            sim: s,
            event: None,
            script: "test".into(),
        };
        let result = crate::engine::call(
            &mut h,
            who,
            &f,
            &[
                Value::Object(who),
                Value::Engine {
                    ty,
                    handle: Some(handle),
                },
                Value::Int(front),
                Value::Int(i32::from(fixed)),
                Value::Int(behavior),
            ],
        )
        .unwrap()
        .unwrap();
        result == Some(Value::Int(1))
    }

    #[test]
    fn replacement_clears_waiting_ordinary_commands_and_respects_static_current() {
        for behavior in [1, 2] {
            for fixed in [false, true] {
                let (mut s, who) = sim();
                let mut active = wait(10.);
                active.due = Some(10000);
                active.is_static = fixed;
                let ordinary = wait(20.);
                // Older DA2 front insertion could leave a paused executor in
                // the waiting slice. Its deadline does not make it active.
                let mut paused = wait(25.);
                paused.due = Some(25000);
                let mut waiting_static = wait(30.);
                waiting_static.is_static = true;
                s.world.get_mut(who).unwrap().commands =
                    vec![active.clone(), ordinary, paused, waiting_static.clone()];
                assert!(add(&mut s, who, wait(40.), 1, false, behavior));
                let o = s.world.get(who).unwrap();
                if fixed && behavior == 1 {
                    assert_eq!(o.commands, vec![active, wait(40.), waiting_static]);
                    assert!(o.combat.previous_command.is_none());
                } else {
                    assert_eq!(o.commands, vec![wait(40.), waiting_static]);
                    assert_eq!(o.combat.previous_command, Some(active));
                }
                let results: Vec<_> = s
                    .world
                    .queue
                    .values()
                    .map(|q| q.event.ints.clone())
                    .collect();
                assert_eq!(
                    results,
                    if fixed && behavior == 1 {
                        vec![]
                    } else {
                        vec![vec![13, -2]]
                    }
                );
                let json = s.save_json().unwrap();
                s.load_json(&json).unwrap();
                assert_eq!(
                    s.world.get(who).unwrap().commands[0].due.is_some(),
                    fixed && behavior == 1
                );
            }
        }
    }

    #[test]
    fn front_means_waiting_front_and_readded_active_payload_does_not_duplicate() {
        let (mut s, who) = sim();
        let mut active = wait(10.);
        active.due = Some(10000);
        s.world.get_mut(who).unwrap().commands = vec![active.clone(), wait(20.)];
        assert!(add(&mut s, who, wait(30.), 2, false, 0));
        assert_eq!(
            s.world.get(who).unwrap().commands,
            vec![active.clone(), wait(20.), wait(30.)]
        );
        assert!(add(&mut s, who, wait(40.), 1, false, 0));
        assert_eq!(
            s.world.get(who).unwrap().commands,
            vec![active.clone(), wait(40.), wait(20.), wait(30.)]
        );
        // Matching uses payload, not the active copy's due/readiness fields.
        assert!(add(&mut s, who, active.clone(), 1, false, 1));
        assert_eq!(s.world.get(who).unwrap().commands, vec![active]);
    }

    #[test]
    fn script_order_releases_follow_ownership_when_it_replaces_the_follow_move() {
        let (mut s, who) = sim();
        let follow = Command {
            name: "CommandMoveToLocation".into(),
            ty: 2,
            due: Some(10_000),
            ..Default::default()
        };
        s.world.get_mut(who).unwrap().commands = vec![follow.clone()];
        s.world.following.insert(who);
        assert!(add(&mut s, who, wait(10.), 1, false, 0));
        assert!(s.world.following.contains(&who));
        assert_eq!(s.world.get(who).unwrap().commands[0], follow);
        assert!(add(&mut s, who, wait(20.), 1, false, 1));
        assert!(!s.world.following.contains(&who));

        let pending = Command {
            due: None,
            ..follow
        };
        s.world.get_mut(who).unwrap().commands = vec![pending.clone()];
        s.world.following.insert(who);
        assert!(add(&mut s, who, wait(30.), 1, false, 0));
        assert!(!s.world.following.contains(&who));

        let fixed = Command {
            is_static: true,
            ..wait(40.)
        };
        s.world.get_mut(who).unwrap().commands = vec![pending, fixed.clone()];
        s.world.following.insert(who);
        assert!(add(&mut s, who, wait(50.), 0, false, 1));
        assert_eq!(s.world.get(who).unwrap().commands, vec![fixed, wait(50.)]);
        assert!(!s.world.following.contains(&who));
    }

    #[test]
    fn payload_equality_excludes_computed_movement_goals_and_dialog_stall_time() {
        let sheath = Command {
            ty: 39,
            name: "CommandSheatheWeapons".into(),
            ..Default::default()
        };
        let mut unsheath = sheath.clone();
        unsheath.name = "CommandUnsheatheWeapons".into();
        assert!(!same_payload(&sheath, &unsheath));
        for name in ["CommandMoveToObject", "CommandMoveAwayFromObject"] {
            let input = Command {
                name: name.into(),
                ty: 21,
                target: Some(77),
                seconds: 2.0,
                ..Default::default()
            };
            let mut active = input.clone();
            active.location = Some(crate::world::Location {
                area: 1,
                position: [10.0, 20.0, 3.0],
                facing: 45.0,
            });
            active.path = vec![[5.0, 6.0, 7.0]];
            active.due = Some(10000);
            assert!(same_payload(&input, &active));
            fresh(&mut active);
            assert_eq!(active, input);
        }
        let input = Command {
            name: "CommandInitiateDialog".into(),
            ty: 18,
            target: Some(77),
            ..Default::default()
        };
        let mut active = input.clone();
        active.seconds = 1200.0;
        assert!(same_payload(&input, &active));
        fresh(&mut active);
        assert_eq!(active, input);
    }

    #[test]
    fn animation_alias_uses_same_type_defaults_and_drive_checks_only_waiting_front() {
        let (mut s, who) = sim();
        s.world.tables.insert(
            "#94",
            &["#ed36668a", "#7569547a"],
            vec![(
                14,
                vec![
                    thedas_formats::gda::Cell::Int(0),
                    thedas_formats::gda::Cell::Int(1),
                ],
            )],
        );
        let active = Command {
            name: "CommandUseAbility".into(),
            ty: 25,
            due: Some(10000),
            ..Default::default()
        };
        let animation = Command {
            name: "CommandPlayAnimation".into(),
            ty: 14,
            ints: vec![7],
            ..Default::default()
        };
        s.world.get_mut(who).unwrap().commands = vec![active.clone(), wait(20.)];
        assert!(add(&mut s, who, animation.clone(), 0, false, -1));
        assert_eq!(
            s.world.get(who).unwrap().commands,
            vec![active.clone(), animation]
        );
        let drive = Command {
            name: "CommandDrive".into(),
            ty: 33,
            ..Default::default()
        };
        // The active ability does not block Drive; the waiting front does.
        s.world.get_mut(who).unwrap().commands = vec![active.clone(), wait(20.)];
        assert!(add(&mut s, who, drive.clone(), 0, false, 0));
        s.world.get_mut(who).unwrap().commands = vec![
            active.clone(),
            Command {
                due: None,
                ..active
            },
        ];
        let before = s.world.get(who).unwrap().commands.clone();
        assert!(!add(&mut s, who, drive, 1, false, 2));
        assert_eq!(s.world.get(who).unwrap().commands, before);
        assert!(!add(&mut s, who, Command::default(), 0, false, 0));
        assert_eq!(s.world.get(who).unwrap().commands, before);
    }

    #[test]
    fn default_behavior_preserves_protected_animation_until_successful_ready_handoff() {
        let (mut s, who) = sim();
        s.world.tables.insert(
            "#94",
            &["#ed36668a", "#7569547a"],
            vec![(
                25,
                vec![
                    thedas_formats::gda::Cell::Int(0),
                    thedas_formats::gda::Cell::Int(1),
                ],
            )],
        );
        let mut current = Command {
            name: "CommandUseAbility".into(),
            ty: 25,
            ints: vec![3001],
            due: Some(10000),
            ..Default::default()
        };
        let next = Command {
            name: "CommandUseAbility".into(),
            ty: 25,
            ints: vec![3002],
            ..Default::default()
        };
        s.world.get_mut(who).unwrap().commands = vec![current.clone(), wait(20.)];
        assert!(add(&mut s, who, next.clone(), 1, false, -1));
        assert_eq!(
            s.world.get(who).unwrap().commands,
            vec![current.clone(), next.clone()]
        );
        assert!(s.world.queue.is_empty());
        s.finish_chained_ability(who);
        assert_eq!(
            s.world.get(who).unwrap().commands,
            vec![current.clone(), next.clone()]
        );
        current.cancellable = true;
        s.world.get_mut(who).unwrap().commands = vec![current.clone(), wait(20.)];
        assert!(add(&mut s, who, next.clone(), 1, false, -1));
        assert_eq!(
            s.world.get(who).unwrap().commands,
            vec![current.clone(), next.clone()]
        );
        s.finish_chained_ability(who);
        assert_eq!(s.world.get(who).unwrap().commands, vec![next]);
        assert_eq!(
            s.world
                .queue
                .values()
                .map(|q| q.event.ints.clone())
                .collect::<Vec<_>>(),
            vec![vec![25, 1]]
        );
        current.impacted = true;
        assert_eq!(
            s.world.get(who).unwrap().combat.previous_command,
            Some(current)
        );
        assert!(!add(
            &mut s,
            thedas_script::vm::OBJECT_INVALID,
            wait(1.),
            1,
            false,
            0
        ));
    }

    #[test]
    fn player_flag_distinguishes_payloads_and_only_equal_abilities_queue_again() {
        for (ty, name) in [
            (25, "CommandUseAbility"),
            (14, "CommandPlayAnimation"),
            (13, "CommandWait"),
        ] {
            for player in [false, true] {
                let (mut s, who) = sim();
                let current = Command {
                    ty,
                    name: name.into(),
                    da2_player_issued: player,
                    due: Some(1000),
                    ..Default::default()
                };
                let mut other = current.clone();
                other.da2_player_issued = !player;
                assert!(!same_payload(&current, &other));
                s.world.get_mut(who).unwrap().commands = vec![current.clone(), wait(5.)];
                assert!(add(&mut s, who, current.clone(), 0, false, 1));
                let commands = &s.world.get(who).unwrap().commands;
                assert_eq!(commands[0], current);
                assert_eq!(commands.len(), if player && ty != 13 { 2 } else { 1 });
                if commands.len() == 2 {
                    assert!(commands[1].due.is_none());
                }
            }
        }
    }

    #[test]
    fn player_active_downgrades_hard_clear_except_for_native_type_32() {
        for player in [false, true] {
            for ty in [13, 32] {
                let (mut s, who) = sim();
                let current = Command {
                    is_static: true,
                    da2_player_issued: player,
                    due: Some(1000),
                    ..wait(1.)
                };
                s.world.get_mut(who).unwrap().commands = vec![current.clone(), wait(5.)];
                let incoming = Command { ty, ..wait(10.) };
                assert!(add(&mut s, who, incoming.clone(), 0, false, 2));
                let kept = player && ty != 32;
                assert_eq!(
                    s.world.get(who).unwrap().commands,
                    if kept {
                        vec![current, incoming]
                    } else {
                        vec![incoming]
                    }
                );
                assert_eq!(s.world.queue.len(), usize::from(!kept));
            }
        }
    }

    #[test]
    fn player_added_callback_precedes_admission_and_can_clear_the_old_front() {
        let ldf = b"#define ENGINE_STRUCTURE_0 command\n#define ENGINE_STRUCTURE_1 event\nevent GetCurrentEvent()=1;\nint GetEventType(event e)=2;\nint GetEventInteger(event e, int i)=3;\nvoid SetLocalInt(object o, int n, int v)=4;\nint GetCommandQueueSize(object o)=5;\nvoid ClearAllCommands(object o)=6;\nint EVENT_TYPE_PLAYER_COMMAND_ADDED=94;\nint COMMAND_TYPE_USE_ABILITY=25;\n";
        let mut s = Sim::new(
            Arc::new(Vfs::new()),
            Arc::new(EngineTable::parse(ldf).unwrap()),
            1,
        );
        s.game = Game::DragonAge2;
        let _area = s.world.spawn(ObjectKind::Area, "area");
        let who = s.world.spawn(ObjectKind::Creature, "actor");
        let mut code = Vec::new();
        let integer = |code: &mut Vec<u8>, n: i32| {
            code.extend([4, 3]);
            code.extend(n.to_be_bytes());
        };
        let object = |code: &mut Vec<u8>| {
            code.extend([4, 6]);
            code.extend(who.to_be_bytes());
        };
        let action = |code: &mut Vec<u8>, n: u8, argc: u8| {
            code.extend([5, 0, 0, n, argc]);
        };
        // Record type, command type, and waiting count before the incoming command.
        action(&mut code, 1, 0);
        action(&mut code, 2, 1);
        integer(&mut code, 10);
        object(&mut code);
        action(&mut code, 4, 3);
        integer(&mut code, 0);
        action(&mut code, 1, 0);
        action(&mut code, 3, 2);
        integer(&mut code, 11);
        object(&mut code);
        action(&mut code, 4, 3);
        object(&mut code);
        action(&mut code, 5, 1);
        integer(&mut code, 12);
        object(&mut code);
        action(&mut code, 4, 3);
        object(&mut code);
        action(&mut code, 6, 1);
        code.extend([0x20, 0]);
        let mut bytes = b"NCS V1.0B".to_vec();
        bytes.extend(((13 + code.len()) as u32).to_be_bytes());
        bytes.extend(code);
        s.add_script("added.ncs", thedas_script::Ncs::parse(&bytes).unwrap());
        let o = s.world.get_mut(who).unwrap();
        o.script = Some("added.ncs".into());
        o.locals.order = (0..13).map(|i| format!("slot{i}")).collect();
        o.commands = vec![wait(5.)];
        s.order_ability(who, 200, who, "");
        assert!(
            s.world.trace.script_errors.is_empty(),
            "{:?}",
            s.world.trace.script_errors
        );
        let o = s.world.get(who).unwrap();
        assert_eq!(o.locals.ints["10"], 94);
        assert_eq!(o.locals.ints["11"], 25);
        assert_eq!(o.locals.ints["12"], 1);
        assert_eq!(o.commands.len(), 1);
        assert!(o.commands[0].da2_player_issued);
        assert_eq!(o.commands[0].ints, vec![200]);
        let saved = s.save_json().unwrap();
        s.load_json(&saved).unwrap();
        assert!(s.world.get(who).unwrap().commands[0].da2_player_issued);
        let mut old = serde_json::to_value(&s.world.get(who).unwrap().commands[0]).unwrap();
        old.as_object_mut().unwrap().remove("da2_player_issued");
        assert!(
            !serde_json::from_value::<Command>(old)
                .unwrap()
                .da2_player_issued
        );
    }
    #[test]
    fn traced_running_tasks_survive_admission_and_waiting_copies_do_not() {
        for (ty, name) in [
            (1, "CommandAttack"),
            (14, "CommandPlayAnimation"),
            (18, "CommandStartConversation"),
            (31, "CommandUseObject"),
            (9, "CommandJumpToObject"),
            (10, "CommandJumpToLocation"),
            (41, "CommandTurn"),
            (40, "CommandSwitchWeaponSet"),
        ] {
            for behavior in [1, 2] {
                let (mut s, who) = sim();
                let current = Command {
                    ty,
                    name: name.into(),
                    due: Some(100),
                    cancellable: true,
                    ..Default::default()
                };
                let waiting_copy = Command {
                    due: None,
                    ..current.clone()
                };
                s.world.get_mut(who).unwrap().commands = vec![current.clone(), waiting_copy];
                let incoming = wait(20.);
                assert!(add(&mut s, who, incoming.clone(), 1, false, behavior));
                assert_eq!(
                    s.world.get(who).unwrap().commands,
                    vec![current.clone(), incoming]
                );
                assert!(s.world.queue.is_empty());
                let saved = s.save_json().unwrap();
                s.load_json(&saved).unwrap();
                assert!(add(&mut s, who, wait(30.), 1, false, behavior));
                assert_eq!(s.world.get(who).unwrap().commands, vec![current, wait(30.)]);
            }
        }
    }

    #[test]
    fn selection_without_a_task_waits_and_locomotion_are_unprotected() {
        for behavior in [1, 2] {
            for mut current in [
                Command {
                    due: Some(1000),
                    ..wait(1.)
                },
                Command {
                    name: "CommandMoveToLocation".into(),
                    ty: 20,
                    due: Some(1000),
                    from: Some(([0.; 3], 0)),
                    ..Default::default()
                },
                Command {
                    name: "CommandUseAbility".into(),
                    ty: 25,
                    due: Some(1000),
                    from: Some(([0.; 3], 0)),
                    ..Default::default()
                },
                Command {
                    name: "CommandUseAbility".into(),
                    ty: 25,
                    ints: vec![1],
                    due: Some(1000),
                    ..Default::default()
                },
            ] {
                let (mut s, who) = sim();
                current.is_static = false;
                s.world.get_mut(who).unwrap().commands = vec![current.clone()];
                assert!(add(&mut s, who, wait(20.), 1, false, behavior));
                assert_eq!(s.world.get(who).unwrap().commands, vec![wait(20.)]);
                assert_eq!(
                    s.world.get(who).unwrap().combat.previous_command,
                    Some(current)
                );
            }
        }
    }

    #[test]
    fn protected_animation_still_completes_at_its_endpoint() {
        let (mut s, who) = sim();
        s.world.get_mut(who).unwrap().commands = vec![Command {
            name: "CommandPlayAnimation".into(),
            ty: 14,
            due: Some(100),
            ..Default::default()
        }];
        assert!(add(&mut s, who, wait(20.), 1, false, 2));
        assert_eq!(s.world.get(who).unwrap().commands.len(), 2);
        s.tick(100);
        let o = s.world.get(who).unwrap();
        assert_eq!(o.commands.len(), 1);
        assert_eq!(o.commands[0].name, "CommandWait");
        assert_eq!(o.combat.previous_command.as_ref().unwrap().ty, 14);
    }
}
