//! Origins' EQUAL/NEQUAL callbacks. See docs/notes/script-engine-equality.md.

use thedas_script::vm::OBJECT_INVALID;

use crate::sim::ScriptHost;
use crate::world::{Effect, EngineData, Event, Location};

pub(crate) fn equal(h: &ScriptHost<'_>, ty: u8, a: Option<u64>, b: Option<u64>) -> bool {
    if h.sim.game == thedas_vfs::Game::DragonAge2 {
        return a == b;
    }
    let types = &h.sim.types;
    // None represents the VM's default engine value (RSADD and missing returns),
    // not a native null pointer. Native RSADD allocates the corresponding default
    // structure. A stale/nonmatching populated handle must not become a default.
    let default = if ty == types.event {
        EngineData::Event(Event::default())
    } else if ty == types.location {
        EngineData::Location(Location {
            area: OBJECT_INVALID,
            position: [0.0; 3],
            facing: 0.0,
        })
    } else if ty == types.effect {
        EngineData::Effect(Effect::default())
    } else {
        // Command and unmodelled types retain their existing behavior.
        return a == b;
    };
    let resolve = |handle| match handle {
        None => Some(&default),
        Some(_) => h.sim.world.handle(handle),
    };
    match (&default, resolve(a), resolve(b)) {
        (EngineData::Event(_), Some(EngineData::Event(a)), Some(EngineData::Event(b))) => {
            event_equal(a, b)
        }
        (EngineData::Location(_), Some(EngineData::Location(a)), Some(EngineData::Location(b))) => {
            a == b
        }
        (EngineData::Effect(_), Some(EngineData::Effect(a)), Some(EngineData::Effect(b))) => {
            a.id == b.id
        }
        _ => false,
    }
}

// Native 0xa5a3a0 checks allocation presence, then compares elements only when
// counts match. Two populated lists of different lengths skip element comparison.
// Our vectors model an unallocated list as empty (setters only grow them).
fn parameters_equal<T>(a: &[T], b: &[T], eq: impl Fn(&T, &T) -> bool) -> bool {
    a.is_empty() == b.is_empty() && (a.len() != b.len() || a.iter().zip(b).all(|(a, b)| eq(a, b)))
}

fn event_equal(a: &Event, b: &Event) -> bool {
    a.ty == b.ty
        && a.creator.unwrap_or(OBJECT_INVALID) == b.creator.unwrap_or(OBJECT_INVALID)
        && a.target.unwrap_or(OBJECT_INVALID) == b.target.unwrap_or(OBJECT_INVALID)
        && parameters_equal(&a.ints, &b.ints, |a, b| a == b)
        && parameters_equal(&a.floats, &b.floats, |a, b| a == b)
        && parameters_equal(&a.strings, &b.strings, |a, b| a == b)
        && parameters_equal(&a.objects, &b.objects, |a, b| a == b)
        && parameters_equal(&a.resources, &b.resources, |a, b| {
            a.eq_ignore_ascii_case(b)
        })
        // Origins SetEventLocation stores only a position vector (0x6585f0).
        && parameters_equal(&a.locations, &b.locations, |a, b| a.position == b.position)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use thedas_script::vm::{Host, Value, Vm};
    use thedas_script::{EngineTable, Ncs};
    use thedas_vfs::{Game, Vfs};

    use super::*;
    use crate::sim::Sim;
    use crate::world::ObjectKind;

    fn sim() -> Sim {
        let table = EngineTable::parse(b"#define ENGINE_STRUCTURE_0 event\n#define ENGINE_STRUCTURE_1 location\n#define ENGINE_STRUCTURE_2 command\n#define ENGINE_STRUCTURE_3 effect\nevent GetCurrentEvent() = 0;\nevent SetEventInteger(event e, int index, int value) = 1;\nlocation GetLocation(object o) = 2;\neffect GetCurrentEffect() = 3;\n").unwrap();
        Sim::new(Arc::new(Vfs::new()), Arc::new(table), 1)
    }

    fn compare(s: &mut Sim, ty: u8, a: Option<u64>, b: Option<u64>) -> bool {
        ScriptHost {
            sim: s,
            event: None,
            script: "test".into(),
        }
        .engine_equal(ty, a, b)
    }

    fn values(s: &mut Sim, ty: u8, a: EngineData, b: EngineData) -> bool {
        let a = Some(s.world.store(a));
        let b = Some(s.world.store(b));
        compare(s, ty, a, b)
    }

    fn location() -> Location {
        Location {
            area: 12,
            position: [1.0, 2.0, 3.0],
            facing: 90.0,
        }
    }

    #[test]
    fn origins_effect_equality_compares_persistent_identity() {
        let mut s = sim();
        let a = Effect {
            ty: 1012,
            id: 42,
            ..Default::default()
        };
        let mut b = a.clone();
        b.ints = vec![9];
        b.floats = vec![f32::NAN];
        b.creator = Some(7);
        b.expires = Some(1000);
        b.flags = 1;
        assert!(values(
            &mut s,
            3,
            EngineData::Effect(a.clone()),
            EngineData::Effect(b)
        ));
        let mut b = a.clone();
        b.id += 1;
        assert!(!values(
            &mut s,
            3,
            EngineData::Effect(a),
            EngineData::Effect(b)
        ));
        // Even independently made invalid effects have the native zero identity.
        assert!(values(
            &mut s,
            3,
            EngineData::Effect(Effect::default()),
            EngineData::Effect(Effect::default())
        ));
    }

    #[test]
    fn origins_location_equality_compares_every_represented_field_exactly() {
        let mut s = sim();
        let a = location();
        assert!(values(
            &mut s,
            1,
            EngineData::Location(a),
            EngineData::Location(a)
        ));
        for field in 0..5 {
            let mut b = a;
            match field {
                0 => b.area += 1,
                1..=3 => {
                    b.position[field - 1] = f32::from_bits(b.position[field - 1].to_bits() + 1)
                }
                _ => b.facing += 360.0,
            }
            assert!(
                !values(&mut s, 1, EngineData::Location(a), EngineData::Location(b)),
                "field {field}"
            );
        }
        let mut a = a;
        a.facing = -0.0;
        let mut b = a;
        b.facing = 0.0;
        assert!(values(
            &mut s,
            1,
            EngineData::Location(a),
            EngineData::Location(b)
        ));
        a.position[2] = f32::NAN;
        let handle = Some(s.world.store(EngineData::Location(a)));
        assert!(!compare(&mut s, 1, handle, handle));
    }

    fn event() -> Event {
        Event {
            ty: 1000,
            creator: Some(7),
            target: Some(8),
            ints: vec![1],
            floats: vec![2.0],
            strings: vec!["text".into()],
            objects: vec![9],
            resources: vec!["res".into()],
            locations: vec![location()],
            ..Default::default()
        }
    }

    #[test]
    fn origins_event_equality_compares_headers_and_each_parameter_kind() {
        let mut s = sim();
        let a = event();
        assert!(values(
            &mut s,
            0,
            EngineData::Event(a.clone()),
            EngineData::Event(a.clone())
        ));
        for field in 0..9 {
            let mut b = a.clone();
            match field {
                0 => b.ty += 1,
                1 => b.creator = Some(99),
                2 => b.target = Some(99),
                3 => b.ints[0] += 1,
                4 => b.floats[0] += 1.0,
                5 => b.strings[0] = "Text".into(),
                6 => b.objects[0] += 1,
                7 => b.resources[0] = "other".into(),
                _ => b.locations[0].position[0] += 1.0,
            }
            assert!(
                !values(
                    &mut s,
                    0,
                    EngineData::Event(a.clone()),
                    EngineData::Event(b)
                ),
                "field {field}"
            );
        }
        let mut b = a.clone();
        b.resources[0] = "RES".into();
        b.locations[0].area += 1;
        b.locations[0].facing += 1.0;
        assert!(values(
            &mut s,
            0,
            EngineData::Event(a),
            EngineData::Event(b)
        ));
        let mut a = event();
        a.floats[0] = f32::NAN;
        let handle = Some(s.world.store(EngineData::Event(a)));
        assert!(!compare(&mut s, 0, handle, handle));
    }

    #[test]
    fn origins_event_equality_keeps_native_parameter_count_behavior() {
        let mut s = sim();
        let a = event();
        for field in 0..6 {
            let mut b = a.clone();
            match field {
                0 => b.ints = vec![99, 99],
                1 => b.floats = vec![99.0, 99.0],
                2 => b.strings = vec!["other".into(); 2],
                3 => b.objects = vec![99, 99],
                4 => b.resources = vec!["other".into(); 2],
                _ => {
                    b.locations = vec![
                        Location {
                            position: [99.0; 3],
                            ..location()
                        };
                        2
                    ]
                }
            }
            assert!(
                values(
                    &mut s,
                    0,
                    EngineData::Event(a.clone()),
                    EngineData::Event(b)
                ),
                "populated counts {field}"
            );
            let mut b = a.clone();
            match field {
                0 => b.ints.clear(),
                1 => b.floats.clear(),
                2 => b.strings.clear(),
                3 => b.objects.clear(),
                4 => b.resources.clear(),
                _ => b.locations.clear(),
            }
            assert!(
                !values(
                    &mut s,
                    0,
                    EngineData::Event(a.clone()),
                    EngineData::Event(b)
                ),
                "missing list {field}"
            );
        }
    }

    #[test]
    fn engine_equality_distinguishes_defaults_stale_handles_and_types() {
        let mut s = sim();
        for (ty, default) in [
            (0, EngineData::Event(Event::default())),
            (
                1,
                EngineData::Location(Location {
                    area: OBJECT_INVALID,
                    position: [0.0; 3],
                    facing: 0.0,
                }),
            ),
            (3, EngineData::Effect(Effect::default())),
        ] {
            let handle = Some(s.world.store(default));
            assert!(compare(&mut s, ty, None, None));
            assert!(compare(&mut s, ty, None, handle));
            assert!(compare(&mut s, ty, handle, None));
            assert!(!compare(&mut s, ty, Some(u64::MAX), Some(u64::MAX)));
            assert!(!compare(&mut s, ty, None, Some(u64::MAX)));
        }
        let event = Some(s.world.store(EngineData::Event(Event::default())));
        assert!(!compare(&mut s, 1, event, event));
        assert!(!compare(&mut s, 3, event, event));
        assert!(compare(&mut s, 2, Some(1), Some(1)));
        assert!(!compare(&mut s, 2, Some(1), Some(2)));
        let a = Event::default();
        let b = Event {
            creator: Some(OBJECT_INVALID),
            target: Some(OBJECT_INVALID),
            ..Default::default()
        };
        assert!(values(
            &mut s,
            0,
            EngineData::Event(a),
            EngineData::Event(b)
        ));
    }

    #[test]
    fn da2_engine_equality_retains_handle_comparisons() {
        let mut s = sim();
        s.game = Game::DragonAge2;
        for ty in [0, 1, 2, 3, 5] {
            assert!(compare(&mut s, ty, None, None));
            assert!(compare(&mut s, ty, Some(1), Some(1)));
            assert!(!compare(&mut s, ty, Some(1), Some(2)));
            assert!(!compare(&mut s, ty, None, Some(1)));
        }
    }

    #[test]
    fn origins_vm_compares_fresh_event_location_and_effect_handles() {
        let mut s = sim();
        let caller = s.world.spawn(ObjectKind::Creature, "caller");
        let event = Some(s.world.store(EngineData::Event(Event {
            ints: vec![7],
            ..Default::default()
        })));
        s.current_effect = Some((
            Effect {
                id: 42,
                ty: 1012,
                ..Default::default()
            },
            true,
        ));
        // Own NCS fixtures: compare an event with its no-op setter's copy, two
        // GetLocation calls, and two GetCurrentEffect calls. Each pair differs
        // in host handle and would fail before the host comparison hook.
        let pairs = [
            (
                0,
                vec![
                    0x04, 3, 0, 0, 0, 7, 0x04, 3, 0, 0, 0, 0, 0x05, 0, 0, 0, 0, 0x05, 0, 0, 1, 3,
                    0x05, 0, 0, 0, 0,
                ],
            ),
            (
                1,
                vec![
                    0x04, 6, 0, 0, 0, 0, 0x05, 0, 0, 2, 1, 0x04, 6, 0, 0, 0, 0, 0x05, 0, 0, 2, 1,
                ],
            ),
            (3, vec![0x05, 0, 0, 3, 0, 0x05, 0, 0, 3, 0]),
        ];
        for (ty, pair) in pairs {
            for (op, expected) in [(0x0b, 1), (0x0c, 0)] {
                for structure in [false, true] {
                    let mut body = pair.clone();
                    body.extend([op, if structure { 0x24 } else { 0x30 + ty }]);
                    if structure {
                        body.extend(4_u16.to_be_bytes());
                    }
                    body.extend([0x20, 0]);
                    let mut bytes = b"NCS V1.0B".to_vec();
                    bytes.extend(((13 + body.len()) as u32).to_be_bytes());
                    bytes.extend(body);
                    let vm = Vm::new(Arc::new(Ncs::parse(&bytes).unwrap()), s.table.clone());
                    let mut host = ScriptHost {
                        sim: &mut s,
                        event,
                        script: "compare".into(),
                    };
                    assert_eq!(
                        vm.run(&mut host, caller).unwrap().value,
                        Some(Value::Int(expected)),
                        "type {ty}, opcode {op}, struct {structure}"
                    );
                }
            }
        }
    }
}
