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
//! The game pauses only when the mode starts in explore or combat (0x692f81 to 0x692f9b:
//! game modes 0 and 1); started from a conversation (a dialogue line's action starts the
//! Dwarf Commoner's tactical-camera lesson while the hero still talks to the sister), the
//! conversation and the world run on. Saving is refused either way.
//!
//! Provisional: the done modes are kept for the session, not in a profile.

use crate::sim::Sim;
use serde::{Deserialize, Serialize};
use thedas_script::vm::ObjectId;

/// The training under way: its mode (0 none), the step shown (0 none), and the training event
/// (`EVENT_TYPE_TRAINING_*`) the step waits for, with the argument it must carry (0 any).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Training {
    pub mode: i32,
    pub step: i32,
    pub expected: i32,
    pub arg: i32,
    /// The game holds still for it (it started in explore or combat).
    #[serde(default)]
    pub paused: bool,
    /// The interface elements the step highlights (`HighlightGuiElement`, by
    /// `GUI_HIGHLIGHT_*` index 0 to 7: a bit each), and each one's argument (a follower's or
    /// slot's number).
    #[serde(default)]
    pub highlighted: u8,
    #[serde(default)]
    pub highlight_args: [i32; 8],
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
        let explore = crate::sim::constant(&self.table, "GM_EXPLORE");
        let combat = crate::sim::constant(&self.table, "GM_COMBAT");
        let gm = self.world.game_mode;
        self.world.training = Training {
            mode,
            paused: mode != 0 && (gm == explore || gm == combat),
            ..Default::default()
        };
        if mode != 0 {
            let module = self.world.module;
            self.raise_training_event("TRAINING_BEGIN", mode, module);
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

    /// `HighlightGuiElement`: the step points at interface element `element`
    /// (`GUI_HIGHLIGHT_FOLLOWER` 0, `TACTICS` 1, `INVENTORY` 2, `HEALTH_POULTICE` 3,
    /// `EMPTY_QBAR` 4, `TACTIC_PRESET` 5, `EQUIPMENT_SLOT` 6, `UNEQUIPPED_ITEM` 7; what the
    /// movies read as `GUI.GuiHighlights.<n>.Highlight` and `.HighlightArg`) until the player does what the step asks.
    /// step's event. Provisional: the exe's handler is not traced; the argument order is read
    /// from the scripts' calls (the element is the first parameter), and the highlight is
    /// taken to end when the player does what the step asks.
    pub fn highlight_gui_element(&mut self, element: i32, arg: i32) {
        if !(0..8).contains(&element) {
            return;
        }
        let t = &mut self.world.training;
        t.highlighted |= 1 << element;
        t.highlight_args[element as usize] = arg;
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
        // What the step pointed at is let go: the player did it.
        self.world.training.highlighted = 0;
        self.world.training.highlight_args = [0; 8];
        // The step's events run on the main controlled creature, not the module
        // (0x693290 targets 0x5d5cc0(), the player's main creature), and only for the
        // event the step waits for (+0x9c, checked above).
        let target = self.world.controlled().or(self.world.hero);
        self.raise_training_event(&name, t.mode, target);
        true
    }

    /// Whether training holds the game paused (see the module's doc).
    pub fn training_paused(&self) -> bool {
        self.world.training.mode != 0 && self.world.training.paused && !self.tutorials_held()
    }

    /// Tutorials wait for cinematic playback, including the handoff to a queued cut.
    /// The native tutorial pause path only pauses explore/combat (0x6940cd), not
    /// GameModeCutscene. Our cutscene clock is separate from `game_mode`.
    pub fn tutorials_held(&self) -> bool {
        self.game == thedas_vfs::Game::Origins
            && (self.world.cutscene.is_some() || !self.world.cutscenes.is_empty())
    }

    /// A queued tutorial's pause becomes effective when its panel can be shown.
    pub fn tutorial_paused(&self) -> bool {
        self.world.tutorial_paused && !self.tutorials_held()
    }

    /// The player's unpause request. A popup's pause belongs to its Next/Close
    /// action, as captured in Combat Basics; Space cannot dismiss that pause.
    pub fn unpause(&mut self) -> bool {
        if self.tutorial_paused() {
            return false;
        }
        if self.training_paused() && !self.training_event("UNPAUSE", 0) {
            return false;
        }
        true
    }

    /// Why the game cannot be saved now, if it cannot: training mode disables saving
    /// (script.ldf, SetTrainingMode: "Unpausing, Saving, Item Destruction"), so no save
    /// holds a training step. The player's save screen and the MCP's save tool refuse;
    /// `Sim::save_json` itself still writes (a tool's state dump is not a save).
    pub fn saving_blocked(&self) -> Option<&'static str> {
        (self.world.training.mode != 0)
            .then_some("a training tutorial is under way (it disables saving)")
    }

    /// Runs `genev_tutorial` on `target` for training event `name`, its integer the mode,
    /// now: `TRAINING_BEGIN` on the module (0x692e90), a step's events on the main
    /// controlled creature (0x693290 targets 0x5d5cc0(); sim's trace,
    /// equip-queue-and-disabled-events-trace.md). As DAOrigins.exe does: 0x693290 (at 0x693362)
    /// and 0x692e90 call 0x6985f0, which runs the script in the same call through the
    /// script VM (0xa31e70 -> 0xa31d00 -> the interpreter 0xa301b0, at 0x6986a0); nothing
    /// is queued. So the script's `BeginTrainingStep` (0x663760 -> 0x693190, which writes
    /// the next expected event at once) has armed the next step before the caller goes
    /// on: a step's event that arms the next lets an event raised right after it (in the
    /// same action) find that step. Reached from inside
    /// a running script the run nests (the VM's depth at +0x64; it refuses a tenth level,
    /// 0xa2dc53), as the simulation's other engine-raised events do. Not followed: the
    /// module's set of disabled event types (module+0x28c) that 0x6985f0 checks first.
    fn raise_training_event(&mut self, name: &str, mode: i32, target: Option<ObjectId>) {
        let Some(target) = target else {
            return;
        };
        let mut ev = self.new_event(name, self.world.hero);
        ev.ints = vec![mode];
        self.deliver(target, ev, Some(TRAINING_SCRIPT.to_string()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn sim() -> Sim {
        let ldf = "int EVENT_TYPE_TRAINING_BEGIN = 1059;\n\
                   int EVENT_TYPE_TRAINING_GUI_INVENTORY_OPENED = 1063;\n\
                   int EVENT_TYPE_TRAINING_MAXIMUM_ZOOM_IN = 1065;\n\
                   int EVENT_TYPE_TRAINING_UNPAUSE = 1069;\n\
                   int EVENT_TYPE_TRAINING_MOVEMENT_COMMAND_ISSUED = 1073;\n";
        let table = thedas_script::EngineTable::parse(ldf.as_bytes()).unwrap_or_default();
        let mut s = Sim::new(Arc::new(thedas_vfs::Vfs::new()), Arc::new(table), 1);
        s.world.trace.keep = true;
        let m = s.world.spawn(crate::world::ObjectKind::Module, "module");
        s.world.module = Some(m);
        let hero = s.world.spawn(crate::world::ObjectKind::Creature, "player");
        s.world.hero = Some(hero);
        s
    }

    /// The training events delivered so far (to `genev_tutorial`), and clears the record.
    fn delivered(s: &mut Sim) -> Vec<i32> {
        let out = s
            .world
            .trace
            .entries
            .iter()
            .filter_map(|e| match e {
                crate::trace::TraceEntry::EventDelivered { ty, script, .. }
                    if script == TRAINING_SCRIPT =>
                {
                    Some(*ty)
                }
                _ => None,
            })
            .collect();
        s.world.trace.entries.clear();
        out
    }

    #[test]
    fn ordinary_tutorial_next_releases_pause_and_replays() {
        use crate::commands::{CommandSource, SimCommand};
        use crate::replay::{JournalWriter, replay};

        let mut s = sim();
        s.world.tutorials.push(crate::journal::Tutorial {
            resref: "tut_combat_basic".into(),
            title: Some(264859),
            text: Some(264857),
            pauses: true,
        });
        s.world.tutorial_paused = true;
        let checkpoint = s.world.clone();
        let mut journal = JournalWriter::start(&mut s, "test", Vec::new()).unwrap();
        assert!(!s.input(CommandSource::Player, SimCommand::Unpause));
        assert!(s.tutorial_paused());
        assert_eq!(s.world.tutorials.len(), 1);
        assert!(s.input(
            CommandSource::Player,
            SimCommand::ConsumeTutorial { all: false }
        ));
        journal.take(&mut s).unwrap();
        assert!(!s.tutorial_paused());
        assert!(s.world.tutorials.is_empty());
        assert_eq!(s.world.time_ms, checkpoint.time_ms);

        let mut again = sim();
        again.world = checkpoint;
        let log = journal.into_inner();
        let report = replay(&mut again, &log[..], "test", false);
        assert_eq!(
            (report.applied, report.mismatch, report.refused),
            (2, None, None)
        );
        assert!(!again.tutorial_paused());
        assert!(again.world.tutorials.is_empty());
    }

    #[test]
    fn unpause_cannot_bypass_the_movement_lesson() {
        use crate::commands::{CommandSource, SimCommand};

        let mut s = sim();
        s.set_training_mode(2);
        s.begin_training_step(2, 1073, 0);
        delivered(&mut s);
        assert!(!s.input(CommandSource::Player, SimCommand::Unpause));
        assert!(s.training_paused());
        assert_eq!(s.world.training.expected, 1073);
        assert!(delivered(&mut s).is_empty());

        assert!(s.training_event("MOVEMENT_COMMAND_ISSUED", 0));
        assert_eq!(delivered(&mut s), vec![1073]);
        // The shipped movement handler arms step 9 with this expected event.
        s.begin_training_step(9, 1069, 0);
        assert!(s.input(CommandSource::Player, SimCommand::Unpause));
        assert_eq!(delivered(&mut s), vec![1069]);
        assert!(!s.tutorial_paused());
        assert!(
            !s.input(CommandSource::Player, SimCommand::Unpause),
            "heard once"
        );
        // The shipped UNPAUSE handler ends the mode with SetTrainingMode(0).
        s.set_training_mode(0);
        assert!(!s.training_paused());
    }

    #[test]
    fn unpause_during_a_cinematic_keeps_the_deferred_tutorial_pause() {
        let mut s = sim();
        s.world.tutorial_paused = true;
        s.world.cutscenes.push("intro.cut".into());
        assert!(s.unpause());
        assert!(!s.tutorial_paused());
        s.world.cutscenes.clear();
        assert!(s.tutorial_paused());
    }

    #[test]
    fn a_lesson_started_in_a_conversation_does_not_pause_the_game() {
        let mut s = sim();
        s.world.game_mode = 4;
        s.set_training_mode(1);
        assert_eq!(s.world.training.mode, 1);
        assert!(!s.training_paused(), "the conversation runs on");
        assert!(s.saving_blocked().is_some(), "saving is still refused");
    }

    #[test]
    fn cinematic_playback_holds_tutorials_without_losing_the_pending_pause() {
        let mut s = sim();
        s.set_training_mode(1);
        s.begin_training_step(1, 1065, 0);
        s.world.tutorial_paused = true;
        assert!(s.training_paused());
        assert!(s.tutorial_paused());
        let cut = crate::cutscene::Cutscene {
            name: "intro.cut".into(),
            seconds: 5.0,
            lines: vec![],
            elapsed_ms: 0,
            from_dialog: false,
            bound: vec![],
            placement: None,
        };
        s.world.cutscenes.push(cut.name.clone());
        assert!(s.tutorials_held(), "hold the queued handoff too");
        assert!(!s.training_paused());
        assert!(!s.tutorial_paused());
        s.world.cutscenes.clear();
        s.world.cutscene = Some(cut);
        assert!(s.tutorials_held());
        assert!(!s.training_paused());
        assert!(!s.tutorial_paused());
        assert!(s.saving_blocked().is_some());
        s.game = thedas_vfs::Game::DragonAge2;
        assert!(!s.tutorials_held(), "DA2 keeps its existing behavior");
        s.game = thedas_vfs::Game::Origins;
        s.world.cutscene = None;
        assert_eq!(s.world.training.step, 1);
        assert!(s.training_paused(), "the lesson resumes after the cut");
        assert!(s.tutorial_paused());
    }

    #[test]
    fn a_training_mode_pauses_waits_for_its_event_and_runs_once() {
        let mut s = sim();
        s.set_training_mode(1);
        assert!(s.training_paused());
        // Delivered at once, not queued (see `Sim::raise_training_event`).
        assert_eq!(delivered(&mut s), vec![1059]);
        assert!(s.world.queue.is_empty());
        s.begin_training_step(1, 1065, 0);
        assert_eq!(s.world.training.step, 1);
        // Only the event the step waits for goes to the module.
        assert!(!s.training_event("GUI_INVENTORY_OPENED", 0));
        assert!(s.training_event("MAXIMUM_ZOOM_IN", 0));
        assert_eq!(delivered(&mut s), vec![1065]);
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

    #[test]
    fn zooming_fully_in_ends_the_tactical_step_once() {
        let mut s = sim();
        s.set_training_mode(1);
        s.begin_training_step(1, 1065, 0);
        delivered(&mut s);
        assert!(s.player_zoomed_in());
        assert_eq!(delivered(&mut s), vec![1065]);
        // Heard once: zooming in again does nothing more.
        assert!(!s.player_zoomed_in());
    }

    #[test]
    fn with_tutorials_off_or_the_mode_done_no_tactical_step_starts() {
        let mut s = sim();
        s.show_tutorials = false;
        s.set_training_mode(1);
        assert!(!s.training_paused());
        let mut s = sim();
        s.training_done = 1;
        s.set_training_mode(1);
        assert!(!s.training_paused());
        assert!(s.world.zoom_request.is_none());
    }

    /// `genev_tutorial` as the inventory lesson's step 7 handler: whatever it hears, it
    /// arms step 10 waiting for `ITEM_EQUIPPED` (1071) in slot 4.
    fn arms_equip_step() -> thedas_script::ncs::Ncs {
        let mut b: Vec<u8> = Vec::new();
        for v in [4i32, 1071, 10] {
            b.extend([0x04, 0x03]);
            b.extend(v.to_be_bytes());
        }
        // ACTION BeginTrainingStep (843), 3 arguments; RETN.
        b.extend([0x05, 0x00]);
        b.extend(843u16.to_be_bytes());
        b.extend([0x03, 0x20, 0x00]);
        let mut v = b"NCS V1.0B".to_vec();
        v.extend(((13 + b.len()) as u32).to_be_bytes());
        v.extend(b);
        thedas_script::ncs::Ncs::parse(&v).unwrap()
    }

    #[test]
    fn an_event_finds_the_step_the_one_before_it_armed() {
        let ldf = "int EVENT_TYPE_TRAINING_BEGIN = 1059;\n\
                   int EVENT_TYPE_TRAINING_ITEM_UNEQUIPPED = 1070;\n\
                   int EVENT_TYPE_TRAINING_ITEM_EQUIPPED = 1071;\n\
                   void BeginTrainingStep(int nStepID, int nExpectedTrainingEvent, int nTrainingEventArg = 0) = 843;\n";
        let table = thedas_script::EngineTable::parse(ldf.as_bytes()).unwrap();
        let mut s = Sim::new(Arc::new(thedas_vfs::Vfs::new()), Arc::new(table), 1);
        s.world.trace.keep = true;
        let m = s.world.spawn(crate::world::ObjectKind::Module, "module");
        s.world.module = Some(m);
        let hero = s.world.spawn(crate::world::ObjectKind::Creature, "player");
        s.world.hero = Some(hero);
        s.add_script(TRAINING_SCRIPT, arms_equip_step());
        s.set_training_mode(5);
        s.begin_training_step(7, 1070, 4);
        // An unequip, then an equip, with nothing run in between: the first arms the step
        // the second ends (the step's script runs during the first, see
        // `Sim::raise_training_event`).
        assert!(s.training_event("ITEM_UNEQUIPPED", 4));
        assert_eq!(s.world.training.step, 10);
        assert_eq!(s.world.training.expected, 1071);
        assert!(s.training_event("ITEM_EQUIPPED", 4));
        // Delivered to the main controlled creature, not the module (0x693290).
        assert!(s.world.trace.entries.iter().any(|e| matches!(
            e,
            crate::trace::TraceEntry::EventDelivered { ty: 1071, target, .. } if *target == hero
        )));
    }

    #[test]
    fn a_step_highlights_interface_elements_until_the_next_step() {
        let mut s = sim();
        s.set_training_mode(2);
        s.begin_training_step(3, 1063, 0);
        s.highlight_gui_element(2, 0);
        s.highlight_gui_element(6, 4);
        let t = s.world.training;
        assert_eq!(t.highlighted, 1 << 2 | 1 << 6);
        assert_eq!(t.highlight_args[6], 4);
        assert!(s.training_event("GUI_INVENTORY_OPENED", 0));
        assert_eq!(s.world.training.highlighted, 0);
        // An element out of range is ignored.
        s.highlight_gui_element(9, 0);
        assert_eq!(s.world.training.highlighted, 0);
    }
}
