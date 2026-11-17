//! Origins' scripted animation command timing. See docs/notes/play-animation-duration.md.

use crate::Sim;
use crate::combat::{COL_ANIM_ACTION, COL_ANIM_NEXT, anim_loops};
use crate::engine::clean;
use crate::m2da::Table;
use crate::world::Command;
use thedas_formats::gda::Cell;
use thedas_script::vm::ObjectId;

/// `apr_base`'s own animation tree, including emotes absent from its combat tree.
const COL_AMBIENT_TREE: u32 = 0x5009_76ec;

impl Sim {
    /// The executor (0x7a4390) waits for the initial animation, any requested loops,
    /// and the following transition. A negative count releases the queue when its
    /// loop starts, with COMMAND_LOOPING rather than COMMAND_SUCCESSFUL.
    /// `None` means the resource is missing; the caller completes at its minimum tick.
    pub(crate) fn play_animation_duration(&mut self, id: ObjectId, cmd: &Command) -> Option<u64> {
        let anims = self.world.tables().get(0, "")?;
        let anim = i64::from(*cmd.ints.first()?);
        if anim <= 0 || !anims.has_row(anim) {
            return None;
        }
        let loops = cmd.ints.get(1).copied().unwrap_or(0);
        // Native boolean 4 is bPlayNext; native integer 2 is nLoops. The
        // executor also stages an explicit next-animation override in integer 4,
        // which this script constructor does not set (0x66fc20 -> 0x744870).
        let staged = cmd.ints.get(2) == Some(&1);
        let mut trees = Vec::new();
        let appearance = self.world.get(id)?.appearance as u16;
        if let Some(t) = self.world.tables().get(2, "")
            && let Some(Cell::String(tree)) =
                t.cell_by_hash(COL_AMBIENT_TREE, i64::from(appearance))
        {
            let tree = clean(tree).to_ascii_lowercase();
            if !tree.is_empty() {
                trees.push(tree);
            }
        }
        // Combat actions can still be requested explicitly. Lookup precedence beyond
        // the creature's own tree remains provisional, as it is for ability timing.
        trees.extend(self.ability_trees(id));
        let first = if anim_loops(&anims, anim) || staged {
            self.animation_length(&anims, &trees, anim)
        } else {
            self.animation_chain_length(&anims, &trees, anim)
        };
        // Native float 3, set by SetCommandFloat, scales only the initial stage.
        let first = if cmd.seconds > 0.0 {
            (first as f32 / cmd.seconds) as u64
        } else {
            first
        };
        if anim_loops(&anims, anim) {
            if loops < 0 {
                return Some(0);
            }
            let first = first.saturating_mul(loops as u64);
            return Some(if staged {
                first.saturating_add(self.animation_chain_length(
                    &anims,
                    &trees,
                    next(&anims, anim),
                ))
            } else {
                first
            });
        }
        if !staged {
            return Some(first);
        }
        let following = next(&anims, anim);
        if anim_loops(&anims, following) {
            if loops < 0 {
                return Some(first);
            }
            let repeated = self
                .animation_length(&anims, &trees, following)
                .saturating_mul(loops as u64);
            let end = self.animation_chain_length(&anims, &trees, next(&anims, following));
            Some(first.saturating_add(repeated).saturating_add(end))
        } else {
            // State 0 enters state 2 directly for a non-looping successor.
            Some(first.saturating_add(self.animation_length(&anims, &trees, following)))
        }
    }

    /// The loop left playing when the command reports COMMAND_LOOPING (2). The
    /// subaction runner removes that result, just as it removes success (0x73cc99).
    pub(crate) fn continuing_animation(&self, cmd: &Command) -> Option<Command> {
        if cmd.name != "CommandPlayAnimation" || cmd.ints.get(1).is_none_or(|&n| n >= 0) {
            return None;
        }
        let anims = self.world.tables().get(0, "")?;
        let mut anim = i64::from(*cmd.ints.first()?);
        if anim <= 0 || !anims.has_row(anim) {
            return None;
        }
        let initial_loop = anim_loops(&anims, anim);
        if !initial_loop {
            if cmd.ints.get(2) != Some(&1) {
                return None;
            }
            anim = next(&anims, anim);
        }
        if !anim_loops(&anims, anim) {
            return None;
        }
        let mut active = cmd.clone();
        active.ints[0] = anim as i32;
        if !initial_loop {
            active.seconds = 0.0;
            active.from = Some(([0.0; 3], self.world.time_ms));
        }
        active.due = None;
        Some(active)
    }

    fn animation_length(&mut self, anims: &Table, trees: &[String], anim: i64) -> u64 {
        if anim <= 0 {
            return 0;
        }
        let Some(Cell::String(action)) = anims.cell_by_hash(COL_ANIM_ACTION, anim) else {
            return 0;
        };
        let action = clean(action).to_ascii_lowercase();
        // Reuse the tree -> .gad reader and cache. The native command timer reads
        // raw duration (0x694e50 -> 0xa38ed0), independently of tree/.gad speed.
        self.ability_anim(trees, &action).map_or(0, |a| a.raw_ms)
    }

    /// 0x6957a0 adds up to five non-looping rows, stopping before a loop.
    fn animation_chain_length(&mut self, anims: &Table, trees: &[String], mut anim: i64) -> u64 {
        let mut ms = 0u64;
        for _ in 0..5 {
            ms = ms.saturating_add(self.animation_length(anims, trees, anim));
            // A chain beginning with a loop reads just that animation.
            if anim_loops(anims, anim) {
                break;
            }
            anim = next(anims, anim);
            if anim <= 0 || anim_loops(anims, anim) {
                break;
            }
        }
        ms
    }
}

fn next(anims: &Table, anim: i64) -> i64 {
    match anims.cell_by_hash(COL_ANIM_NEXT, anim) {
        Some(Cell::Int(id)) => *id,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::combat::AbilityAnimLen;
    use crate::sim::ScriptHost;
    use crate::world::{EngineData, ObjectKind};
    use std::sync::Arc;
    use thedas_script::ldf::EngineTable;
    use thedas_script::vm::Value;
    use thedas_vfs::{Game, Vfs};

    fn call(s: &mut Sim, name: &str, args: &[Value]) -> Value {
        let f = s.table.function_by_name(name).unwrap().clone();
        let mut host = ScriptHost {
            sim: s,
            event: None,
            script: "test".into(),
        };
        crate::engine::call(&mut host, 0, &f, args)
            .unwrap()
            .unwrap()
            .unwrap()
    }

    fn command(s: &mut Sim, anim: i32, loops: i32, next: i32) -> Command {
        let Value::Engine { handle, .. } = call(
            s,
            "CommandPlayAnimation",
            &[
                Value::Int(anim),
                Value::Int(loops),
                Value::Int(next),
                Value::Int(1),
            ],
        ) else {
            panic!("command handle")
        };
        let Some(EngineData::Command(c)) = s.world.handle(handle) else {
            panic!("command")
        };
        c.clone()
    }

    fn fixture() -> (Sim, ObjectId) {
        let ldf = b"#define ENGINE_STRUCTURE_2 command\n\
            #define ENGINE_STRUCTURE_0 event\n\
            int COMMAND_TYPE_PLAY_ANIMATION = 14;\n\
            int EVENT_TYPE_COMMAND_COMPLETE = 36;\n\
            command CommandPlayAnimation(int a, int loops = 0, int next = 0, int blend = 1) = 569;\n\
            command SetCommandFloat(command c, float v, int i = 0) = 547;\n\
            float GetCommandFloat(command c, int i = 0) = 546;\n\
            event GetCurrentEvent() = 108;\n\
            int GetEventInteger(event e, int i) = 109;\n\
            void SetLocalInt(object o, string s, int n) = 64;\n";
        let mut s = Sim::new(
            Arc::new(Vfs::new()),
            Arc::new(EngineTable::parse(ldf).unwrap()),
            7,
        );
        s.world.trace.keep = true;
        let tables = crate::m2da::Tables::empty();
        tables.insert(
            "#0",
            &["ID", "#7a398af9", "#12a02374", "#ffc372e7"],
            [
                (1, "pause", 1, 1),
                (10, "enter", 0, 11),
                (11, "loop", 1, 12),
                (12, "exit", 0, 13),
                (13, "end", 0, 1),
                (14, "exit", 0, 13),
                (17, "end", 0, 17),
                (18, "enter", 0, 999),
                (19, "absent", 0, 1),
            ]
            .into_iter()
            .map(|(id, action, flags, next)| {
                (
                    id,
                    vec![
                        Cell::Int(id),
                        Cell::String(action.into()),
                        Cell::Int(flags),
                        Cell::Int(next),
                    ],
                )
            })
            .collect(),
        );
        tables.insert(
            "#2",
            &["ID", "#500976ec", "#de80b418"],
            vec![
                (
                    1,
                    vec![
                        Cell::Int(1),
                        Cell::String("Emotes.ANB\0".into()),
                        Cell::String("combat.anb".into()),
                    ],
                ),
                (
                    2,
                    vec![
                        Cell::Int(2),
                        Cell::String("other.anb".into()),
                        Cell::String("combat.anb".into()),
                    ],
                ),
            ],
        );
        s.world.tables = Arc::new(tables);
        for (action, raw_ms) in [
            ("pause", 1000),
            ("enter", 1200),
            ("loop", 2400),
            ("exit", 600),
            ("end", 300),
        ] {
            s.ability_anims.insert(
                format!("emotes.anb|{action}"),
                Some(AbilityAnimLen {
                    raw_ms,
                    len_ms: raw_ms / 4,
                    impact_ms: None,
                }),
            );
        }
        s.ability_anims.insert(
            "combat.anb|enter".into(),
            Some(AbilityAnimLen {
                raw_ms: 50,
                len_ms: 50,
                impact_ms: None,
            }),
        );
        let creature = s.world.spawn(ObjectKind::Creature, "actor");
        s.world.get_mut(creature).unwrap().appearance = 1;
        (s, creature)
    }

    #[test]
    fn scripted_animation_durations_follow_transitions_and_loop_counts() {
        let (mut s, c) = fixture();
        for (anim, loops, next, expected) in [
            (10, 0, 0, 1200),
            (10, 0, 1, 2100),
            (10, 2, 0, 1200),
            (10, 2, 1, 6900),
            (11, 0, 0, 0),
            (11, 0, 1, 900),
            (11, 2, 0, 4800),
            (11, 2, 1, 5700),
            (12, 0, 0, 900),
            (14, 0, 1, 900),
            (17, 0, 0, 1500),
            (18, 0, 0, 1200),
            (19, 0, 0, 0),
            (10, -1, 0, 1200),
            (11, -1, 1, 0),
        ] {
            let cmd = command(&mut s, anim, loops, next);
            assert_eq!(
                s.play_animation_duration(c, &cmd),
                Some(expected),
                "{anim}/{loops}/{next}"
            );
        }
        for anim in [0, -1, 999] {
            let cmd = command(&mut s, anim, 0, 0);
            assert_eq!(s.play_animation_duration(c, &cmd), None);
        }
    }

    #[test]
    fn animation_tree_comes_from_the_current_16_bit_appearance() {
        let (mut s, c) = fixture();
        let cmd = command(&mut s, 10, 0, 0);
        assert_eq!(s.play_animation_duration(c, &cmd), Some(1200));
        s.world.get_mut(c).unwrap().appearance = 65537;
        assert_eq!(s.play_animation_duration(c, &cmd), Some(1200));
        s.world.get_mut(c).unwrap().appearance = 2;
        assert_eq!(
            s.play_animation_duration(c, &cmd),
            Some(50),
            "combat action fallback"
        );
    }

    #[test]
    fn animation_speed_is_native_float_three_and_scales_the_initial_stage() {
        let (mut s, c) = fixture();
        let cmd = command(&mut s, 10, 1, 1);
        let handle = s.world.store(EngineData::Command(cmd));
        let value = Value::Engine {
            ty: s.types.command,
            handle: Some(handle),
        };
        let changed = call(
            &mut s,
            "SetCommandFloat",
            &[value.clone(), Value::Float(2.0), Value::Int(3)],
        );
        assert_eq!(
            call(&mut s, "GetCommandFloat", &[changed.clone(), Value::Int(3)]),
            Value::Float(2.0)
        );
        assert_eq!(
            call(&mut s, "GetCommandFloat", &[changed.clone(), Value::Int(0)]),
            Value::Float(0.0)
        );
        assert_eq!(
            call(&mut s, "GetCommandFloat", &[value, Value::Int(3)]),
            Value::Float(0.0)
        );
        let Value::Engine { handle, .. } = changed else {
            panic!("handle")
        };
        let Some(EngineData::Command(cmd)) = s.world.handle(handle) else {
            panic!("command")
        };
        let mut cmd = cmd.clone();
        assert_eq!(s.play_animation_duration(c, &cmd), Some(600 + 2400 + 900));
        for speed in [0.0, -1.0, f32::NAN] {
            cmd.seconds = speed;
            assert_eq!(s.play_animation_duration(c, &cmd), Some(1200 + 2400 + 900));
        }
    }

    #[test]
    fn finite_completion_and_continuing_animation_survive_reload() {
        let (mut s, c) = fixture();
        // An authored synthetic VM handler records COMMAND_COMPLETE's result in a local.
        let mut code = vec![
            0x04, 0x03, 0, 0, 0, 1, // integer index 1
            0x05, 0, 0, 108, 0, // GetCurrentEvent()
            0x05, 0, 0, 109, 2, // GetEventInteger(event, 1)
            0x04, 0x05, 0, 6,
        ];
        code.extend(b"result");
        code.extend([
            0x04, 0x06, 0, 0, 0, 0, // OBJECT_SELF
            0x05, 0, 0, 64, 3, 0x20, 0,
        ]); // SetLocalInt, return
        let mut ncs = b"NCS V1.0B".to_vec();
        ncs.extend(((13 + code.len()) as u32).to_be_bytes());
        ncs.extend(code);
        s.add_script("animation_result", thedas_script::Ncs::parse(&ncs).unwrap());
        s.world.get_mut(c).unwrap().script = Some("animation_result".into());
        let cmd = command(&mut s, 10, 1, 1);
        s.world.get_mut(c).unwrap().commands.push(cmd);
        s.tick(2000);
        let saved = serde_json::to_string(&s.world).unwrap();
        let tables = s.world.tables.clone();
        s.world = serde_json::from_str(&saved).unwrap();
        s.world.tables = tables;
        let cmd = &s.world.get(c).unwrap().commands[0];
        assert_eq!(cmd.due, Some(4500));
        assert_eq!(cmd.from, Some(([0.0; 3], 0)));
        s.tick(2499);
        assert_eq!(s.world.get(c).unwrap().commands.len(), 1);
        s.tick(1);
        assert!(s.world.get(c).unwrap().commands.is_empty());
        assert_eq!(s.world.get(c).unwrap().locals.ints["result"], 1);
        let cmd = command(&mut s, 10, -1, 1);
        s.world.get_mut(c).unwrap().commands.push(cmd);
        s.tick(1200);
        let actor = s.world.get(c).unwrap();
        assert!(actor.commands.is_empty());
        assert_eq!(actor.looping_animation.as_ref().unwrap().ints[0], 11);
        assert_eq!(actor.locals.ints["result"], 2);
        let saved = serde_json::to_string(&s.world).unwrap();
        let tables = s.world.tables.clone();
        s.world = serde_json::from_str(&saved).unwrap();
        s.world.tables = tables;
        s.tick(100_000);
        assert!(s.world.get(c).unwrap().commands.is_empty());
        assert_eq!(
            s.world
                .get(c)
                .unwrap()
                .looping_animation
                .as_ref()
                .unwrap()
                .ints[0],
            11
        );
        let mut wait = command(&mut s, 19, 0, 0);
        wait.name = "CommandWait".into();
        wait.seconds = 0.1;
        s.world.get_mut(c).unwrap().commands.push(wait);
        s.tick(100);
        assert!(s.world.get(c).unwrap().commands.is_empty());
        assert!(s.world.get(c).unwrap().looping_animation.is_some());
        let cmd = command(&mut s, 19, 0, 0);
        s.world.get_mut(c).unwrap().commands.push(cmd);
        s.tick(1);
        assert!(s.world.get(c).unwrap().looping_animation.is_none());
        assert!(
            s.world.get(c).unwrap().commands.is_empty(),
            "missing action completes at minimum tick"
        );
    }

    #[test]
    fn da2_retains_its_previous_animation_duration() {
        let (mut s, c) = fixture();
        s.game = Game::DragonAge2;
        let cmd = command(&mut s, 11, -1, 1);
        s.world.get_mut(c).unwrap().commands.push(cmd);
        s.tick(499);
        assert_eq!(s.world.get(c).unwrap().commands.len(), 1);
        s.tick(1);
        assert!(s.world.get(c).unwrap().commands.is_empty());
    }

    #[test]
    fn animation_duration_reads_the_appearance_tree_and_gad_through_the_shared_cache() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use thedas_formats::gff4::{
            FieldDef, FourCC, Gff4, List, StructData, StructId, StructTemplate, Value as G4,
            Version,
        };
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        struct Assets(std::path::PathBuf);
        impl Drop for Assets {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let assets = Assets(std::env::temp_dir().join(format!(
            "thedas-play-animation-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )));
        std::fs::create_dir_all(&assets.0).unwrap();
        let field = |label, type_code, flags, offset| FieldDef {
            label,
            type_code,
            flags,
            offset,
        };
        let doc = |kind, templates, structs| Gff4 {
            version: Version::V40,
            platform: FourCC::new(b"PC  "),
            file_type: FourCC::new(kind),
            file_version: FourCC::new(b"V1.0"),
            templates,
            structs,
            root: StructId(0),
            shared_strings: None,
        };
        // Authored synthetic assets, with tree speed 2 and .gad speed 2.
        let tree = doc(
            b"ANBT",
            vec![
                StructTemplate {
                    label: FourCC::new(b"tree"),
                    fields: vec![field(4021, 1, 0xc000, 0)],
                    size: 4,
                },
                StructTemplate {
                    label: FourCC::new(b"node"),
                    fields: vec![
                        field(4024, 14, 0, 0),
                        field(4025, 14, 0, 4),
                        field(4026, 8, 0, 8),
                    ],
                    size: 12,
                },
            ],
            vec![
                StructData {
                    template: 0,
                    values: vec![G4::List(Some(List::Struct(vec![Some(StructId(1))])))],
                },
                StructData {
                    template: 1,
                    values: vec![
                        G4::String(Some("enter".into())),
                        G4::String(Some("fixture.ani".into())),
                        G4::F32(2.0),
                    ],
                },
            ],
        );
        let gad = doc(
            b"GAD ",
            vec![StructTemplate {
                label: FourCC::new(b"data"),
                fields: vec![field(4009, 8, 0, 0), field(4010, 8, 0, 4)],
                size: 8,
            }],
            vec![StructData {
                template: 0,
                values: vec![G4::F32(1.25), G4::F32(2.0)],
            }],
        );
        std::fs::write(assets.0.join("emotes.anb"), tree.write().unwrap()).unwrap();
        std::fs::write(assets.0.join("fixture.gad"), gad.write().unwrap()).unwrap();
        let (mut s, c) = fixture();
        let mut vfs = Vfs::new();
        vfs.push_directory(&assets.0, false, "synthetic animation")
            .unwrap();
        s.vfs = Arc::new(vfs);
        s.ability_anims.remove("emotes.anb|enter");
        let cmd = command(&mut s, 10, 0, 0);
        assert_eq!(s.play_animation_duration(c, &cmd), Some(1250));
        let sample = s.ability_anims["emotes.anb|enter"].unwrap();
        assert_eq!((sample.raw_ms, sample.len_ms), (1250, 313));
        // A cache hit still works after the source has gone.
        std::fs::remove_file(assets.0.join("fixture.gad")).unwrap();
        assert_eq!(s.play_animation_duration(c, &cmd), Some(1250));
    }
}
