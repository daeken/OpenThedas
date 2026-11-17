//! Local, read-only-save diagnostics. Game files and replay outputs stay outside the repo.
use std::error::Error;
use std::io::Write;
use std::path::PathBuf;

use thedas_formats::gda::Cell;
use thedas_sim::commands::InputEntry;
use thedas_sim::replay::JournalLine;
use thedas_sim::session::{SessionOptions, open};
use thedas_sim::sim::{ChargenPolicy, CutscenePolicy};
use thedas_sim::world::{Command, ObjectKind};
use thedas_sim::{Sim, collision::Collision};

type Point = [f32; 3];

fn distance(a: Point, b: Point) -> f32 {
    a.iter()
        .zip(b)
        .map(|(a, b)| (a - b).powi(2))
        .sum::<f32>()
        .sqrt()
}

fn nearby(g: &thedas_formats::level::NavGrid, p: Point, reach: f32) -> Vec<Point> {
    let mut points = Vec::new();
    for y in 0..g.rows {
        for x in 0..g.columns {
            let m = g.center((x, y));
            if (m[0] - p[0]).abs() <= reach
                && (m[1] - p[1]).abs() <= reach
                && thedas_sim::nav::walkable(g, (x, y))
            {
                points.push([m[0], m[1], thedas_sim::nav::height(g, (x, y))]);
            }
        }
    }
    points
}

fn across(leaf: &Collision, points: &[Point], centre: Point) -> Option<(Point, Point)> {
    let eye = |p: Point| [p[0], p[1], p[2] + 1.0];
    let mut best = None;
    let mut score = f32::INFINITY;
    for &a in points {
        if leaf.blocked(eye(a), [a[0], a[1], a[2] + 1.01]) {
            continue;
        }
        for &b in points {
            let d = distance(a, b);
            let rank = d + distance(a, centre) + distance(b, centre);
            if (3.0..=5.0).contains(&d)
                && (a[2] - b[2]).abs() < 0.5
                && rank < score
                && !leaf.blocked(eye(b), [b[0], b[1], b[2] + 1.01])
                && leaf.blocked(eye(a), eye(b))
            {
                best = Some((a, b));
                score = rank;
            }
        }
    }
    best
}

fn isolate(sim: &mut Sim) {
    sim.world.queue.clear();
    sim.world.game_mode = 0;
    sim.world.transition = None;
    sim.world.cutscene = None;
    sim.world.cutscenes.clear();
    sim.world.controlled = sim.world.hero;
    let hero = sim.world.hero;
    for o in sim.world.objects.values_mut() {
        o.script = None;
        o.commands.clear();
        if o.kind == ObjectKind::Creature {
            o.active = Some(o.id) == hero;
            o.combat = Default::default();
        }
    }
    sim.restore((sim.world.clone(), None));
}

fn crosses_order(sim: &mut Sim, leaf: &Collision, a: Point, b: Point) -> bool {
    let hero = sim.world.hero.unwrap();
    sim.world.get_mut(hero).unwrap().position = a;
    sim.order_move(hero, b);
    sim.tick(0);
    let Some(c) = sim.world.get(hero).unwrap().commands.first() else {
        return false;
    };
    let path = if c.path.is_empty() {
        vec![b]
    } else {
        c.path.clone()
    };
    let mut from = a;
    for to in path {
        if leaf.blocked(
            [from[0], from[1], from[2] + 1.0],
            [to[0], to[1], to[2] + 1.0],
        ) {
            return true;
        }
        from = to;
    }
    false
}

fn capture_save(sim: &Sim, label: &str) -> Result<(), Box<dyn Error>> {
    if let Some(root) = std::env::var_os("DOORS_CAPTURE_DIR") {
        let root = PathBuf::from(root);
        std::fs::create_dir_all(&root)?;
        std::fs::write(root.join(format!("{label}.json")), sim.save_json()?)?;
    }
    Ok(())
}

fn use_fixture(sim: &mut Sim) -> Result<(), Box<dyn Error>> {
    let area = sim.world.area.ok_or("area")?;
    let hero = sim.world.hero.ok_or("hero")?;
    let grid = sim.nav_grid(area).ok_or("grid")?;
    let apr = sim.world.tables().get(0, "apr_base").ok_or("apr_base")?;
    // Pick an actual door whose appearance collides with a large creature row.
    // The test intentionally queries this wrong namespace to identify C's victims.
    let mut case = None;
    for o in sim
        .world
        .objects
        .values()
        .filter(|o| o.active && o.area == Some(area) && sim.is_door(o.id))
    {
        if !matches!(apr.cell("PERSPACE", i64::from(o.appearance)), Some(Cell::Float(r)) if *r > 2.0)
        {
            continue;
        }
        let angle = o.facing.to_radians();
        let normal = [-angle.sin(), angle.cos()];
        let point = nearby(&grid, o.position, 4.0)
            .into_iter()
            .filter(|p| {
                (3.5..3.9).contains(&distance(*p, o.position)) && (p[2] - o.position[2]).abs() < 0.5
            })
            .min_by(|a, b| {
                let rank = |p: Point| {
                    ((p[0] - o.position[0]) * normal[1] - (p[1] - o.position[1]) * normal[0]).abs()
                };
                rank(*a).total_cmp(&rank(*b))
            });
        if let Some(p) = point {
            case = Some((o.id, o.script.clone(), p));
            break;
        }
    }
    let (id, script, p) = case.ok_or("no large-radius door with a nearby walkable point")?;
    isolate(sim);
    let target_position = sim.world.get(id).unwrap().position;
    let actor = sim.world.get_mut(hero).unwrap();
    actor.position = p;
    actor.facing =
        thedas_sim::world::facing_toward(target_position[0] - p[0], target_position[1] - p[1]);
    let o = sim.world.get_mut(id).unwrap();
    o.script = script;
    o.tag = "doors_repro_use".into();
    o.fields.insert("State".into(), 0);
    println!(
        "app USE fixture door={id} appearance={} at={:?} hero={p:?}",
        o.appearance, o.position
    );
    capture_save(sim, "app-use-start")
}

fn checks(sim: &mut Sim) -> Result<(), Box<dyn Error>> {
    let saved = sim.snapshot();
    let area = sim.world.area.ok_or("area")?;
    let hero = sim.world.hero.ok_or("hero")?;
    let g = sim.nav_grid(area).ok_or("grid")?;
    let types = sim.world.tables().get(93, "").ok_or("types")?;
    let mut door_case = None;
    let mut crate_case = None;
    for o in sim
        .world
        .objects
        .values()
        .filter(|o| o.active && o.area == Some(area) && o.kind == ObjectKind::Placeable)
    {
        let Some(Cell::String(model)) = types.cell("ModelName", o.appearance as i64) else {
            continue;
        };
        let door = sim.is_door(o.id) && o.fields.get("State").is_none_or(|s| *s == 0 || *s == 1);
        let container = model.to_ascii_lowercase().contains("box");
        if (!door || door_case.is_some()) && (!container || crate_case.is_some()) {
            continue;
        }
        let leaf = thedas_sim::collision::load_placeables(
            &sim.vfs,
            &[(model.trim_matches('\0').into(), o.position, o.facing)],
        );
        let points = nearby(&g, o.position, 4.0);
        if let Some((a, b)) = across(&leaf, &points, o.position) {
            let case = (o.id, o.tag.clone(), o.position, leaf, a, b);
            if door {
                door_case = Some(case)
            } else {
                crate_case = Some(case)
            }
        }
        if door_case.is_some() && crate_case.is_some() {
            break;
        }
    }
    let mut failed = false;
    if let Some((id, tag, pos, leaf, a, b)) = door_case {
        for state in [0, 1] {
            sim.restore(saved.clone());
            isolate(sim);
            sim.world
                .get_mut(id)
                .unwrap()
                .fields
                .insert("State".into(), state);
            let crossed = crosses_order(sim, &leaf, a, b);
            println!(
                "{} A state={state} {tag}, {a:?} -> {b:?}",
                if crossed { "FAIL" } else { "PASS" }
            );
            failed |= crossed;
            for _ in 0..25 {
                sim.tick(33);
            }
            capture_save(sim, &format!("a-state-{state}"))?;
        }
        sim.restore(saved.clone());
        isolate(sim);
        // Keep the actual door script for USE/state/presentation diagnostics.
        // Other scripts remain isolated so combat and story triggers do not steer it.
        let script = saved.0.get(id).unwrap().script.clone();
        let o = sim.world.get_mut(id).unwrap();
        o.script = script;
        o.fields.insert("State".into(), 0);
        let from = nearby(&g, pos, 8.0)
            .into_iter()
            .filter(|p| (p[2] - a[2]).abs() < 2.0 && distance(*p, pos) > 6.0)
            .min_by(|a, b| distance(*a, pos).total_cmp(&distance(*b, pos)))
            .ok_or("no distant use point")?;
        sim.world.get_mut(hero).unwrap().position = from;
        sim.world.get_mut(hero).unwrap().commands = vec![Command {
            ty: 31,
            name: "CommandUseObject".into(),
            target: Some(id),
            ..Default::default()
        }];
        sim.world.trace.entries.clear();
        sim.tick(500);
        let remote=sim.world.trace.entries.iter().any(|e|matches!(e,thedas_sim::trace::TraceEntry::Command {object,command,status,..} if *object==hero && command=="CommandUseObject" && status=="complete"));
        println!(
            "{} C scripted USE after 500 ms, start={from:?}, now={:?}",
            if remote { "FAIL" } else { "PASS" },
            sim.world.get(hero).unwrap().position
        );
        failed |= remote;
        for _ in 0..600 {
            sim.tick(16);
        }
        let used=sim.world.trace.entries.iter().any(|e|matches!(e,thedas_sim::trace::TraceEntry::Command {object,command,status,..} if *object==hero && command=="CommandUseObject" && status=="complete"));
        println!(
            "{} C USE eventually completes at {:?}",
            if used { "PASS" } else { "FAIL" },
            sim.world.get(hero).unwrap().position
        );
        failed |= !used;
        capture_save(sim, "c-use")?;
        let state = sim.world.get(id).unwrap().fields.get("State").copied();
        let state_label = state.and_then(|state| {
            let table = sim.world.tables().get(0, "statecnt_door")?;
            match table.cell("Label", i64::from(state))? {
                Cell::String(label) => Some(label.trim_matches('\0').to_string()),
                _ => None,
            }
        });
        let opened = state_label
            .as_ref()
            .is_some_and(|s| s.eq_ignore_ascii_case("Open") || s.eq_ignore_ascii_case("Open_2"));
        println!(
            "{} D real door USE leaves State={state:?} {state_label:?}; visual playback must be checked in the app",
            if opened { "PASS" } else { "FAIL" }
        );
        failed |= !opened;
        if opened {
            isolate(sim);
            let follower = sim
                .world
                .get(hero)
                .unwrap()
                .followers
                .first()
                .copied()
                .unwrap_or(hero);
            sim.world.get_mut(hero).unwrap().active = follower == hero;
            let o = sim.world.get_mut(follower).unwrap();
            o.active = true;
            o.position = a;
            sim.order_move(follower, b);
            for _ in 0..250 {
                sim.tick(16);
            }
            let p = sim.world.get(follower).unwrap().position;
            let crossed = distance(p, b) < 0.05;
            println!(
                "{} B follower crosses the opened saved door: {a:?} -> {p:?}",
                if crossed { "PASS" } else { "FAIL" }
            );
            failed |= !crossed;
        }
    } else {
        println!("FAIL A/C no usable door fixture");
        failed = true;
    }
    if let Some((_, tag, _, leaf, a, b)) = crate_case {
        sim.restore(saved.clone());
        isolate(sim);
        let crossed = crosses_order(sim, &leaf, a, b);
        if crossed {
            println!(
                "F diagnostic shapes={:?}, mover={:?}",
                (0..leaf.len().min(4))
                    .map(|i| leaf.shape(i))
                    .collect::<Vec<_>>(),
                sim.world.get(hero).unwrap().commands.first()
            );
        }
        println!(
            "{} F {tag}, {a:?} -> {b:?}",
            if crossed { "FAIL" } else { "PASS" }
        );
        failed |= crossed;
        for _ in 0..8 {
            sim.tick(33);
        }
        capture_save(sim, "f-crate")?;
    }
    sim.restore(saved.clone());
    isolate(sim);
    let here = sim.world.get(hero).unwrap().position;
    let points = nearby(&g, here, 8.0);
    let stairs = points
        .iter()
        .filter_map(|&p| {
            let (x, y) = g.cell(p)?;
            let q = (x + 1, y);
            if q.0 >= g.columns || !thedas_sim::nav::walkable(&g, q) {
                return None;
            }
            let m = g.center(q);
            let b = [m[0], m[1], thedas_sim::nav::height(&g, q)];
            if b[2] - p[2] <= 0.125 || b[2] - p[2] > 0.5 {
                return None;
            }
            let mut end = b;
            for n in 2..=6 {
                let c = (x + n, y);
                if c.0 >= g.columns || !thedas_sim::nav::walkable(&g, c) {
                    break;
                }
                let m = g.center(c);
                let z = thedas_sim::nav::height(&g, c);
                if (z - end[2]).abs() > 0.5 {
                    break;
                }
                end = [m[0], m[1], z];
            }
            (end[2] - p[2] > 0.625).then_some((p, end))
        })
        .min_by(|a, b| distance(a.0, here).total_cmp(&distance(b.0, here)));
    if let Some((a, b)) = stairs {
        sim.world.get_mut(hero).unwrap().position = a;
        sim.order_move(hero, b);
        sim.tick(16);
        let p = sim.world.get(hero).unwrap().position;
        let floor = thedas_sim::nav::height(&g, g.cell(p).unwrap());
        capture_save(sim, "g-first-step")?;
        let mut bad = (p[2] - floor).abs() > 0.01;
        for _ in 0..200 {
            sim.tick(16);
        }
        let end = sim.world.get(hero).unwrap().position;
        bad |= distance(end, b) > 0.05;
        println!(
            "{} G stair flight {a:?} -> {b:?}, after 16 ms {p:?}, floor={floor}, end={end:?}",
            if bad { "FAIL" } else { "PASS" }
        );
        failed |= bad;
        capture_save(sim, "g-stair-flight")?;
    }
    sim.restore(saved.clone());
    isolate(sim);
    let follower = sim.world.get(hero).unwrap().followers.first().copied();
    if let Some(f) = follower {
        let p = sim.world.get(hero).unwrap().position;
        let o = sim.world.get_mut(f).unwrap();
        o.active = true;
        o.dead = false;
        o.commands.clear();
        o.position = [p[0] + 80.0, p[1], p[2]];
        o.fields.insert("FollowPartyLeader".into(), 1);
        sim.world.time_ms += 2000;
        sim.keep_party_together();
        let d = distance(sim.world.get(f).unwrap().position, p);
        println!(
            "{} B unseen follower ends {d:.3} m from leader",
            if d < 4.0 { "PASS" } else { "FAIL" }
        );
        failed |= d >= 4.0;
        capture_save(sim, "b-catch-up")?;
    }
    if failed {
        Err("one or more saved-area checks failed".into())
    } else {
        Ok(())
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let save = args.first().ok_or("usage: doors_probe SAVE [JOURNAL]")?;
    let data = std::fs::read(save)?;
    let saved: thedas_sim::save::SaveGame = serde_json::from_slice(&data)?;
    let area = saved
        .world
        .area
        .and_then(|a| saved.world.get(a))
        .map(|o| o.tag.clone());
    let mut sim = open(&SessionOptions {
        install: std::env::var_os("THEDAS_INSTALL")
            .map(PathBuf::from)
            .ok_or("THEDAS_INSTALL")?,
        module: saved.module.clone(),
        area,
        waypoint: None,
        seed: 1,
        keep_trace: true,
        cutscenes: CutscenePolicy::Play,
        chargen: ChargenPolicy::default(),
    })?;
    sim.load_json(&data)?;
    if args.get(1).is_some_and(|s| s == "--use-fixture") {
        return use_fixture(&mut sim);
    }
    if args.get(1).is_some_and(|s| s == "--checks") {
        return checks(&mut sim);
    }
    if let Some(journal) = args.get(1) {
        // Journal checkpoints must be restored without ordinary save-load events.
        thedas_sim::replay::load_checkpoint(&mut sim, saved)?;
        let mut previous: Option<InputEntry> = None;
        let hero = sim.world.hero.ok_or("hero")?;
        let mut motion = std::env::var_os("DOORS_MOTION_LOG")
            .map(std::fs::File::create)
            .transpose()?;
        for (line, text) in std::fs::read_to_string(journal)?.lines().enumerate() {
            let (entry, count) = match serde_json::from_str::<JournalLine>(text)? {
                JournalLine::Input(e) => {
                    previous = Some(e.clone());
                    (e, 1)
                }
                JournalLine::Repeat { n } => (previous.clone().ok_or("repeat before input")?, n),
                _ => continue,
            };
            for _ in 0..count {
                let before = sim.world.get(hero).unwrap();
                let (from, area, cmd) = (
                    before.position,
                    before.area,
                    before.commands.first().cloned(),
                );
                sim.apply(entry.source, entry.command.clone());
                let after = sim.world.get(hero).unwrap();
                let d = from
                    .iter()
                    .zip(after.position)
                    .map(|(a, b)| (a - b).powi(2))
                    .sum::<f32>()
                    .sqrt();
                if d > 2.0 {
                    println!(
                        "JUMP line={} t={} distance={d:.3} from={from:?} to={:?} area={area:?}->{:?} input={:?} front={cmd:?}",
                        line + 1,
                        sim.world.time_ms,
                        after.position,
                        after.area,
                        entry.command
                    );
                }
                if let Some(file) = motion.as_mut() {
                    let (to, area, front) =
                        (after.position, after.area, after.commands.first().cloned());
                    let floor = area
                        .and_then(|a| sim.nav_grid(a))
                        .and_then(|g| g.cell(to).map(|c| thedas_sim::nav::height(&g, c)));
                    serde_json::to_writer(
                        &mut *file,
                        &serde_json::json!({
                            "line": line + 1, "t": sim.world.time_ms, "from": from,
                            "to": to, "floor": floor, "input": entry.command, "front": front,
                        }),
                    )?;
                    writeln!(file)?;
                }
            }
        }
        println!(
            "replay end t={} hero={:?}",
            sim.world.time_ms,
            sim.world.get(hero).unwrap().position
        );
        capture_save(&sim, "journal-end")?;
        return Ok(());
    }
    let area = sim.world.area.ok_or("area")?;
    let grid = sim.nav_grid(area).ok_or("nav grid")?;
    let types = sim.world.tables().get(93, "").ok_or("placeable types")?;
    let closed = sim.closed_doors(area);
    println!(
        "area={area} grid={}x{} size={} hero={:?}",
        grid.columns,
        grid.rows,
        grid.cell_size,
        sim.world
            .hero
            .and_then(|h| sim.world.get(h))
            .map(|o| o.position)
    );
    for o in sim
        .world
        .objects
        .values()
        .filter(|o| o.area == Some(area) && o.kind == ObjectKind::Placeable)
    {
        let row = i64::from(o.appearance as u16);
        let model = match types.cell("ModelName", row) {
            Some(Cell::String(s)) => s.trim_matches('\0'),
            _ => "",
        };
        let controller = types.cell_by_hash(0x9a32_0a51, row);
        let phy = sim
            .vfs
            .read(&format!("{model}.phy"))
            .ok()
            .and_then(|(b, _)| thedas_formats::phy::Phy::parse(&b).ok());
        let pwk = sim
            .vfs
            .read(&format!("{model}_{}.pwk", o.appearance))
            .ok()
            .and_then(|(b, _)| thedas_formats::pwk::Pwk::parse(&b).ok());
        println!(
            "{} {} app={} at={:?} facing={} active={} group={:?} state={:?} type={controller:?} model={model} phy={:?} cells={} pwk={:?}",
            o.id,
            o.tag,
            o.appearance,
            o.position,
            o.facing,
            o.active,
            o.placeable_group,
            o.fields.get("State"),
            phy.as_ref().map(|p| &p.actor_masks),
            closed.iter().find(|d| d.0 == o.id).map_or(0, |d| d.1.len()),
            pwk.as_ref().map(|p| p
                .states
                .iter()
                .map(|g| g.as_ref().map(|g| (g.columns, g.rows)))
                .collect::<Vec<_>>())
        );
    }
    Ok(())
}
