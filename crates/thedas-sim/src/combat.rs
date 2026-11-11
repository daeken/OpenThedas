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
//! - **Game mode.** The party seeing living hostiles switches `GM_EXPLORE` to `GM_COMBAT`
//!   and back, with `GAMEMODE_CHANGE` to the module; the scripts end combat (and get fallen
//!   party members up) on the change back.
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

/// `EFFECT_TYPE_DEATH` in effects.gda.
pub const EFFECT_TYPE_DEATH: i32 = 7;

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
    /// `SetStealthEnabled`: the creature is in stealth (the stealth effect's handler sets
    /// and clears it).
    #[serde(default)]
    pub stealth: bool,
    /// Stealthed hostiles in range this creature has been told about (`PERCEPTION_APPEAR`
    /// with the stealthed flag), without perceiving them.
    #[serde(default)]
    pub noticed_stealthed: Vec<ObjectId>,
}

/// One tactics slot: when `condition` holds for a target of `target` type, do `command`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Tactic {
    pub enabled: bool,
    pub target: i32,
    pub condition: i32,
    pub command: i32,
    pub param: i32,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AttackResult {
    pub results: [i32; 2],
    pub effects: [Option<Effect>; 2],
}

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
        // Closed doors block sight (their doorways, see `Sim::closed_doors`).
        let doors: Vec<[f32; 3]> = self
            .closed_doors(area)
            .into_iter()
            .filter_map(|(d, _)| self.world.get(d).map(|o| o.position))
            .collect();
        let blocked = |a: [f32; 3], b: [f32; 3]| Sim::door_between(&doors, a, b);
        // Walls block noticing (`nav::wall_between`); once noticed, a creature is kept
        // track of round corners out to the visual radius.
        let grid = self.nav_grid(area);
        let wall = |a: [f32; 3], b: [f32; 3]| {
            grid.as_ref()
                .is_some_and(|g| crate::nav::wall_between(g, a, b))
        };
        for &(id, pos) in &creatures {
            let alive = self.alive(id);
            // A creature notices others within its perception range (see
            // `Sim::perception_range`) and keeps track of those it has noticed out to the
            // visual radius. Closed doors block sight, and a wall of the pathing grid blocks
            // noticing. Provisional: the grid, the only geometry the simulation has, marks
            // large furniture and rubble the same way as walls, so those block too.
            let ring = self.perception_range(id);
            let seen = self
                .world
                .get(id)
                .map(|o| o.combat.perceived.clone())
                .unwrap_or_default();
            let in_view: Vec<ObjectId> = if alive {
                creatures
                    .iter()
                    .filter(|&&(o, p)| {
                        if o == id || !self.alive(o) {
                            return false;
                        }
                        let d = dist(pos, p);
                        let tracked = seen.contains(&o) && !self.hidden_from(id, o);
                        if tracked {
                            d <= VISUAL_RANGE && !blocked(pos, p)
                        } else {
                            d <= ring && !blocked(pos, p) && !wall(pos, p)
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

    /// Switches between `GM_EXPLORE` and `GM_COMBAT` as the party starts or stops seeing
    /// living hostiles, telling the module (`GAMEMODE_CHANGE`: new mode, old mode). Other
    /// modes (conversation, cutscene) are left alone. Provisional.
    pub fn update_game_mode(&mut self) {
        let (explore, combat) = (
            constant_of(self, "GM_EXPLORE"),
            constant_of(self, "GM_COMBAT"),
        );
        let mode = self.world.game_mode;
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
        // A fight that ended with fallen party members gets them up
        // (EVENT_TYPE_DEATH_RES_PARTY to the module), whatever the game mode is, as long as
        // someone was left standing. When the whole party falls the game is over: the engine
        // shows the death screen itself (no script calls ShowDeathScreen).
        // The party AI holds followers back until the party is "clear to attack" (the
        // module's AI_PARTY_CLEAR_TO_ATTACK), which rules_core sets when the controlled
        // character attacks or uses an ability on a hostile. On autopilot nobody is controlled,
        // so the autopilot stands in for the player and opens every fight itself.
        if self.world.autopilot && fighting {
            if let Some(m) = self.world.module.and_then(|m| self.world.get_mut(m)) {
                m.locals.ints.insert("AI_PARTY_CLEAR_TO_ATTACK".into(), 1);
            }
        }
        let was = std::mem::replace(&mut self.world.party_fighting, fighting);
        let standing = party.iter().any(|&m| self.alive(m));
        if !party.is_empty() && !standing && !self.world.game_over {
            self.world.game_over = true;
            let t = self.world.time_ms;
            self.world.trace.push(crate::trace::TraceEntry::Log {
                t,
                channel: -1,
                text: "the whole party has fallen: game over".into(),
            });
        }
        if was
            && !fighting
            && standing
            && party
                .iter()
                .any(|&m| self.world.get(m).is_some_and(|o| o.dead))
        {
            if let Some(m) = self.world.module {
                let ev = self.new_event("DEATH_RES_PARTY", Some(m));
                self.world.queue_event(0, m, ev, None);
            }
        }
        if mode != explore && mode != combat {
            return;
        }
        let new = if fighting { combat } else { explore };
        if new == mode || explore == combat {
            return;
        }
        self.world.game_mode = new;
        if let Some(m) = self.world.module {
            let mut ev = self.new_event("GAMEMODE_CHANGE", Some(m));
            ev.ints = vec![new, mode];
            self.world.queue_event(0, m, ev, None);
        }
    }

    /// Recovery: living creatures regain health and mana/stamina each second by their
    /// `Regeneration_Health` and `Regeneration_Stamina` properties (`properties.gda`), or by
    /// the `_Combat` variants while the game is in combat mode or they are fighting, up to
    /// their maximum. Provisional: per-second rates are a guess.
    pub fn regenerate(&mut self, ms: u64) {
        if ms == 0 {
            return;
        }
        let (health, mana) = (
            constant_of(self, "PROPERTY_DEPLETABLE_HEALTH"),
            constant_of(self, "PROPERTY_DEPLETABLE_MANA_STAMINA"),
        );
        let rates = {
            let t = self.world.tables.get(0, "properties");
            let id = |label: &str| {
                t.as_ref()
                    .and_then(|t| t.find("Stat", label))
                    .map_or(-1, |v| v as i32)
            };
            [
                (id("Regeneration_Health"), id("Regeneration_Stamina")),
                (
                    id("Regeneration_Health_Combat"),
                    id("Regeneration_Stamina_Combat"),
                ),
            ]
        };
        let combat_mode = self.world.game_mode == constant_of(self, "GM_COMBAT");
        let secs = ms as f32 / 1000.0;
        // Each living creature's maxima (base, equipment and attributes).
        let maxima: Vec<(ObjectId, f32, f32)> = self
            .world
            .objects
            .values()
            .filter(|o| o.kind == ObjectKind::Creature && !o.dead)
            .map(|o| {
                (
                    o.id,
                    self.depletable_max(o.id, health),
                    self.depletable_max(o.id, mana),
                )
            })
            .collect();
        for (id, max_h, max_m) in maxima {
            let Some(o) = self.world.objects.get_mut(&id) else {
                continue;
            };
            let (h, m) = rates[(combat_mode || o.combat.in_combat) as usize];
            for (p, r, max) in [(health, h, max_h), (mana, m, max_m)] {
                let rate = o.properties.get(&r).map_or(0.0, |p| p.0)
                    + o.modifiers.get(&r).copied().unwrap_or(0.0);
                if rate == 0.0 {
                    continue;
                }
                if let Some(v) = o.properties.get_mut(&p) {
                    if v.1 < max || rate < 0.0 {
                        v.1 = (v.1 + rate * secs).clamp(0.0, max);
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
        let cmd = crate::world::Command {
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
            path: Vec::new(),
        };
        if let Some(o) = self.world.get_mut(attacker) {
            o.commands.clear();
            o.commands.push(cmd);
            o.combat.in_combat = true;
        }
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

    /// The front `CommandUseAbility` started: the owner's script vets it
    /// (`COMMAND_PENDING`: objects owner, target, item; integers command type, ability), then
    /// `ability_core` hears `ABILITY_CAST_START`.
    /// Provisional: the pending event's third object is inferred from `ability_core`, which
    /// reads it as the item.
    pub(crate) fn start_ability(&mut self, id: ObjectId, cmd: &crate::world::Command) {
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
        self.ability_event("ABILITY_CAST_START", id, target, ability, cmd);
    }

    /// `ABILITY_CAST_START` or `ABILITY_CAST_IMPACT` to `ability_core`, which the original
    /// executable names directly (the rules scripts do not handle these events): objects
    /// caster, item, target; integers ability, then three the script reads but we leave 0.
    /// Provisional: the layout is read from `ability_core`'s bytecode, the zeros are guesses.
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
    pub(crate) fn finish_attack(&mut self, id: ObjectId, target: ObjectId) {
        let Some(result) = self.world.get(id).and_then(|o| o.combat.attack.clone()) else {
            return;
        };
        for hand in 0..2 {
            let hit = result.results[hand];
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
            if let Some(script) = script {
                self.run_script(&script, id, Some(ev));
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
        let tree = attack_tree(&tree, &kind(main), slots(main), &kind(off));
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
/// measures across the ground. The radii are taken to be `PERSPACE` (provisional).
pub const MELEE_GAP: f32 = 1.2;

/// The slack the same test allows past that reach (metres, 0xade218): an attack may start
/// with the target up to this much farther away.
pub const MELEE_REACH_TOLERANCE: f32 = 1.0;

/// A creature's body radius when its appearance gives none (metres). Provisional.
pub const DEFAULT_RADIUS: f32 = 0.4;

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

    /// How close `id` must be to attack: its main-hand weapon's range (item stat 9 in
    /// `itemstats.gda`) when that is a ranged weapon, otherwise [`MELEE_REACH`].
    pub fn attack_reach(&self, id: ObjectId) -> f32 {
        let slot = constant_of(self, "INVENTORY_SLOT_MAIN");
        let base = self
            .world
            .get(id)
            .and_then(|o| o.equipment.get(&slot))
            .and_then(|w| self.world.get(*w))
            .and_then(|w| w.fields.get("BaseItem").copied());
        let tables = self.world.tables();
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
        int EVENT_TYPE_PERCEPTION_DISAPPEAR = 26;\n\
        int EVENT_TYPE_DEATH = 4;\n\
        int PROPERTY_DEPLETABLE_HEALTH = 7;\n\
        int GM_EXPLORE = 0;\n\
        int GM_COMBAT = 1;\n\
        int EVENT_TYPE_GAMEMODE_CHANGE = 32;\n";

    fn sim() -> Sim {
        let table = Arc::new(EngineTable::parse(LDF.as_bytes()).unwrap());
        Sim::new(Arc::new(Vfs::new()), table, 3)
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
    fn seeing_hostiles_starts_combat_and_losing_them_ends_it() {
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
        assert_eq!(s.world.game_mode, 1);
        let modes: Vec<_> = s
            .world
            .queue
            .values()
            .filter(|q| q.target == module)
            .map(|q| q.event.ints.clone())
            .collect();
        assert_eq!(modes, vec![vec![1, 0]]);
        s.world.get_mut(foe).unwrap().dead = true;
        s.update_perception();
        assert_eq!(s.world.game_mode, 0);
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
