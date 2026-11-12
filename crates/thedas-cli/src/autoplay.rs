//! `thedas autoplay`: plays a scenario from a save over several seeds and reports how it went,
//! so changes to the simulation can be judged by numbers rather than single runs.
//!
//! The player is simple and deterministic: it takes the first reply in conversations, and the
//! party fights under autopilot (party AI, the hero's own tactics included). It steers only
//! when the party has been out of a fight for 30 s: it then walks towards the nearest living
//! hostile in the area (in view or not), and when the area holds none it uses a door to an
//! area it has not been to (whose tag starts with `--exit-prefix`), levelling the party (the
//! auto-level tables) first and at the start; it opens closed doors in its way. After each
//! fight it waits (up to 30 s) for the party's health to come back. It drinks no potions
//! beyond what the tactics do. With `--player` (harness player mode, not the game's AI) it
//! also plays the party as a human would: it puts the best gear in the pack on the party
//! before the run and after each fight, and in fights orders talents and spells on foes and
//! drinks health and lyrium potions when low. With `--steer` it plays as before: once hostiles are in view it
//! orders the hero's attacks (the same target until it falls; `--ai-hero` leaves the hero to
//! its tactics) and walks the idle party towards them, and it seeks unseen hostiles at once.
//!
//! Each run loads the save, reseeds the simulation's generator with the run's seed and plays
//! until a plot flag is set, an area is reached, the whole party falls (a wipe: game over),
//! or the time limit. The report gives hero deaths (mean, spread, range) and, for each
//! watched creature, how often it was alive at fixed times after a marker flag (or the
//! start). `--activity` adds how the party and the foes it fights spent the fighting time
//! (walking, attacking, using abilities, idle, down) and their attacks (swings, hits, damage;
//! for the foes, those of hostiles that took part in the party's fights, see [`sides`]).

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
    /// The older harness player: it orders the hero's attacks (unless `ai_hero`) and walks
    /// the party towards hostiles in view. Without it (the standard measurement) the party
    /// AI alone fights, the hero included, and the harness only explores while nothing is
    /// going on (see [`QUIET_MS`]).
    pub steer: bool,
    /// Harness player mode (see [`player_gear`] and [`player_act`]): the harness equips the
    /// best gear in the pack and orders abilities and potions in fights, as a human would.
    /// None of it is the game's AI.
    pub player: bool,
    /// Before anything else, walk the hero up to the first creature with this tag and talk
    /// to it (to start an encounter that begins with that conversation).
    pub approach: Option<&'a str>,
    /// Report how the party and its foes spent the fighting time, and their attacks.
    pub activity: bool,
    /// Followers waiting in the party pool get the hero's experience (provisional).
    pub pool_xp: bool,
    /// Ready the party before each run (see [`prepare`]).
    pub prepare: bool,
    /// End a run ("won") once this many fights are won: every hostile the party saw in the
    /// fight is down (0: no such end).
    pub fights: u32,
    /// Write the last run's full trace here (JSON lines).
    pub trace_to: Option<&'a Path>,
    /// Add the runs' coverage counters (unimplemented, stubbed and presentation-only engine
    /// calls, missing 2DA data) to this JSON file.
    pub coverage_to: Option<&'a Path>,
    /// Write the last run's end state here (a save the MCP can load).
    pub save_to: Option<&'a Path>,
}

struct RunResult {
    seed: u64,
    deaths: u32,
    /// Milliseconds the party spent fighting (from seeing hostiles to seeing none).
    fight_ms: u64,
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
    /// Health taken, by side ("party" or "others": foes and allies alike), the source's tag
    /// and the ability (0 for weapon attacks).
    sources: std::collections::BTreeMap<(&'static str, String, i32), f64>,
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
/// fight: they see a party member, or a party member sees them attacking (anyone: allies of
/// the party count). A hostile attacking out of the party's sight (the Wilds' darkspawn
/// fighting the hero's immortal mabari far off, before it joins) is not in the party's fight.
fn sides(sim: &Sim, hero: ObjectId) -> (Vec<ObjectId>, Vec<ObjectId>) {
    let mut party = vec![hero];
    party.extend(
        sim.world
            .get(hero)
            .map(|h| h.followers.clone())
            .unwrap_or_default(),
    );
    let seen: Vec<ObjectId> = party
        .iter()
        .filter_map(|&m| sim.world.get(m))
        .flat_map(|m| m.combat.perceived_hostile.iter().copied())
        .collect();
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
                    || (seen.contains(&o.id)
                        && o.commands
                            .first()
                            .is_some_and(|c| c.name == "CommandAttack")))
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

/// How long the party must have been out of a fight before the harness, in its standard mode,
/// walks towards hostiles (seen or not) or takes a door: enemies that are coming get to
/// arrive, and the party is not steered into a fight the game would bring to it.
const QUIET_MS: u64 = 30_000;

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

/// Waits (up to 30 s) until every standing party member has 95% of its health, as a player
/// pauses between fights, stopping if a fight starts.
fn rest(sim: &mut Sim, hero: ObjectId) {
    let health = 7;
    for _ in 0..30 {
        let mut party = vec![hero];
        party.extend(
            sim.world
                .get(hero)
                .map(|h| h.followers.clone())
                .unwrap_or_default(),
        );
        let rested = party.iter().all(|&m| {
            !alive(sim, m)
                || sim
                    .world
                    .get(m)
                    .and_then(|o| o.properties.get(&health))
                    .is_none_or(|v| v.1 >= 0.95 * sim.depletable_max(m, health))
        });
        if rested || sim.world.party_fighting || sim.world.game_over {
            return;
        }
        sim.tick(1000);
    }
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

/// Readies the party for a fight as a player would, through the game's own developer
/// scripts (designerscripts.rim, run as the console's `runscript` runs them: arguments in the
/// module's `RUNSCRIPT_VAR`), so that a scenario save taken mid-play measures the fight rather
/// than the state the party arrived in:
/// - `injury remparty` removes the party's injuries;
/// - `addxp <n>` (sys_reward's `RewardXPParty`) pays the party experience until every member
///   reaches the area's minimum level (`areadata.gda` `MinLevel` for the area's `AREA_ID`,
///   the level creature_core's area balance scales foes to at least), each member then
///   spending its points from its auto-level table ([`level_party`]);
/// - `healplayer` restores the party's health and mana/stamina.
///
/// Harness only: nothing in the game runs these on its own.
fn prepare(sim: &mut Sim, hero: ObjectId) {
    let run = |sim: &mut Sim, script: &str, args: String| {
        if let Some(m) = sim.world.module {
            if let Some(o) = sim.world.get_mut(m) {
                o.locals.strings.insert("RUNSCRIPT_VAR".into(), args);
            }
        }
        sim.run_script(script, hero, None);
        sim.tick(0);
    };
    run(sim, "injury", "remparty".into());
    let area = sim.world.get(hero).and_then(|h| h.area);
    let area_id = area
        .and_then(|a| sim.world.get(a))
        .and_then(|a| a.locals.ints.get("AREA_ID").copied())
        .unwrap_or(0);
    let int = |sim: &Sim, table: i32, column: &str, row: i64| match sim
        .world
        .tables()
        .get(table, "")
        .and_then(|t| t.cell(column, row).cloned())
    {
        Some(thedas_formats::gda::Cell::Int(v)) => Some(v),
        Some(thedas_formats::gda::Cell::Float(v)) => Some(v as i64),
        _ => None,
    };
    // areadata (M2DA 225) and the experience table (M2DA 164), as creature_core and
    // player_core read them.
    let min_level = int(sim, 225, "MinLevel", area_id as i64).unwrap_or(0);
    if min_level > 0 {
        let needed = int(sim, 164, "XP", min_level).unwrap_or(0) as f32;
        for _ in 0..8 {
            let lowest = members(sim, hero)
                .iter()
                .filter_map(|&m| sim.world.get(m))
                .map(|o| o.properties.get(&XP).map_or(0.0, |v| v.0))
                .fold(f32::MAX, f32::min);
            if lowest >= needed || lowest == f32::MAX {
                break;
            }
            // addxp reads the amount up to the first space.
            run(
                sim,
                "addxp",
                format!("{} ", (needed - lowest).ceil() as i64),
            );
            level_party(sim, hero);
        }
    }
    run(sim, "healplayer", String::new());
}

/// `PROPERTY_SIMPLE_EXPERIENCE`.
const XP: i32 = 19;

/// The hero and its followers.
fn members(sim: &Sim, hero: ObjectId) -> Vec<ObjectId> {
    let mut party = vec![hero];
    party.extend(
        sim.world
            .get(hero)
            .map(|h| h.followers.clone())
            .unwrap_or_default(),
    );
    party
}

/// `PROPERTY_DEPLETABLE_HEALTH` and `PROPERTY_DEPLETABLE_MANA_STAMINA`.
const HEALTH: i32 = 7;
const MANA: i32 = 8;

/// Harness player mode (not the game): puts the best gear in the party's pack on the party,
/// as a player does between fights. An item replaces what a member wears in a slot only when
/// it has the same base item type (so weapon style, armour weight and class restrictions stay
/// as the save has them) and its armour, damage, defence and spellpower (`Sim::item_stat`)
/// sum higher. Provisional: attribute requirements are not checked (the simulation does not
/// model them), so it may equip something the game would refuse.
fn player_gear(sim: &mut Sim, hero: ObjectId) {
    const STATS: [i32; 4] = [1, 2, 3, 10];
    let score = |sim: &Sim, i: ObjectId| STATS.iter().map(|&s| sim.item_stat(i, s)).sum::<f32>();
    let base = |sim: &Sim, i: ObjectId| {
        sim.world
            .get(i)
            .and_then(|o| o.fields.get("BaseItem").copied())
    };
    for m in members(sim, hero) {
        if !alive(sim, m) {
            continue;
        }
        let worn: Vec<(i32, ObjectId)> = sim
            .world
            .get(m)
            .map(|o| o.equipment.iter().map(|(s, i)| (*s, *i)).collect())
            .unwrap_or_default();
        for (slot, cur) in worn {
            let Some(b) = base(sim, cur) else { continue };
            let mut best = (cur, score(sim, cur));
            for i in sim.party_backpack() {
                if base(sim, i) == Some(b) && sim.item_slots(i).contains(&slot) {
                    let v = score(sim, i);
                    if v > best.1 + 1e-3 {
                        best = (i, v);
                    }
                }
            }
            if best.0 != cur {
                sim.equip_item(m, best.0, slot);
            }
        }
    }
}

/// An `ABI_base` cell as a number (0 when missing).
fn abi(sim: &Sim, ability: i32, column: &str) -> f32 {
    use thedas_formats::gda::Cell;
    match sim
        .world
        .tables()
        .get(0, "ABI_base")
        .and_then(|t| t.cell(column, ability as i64).cloned())
    {
        Some(Cell::Int(v)) => v as f32,
        Some(Cell::Float(v)) => v as f32,
        _ => 0.0,
    }
}

/// Harness player mode (not the game's AI), in a fight, every half second: what a human
/// player orders. A member under 40% health drinks a health potion or poultice (the lesser
/// first), a mage under 20% mana a lyrium potion; otherwise a member attacking (or doing
/// nothing) uses its first ready talent or spell aimed at foes on its target, or the nearest
/// foe it sees: an activated (`UseType` 4) talent, spell or skill (`AbilityType` 1 to 3)
/// whose `TargetType` takes a hostile (bit 4; a place, bit 32, is aimed at the foe's feet),
/// whose use conditions hold, whose cooldown is over and whose `cost` the member's mana or
/// stamina covers. A member it ordered is left alone for 3 s. Sustained abilities are not
/// switched on.
fn player_act(
    sim: &mut Sim,
    hero: ObjectId,
    ordered: &mut std::collections::BTreeMap<ObjectId, u64>,
) {
    let now = sim.world.time_ms;
    let wizard = thedas_sim::sim::constant(&sim.table, "CLASS_WIZARD");
    for m in members(sim, hero) {
        if !alive(sim, m) || ordered.get(&m).is_some_and(|&t| now < t + 3000) {
            continue;
        }
        let Some(o) = sim.world.get(m) else { continue };
        let front = o.commands.first().map(|c| (c.name.clone(), c.target));
        if front
            .as_ref()
            .is_some_and(|(n, _)| n != "CommandAttack" && n != "CommandWait")
        {
            continue;
        }
        let cur = |p: i32| o.properties.get(&p).map_or(0.0, |v| v.1);
        let (hp, mp) = (cur(HEALTH), cur(MANA));
        let (pos, area) = (o.position, o.area);
        let mut seen: Vec<ObjectId> = o.combat.perceived_hostile.clone();
        let abilities: Vec<i32> = o.abilities.iter().copied().collect();
        seen.retain(|&f| alive(sim, f) && sim.hostile(m, f));
        let potion = |sim: &Sim, prefix: &str| {
            let mut all: Vec<(String, i32, i32)> = sim
                .quick_items()
                .into_iter()
                .filter(|(t, a, n)| {
                    *n > 0
                        && t.to_ascii_lowercase().starts_with(prefix)
                        && sim.cooldown_left(m, *a, t) == 0.0
                })
                .collect();
            all.sort();
            all.into_iter().next()
        };
        let class = sim.property_total(m, 27) as i32;
        let drink = if hp < 0.4 * sim.depletable_max(m, HEALTH) {
            potion(sim, "gen_im_qck_health")
        } else if class == wizard && mp < 0.2 * sim.depletable_max(m, MANA) {
            potion(sim, "gen_im_qck_mana")
        } else {
            None
        };
        if let Some((tag, ability, _)) = drink {
            sim.order_ability(m, ability, m, &tag);
            ordered.insert(m, now);
            continue;
        }
        let target = front
            .and_then(|(n, t)| t.filter(|_| n == "CommandAttack"))
            .filter(|&t| alive(sim, t))
            .or_else(|| {
                seen.iter()
                    .copied()
                    .filter(|&f| sim.world.get(f).is_some_and(|x| x.area == area))
                    .min_by(|&a, &b| {
                        let d = |c| sim.world.get(c).map_or(f32::MAX, |x| dist(pos, x.position));
                        d(a).total_cmp(&d(b))
                    })
            });
        let Some(target) = target else { continue };
        let usable = abilities.into_iter().find(|&a| {
            let tt = sim.ability_target_type(a);
            sim.ability_use_type(a) == 4
                && (1..=3).contains(&(abi(sim, a, "AbilityType") as i32))
                && tt & (4 | 32) != 0
                && sim.ability_conditions_met(m, a, -1)
                && sim.cooldown_left(m, a, "") == 0.0
                && abi(sim, a, "cost") <= mp
        });
        let Some(a) = usable else { continue };
        if sim.ability_target_type(a) & 4 != 0 {
            sim.order_ability(m, a, target, "");
        } else if let Some(p) = sim.world.get(target).map(|x| x.position) {
            sim.order_ability_at(m, a, p, "");
        }
        ordered.insert(m, now);
    }
}

fn play(sim: &mut Sim, save: &[u8], seed: u64, o: &Options) -> Result<RunResult> {
    sim.load_json(save).context("loading the save")?;
    sim.world.rng = Rng::new(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0x5EED);
    sim.world.autopilot = true;
    sim.world.pool_xp = o.pool_xp;
    let hero = sim.world.hero.context("the save has no hero")?;
    // A save taken after a wipe that the party got up from still carries the game-over
    // latch; a run starts from a standing party, so the harness clears it.
    if sim.world.game_over && alive(sim, hero) {
        sim.world.game_over = false;
    }
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
    if o.prepare {
        prepare(sim, hero);
    }
    level_party(sim, hero);
    if o.player {
        player_gear(sim, hero);
    }
    let mut ordered = std::collections::BTreeMap::new();
    sim.world.trace.attacks.clear();
    sim.world.trace.keep = o.trace_to.is_some();
    sim.world.trace.entries.clear();
    sim.world.trace.damage.clear();
    let mut activity = Activity::default();
    // The states seen at the last step, credited with the time the step took.
    let mut seen: Option<(u64, Vec<usize>, Vec<usize>)> = None;
    let mut party_ids: Vec<ObjectId> = Vec::new();
    // The hostiles that took part in the party's fights (whose attacks count for the foes).
    let mut foe_ids: std::collections::BTreeSet<ObjectId> = std::collections::BTreeSet::new();
    let (mut fight_ms, mut fights_won, mut last_t) = (0u64, 0u32, sim.world.time_ms);
    let mut engaged: std::collections::BTreeSet<ObjectId> = std::collections::BTreeSet::new();
    // Hostiles of fights already won: a fallen foe can stay in a party member's perceived
    // list, and counting it again would win the same fight over and over.
    let mut beaten: std::collections::BTreeSet<ObjectId> = std::collections::BTreeSet::new();
    let mut quiet_since = sim.world.time_ms;
    if let Some(tag) = o.approach {
        let to = sim
            .world
            .by_tag(tag)
            .find(|x| x.kind == ObjectKind::Creature)
            .map(|x| (x.id, x.position, x.area));
        if let Some((who, p, a)) = to {
            // One walk along the route, as the player clicks there: it stops when a
            // conversation or a fight starts on the way.
            sim.walk(hero, p, a);
            // Then talk to it, as the player does by clicking it (from where the walk ended:
            // the route may not reach it).
            if !sim.world.party_fighting && sim.dialog_state().is_none_or(|d| d.ended) {
                sim.start_conversation(hero, who, "");
            }
        }
    }
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
                foe_ids.extend(foes.iter().copied());
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
        // A fight is won when every hostile the party saw in it is down.
        let fighting = sim.world.party_fighting;
        if fighting {
            fight_ms += now.saturating_sub(last_t);
            let (_, foes) = sides(sim, hero);
            engaged.extend(foes.into_iter().filter(|f| !beaten.contains(f)));
            let mut party = vec![hero];
            party.extend(
                sim.world
                    .get(hero)
                    .map(|h| h.followers.clone())
                    .unwrap_or_default(),
            );
            for m in party {
                if let Some(x) = sim.world.get(m) {
                    engaged.extend(
                        x.combat
                            .perceived_hostile
                            .iter()
                            .copied()
                            .filter(|f| !beaten.contains(f)),
                    );
                }
            }
        }
        let won = !engaged.is_empty() && engaged.iter().all(|&f| !alive(sim, f));
        if won {
            fights_won += 1;
            beaten.append(&mut engaged);
        }
        last_t = now;
        if o.fights > 0 && fights_won >= o.fights {
            end = "won".to_string();
            break;
        }
        // After a fight, the player lets the party get its breath back (health regenerates
        // quickly out of combat) before seeking the next.
        if won {
            if o.player {
                player_gear(sim, hero);
            }
            rest(sim, hero);
            // Time moves on even when the party is still in combat (rest returns at once).
            if sim.world.time_ms == now {
                sim.tick(500);
            }
            continue;
        }
        if o.player && sim.world.party_fighting && sim.dialog_state().is_none_or(|d| d.ended) {
            player_act(sim, hero, &mut ordered);
        }
        // A conversation is answered with its first reply; one that has shown its last line
        // is closed the same way, as continuing does in the game (its end may start a fight).
        if let Some(d) = sim.dialog_state() {
            if d.ended {
                sim.end_conversation();
            } else {
                sim.choose_reply(0);
            }
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
        if sim.world.party_fighting {
            quiet_since = now;
        }
        let quiet = now.saturating_sub(quiet_since) >= QUIET_MS;
        // The standard measurement: while a fight is on or one may be coming, the party AI
        // alone plays; the harness waits.
        if !o.steer && !quiet {
            sim.tick(500);
            continue;
        }
        if !hostiles.is_empty() {
            if idle && !o.ai_hero && o.steer {
                let t = target
                    .filter(|t| hostiles.contains(t))
                    .unwrap_or(hostiles[0]);
                target = Some(t);
                sim.order_attack(hero, t);
            }
            // The party AI does not chase a hostile far from the controlled character (a
            // wolf seen 35 m off that has not noticed the party): with everyone idle, the
            // player walks toward the nearest one, as they would.
            if idle && (o.ai_hero || !o.steer) && party_idle(sim, hero) {
                let near = hostiles.iter().copied().min_by(|&a, &b| {
                    let d = |c| {
                        sim.world.get(c).map_or(f32::MAX, |x| {
                            let h = sim.world.get(hero).map(|h| h.position).unwrap_or_default();
                            (x.position[0] - h[0]).hypot(x.position[1] - h[1])
                        })
                    };
                    d(a).total_cmp(&d(b))
                });
                if let Some((p, a)) = near
                    .and_then(|f| sim.world.get(f))
                    .map(|f| (f.position, f.area))
                {
                    let next = leg(sim, hero, p);
                    sim.walk(hero, next, a);
                }
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
                // A closed door in the way: the player opens it (uses it), as by clicking.
                if let Some(door) = sim.walk_blocked_by {
                    let ev = sim.new_event("USE", Some(hero));
                    sim.deliver(door, ev, None);
                }
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
        fight_ms,
        activity: {
            for (id, a) in &sim.world.trace.attacks {
                let side = if party_ids.contains(id) || Some(*id) == sim.world.hero {
                    &mut activity.party
                } else if foe_ids.contains(id) {
                    &mut activity.foes
                } else {
                    continue;
                };
                side.swings += a.swings;
                side.hits += a.hits;
                side.damage += a.damage as f64;
            }
            for ((src, ability), amount) in &sim.world.trace.damage {
                let side = if party_ids.contains(src) || Some(*src) == sim.world.hero {
                    "party"
                } else {
                    "others"
                };
                let tag = sim
                    .world
                    .get(*src)
                    .map_or_else(|| src.to_string(), |o| o.tag.clone());
                *activity.sources.entry((side, tag, *ability)).or_default() += *amount as f64;
            }
            activity
        },
        samples,
    })
}

/// Whether the hero's party is doing nothing (each member's queue empty or waiting).
fn party_idle(sim: &Sim, hero: ObjectId) -> bool {
    let mut party = vec![hero];
    party.extend(
        sim.world
            .get(hero)
            .map(|h| h.followers.clone())
            .unwrap_or_default(),
    );
    party.iter().all(|&m| {
        sim.world
            .get(m)
            .is_none_or(|x| x.dead || x.commands.first().is_none_or(|c| c.name == "CommandWait"))
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
        cutscenes: Default::default(),
        chargen: Default::default(),
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
    if let Some(p) = o.coverage_to {
        sim.world
            .trace
            .merge_coverage(p)
            .with_context(|| format!("writing {}", p.display()))?;
    }
    if let Some(p) = o.trace_to {
        std::fs::write(p, sim.world.trace.to_jsonl())
            .with_context(|| format!("writing {}", p.display()))?;
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
         ended {ends:?}; game time mean {:.0} s, fighting {:.0} s",
        runs.len(),
        o.first_seed,
        o.first_seed + o.seeds - 1,
        secs.iter().sum::<f64>() / n,
        runs.iter().map(|r| r.fight_ms as f64).sum::<f64>() / n / 1000.0
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
        let mut sources: std::collections::BTreeMap<(&str, String, i32), f64> =
            std::collections::BTreeMap::new();
        for r in &runs {
            for (k, v) in &r.activity.sources {
                *sources.entry(k.clone()).or_default() += v;
            }
        }
        println!("health taken per run, by source (ability 0: weapon attacks):");
        for side in ["others", "party"] {
            let mut rows: Vec<_> = sources.iter().filter(|(k, _)| k.0 == side).collect();
            rows.sort_by(|a, b| b.1.total_cmp(a.1));
            let total: f64 = rows.iter().map(|r| r.1).sum();
            println!("  by {side}: {:.0}", total / n);
            for ((_, tag, ability), v) in rows.iter().take(8) {
                println!("    {tag:<28} ability {ability:<6} {:>6.0}", *v / n);
            }
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
