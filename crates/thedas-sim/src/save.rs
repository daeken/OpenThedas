//! Saving and loading the simulation as JSON.
//!
//! This is OpenThedas's own save format, not the game's `.das` saves: the world (objects,
//! locals, engine values, plots, the event queue, time and the random generator) and the
//! conversation in progress. Loading needs a simulation of the same module, which supplies
//! the resources, scripts and tables; the trace starts afresh.

use serde::{Deserialize, Serialize};

use crate::dialog::Conversation;
use crate::sim::{Sim, SimError};
use crate::world::World;

/// Bumped when the layout of a save changes incompatibly.
pub const SAVE_FORMAT: u32 = 1;

#[derive(Serialize, Deserialize)]
pub struct SaveGame {
    pub format: u32,
    /// The module the world belongs to (`ModuleInfo::name`).
    pub module: String,
    pub world: World,
    pub talk: Option<Conversation>,
}

impl Sim {
    pub fn save_game(&self) -> SaveGame {
        SaveGame {
            format: SAVE_FORMAT,
            module: self.module_info.name.clone(),
            world: self.world.clone(),
            talk: self.talk.clone(),
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
        let mut world = save.world;
        world.tables = self.world.tables.clone();
        world.trace = std::mem::take(&mut self.world.trace);
        world.trace.entries.clear();
        self.world = world;
        self.talk = save.talk;
        // Caches keyed by object id belong to the world just replaced.
        self.area_layouts.clear();
        self.door_cells.clear();
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
