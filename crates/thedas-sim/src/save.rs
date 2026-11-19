//! Saving and loading the simulation as JSON.
//!
//! This is OpenThedas's own save format, not the game's `.das` saves: the world (objects,
//! locals, engine values, plots, the event queue, time and the random generator) and the
//! conversation in progress. Loading needs a simulation of the same module, which supplies
//! the resources, scripts and tables; the trace starts afresh.

use serde::{Deserialize, Serialize};

use thedas_script::vm::ObjectId;

use crate::dialog::Conversation;
use crate::sim::{Sim, SimError};
use crate::world::World;

/// Bumped when the layout of a save changes incompatibly.
pub const SAVE_FORMAT: u32 = 1;

/// Bumped when what a save means changes compatibly, so that loading can bring older saves
/// up to date:
/// - 1: worn items' properties are effects on their wearers (before, gear's bonuses were
///   not applied; loading a revision 0 save applies them).
/// - 2: creatures keep an appearance: saves from earlier runs have party members whose
///   appearance was lost (0, so they drew nothing); loading gives them their template's.
/// - 3: creature properties keep their values by kind (a script writing a property's
///   modifier no longer overwrites its base). Earlier saves may carry bases that such
///   writes destroyed; loading one checks for what can be seen of that (see
///   [`SaveIssue`]).
/// - 4: the module hears followers join and leave the party (`EVENT_TYPE_PARTYMEMBER_ADDED`
///   / `_DROPPED`), on which module_core keeps the party plot's `GEN_<name>_IN_PARTY` flags.
///   Loading an earlier save tells it of the party as it stands.
/// - 5: creatures placed dead (`CREATURE_SPAWN_DEAD`, see [`Sim::spawns_dead`]) are dead from
///   their creation; earlier saves hold them alive, and loading one lays them out.
/// - 6: stackable items merge into the stacks already held, and creatures hear
///   `EVENT_TYPE_EQUIP` for their starting gear (rules_core works out the main-hand damage,
///   property 50, from it). Loading an earlier save merges each creature's carried stacks
///   and tells the party's members of what they wear (no effects are applied again).
/// - 7: placeables hold their template's items (`ItemList`) from the area's loading, as
///   containers do in the original. Loading an earlier save fills each empty container with
///   its template's items (one that was emptied by looting is filled again: earlier saves
///   cannot tell the two apart).
/// - 8: creatures have the abilities their templates list (`TalentList`, `SpellList`,
///   `SkillList`: a monster's own abilities). Loading an earlier save adds each creature's
///   template abilities to what it has (one a script took away since is given back: earlier
///   saves cannot tell).
/// - 9: Origins placeables load their `Useable` interaction flag. Earlier saves left
///   non-useable scenery interactive; loading disables those objects. An earlier script
///   explicitly enabling such scenery cannot be distinguished from the old default.
/// - 10: Origins placed objects initialize their current State from InitialState.
///   Earlier saves without State receive that initial value and its PWK footprint;
///   explicit saved states and their existing pathing writes are preserved.
///   An older no-op setter that left no State field cannot be distinguished.
/// - 11: Origins death blows retain paired animation deadlines. Earlier opaque active
///   death-blow commands restart preparation when loaded.
/// - 12: Origins visual effects bypass script validation. Earlier saves can have
///   already-spawned creatures without their appearance's crust; restore missing crusts.
/// - 13: Origins retains its prepared combat target between commands. Earlier active
///   attacks in combat recover their explicit target; idle queues have no history to recover.
pub const SAVE_REVISION: u32 = 13;

/// The trace channel loading reports save damage on.
pub const SAVE_CHANNEL: i32 = -7;

/// Properties only ever changed through their modifier (every script write to them is a
/// MODIFIER write), so their base is 0: Fatigue and Damage_Shield_points.
pub const MODIFIER_ONLY_PROPERTIES: [i32; 2] = [41, 47];

/// Damage an older save shows from before revision 3, when a script adding to a property's
/// modifier set its base (and current value) to the current value plus what it added
/// instead: an attribute (1-6) left at 0, which none can be; or a non-depletable property
/// whose current value equals its base and is not 0, which only that write left (their
/// current value is otherwise never set). The latter is repaired for
/// [`MODIFIER_ONLY_PROPERTIES`] (their base and current value back to 0); otherwise the true
/// base is lost and nothing records it, so the save is only reported.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SaveIssue {
    pub creature: ObjectId,
    pub tag: String,
    pub property: i32,
    pub base: f32,
    pub current: f32,
    pub repaired: bool,
}

impl std::fmt::Display for SaveIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} ({}): property {} has base {} and current {}, damaged by an older build ({})",
            self.tag,
            self.creature,
            self.property,
            self.base,
            self.current,
            if self.repaired {
                "reset to 0"
            } else {
                "its true value is lost; starting a new game is advised"
            }
        )
    }
}

#[derive(Serialize, Deserialize)]
pub struct SaveGame {
    pub format: u32,
    #[serde(default)]
    pub revision: u32,
    /// The module the world belongs to (`ModuleInfo::name`).
    pub module: String,
    pub world: World,
    pub talk: Option<Conversation>,
}

impl Sim {
    pub fn save_game(&self) -> SaveGame {
        SaveGame {
            format: SAVE_FORMAT,
            revision: SAVE_REVISION,
            module: self.module_info.name.clone(),
            world: self.world.clone(),
            talk: self.talk.clone(),
        }
    }

    /// Puts the properties of what each creature wears on it, for items whose effects it
    /// does not carry (saves from before worn items' properties were applied).
    fn apply_worn_properties(&mut self) {
        let worn: Vec<(ObjectId, ObjectId)> = self
            .world
            .objects
            .values()
            .flat_map(|o| o.equipment.values().map(move |&i| (o.id, i)))
            .collect();
        for (who, item) in worn {
            let carried = self
                .world
                .get(who)
                .is_some_and(|o| o.effects.iter().any(|e| e.creator == Some(item)));
            if carried {
                continue;
            }
            for e in self.property_effects(item) {
                self.apply_effect(who, e);
            }
        }
    }

    /// Looks for what a pre-revision-3 save shows of property writes that destroyed bases
    /// (see [`SaveIssue`]), repairing what can be, and reports each on the trace.
    fn check_property_damage(&mut self) -> Vec<SaveIssue> {
        use crate::engine::{PropertyKind, property_kind};
        let mut found = Vec::new();
        let creatures: Vec<ObjectId> = self
            .world
            .objects
            .values()
            .filter(|o| o.kind == crate::world::ObjectKind::Creature)
            .map(|o| o.id)
            .collect();
        for id in creatures {
            let Some(o) = self.world.get(id) else {
                continue;
            };
            let tag = o.tag.clone();
            let props: Vec<(i32, f32, f32)> =
                o.properties.iter().map(|(&p, &(b, c))| (p, b, c)).collect();
            for (p, base, current) in props {
                let zero_attribute = (1..=6).contains(&p) && base <= 0.0;
                let written_both = property_kind(self, p) != PropertyKind::Depletable
                    && current != 0.0
                    && base == current;
                if !zero_attribute && !written_both {
                    continue;
                }
                let repaired = written_both && MODIFIER_ONLY_PROPERTIES.contains(&p);
                if repaired {
                    if let Some(v) = self
                        .world
                        .get_mut(id)
                        .and_then(|o| o.properties.get_mut(&p))
                    {
                        *v = (0.0, 0.0);
                    }
                }
                found.push(SaveIssue {
                    creature: id,
                    tag: tag.clone(),
                    property: p,
                    base,
                    current,
                    repaired,
                });
            }
        }
        let t = self.world.time_ms;
        for issue in &found {
            self.world.trace.push(crate::trace::TraceEntry::Log {
                t,
                channel: SAVE_CHANNEL,
                text: issue.to_string(),
            });
        }
        found
    }

    /// Gives creatures with no appearance their template's (saves from before revision 2).
    fn restore_lost_appearances(&mut self) {
        let lost: Vec<ObjectId> = self
            .world
            .objects
            .values()
            .filter(|o| o.kind == crate::world::ObjectKind::Creature && o.appearance <= 0)
            .map(|o| o.id)
            .collect();
        for id in lost {
            if let Some(app) = self.original_appearance(id) {
                if let Some(o) = self.world.get_mut(id) {
                    o.appearance = app;
                }
            }
        }
    }

    /// Repairs our pre-revision-12 rejection of creature_core's spawn crust.
    /// This is snapshot migration, not a native load event or a SPAWN replay.
    fn restore_missing_spawn_crusts(&mut self) {
        let Some(appearances) = self.world.tables.get(2, "") else {
            return;
        };
        // World objects are ordered by ID, keeping new effect identities stable.
        let missing: Vec<_> = self
            .world
            .objects
            .values()
            .filter(|o| {
                o.kind == crate::world::ObjectKind::Creature
                    && !o.dead
                    && o.locals.ints.get("CREATURE_SPAWNED") == Some(&1)
                    && o.locals.ints.get("CREATURE_SPAWN_DEAD") != Some(&1)
            })
            .filter_map(|o| {
                let vfx = crate::engine::cell_int(
                    appearances.cell("CRUST_EFFECT", i64::from(o.appearance)),
                );
                (vfx > 0
                    && !o
                        .effects
                        .iter()
                        .any(|e| e.ty == 4 && e.ints.first() == Some(&vfx)))
                .then_some((o.id, vfx))
            })
            .collect();
        for (id, vfx) in missing {
            self.apply_effect(
                id,
                crate::world::Effect {
                    ty: 4,
                    creator: Some(id),
                    ints: vec![vfx],
                    duration_type: 5,
                    ..Default::default()
                },
            );
            self.world.trace.push(crate::trace::TraceEntry::Log {
                t: self.world.time_ms,
                channel: SAVE_CHANNEL,
                text: format!("restored missing spawn crust {vfx} on {id} (save revision < 12)"),
            });
        }
    }

    pub fn save_json(&self) -> Result<Vec<u8>, SimError> {
        serde_json::to_vec(&self.save_game())
            .map_err(|e| SimError::Parse("save".into(), e.to_string()))
    }

    /// Replaces the world with a saved one. The save must be for this simulation's module.
    /// Takes a script's request for an autosave (`DoAutoSave`), for the presentation to
    /// make it: true once per request.
    pub fn take_autosave_request(&mut self) -> bool {
        std::mem::take(&mut self.world.autosave_requested)
    }

    pub fn load_game(&mut self, save: SaveGame) -> Result<(), SimError> {
        self.loads += 1;
        if save.format != SAVE_FORMAT {
            return Err(SimError::Parse(
                "save".into(),
                format!("format {} (expected {SAVE_FORMAT})", save.format),
            ));
        }
        if save.module != self.module_info.name {
            return Err(SimError::Parse(
                "save".into(),
                format!(
                    "save is for module {:?}, this simulation runs {:?}",
                    save.module, self.module_info.name
                ),
            ));
        }
        let save_revision = save.revision;
        let mut world = save.world;
        world.tables = self.world.tables.clone();
        world.trace = std::mem::take(&mut self.world.trace);
        world.trace.entries.clear();
        // Older snapshots timed a death blow as an opaque 500 ms command. Its
        // animation start was not saved, so restart preparation at the load time.
        // A saved paired runtime already has exact deadlines and resumes intact.
        if self.game == thedas_vfs::Game::Origins && save_revision < 11 {
            for object in world.objects.values_mut() {
                if object.combat.death_blow.is_none()
                    && let Some(command) = object.commands.first_mut()
                    && command.name == "CommandDeathBlow"
                {
                    command.due = None;
                }
            }
        }
        // No training step survives a load: the exe disables saving while training mode is on
        // (script.ldf, SetTrainingMode: "Unpausing, Saving, Item Destruction" are disabled),
        // so no save of the original holds one, and a step waiting for a party member who is
        // not there could never end. Saves written mid-training by tools would otherwise load
        // paused under it.
        world.training = Default::default();
        // Nor does a HUD notification: they are the HUD's, raised as journal and codex entries
        // arrive, and the notifications movie shows every one it is handed, so carried in a
        // save they came back on every load. Provisional: the exe's notification queue is
        // not traced.
        world.alerts.clear();
        world.uber_alerts.clear();
        if self.game == thedas_vfs::Game::Origins
            && let Some(cut) = world.cutscene.as_mut()
            && cut.from_dialog
            && !cut.lines.is_empty()
            && cut.lines.iter().all(|line| !line.explicit_text)
            && let Ok((data, _)) = self.vfs.read(&cut.name)
            && let Some(authored) = crate::cutscene::Cutscene::parse(&data, &cut.name)
        {
            cut.restore_explicit_lines(&authored);
        }
        if self.game == thedas_vfs::Game::Origins {
            // Older saves have the parked flag but no pause timestamp. The save's
            // clock is the earliest known point from which we can suspend the front.
            for o in world.objects.values_mut() {
                if o.parked.is_some() && o.commands.first().is_some_and(|c| c.due.is_some()) {
                    o.command_parked_at.get_or_insert(world.time_ms);
                }
            }
        }
        self.world = world;
        if self.game == thedas_vfs::Game::Origins && save_revision < 13 {
            for object in self.world.objects.values_mut() {
                if object.kind == crate::world::ObjectKind::Creature
                    && let Some(command) = object.commands.first_mut()
                    && command.name == "CommandAttack"
                    && command.due.is_some()
                    && !command.da2_target_prepared
                {
                    // An active attack has already prepared its explicit target.
                    // Preserve an existing register and combat-exit clears. Older
                    // idle/wait queues cannot tell us what was attacked before them.
                    if object.combat.in_combat && object.combat.target.is_none() {
                        object.combat.target = command
                            .target
                            .filter(|&target| target != thedas_script::vm::OBJECT_INVALID);
                    }
                    command.da2_target_prepared = true;
                }
            }
        }
        self.restore_da2_encounters();
        self.restore_script_heartbeats();
        self.repair_body_bags();
        // A conversation whose last line was already shown does not come back: the game
        // cannot be saved during a conversation, so a loaded game never resumes one at its
        // end (saves made by test drivers can hold one; reloading it would put the last
        // line on screen again). One still going is kept.
        self.talk = save.talk.filter(|c| !c.ended);
        if self.game == thedas_vfs::Game::Origins
            && let Some(dialog) = self.talk.as_ref().map(|c| c.dlg.name.clone())
        {
            self.conversation_line_cuts(&dialog);
        }
        // What was being said in passing belonged to the game left behind.
        self.ambient.clear();
        self.ambient_speaking.clear();
        self.world.ambient_lines.clear();
        self.forget_world_caches();
        if self.game == thedas_vfs::Game::DragonAge2 {
            self.restore_da2_aoe_spheres();
            // Native DA2 template loading initializes race from appearance. Earlier saves
            // omitted it; filling only absent fields preserves script-assigned races.
            let ids: Vec<ObjectId> = self.world.objects.keys().copied().collect();
            for id in ids {
                self.init_da2_race(id);
            }
        }
        // The area list is loaded again, so the add-ins' scripts for it run again.
        self.prcscr_ran = None;
        if save_revision < 1 {
            self.apply_worn_properties();
        }
        if save_revision < 2 {
            self.restore_lost_appearances();
        }
        if save_revision < 4 {
            let members = self
                .world
                .hero
                .and_then(|h| self.world.get(h))
                .map(|h| h.followers.clone())
                .unwrap_or_default();
            for m in members {
                self.party_membership_changed(m, true);
            }
        }
        if save_revision < 5 {
            let ids: Vec<ObjectId> = self.world.objects.values().map(|o| o.id).collect();
            for id in ids {
                self.lay_out_if_spawned_dead(id);
            }
        }
        if save_revision < 6 {
            let owners: Vec<ObjectId> = self
                .world
                .objects
                .values()
                .filter(|o| !o.inventory.is_empty())
                .map(|o| o.id)
                .collect();
            for owner in owners {
                self.restack(owner);
            }
            let party = crate::engine::party_of(self);
            for m in party {
                let worn: Vec<(i32, ObjectId)> = self
                    .world
                    .get(m)
                    .map(|o| o.equipment.iter().map(|(s, i)| (*s, *i)).collect())
                    .unwrap_or_default();
                for (slot, item) in worn {
                    self.equip_event("EQUIP", m, item, slot, false);
                }
            }
        }
        if save_revision < 7 {
            let empty: Vec<(ObjectId, String)> = self
                .world
                .objects
                .values()
                .filter(|o| {
                    o.kind == crate::world::ObjectKind::Placeable
                        && o.inventory.is_empty()
                        && !o.template.is_empty()
                })
                .map(|o| (o.id, o.template.clone()))
                .collect();
            for (id, template) in empty {
                if let Ok(t) = self.read_gff3(&format!("{}.utp", template.to_ascii_lowercase())) {
                    self.load_template_items(id, &t);
                }
            }
        }
        self.learn_held_recipes();
        // A loot panel does not outlive a load: the game is saved from its menu, with the
        // panel closed, and an open one from a scripted save would point at a container that
        // may be in another area. Provisional: the exe's save holds no open GUI.
        self.world.container = None;
        // So do a store and a Chanter's Board (see the container above).
        self.world.store = None;
        self.world.chanter_board = None;
        // Saves made before the ability functions checked their ids may hold ones the exe
        // never would (0 from player_core's hire handler; see `Sim::ability_known`).
        let bad: Vec<(ObjectId, Vec<i32>)> = self
            .world
            .objects
            .values()
            .filter_map(|o| {
                let b: Vec<i32> = o
                    .abilities
                    .iter()
                    .copied()
                    .filter(|&a| !self.ability_known(a))
                    .collect();
                (!b.is_empty()).then_some((o.id, b))
            })
            .collect();
        for (id, ids) in bad {
            if let Some(o) = self.world.get_mut(id) {
                for a in ids {
                    o.abilities.remove(&a);
                }
            }
        }
        // A tutorial pause holds until the queue is empty (0x693630); a save that holds it
        // with no tutorial left to dismiss would never resume. Provisional: whether the
        // original saves the tutorial queue at all is not traced.
        if self.world.tutorials.is_empty() {
            self.world.tutorial_paused = false;
        }
        if self.world.tutorial_paused {
            self.prepare_pausing_tutorials();
        }
        if save_revision < 8 {
            self.restore_template_abilities();
        }
        if save_revision < 9 && self.game == thedas_vfs::Game::Origins {
            for object in self.world.objects.values_mut().filter(|o| {
                o.kind == crate::world::ObjectKind::Placeable && o.fields.get("Useable") == Some(&0)
            }) {
                object.interactive = false;
            }
        }
        if save_revision < 10 && self.game == thedas_vfs::Game::Origins {
            // Stable order matters where placed footprints overlap.
            let mut ids: Vec<_> = self.world.objects.keys().copied().collect();
            ids.sort_unstable();
            for id in ids {
                self.init_placeable_state(id);
            }
        }
        if save_revision < 12 && self.game == thedas_vfs::Game::Origins {
            self.restore_missing_spawn_crusts();
        }
        // DA2 snapshots can contain PC followers initialized through the monster path
        // and already-perceived NPCs with no level. Rebuild only absent stats; the
        // predicates make this idempotent without changing Origins' save revision.
        self.repair_da2_stats();
        self.save_issues = if save_revision < 3 {
            self.check_property_damage()
        } else {
            Vec::new()
        };
        // Saves from before world maps were simulated have none.
        if !self
            .world
            .objects
            .values()
            .any(|o| o.kind == crate::world::ObjectKind::Map)
        {
            self.load_world_maps();
        }
        // The area hears it was loaded from a save: DAOrigins.exe runs its area-load
        // sequence with the save flag set, which turns SPECIAL, PRELOADEXIT and POSTLOADEXIT
        // into their AREALOADSAVE_ forms (41, 42, 43: 0x67f1c0, 0x6c90ab, 0x6c92b9), from the
        // controlled creature, with no payload. Provisional: what sets the flag is inferred
        // from the names.
        if let Some(area) = self.world.area {
            let creator = self.world.controlled().or(self.world.hero);
            for name in [
                "AREALOADSAVE_SPECIAL",
                "AREALOADSAVE_PRELOADEXIT",
                "AREALOADSAVE_POSTLOADEXIT",
            ] {
                let ev = self.new_event(name, creator);
                self.world.queue_event(0, area, ev, None);
            }
        }
        Ok(())
    }

    /// Before the game is saved: the module hears `EVENT_TYPE_MODULE_PRESAVE` (62) from the
    /// player's creature, with no payload, and handles it before the save is taken
    /// (DAOrigins.exe 0x5d6c40, called by the save routine 0x5d7fc0).
    pub fn presave(&mut self) {
        let Some(module) = self.world.module else {
            return;
        };
        let creator = self.world.controlled().or(self.world.hero);
        let ev = self.new_event("MODULE_PRESAVE", creator);
        self.deliver(module, ev, None);
    }

    pub fn load_json(&mut self, data: &[u8]) -> Result<(), SimError> {
        let save: SaveGame = serde_json::from_slice(data)
            .map_err(|e| SimError::Parse("save".into(), e.to_string()))?;
        self.load_game(save)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use thedas_script::EngineTable;
    use thedas_script::vm::OBJECT_INVALID;
    use thedas_vfs::Vfs;

    use crate::world::{Event, ObjectKind, QueuedEvent};

    use super::*;

    fn sim() -> Sim {
        let mut s = Sim::new(Arc::new(Vfs::new()), Arc::new(EngineTable::default()), 7);
        s.module_info.name = "test".into();
        s
    }

    #[test]
    fn older_origins_saves_restore_only_missing_spawn_crusts_in_id_order() {
        use crate::world::Effect;
        use thedas_formats::gda::Cell;

        for game in [thedas_vfs::Game::Origins, thedas_vfs::Game::DragonAge2] {
            let mut s = sim();
            s.game = game;
            let tables = crate::m2da::Tables::empty();
            // Synthetic appearance: the repair reads data rather than a wisp ID.
            tables.insert("#2", &["CRUST_EFFECT"], vec![(77, vec![Cell::Int(4321)])]);
            s.world.tables = Arc::new(tables);
            let ids: Vec<_> = (0..9)
                .map(|_| s.world.spawn(ObjectKind::Creature, "crust"))
                .collect();
            for &id in &ids {
                let o = s.world.get_mut(id).unwrap();
                o.appearance = 77;
                o.locals.ints.insert("CREATURE_SPAWNED".into(), 1);
            }
            s.world.get_mut(ids[1]).unwrap().active = false; // Still needs its saved body.
            s.world.get_mut(ids[2]).unwrap().dead = true;
            s.world
                .get_mut(ids[3])
                .unwrap()
                .locals
                .ints
                .insert("CREATURE_SPAWN_DEAD".into(), 1);
            s.world
                .get_mut(ids[4])
                .unwrap()
                .locals
                .ints
                .remove("CREATURE_SPAWNED");
            s.world.get_mut(ids[5]).unwrap().appearance = 78; // No configured crust.
            s.world.get_mut(ids[6]).unwrap().kind = ObjectKind::Placeable;
            let existing = Effect {
                id: 20,
                ty: 4,
                ints: vec![4321],
                duration_type: 5,
                ..Default::default()
            };
            s.world
                .get_mut(ids[7])
                .unwrap()
                .effects
                .push(existing.clone());
            let unrelated = Effect {
                id: 21,
                ty: 4,
                ints: vec![999],
                duration_type: 3,
                ..Default::default()
            };
            s.world
                .get_mut(ids[8])
                .unwrap()
                .effects
                .push(unrelated.clone());
            let mut old = s.save_game();
            old.revision = 11;
            let input = serde_json::to_vec(&old).unwrap();
            s.load_json(&input).unwrap();
            let first = s.save_json().unwrap();
            for &id in &[ids[0], ids[1], ids[8]] {
                let effects = &s.world.get(id).unwrap().effects;
                assert_eq!(
                    effects.len(),
                    usize::from(id == ids[8]) + usize::from(game == thedas_vfs::Game::Origins)
                );
                if game == thedas_vfs::Game::Origins {
                    let effect = effects.last().unwrap();
                    assert_eq!(
                        (effect.ty, effect.ints.as_slice(), effect.duration_type),
                        (4, &[4321][..], 5)
                    );
                    assert_eq!(
                        (effect.creator, effect.visual_target, effect.expires),
                        (Some(id), Some(id), None)
                    );
                    assert_eq!(
                        effect.id,
                        22 + if id == ids[0] {
                            0
                        } else if id == ids[1] {
                            1
                        } else {
                            2
                        }
                    );
                }
            }
            for &id in &ids[2..7] {
                assert!(s.world.get(id).unwrap().effects.is_empty());
            }
            assert_eq!(s.world.get(ids[7]).unwrap().effects, [existing]);
            assert_eq!(s.world.get(ids[8]).unwrap().effects[0], unrelated);
            // The same damaged input repairs deterministically, and a migrated save
            // is idempotent. Its explicit removal then stays removed on later loads.
            s.load_json(&input).unwrap();
            assert_eq!(s.save_json().unwrap(), first);
            s.load_json(&first).unwrap();
            assert_eq!(s.save_json().unwrap(), first);
            s.world.get_mut(ids[0]).unwrap().effects.clear();
            s.load_game(s.save_game()).unwrap();
            assert!(s.world.get(ids[0]).unwrap().effects.is_empty());
        }
    }

    #[test]
    fn old_origins_scenery_loses_the_incorrect_interaction_default() {
        for game in [thedas_vfs::Game::Origins, thedas_vfs::Game::DragonAge2] {
            let mut s = sim();
            s.game = game;
            let door = s.world.spawn(ObjectKind::Placeable, "scenery_door");
            s.world
                .get_mut(door)
                .unwrap()
                .fields
                .insert("Useable".into(), 0);
            let usable = s.world.spawn(ObjectKind::Placeable, "usable_door");
            s.world
                .get_mut(usable)
                .unwrap()
                .fields
                .insert("Useable".into(), 1);
            let creature = s.world.spawn(ObjectKind::Creature, "creature");
            s.world
                .get_mut(creature)
                .unwrap()
                .fields
                .insert("Useable".into(), 0);
            let mut old = s.save_game();
            old.revision = 8;
            s.load_game(old).unwrap();
            assert_eq!(
                s.world.get(door).unwrap().interactive,
                game != thedas_vfs::Game::Origins
            );
            assert!(s.world.get(usable).unwrap().interactive);
            assert!(s.world.get(creature).unwrap().interactive);
            // A script's explicit choice in a new save remains authoritative.
            s.world.get_mut(door).unwrap().interactive = true;
            let bytes = s.save_json().unwrap();
            s.load_json(&bytes).unwrap();
            assert!(s.world.get(door).unwrap().interactive);
        }
    }

    #[test]
    fn notifications_do_not_come_back_on_a_load() {
        let mut s = sim();
        let alert = crate::journal::Alert {
            kind: 1,
            resref: "bec100pt_main".into(),
            name: None,
            completed: false,
            ..Default::default()
        };
        s.world.alerts.push(alert.clone());
        s.world.uber_alerts.push(alert);
        let bytes = s.save_json().unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("bec100pt_main"));
        for _ in 0..2 {
            let mut t = sim();
            t.load_json(&bytes).unwrap();
            assert!(t.world.alerts.is_empty() && t.world.uber_alerts.is_empty());
        }
        // An in-memory save (the MCP's) does not carry them either.
        let save = s.save_game();
        let mut t = sim();
        t.load_game(save).unwrap();
        assert!(t.world.alerts.is_empty() && t.world.uber_alerts.is_empty());
    }

    #[test]
    fn a_load_clears_training() {
        let mut s = sim();
        s.world.training.mode = 3;
        s.world.training.step = 3;
        s.world.training.expected = 1060;
        let bytes = s.save_json().unwrap();
        let mut t = sim();
        t.load_json(&bytes).unwrap();
        assert_eq!(t.world.training.mode, 0);
        assert!(!t.training_paused());
    }

    #[test]
    fn training_blocks_saving() {
        let mut s = sim();
        assert!(s.saving_blocked().is_none());
        s.world.training.mode = 5;
        assert!(s.saving_blocked().is_some());
    }

    #[test]
    fn a_finished_conversation_does_not_come_back_with_a_load() {
        use crate::dialog::{Conversation, Dlg};
        let mut s = sim();
        let hero = s.world.spawn(ObjectKind::Creature, "player");
        s.world.hero = Some(hero);
        let talk = |ended| Conversation {
            dlg: Dlg {
                name: "test".into(),
                starts: Vec::new(),
                nodes: Vec::new(),
            },
            owner: hero,
            player: hero,
            line: None,
            replies: Vec::new(),
            ended,
            shown: Vec::new(),
            chain: Vec::new(),
            cut_from: None,
            cut_lines_left: 0,
            cut_line_speech: crate::dialog::CutLineSpeech::Dialog,
            staged: Vec::new(),
            staged_at: None,
            ambient_run: false,
        };
        s.talk = Some(talk(true));
        let save = s.save_game();
        assert!(save.talk.is_some());
        s.load_game(save).unwrap();
        assert!(s.talk.is_none());
        // One still going is resumed.
        s.talk = Some(talk(false));
        s.load_game(s.save_game()).unwrap();
        assert!(s.talk.as_ref().is_some_and(|c| !c.ended));
    }

    #[test]
    fn an_open_loot_panel_does_not_survive_a_load() {
        let mut s = sim();
        let chest = s.world.spawn(crate::world::ObjectKind::Placeable, "chest");
        s.open_container(chest);
        s.load_game(s.save_game()).unwrap();
        assert_eq!(s.world.container, None);
    }

    #[test]
    fn invalid_ability_ids_are_dropped_on_load() {
        let mut s = sim();
        let c = s.world.spawn(crate::world::ObjectKind::Creature, "c");
        let o = s.world.get_mut(c).unwrap();
        o.abilities.insert(0);
        o.abilities.insert(-3);
        o.abilities.insert(617);
        s.load_game(s.save_game()).unwrap();
        let a: Vec<i32> = s.world.get(c).unwrap().abilities.iter().copied().collect();
        assert_eq!(a, vec![617]);
    }

    #[test]
    fn a_tutorial_pause_with_nothing_left_to_dismiss_does_not_survive_a_load() {
        let mut s = sim();
        s.world.tutorial_paused = true;
        s.load_game(s.save_game()).unwrap();
        assert!(!s.world.tutorial_paused);
        // With tutorials still queued it holds, as the original's does until they are all
        // dismissed.
        s.world.tutorials.push(crate::journal::Tutorial {
            resref: "tut_explore".into(),
            title: None,
            text: Some(1),
            pauses: false,
        });
        s.world.tutorial_paused = true;
        s.load_game(s.save_game()).unwrap();
        assert!(s.world.tutorial_paused);
    }

    #[test]
    fn lost_appearances_come_back_from_templates() {
        use thedas_formats::gff3::{Field, Gff3, Struct, Value as G3};
        let dir = std::env::temp_dir().join(format!("thedas-app-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let utc = dir.join("hm_follower.utc");
        let gff = Gff3 {
            file_type: *b"UTC ",
            file_version: *b"V3.2",
            structs: vec![Struct {
                id: u32::MAX,
                fields: vec![Field {
                    label: "Appearance".into(),
                    value: G3::Short(15),
                }],
            }],
        };
        std::fs::write(&utc, gff.write().unwrap()).unwrap();
        let mut vfs = Vfs::new();
        vfs.push_files(&[utc], "test");
        let table = EngineTable::parse(
            b"void SetAppearanceType(object oCreature, int nType, int bUpdate) = 1;\n",
        )
        .unwrap();
        let mut s = Sim::new(Arc::new(vfs), Arc::new(table), 7);
        s.module_info.name = "test".into();
        s.world.trace.keep = true;
        let hero = s.world.spawn(ObjectKind::Creature, "player");
        let follower = s.world.spawn(ObjectKind::Creature, "follower");
        s.world.hero = Some(hero);
        s.world.get_mut(hero).unwrap().followers.push(follower);
        s.world.get_mut(follower).unwrap().template = "hm_follower".into();
        // An older save with the follower's appearance lost: loading gives it back.
        let mut old = s.save_game();
        old.revision = 1;
        s.load_game(old).unwrap();
        assert_eq!(s.world.get(follower).unwrap().appearance, 15);
        // A current save keeps what it says.
        s.world.get_mut(follower).unwrap().appearance = 0;
        s.load_game(s.save_game()).unwrap();
        assert_eq!(s.world.get(follower).unwrap().appearance, 0);
        // Older worlds without an assigned base still restore the template appearance.
        let set = |s: &mut Sim, n: i32| {
            let f = s
                .table
                .function_by_name("SetAppearanceType")
                .cloned()
                .unwrap();
            let mut host = crate::sim::ScriptHost {
                sim: s,
                event: None,
                script: "test".into(),
            };
            let args = [
                thedas_script::vm::Value::Object(follower),
                thedas_script::vm::Value::Int(n),
                thedas_script::vm::Value::Int(0),
            ];
            crate::engine::call(&mut host, OBJECT_INVALID, &f, &args);
        };
        set(&mut s, -1);
        assert_eq!(s.world.get(follower).unwrap().appearance, 15);
        set(&mut s, 0);
        assert_eq!(s.world.get(follower).unwrap().appearance, 15);
        assert!(!s.world.trace.entries.iter().any(|e| matches!(
            e,
            crate::trace::TraceEntry::Log { text, .. } if text.contains("leaves it nothing to draw")
        )));
    }

    #[test]
    fn older_saves_tell_the_module_who_is_in_the_party() {
        let table = EngineTable::parse(b"int EVENT_TYPE_PARTYMEMBER_ADDED = 66;\n").unwrap();
        let mk = || {
            let mut s = Sim::new(Arc::new(Vfs::new()), Arc::new(table.clone()), 7);
            s.module_info.name = "test".into();
            s
        };
        let mut s = mk();
        let hero = s.world.spawn(ObjectKind::Creature, "hero");
        let ally = s.world.spawn(ObjectKind::Creature, "ally");
        let module = s.world.spawn(ObjectKind::Module, "module");
        s.world.hero = Some(hero);
        s.world.module = Some(module);
        s.world.get_mut(hero).unwrap().followers = vec![ally];
        let mut old = s.save_game();
        old.revision = 3;
        let mut t = mk();
        t.load_game(old).unwrap();
        let heard: Vec<_> = t
            .world
            .queue
            .values()
            .filter(|q| q.target == module)
            .map(|q| (q.event.ty, q.event.objects.clone()))
            .collect();
        assert_eq!(heard, vec![(66, vec![ally])]);
        let mut u = mk();
        u.load_game(s.save_game()).unwrap();
        assert!(u.world.queue.values().all(|q| q.target != module));
    }

    #[test]
    fn older_saves_lay_out_creatures_placed_dead() {
        let mut s = sim();
        let corpse = s.world.spawn(ObjectKind::Creature, "genlock_mace");
        let alive = s.world.spawn(ObjectKind::Creature, "genlock_mace");
        for (id, v) in [(corpse, 1), (alive, 0)] {
            s.world
                .get_mut(id)
                .unwrap()
                .locals
                .ints
                .insert("CREATURE_SPAWN_DEAD".into(), v);
        }
        let mut save = s.save_game();
        save.revision = 4;
        let data = serde_json::to_vec(&save).unwrap();
        let mut t = sim();
        t.load_json(&data).unwrap();
        assert!(t.world.get(corpse).unwrap().dead);
        assert!(!t.world.get(alive).unwrap().dead);
        // A save from this build is taken as it is.
        let mut u = sim();
        u.load_json(&s.save_json().unwrap()).unwrap();
        assert!(!u.world.get(corpse).unwrap().dead);
    }

    #[test]
    fn older_saves_report_bases_lost_to_modifier_writes() {
        let mut s = sim();
        let c = s.world.spawn(ObjectKind::Creature, "alistair");
        let o = s.world.get_mut(c).unwrap();
        o.properties.insert(1, (18.0, 0.0));
        o.properties.insert(2, (0.0, 0.0)); // dexterity lost
        o.properties.insert(7, (100.0, 100.0)); // a depletable: current is its own
        o.properties.insert(41, (5.0, 5.0)); // fatigue written into its base
        o.properties.insert(9, (60.0, 60.0)); // attack: damaged, but its base is unknown
        let mut save = s.save_game();
        save.revision = 2;
        let data = serde_json::to_vec(&save).unwrap();
        let mut t = sim();
        t.world.trace.keep = true;
        t.load_json(&data).unwrap();
        let found: Vec<(i32, bool)> = t
            .save_issues
            .iter()
            .map(|i| (i.property, i.repaired))
            .collect();
        assert_eq!(found, vec![(2, false), (9, false), (41, true)]);
        let p = &t.world.get(c).unwrap().properties;
        assert_eq!(p[&41], (0.0, 0.0), "fatigue is only ever a modifier");
        assert_eq!(p[&9], (60.0, 60.0), "reported, not guessed at");
        assert!(
            t.world.trace.entries.iter().any(|e| matches!(
                e,
                crate::trace::TraceEntry::Log { channel: SAVE_CHANNEL, text, .. }
                    if text.contains("property 2")
            )),
            "loading reports on the trace"
        );
        assert!(t.save_issues[0].to_string().contains("starting a new game"));
        // The same world saved by this build is not suspect.
        let mut u = sim();
        u.load_json(&s.save_json().unwrap()).unwrap();
        assert!(u.save_issues.is_empty());
    }

    #[test]
    fn older_saves_get_their_worn_items_properties() {
        use thedas_formats::gda::Cell;
        let table = EngineTable::parse(
            b"int EFFECT_DURATION_TYPE_INSTANT = 1;\nint EFFECT_DURATION_TYPE_PERMANENT = 3;\n",
        )
        .unwrap();
        let with_tables = || {
            let mut s = Sim::new(Arc::new(Vfs::new()), Arc::new(table.clone()), 7);
            s.module_info.name = "test".into();
            s.world.tables().insert(
                "itemprps",
                &[
                    "#4fe5e973",
                    "Effect",
                    "Int0",
                    "Int1",
                    "Float0",
                    "Float1",
                    "#36001b03",
                ],
                vec![(
                    1005,
                    vec![
                        Cell::Int(1),
                        Cell::Int(1012),
                        Cell::Int(6),
                        Cell::Int(0),
                        Cell::Float(1.0),
                        Cell::Float(0.0),
                        Cell::String("Float0".into()),
                    ],
                )],
            );
            s.set_item_look(
                "amulet",
                crate::sim::ItemLook {
                    properties: vec![(1005, 2)],
                    ..Default::default()
                },
            );
            s
        };
        let mut a = with_tables();
        let hero = a.world.spawn(ObjectKind::Creature, "hero");
        let amulet = a.world.spawn(ObjectKind::Item, "amulet");
        a.world.get_mut(amulet).unwrap().template = "amulet".into();
        a.world.get_mut(hero).unwrap().equipment.insert(11, amulet);
        let mut old = a.save_game();
        old.revision = 0;
        let mut b = with_tables();
        b.load_game(old).unwrap();
        let effects = &b.world.get(hero).unwrap().effects;
        assert_eq!(effects.len(), 1);
        assert_eq!(
            (effects[0].creator, effects[0].floats[0]),
            (Some(amulet), 2.0)
        );
        // A current save is taken as it is.
        let mut c = with_tables();
        c.load_game(a.save_game()).unwrap();
        assert!(c.world.get(hero).unwrap().effects.is_empty());
    }

    #[test]
    fn round_trips_the_world() {
        let mut a = sim();
        let id = a.world.spawn(ObjectKind::Creature, "dog");
        a.world.hero = Some(id);
        if let Some(o) = a.world.get_mut(id) {
            o.locals.ints.insert("barks".into(), 3);
            o.properties.insert(1, (10.0, 20.0));
            o.position = [1.0, 2.0, 3.0];
        }
        a.world.plots.insert(("PLOT".into(), 4), true);
        a.world.queue.insert(
            (500, 1),
            QueuedEvent {
                target: id,
                event: Event {
                    ty: 9,
                    ..Default::default()
                },
                script: None,
            },
        );
        a.world.time_ms = 1234;
        a.world.rng.next_u64();
        let bytes = a.save_json().unwrap();
        // Deterministic bytes.
        assert_eq!(bytes, a.save_json().unwrap());

        let mut b = sim();
        b.load_json(&bytes).unwrap();
        assert_eq!(b.world.time_ms, 1234);
        assert_eq!(b.world.hero, Some(id));
        assert_eq!(b.world.objects, a.world.objects);
        assert_eq!(b.world.plots, a.world.plots);
        assert_eq!(b.world.queue.len(), 1);
        assert_eq!(b.world.rng.next_u64(), a.world.rng.next_u64());
        // A new object gets the same id in both.
        assert_eq!(
            b.world.spawn(ObjectKind::Placeable, "x"),
            a.world.spawn(ObjectKind::Placeable, "x")
        );
        assert_ne!(id, OBJECT_INVALID);
    }

    #[test]
    fn refuses_other_modules_and_garbage() {
        let a = sim();
        let bytes = a.save_json().unwrap();
        let mut b = sim();
        b.module_info.name = "other".into();
        assert!(b.load_json(&bytes).is_err());
        assert!(b.load_json(b"{not json").is_err());
    }
}
