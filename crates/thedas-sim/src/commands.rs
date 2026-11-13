//! Commands given to the simulation from outside it: mod mode, the MCP server and test
//! harnesses set plot flags and start conversations through [`Sim::apply`], and every call
//! is kept in the world's journal ([`crate::world::World::command_journal`]) with what it
//! did. Mod mode's entries are its edit history: they are never dropped, and a flag set
//! records the value it replaced, for undo.

use thedas_script::vm::{OBJECT_INVALID, ObjectId};

use crate::sim::Sim;
use crate::world::ObjectKind;

/// Entries kept from sources other than mod mode; the oldest go first.
pub const JOURNAL_CAP: usize = 512;

/// Who gave a command.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandSource {
    ModMode,
    Mcp,
    Test,
}

/// A command from outside the simulation.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum SimCommand {
    /// Sets or clears a plot flag (`plot`: its resource name or GUID), running the plot's
    /// script with `call_script` as a script's `WR_SetPlotFlag` would.
    SetPlotFlag {
        plot: String,
        flag: i32,
        value: bool,
        call_script: bool,
    },
    /// Starts a conversation between `initiator` and `target` (its owner); an empty
    /// `dialog` is the target's own conversation.
    StartConversation {
        initiator: ObjectId,
        target: ObjectId,
        dialog: String,
    },
}

/// What a command did.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum CommandOutcome {
    /// The flag was set. `replaced` is the stored value it replaced (what to set back to
    /// undo it); `None` for a defined flag, which stores nothing (its plot script answers
    /// for it, so there is nothing to put back).
    PlotFlagSet {
        plot: String,
        replaced: Option<bool>,
    },
    /// The conversation started (`opened`: on the dialog screen; otherwise said in passing
    /// or waiting for the open one to end).
    ConversationStarted { dialog: String, opened: bool },
    /// Nothing was done.
    Refused { reason: String },
}

/// A command given, and what it did.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct JournalEntry {
    /// Simulation time when it was given, in milliseconds.
    pub t: u64,
    pub source: CommandSource,
    pub command: SimCommand,
    pub outcome: CommandOutcome,
}

impl Sim {
    /// Carries out a command from outside the simulation and keeps it in the journal.
    pub fn apply(&mut self, source: CommandSource, command: SimCommand) -> CommandOutcome {
        let outcome = match &command {
            SimCommand::SetPlotFlag {
                plot,
                flag,
                value,
                call_script,
            } => {
                if self.plot_info(plot).is_none() {
                    CommandOutcome::Refused {
                        reason: format!("no plot {plot}"),
                    }
                } else {
                    let key = self.plot_key(plot);
                    let replaced = (*flag < crate::plot::DEFINED_FLAGS)
                        .then(|| self.stored_flag_for_undo(plot, *flag));
                    let owner = self.world.hero.unwrap_or(OBJECT_INVALID);
                    self.set_plot_flag(plot, *flag, *value, *call_script, owner);
                    CommandOutcome::PlotFlagSet {
                        plot: key,
                        replaced,
                    }
                }
            }
            SimCommand::StartConversation {
                initiator,
                target,
                dialog,
            } => self.start_commanded_conversation(*initiator, *target, dialog),
        };
        let entry = JournalEntry {
            t: self.world.time_ms,
            source,
            command,
            outcome: outcome.clone(),
        };
        let journal = &mut self.world.command_journal;
        journal.push(entry);
        if source != CommandSource::ModMode {
            let others = journal
                .iter()
                .filter(|e| e.source != CommandSource::ModMode)
                .count();
            if others > JOURNAL_CAP
                && let Some(i) = journal
                    .iter()
                    .position(|e| e.source != CommandSource::ModMode)
            {
                journal.remove(i);
            }
        }
        outcome
    }

    /// A conversation asked for from outside: its owner must be an active, living creature
    /// (or nobody), as DAOrigins.exe checks before a conversation is requested (0x6976b0).
    fn start_commanded_conversation(
        &mut self,
        initiator: ObjectId,
        target: ObjectId,
        dialog: &str,
    ) -> CommandOutcome {
        let refused = |reason: &str| CommandOutcome::Refused {
            reason: reason.to_string(),
        };
        if target != OBJECT_INVALID {
            match self.world.get(target) {
                None => return refused("no such owner"),
                Some(o) if o.kind == ObjectKind::Creature && (!o.active || o.dead) => {
                    return refused("the owner is not an active, living creature");
                }
                _ => {}
            }
        }
        if self.world.get(initiator).is_none() {
            return refused("no such initiator");
        }
        let was_open = self.talk.as_ref().is_some_and(|c| !c.ended);
        if !self.start_conversation(initiator, target, dialog) {
            return refused("the conversation could not be read, or the owner has none");
        }
        let name = self
            .talk
            .as_ref()
            .map(|c| c.dlg.name.clone())
            .unwrap_or_else(|| dialog.to_string());
        let opened = !was_open && self.talk.as_ref().is_some_and(|c| !c.ended);
        CommandOutcome::ConversationStarted {
            dialog: name,
            opened,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sim() -> Sim {
        Sim::new(
            std::sync::Arc::new(thedas_vfs::Vfs::new()),
            std::sync::Arc::new(thedas_script::EngineTable::default()),
            1,
        )
    }

    #[test]
    fn commands_are_journalled_and_refusals_say_why() {
        let mut s = sim();
        let hero = s.world.spawn(ObjectKind::Creature, "player");
        s.world.hero = Some(hero);
        let out = s.apply(
            CommandSource::Mcp,
            SimCommand::SetPlotFlag {
                plot: "no_such_plot".into(),
                flag: 1,
                value: true,
                call_script: true,
            },
        );
        assert!(matches!(out, CommandOutcome::Refused { .. }));
        let gone = s.world.spawn(ObjectKind::Creature, "gone");
        s.world.get_mut(gone).unwrap().active = false;
        let out = s.apply(
            CommandSource::ModMode,
            SimCommand::StartConversation {
                initiator: hero,
                target: gone,
                dialog: String::new(),
            },
        );
        assert_eq!(
            out,
            CommandOutcome::Refused {
                reason: "the owner is not an active, living creature".into()
            }
        );
        assert_eq!(s.world.command_journal.len(), 2);
        assert_eq!(s.world.command_journal[1].source, CommandSource::ModMode);
    }

    #[test]
    fn only_entries_from_outside_mod_mode_are_capped() {
        let mut s = sim();
        let cmd = |n: i32| SimCommand::SetPlotFlag {
            plot: "none".into(),
            flag: n,
            value: true,
            call_script: false,
        };
        for n in 0..10 {
            s.apply(CommandSource::ModMode, cmd(n));
        }
        for n in 0..(JOURNAL_CAP as i32 + 50) {
            s.apply(CommandSource::Test, cmd(n));
        }
        let j = &s.world.command_journal;
        assert_eq!(
            j.iter()
                .filter(|e| e.source == CommandSource::ModMode)
                .count(),
            10
        );
        assert_eq!(
            j.iter().filter(|e| e.source == CommandSource::Test).count(),
            JOURNAL_CAP
        );
    }
}
