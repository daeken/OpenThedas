//! A minimal combat core: perception, hostility, threat, attacks and death.
//!
//! The game's rules live in its scripts (`rules_core` and the AI includes): they decide what
//! to attack, roll hits and damage from the rules 2DAs, and apply damage effects. The engine
//! supplies the plumbing those scripts expect, which is what this module does:
//!
//! - **Perception.** Creatures in the hero's area perceive each other within
//!   their perception range ([`Sim::perception_range`]) and get `EVENT_TYPE_PERCEPTION_APPEAR` (object 0 is the creature
//!   seen; integers are hostility changed, stealthed, hostile) and `PERCEPTION_DISAPPEAR`
//!   when it leaves the range. A creature in stealth (`SetStealthEnabled`, which the stealth
//!   effect's handlers call) is not perceived by those hostile to it: they get one
//!   appearance with the stealthed flag, on which rules_core runs its own detection check.
//!   The scripts also end stealth themselves (after an attack, when targeted, on detection).
//!   Walls (`nav::wall_between`) and closed doors block noticing.
//! - **Game mode.** The scripts switch `GM_EXPLORE` to `GM_COMBAT` and back with
//!   `SetGameMode` (rules_core asks the module when a follower perceives a hostile; the
//!   module's end-of-fight check returns to explore and gets fallen members up). The engine
//!   only applies the change and tells the module with `GAMEMODE_CHANGE`
//!   ([`Sim::update_game_mode`] keeps the harness's view of the fight).
//! - **Hostility.** Creatures are hostile when their groups are set hostile with
//!   `SetGroupHostility`.
//! - **Attacks.** When a `CommandAttack` reaches the front of the queue, its owner gets
//!   `EVENT_TYPE_COMMAND_PENDING` (objects: owner, target; integers: command type, id). The
//!   script answers with `SetAttackResult` and `SetAttackDuration`. When the attack's time is
//!   up, the owner gets one `EVENT_TYPE_ATTACK_IMPACT` per hand the script gave a result
//!   (objects: attacker, target; integers: that hand's hit result and its damage effect), and
//!   `GetAttackImpactDamageEffect` returns that hand's damage effect (see
//!   [`Sim::finish_attack`]).
//! - **Effects.** `ApplyEffectOnObject` runs the target's `EVENT_TYPE_APPLY_EFFECT` handler at
//!   once, with `GetCurrentEffect` returning the effect. Unless the handler calls
//!   `SetIsCurrentEffectValid(FALSE)`, an effect with a duration stays on the target.
//! - **Resurrection.** An accepted `EFFECT_TYPE_RESURRECTION` brings the target back; its
//!   handler restores health.
//! - **Death.** An `EFFECT_TYPE_DEATH` that its handler accepts kills the target (the rules
//!   scripts turn a lethal hit into one). A creature whose current health (`PROPERTY_DEPLETABLE_HEALTH`) drops to zero
//!   or below and is neither immortal nor plot dies and gets `EVENT_TYPE_DEATH`, with the
//!   killer as creator and object 0.
//!
//! All of this is provisional: event layouts are read from how `rules_core` reads them, and
//! the range and default attack time are guesses, not values taken from the executable.

use serde::{Deserialize, Serialize};
use thedas_script::vm::{OBJECT_INVALID, ObjectId};

use crate::sim::Sim;
use crate::world::{Effect, ObjectKind};

/// How far a creature notices others when ranges.gda cannot say, in metres. Provisional.
pub const PERCEPTION_RANGE: f32 = 30.0;

/// ranges.gda (M2DA 134): a creature's `PerceptionClass` is one of its rows
/// (`PercepRngShrt` 8 .. `PercepRngLargeCreature` 15).
pub(crate) const RANGES_TABLE: i32 = 134;
/// ranges.gda rows for creatures whose template gives no `PerceptionClass`.
const PERCEPTION_DEFAULT: i64 = 11;
const PERCEPTION_PLAYER: i64 = 12;

/// How far a creature keeps track of one it has noticed: script.ldf's `TriggerPerception`
/// speaks of an outer "visual radius (about 60m)" beyond the normal perception ring.
/// Provisional.
pub const VISUAL_RANGE: f32 = 60.0;

/// `properties.gda`: a depletable's regeneration property out of combat and in combat
/// (DAOrigins.exe reads them at 0x796620 and 0x79663c; see [`Sim::regenerate`]).
const COL_REGEN: u32 = 0x7a3d_f160;
const COL_REGEN_COMBAT: u32 = 0xe24b_074d;
/// How often a creature regenerates, out of combat and in combat (0x71b101, 0x71b0f5).
pub const REGEN_PERIOD_MS: u64 = 3000;
pub const REGEN_PERIOD_COMBAT_MS: u64 = 2000;

/// `EFFECT_TYPE_DEATH` in effects.gda.
pub const EFFECT_TYPE_DEATH: i32 = 7;

/// `EFFECT_TYPE_DOT` in effects.gda.
pub const EFFECT_TYPE_DOT: i32 = 1017;

/// `EFFECT_TYPE_RESURRECTION` in effects.gda.
pub const EFFECT_TYPE_RESURRECTION: i32 = 1005;

/// How long an attack takes when the script does not say (`SetAttackDuration`). Provisional.
/// The script the original executable sends ability cast events to.
pub const ABILITY_SCRIPT: &str = "ability_core";
pub const DEFAULT_ATTACK_MS: u64 = 1500;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CombatData {
    /// `GetCombatState`.
    pub in_combat: bool,
    /// Creatures this one currently perceives.
    pub perceived: Vec<ObjectId>,
    /// Those of them it perceived as hostile (an appearance is announced again when a
    /// perceived creature turns hostile).
    #[serde(default)]
    pub perceived_hostile: Vec<ObjectId>,
    /// Threat by enemy, highest first (see [`crate::threat`]).
    #[serde(deserialize_with = "crate::threat::de_table")]
    pub threat: Vec<crate::threat::Threat>,
    /// What the script decided for the attack in progress (`SetAttackResult`).
    pub attack: Option<AttackResult>,
    /// `SetAttackDuration` for the attack in progress.
    pub attack_ms: Option<u64>,
    /// DA2's `SetAbilityResult`: the command and result captured for the later impact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ability_cast: Option<AbilityCast>,
    /// Milliseconds gathered towards the next regeneration pass (see `Sim::regenerate`).
    #[serde(default)]
    pub regen_ms: u64,
    /// `SetAimLoopDuration` for a ranged attack in progress: the time spent aiming before
    /// the shot, on top of the attack's own duration. Provisional.
    #[serde(default)]
    pub aim_ms: Option<u64>,
    /// Tactics slots (`SetTacticEntry`, or a preset).
    #[serde(default)]
    pub tactics: Vec<Tactic>,
    /// When each cooldown ends (simulation milliseconds), by `ability:item tag`.
    #[serde(default)]
    pub cooldowns: std::collections::BTreeMap<String, u64>,
    /// The last command that finished or failed (`GetPreviousCommand`).
    #[serde(default)]
    pub previous_command: Option<crate::world::Command>,
    /// Orders the player has given this creature (`Sim::order_*`), counted.
    #[serde(default)]
    pub player_orders: u64,
    /// When a walk of this creature's failed at a closed door: the count of player orders
    /// then (see [`crate::sim::Sim::fail_blocked`]).
    #[serde(default)]
    pub path_action_at: Option<u64>,
    /// `SetStealthEnabled`: the creature is in stealth (the stealth effect's handler sets
    /// and clears it).
    #[serde(default)]
    pub stealth: bool,
    /// Stealthed hostiles in range this creature has been told about (`PERCEPTION_APPEAR`
    /// with the stealthed flag), without perceiving them.
    #[serde(default)]
    pub noticed_stealthed: Vec<ObjectId>,
}

/// DA2's active ability context (native 0x4dd360 / 0x5b0380), separate from the
/// creature's mutable command result. The projectile target is a node selector.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AbilityCast {
    pub ability: i32,
    pub target: ObjectId,
    pub item: ObjectId,
    pub point: [f32; 3],
    pub result: i32,
    pub projectile_target: i32,
}

/// One tactics slot: when `condition` holds for a target of `target` type, do `command`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Tactic {
    pub enabled: bool,
    pub target: i32,
    pub condition: i32,
    pub command: i32,
    pub param: i32,
    /// The party member a per-follower target type names (`GetTacticTargetObject`; the
    /// tactics screen's `TargetID`).
    #[serde(default)]
    pub target_object: Option<ObjectId>,
    /// The party member a per-follower condition names (`GetTacticConditionObject`; the
    /// screen's `ConditionID`).
    #[serde(default)]
    pub condition_object: Option<ObjectId>,
    /// The item a use-item command uses (`GetTacticCommandItemTag` gives its tag; the
    /// screen's `CommandItem`).
    #[serde(default)]
    pub item: Option<ObjectId>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AttackResult {
    pub results: [i32; 2],
    pub effects: [Option<Effect>; 2],
}

/// How far from where it stands a creature whose eye is inside a solid may be taken to
/// stand for sight (see `Sim::update_perception`): the pathing cell it is in or the next
/// one, in metres. Provisional.
const STAND_ASIDE: f32 = 0.75;

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

impl Sim {
    /// How far a creature notices others, in metres: the `PrimaryRange` of its
    /// `PerceptionClass` row of ranges.gda (`PercepRngShrt` 10, `PercepRngMed` 30,
    /// `PercepRngLng` 40, `PercepRngMonster` 30, ...); a creature with none uses
    /// `PercepRngPlayer` if it is in the party, `PercepRngDefault` otherwise. Provisional: the
    /// table's `SecondaryRange` (20 for the medium and monster rows, 30 for the long ones) is
    /// not used, its meaning not recovered.
    pub fn perception_range(&self, id: ObjectId) -> f32 {
        let class = self
            .world
            .get(id)
            .and_then(|o| o.fields.get("PerceptionClass").copied())
            .filter(|&r| r > 0)
            .map(i64::from)
            .unwrap_or_else(|| {
                let party =
                    Some(id) == self.world.hero || crate::engine::party_of(self).contains(&id);
                if party {
                    PERCEPTION_PLAYER
                } else {
                    PERCEPTION_DEFAULT
                }
            });
        self.world
            .tables()
            .get(RANGES_TABLE, "")
            .and_then(|t| match t.cell("PrimaryRange", class) {
                Some(thedas_formats::gda::Cell::Float(v)) => Some(*v as f32),
                Some(thedas_formats::gda::Cell::Int(v)) => Some(*v as f32),
                _ => None,
            })
            .filter(|r| *r > 0.0)
            .unwrap_or(PERCEPTION_RANGE)
    }

    /// Whether two objects are hostile: both creatures, in groups set hostile.
    pub fn hostile(&self, a: ObjectId, b: ObjectId) -> bool {
        let (Some(x), Some(y)) = (self.world.get(a), self.world.get(b)) else {
            return false;
        };
        if x.kind != ObjectKind::Creature || y.kind != ObjectKind::Creature || a == b {
            return false;
        }
        let (g, h) = (x.group, y.group);
        g != h
            && self
                .world
                .hostility
                .get(&(g.min(h), g.max(h)))
                .copied()
                .unwrap_or(false)
    }

    pub(crate) fn alive(&self, id: ObjectId) -> bool {
        self.world
            .get(id)
            .is_some_and(|o| o.kind == ObjectKind::Creature && o.active && !o.dead)
    }

    /// Updates who sees whom in the hero's area and queues perception events.
    pub fn update_perception(&mut self) {
        let Some(area) = self
            .world
            .hero
            .and_then(|h| self.world.get(h))
            .and_then(|h| h.area)
        else {
            return;
        };
        let creatures: Vec<(ObjectId, [f32; 3])> = self
            .world
            .objects
            .values()
            .filter(|o| o.kind == ObjectKind::Creature && o.area == Some(area))
            .map(|o| (o.id, o.position))
            .collect();
        // What the pair test reads of each creature, looked up once a tick rather than once
        // a pair (the settled Ostagar camp has some hundreds of creatures): alive, group and
        // stealth, as `Sim::alive`, `Sim::hostile` and `Sim::hidden_from` read them.
        let facts: std::collections::HashMap<ObjectId, (bool, i32, bool)> = creatures
            .iter()
            .filter_map(|&(c, _)| {
                let o = self.world.get(c)?;
                Some((c, (o.active && !o.dead, o.group, o.combat.stealth)))
            })
            .collect();
        // (Perception only queues events, so none of this changes during the update.)
        let hostility = self.world.hostility.clone();
        let hostile = |a: ObjectId, b: ObjectId| -> bool {
            let (Some(&(_, g, _)), Some(&(_, h, _))) = (facts.get(&a), facts.get(&b)) else {
                return false;
            };
            a != b
                && g != h
                && hostility
                    .get(&(g.min(h), g.max(h)))
                    .copied()
                    .unwrap_or(false)
        };
        let hidden = |who: ObjectId, seen: ObjectId| {
            facts.get(&seen).is_some_and(|f| f.2) && hostile(who, seen)
        };
        // Closed doors block sight (see `Sim::door_sight`).
        let doors = self.door_sight(area);
        let blocked = |a: [f32; 3], b: [f32; 3]| doors.blocks(a, b);
        // Walls block noticing (`nav::wall_between`); once noticed, a creature is kept
        // track of round corners out to the visual radius.
        // A hostile's sight line is a ray through the level's collision from 1.8 m above
        // each one's feet (see `crate::collision`); without the level's collision files, the
        // pathing grid's walls stand in.
        let grid = self.nav_grid(area);
        let solid = self.collision(area).filter(|c| !c.is_empty());
        let props = solid
            .as_ref()
            .and_then(|_| self.placeable_collision(area))
            .filter(|c| !c.is_empty());
        // A creature whose eye is inside a solid of the level sees from the middle of the
        // pathing cell it stands in (or the nearest walkable one): the original's capsules
        // are kept out of the level's geometry, while ours stand anywhere in a cell, and a
        // cell that straddles a wall's end is partly inside the wall (see
        // `Collision::sight_blocked_standing`). Provisional.
        let stand = |p: [f32; 3]| -> [f32; 3] {
            let inside = solid.as_ref().is_some_and(|c| c.eye_inside(p))
                || props.as_ref().is_some_and(|c| c.eye_inside(p));
            if !inside {
                return p;
            }
            grid.as_ref()
                .and_then(|g| crate::nav::nearest_walkable_point(g, p))
                .filter(|q| ((q[0] - p[0]).powi(2) + (q[1] - p[1]).powi(2)).sqrt() <= STAND_ASIDE)
                .unwrap_or(p)
        };
        let wall = |a: [f32; 3], b: [f32; 3]| match &solid {
            Some(c) => {
                let (a, b) = (stand(a), stand(b));
                c.sight_blocked_standing(a, b)
                    || props
                        .as_ref()
                        .is_some_and(|p| p.sight_blocked_standing(a, b))
            }
            None => grid
                .as_ref()
                .is_some_and(|g| crate::nav::wall_between(g, a, b)),
        };
        for &(id, pos) in &creatures {
            let alive = self.alive(id);
            // A creature notices others within its perception range (see
            // `Sim::perception_range`) and keeps track of those it has noticed out to the
            // visual radius. DAOrigins.exe's perception update (0x6b89b0, from each
            // creature's tick at 0x6be7a4) notices a creature that is not hostile on range
            // alone (0x6b8873); only a hostile one needs a clear line of sight, a ray through
            // the level's collision from 1.8 m above each one's feet (0xb004d8; 0x67c530).
            // Keeping track (0x6a8d60) tests no sight, out to 60 m (0xadef18).
            // Provisional: closed doors (their own collision, `Sim::door_sight`) stand in for
            // the exe's room visibility for everyone; without the level's collision files a
            // wall of the pathing grid stands in for the ray, and the grid marks large
            // furniture and rubble the same way as walls, so those block hostiles too.
            let ring = self.perception_range(id);
            let seen = self
                .world
                .get(id)
                .map(|o| o.combat.perceived.clone())
                .unwrap_or_default();
            let reach = ring.max(VISUAL_RANGE);
            let in_view: Vec<ObjectId> = if alive {
                creatures
                    .iter()
                    .filter(|&&(o, p)| {
                        // Nothing beyond both ranges is seen; tested first as the cheapest.
                        let d = dist(pos, p);
                        if d > reach || o == id || !facts.get(&o).is_some_and(|f| f.0) {
                            return false;
                        }
                        let tracked = seen.contains(&o) && !hidden(id, o);
                        if tracked {
                            d <= VISUAL_RANGE && !blocked(pos, p)
                        } else {
                            d <= ring && !blocked(pos, p) && (!hostile(id, o) || !wall(pos, p))
                        }
                    })
                    .map(|&(o, _)| o)
                    .collect()
            } else {
                Vec::new()
            };
            // A stealthed hostile is not perceived: the creature is told it is there once
            // (PERCEPTION_APPEAR with the stealthed flag), and rules_core runs its own
            // detection check on it ("Perception check for ... detecting ...") without the AI
            // taking it on. Provisional: allies and neutrals see stealthed creatures, and the
            // notice is not repeated while the creature stays in range.
            let (stealthed, now): (Vec<ObjectId>, Vec<ObjectId>) =
                in_view.into_iter().partition(|&o| self.hidden_from(id, o));
            let noticed = self
                .world
                .get(id)
                .map(|o| o.combat.noticed_stealthed.clone())
                .unwrap_or_default();
            for &o in stealthed.iter().filter(|o| !noticed.contains(o)) {
                self.perceive_with(id, o, true, true, true);
            }
            if let Some(obj) = self.world.get_mut(id) {
                obj.combat.noticed_stealthed = stealthed;
            }
            let (before, before_hostile) = self
                .world
                .get(id)
                .map(|o| {
                    (
                        o.combat.perceived.clone(),
                        o.combat.perceived_hostile.clone(),
                    )
                })
                .unwrap_or_default();
            let now_hostile: Vec<ObjectId> = now
                .iter()
                .copied()
                .filter(|&o| self.hostile(id, o))
                .collect();
            if now == before && now_hostile == before_hostile {
                continue;
            }
            // A creature already in view whose hostility changes (a group turning hostile
            // mid-conversation, or surrendering) is announced again, with its new hostility:
            // the AI wakes for a new enemy, and rules_core ends combat when an appearance
            // leaves it seeing no hostiles. Provisional.
            for &o in now.iter().filter(|o| {
                !before.contains(o) || now_hostile.contains(o) != before_hostile.contains(o)
            }) {
                // The flag carries what the creature was seen as: rules_core takes "hostile
                // but no longer hostile" as the cue to check whether its fight is over.
                let flag = now_hostile.contains(&o) || before_hostile.contains(&o);
                self.perceive_flagged(id, o, true, flag);
            }
            for &o in before.iter().filter(|o| !now.contains(o)) {
                self.perceive(id, o, false);
            }
            if let Some(obj) = self.world.get_mut(id) {
                obj.combat.perceived = now;
                obj.combat.perceived_hostile = now_hostile;
            }
        }
        self.update_game_mode();
    }

    /// Keeps the harness's view of the fight (`World::party_fighting`: a standing party
    /// member perceives a living hostile) and its game-over flag. It does not change the game
    /// mode: in DAOrigins.exe the scripts do. rules_core asks the module for `GM_COMBAT`
    /// (event 1021, module_core calls `SetGameMode`) when a follower perceives a hostile or
    /// is attacked; player_core's perception-disappear handler starts the module's end-of-
    /// fight check (event 2000, after 3 s), which gets fallen members up and returns to
    /// `GM_EXPLORE`; player_core's death handler asks for `GM_DEAD` when the whole party has
    /// fallen. The engine only applies the change (`SetGameMode`, 0x65e4e0 -> 0x6414a0, the
    /// module told with `GAMEMODE_CHANGE` at 0x641e72-0x641f77) and sends
    /// `EVENT_TYPE_DEATH_RES_PARTY` only from the death screen (0x76e400). See
    /// OpenThedas-testdata/notes/combat-mode-trace.md.
    pub fn update_game_mode(&mut self) {
        let party = crate::engine::party_of(self);
        let fighting = party.iter().any(|&m| {
            self.alive(m)
                && self.world.get(m).is_some_and(|o| {
                    o.combat
                        .perceived
                        .iter()
                        .any(|&p| self.alive(p) && self.hostile(m, p))
                })
        });
        // The party AI holds followers back until the party is "clear to attack" (the
        // module's AI_PARTY_CLEAR_TO_ATTACK), which rules_core sets when the controlled
        // character attacks or uses an ability on a hostile. On autopilot nobody is controlled,
        // so the autopilot stands in for the player and opens every fight itself.
        if self.world.autopilot && fighting {
            if let Some(m) = self.world.module.and_then(|m| self.world.get_mut(m)) {
                m.locals.ints.insert("AI_PARTY_CLEAR_TO_ATTACK".into(), 1);
            }
        }
        self.world.party_fighting = fighting;
        // The whole party down is the end of the game: the scripts set `GM_DEAD` (the death
        // screen); the flag lets callers stop there.
        let standing = party.iter().any(|&m| self.alive(m));
        let dead_mode = self.table.constant("GM_DEAD").and_then(|c| match &c.value {
            thedas_script::ldf::ConstValue::Int(v) => Some(*v as i32),
            _ => None,
        });
        let over = (!party.is_empty() && !standing) || Some(self.world.game_mode) == dead_mode;
        if over && !self.world.game_over {
            self.world.game_over = true;
            let t = self.world.time_ms;
            self.world.trace.push(crate::trace::TraceEntry::Log {
                t,
                channel: -1,
                text: "the whole party has fallen: game over".into(),
            });
        }
    }

    /// Recovery, as DAOrigins.exe ticks it: each creature's update (0x6be829) passes its
    /// stats the elapsed milliseconds and whether it is in combat (its combat state,
    /// `GetCombatState`: `*(creature + 0xc60) == 1`, as 0x6a2580 reads it). The stats
    /// (0x71b0e0) add the time to their own clock (+0x58) and, while it is past the period
    /// (2000 ms in combat, 3000 ms out of it; 0x71b0f5, 0x71b101), take a period off and,
    /// when the creature's health is above 0 (0x71b1d2), run one regeneration pass
    /// (0x795ba0): every depletable property adds the total of its regeneration property
    /// once (0x79cad0 -> 0x79c4a0), the out-of-combat one (`properties.gda` column
    /// 0x7a3df160, read at 0x796620: `Regeneration_Health` 14 for health,
    /// `Regeneration_Stamina` 29 for mana/stamina) or in combat the combat one (column
    /// 0xe24b074d, 0x79663c: 28 and 30). So a rate is an amount per pass, not per second,
    /// and outside combat only the creature's own combat state counts, not the game mode.
    /// The current value stays within 0 and the property's maximum.
    /// A creature's current mana or stamina (`PROPERTY_DEPLETABLE_MANA_STAMINA`).
    pub(crate) fn mana_now(&self, id: ObjectId) -> f32 {
        self.world
            .get(id)
            .and_then(|o| o.properties.get(&8))
            .map_or(0.0, |v| v.1)
    }

    /// After a change to `id`'s mana or stamina: when it went from above 0 to 0 or below,
    /// the creature hears `EVENT_TYPE_MANA_STAM_DEPLETED` (64) from itself, with no other
    /// payload, as DAOrigins.exe sends it (0x6afb90, from every change of property 8 that
    /// goes through 0x719a00: regeneration and the property setters). Provisional: the
    /// exe's per-creature enable check for the event (0x698490) is not modelled.
    pub(crate) fn note_mana_change(&mut self, id: ObjectId, before: f32) {
        if before > 0.0
            && self.mana_now(id) <= 0.0
            && self
                .world
                .get(id)
                .is_some_and(|o| o.kind == ObjectKind::Creature)
        {
            let ev = self.new_event("MANA_STAM_DEPLETED", Some(id));
            self.world.queue_event(0, id, ev, None);
        }
    }

    pub fn regenerate(&mut self, ms: u64) {
        if ms == 0 {
            return;
        }
        let (health, mana) = (
            constant_of(self, "PROPERTY_DEPLETABLE_HEALTH"),
            constant_of(self, "PROPERTY_DEPLETABLE_MANA_STAMINA"),
        );
        // Each depletable's regeneration properties: out of combat, in combat.
        let regen = |sim: &Sim, depletable: i32| -> [i32; 2] {
            let t = sim.world.tables.get(0, "properties");
            let col = |hash: u32| {
                t.as_ref()
                    .and_then(|t| match t.cell_by_hash(hash, depletable as i64)? {
                        thedas_formats::gda::Cell::Int(v) => Some(*v as i32),
                        _ => None,
                    })
                    .unwrap_or(0)
            };
            [col(COL_REGEN), col(COL_REGEN_COMBAT)]
        };
        let rates = [(health, regen(self, health)), (mana, regen(self, mana))];
        let living: Vec<ObjectId> = self
            .world
            .objects
            .values()
            .filter(|o| o.kind == ObjectKind::Creature && !o.dead)
            .map(|o| o.id)
            .collect();
        for id in living {
            let Some(o) = self.world.objects.get_mut(&id) else {
                continue;
            };
            let combat = o.combat.in_combat;
            let period = if combat {
                REGEN_PERIOD_COMBAT_MS
            } else {
                REGEN_PERIOD_MS
            };
            o.combat.regen_ms += ms;
            let mut passes = 0;
            while o.combat.regen_ms > period {
                o.combat.regen_ms -= period;
                passes += 1;
            }
            for _ in 0..passes {
                let alive = self
                    .world
                    .get(id)
                    .and_then(|o| o.properties.get(&health))
                    .is_some_and(|v| v.1 > 0.0);
                if !alive {
                    break;
                }
                for (p, ids) in rates {
                    let r = ids[combat as usize];
                    if r <= 0 {
                        continue;
                    }
                    let rate = self.property_total(id, r);
                    if rate == 0.0 {
                        continue;
                    }
                    let max = self.depletable_max(id, p);
                    let before = self.mana_now(id);
                    if let Some(v) = self
                        .world
                        .get_mut(id)
                        .and_then(|o| o.properties.get_mut(&p))
                    {
                        if v.1 < max || rate < 0.0 {
                            v.1 = (v.1 + rate).clamp(0.0, max);
                        }
                    }
                    if p == mana {
                        self.note_mana_change(id, before);
                    }
                }
            }
        }
    }

    /// Queues `PERCEPTION_APPEAR` (or `DISAPPEAR`) for `who` about `seen`.
    pub fn perceive(&mut self, who: ObjectId, seen: ObjectId, appear: bool) {
        let hostile = self.hostile(who, seen);
        self.perceive_flagged(who, seen, appear, hostile);
    }

    /// As [`Sim::perceive`], with the hostility flag (integer 2) given.
    pub fn perceive_flagged(&mut self, who: ObjectId, seen: ObjectId, appear: bool, hostile: bool) {
        self.perceive_with(who, seen, appear, hostile, false);
    }

    /// As [`Sim::perceive_flagged`], with the stealthed flag (integer 1) given.
    pub fn perceive_with(
        &mut self,
        who: ObjectId,
        seen: ObjectId,
        appear: bool,
        hostile: bool,
        stealthed: bool,
    ) {
        let name = if appear {
            "PERCEPTION_APPEAR"
        } else {
            "PERCEPTION_DISAPPEAR"
        };
        let mut ev = self.new_event(name, Some(seen));
        ev.objects = vec![seen];
        ev.ints = vec![0, stealthed as i32, hostile as i32];
        self.world.queue_event(0, who, ev, None);
    }

    /// Whether `seen` is in stealth and hidden from `who` (a creature it is hostile to).
    pub fn hidden_from(&self, who: ObjectId, seen: ObjectId) -> bool {
        self.world.get(seen).is_some_and(|o| o.combat.stealth) && self.hostile(who, seen)
    }

    /// Runs a target's `APPLY_EFFECT` handler for an effect, keeps it if it lasts, and
    /// checks for death afterwards.
    pub fn apply_effect(&mut self, target: ObjectId, effect: Effect) {
        if self.world.get(target).is_none() {
            return;
        }
        let creator = effect.creator;
        let t = self.world.time_ms;
        self.world.trace.push(crate::trace::TraceEntry::Log {
            t,
            channel: -4,
            text: format!(
                "effect {} on {target} from {:?} ({:?})",
                effect.ty, creator, effect.floats
            ),
        });
        let health = constant_of(self, "PROPERTY_DEPLETABLE_HEALTH");
        let health_of = |s: &Sim| {
            s.world
                .get(target)
                .filter(|o| !o.dead)
                .and_then(|o| o.properties.get(&health))
                .map_or(0.0, |v| v.1.max(0.0))
        };
        let before = health_of(self);
        let (ability, ty) = (effect.ability, effect.ty);
        let mut ev = self.new_event("APPLY_EFFECT", creator);
        ev.objects = vec![target];
        let saved = self.current_effect.replace((effect.clone(), true));
        let saved_target = self.current_effect_target.replace(target);
        let script = self.world.get(target).and_then(|o| o.script.clone());
        if let Some(script) = script {
            self.run_script(&script, target, Some(ev));
        }
        let (effect, valid) =
            std::mem::replace(&mut self.current_effect, saved).unwrap_or((effect, true));
        self.current_effect_target = saved_target;
        // A lethal hit arrives as EFFECT_TYPE_DEATH (7 in effects.gda) rather than as damage
        // that empties the health bar: the engine does the killing.
        if valid && effect.ty == EFFECT_TYPE_RESURRECTION {
            if let Some(o) = self.world.get_mut(target) {
                o.dead = false;
                o.died_at = None;
            }
            return;
        }
        // Health the scripts take is recorded where they set it (`record_health_loss`); a
        // killing blow arrives as EFFECT_TYPE_DEATH and takes what was left.
        let lost = if valid && effect.ty == EFFECT_TYPE_DEATH {
            before
        } else {
            0.0
        };
        if lost > 0.0 {
            self.world.trace.push(crate::trace::TraceEntry::Damage {
                t,
                loss: crate::trace::HealthLoss {
                    source: creator.unwrap_or(OBJECT_INVALID),
                    target,
                    ability,
                    effect: ty,
                    amount: lost,
                },
            });
        }
        if valid && effect.ty == EFFECT_TYPE_DEATH {
            self.kill(target, creator.unwrap_or(OBJECT_INVALID));
            return;
        }
        let instant = constant_of(self, "EFFECT_DURATION_TYPE_INSTANT");
        if valid && effect.duration_type != instant {
            if let Some(o) = self.world.get_mut(target) {
                o.effects.push(effect);
            }
        }
        self.check_death(target, creator.unwrap_or(OBJECT_INVALID));
    }

    /// Records health a script took from `target` (`TraceEntry::Damage`): the source is the
    /// creator of the effect being applied to it, if any, else the script's owner (a weapon
    /// hit is applied by the attacker's own script); the ability is the effect's (0 for a
    /// weapon hit). Health the module's scripts set outside an effect is not recorded.
    ///
    /// Damage over time is the exception: rules_core's tick for it runs in the burning
    /// creature's own script and lowers its health directly, with no effect being applied,
    /// so the owner would be credited with its own burn. When a creature's own script takes
    /// its health outside an effect and it carries an `EFFECT_TYPE_DOT` (1017) from someone
    /// else, the loss is credited to that effect's creator and ability (the latest such
    /// effect when there are several). Attribution only: nothing in the simulation reads it.
    pub(crate) fn record_health_loss(&mut self, caller: ObjectId, target: ObjectId, amount: f32) {
        if amount <= 0.0 {
            return;
        }
        let effect = self
            .current_effect
            .as_ref()
            .filter(|_| self.current_effect_target == Some(target))
            .map(|(e, _)| e);
        // The module's own scripts set health when they get the fallen up after a fight
        // (DEATH_RES_PARTY); that is no hit.
        if effect.is_none() && Some(caller) == self.world.module {
            return;
        }
        let dot = effect
            .is_none()
            .then(|| self.world.get(target))
            .flatten()
            .filter(|_| caller == target)
            .and_then(|o| {
                o.effects
                    .iter()
                    .rev()
                    .find(|e| e.ty == EFFECT_TYPE_DOT && e.creator.is_some_and(|c| c != target))
            });
        let effect = effect.or(dot);
        let source = effect.and_then(|e| e.creator).unwrap_or(caller);
        let (ability, ty) = effect.map_or((0, 0), |e| (e.ability, e.ty));
        let t = self.world.time_ms;
        self.world.trace.push(crate::trace::TraceEntry::Damage {
            t,
            loss: crate::trace::HealthLoss {
                source,
                target,
                ability,
                effect: ty,
                amount,
            },
        });
    }

    /// Takes an effect off a creature and tells its script (`EVENT_TYPE_REMOVE_EFFECT`, with
    /// the effect as the current one), which undoes what applying it did (a property
    /// modifier's change, for instance). Returns whether the creature had it.
    pub fn remove_effect(&mut self, target: ObjectId, effect: &Effect) -> bool {
        let removed = match self.world.get_mut(target) {
            Some(o) => match o.effects.iter().position(|x| x == effect) {
                Some(i) => Some(o.effects.remove(i)),
                None => None,
            },
            None => None,
        };
        let Some(effect) = removed else { return false };
        // An area of effect goes with its effect (DAOrigins.exe 0x7a0520).
        let aoe = (effect.ty == crate::aoe::EFFECT_TYPE_AOE)
            .then(|| effect.objects.first().copied())
            .flatten();
        let mut ev = self.new_event("REMOVE_EFFECT", effect.creator);
        ev.objects = vec![target];
        let saved = self.current_effect.replace((effect, true));
        let saved_target = self.current_effect_target.replace(target);
        let script = self.world.get(target).and_then(|o| o.script.clone());
        if let Some(script) = script {
            self.run_script(&script, target, Some(ev));
        }
        self.current_effect = saved;
        self.current_effect_target = saved_target;
        if let Some(a) = aoe {
            self.destroy_aoe(a);
        }
        true
    }

    /// Removes temporary effects whose time is up.
    pub fn expire_effects(&mut self) {
        let now = self.world.time_ms;
        let due: Vec<(ObjectId, Effect)> = self
            .world
            .objects
            .values()
            .flat_map(|o| {
                o.effects
                    .iter()
                    .filter(|e| e.expires.is_some_and(|t| t <= now))
                    .map(move |e| (o.id, e.clone()))
            })
            .collect();
        for (id, e) in due {
            self.remove_effect(id, &e);
        }
    }

    /// Whether a modal ability (a sustained talent or spell) is on: the creature carries an
    /// effect from it, such as its upkeep (`EFFECT_TYPE_UPKEEP`, ability in integer 1).
    /// Provisional.
    pub fn modal_active(&self, id: ObjectId, ability: i32) -> bool {
        const EFFECT_TYPE_UPKEEP: i32 = 1016;
        ability != 0
            && self.world.get(id).is_some_and(|o| {
                o.effects.iter().any(|e| {
                    e.ability == ability
                        || (e.ty == EFFECT_TYPE_UPKEEP && e.ints.get(1) == Some(&ability))
                })
            })
    }

    /// The modal abilities a creature has on, in the order their upkeep effects were applied
    /// (see [`Sim::modal_active`]).
    pub fn active_modal_abilities(&self, id: ObjectId) -> Vec<i32> {
        const EFFECT_TYPE_UPKEEP: i32 = 1016;
        let mut v: Vec<i32> = Vec::new();
        if let Some(o) = self.world.get(id) {
            for e in o.effects.iter().filter(|e| e.ty == EFFECT_TYPE_UPKEEP) {
                if let Some(&a) = e.ints.get(1).filter(|&&a| a != 0) {
                    if !v.contains(&a) {
                        v.push(a);
                    }
                }
            }
        }
        v
    }

    /// Whether a creature meets an ability's use conditions (`ABI_base` column hashed
    /// 0xb1caee1d) among those in `mask`, as DAOrigins.exe checks them (0x71c310, reached
    /// from `GetConditionedAbilities` 0x6743a0 through 0x71dd10 with no reason wanted):
    ///
    /// - weapon bits, by the `bitm_base.gda` `Type` of the main-hand and off-hand items
    ///   (2 melee, 3 shield, 6 ranged): 0x4 a ranged main weapon (the others are then not
    ///   checked); otherwise 0x1 a melee weapon in either hand or none in the main hand, 0x2 a
    ///   shield, 0x40 melee weapons in both hands, each overriding the one before; 0x80 a
    ///   two-handed melee weapon decides on its own;
    /// - 0xe00, by `CurrentClass` (0x71b010 reads property 27): a warrior needs 0x800, a mage
    ///   0x200, a rogue 0x400;
    /// - 0x10: the modal ability in column 0x07342ea2 must be on (0x66dd60).
    ///
    /// Provisional: "two-handed" is the simulation's weapon style (the executable compares
    /// two item fields, 0xa664c0, not traced), and 0x100 (column 0xc7e5bb24 against the
    /// appearance's flags, 0x71a210; only the mabari crunch items use it) is not checked.
    pub fn ability_conditions_met(&self, id: ObjectId, ability: i32, mask: i32) -> bool {
        const COL_CONDITIONS: u32 = 0xb1ca_ee1d;
        const COL_REQUIRED_MODAL: u32 = 0x0734_2ea2;
        let tables = self.world.tables();
        let abi = tables.get(0, "ABI_base");
        let col = |h: u32| -> i32 {
            match abi.as_ref().and_then(|t| t.cell_by_hash(h, ability as i64)) {
                Some(thedas_formats::gda::Cell::Int(v)) => *v as i32,
                Some(thedas_formats::gda::Cell::Float(v)) => *v as i32,
                _ => 0,
            }
        };
        let c = col(COL_CONDITIONS) & mask;
        if c == 0 {
            return true;
        }
        let Some(o) = self.world.get(id) else {
            return true;
        };
        let item_type = |slot: i32| -> i32 {
            o.equipment
                .get(&slot)
                .and_then(|i| self.world.get(*i))
                .and_then(|i| i.fields.get("BaseItem").copied())
                .and_then(|b| match tables.get(6, "")?.cell("Type", b as i64)? {
                    thedas_formats::gda::Cell::Int(v) => Some(*v as i32),
                    _ => None,
                })
                .unwrap_or(0)
        };
        if c & 0xc7 != 0 {
            let (main, off) = (item_type(0), item_type(1));
            let ok = if c & 0x80 != 0 {
                main == 2 && self.weapon_style(id) == constant_of(self, "WEAPONSTYLE_TWOHANDED")
            } else if c & 0x4 != 0 {
                main == 6
            } else {
                let mut ok = true;
                if c & 0x1 != 0 {
                    ok = main == 2 || off == 2 || main == 0;
                }
                if c & 0x2 != 0 {
                    ok = off == 3;
                }
                if c & 0x40 != 0 {
                    ok = off == 2 && main == 2;
                }
                ok
            };
            if !ok {
                return false;
            }
        }
        if c & 0xe00 != 0 {
            let class = self.property_total(id, 27) as i32;
            let needs = match class {
                1 => 0x800,
                2 => 0x200,
                3 => 0x400,
                _ => 0,
            };
            if needs != 0 && c & needs == 0 {
                return false;
            }
        }
        if c & 0x10 != 0 {
            let required = col(COL_REQUIRED_MODAL);
            if required != 0 && !self.modal_active(id, required) {
                return false;
            }
        }
        true
    }

    /// `GetConditionedAbilities`: the modal abilities a creature has on whose conditions in
    /// `mask` it no longer meets (DAOrigins.exe 0x6743a0, 0x71dd10). player_core asks on
    /// equipping and unequipping and turns them off.
    pub fn conditioned_abilities(&self, id: ObjectId, mask: i32) -> Vec<i32> {
        self.active_modal_abilities(id)
            .into_iter()
            .filter(|&a| !self.ability_conditions_met(id, a, mask))
            .collect()
    }

    /// Whether creature `id` is a corpse from its creation: designers place one with the
    /// local `CREATURE_SPAWN_DEAD` at 1, creature_core's `SPAWN` handler then leaves it
    /// uninitialised (it only sets its gore), and it has no health.
    ///
    /// In DAOrigins.exe a creature is dead when its health is 0 or less (`IsDead`, 0x65a3a0
    /// -> 0x695300, through the creature's current-health getter 0x6ab4b0), and a creature
    /// without health counts 0 (0x6ab4b0 returns 0.0 when it has no stats or no property 7).
    /// The engine treats these creatures as corpses: its corpse predicate (0x6a3ad0, reading
    /// the local at 0x6a3b47) and a nearby-creature query (0x73f1cd) single them out, and
    /// its health-change handler (0x6bc4b0) clears the local only when health rises from 0
    /// or less to above 0 (0x6bc5fb-0x6bc631), that is when one is brought back.
    ///
    /// Provisional: the simulation has no property table for creatures the scripts never
    /// set up, so it cannot derive death from missing health in general (non-combatants
    /// have none either, and stay alive here); the local stands in for it for the creatures
    /// placed dead.
    pub fn spawns_dead(&self, id: ObjectId) -> bool {
        let health = constant_of(self, "PROPERTY_DEPLETABLE_HEALTH");
        self.world.get(id).is_some_and(|o| {
            o.kind == ObjectKind::Creature
                && o.locals.ints.get("CREATURE_SPAWN_DEAD") == Some(&1)
                && o.properties
                    .get(&health)
                    .is_none_or(|&(_, current)| current <= 0.0)
        })
    }

    /// Lays a creature that [`Sim::spawns_dead`] down dead, without a death: no event and no
    /// time of death (it shows the end of its death animation).
    pub fn lay_out_if_spawned_dead(&mut self, id: ObjectId) {
        if self.spawns_dead(id)
            && let Some(o) = self.world.get_mut(id)
        {
            o.dead = true;
            o.died_at = None;
        }
    }

    /// Kills a creature whose health ran out.
    pub fn check_death(&mut self, id: ObjectId, killer: ObjectId) {
        let health = constant_of(self, "PROPERTY_DEPLETABLE_HEALTH");
        let Some(o) = self.world.get(id) else { return };
        if o.kind != ObjectKind::Creature || o.dead || o.immortal || o.plot {
            return;
        }
        let Some(&(_, current)) = o.properties.get(&health) else {
            return;
        };
        if current > 0.0 {
            return;
        }
        self.kill(id, killer);
    }

    /// Marks a creature dead, clears what it was doing and sends it `EVENT_TYPE_DEATH`.
    pub fn kill(&mut self, id: ObjectId, killer: ObjectId) {
        let health = constant_of(self, "PROPERTY_DEPLETABLE_HEALTH");
        let now = self.world.time_ms;
        if let Some(o) = self.world.get_mut(id) {
            if o.dead {
                return;
            }
            o.dead = true;
            o.died_at = Some(now);
            // The fallen have no health left (a killing blow arrives as EFFECT_TYPE_DEATH,
            // not as damage that empties the bar).
            if let Some(v) = o.properties.get_mut(&health) {
                v.1 = 0.0;
            }
            o.commands.clear();
            o.combat.attack = None;
            o.combat.in_combat = false;
        }
        let mut ev = self.new_event("DEATH", Some(killer));
        ev.objects = vec![killer];
        self.world.queue_event(0, id, ev, None);
    }

    /// Fills a creature's tactics slots from a preset: the preset's row in
    /// `tacticsuserpresets.gda` names an M2DA table (`TacticsTable`, e.g. 2001 for Tank) whose
    /// rows are the slots in order (target type, condition, command, parameter), numbered from
    /// 1 as the scripts number them (slot 0 is unused). Provisional.
    pub fn apply_tactic_preset(&mut self, id: ObjectId, preset: i32) -> bool {
        let Some(slots) = self.preset_tactics(preset) else {
            return false;
        };
        match self.world.get_mut(id) {
            Some(o) => {
                o.combat.tactics = slots;
                o.fields.insert("TacticPreset".into(), preset);
                true
            }
            None => false,
        }
    }

    /// The slots a preset gives (see [`Sim::apply_tactic_preset`]), slot 0 unused.
    /// Provisional.
    pub fn preset_tactics(&self, preset: i32) -> Option<Vec<Tactic>> {
        let tables = self.world.tables.clone();
        let presets = tables.get(0, "tacticsuserpresets")?;
        let table_id = match presets.cell("TacticsTable", preset as i64) {
            Some(thedas_formats::gda::Cell::Int(v)) => *v as i32,
            _ => return None,
        };
        let t = tables.get(table_id, "")?;
        let int = |row: i64, col: &str| match t.cell(col, row) {
            Some(thedas_formats::gda::Cell::Int(v)) => *v as i32,
            _ => 0,
        };
        // The parameter column's name is not known; it is the fifth column.
        let param_hash = t.columns.iter().find(|(_, i)| **i == 4).map(|(h, _)| *h);
        let slots: Vec<Tactic> = std::iter::once(Tactic::default())
            .chain(t.order.iter().map(|&row| {
                Tactic {
                    enabled: true,
                    target: int(row, "TargetType"),
                    condition: int(row, "Condition"),
                    command: int(row, "Command"),
                    param: param_hash
                        .and_then(|h| t.cell_by_hash(h, row))
                        .map_or(0, |c| match c {
                            thedas_formats::gda::Cell::Int(v) => *v as i32,
                            _ => 0,
                        }),
                    ..Default::default()
                }
            }))
            .collect();
        Some(slots)
    }

    /// A creature's tactics as [`Sim::tactics_of`] gives them, without applying a preset:
    /// the slots of the preset it would apply when it has none of its own.
    pub fn tactics_shown(&self, id: ObjectId) -> Vec<Tactic> {
        let Some(o) = self.world.get(id) else {
            return Vec::new();
        };
        if !o.combat.tactics.is_empty() {
            return o.combat.tactics.clone();
        }
        o.fields
            .get("TacticPreset")
            .or_else(|| o.fields.get("TacticPreset.0"))
            .and_then(|&p| self.preset_tactics(p))
            .unwrap_or_default()
    }

    /// A creature's tactics, applying its first added preset if it has none yet (followers
    /// get presets with `AddTacticPresetID` and no slots of their own). Provisional.
    pub fn tactics_of(&mut self, id: ObjectId) -> Vec<Tactic> {
        let (empty, first) = match self.world.get(id) {
            Some(o) => (
                o.combat.tactics.is_empty(),
                o.fields
                    .get("TacticPreset")
                    .or_else(|| o.fields.get("TacticPreset.0"))
                    .copied(),
            ),
            None => return Vec::new(),
        };
        if empty {
            if let Some(p) = first {
                self.apply_tactic_preset(id, p);
            }
        }
        self.world
            .get(id)
            .map(|o| o.combat.tactics.clone())
            .unwrap_or_default()
    }

    /// Makes `set` (0 or 1) a creature's active weapon set, swapping the weapon slots with
    /// the inactive set's; any other value (`INVALID_WEAPON_SET`) takes the next set. As
    /// DAOrigins.exe's creature switch (0x6a8fa0) does for both `SwitchWeaponSet` (0x65ccc0)
    /// and the `CommandSwitchWeaponSet` command's executor (0x7a1ed0). Provisional: no equip
    /// events are sent for the swap.
    pub fn switch_weapon_set(&mut self, id: ObjectId, set: i32) {
        let Some(o) = self.world.get_mut(id) else {
            return;
        };
        let now = o.fields.get("ActiveWeaponSet").copied().unwrap_or(0);
        let to = match set {
            s @ (0 | 1) => s,
            _ => 1 - now,
        };
        if to != now {
            for slot in crate::world::WEAPON_SET_SLOTS {
                let held = o.equipment.remove(&slot);
                if let Some(i) = o.other_set.remove(&slot) {
                    o.equipment.insert(slot, i);
                }
                if let Some(i) = held {
                    o.other_set.insert(slot, i);
                }
            }
        }
        o.fields.insert("ActiveWeaponSet".into(), to);
    }

    /// Orders a creature to attack another, as the player does by clicking an enemy.
    pub fn order_attack(&mut self, attacker: ObjectId, target: ObjectId) {
        self.note_player_order(attacker);
        let mut cmd = crate::world::Command {
            ty: constant_of(self, "COMMAND_TYPE_ATTACK"),
            name: "CommandAttack".into(),
            target: Some(target),
            location: None,
            seconds: 0.0,
            ints: Vec::new(),
            strings: Vec::new(),
            due: None,
            from: None,
            approach: None,
            ring_wait: false,
            after_impact_ms: None,
            impacted: false,
            arrived: false,
            path: Vec::new(),
        };
        self.da2_default_attack_command(&mut cmd);
        if let Some(o) = self.world.get_mut(attacker) {
            o.commands.clear();
            o.commands.push(cmd);
            o.combat.in_combat = true;
        }
    }

    /// DA2 CommandAttack (0x4dda50) constructs modern command 25 with default
    /// ability 1. Its forced legacy result is not an ability argument.
    pub(crate) fn da2_default_attack_command(&self, cmd: &mut crate::world::Command) {
        if self.game == thedas_vfs::Game::DragonAge2 && cmd.name == "CommandAttack" {
            cmd.name = "CommandUseAbility".into();
            cmd.ty = constant_of(self, "COMMAND_TYPE_USE_ABILITY");
            cmd.ints = vec![1];
        }
    }

    /// Native modern command preparation (0x640d6d): ability 1 asks the owner's
    /// script to choose the actual basic attack. The shipped ability_core handles
    /// CHOOSE_BASIC_ATTACK and returns its choice in the creature's CommandResult.
    /// Keep selection in that script: it owns range, arc and combo-table rules.
    pub(crate) fn choose_da2_basic_attack(
        &mut self,
        id: ObjectId,
        cmd: &mut crate::world::Command,
    ) -> bool {
        if self.game != thedas_vfs::Game::DragonAge2
            || cmd.name != "CommandUseAbility"
            || cmd.ints.first() != Some(&1)
        {
            return true;
        }
        let target = cmd.target.unwrap_or(OBJECT_INVALID);
        // The legacy attack path already turns before its scripts run. Keep
        // that behavior when DA2 default attacks take the modern ability path:
        // the authored selector and close-attack damage both test the arc.
        self.face_target(id, target);
        let point = self
            .world
            .get(target)
            .map(|o| o.position)
            .or_else(|| cmd.location.map(|l| l.position))
            .unwrap_or([0.0; 3]);
        let mut ev = self.new_event("CHOOSE_BASIC_ATTACK", Some(id));
        ev.objects = vec![id, target, OBJECT_INVALID];
        // Integer 2 is native command boolean 2. The attack constructor leaves it
        // false. The sim has no separate alternate/manual combo input yet.
        ev.ints = vec![0, 0, 0];
        ev.vectors = vec![point];
        let script = self.world.get_mut(id).and_then(|o| {
            o.fields.insert("CommandResult".into(), 0);
            o.script.clone()
        });
        if let Some(script) = script {
            self.run_script(&script, id, Some(ev));
        }
        let choice = self
            .world
            .get(id)
            .and_then(|o| o.fields.get("CommandResult"))
            .copied()
            .unwrap_or(0);
        if choice <= 1 {
            return false;
        }
        cmd.ints[0] = choice;
        if let Some(area) = self.world.get(id).and_then(|o| o.area) {
            cmd.location = Some(crate::world::Location {
                area,
                position: point,
                facing: 0.0,
            });
        }
        true
    }

    /// On autopilot, during a fight: a party member's AI round runs when its command
    /// completes, so a hero left with nothing to do (its round chose nothing, or it never had
    /// one) stands for a second, after which its round comes. Called once a tick's events
    /// are delivered, so that a pending `COMMAND_COMPLETE` gets its round first.
    pub(crate) fn autopilot_idle_hero(&mut self) {
        if !self.world.autopilot || !self.world.party_fighting {
            return;
        }
        if let Some(hero) = self.world.hero.filter(|&h| self.alive(h)) {
            self.order_wait(hero, 1.0);
        }
    }

    /// Queues a `CommandWait` for a creature that has nothing to do, as a player does by
    /// letting the controlled character stand. When it ends the owner hears
    /// `COMMAND_COMPLETE`, which runs its AI round when its party AI is on.
    pub fn order_wait(&mut self, who: ObjectId, seconds: f32) {
        let cmd = crate::world::Command {
            ty: constant_of(self, "COMMAND_TYPE_WAIT"),
            name: "CommandWait".into(),
            target: None,
            location: None,
            seconds,
            ints: Vec::new(),
            strings: Vec::new(),
            due: None,
            from: None,
            approach: None,
            ring_wait: false,
            after_impact_ms: None,
            impacted: false,
            arrived: false,
            path: Vec::new(),
        };
        if let Some(o) = self.world.get_mut(who).filter(|o| o.commands.is_empty()) {
            o.commands.push(cmd);
        }
    }

    /// The item a `CommandUseAbility` uses: the item tagged as the command's string 0, the
    /// owner's own or else one in the party's pack ([`Sim::party_item`]), or
    /// `OBJECT_INVALID`.
    fn ability_item(&self, id: ObjectId, cmd: &crate::world::Command) -> ObjectId {
        let Some(tag) = cmd.strings.first().filter(|t| !t.is_empty()) else {
            return OBJECT_INVALID;
        };
        self.party_item(id, tag).unwrap_or(OBJECT_INVALID)
    }

    /// Native SetAbilityResult (0x4dd360): copy the front command's integer 0,
    /// objects 0/1 and vector 0, plus the creature's current command result. The
    /// result must survive other commands changing that creature field before impact.
    pub(crate) fn remember_ability_result(&mut self, id: ObjectId, projectile_target: i32) {
        let Some(o) = self
            .world
            .get(id)
            .filter(|o| o.kind == ObjectKind::Creature)
        else {
            return;
        };
        let Some(cmd) = o.commands.first() else {
            return;
        };
        let cast = AbilityCast {
            ability: cmd.ints.first().copied().unwrap_or(0),
            target: cmd.target.unwrap_or(OBJECT_INVALID),
            item: self.ability_item(id, cmd),
            point: cmd.location.map(|l| l.position).unwrap_or([0.0; 3]),
            result: o.fields.get("CommandResult").copied().unwrap_or(0),
            projectile_target,
        };
        self.world.get_mut(id).unwrap().combat.ability_cast = Some(cast);
    }

    /// The front `CommandUseAbility` started: the owner's script vets it
    /// (`COMMAND_PENDING`: objects owner, target, item; integers command type, ability), then
    /// `ability_core` hears `ABILITY_CAST_START`.
    /// Provisional: the pending event's third object is inferred from `ability_core`, which
    /// reads it as the item.
    pub(crate) fn start_ability(&mut self, id: ObjectId, cmd: &crate::world::Command) {
        if self.game == thedas_vfs::Game::DragonAge2
            && let Some(o) = self.world.get_mut(id)
        {
            o.combat.ability_cast = None;
        }
        let target = cmd.target.unwrap_or(OBJECT_INVALID);
        let ability = cmd.ints.first().copied().unwrap_or(0);
        let item = self.ability_item(id, cmd);
        let mut ev = self.new_event("COMMAND_PENDING", Some(id));
        ev.objects = vec![id, target, item];
        ev.ints = vec![cmd.ty, ability];
        let script = self.world.get(id).and_then(|o| o.script.clone());
        if let Some(s) = &script {
            self.run_script(s, id, Some(ev));
        }
        let still = self
            .world
            .get(id)
            .and_then(|o| o.commands.first())
            .is_some_and(|c| c.name == cmd.name && c.ints == cmd.ints && c.target == cmd.target);
        if !still {
            return;
        }
        // Native modern preprocessor 0x6b6cd8 only schedules the executor for
        // a positive pending result. Failed resource/target checks must not cast.
        if self.game == thedas_vfs::Game::DragonAge2
            && self
                .world
                .get(id)
                .and_then(|o| o.fields.get("CommandResult"))
                .copied()
                .unwrap_or(0)
                <= 0
        {
            self.fail_front_command(id, "COMMAND_FAILED");
            return;
        }
        self.ability_event("ABILITY_CAST_START", id, target, ability, cmd);
    }

    /// `ABILITY_CAST_START` or `ABILITY_CAST_IMPACT` to `ability_core`, which the original
    /// executable names directly (the rules scripts do not handle these events): objects
    /// caster, item, target. DA2's result and vector come from its native active context;
    /// Origins' trailing integer zeros remain provisional.
    fn ability_event(
        &mut self,
        name: &str,
        id: ObjectId,
        target: ObjectId,
        ability: i32,
        cmd: &crate::world::Command,
    ) {
        let item = self.ability_item(id, cmd);
        let mut ev = self.new_event(name, Some(id));
        ev.objects = vec![id, item, target];
        ev.ints = vec![ability, 0, 0, 0];
        if self.game == thedas_vfs::Game::DragonAge2 {
            self.da2_ability_event(&mut ev, name, id, cmd);
        }
        ev.strings = cmd.strings.clone();
        if let Some(l) = cmd.location {
            ev.locations = vec![l];
        }
        let t = self.world.time_ms;
        self.world.trace.push(crate::trace::TraceEntry::Log {
            t,
            channel: -4,
            text: format!("{name} {id} -> {target}: ability {ability}"),
        });
        self.run_script(ABILITY_SCRIPT, id, Some(ev));
    }

    /// Native modern executor 0x6d7ec0: START (0x6d80f4) has the pending
    /// command's result and horizontal direction; IMPACT (0x6d8715) has the
    /// saved result and target point from SetAbilityResult.
    fn da2_ability_event(
        &self,
        ev: &mut crate::world::Event,
        name: &str,
        id: ObjectId,
        cmd: &crate::world::Command,
    ) {
        let o = self.world.get(id);
        if name == "ABILITY_CAST_IMPACT"
            && let Some(c) = o.and_then(|o| o.combat.ability_cast.as_ref())
        {
            ev.objects = vec![id, c.item, c.target];
            ev.ints = vec![c.ability, c.result];
            ev.vectors = vec![c.point];
        } else {
            ev.ints.truncate(2);
            let point = cmd.location.map(|l| l.position).unwrap_or([0.0; 3]);
            if name == "ABILITY_CAST_IMPACT" {
                // Old snapshots/scripts without a captured context retain their prior
                // zero result. Native allocation defaults are not established yet.
                ev.vectors = vec![point];
                return;
            }
            ev.ints[1] = o
                .and_then(|o| o.fields.get("CommandResult"))
                .copied()
                .unwrap_or(0);
            let pos = o.map(|o| o.position).unwrap_or([0.0; 3]);
            let (x, y) = (point[0] - pos[0], point[1] - pos[1]);
            let length = (x * x + y * y).sqrt();
            ev.vectors = vec![if length > 0.0 {
                [x / length, y / length, 0.0]
            } else {
                [1.0, 0.0, 0.0]
            }];
        }
    }

    /// The ability's casting time is up: `ABILITY_CAST_IMPACT` to its owner. An ability
    /// that fires a projectile (`projectile` in `ABI_base`) first announces the launch
    /// (`ABILITY_PROJECTILE_LAUNCHED`, on which `ability_core` starts the cooldown of a
    /// projectile spell; objects caster, target, item, as `ability_core` reads them).
    /// Provisional: the projectile's flight is not simulated, it lands at once.
    pub(crate) fn finish_ability(&mut self, id: ObjectId, cmd: &crate::world::Command) {
        if !self.alive(id) {
            return;
        }
        let target = cmd.target.unwrap_or(OBJECT_INVALID);
        let ability = cmd.ints.first().copied().unwrap_or(0);
        let projectile = self
            .world
            .tables()
            .get(1, "")
            .and_then(|t| match t.cell("projectile", ability as i64) {
                Some(thedas_formats::gda::Cell::Int(v)) => Some(*v),
                _ => None,
            })
            .unwrap_or(0);
        if projectile > 0 {
            let item = self.ability_item(id, cmd);
            let mut ev = self.new_event("ABILITY_PROJECTILE_LAUNCHED", Some(id));
            ev.objects = vec![id, target, item];
            ev.ints = vec![ability, 0, 0, 0];
            ev.strings = cmd.strings.clone();
            self.run_script(ABILITY_SCRIPT, id, Some(ev));
            if !self.alive(id) {
                return;
            }
        }
        self.ability_event("ABILITY_CAST_IMPACT", id, target, ability, cmd);
    }

    /// The front command started: the attacker turns to face its target, then asks its
    /// owner's script for the result.
    ///
    /// Facing: no script turns attackers (`rules_core`, `ability_core` and `creature_core`
    /// never call `SetFacing*`), while the engine requires them to face their target:
    /// DAOrigins.exe 0x7a2410 accepts a target only within 67.5 degrees (0xb2117c) of the
    /// attacker's facing (0x4d4bf0 measures the angle). How the engine turns them (at once
    /// or at a turn rate) is not confirmed; here they turn at once when the swing starts
    /// (provisional).
    pub(crate) fn start_attack(&mut self, id: ObjectId, target: ObjectId, ty: i32) {
        self.face_target(id, target);
        let mut ev = self.new_event("COMMAND_PENDING", Some(id));
        ev.objects = vec![id, target];
        ev.ints = vec![ty, 0];
        if let Some(o) = self.world.get_mut(id) {
            o.combat.attack = None;
            o.combat.attack_ms = None;
            o.combat.aim_ms = None;
        }
        let script = self.world.get(id).and_then(|o| o.script.clone());
        if let Some(script) = script {
            self.run_script(&script, id, Some(ev));
        }
        self.resolve_da2_attack(id, target);
    }

    /// DA2's ability_core asks the default attack's script to resolve event 1006 with
    /// objects attacker/target and integer ability. Its answer is module local 8, reset
    /// to HIT before the synchronous call. Run the player's script so its attack/defense,
    /// difficulty, dodge, glancing-blow and talent rules use the same VM and saved dice.
    ///
    /// DragonAge2.exe 0x4dd210 stores SetAttackResult verbatim and 0x56ba70 skips result
    /// zero. The newer ability executor (0x6d823e) dispatches to ability_core, whose
    /// resolution handshake supplies the missing result. Our simplified attack executor
    /// uses DEFAULT_ATTACK (1). Selection of animation-specific basic attack abilities
    /// from the combat combo tables is still provisional.
    fn resolve_da2_attack(&mut self, id: ObjectId, target: ObjectId) {
        if self.game != thedas_vfs::Game::DragonAge2 {
            return;
        }
        let unresolved = |attack: &AttackResult, hand: usize| {
            attack.results[hand] == 0 && attack.effects[hand].as_ref().is_some_and(|e| e.ty == 1024)
        };
        let needs_roll = self
            .world
            .get(id)
            .and_then(|o| o.combat.attack.as_ref())
            .is_some_and(|attack| (0..2).any(|hand| unresolved(attack, hand)));
        if !needs_roll {
            return;
        }
        let script = self
            .world
            .tables()
            .get(1, "")
            .map(|t| crate::engine::cell_string(t.cell_by_hash(0xf333_fe09, 1)))
            .filter(|s| !s.is_empty());
        let Some((script, module)) = script.zip(self.world.module) else {
            return;
        };
        let key = crate::engine::local_name(self, module, &[thedas_script::vm::Value::Int(8)], 0);
        let hit = constant_of(self, "COMBAT_RESULT_HIT");
        let Some(o) = self.world.get_mut(module) else {
            return;
        };
        o.locals.ints.insert(key.clone(), hit);
        let mut ev = self.new_event("", Some(id));
        ev.ty = 1006;
        ev.objects = vec![id, target];
        ev.ints = vec![1];
        self.run_script(&script, id, Some(ev));
        let answer = self
            .world
            .get(module)
            .and_then(|o| o.locals.ints.get(&key))
            .copied();
        if let (Some(answer), Some(attack)) = (
            answer,
            self.world
                .get_mut(id)
                .and_then(|o| o.combat.attack.as_mut()),
        ) {
            for hand in 0..2 {
                if unresolved(attack, hand) {
                    attack.results[hand] = answer;
                }
            }
        }
    }

    /// Turns `id` to face `target` (both in the same area; nothing when they stand on the
    /// same spot).
    pub(crate) fn face_target(&mut self, id: ObjectId, target: ObjectId) {
        let Some(t) = self.world.get(target).map(|t| (t.position, t.area)) else {
            return;
        };
        if let Some(o) = self.world.get_mut(id) {
            let (dx, dy) = (t.0[0] - o.position[0], t.0[1] - o.position[1]);
            if id != target && o.area == t.1 && dx * dx + dy * dy > 1e-6 {
                o.facing = crate::world::facing_toward(dx, dy);
            }
        }
    }

    /// The attack's time is up: its owner applies the impact, once for each of the two hands
    /// (`SetAttackResult`'s result and damage effect pairs) that has a result.
    ///
    /// As DAOrigins.exe does it: `SetAttackResult` (0x65de40) keeps each hand's result,
    /// target and damage effect in its own slot of the attacker's combat state (0x6a0160 for
    /// the main hand, 0x6a0170 for the off hand). Each hit event of the attack animation
    /// (0x6c0c46 calls 0x6a6230) sends the impact of one slot, the off hand's when the
    /// animation event's name holds `Left` (0xb0f840); the impact (0x6a6080) is
    /// `EVENT_TYPE_ATTACK_IMPACT` (0x1d) with objects attacker and the slot's target and
    /// integers the slot's result and its damage effect's id, and nothing at all for a slot
    /// whose result is `COMBAT_RESULT_INVALID` (0). rules_core passes integer 1 to
    /// `GetAttackImpactDamageEffect`; here the id is the hand's index, which that function
    /// reads. rules_core's dual-weapon attacks give one hand a result and the other none
    /// (alternating), so the off hand's blows land only through their own impact.
    ///
    /// Provisional: animation events are not played, so both hands' impacts come when the
    /// attack's time is up, main hand first.
    /// The target of the attack `id` is making: its `CommandAttack`'s target while the
    /// command is running. Read-only, for the presentation.
    pub fn attack_target(&self, id: ObjectId) -> Option<ObjectId> {
        let cmd = self.world.get(id)?.commands.first()?;
        (cmd.name == "CommandAttack")
            .then_some(cmd.target)
            .flatten()
            .filter(|&t| t != OBJECT_INVALID)
    }

    /// One hand's (0 main, 1 off) result for the attack in progress, as `SetAttackResult`
    /// gave it (`COMBAT_RESULT_*`), and its target; `None` for a hand with no result
    /// (`COMBAT_RESULT_INVALID`), which sends no impact. Read-only, for the presentation.
    ///
    /// The exe keeps a target per hand slot (0x6a0160 main, 0x6a0170 off); rules_core gives
    /// both the attack's target, so here both hands report the `CommandAttack` target.
    pub fn hand_attack(&self, id: ObjectId, hand: usize) -> Option<(i32, ObjectId)> {
        let result = *self
            .world
            .get(id)?
            .combat
            .attack
            .as_ref()?
            .results
            .get(hand)?;
        (result != 0).then_some((result, self.attack_target(id)?))
    }

    pub(crate) fn finish_attack(&mut self, id: ObjectId, target: ObjectId) {
        // An attack restored from a save made before DA2 resolution was implemented can
        // still have result zero. Resolve it once before delivering its impact.
        self.resolve_da2_attack(id, target);
        let Some(result) = self.world.get(id).and_then(|o| o.combat.attack.clone()) else {
            return;
        };
        for hand in 0..2 {
            let hit = result.results[hand];
            // Keep the previous fallback when the module or default attack's script
            // metadata is unavailable. Normal attacks already hold their script's roll.
            let da2_blow = self.game == thedas_vfs::Game::DragonAge2
                && hit == 0
                && result.effects[hand].iter().any(|e| e.ty == 1024);
            let hit = if da2_blow {
                constant_of(self, "COMBAT_RESULT_HIT")
            } else {
                hit
            };
            if hit == 0 || !self.alive(id) {
                continue;
            }
            let t = self.world.time_ms;
            // The damage the impact carries (float 0 of EFFECT_TYPE_IMPACT or _DAMAGE).
            let damage = result.effects[hand]
                .iter()
                .filter(|e| matches!(e.ty, 1013 | 1024))
                .map(|e| e.floats.first().copied().unwrap_or(0.0))
                .sum();
            self.world.trace.push(crate::trace::TraceEntry::Attack {
                t,
                attacker: id,
                target,
                result: hit,
                damage,
            });
            let mut ev = self.new_event("ATTACK_IMPACT", Some(id));
            ev.objects = vec![id, target];
            ev.ints = vec![hit, hand as i32];
            let script = self.world.get(id).and_then(|o| o.script.clone());
            if let Some(script) = script.clone() {
                self.run_script(&script, id, Some(ev));
            }
            // A ranged shot's ammunition then hits with its on-hit properties: DAOrigins.exe
            // stores one `EVENT_TYPE_ITEM_ONHIT` (65) per on-hit property of the
            // `INVENTORY_SLOT_RANGEDAMMO` item at launch (0x6b6f60) and sends them to the
            // shooter after the impact unless the result is a miss or blocked (0x7e87c0):
            // creator the shooter, target the victim, integers the property and its power,
            // objects the shooter and the ammunition; rules_core rolls the property's
            // `ProcChance`. A melee hit's on-hit events go to the item itself, whose script
            // is empty in every template, so they do nothing and are not sent here.
            let missed = [
                "COMBAT_RESULT_INVALID",
                "COMBAT_RESULT_MISS",
                "COMBAT_RESULT_BLOCKED",
            ]
            .iter()
            .any(|c| constant_of(self, c) == hit);
            if !missed && self.attack_reach(id) > MELEE_REACH {
                let ammo_slot = constant_of(self, "INVENTORY_SLOT_RANGEDAMMO");
                let ammo = self
                    .world
                    .get(id)
                    .and_then(|o| o.equipment.get(&ammo_slot).copied());
                if let (Some(ammo), Some(script)) = (ammo, script) {
                    for (property, power) in self.on_hit_properties(ammo) {
                        let mut ev = self.new_event("ITEM_ONHIT", Some(id));
                        ev.target = Some(target);
                        ev.ints = vec![property, power];
                        ev.objects = vec![id, ammo];
                        self.run_script(&script, id, Some(ev));
                    }
                }
            }
        }
        if let Some(o) = self.world.get_mut(id) {
            o.combat.attack = None;
        }
    }
}

/// `bitm_base.gda`: the base item's combat animation tree (`Bow.anb`, `Cbow.anb`,
/// `Staff.anb`; empty for melee weapons). The column's name is not known.
const COL_ANIM_TREE: u32 = 0x8bf8_fc78;

impl Sim {
    /// How long a ranged weapon's shot takes to play: the `fire` action of the weapon's
    /// animation tree (`bitm_base.gda`), its animation's length (`.gad`) over its speed, in
    /// milliseconds. 0 for weapons without a tree (melee) or when the data is missing.
    ///
    /// A ranged attack is then the attack duration the rules set (`SetAttackDuration`: 1.5 s
    /// for a bow, 0.3 s for a staff), the aim (`SetAimLoopDuration`) and this shot (bow 0.43
    /// s, crossbow 0.6 s, staff 1.0 s), as DAOrigins.exe sequences it: the attack's start
    /// step (0x7a87c0, calling 0x7a4dd0) plays `Aim` (anim_combat.gda 249) at the speed
    /// that makes it last the attack duration (0x7a4e6d-0x7a4ea6: its length over the
    /// duration, which `SetAttackDuration`, 0x659d30, keeps in milliseconds), then `Fire`
    /// (243, 0x7a4f51) at its own speed; between them the attack's executor (0x7a2410)
    /// holds for the aim loop duration (0x7a244b). Melee durations match whole swings
    /// (1.9 s against 1.97-2.0 s weapon-and-shield attacks) and stay the whole attack
    /// (provisional).
    pub fn fire_ms(&mut self, id: ObjectId) -> u64 {
        let slot = constant_of(self, "INVENTORY_SLOT_MAIN");
        let base = self
            .world
            .get(id)
            .and_then(|o| o.equipment.get(&slot))
            .and_then(|w| self.world.get(*w))
            .and_then(|w| w.fields.get("BaseItem").copied());
        let Some(base) = base else { return 0 };
        let tree = self
            .world
            .tables()
            .get(6, "")
            .and_then(|t| match t.cell_by_hash(COL_ANIM_TREE, base as i64)? {
                thedas_formats::gda::Cell::String(s) => Some(crate::engine::clean(s).to_string()),
                _ => None,
            })
            .filter(|s| !s.is_empty());
        let Some(tree) = tree else { return 0 };
        if let Some(ms) = self.fire_lengths.get(&tree.to_ascii_lowercase()) {
            return ms.unwrap_or(0);
        }
        let read = |name: &str| self.vfs.read(name).ok().map(|(d, _)| d);
        let ms = read(&tree)
            .and_then(|d| thedas_formats::anim::AnimTree::parse(&d).ok())
            .and_then(|t| {
                let node = t.action("fire")?.clone();
                let ani = node.animation?;
                let gad = format!("{}.gad", ani.strip_suffix(".ani").unwrap_or(&ani));
                let info = thedas_formats::anim::AnimInfo::parse(&read(&gad)?).ok()?;
                let speed = (node.speed * info.speed).max(0.01);
                Some((info.duration / speed * 1000.0).round() as u64)
            });
        self.fire_lengths.insert(tree.to_ascii_lowercase(), ms);
        ms.unwrap_or(0)
    }
}

/// `apr_base.gda`: the appearance's combat animation tree (`combat_humanoid.anb`,
/// `C_DOGa_cbt.anb`, ...).
const COL_COMBAT_TREE: u32 = 0xde80_b418;
/// `bitm_base.gda`: the slots an item fits (1 main hand, 2 off hand, 3 either).
const COL_EQUIPPABLE_SLOTS: u32 = 0xd5b0_ae22;
/// `bitm_base.gda`: what an item is (`Melee`, `Ranged`, `Wand`, `Shield`, ...).
const COL_ITEM_TYPE: u32 = 0xc0d6_c7f2;

impl Sim {
    /// How long a melee swing takes when the rules set no duration: its attack animation
    /// played at its own speed, as DAOrigins.exe does when the attack duration is not set
    /// (0x7a4e6d; `SetAttackDuration`, 0x659d30, scales the animation to fit only when the
    /// rules give one, and rules_core gives one for ranged weapons only). One of the six
    /// attack actions (`attk_v1`-`attk_v6`) of the creature's combat animation tree
    /// (`apr_base.gda`), by the simulation's dice; a humanoid's tree is its weapon style's:
    /// `ws` (weapon and shield, 2.0 s), `ow` (one weapon, 2.0 s), `tw` (two-handed, 2.3-2.5
    /// s), `dw` (two weapons, 1.47 s), or the humanoid tree itself unarmed (1.5 s).
    ///
    /// Provisional: how the executable picks the weapon-style tree and the attack variant is
    /// not traced; the style is read from the weapons' slots and kinds (`bitm_base.gda`),
    /// the tree from its name.
    pub fn melee_attack_ms(&mut self, id: ObjectId) -> Option<u64> {
        let tree = self.style_tree(id)?;
        let lengths = match self.attack_lengths.get(&tree) {
            Some(l) => l.clone(),
            None => {
                let l = self.read_attack_lengths(&tree);
                self.attack_lengths.insert(tree.clone(), l.clone());
                l
            }
        };
        if lengths.is_empty() {
            return None;
        }
        let pick = self.world.rng.below(lengths.len() as i32).max(0) as usize;
        lengths.get(pick).copied()
    }

    /// The combat animation tree of a creature (`apr_base.gda`, lower case) and the tree its
    /// weapons pick from it ([`attack_tree`]): a humanoid's weapon-style tree, any other
    /// creature's own combat tree. None without a combat tree.
    fn combat_trees(&self, id: ObjectId) -> Option<(String, String)> {
        let o = self.world.get(id)?;
        let appearance = o.appearance;
        let slot_item = |slot: &str| {
            let slot = constant_of(self, slot);
            o.equipment
                .get(&slot)
                .and_then(|i| self.world.get(*i))
                .and_then(|i| i.fields.get("BaseItem").copied())
        };
        let (main, off) = (
            slot_item("INVENTORY_SLOT_MAIN"),
            slot_item("INVENTORY_SLOT_OFFHAND"),
        );
        let tables = self.world.tables();
        let tree = tables
            .get(2, "")
            .and_then(
                |t| match t.cell_by_hash(COL_COMBAT_TREE, appearance as i64)? {
                    thedas_formats::gda::Cell::String(s) => {
                        Some(crate::engine::clean(s).to_ascii_lowercase())
                    }
                    _ => None,
                },
            )
            .filter(|s| !s.is_empty())?;
        let items = tables.get(6, "");
        let kind = |b: Option<i32>| {
            b.and_then(
                |b| match items.as_ref()?.cell_by_hash(COL_ITEM_TYPE, b as i64)? {
                    thedas_formats::gda::Cell::String(s) => {
                        Some(crate::engine::clean(s).to_ascii_lowercase())
                    }
                    _ => None,
                },
            )
            .unwrap_or_default()
        };
        let slots = |b: Option<i32>| {
            b.and_then(|b| {
                match items
                    .as_ref()?
                    .cell_by_hash(COL_EQUIPPABLE_SLOTS, b as i64)?
                {
                    thedas_formats::gda::Cell::Int(v) => Some(*v),
                    _ => None,
                }
            })
            .unwrap_or(0)
        };
        let style = attack_tree(&tree, &kind(main), slots(main), &kind(off));
        Some((tree, style))
    }

    /// The weapon-style tree a creature's melee attacks come from (see [`attack_tree`]).
    fn style_tree(&self, id: ObjectId) -> Option<String> {
        self.combat_trees(id).map(|t| t.1)
    }

    /// The lengths (milliseconds) of an animation tree's attack actions (`attk_v1`-`attk_v6`,
    /// in order), each its animation's `.gad` duration over its speeds.
    fn read_attack_lengths(&self, tree: &str) -> Vec<u64> {
        let read = |name: &str| self.vfs.read(name).ok().map(|(d, _)| d);
        let Some(t) = read(tree).and_then(|d| thedas_formats::anim::AnimTree::parse(&d).ok())
        else {
            return Vec::new();
        };
        (1..=6)
            .filter_map(|v| {
                let node = t.action(&format!("attk_v{v}"))?;
                let ani = node.animation.as_ref()?;
                let gad = format!("{}.gad", ani.strip_suffix(".ani").unwrap_or(ani));
                let info = thedas_formats::anim::AnimInfo::parse(&read(&gad)?).ok()?;
                let speed = (node.speed * info.speed).max(0.01);
                Some((info.duration / speed * 1000.0).round() as u64)
            })
            .collect()
    }
}

/// `ABI_base.gda` columns the cast reads: `castanim` (DAOrigins.exe 0x7a749f) and
/// `conjureanim` (0x744559), animation ids; `speed` (0x744587), a row of
/// `spellconjuringspeeds.gda`, whose own `speed` column (0x74463b) counts conjure loops.
const COL_CAST_ANIM: u32 = 0xf954_09f5;
const COL_CONJURE_ANIM: u32 = 0x9abe_aef1;
const COL_CONJURE_SPEED: u32 = 0x07f4_f006;
/// M2DA 0x87 (0x744641): `spellconjuringspeeds`.
const SPELL_CONJURING_SPEEDS: i32 = 0x87;
/// The animation tables (M2DA 0, `anim_*.gda`): the tree action an animation id plays
/// (0x5cb558 maps the actions to ids), and the animation that follows it (0x5ca6c3: a
/// conjure's start is followed by its loop).
const COL_ANIM_ACTION: u32 = 0x7a39_8af9;
const COL_ANIM_NEXT: u32 = 0xffc3_72e7;
/// Animation event kinds that deliver an ability: the creature's animation-event handler
/// (0x6c0bf0, its switch at 0x6c0c37) sends `ABILITY_CAST_IMPACT` on kind 7 (0x6c0c5f,
/// 0x6af430); kind 9 goes on (0x6c0d2e, 0x694ee0) to the creature's release (0x6b6f60),
/// which for an ability with no projectile (`ABI_base` `projectile`, 0x6b71ce) also calls
/// 0x6af430 (0x6b7323) and otherwise launches the projectile, whose arrival sends the
/// impact (0x7e71c0). The impact is sent once: 0x6af430 consumes the cast's slot.
const IMPACT_EVENT_KINDS: [i64; 2] = [7, 9];

/// When an ability's impact comes and when its command ends, in milliseconds from its
/// start (see [`Sim::ability_timing`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AbilityTiming {
    pub impact_ms: u64,
    pub end_ms: u64,
}

impl Sim {
    /// When a `CommandUseAbility` deals its impact and ends, from its animations, as
    /// DAOrigins.exe plays them; None (the caller keeps its default) for an ability with no
    /// cast animation or whose animations the creature's trees lack.
    ///
    /// - **Conjure** (spells: `conjureanim` set). The use-ability command (0x743f50, the
    ///   handler of command type 25) times it at 0x7445cf-0x744665: the start animation's
    ///   length, plus, when the following animation (the loop, `COL_ANIM_NEXT`) loops
    ///   (0x695390), the loop's length times the conjure speed's count
    ///   (`spellconjuringspeeds.gda`: Instant 0, Fast 1, Normal 2, Slow 4, Ritual 6). Lengths
    ///   are the animations' `.gad` length (field 4009) in milliseconds (0x694e50 ->
    ///   0xa5f7a0 -> 0xa38ed0: the length times 1000.0, 0xaebc00). A count of 0 leaves the
    ///   start animation alone, so the conjure lasts its length.
    /// - **Cast**: `castanim`'s animation (the cast step, 0x7a7440, starts it after
    ///   `ABILITY_CAST_START`); the impact comes at its first kind 7 or 9 event (see
    ///   `IMPACT_EVENT_KINDS`), the command ends with it.
    ///
    /// Provisional: which of the creature's trees supplies an action is not traced (the
    ///   weapon's own tree, then its weapon-style tree, then its combat tree are searched, as
    ///   the attack timings choose theirs); the cast's length and event times are scaled by
    ///   the tree node's and animation's speeds as the attack lengths are, while the conjure
    ///   uses the raw length as the executable does; the cast's end is taken as the command's
    ///   end (the following `_end` animation is not counted); a projectile's flight is not
    ///   simulated (the impact comes at the release).
    pub fn ability_timing(&mut self, id: ObjectId, ability: i32) -> Option<AbilityTiming> {
        let abi = self.world.tables().get(1, "")?;
        let int = |t: &crate::m2da::Table, hash: u32, row: i64| match t.cell_by_hash(hash, row) {
            Some(thedas_formats::gda::Cell::Int(v)) => Some(*v),
            Some(thedas_formats::gda::Cell::Float(v)) => Some(*v as i64),
            _ => None,
        };
        let cast = int(&abi, COL_CAST_ANIM, ability as i64).filter(|&c| c > 0)?;
        let conjure = int(&abi, COL_CONJURE_ANIM, ability as i64).unwrap_or(0);
        let speed_row = int(&abi, COL_CONJURE_SPEED, ability as i64).unwrap_or(0);
        let anims = self.world.tables().get(0, "")?;
        let action = |anim: i64| match anims.cell_by_hash(COL_ANIM_ACTION, anim) {
            Some(thedas_formats::gda::Cell::String(s)) => {
                Some(crate::engine::clean(s).to_ascii_lowercase()).filter(|s| !s.is_empty())
            }
            _ => None,
        };
        let trees = self.ability_trees(id);
        let cast = self.ability_anim(&trees, &action(cast)?)?;
        let cast_impact = cast.impact_ms?;
        let conjure_ms = if conjure > 0 {
            let start = self.ability_anim(&trees, &action(conjure)?)?;
            let loop_ms = int(&anims, COL_ANIM_NEXT, conjure)
                .filter(|&n| n > 0 && anim_loops(&anims, n))
                .and_then(action)
                .and_then(|a| self.ability_anim(&trees, &a))
                .map(|a| a.raw_ms);
            let count = self
                .world
                .tables()
                .get(SPELL_CONJURING_SPEEDS, "")
                .and_then(|t| int(&t, COL_CONJURE_SPEED, speed_row))
                .unwrap_or(0)
                .max(0) as u64;
            match loop_ms {
                Some(l) if count > 0 => count * l + start.raw_ms,
                _ => start.raw_ms,
            }
        } else {
            0
        };
        Some(AbilityTiming {
            impact_ms: conjure_ms + cast_impact,
            end_ms: conjure_ms + cast.len_ms.max(cast_impact),
        })
    }

    /// The trees a creature's ability animations are looked up in, first to last: its main
    /// weapon's own tree (`bitm_base.gda`, e.g. `Staff.anb`), its weapon-style tree, its
    /// combat tree. Provisional (see [`Sim::ability_timing`]).
    fn ability_trees(&self, id: ObjectId) -> Vec<String> {
        let mut trees = Vec::new();
        let slot = constant_of(self, "INVENTORY_SLOT_MAIN");
        let weapon = self
            .world
            .get(id)
            .and_then(|o| o.equipment.get(&slot))
            .and_then(|w| self.world.get(*w))
            .and_then(|w| w.fields.get("BaseItem").copied())
            .and_then(|base| {
                match self
                    .world
                    .tables()
                    .get(6, "")?
                    .cell_by_hash(COL_ANIM_TREE, base as i64)?
                {
                    thedas_formats::gda::Cell::String(s) => {
                        Some(crate::engine::clean(s).to_ascii_lowercase())
                    }
                    _ => None,
                }
            })
            .filter(|s| !s.is_empty());
        trees.extend(weapon);
        if let Some((combat, style)) = self.combat_trees(id) {
            if style != combat {
                trees.push(style);
            }
            trees.push(combat);
        }
        trees
    }

    /// An action's animation in the first of `trees` that has it (cached by tree and
    /// action).
    fn ability_anim(&mut self, trees: &[String], action: &str) -> Option<AbilityAnimLen> {
        for tree in trees {
            let key = format!("{tree}|{action}");
            let found = match self.ability_anims.get(&key) {
                Some(a) => *a,
                None => {
                    let a = self.read_ability_anim(tree, action);
                    self.ability_anims.insert(key, a);
                    a
                }
            };
            if found.is_some() {
                return found;
            }
        }
        None
    }

    fn read_ability_anim(&self, tree: &str, action: &str) -> Option<AbilityAnimLen> {
        let read = |name: &str| self.vfs.read(name).ok().map(|(d, _)| d);
        let t = thedas_formats::anim::AnimTree::parse(&read(tree)?).ok()?;
        let node = t.action(action)?;
        let ani = node.animation.as_ref()?;
        let base = ani.strip_suffix(".ani").unwrap_or(ani);
        let info = thedas_formats::anim::AnimInfo::parse(&read(&format!("{base}.gad"))?).ok()?;
        let speed = (node.speed * info.speed).max(0.01);
        let events = read(&format!("{base}.evt"))
            .and_then(|d| thedas_formats::anim::parse_events(&d).ok())
            .unwrap_or_default();
        let impact = events
            .iter()
            .filter(|e| IMPACT_EVENT_KINDS.contains(&e.kind))
            .map(|e| e.time)
            .min_by(f32::total_cmp);
        Some(AbilityAnimLen {
            raw_ms: (info.duration * 1000.0).round() as u64,
            len_ms: (info.duration / speed * 1000.0).round() as u64,
            impact_ms: impact.map(|t| (t / speed * 1000.0).round() as u64),
        })
    }
}

/// An animation's lengths for [`Sim::ability_timing`]: as the executable measures it
/// (`raw_ms`), as it plays (`len_ms`, over its speeds), and its first impact event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AbilityAnimLen {
    pub raw_ms: u64,
    pub len_ms: u64,
    pub impact_ms: Option<u64>,
}

/// Whether an animation loops: bit 0 of the animation tables' flags column (0x12a02374),
/// as DAOrigins.exe's test (0x695390) reads it when the creature has no animation component
/// to ask (0x6953ad -> 0x5ca610 with mask 1): set on the conjure loops (4097), clear on
/// their starts (4096). With a component it asks the component (0xa5e940), not traced.
fn anim_loops(anims: &crate::m2da::Table, anim: i64) -> bool {
    matches!(anims.cell_by_hash(COL_ANIM_FLAGS, anim), Some(thedas_formats::gda::Cell::Int(v)) if v & 1 != 0)
}

/// The animation tables' flags column (see [`anim_loops`]).
const COL_ANIM_FLAGS: u32 = 0x12a0_2374;

/// The animation tree a creature's melee attacks come from: a humanoid's (`combat_humanoid`)
/// by its weapons (the main hand's kind and slots, the off hand's kind; see
/// [`Sim::melee_attack_ms`]), any other creature's its own combat tree. Provisional.
pub fn attack_tree(combat_tree: &str, main: &str, main_slots: i64, off: &str) -> String {
    if combat_tree != "combat_humanoid.anb" || main != "melee" {
        return combat_tree.to_string();
    }
    match off {
        "melee" => "dw.anb",
        "shield" => "ws.anb",
        _ if main_slots == 1 => "tw.anb",
        _ => "ow.anb",
    }
    .to_string()
}

impl Sim {
    /// `GetWeaponStyle`: `WEAPONSTYLE_NONE` without a main-hand weapon; with a melee one,
    /// `DUAL` beside an off-hand melee weapon, `TWOHANDED` for one that fits only the main
    /// hand, else `SINGLE`; ranged weapons and wands `SINGLE`. Provisional: the executable
    /// keeps the style in the creature's combat state (0x6a3c30); how it sets it is not
    /// traced, so it is read from the weapons as [`attack_tree`] reads them.
    pub fn weapon_style(&self, id: ObjectId) -> i32 {
        if self.game == thedas_vfs::Game::DragonAge2 {
            return self.da2_combat_style(id);
        }
        let Some(o) = self.world.get(id) else {
            return constant_of(self, "WEAPONSTYLE_NONE");
        };
        let base = |slot: &str| {
            let slot = constant_of(self, slot);
            o.equipment
                .get(&slot)
                .and_then(|i| self.world.get(*i))
                .and_then(|i| i.fields.get("BaseItem").copied())
        };
        let Some(main) = base("INVENTORY_SLOT_MAIN") else {
            return constant_of(self, "WEAPONSTYLE_NONE");
        };
        let items = self.world.tables().get(6, "");
        let kind = |b: Option<i32>| {
            b.and_then(
                |b| match items.as_ref()?.cell_by_hash(COL_ITEM_TYPE, b as i64)? {
                    thedas_formats::gda::Cell::String(s) => {
                        Some(crate::engine::clean(s).to_ascii_lowercase())
                    }
                    _ => None,
                },
            )
            .unwrap_or_default()
        };
        let slots = items
            .as_ref()
            .and_then(
                |t| match t.cell_by_hash(COL_EQUIPPABLE_SLOTS, main as i64)? {
                    thedas_formats::gda::Cell::Int(v) => Some(*v),
                    _ => None,
                },
            )
            .unwrap_or(0);
        let name = match attack_tree(
            "combat_humanoid.anb",
            &kind(Some(main)),
            slots,
            &kind(base("INVENTORY_SLOT_OFFHAND")),
        )
        .as_str()
        {
            "dw.anb" => "WEAPONSTYLE_DUAL",
            "tw.anb" => "WEAPONSTYLE_TWOHANDED",
            _ => "WEAPONSTYLE_SINGLE",
        };
        constant_of(self, name)
    }
}

/// How close a melee attacker gets before swinging (metres) when neither side's size is
/// known. Provisional: rules_core counts any melee attack from beyond 5 m as a miss.
pub const MELEE_REACH: f32 = 2.0;

/// How far past the two bodies a melee attack reaches (metres): DAOrigins.exe's melee range
/// test (0x6a3dc0, used by the attack command's start step 0x7a87c0) adds 1.2 (0xafebc0) to
/// the two creatures' radii (0x695320, which adds nothing between them, 0xade210) and
/// measures across the ground. The radii are `PERSPACE` (0x6a21a0 reads the appearance's
/// copy of it, set by 0x748a40).
pub const MELEE_GAP: f32 = 1.2;

/// The slack the same test allows past that reach (metres, 0xade218): an attack may start
/// with the target up to this much farther away.
pub const MELEE_REACH_TOLERANCE: f32 = 1.0;

/// A creature's body radius when its appearance gives none (metres): DAOrigins.exe's
/// `SetAppearanceType` (0x748a40) default for a missing `PERSPACE` (0xaf7618).
pub const DEFAULT_RADIUS: f32 = 0.6;

impl Sim {
    /// A creature's body radius: its appearance's personal space in `apr_base.gda`
    /// (`PERSPACE`: 0.35 for humans and walking corpses, 0.4 for hurlocks, 0.95 for an
    /// ogre), else [`DEFAULT_RADIUS`].
    pub fn body_radius(&self, id: ObjectId) -> f32 {
        let app = self.world.get(id).map(|o| o.appearance).filter(|&a| a > 0);
        app.and_then(|a| {
            let t = self.world.tables().get(0, "apr_base")?;
            match t.cell("PERSPACE", a as i64)? {
                thedas_formats::gda::Cell::Float(v) if *v > 0.0 => Some(*v as f32),
                _ => None,
            }
        })
        .unwrap_or(DEFAULT_RADIUS)
    }

    /// How close `id` must be to `target` to attack it: a ranged weapon's range, else the
    /// two bodies' radii and [`MELEE_GAP`] (an attack may start from up to
    /// [`MELEE_REACH_TOLERANCE`] farther).
    pub fn reach_to(&self, id: ObjectId, target: ObjectId) -> f32 {
        let r = self.attack_reach(id);
        if r > MELEE_REACH {
            return r;
        }
        self.body_radius(id) + self.body_radius(target) + MELEE_GAP
    }

    /// How close `id` must be to attack. DA:O reads item stat 9 in `itemstats.gda`.
    /// DA2 reads the equipped ranged weapon's combat style and its maximum range class
    /// in tables 12000 -> 300 -> 301. Other weapons use [`MELEE_REACH`].
    pub fn attack_reach(&self, id: ObjectId) -> f32 {
        let slot = constant_of(self, "INVENTORY_SLOT_MAIN");
        let base = self
            .world
            .get(id)
            .and_then(|o| o.equipment.get(&slot))
            .and_then(|w| self.world.get(*w))
            .and_then(|w| w.fields.get("BaseItem").copied());
        let tables = self.world.tables();
        if self.game == thedas_vfs::Game::DragonAge2 {
            let range = base.and_then(|base| {
                let items = tables.get(12000, "")?;
                // DragonAge2.exe 0x53b1a0 classifies types 5 and 6 as ranged.
                let kind = crate::engine::cell_int(items.cell_by_hash(0x12a0_2374, base as i64));
                if !matches!(kind, 5 | 6) {
                    return None;
                }
                // 0x55ff60 selects the equipped weapon's combat style from ITM_.
                let style = crate::engine::cell_int(items.cell_by_hash(0x93b9_42d7, base as i64));
                let styles = tables.get(300, "")?;
                // ability_core reads this style's maximum range class. The native
                // ability range checks at 0x561033 and 0x641104 read its upper distance.
                let row = crate::engine::cell_int(styles.cell_by_hash(0x065e_4d58, style as i64));
                let ranges = tables.get(301, "")?;
                Some(crate::engine::cell_float(
                    ranges.cell_by_hash(0x814f_8b88, row as i64),
                ))
            });
            return range
                .filter(|r| r.is_finite() && *r > MELEE_REACH)
                .unwrap_or(MELEE_REACH);
        }
        let range = base.and_then(|b| {
            let label = tables
                .get(0, "itemstatdefines")
                .map(|t| crate::engine::cell_string(t.cell("Label", 9)))
                .filter(|l| !l.is_empty())?;
            let t = tables.get(0, "itemstats")?;
            Some(crate::engine::cell_float(
                t.cell(crate::engine::clean(&label), b as i64),
            ))
        });
        match range {
            Some(r) if r > 5.0 => r,
            _ => MELEE_REACH,
        }
    }
}

fn constant_of(sim: &Sim, name: &str) -> i32 {
    crate::sim::constant(&sim.table, name)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::world::Effect;
    use thedas_formats::gda::Cell;

    use thedas_script::EngineTable;
    use thedas_vfs::Vfs;

    use super::*;

    const LDF: &str = "int EVENT_TYPE_PERCEPTION_APPEAR = 25;\n\
        int EVENT_TYPE_MANA_STAM_DEPLETED = 64;\n\
        int EVENT_TYPE_PERCEPTION_DISAPPEAR = 26;\n\
        int EVENT_TYPE_DEATH = 4;\n\
        int PROPERTY_DEPLETABLE_HEALTH = 7;\n\
        int PROPERTY_DEPLETABLE_MANA_STAMINA = 8;\n\
        int GM_EXPLORE = 0;\n\
        int GM_COMBAT = 1;\n\
        int EVENT_TYPE_GAMEMODE_CHANGE = 32;\n";

    fn sim() -> Sim {
        let table = Arc::new(EngineTable::parse(LDF.as_bytes()).unwrap());
        Sim::new(Arc::new(Vfs::new()), table, 3)
    }

    #[test]
    fn da2_basic_attack_uses_the_script_choice_and_keeps_it_on_reload() {
        use crate::world::Location;
        let table = EngineTable::parse(
            b"void SetCommandResult(object who, int result) = 1;\n\
              int COMMAND_TYPE_ATTACK = 1;\n\
              int COMMAND_FAILED = -1;\n\
              int COMMAND_TYPE_USE_ABILITY = 25;\n",
        )
        .unwrap();
        let mut s = Sim::new(Arc::new(Vfs::new()), Arc::new(table), 3);
        s.game = thedas_vfs::Game::DragonAge2;
        let area = s.world.spawn(ObjectKind::Area, "arena");
        let who = s.world.spawn(ObjectKind::Creature, "attacker");
        let target = s.world.spawn(ObjectKind::Creature, "target");
        s.world.get_mut(who).unwrap().area = Some(area);
        s.world.get_mut(who).unwrap().script = Some("select.ncs".into());
        s.world.get_mut(who).unwrap().facing = 170.;
        s.world.get_mut(target).unwrap().area = Some(area);
        s.world.get_mut(target).unwrap().position = [3., 4., 5.];
        // Own script: SetCommandResult(attacker, 3110). The native event lets
        // the shipped script return the selected attack through this API.
        let mut code = vec![0x04, 0x03];
        code.extend(3110_i32.to_be_bytes());
        code.extend([0x04, 0x06]);
        code.extend(who.to_be_bytes());
        code.extend([0x05, 0, 0, 1, 2, 0x20, 0]);
        let mut bytes = b"NCS V1.0B".to_vec();
        bytes.extend(((13 + code.len()) as u32).to_be_bytes());
        bytes.extend(code);
        s.add_script("select.ncs", thedas_script::Ncs::parse(&bytes).unwrap());
        s.order_attack(who, target);
        let mut cmd = s.world.get(who).unwrap().commands[0].clone();
        assert_eq!(
            (&*cmd.name, cmd.ty, &*cmd.ints),
            ("CommandUseAbility", 25, &[1][..])
        );
        assert!(s.choose_da2_basic_attack(who, &mut cmd));
        assert!(
            (s.world.get(who).unwrap().facing - crate::world::facing_toward(3., 4.)).abs() < 0.001
        );
        assert_eq!(cmd.ints, [3110]);
        assert_eq!(cmd.target, Some(target));
        assert_eq!(
            cmd.location,
            Some(Location {
                area,
                position: [3., 4., 5.],
                facing: 0.
            })
        );
        s.world.get_mut(who).unwrap().commands[0] = cmd;
        s.world
            .get_mut(who)
            .unwrap()
            .fields
            .insert("CommandResult".into(), 99);
        let json = serde_json::to_string(&s.world).unwrap();
        s.world = serde_json::from_str(&json).unwrap();
        let mut restored = s.world.get(who).unwrap().commands[0].clone();
        assert!(s.choose_da2_basic_attack(who, &mut restored));
        assert_eq!(restored.ints, [3110]);
        // A concrete attack must not run the selector again on reload or approach.
        assert_eq!(s.world.get(who).unwrap().fields["CommandResult"], 99);
        // The native preprocessor stops a cast when its pending resource/target
        // check fails. Reuse our own script with a negative result.
        bytes[15..19].copy_from_slice(&(-1_i32).to_be_bytes());
        s.add_script("select.ncs", thedas_script::Ncs::parse(&bytes).unwrap());
        s.world.trace.keep = true;
        s.start_ability(who, &restored);
        assert!(s.world.get(who).unwrap().commands.is_empty());
        assert!(!s.world.trace.entries.iter().any(|e| matches!(e,
            crate::trace::TraceEntry::Log { text, .. } if text.starts_with("ABILITY_CAST_START"))));
    }

    #[test]
    fn da2_ability_impact_keeps_the_cast_result_across_commands_and_reload() {
        use crate::world::{Event, Location};
        use thedas_script::vm::Value;
        let mut s = sim();
        s.game = thedas_vfs::Game::DragonAge2;
        let who = s.world.spawn(ObjectKind::Creature, "caster");
        let target = s.world.spawn(ObjectKind::Creature, "target");
        let item = s.world.spawn(ObjectKind::Item, "potion");
        s.world.get_mut(who).unwrap().inventory.push(item);
        s.order_ability(who, 3110, target, "potion");
        let o = s.world.get_mut(who).unwrap();
        o.fields.insert("CommandResult".into(), 1);
        o.commands[0].location = Some(Location {
            area: OBJECT_INVALID,
            position: [3.0, 4.0, 99.0],
            facing: 0.0,
        });
        let cmd = o.commands[0].clone();
        let mut start = Event {
            objects: vec![who, item, target],
            ints: vec![3110, 0, 0, 0],
            ..Default::default()
        };
        s.da2_ability_event(&mut start, "ABILITY_CAST_START", who, &cmd);
        assert_eq!(start.ints, [3110, 1]);
        assert_eq!(start.vectors, [[0.6, 0.8, 0.0]]);
        let table = EngineTable::parse(
            b"void SetAbilityResult(object oUser, int nProjectileTarget) = 715;",
        )
        .unwrap();
        let f = table.function_by_name("SetAbilityResult").unwrap();
        assert_eq!(
            crate::engine::call(
                &mut crate::sim::ScriptHost {
                    sim: &mut s,
                    event: None,
                    script: "test".into()
                },
                who,
                f,
                &[Value::Object(who), Value::Int(2)]
            ),
            Some(Ok(None))
        );
        let cast = s
            .world
            .get(who)
            .unwrap()
            .combat
            .ability_cast
            .clone()
            .unwrap();
        assert_eq!(cast.projectile_target, 2);
        // A later command changes both the target and the pending result. The impact
        // belongs to the cast that took the snapshot, including after restoring it.
        s.order_ability(who, 999, who, "");
        s.world
            .get_mut(who)
            .unwrap()
            .fields
            .insert("CommandResult".into(), -2);
        let saved = serde_json::to_string(&s.world).unwrap();
        s.world = serde_json::from_str(&saved).unwrap();
        let mut impact = Event::default();
        s.da2_ability_event(&mut impact, "ABILITY_CAST_IMPACT", who, &cmd);
        assert_eq!(impact.objects, [who, item, target]);
        assert_eq!(impact.ints, [3110, 1]);
        assert_eq!(impact.vectors, [[3.0, 4.0, 99.0]]);
        assert_eq!(
            s.world.get(who).unwrap().combat.ability_cast.as_ref(),
            Some(&cast)
        );
        // A coincident target has the native +X fallback direction.
        let o = s.world.get_mut(who).unwrap();
        o.position = [3.0, 4.0, 0.0];
        s.da2_ability_event(&mut start, "ABILITY_CAST_START", who, &cmd);
        assert_eq!(start.vectors, [[1.0, 0.0, 0.0]]);
    }

    #[test]
    fn da2_attack_resolution_runs_the_default_ability_script_once() {
        let ldf = "object GetModule() = 1;\n\
            void SetLocalInt(object o, int row, int value) = 2;\n\
            int COMBAT_RESULT_HIT = 1;\n";
        let mut s = Sim::new(
            Arc::new(Vfs::new()),
            Arc::new(EngineTable::parse(ldf.as_bytes()).unwrap()),
            3,
        );
        s.world.trace.keep = true;
        let module = s.world.spawn(ObjectKind::Module, "module");
        s.world.module = Some(module);
        let attacker = s.world.spawn(ObjectKind::Creature, "attacker");
        let target = s.world.spawn(ObjectKind::Creature, "target");
        let tables = crate::m2da::Tables::empty();
        tables.insert(
            "#1",
            &["#f333fe09"],
            vec![(1, vec![Cell::String("resolve.ncs".into())])],
        );
        tables.insert(
            "var_module",
            &["Label"],
            vec![(8, vec![Cell::String("answer".into())])],
        );
        s.world.tables = Arc::new(tables);
        // Own synthetic script, SetLocalInt(GetModule(), 8, 3).
        let code = [
            0x04, 0x03, 0, 0, 0, 3, 0x04, 0x03, 0, 0, 0, 8, 0x05, 0x00, 0, 1, 0, 0x05, 0x00, 0, 2,
            3, 0x20, 0,
        ];
        let mut bytes = b"NCS V1.0B".to_vec();
        bytes.extend(((13 + code.len()) as u32).to_be_bytes());
        bytes.extend(code);
        s.add_script("resolve.ncs", thedas_script::Ncs::parse(&bytes).unwrap());
        let attack = AttackResult {
            results: [0, 2],
            effects: [
                Some(Effect {
                    ty: 1024,
                    ..Default::default()
                }),
                None,
            ],
        };
        s.world.get_mut(attacker).unwrap().combat.attack = Some(attack.clone());
        // The same objects and table in DA:O never enter the DA2 resolver.
        s.resolve_da2_attack(attacker, target);
        assert_eq!(
            s.world
                .get(attacker)
                .unwrap()
                .combat
                .attack
                .as_ref()
                .unwrap()
                .results,
            [0, 2]
        );
        assert!(s.world.get(module).unwrap().locals.ints.is_empty());
        s.game = thedas_vfs::Game::DragonAge2;
        s.resolve_da2_attack(attacker, target);
        assert_eq!(
            s.world
                .get(attacker)
                .unwrap()
                .combat
                .attack
                .as_ref()
                .unwrap()
                .results,
            [3, 2]
        );
        assert_eq!(s.world.get(module).unwrap().locals.ints["answer"], 3);
        s.resolve_da2_attack(attacker, target);
        assert_eq!(
            s.world
                .trace
                .entries
                .iter()
                .filter(|e| matches!(e,
                    crate::trace::TraceEntry::ScriptStart { script, .. } if script == "resolve.ncs"
                ))
                .count(),
            1
        );
        // A non-impact effect with result zero remains absent.
        s.world.get_mut(attacker).unwrap().combat.attack = Some(AttackResult {
            results: [0, 0],
            effects: [
                Some(Effect {
                    ty: 1013,
                    ..Default::default()
                }),
                None,
            ],
        });
        s.resolve_da2_attack(attacker, target);
        assert_eq!(
            s.world
                .get(attacker)
                .unwrap()
                .combat
                .attack
                .as_ref()
                .unwrap()
                .results,
            [0, 0]
        );
    }

    #[test]
    fn da2_ranged_attacks_read_equipment_style_and_combat_range() {
        let mut s = sim();
        let tables = crate::m2da::Tables::empty();
        tables.insert(
            "#12000",
            &["#12a02374", "#93b942d7"],
            vec![
                (1200, vec![Cell::Int(5), Cell::Int(15)]),
                (1101, vec![Cell::Int(6), Cell::Int(16)]),
                (1000, vec![Cell::Int(2), Cell::Int(15)]),
                (1201, vec![Cell::Int(5), Cell::Int(99)]),
                (1202, vec![Cell::Int(5), Cell::Int(17)]),
            ],
        );
        tables.insert(
            "#300",
            &["#065e4d58"],
            vec![
                (15, vec![Cell::Int(7)]),
                (16, vec![Cell::Int(6)]),
                (17, vec![Cell::Int(8)]),
            ],
        );
        tables.insert(
            "#301",
            &["#814f8b88"],
            vec![
                (7, vec![Cell::Float(27.0)]),
                (6, vec![Cell::Float(32.0)]),
                (8, vec![Cell::Float(f64::NAN)]),
            ],
        );
        tables.insert(
            "itemstatdefines",
            &["Label"],
            vec![(9, vec![Cell::String("Range".into())])],
        );
        tables.insert(
            "itemstats",
            &["Range"],
            vec![(1200, vec![Cell::Float(25.0)])],
        );
        s.world.tables = Arc::new(tables);
        let attacker = s.world.spawn(ObjectKind::Creature, "mage");
        let target = s.world.spawn(ObjectKind::Creature, "target");
        let staff = s.world.spawn(ObjectKind::Item, "staff");
        s.world
            .get_mut(staff)
            .unwrap()
            .fields
            .insert("BaseItem".into(), 1200);
        let slot = constant_of(&s, "INVENTORY_SLOT_MAIN");
        s.world
            .get_mut(attacker)
            .unwrap()
            .equipment
            .insert(slot, staff);
        assert_eq!(s.attack_reach(attacker), 25.0, "DA:O's item stat");
        s.game = thedas_vfs::Game::DragonAge2;
        assert_eq!(s.attack_reach(attacker), 27.0, "DA2's table chain");
        assert_eq!(s.reach_to(attacker, target), 27.0);
        for (base, expected) in [
            (1101, 32.0),
            (1000, MELEE_REACH),
            (1201, MELEE_REACH),
            (1202, MELEE_REACH),
            (9999, MELEE_REACH),
        ] {
            s.world
                .get_mut(staff)
                .unwrap()
                .fields
                .insert("BaseItem".into(), base);
            assert_eq!(s.attack_reach(attacker), expected, "base {base}");
        }
        s.world.get_mut(attacker).unwrap().equipment.clear();
        assert_eq!(s.attack_reach(attacker), MELEE_REACH);
        assert_eq!(s.attack_reach(OBJECT_INVALID), MELEE_REACH);
    }

    #[test]
    fn creatures_placed_dead_lie_dead_without_dying() {
        let mut s = sim();
        let put = |s: &mut Sim, spawn_dead: Option<i32>, health: Option<f32>| {
            let id = s.world.spawn(ObjectKind::Creature, "genlock");
            let o = s.world.get_mut(id).unwrap();
            if let Some(v) = spawn_dead {
                o.locals.ints.insert("CREATURE_SPAWN_DEAD".into(), v);
            }
            if let Some(h) = health {
                o.properties.insert(7, (100.0, h));
            }
            s.lay_out_if_spawned_dead(id);
            s.world.get(id).unwrap().dead
        };
        // Placed dead, never given health: a corpse (the original's health is 0).
        assert!(put(&mut s, Some(1), None));
        assert!(put(&mut s, Some(1), Some(0.0)));
        // Healthy, or not placed dead: alive. Neither are those without the local and
        // without health (non-combatants), which the simulation keeps alive.
        assert!(!put(&mut s, Some(1), Some(50.0)));
        assert!(!put(&mut s, Some(0), None));
        assert!(!put(&mut s, None, None));
        // Laying one out is no death: no event, no time of death.
        assert!(s.world.queue.is_empty());
        let id = s.world.spawn(ObjectKind::Creature, "genlock");
        s.world
            .get_mut(id)
            .unwrap()
            .locals
            .ints
            .insert("CREATURE_SPAWN_DEAD".into(), 1);
        s.lay_out_if_spawned_dead(id);
        assert_eq!(s.world.get(id).unwrap().died_at, None);
        // Placeables with the local are not creatures.
        let p = s.world.spawn(ObjectKind::Placeable, "crate");
        s.world
            .get_mut(p)
            .unwrap()
            .locals
            .ints
            .insert("CREATURE_SPAWN_DEAD".into(), 1);
        s.lay_out_if_spawned_dead(p);
        assert!(!s.world.get(p).unwrap().dead);
    }

    #[test]
    fn modal_abilities_whose_conditions_break_are_listed() {
        let mut s = sim();
        let t = crate::m2da::Tables::empty();
        // Shield Wall needs a melee weapon and a shield (1 | 2), a sword-only stance a
        // melee weapon (1), a follow-up stance the first (0x10, needing 12), and a
        // warrior-only one 0x800.
        t.insert(
            "ABI_base",
            &["ID", "#b1caee1d", "#07342ea2"],
            vec![
                (12, vec![Cell::Int(12), Cell::Int(3), Cell::Int(0)]),
                (20, vec![Cell::Int(20), Cell::Int(1), Cell::Int(0)]),
                (21, vec![Cell::Int(21), Cell::Int(0x10), Cell::Int(12)]),
                (22, vec![Cell::Int(22), Cell::Int(0x800), Cell::Int(0)]),
            ],
        );
        // bitm_base Type: 2 melee, 3 shield.
        t.insert(
            "#6",
            &["ID", "Type"],
            vec![
                (2, vec![Cell::Int(2), Cell::Int(2)]),
                (7, vec![Cell::Int(7), Cell::Int(3)]),
            ],
        );
        s.world.tables = Arc::new(t);
        let c = s.world.spawn(ObjectKind::Creature, "warrior");
        let sword = s.world.spawn(ObjectKind::Item, "sword");
        let shield = s.world.spawn(ObjectKind::Item, "shield");
        s.world
            .get_mut(sword)
            .unwrap()
            .fields
            .insert("BaseItem".into(), 2);
        s.world
            .get_mut(shield)
            .unwrap()
            .fields
            .insert("BaseItem".into(), 7);
        let upkeep = |a: i32| Effect {
            ty: 1016,
            ints: vec![0, a],
            ..Default::default()
        };
        {
            let o = s.world.get_mut(c).unwrap();
            o.equipment.insert(0, sword);
            o.equipment.insert(1, shield);
            o.properties.insert(27, (2.0, 0.0)); // a mage
            o.effects = vec![upkeep(12), upkeep(20), upkeep(21), upkeep(22)];
        }
        assert_eq!(s.active_modal_abilities(c), vec![12, 20, 21, 22]);
        // All met but the warrior-only one (the creature is a mage).
        assert_eq!(s.conditioned_abilities(c, 199 | 0xe00 | 0x10), vec![22]);
        // The mask limits what is checked.
        assert!(s.conditioned_abilities(c, 199).is_empty());
        // Unequipping the shield breaks Shield Wall only; the sword stance still holds.
        s.world.get_mut(c).unwrap().equipment.remove(&1);
        assert_eq!(s.conditioned_abilities(c, 199), vec![12]);
        // With Shield Wall gone, the stance that needs it breaks.
        s.world.get_mut(c).unwrap().effects.remove(0);
        assert_eq!(s.conditioned_abilities(c, 0x10), vec![21]);
    }

    #[test]
    fn attackers_turn_to_face_their_target_when_the_swing_starts() {
        let mut s = sim();
        let area = s.world.spawn(ObjectKind::Area, "a");
        let bandit = s.world.spawn(ObjectKind::Creature, "bandit");
        let hero = s.world.spawn(ObjectKind::Creature, "hero");
        for (id, p) in [(bandit, [0.0, 0.0, 0.0]), (hero, [0.0, -2.0, 0.0])] {
            let o = s.world.get_mut(id).unwrap();
            o.area = Some(area);
            o.position = p;
            o.facing = 0.0; // looking along +Y, away from the hero
        }
        s.start_attack(bandit, hero, 0);
        // The hero is along -Y: a half turn.
        let f = s.world.get(bandit).unwrap().facing;
        assert!((f - 180.0).abs() < 1e-3, "{f}");
        // A target elsewhere or the attacker itself leaves the facing alone.
        s.face_target(bandit, bandit);
        s.world.get_mut(hero).unwrap().area = None;
        s.face_target(bandit, hero);
        assert!((s.world.get(bandit).unwrap().facing - 180.0).abs() < 1e-3);
    }

    /// A creature with the combat tree `test.anb` and the tables an ability's timing reads:
    /// a spell (10) conjured with 244 (followed by its loop, 264) and cast with 246 at
    /// conjure speed row `speed`, and a talent (20) cast with 147.
    fn timed(speed: i64) -> (Sim, ObjectId) {
        let mut s = sim();
        let t = crate::m2da::Tables::empty();
        t.insert(
            "#1",
            &["ID", "#f95409f5", "#9abeaef1", "#07f4f006"],
            vec![
                (
                    10,
                    vec![
                        Cell::Int(10),
                        Cell::Int(246),
                        Cell::Int(244),
                        Cell::Int(speed),
                    ],
                ),
                (
                    20,
                    vec![Cell::Int(20), Cell::Int(147), Cell::Int(0), Cell::Int(3)],
                ),
            ],
        );
        let anim = |id: i64, action: &str, flags: i64| {
            (
                id,
                vec![
                    Cell::Int(id),
                    Cell::String(action.into()),
                    Cell::Int(flags),
                    Cell::Int(if id == 244 { 264 } else { 1 }),
                ],
            )
        };
        t.insert(
            "#0",
            &["ID", "#7a398af9", "#12a02374", "#ffc372e7"],
            vec![
                anim(147, "talent_charge", 4096),
                anim(244, "ua_dconjure_start", 4096),
                anim(246, "ua_dcast", 4096),
                anim(264, "ua_dconjure_loop", 4097),
            ],
        );
        t.insert(
            "#135",
            &["ID", "#07f4f006"],
            (1..=3)
                .map(|r| (r, vec![Cell::Int(r), Cell::Int(r - 1)]))
                .collect(),
        );
        t.insert(
            "#2",
            &["ID", "#de80b418"],
            vec![(1, vec![Cell::Int(1), Cell::String("test.anb".into())])],
        );
        s.world.tables = Arc::new(t);
        // The animations as the trees would give them: lengths and first impact event.
        let len = |raw_ms, impact_ms| {
            Some(AbilityAnimLen {
                raw_ms,
                len_ms: raw_ms,
                impact_ms,
            })
        };
        for (action, a) in [
            ("talent_charge", len(1567, Some(970))),
            ("ua_dconjure_start", len(467, None)),
            ("ua_dconjure_loop", len(3400, None)),
            ("ua_dcast", len(1333, Some(900))),
        ] {
            s.ability_anims.insert(format!("test.anb|{action}"), a);
        }
        let area = s.world.spawn(ObjectKind::Area, "a");
        let c = s.world.spawn(ObjectKind::Creature, "caster");
        let o = s.world.get_mut(c).unwrap();
        o.appearance = 1;
        o.area = Some(area);
        (s, c)
    }

    #[test]
    fn an_abilitys_impact_and_end_come_from_its_animations() {
        // A talent: its cast animation's impact event, and its end.
        let (mut s, c) = timed(1);
        let t = s.ability_timing(c, 20).unwrap();
        assert_eq!((t.impact_ms, t.end_ms), (970, 1567));
        // An instant spell: the conjure's start animation, then the cast's.
        assert_eq!(
            s.ability_timing(c, 10),
            Some(AbilityTiming {
                impact_ms: 467 + 900,
                end_ms: 467 + 1333,
            })
        );
        // A fast one loops the conjure once, a normal one twice.
        let (mut s, c) = timed(2);
        assert_eq!(s.ability_timing(c, 10).unwrap().impact_ms, 467 + 3400 + 900);
        let (mut s, c) = timed(3);
        assert_eq!(
            s.ability_timing(c, 10).unwrap().impact_ms,
            467 + 2 * 3400 + 900
        );
        // No cast animation, or none in the creature's trees: no timing (the default holds).
        assert_eq!(s.ability_timing(c, 30), None);
        s.world.get_mut(c).unwrap().appearance = 2;
        assert_eq!(s.ability_timing(c, 20), None);
    }

    #[test]
    fn an_ability_lands_at_its_impact_and_its_command_ends_with_the_animation() {
        let (mut s, c) = timed(1);
        s.world.trace.keep = true;
        s.order_ability(c, 20, c, "");
        s.tick(0);
        let t0 = s.world.time_ms;
        let impacts = |s: &Sim| {
            s.world
                .trace
                .entries
                .iter()
                .filter(|e| {
                    matches!(e, crate::trace::TraceEntry::Log { text, .. }
                        if text.starts_with("ABILITY_CAST_IMPACT"))
                })
                .count()
        };
        s.tick(969);
        assert_eq!(impacts(&s), 0);
        s.tick(1);
        assert_eq!(impacts(&s), 1);
        // The command runs on to the animation's end, without a second impact.
        assert_eq!(s.world.get(c).unwrap().commands.len(), 1);
        s.tick(1567 - 970 - 1);
        assert_eq!(s.world.get(c).unwrap().commands.len(), 1);
        s.tick(1);
        assert!(s.world.get(c).unwrap().commands.is_empty());
        assert_eq!(impacts(&s), 1);
        assert_eq!(s.world.time_ms, t0 + 1567);
    }

    #[test]
    fn mana_running_out_tells_the_creature_once() {
        let mut s = sim();
        let c = s.world.spawn(ObjectKind::Creature, "mage");
        s.world
            .get_mut(c)
            .unwrap()
            .properties
            .insert(8, (10.0, 3.0));
        let depleted = |s: &Sim| {
            s.world
                .queue
                .values()
                .filter(|q| q.target == c && q.event.ty == 64)
                .count()
        };
        s.note_mana_change(c, 5.0);
        assert_eq!(depleted(&s), 0, "still above 0");
        s.world
            .get_mut(c)
            .unwrap()
            .properties
            .insert(8, (10.0, 0.0));
        s.note_mana_change(c, 3.0);
        assert_eq!(depleted(&s), 1);
        // Already at 0: nothing more.
        s.note_mana_change(c, 0.0);
        assert_eq!(depleted(&s), 1);
    }

    #[test]
    fn regeneration_adds_each_rate_once_a_period_by_the_creatures_combat_state() {
        let mut s = sim();
        let t = crate::m2da::Tables::empty();
        // Health (7) regenerates by 14 out of combat and 28 in it, mana/stamina (8) by 29
        // and 30; the regeneration properties' bounds clamp their totals.
        let row = |id: i64, ty: &str, min: i64, max: i64, regen: i64, combat: i64| {
            (
                id,
                vec![
                    Cell::Int(id),
                    Cell::String(ty.into()),
                    Cell::Int(min),
                    Cell::Int(max),
                    Cell::Int(regen),
                    Cell::Int(combat),
                ],
            )
        };
        t.insert(
            "properties",
            &["ID", "Type", "Min", "Max", "#7a3df160", "#e24b074d"],
            vec![
                row(7, "DEPLETABLE", 1, 9999, 14, 28),
                row(8, "DEPLETABLE", 0, 9999, 29, 30),
                row(14, "ATTRIBUTE", 0, 50, 0, 0),
                row(28, "ATTRIBUTE", 0, 20, 0, 0),
                row(29, "ATTRIBUTE", -20, 50, 0, 0),
                row(30, "ATTRIBUTE", -20, 20, 0, 0),
            ],
        );
        s.world.tables = Arc::new(t);
        let c = s.world.spawn(ObjectKind::Creature, "warden");
        {
            let o = s.world.get_mut(c).unwrap();
            for (p, v) in [(7, (100.0, 50.0)), (8, (100.0, 0.0))] {
                o.properties.insert(p, v);
            }
            for (p, v) in [(14, 6.0), (28, 2.0), (29, 3.0), (30, 1.0)] {
                o.properties.insert(p, (v, 0.0));
            }
        }
        let now = |s: &Sim| {
            let o = s.world.get(c).unwrap();
            (o.properties[&7].1, o.properties[&8].1)
        };
        // Out of combat, a pass comes once more than 3 s have gathered.
        s.regenerate(3000);
        assert_eq!(now(&s), (50.0, 0.0));
        s.regenerate(1);
        assert_eq!(now(&s), (56.0, 3.0));
        // Many periods at once: one pass each.
        s.regenerate(6000);
        assert_eq!(now(&s), (68.0, 9.0));
        // In combat (the creature's own state), every 2 s, by the combat rates.
        s.world.get_mut(c).unwrap().combat.in_combat = true;
        s.regenerate(2000);
        assert_eq!(now(&s), (70.0, 10.0));
        // Never past the maximum; nothing for the fallen (no health left).
        s.world.get_mut(c).unwrap().combat.in_combat = false;
        s.regenerate(30_000);
        assert_eq!(now(&s), (100.0, 40.0));
        s.world
            .get_mut(c)
            .unwrap()
            .properties
            .insert(7, (100.0, 0.0));
        s.regenerate(9000);
        assert_eq!(now(&s), (0.0, 40.0));
    }

    #[test]
    fn each_hand_with_a_result_lands_its_own_blow() {
        let mut s = sim();
        let area = s.world.spawn(ObjectKind::Area, "a");
        let rogue = s.world.spawn(ObjectKind::Creature, "rogue");
        let wolf = s.world.spawn(ObjectKind::Creature, "wolf");
        for id in [rogue, wolf] {
            s.world.get_mut(id).unwrap().area = Some(area);
        }
        let hit = |amount: f32| {
            Some(Effect {
                ty: 1024,
                floats: vec![amount],
                ..Default::default()
            })
        };
        let attack = |s: &mut Sim, results: [i32; 2], effects: [Option<Effect>; 2]| {
            s.world.get_mut(rogue).unwrap().combat.attack = Some(AttackResult { results, effects });
            s.finish_attack(rogue, wolf);
            assert!(s.world.get(rogue).unwrap().combat.attack.is_none());
            let a = s
                .world
                .trace
                .attacks
                .get(&rogue)
                .cloned()
                .unwrap_or_default();
            (a.swings, a.hits, a.damage)
        };
        // The presentation's view of an attack in progress: each hand's result and target.
        s.world
            .get_mut(rogue)
            .unwrap()
            .commands
            .push(crate::world::Command {
                ty: 0,
                name: "CommandAttack".into(),
                target: Some(wolf),
                location: None,
                seconds: 0.0,
                ints: Vec::new(),
                strings: Vec::new(),
                due: None,
                from: None,
                approach: None,
                ring_wait: false,
                after_impact_ms: None,
                impacted: false,
                arrived: false,
                path: Vec::new(),
            });
        s.world.get_mut(rogue).unwrap().combat.attack = Some(AttackResult {
            results: [0, 2],
            effects: [None, None],
        });
        assert_eq!(s.attack_target(rogue), Some(wolf));
        assert_eq!(s.hand_attack(rogue, 0), None);
        assert_eq!(s.hand_attack(rogue, 1), Some((2, wolf)));
        assert_eq!(s.hand_attack(rogue, 2), None);
        s.world.get_mut(rogue).unwrap().commands.clear();
        assert_eq!(s.attack_target(rogue), None);
        // The off hand alone (the main hand has no result, 0): its blow lands.
        assert_eq!(attack(&mut s, [0, 1], [None, hit(7.0)]), (1, 1, 7.0));
        // The main hand alone misses (3); nothing for the empty off hand.
        assert_eq!(attack(&mut s, [3, 0], [None, None]), (2, 1, 7.0));
        // Both hands: two blows, each with its own damage.
        assert_eq!(attack(&mut s, [1, 2], [hit(5.0), hit(9.0)]), (4, 3, 21.0));
    }

    #[test]
    fn creatures_in_view_are_announced_again_when_their_hostility_changes() {
        let mut s = sim();
        let area = s.world.spawn(ObjectKind::Area, "a");
        let hero = s.world.spawn(ObjectKind::Creature, "hero");
        let bandit = s.world.spawn(ObjectKind::Creature, "bandit");
        s.world.hero = Some(hero);
        for (id, x, g) in [(hero, 0.0, 0), (bandit, 5.0, 2)] {
            let o = s.world.get_mut(id).unwrap();
            o.area = Some(area);
            o.position = [x, 0.0, 0.0];
            o.group = g;
        }
        // (time, target, hostile flag) of the appearances the bandit is told about.
        let appearances = |s: &Sim| -> Vec<i32> {
            s.world
                .queue
                .values()
                .filter(|q| q.target == bandit && q.event.ty == 25)
                .map(|q| q.event.ints[2])
                .collect()
        };
        s.update_perception();
        assert_eq!(appearances(&s), vec![0], "first seen, not hostile");
        s.world.queue.clear();
        s.update_perception();
        assert!(appearances(&s).is_empty(), "nothing changed");
        s.world.hostility.insert((0, 2), true);
        s.update_perception();
        assert_eq!(appearances(&s), vec![1], "turned hostile");
        s.world.queue.clear();
        s.world.hostility.insert((0, 2), false);
        s.update_perception();
        assert_eq!(
            appearances(&s),
            vec![1],
            "no longer hostile: flagged as it was seen"
        );
    }

    #[test]
    fn modal_abilities_stay_on_until_their_effects_run_out() {
        let mut s = sim();
        let who = s.world.spawn(ObjectKind::Creature, "berserker");
        let upkeep = Effect {
            ty: 1016,
            ints: vec![1, 700],
            expires: Some(1000),
            ..Default::default()
        };
        s.world.get_mut(who).unwrap().effects.push(upkeep);
        assert!(s.modal_active(who, 700));
        assert!(!s.modal_active(who, 701));
        s.world.time_ms = 999;
        s.expire_effects();
        assert!(s.modal_active(who, 700), "not yet");
        s.world.time_ms = 1000;
        s.expire_effects();
        assert!(!s.modal_active(who, 700));
        assert!(s.world.get(who).unwrap().effects.is_empty());
    }

    #[test]
    fn the_engine_tracks_the_fight_and_leaves_the_game_mode_to_the_scripts() {
        let mut s = sim();
        let area = s.world.spawn(ObjectKind::Area, "a");
        let module = s.world.spawn(ObjectKind::Module, "m");
        s.world.module = Some(module);
        let hero = s.world.spawn(ObjectKind::Creature, "hero");
        let foe = s.world.spawn(ObjectKind::Creature, "foe");
        s.world.hero = Some(hero);
        for (id, x, g) in [(hero, 0.0, 0), (foe, 5.0, 2)] {
            let o = s.world.get_mut(id).unwrap();
            o.area = Some(area);
            o.position = [x, 0.0, 0.0];
            o.group = g;
        }
        s.world.hostility.insert((0, 2), true);
        s.update_perception();
        assert!(s.world.party_fighting);
        // The mode is the scripts' to change (rules_core asks the module for GM_COMBAT).
        assert_eq!(s.world.game_mode, 0);
        assert!(
            !s.world
                .queue
                .values()
                .any(|q| q.target == module && q.event.ints.len() == 2)
        );
        s.world.get_mut(foe).unwrap().dead = true;
        s.update_perception();
        assert!(!s.world.party_fighting);
        assert!(!s.world.game_over);
        // A resurrection effect brings the dead back.
        s.world.get_mut(hero).unwrap().dead = true;
        s.apply_effect(
            hero,
            Effect {
                ty: EFFECT_TYPE_RESURRECTION,
                ..Default::default()
            },
        );
        assert!(!s.world.get(hero).unwrap().dead);
    }

    #[test]
    fn on_autopilot_the_party_is_clear_to_attack_and_an_idle_hero_waits_for_its_ai() {
        let mut s = sim();
        let area = s.world.spawn(ObjectKind::Area, "a");
        let module = s.world.spawn(ObjectKind::Module, "m");
        s.world.module = Some(module);
        let hero = s.world.spawn(ObjectKind::Creature, "hero");
        let foe = s.world.spawn(ObjectKind::Creature, "foe");
        s.world.hero = Some(hero);
        for (id, x, g) in [(hero, 0.0, 0), (foe, 5.0, 2)] {
            let o = s.world.get_mut(id).unwrap();
            o.area = Some(area);
            o.position = [x, 0.0, 0.0];
            o.group = g;
        }
        s.world.hostility.insert((0, 2), true);
        let clear = |s: &Sim| {
            s.world
                .get(module)
                .unwrap()
                .locals
                .ints
                .get("AI_PARTY_CLEAR_TO_ATTACK")
                .copied()
        };
        // Under the player's control the player opens fights (the scripts set the flag).
        s.update_perception();
        assert_eq!(clear(&s), None);
        assert!(s.world.get(hero).unwrap().commands.is_empty());
        s.world.autopilot = true;
        s.update_perception();
        assert_eq!(clear(&s), Some(1));
        s.autopilot_idle_hero();
        let waits: Vec<_> = s
            .world
            .get(hero)
            .unwrap()
            .commands
            .iter()
            .map(|c| (c.name.clone(), c.seconds))
            .collect();
        assert_eq!(waits, vec![("CommandWait".to_string(), 1.0)]);
        // A busy hero is left alone.
        s.autopilot_idle_hero();
        assert_eq!(s.world.get(hero).unwrap().commands.len(), 1);
    }

    #[test]
    fn the_fallen_have_no_health_left() {
        let mut s = sim();
        let c = s.world.spawn(ObjectKind::Creature, "c");
        s.world
            .get_mut(c)
            .unwrap()
            .properties
            .insert(7, (120.0, 64.0));
        s.kill(c, OBJECT_INVALID);
        let o = s.world.get(c).unwrap();
        assert!(o.dead);
        assert_eq!(o.properties[&7], (120.0, 0.0));
    }

    #[test]
    fn health_taken_is_credited_to_the_effect_creator_or_the_scripts_owner() {
        let mut s = sim();
        let (archer, mage, hero) = (
            s.world.spawn(ObjectKind::Creature, "archer"),
            s.world.spawn(ObjectKind::Creature, "mage"),
            s.world.spawn(ObjectKind::Creature, "hero"),
        );
        // A weapon hit: the attacker's own script lowers the target's health.
        s.record_health_loss(archer, hero, 8.0);
        // A spell: the target's script applies the mage's damage effect.
        s.current_effect = Some((
            Effect {
                ty: 1013,
                creator: Some(mage),
                ability: 14001,
                ..Default::default()
            },
            true,
        ));
        s.current_effect_target = Some(hero);
        s.record_health_loss(hero, hero, 30.0);
        s.record_health_loss(hero, hero, 0.0);
        let d = &s.world.trace.damage;
        assert_eq!(d.get(&(archer, 0)), Some(&8.0));
        assert_eq!(d.get(&(mage, 14001)), Some(&30.0));
        assert_eq!(d.len(), 2);
        // A burn's tick: the burning hero's own script takes its health, outside any
        // effect; the burn's creator (and ability) is credited, the latest burn's when there
        // are several.
        s.current_effect = None;
        let burn = |creator, ability| Effect {
            ty: EFFECT_TYPE_DOT,
            creator: Some(creator),
            ability,
            ..Default::default()
        };
        s.world.get_mut(hero).unwrap().effects = vec![burn(archer, 3000), burn(mage, 10003)];
        s.record_health_loss(hero, hero, 9.0);
        assert_eq!(s.world.trace.damage.get(&(mage, 10003)), Some(&9.0));
        // Without a burn from someone else, the owner is credited as before.
        s.world.get_mut(hero).unwrap().effects = vec![burn(hero, 1)];
        s.record_health_loss(hero, hero, 4.0);
        assert_eq!(s.world.trace.damage.get(&(hero, 0)), Some(&4.0));
    }

    #[test]
    fn only_hostiles_need_a_clear_line_to_be_noticed() {
        let mut s = sim();
        let area = s.world.spawn(ObjectKind::Area, "a");
        // A wall (9) across x = 5 on a 10 x 3 grid of 1 m cells.
        let (w, h) = (10u32, 3u32);
        let mut data = vec![1u8; (w * h) as usize];
        for y in 0..h {
            data[(y * w + 5) as usize] = 9;
        }
        s.area_layouts.insert(area, "walled".into());
        s.nav_grids.insert(
            "walled".into(),
            Some(std::sync::Arc::new(thedas_formats::level::NavGrid {
                columns: w,
                rows: h,
                cell_size: 1.0,
                clearance: 2.0,
                base: [0.0, 0.0, 0.0],
                normal: [0.0, 0.0, 1.0],
                data,
                heights: vec![0.0; (w * h) as usize],
                surfaces: Vec::new(),
            })),
        );
        let hero = s.world.spawn(ObjectKind::Creature, "hero");
        let friend = s.world.spawn(ObjectKind::Creature, "friend");
        let foe = s.world.spawn(ObjectKind::Creature, "foe");
        s.world.hero = Some(hero);
        for (id, y, group) in [(hero, 1.5, 0), (friend, 0.5, 0), (foe, 2.5, 1)] {
            let o = s.world.get_mut(id).unwrap();
            o.area = Some(area);
            o.position = [1.5, y, 0.0];
            o.group = group;
        }
        for id in [friend, foe] {
            s.world.get_mut(id).unwrap().position[0] = 8.5;
        }
        s.world.hostility.insert((0, 1), true);
        s.update_perception();
        assert_eq!(s.world.get(hero).unwrap().combat.perceived, vec![friend]);
    }

    #[test]
    fn perception_class_sets_the_ring_and_noticed_creatures_stay_seen() {
        let mut s = sim();
        // ranges.gda: PercepRngShrt (8) 10 m, PercepRngPlayer (12) 35 m.
        s.world.tables().insert(
            "#134",
            &["PrimaryRange", "SecondaryRange"],
            vec![
                (8, vec![Cell::Float(10.0), Cell::Float(10.0)]),
                (12, vec![Cell::Float(35.0), Cell::Float(30.0)]),
            ],
        );
        let area = s.world.spawn(ObjectKind::Area, "a");
        let hero = s.world.spawn(ObjectKind::Creature, "hero");
        let near_sighted = s.world.spawn(ObjectKind::Creature, "corpse");
        s.world.hero = Some(hero);
        for (id, x) in [(hero, 0.0), (near_sighted, 20.0)] {
            let o = s.world.get_mut(id).unwrap();
            o.area = Some(area);
            o.position = [x, 0.0, 0.0];
        }
        s.world
            .get_mut(near_sighted)
            .unwrap()
            .fields
            .insert("PerceptionClass".into(), 8);
        assert_eq!(s.perception_range(hero), 35.0, "the party's row");
        assert_eq!(s.perception_range(near_sighted), 10.0);
        s.update_perception();
        // 20 m apart: inside the hero's 35 m ring, outside the corpse's 10 m one.
        assert_eq!(
            s.world.get(hero).unwrap().combat.perceived,
            vec![near_sighted]
        );
        assert!(
            s.world
                .get(near_sighted)
                .unwrap()
                .combat
                .perceived
                .is_empty()
        );
        // Once noticed, a creature is kept track of well beyond the ring.
        s.world.get_mut(near_sighted).unwrap().position = [50.0, 0.0, 0.0];
        s.update_perception();
        assert_eq!(
            s.world.get(hero).unwrap().combat.perceived,
            vec![near_sighted]
        );
        s.world.get_mut(near_sighted).unwrap().position = [70.0, 0.0, 0.0];
        s.update_perception();
        assert!(s.world.get(hero).unwrap().combat.perceived.is_empty());
    }

    #[test]
    fn a_creature_standing_in_a_walls_end_still_sees_its_enemies() {
        // The Dalish ruins' mirror room, turned to the axes: a wall 0.7 m thick ends at
        // y = 2.17 beside a doorway, and a companion stands 16 cm inside its end, in a
        // pathing cell the wall's end straddles. The enemy is 3.8 m away in the open.
        let mut s = sim();
        let area = s.world.spawn(ObjectKind::Area, "a");
        let (w, h) = (24u32, 24u32);
        let mut data = vec![1u8; (w * h) as usize];
        // The wall's cells are unwalkable up to y = 2 (the cell 2.0..2.5 holds its end).
        for y in 0..4 {
            data[(y * w + 4) as usize] = 10;
        }
        s.area_layouts.insert(area, "ruins".into());
        s.nav_grids.insert(
            "ruins".into(),
            Some(std::sync::Arc::new(thedas_formats::level::NavGrid {
                columns: w,
                rows: h,
                cell_size: 0.5,
                clearance: 2.0,
                base: [0.0, 0.0, 0.0],
                normal: [0.0, 0.0, 1.0],
                data,
                heights: vec![0.0; (w * h) as usize],
                surfaces: Vec::new(),
            })),
        );
        s.collisions.insert(
            "ruins".into(),
            Some(std::sync::Arc::new(
                crate::collision::Collision::from_boxes(&[([2.25, 0.5, 2.75], [0.35, 1.67, 2.75])]),
            )),
        );
        let hero = s.world.spawn(ObjectKind::Creature, "hero");
        let tamlen = s.world.spawn(ObjectKind::Creature, "tamlen");
        let bear = s.world.spawn(ObjectKind::Creature, "bear");
        s.world.hero = Some(hero);
        for (id, p, g) in [
            (hero, [7.75, 2.25, 0.0], 0),
            (tamlen, [2.13, 2.01, 0.0], 0),
            (bear, [5.9, 2.2, 0.0], 2),
        ] {
            let o = s.world.get_mut(id).unwrap();
            o.area = Some(area);
            o.position = p;
            o.group = g;
        }
        s.world.hostility.insert((0, 2), true);
        s.update_perception();
        let sees = |s: &Sim, a: ObjectId| s.world.get(a).unwrap().combat.perceived_hostile.clone();
        assert_eq!(sees(&s, tamlen), vec![bear]);
        assert!(sees(&s, bear).contains(&tamlen) && sees(&s, bear).contains(&hero));
        // Behind the wall he is hidden as before.
        s.world.get_mut(tamlen).unwrap().position = [1.25, 0.75, 0.0];
        s.world.get_mut(tamlen).unwrap().combat.perceived.clear();
        s.world.get_mut(bear).unwrap().combat.perceived.clear();
        s.world.get_mut(bear).unwrap().position = [5.9, 0.6, 0.0];
        s.update_perception();
        assert!(sees(&s, tamlen).is_empty());
    }

    #[test]
    fn creatures_perceive_hostiles_in_range_and_die_at_zero_health() {
        let mut s = sim();
        let area = s.world.spawn(ObjectKind::Area, "a");
        let hero = s.world.spawn(ObjectKind::Creature, "hero");
        let foe = s.world.spawn(ObjectKind::Creature, "foe");
        let far = s.world.spawn(ObjectKind::Creature, "far");
        s.world.hero = Some(hero);
        for (id, x, g) in [(hero, 0.0, 0), (foe, 5.0, 2), (far, 50.0, 2)] {
            let o = s.world.get_mut(id).unwrap();
            o.area = Some(area);
            o.position = [x, 0.0, 0.0];
            o.group = g;
        }
        s.world.hostility.insert((0, 2), true);
        assert!(s.hostile(hero, foe) && !s.hostile(foe, far));
        s.update_perception();
        assert_eq!(s.world.get(hero).unwrap().combat.perceived, vec![foe]);
        let appear: Vec<_> = s
            .world
            .queue
            .values()
            .map(|q| (q.target, q.event.ty, q.event.ints.clone()))
            .collect();
        assert!(appear.contains(&(hero, 25, vec![0, 0, 1])));
        assert!(appear.contains(&(foe, 25, vec![0, 0, 1])));
        s.world.queue.clear();

        s.world
            .get_mut(foe)
            .unwrap()
            .properties
            .insert(7, (10.0, 0.0));
        s.world.time_ms = 4_200;
        s.check_death(foe, hero);
        assert!(s.world.get(foe).unwrap().dead);
        assert_eq!(s.world.get(foe).unwrap().died_at, Some(4_200));
        let death: Vec<_> = s
            .world
            .queue
            .values()
            .map(|q| (q.target, q.event.ty))
            .collect();
        assert_eq!(death, vec![(foe, 4)]);
        // The dead drop out of perception.
        s.world.queue.clear();
        s.update_perception();
        assert!(s.world.get(hero).unwrap().combat.perceived.is_empty());
    }
}

#[cfg(test)]
mod attack_tree_tests {
    use super::attack_tree;

    #[test]
    fn a_humanoids_weapons_pick_its_attack_tree() {
        let h = "combat_humanoid.anb";
        assert_eq!(attack_tree(h, "melee", 3, "shield"), "ws.anb");
        assert_eq!(attack_tree(h, "melee", 3, "melee"), "dw.anb");
        assert_eq!(attack_tree(h, "melee", 1, ""), "tw.anb");
        assert_eq!(attack_tree(h, "melee", 3, ""), "ow.anb");
        assert_eq!(attack_tree(h, "", 0, ""), h, "unarmed");
        assert_eq!(attack_tree(h, "ranged", 1, ""), h);
        assert_eq!(
            attack_tree("c_doga_cbt.anb", "melee", 3, ""),
            "c_doga_cbt.anb"
        );
    }
}
