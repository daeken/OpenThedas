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
pub const SAVE_REVISION: u32 = 2;

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

    pub fn save_json(&self) -> Result<Vec<u8>, SimError> {
        serde_json::to_vec(&self.save_game())
            .map_err(|e| SimError::Parse("save".into(), e.to_string()))
    }

    /// Replaces the world with a saved one. The save must be for this simulation's module.
    pub fn load_game(&mut self, save: SaveGame) -> Result<(), SimError> {
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
        self.world = world;
        self.talk = save.talk;
        // Caches keyed by object id belong to the world just replaced.
        self.area_layouts.clear();
        self.door_cells.clear();
        if save_revision < 1 {
            self.apply_worn_properties();
        }
        if save_revision < 2 {
            self.restore_lost_appearances();
        }
        // Saves from before world maps were simulated have none.
        if !self
            .world
            .objects
            .values()
            .any(|o| o.kind == crate::world::ObjectKind::Map)
        {
            self.load_world_maps();
        }
        Ok(())
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
        // SetAppearanceType -1 is the creature's own; setting a party member's to 0 is traced.
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
        assert!(s.world.trace.entries.iter().any(|e| matches!(
            e,
            crate::trace::TraceEntry::Log { text, .. } if text.contains("leaves it nothing to draw")
        )));
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
