//! Authored loose resources exercise PWK lookup and nav reload without an install.

use super::*;
use crate::engine::dist;
use crate::nav::Routed;
use crate::trace::TraceEntry;
use crate::world::Command;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use thedas_formats::gff3;
use thedas_formats::gff4::{
    FieldDef, FourCC, Gff4, List, StructData, StructId, StructTemplate, Value, Version,
};
use thedas_formats::level::field::*;
use thedas_script::EngineTable;
use thedas_vfs::Vfs;

const START: [f32; 3] = [1.75, 3.75, 0.0];
const ORIGIN: [f32; 3] = [3.5, 3.5, 0.0];
const SCRIPT: &str = "synthetic_pwk_lever";

// Only the field shapes needed by these two navigation documents.
fn document(kind: &[u8; 4], records: [Vec<(u32, Value)>; 3]) -> Vec<u8> {
    let mut doc = Gff4 {
        version: Version::V40,
        platform: FourCC(*b"PC  "),
        file_type: FourCC(*kind),
        file_version: FourCC(*b"V0.1"),
        templates: Vec::new(),
        structs: Vec::new(),
        root: StructId(0),
        shared_strings: None,
    };
    for (i, record) in records.into_iter().enumerate() {
        let mut size = 0;
        let fields = record
            .iter()
            .map(|(label, value)| {
                let (type_code, flags, width) = match value {
                    Value::U32(_) => (4, 0, 4),
                    Value::F32(_) => (8, 0, 4),
                    Value::Vec3(_) => (10, 0, 12),
                    Value::List(Some(List::U8(_))) => (0, 0x8000, 4),
                    Value::List(Some(List::Struct(_))) => (1, 0xe000, 4),
                    Value::Struct(Some(id)) => (id.0 as u16, 0x6000, 4),
                    _ => unreachable!(),
                };
                let offset = size;
                size += width;
                FieldDef {
                    label: *label,
                    type_code,
                    flags,
                    offset,
                }
            })
            .collect();
        doc.templates.push(StructTemplate {
            label: FourCC(*b"test"),
            fields,
            size,
        });
        doc.structs.push(StructData {
            template: i as u32,
            values: record.into_iter().map(|(_, value)| value).collect(),
        });
    }
    doc.write().unwrap()
}

struct Assets(PathBuf);

impl Assets {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "thedas-pwk-use-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        let assets = Self(path);
        let are = gff3::Gff3 {
            file_type: *b"ARE ",
            file_version: *b"V3.2",
            structs: vec![gff3::Struct {
                id: u32::MAX,
                fields: vec![gff3::Field {
                    label: "Layout".into(),
                    value: gff3::Value::ResRef(b"fixture_pwk".to_vec()),
                }],
            }],
        };
        std::fs::write(assets.0.join("fixture_pwk.are"), are.write().unwrap()).unwrap();
        let grid = |columns, rows, size, base, data| {
            vec![
                (GRID_COLUMNS, Value::U32(columns)),
                (GRID_ROWS, Value::U32(rows)),
                (GRID_CELL_SIZE, Value::F32(size)),
                (GRID_BASE, Value::Vec3(base)),
                (GRID_DATA, Value::List(Some(List::U8(data)))),
            ]
        };
        let grid_ref = || vec![(PATHING_GRID, Value::Struct(Some(StructId(2))))];
        std::fs::write(
            assets.0.join("fixture_lever_1.pwk"),
            document(
                b"PWK ",
                [
                    vec![(
                        3016,
                        Value::List(Some(List::Struct(vec![Some(StructId(1))]))),
                    )],
                    grid_ref(),
                    grid(1, 1, 0.25, [-0.125, -0.125, 0.0], vec![1]),
                ],
            ),
        )
        .unwrap();
        // The requested primary stop is in (4,7). The three cells below it are
        // closed, so repair picks the west neighbor (our current cell center).
        // There is a route around its north side to the nearer band of the PWK.
        let mut cells = vec![1; 100];
        for i in [63, 64, 65, 74] {
            cells[i] = 0;
        }
        std::fs::write(
            assets.0.join("fixture_pwk.arl"),
            document(
                b"ARL ",
                [
                    vec![(ARL_PATHING, Value::Struct(Some(StructId(1))))],
                    grid_ref(),
                    grid(10, 10, 0.5, [0.0; 3], cells),
                ],
            ),
        )
        .unwrap();
        assets
    }

    fn open(&self) -> Sim {
        let mut vfs = Vfs::new();
        vfs.push_directory(&self.0, false, "synthetic PWK use")
            .unwrap();
        let table = EngineTable::parse(b"int EVENT_TYPE_USE = 14;\nint COMMAND_TYPE_USE_OBJECT = 31;\nint COMMAND_FAILED_INVALID_PATH = -4;\n").unwrap();
        let mut sim = Sim::new(Arc::new(vfs), Arc::new(table), 7);
        sim.world.trace.keep = true;
        let tables = crate::m2da::Tables::empty();
        tables.insert(
            "#93",
            &["#fd8403d5"],
            vec![(1, vec![Cell::String("fixture_lever".into())])],
        );
        sim.world.tables = Arc::new(tables);
        sim
    }

    fn ordered(&self) -> (Sim, ObjectId, ObjectId) {
        let mut sim = self.open();
        let area = sim.world.spawn(ObjectKind::Area, "room");
        sim.world.area = Some(area);
        sim.world.get_mut(area).unwrap().template = "fixture_pwk".into();
        let actor = sim.world.spawn(ObjectKind::Creature, "walker");
        let o = sim.world.get_mut(actor).unwrap();
        o.area = Some(area);
        o.position = START;
        let target = sim.world.spawn(ObjectKind::Placeable, "lever");
        let o = sim.world.get_mut(target).unwrap();
        o.area = Some(area);
        o.position = ORIGIN;
        o.appearance = 1;
        o.script = Some(SCRIPT.into());
        assert_eq!(
            sim.placeable_use_point(actor, target),
            Some([3.375, 3.625, 0.0])
        );
        let (_, stop) = sim.placeable_use_walk(actor, target).unwrap();
        assert!(dist(START, stop) > 0.05);
        assert_eq!(
            sim.route_and_approach(actor, stop).0,
            Routed::Path(vec![START])
        );
        sim.order_use(actor, target);
        assert!(!front(&sim, actor).arrived);
        (sim, actor, target)
    }
}

impl Drop for Assets {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn front(sim: &Sim, actor: ObjectId) -> &Command {
    &sim.world.get(actor).unwrap().commands[0]
}

fn feet(sim: &Sim, actor: ObjectId) -> [f32; 3] {
    sim.world.get(actor).unwrap().position
}

fn advance_to(sim: &mut Sim, time: u64) {
    assert!(time >= sim.world.time_ms);
    while sim.world.time_ms < time {
        sim.tick((time - sim.world.time_ms).min(20));
    }
}

fn outcomes(sim: &Sim, actor: ObjectId, target: ObjectId) -> (Vec<u64>, Vec<(u64, &str)>) {
    let mut uses = Vec::new();
    let mut terminal = Vec::new();
    for e in &sim.world.trace.entries {
        match e {
            TraceEntry::EventDelivered {
                t,
                target: id,
                ty: 14,
                script,
                ..
            } if *id == target => {
                assert_eq!(script, SCRIPT);
                uses.push(*t);
            }
            TraceEntry::Command {
                t,
                object,
                command,
                status,
            } if *object == actor
                && command == "CommandUseObject"
                && (status == "complete" || status.starts_with("failed")) =>
            {
                terminal.push((*t, status.as_str()));
            }
            _ => {}
        }
    }
    (uses, terminal)
}

fn marked_walk(sim: &mut Sim, actor: ObjectId) -> (u64, [f32; 3]) {
    sim.tick(1);
    let c = front(sim, actor);
    assert!(c.arrived && c.from == Some((START, 0)));
    let goal = *c.path.last().unwrap();
    assert_eq!(c.approach, Some(goal));
    assert!(dist(START, goal) > 1.0 && dist(ORIGIN, goal) < 1.0);
    (c.due.unwrap(), goal)
}

#[test]
fn pwk_use_walk_and_timer_resume() {
    let assets = Assets::new();
    let (sim, actor, target) = assets.ordered();
    // A legacy save before the command starts must deserialize unmarked.
    let mut old: serde_json::Value = serde_json::from_slice(&sim.save_json().unwrap()).unwrap();
    let c = old["world"]["objects"][actor.to_string()]["commands"][0]
        .as_object_mut()
        .unwrap();
    assert_eq!(c.remove("arrived"), Some(serde_json::Value::Bool(false)));
    let mut sim = assets.open();
    sim.load_json(&serde_json::to_vec(&old).unwrap()).unwrap();
    assert!(!front(&sim, actor).arrived);
    let (end, goal) = marked_walk(&mut sim, actor);
    let route = front(&sim, actor).clone();
    advance_to(&mut sim, end / 2);
    assert!(dist(START, feet(&sim, actor)) > 0.1 && dist(goal, feet(&sim, actor)) > 0.1);
    assert_eq!(front(&sim, actor).from, route.from);
    assert_eq!(front(&sim, actor).due, route.due);
    assert_eq!(front(&sim, actor).path, route.path);
    let mut walk = assets.open();
    walk.load_json(&sim.save_json().unwrap()).unwrap();
    assert_eq!(front(&walk, actor), front(&sim, actor));
    assert_eq!(feet(&walk, actor), feet(&sim, actor));
    assert_eq!(walk.world.time_ms, sim.world.time_ms);
    for s in [&mut sim, &mut walk] {
        advance_to(s, end + 1);
        assert!(s.pwk_use_arrived(actor, target, goal));
        assert_eq!(outcomes(s, actor, target), (vec![], vec![]));
    }
    let c = front(&sim, actor);
    assert!(c.arrived && c.from.is_none() && c.approach.is_none());
    assert_eq!(c.path, route.path);
    assert_eq!(front(&walk, actor), c);
    let use_at = c.due.unwrap();
    let mut timer = assets.open();
    timer.load_json(&sim.save_json().unwrap()).unwrap();
    assert_eq!(front(&timer, actor), c);
    assert_eq!(feet(&timer, actor), feet(&sim, actor));
    assert_eq!(timer.world.time_ms, sim.world.time_ms);
    for s in [&mut sim, &mut walk, &mut timer] {
        advance_to(s, use_at + 500);
        assert_eq!(
            outcomes(s, actor, target),
            (vec![use_at], vec![(use_at, "complete")])
        );
        assert!(s.pwk_use_arrived(actor, target, goal));
        assert!(s.world.get(actor).unwrap().commands.is_empty());
    }
}

#[test]
fn pwk_use_obstruction_and_replacement() {
    let assets = Assets::new();
    let (mut sim, actor, target) = assets.ordered();
    let (end, goal) = marked_walk(&mut sim, actor);
    let route = front(&sim, actor).clone();
    let mut replaced = assets.open();
    replaced.load_json(&sim.save_json().unwrap()).unwrap();
    replaced.order_use(actor, target);
    let new_order = front(&replaced, actor);
    assert!(!new_order.arrived && new_order.from.is_none() && new_order.due.is_none());
    assert!(new_order.path.is_empty());

    // The body arrives after path selection, without replacing the PWK or nav.
    let blocker = sim.world.spawn(ObjectKind::Creature, "blocker");
    let area = sim.world.area;
    let o = sim.world.get_mut(blocker).unwrap();
    o.area = area;
    o.position = goal;
    advance_to(&mut sim, end - 1);
    let c = front(&sim, actor);
    assert!(c.arrived);
    assert_eq!(
        (c.from, c.due, c.approach),
        (route.from, route.due, route.approach)
    );
    assert_eq!(c.path, route.path);
    let stopped = feet(&sim, actor);
    assert!(dist(START, stopped) > 0.1 && dist(goal, stopped) > 0.05);
    assert_eq!(feet(&sim, blocker), goal);
    assert!(sim.alternate_use_footprint(actor, target).is_some());
    assert_eq!(outcomes(&sim, actor, target), (vec![], vec![]));
    advance_to(&mut sim, end + 500);
    assert_eq!(
        outcomes(&sim, actor, target),
        (vec![], vec![(end, "failed -4")])
    );
    assert!(sim.world.get(actor).unwrap().commands.is_empty());
    assert!(dist(goal, feet(&sim, actor)) > 0.05);
}
