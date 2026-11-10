//! A minimal combat core: perception, hostility, threat, attacks and death.
//!
//! The game's rules live in its scripts (`rules_core` and the AI includes): they decide what
//! to attack, roll hits and damage from the rules 2DAs, and apply damage effects. The engine
//! supplies the plumbing those scripts expect, which is what this module does:
//!
//! - **Perception.** Creatures in the hero's area perceive each other within
//!   [`PERCEPTION_RANGE`] and get `EVENT_TYPE_PERCEPTION_APPEAR` (object 0 is the creature
//!   seen; integers are hostility changed, stealthed, hostile) and `PERCEPTION_DISAPPEAR`
//!   when it leaves the range.
//! - **Game mode.** The party seeing living hostiles switches `GM_EXPLORE` to `GM_COMBAT`
//!   and back, with `GAMEMODE_CHANGE` to the module; the scripts end combat (and get fallen
//!   party members up) on the change back.
//! - **Hostility.** Creatures are hostile when their groups are set hostile with
//!   `SetGroupHostility`.
//! - **Attacks.** When a `CommandAttack` reaches the front of the queue, its owner gets
//!   `EVENT_TYPE_COMMAND_PENDING` (objects: owner, target; integers: command type, id). The
//!   script answers with `SetAttackResult` and `SetAttackDuration`. When the attack's time is
//!   up, the owner gets `EVENT_TYPE_ATTACK_IMPACT` (objects: attacker, target; integer 0: the
//!   hit result), and `GetAttackImpactDamageEffect` returns the damage effect the script set.
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

/// How far creatures see each other, in metres. Provisional.
pub const PERCEPTION_RANGE: f32 = 20.0;

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
    /// Threat by enemy.
    pub threat: Vec<(ObjectId, f32)>,
    /// What the script decided for the attack in progress (`SetAttackResult`).
    pub attack: Option<AttackResult>,
    /// `SetAttackDuration` for the attack in progress.
    pub attack_ms: Option<u64>,
    /// Tactics slots (`SetTacticEntry`, or a preset).
    #[serde(default)]
    pub tactics: Vec<Tactic>,
    /// When each cooldown ends (simulation milliseconds), by `ability:item tag`.
    #[serde(default)]
    pub cooldowns: std::collections::BTreeMap<String, u64>,
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
        for &(id, pos) in &creatures {
            let alive = self.alive(id);
            let now: Vec<ObjectId> = if alive {
                creatures
                    .iter()
                    .filter(|&&(o, p)| o != id && self.alive(o) && dist(pos, p) <= PERCEPTION_RANGE)
                    .map(|&(o, _)| o)
                    .collect()
            } else {
                Vec::new()
            };
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
        // (EVENT_TYPE_DEATH_RES_PARTY to the module), whatever the game mode is.
        let was = std::mem::replace(&mut self.world.party_fighting, fighting);
        if was
            && !fighting
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
        for o in self.world.objects.values_mut() {
            if o.kind != ObjectKind::Creature || o.dead {
                continue;
            }
            let (h, m) = rates[(combat_mode || o.combat.in_combat) as usize];
            for (p, r) in [(health, h), (mana, m)] {
                let rate = o.properties.get(&r).map_or(0.0, |p| p.0.max(p.1));
                if rate == 0.0 {
                    continue;
                }
                if let Some(v) = o.properties.get_mut(&p) {
                    if v.1 < v.0 || rate < 0.0 {
                        v.1 = (v.1 + rate * secs).clamp(0.0, v.0);
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
        let name = if appear {
            "PERCEPTION_APPEAR"
        } else {
            "PERCEPTION_DISAPPEAR"
        };
        let mut ev = self.new_event(name, Some(seen));
        ev.objects = vec![seen];
        ev.ints = vec![0, 0, hostile as i32];
        self.world.queue_event(0, who, ev, None);
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
            }
            return;
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
        if let Some(o) = self.world.get_mut(id) {
            if o.dead {
                return;
            }
            o.dead = true;
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
    /// rows are the slots in order (target type, condition, command, parameter). Provisional.
    pub fn apply_tactic_preset(&mut self, id: ObjectId, preset: i32) -> bool {
        let tables = self.world.tables.clone();
        let Some(presets) = tables.get(0, "tacticsuserpresets") else {
            return false;
        };
        let table_id = match presets.cell("TacticsTable", preset as i64) {
            Some(thedas_formats::gda::Cell::Int(v)) => *v as i32,
            _ => return false,
        };
        let Some(t) = tables.get(table_id, "") else {
            return false;
        };
        let int = |row: i64, col: &str| match t.cell(col, row) {
            Some(thedas_formats::gda::Cell::Int(v)) => *v as i32,
            _ => 0,
        };
        // The parameter column's name is not known; it is the fifth column.
        let param_hash = t.columns.iter().find(|(_, i)| **i == 4).map(|(h, _)| *h);
        let slots: Vec<Tactic> = t
            .order
            .iter()
            .map(|&row| Tactic {
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
            })
            .collect();
        match self.world.get_mut(id) {
            Some(o) => {
                o.combat.tactics = slots;
                o.fields.insert("TacticPreset".into(), preset);
                true
            }
            None => false,
        }
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
        };
        if let Some(o) = self.world.get_mut(attacker) {
            o.commands.clear();
            o.commands.push(cmd);
            o.combat.in_combat = true;
        }
    }

    /// The item a `CommandUseAbility` uses: the owner's inventory item whose tag is the
    /// command's string 0, or `OBJECT_INVALID`.
    fn ability_item(&self, id: ObjectId, cmd: &crate::world::Command) -> ObjectId {
        let Some(tag) = cmd.strings.first().filter(|t| !t.is_empty()) else {
            return OBJECT_INVALID;
        };
        self.world
            .get(id)
            .and_then(|o| {
                o.inventory.iter().copied().find(|i| {
                    self.world
                        .get(*i)
                        .is_some_and(|x| x.tag.eq_ignore_ascii_case(tag))
                })
            })
            .unwrap_or(OBJECT_INVALID)
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

    /// The ability's casting time is up: `ABILITY_CAST_IMPACT` to its owner.
    pub(crate) fn finish_ability(&mut self, id: ObjectId, cmd: &crate::world::Command) {
        if !self.alive(id) {
            return;
        }
        let target = cmd.target.unwrap_or(OBJECT_INVALID);
        let ability = cmd.ints.first().copied().unwrap_or(0);
        self.ability_event("ABILITY_CAST_IMPACT", id, target, ability, cmd);
    }

    /// The front command started: an attack asks its owner's script for the result.
    pub(crate) fn start_attack(&mut self, id: ObjectId, target: ObjectId, ty: i32) {
        let mut ev = self.new_event("COMMAND_PENDING", Some(id));
        ev.objects = vec![id, target];
        ev.ints = vec![ty, 0];
        if let Some(o) = self.world.get_mut(id) {
            o.combat.attack = None;
            o.combat.attack_ms = None;
        }
        let script = self.world.get(id).and_then(|o| o.script.clone());
        if let Some(script) = script {
            self.run_script(&script, id, Some(ev));
        }
    }

    /// The attack's time is up: its owner applies the impact.
    pub(crate) fn finish_attack(&mut self, id: ObjectId, target: ObjectId) {
        let Some(result) = self.world.get(id).and_then(|o| o.combat.attack.clone()) else {
            return;
        };
        if !self.alive(id) {
            return;
        }
        let t = self.world.time_ms;
        self.world.trace.push(crate::trace::TraceEntry::Log {
            t,
            channel: -4,
            text: format!(
                "attack {id} -> {target}: results {:?}, effects {:?}",
                result.results,
                result
                    .effects
                    .iter()
                    .map(|e| e.as_ref().map(|e| (e.ty, e.floats.clone())))
                    .collect::<Vec<_>>()
            ),
        });
        let mut ev = self.new_event("ATTACK_IMPACT", Some(id));
        ev.objects = vec![id, target];
        ev.ints = vec![result.results[0], result.results[1]];
        let script = self.world.get(id).and_then(|o| o.script.clone());
        if let Some(script) = script {
            self.run_script(&script, id, Some(ev));
        }
        if let Some(o) = self.world.get_mut(id) {
            o.combat.attack = None;
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
        s.check_death(foe, hero);
        assert!(s.world.get(foe).unwrap().dead);
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
