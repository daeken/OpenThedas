//! The training tutorials: the guided first steps (the tactical camera, move orders, tactics,
//! level-up, inventory, the quickbar) that the scripts walk the player through.
//!
//! `SetTrainingMode(mode)` (DAOrigins.exe 0x692e90) starts a training mode: the game pauses
//! (it may not be unpaused or saved meanwhile) and `EVENT_TYPE_TRAINING_BEGIN` with the mode
//! runs `genev_tutorial` on the module. `BeginTrainingStep(step, event, arg)` (0x693190)
//! shows `trainingsteps.gda`'s row `step` (popuplayer.gfx's `Tutorials.TrainingStep`) and
//! arms the one training event the engine raises next: when the player does that action
//! (opens the inventory, orders a move, zooms in fully, ...) `genev_tutorial` gets that event
//! with the mode (0x693290), and its scripts go on to the next step or end the mode with
//! `SetTrainingMode(0)`. A mode that ends is marked done and never starts again (the exe
//! keeps the done modes as bits in the player's profile, `CompletedTraining`). With
//! tutorials off no mode starts.
//!
//! Provisional: the done modes are kept for the session, not in a profile; the exe pauses
//! only in game modes 0 and 1 (explore, combat), which is not checked here.

use crate::sim::Sim;
use serde::{Deserialize, Serialize};

/// The training under way: its mode (0 none), the step shown (0 none), and the training event
/// (`EVENT_TYPE_TRAINING_*`) the step waits for, with the argument it must carry (0 any).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Training {
    pub mode: i32,
    pub step: i32,
    pub expected: i32,
    pub arg: i32,
}

/// The script the training events run (on the module), not the module's own: DAOrigins.exe
/// posts them with it (0x6985f0 with the name at 0xb0e864).
pub const TRAINING_SCRIPT: &str = "genev_tutorial";

/// The highest training mode (the done modes are bits of a 32-bit mask).
const MAX_MODE: i32 = 32;

impl Sim {
    /// `SetTrainingMode`: starts training mode `mode`, or ends the one under way (0).
    pub fn set_training_mode(&mut self, mode: i32) {
        let done = |m: i32| (1..=MAX_MODE).contains(&m) && self.training_done & (1 << (m - 1)) != 0;
        let mode = if !self.show_tutorials || !(0..=MAX_MODE).contains(&mode) || done(mode) {
            0
        } else {
            mode
        };
        let old = self.world.training;
        if mode == old.mode {
            return;
        }
        if old.mode > 0 {
            self.training_done |= 1 << (old.mode - 1);
        }
        self.world.training = Training {
            mode,
            ..Default::default()
        };
        if mode != 0 {
            self.raise_training_event("TRAINING_BEGIN", mode);
        }
    }

    /// `BeginTrainingStep`: shows training step `step` and waits for `expected` (with `arg`,
    /// or any argument for 0).
    pub fn begin_training_step(&mut self, step: i32, expected: i32, arg: i32) {
        if self.world.training.mode == 0 {
            return;
        }
        let t = &mut self.world.training;
        t.step = step;
        t.expected = expected;
        t.arg = arg;
    }

    /// The player did a training action (`EVENT_TYPE_TRAINING_<name>`, with its argument):
    /// when the step under way waits for it, `genev_tutorial` gets it. Returns whether it did.
    pub fn training_event(&mut self, name: &str, arg: i32) -> bool {
        let t = self.world.training;
        let name = format!("TRAINING_{name}");
        let ty = self.event_type(&name);
        if t.mode == 0 || ty == 0 || t.expected != ty || (arg != 0 && t.arg != 0 && arg != t.arg) {
            return false;
        }
        // Heard once: the step waits for nothing more until the scripts set the next.
        // Provisional: the exe's 0x693290 does not clear it; a move order held down would
        // otherwise post the event every frame.
        self.world.training.expected = 0;
        self.raise_training_event(&name, t.mode);
        true
    }

    /// Whether training holds the game paused.
    pub fn training_paused(&self) -> bool {
        self.world.training.mode != 0
    }

    /// Queues training event `name` (`TRAINING_BEGIN`) to the module, its integer the mode.
    fn raise_training_event(&mut self, name: &str, mode: i32) {
        let Some(module) = self.world.module else {
            return;
        };
        let mut ev = self.new_event(name, self.world.hero);
        ev.ints = vec![mode];
        self.world
            .queue_event(0, module, ev, Some(TRAINING_SCRIPT.to_string()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn sim() -> Sim {
        let ldf = "int EVENT_TYPE_TRAINING_BEGIN = 1059;\n\
                   int EVENT_TYPE_TRAINING_GUI_INVENTORY_OPENED = 1063;\n\
                   int EVENT_TYPE_TRAINING_MAXIMUM_ZOOM_IN = 1065;\n";
        let table = thedas_script::EngineTable::parse(ldf.as_bytes()).unwrap_or_default();
        let mut s = Sim::new(Arc::new(thedas_vfs::Vfs::new()), Arc::new(table), 1);
        let m = s.world.spawn(crate::world::ObjectKind::Module, "module");
        s.world.module = Some(m);
        s
    }

    fn queued(s: &Sim) -> Vec<(i32, Vec<i32>)> {
        s.world
            .queue
            .iter()
            .map(|(_, q)| (q.event.ty, q.event.ints.clone()))
            .collect()
    }

    #[test]
    fn a_training_mode_pauses_waits_for_its_event_and_runs_once() {
        let mut s = sim();
        s.set_training_mode(1);
        assert!(s.training_paused());
        assert_eq!(queued(&s), vec![(1059, vec![1])]);
        s.world.queue.clear();
        s.begin_training_step(1, 1065, 0);
        assert_eq!(s.world.training.step, 1);
        // Only the event the step waits for goes to the module.
        assert!(!s.training_event("GUI_INVENTORY_OPENED", 0));
        assert!(s.training_event("MAXIMUM_ZOOM_IN", 0));
        assert_eq!(queued(&s), vec![(1065, vec![1])]);
        s.set_training_mode(0);
        assert!(!s.training_paused());
        assert_eq!(s.world.training, Training::default());
        // Done: it does not start again.
        s.set_training_mode(1);
        assert!(!s.training_paused());
        // With tutorials off nothing starts.
        s.show_tutorials = false;
        s.set_training_mode(2);
        assert!(!s.training_paused());
    }
}
