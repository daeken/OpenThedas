//! `thedas autoplay`: plays a scenario from a save over several seeds and reports how it went,
//! so changes to the simulation can be judged by numbers rather than single runs.
//!
//! The player is simple and deterministic: it takes the first reply in conversations; the
//! party fights under autopilot (party AI); when the hero is idle and hostiles are in view it
//! attacks one (the same one until it falls), or with `--ai-hero` leaves the hero to its own
//! tactics as the followers are; when none are in view it walks to the nearest living hostile
//! in the area; when the area holds none it uses a door to an area it has not been to (whose
//! tag starts with `--exit-prefix`), levelling the party (the auto-level tables) first and at
//! the start. It drinks no potions beyond what the tactics do.
//!
//! Each run loads the save, reseeds the simulation's generator with the run's seed and plays
//! until a plot flag is set, an area is reached, the whole party falls (a wipe: game over),
//! or the time limit. The report gives hero deaths (mean, spread, range) and, for each
//! watched creature, how often it was alive at fixed times after a marker flag (or the
//! start). `--activity` adds how the party and the foes it fights spent the fighting time
//! (walking, attacking, using abilities, idle, down) and their attacks (swings, hits, damage).

use std::path::Path;

use anyhow::{Context, Result, bail};
use thedas_script::vm::{OBJECT_INVALID, ObjectId};
use thedas_sim::Sim;
use thedas_sim::world::{ObjectKind, Rng};

pub struct Options<'a> {
    pub install: &'a Path,
    pub module: &'a str,
    pub save: &'a Path,
    pub seeds: u64,
    pub first_seed: u64,
    pub minutes: u64,
    /// Stop when this plot flag is set (`plot:flag`, plot by name or GUID).
    pub until_flag: Option<&'a str>,
    /// Stop on reaching this area.
    pub until_area: Option<&'a str>,
    /// Doors to new areas are taken only to areas whose tag starts with this.
    pub exit_prefix: Option<&'a str>,
    /// Time zero for the samples: when this flag is first set (else the start).
    pub mark_flag: Option<&'a str>,
    /// Creatures (by tag) whose survival is reported.
    pub watch: &'a [String],
    /// Seconds after the mark at which survival is sampled.
    pub at: &'a [u64],
    pub verbose: bool,
    /// The hero fights only through the party AI (autopilot, with its tactics), as the
    /// followers do; the player just walks it towards enemies not yet seen.
    pub ai_hero: bool,
    /// Report how the party and its foes spent the fighting time, and their attacks.
    pub activity: bool,
    /// Followers waiting in the party pool get the hero's experience (provisional).
    pub pool_xp: bool,
    /// Write the last run's end state here (a save the MCP can load).
    pub save_to: Option<&'a Path>,
}

struct RunResult {
    seed: u64,
    deaths: u32,
    end: String,
    ms: u64,
    /// For each sample time, which watched creatures were alive.
    samples: Vec<Vec<bool>>,
    /// When each watched creature fell, in seconds from the mark (negative: before it).
    died: Vec<Option<f64>>,
    activity: Activity,
}

/// What a side was doing while the party fought: milliseconds summed over its members.
const STATES: [&str; 5] = ["walking", "attacking", "ability", "idle", "down"];

#[derive(Default, Clone)]
struct Side {
    ms: [u64; 5],
    swings: u32,
    hits: u32,
    damage: f64,
}

#[derive(Default, Clone)]
struct Activity {
    fight_ms: u64,
    party: Side,
    foes: Side,
}

/// What a creature is doing: walking (a move, or an attack still closing in), attacking
/// (an attack in reach), using an ability, idle (nothing, or waiting), or down.
fn state(sim: &Sim, id: ObjectId) -> usize {
    let Some(o) = sim.world.get(id) else { return 4 };
    if o.dead {
        return 4;
    }
    match o.commands.first() {
        Some(c) if c.name == "CommandAttack" && c.approach.is_some() => 0,
        Some(c) if c.name == "CommandAttack" => 1,
        Some(c) if c.name == "CommandUseAbility" => 2,
        Some(c) if c.name.starts_with("CommandMove") => 0,
        _ => 3,
    }
}

/// The party (hero and followers) and the living hostiles in the hero's area that are in the
/// fight (they see a party member or are attacking).
fn sides(sim: &Sim, hero: ObjectId) -> (Vec<ObjectId>, Vec<ObjectId>) {
    let mut party = vec![hero];
    party.extend(
        sim.world
            .get(hero)
            .map(|h| h.followers.clone())
            .unwrap_or_default(),
    );
    let area = sim.world.get(hero).and_then(|h| h.area);
    let foes = sim
        .world
        .objects
        .values()
        .filter(|o| {
            o.kind == ObjectKind::Creature
                && o.area == area
                && alive(sim, o.id)
                && sim.hostile(hero, o.id)
                && (o.combat.perceived_hostile.iter().any(|p| party.contains(p))
                    || o.commands
                        .first()
                        .is_some_and(|c| c.name == "CommandAttack"))
        })
        .map(|o| o.id)
        .collect();
    (party, foes)
}

fn parse_flag(s: &str) -> Result<(String, i32)> {
    let (p, f) = s
        .rsplit_once(':')
        .with_context(|| format!("expected plot:flag, got {s}"))?;
    Ok((
        p.to_string(),
        f.parse().with_context(|| format!("flag in {s}"))?,
    ))
}

fn flag(sim: &mut Sim, f: &(String, i32)) -> bool {
    let owner = sim.world.hero.unwrap_or(OBJECT_INVALID);
    sim.get_plot_flag(&f.0, f.1, false, owner)
}

fn alive(sim: &Sim, id: ObjectId) -> bool {
    sim.world
        .get(id)
        .is_some_and(|o| o.kind == ObjectKind::Creature && o.active && !o.dead)
}

fn area_tag(sim: &Sim) -> String {
    sim.world
        .area
        .and_then(|a| sim.world.get(a))
        .map(|a| a.tag.clone())
        .unwrap_or_default()
}

/// The living hostile nearest the hero in its area.
fn nearest_hostile(sim: &Sim, hero: ObjectId) -> Option<ObjectId> {
    let h = sim.world.get(hero)?;
    let (pos, area) = (h.position, h.area);
    sim.world
        .objects
        .values()
        .filter(|o| o.area == area && alive(sim, o.id) && sim.hostile(hero, o.id))
        .map(|o| (o.id, dist(pos, o.position)))
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|x| x.0)
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// A point at most four metres from the hero towards `to`.
fn leg(sim: &Sim, hero: ObjectId, to: [f32; 3]) -> [f32; 3] {
    let from = sim.world.get(hero).map_or(to, |h| h.position);
    let d = dist(from, to);
    if d <= 4.0 {
        return to;
    }
    let f = 4.0 / d;
    [
        from[0] + (to[0] - from[0]) * f,
        from[1] + (to[1] - from[1]) * f,
        from[2] + (to[2] - from[2]) * f,
    ]
}

/// A door in the hero's area to an area not yet visited.
fn new_door(sim: &Sim, hero: ObjectId, seen: &[String], prefix: &str) -> Option<ObjectId> {
    let area = sim.world.get(hero)?.area;
    sim.world
        .objects
        .values()
        .filter(|o| o.kind == ObjectKind::Placeable && o.active && o.area == area)
        .find(|o| {
            o.locals
                .strings
                .get("PLC_AT_DEST_AREA_TAG")
                .is_some_and(|d| {
                    !d.eq_ignore_ascii_case("none")
                        && d.starts_with(prefix)
                        && !seen.iter().any(|s| s.eq_ignore_ascii_case(d))
                })
        })
        .map(|o| o.id)
}

/// Spends every party member's pending level-up points (auto-level tables).
fn level_party(sim: &mut Sim, hero: ObjectId) {
    let mut party = vec![hero];
    party.extend(
        sim.world
            .get(hero)
            .map(|h| h.followers.clone())
            .unwrap_or_default(),
    );
    for m in party {
        sim.level_up_auto(m);
    }
}

fn play(sim: &mut Sim, save: &[u8], seed: u64, o: &Options) -> Result<RunResult> {
    sim.load_json(save).context("loading the save")?;
    sim.world.rng = Rng::new(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0x5EED);
    sim.world.autopilot = true;
    sim.world.pool_xp = o.pool_xp;
    let hero = sim.world.hero.context("the save has no hero")?;
    let until_flag = o.until_flag.map(parse_flag).transpose()?;
    let mark_flag = o.mark_flag.map(parse_flag).transpose()?;
    let start = sim.world.time_ms;
    let deadline = start + o.minutes * 60_000;
    let mut mark = if mark_flag.is_none() {
        Some(start)
    } else {
        None
    };
    // The watched creature in the hero's area (an area left behind keeps its own copy).
    let here = sim.world.get(hero).and_then(|h| h.area);
    let watch: Vec<Option<ObjectId>> = o
        .watch
        .iter()
        .map(|t| {
            let mut all = sim
                .world
                .by_tag(t)
                .filter(|x| x.kind == ObjectKind::Creature);
            let first = all.next().map(|x| (x.id, x.area));
            let in_here = std::iter::once(first)
                .flatten()
                .chain(all.map(|x| (x.id, x.area)))
                .find(|x| x.1 == here)
                .map(|x| x.0);
            in_here.or(first.map(|x| x.0))
        })
        .collect();
    let mut samples: Vec<Vec<bool>> = Vec::new();
    let mut died_at: Vec<Option<u64>> = vec![None; watch.len()];
    let mut seen_areas = vec![area_tag(sim)];
    let mut target: Option<ObjectId> = None;
    let mut deaths = 0;
    let mut was_dead = sim.world.get(hero).is_some_and(|h| h.dead);
    level_party(sim, hero);
    sim.world.trace.attacks.clear();
    let mut activity = Activity::default();
    // The states seen at the last step, credited with the time the step took.
    let mut seen: Option<(u64, Vec<usize>, Vec<usize>)> = None;
    let mut party_ids: Vec<ObjectId> = Vec::new();
    let end;
    loop {
        let now = sim.world.time_ms;
        if o.activity {
            if let Some((t0, p, f)) = seen.take() {
                let dt = now.saturating_sub(t0);
                activity.fight_ms += dt;
                for s in p {
                    activity.party.ms[s] += dt;
                }
                for s in f {
                    activity.foes.ms[s] += dt;
                }
            }
            if sim.world.party_fighting {
                let (party, foes) = sides(sim, hero);
                for m in &party {
                    if !party_ids.contains(m) {
                        party_ids.push(*m);
                    }
                }
                seen = Some((
                    now,
                    party.iter().map(|&m| state(sim, m)).collect(),
                    foes.iter().map(|&m| state(sim, m)).collect(),
                ));
            }
        }
        let dead = sim.world.get(hero).is_none_or(|h| h.dead);
        if dead && !was_dead {
            deaths += 1;
        }
        was_dead = dead;
        if mark.is_none() && mark_flag.as_ref().is_some_and(|f| flag(sim, f)) {
            mark = Some(now);
        }
        for (i, w) in watch.iter().enumerate() {
            if died_at[i].is_none() && w.is_some_and(|w| sim.world.get(w).is_some_and(|o| o.dead)) {
                died_at[i] = Some(now);
            }
        }
        if let Some(m) = mark {
            while samples.len() < o.at.len() && now >= m + o.at[samples.len()] * 1000 {
                samples.push(
                    watch
                        .iter()
                        .map(|w| w.is_some_and(|w| alive(sim, w)))
                        .collect(),
                );
            }
        }
        if until_flag.as_ref().is_some_and(|f| flag(sim, f)) {
            end = "flag".to_string();
            break;
        }
        let here = area_tag(sim);
        if o.until_area.is_some_and(|a| here.eq_ignore_ascii_case(a)) {
            end = "area".to_string();
            break;
        }
        if !seen_areas.contains(&here) {
            seen_areas.push(here);
        }
        if now >= deadline {
            end = "time".to_string();
            break;
        }
        if sim.world.game_over {
            end = "wipe".to_string();
            break;
        }
        if let Some(d) = sim.dialog_state().filter(|d| !d.ended) {
            let _ = d;
            sim.choose_reply(0);
            sim.tick(0);
            continue;
        }
        if dead {
            sim.tick(1000);
            continue;
        }
        let hostiles: Vec<ObjectId> = sim
            .world
            .get(hero)
            .map(|h| h.combat.perceived_hostile.clone())
            .unwrap_or_default()
            .into_iter()
            .filter(|&c| alive(sim, c))
            .collect();
        let idle = sim
            .world
            .get(hero)
            .is_some_and(|h| h.commands.first().is_none_or(|c| c.name == "CommandWait"));
        if !hostiles.is_empty() {
            if idle && !o.ai_hero {
                let t = target
                    .filter(|t| hostiles.contains(t))
                    .unwrap_or(hostiles[0]);
                target = Some(t);
                sim.order_attack(hero, t);
            }
            sim.tick(500);
            continue;
        }
        if let Some(foe) = nearest_hostile(sim, hero) {
            let (p, a) = sim
                .world
                .get(foe)
                .map(|f| (f.position, f.area))
                .unwrap_or_default();
            // A few metres at a time, so the clock (and the samples) keep pace.
            if !sim.walk(hero, leg(sim, hero, p), a) {
                sim.tick(500);
            }
            continue;
        }
        if let Some(door) = o
            .exit_prefix
            .and_then(|pre| new_door(sim, hero, &seen_areas, pre))
        {
            level_party(sim, hero);
            let (p, a) = sim
                .world
                .get(door)
                .map(|d| (d.position, d.area))
                .unwrap_or_default();
            sim.walk(hero, p, a);
            let ev = sim.new_event("USE", Some(hero));
            sim.deliver(door, ev, None);
            sim.tick(500);
            continue;
        }
        sim.tick(1000);
    }
    // Samples not reached (the run ended first) report the creatures' state at the end.
    while samples.len() < o.at.len() {
        samples.push(
            watch
                .iter()
                .map(|w| w.is_some_and(|w| alive(sim, w)))
                .collect(),
        );
    }
    let m = mark.unwrap_or(start) as f64;
    let died = died_at
        .iter()
        .map(|d| d.map(|t| (t as f64 - m) / 1000.0))
        .collect();
    Ok(RunResult {
        died,
        seed,
        deaths,
        end,
        ms: sim.world.time_ms - start,
        activity: {
            for (id, a) in &sim.world.trace.attacks {
                let side = if party_ids.contains(id) || Some(*id) == sim.world.hero {
                    &mut activity.party
                } else {
                    &mut activity.foes
                };
                side.swings += a.swings;
                side.hits += a.hits;
                side.damage += a.damage as f64;
            }
            activity
        },
        samples,
    })
}

pub fn run(o: &Options) -> Result<()> {
    if o.seeds == 0 {
        bail!("--seeds must be at least 1");
    }
    let save = std::fs::read(o.save).with_context(|| format!("reading {}", o.save.display()))?;
    let mut sim = thedas_sim::session::open(&thedas_sim::session::SessionOptions {
        install: o.install.to_path_buf(),
        module: o.module.to_string(),
        area: None,
        waypoint: None,
        seed: 1,
        keep_trace: false,
    })
    .with_context(|| format!("opening module {}", o.module))?;
    let mut runs = Vec::new();
    for seed in o.first_seed..o.first_seed + o.seeds {
        let r = play(&mut sim, &save, seed, o)?;
        if o.verbose {
            println!(
                "seed {:>4}: {:>3} hero deaths, ended by {} after {:.0} s",
                r.seed,
                r.deaths,
                r.end,
                r.ms as f64 / 1000.0
            );
        }
        runs.push(r);
    }
    if let Some(p) = o.save_to {
        std::fs::write(p, sim.save_json()?).with_context(|| format!("writing {}", p.display()))?;
    }
    let n = runs.len() as f64;
    let deaths: Vec<f64> = runs.iter().map(|r| r.deaths as f64).collect();
    let mean = deaths.iter().sum::<f64>() / n;
    let sd = (deaths.iter().map(|d| (d - mean).powi(2)).sum::<f64>() / n).sqrt();
    let min = deaths.iter().copied().fold(f64::INFINITY, f64::min);
    let max = deaths.iter().copied().fold(0.0, f64::max);
    let mut ends = std::collections::BTreeMap::new();
    for r in &runs {
        *ends.entry(r.end.as_str()).or_insert(0) += 1;
    }
    let secs: Vec<f64> = runs.iter().map(|r| r.ms as f64 / 1000.0).collect();
    println!(
        "{} runs (seeds {}..{}): hero deaths mean {mean:.1}, sd {sd:.1}, range {min}-{max}; \
         ended {ends:?}; game time mean {:.0} s",
        runs.len(),
        o.first_seed,
        o.first_seed + o.seeds - 1,
        secs.iter().sum::<f64>() / n
    );
    if o.activity {
        let fight = runs.iter().map(|r| r.activity.fight_ms).sum::<u64>() as f64;
        println!(
            "fighting: mean {:.0} s per run; share of member-time (and attacks per run):",
            fight / n / 1000.0
        );
        for name in ["party", "foes"] {
            let mut total = Side::default();
            for r in &runs {
                let s = if name == "party" {
                    &r.activity.party
                } else {
                    &r.activity.foes
                };
                for i in 0..5 {
                    total.ms[i] += s.ms[i];
                }
                total.swings += s.swings;
                total.hits += s.hits;
                total.damage += s.damage;
            }
            let all = total.ms.iter().sum::<u64>().max(1) as f64;
            print!("  {name:<6}");
            for (i, st) in STATES.iter().enumerate() {
                print!(" {st} {:.0}%", 100.0 * total.ms[i] as f64 / all);
            }
            println!(
                "; {:.1} swings, {:.0}% hit, {:.0} damage",
                total.swings as f64 / n,
                100.0 * total.hits as f64 / (total.swings.max(1)) as f64,
                total.damage / n
            );
        }
    }
    if !o.watch.is_empty() {
        print!("{:<24} {:>9}", "alive at", "fell at");
        for t in o.at {
            print!(" {:>6}", format!("{t}s"));
        }
        println!();
        for (i, tag) in o.watch.iter().enumerate() {
            let fell: Vec<f64> = runs.iter().filter_map(|r| r.died[i]).collect();
            let fell = if fell.is_empty() {
                "-".to_string()
            } else {
                format!("{:.0}s", fell.iter().sum::<f64>() / fell.len() as f64)
            };
            print!("{tag:<24} {fell:>9}");
            for s in 0..o.at.len() {
                let alive = runs.iter().filter(|r| r.samples[s][i]).count();
                print!(" {:>5.0}%", 100.0 * alive as f64 / n);
            }
            println!();
        }
    }
    Ok(())
}
