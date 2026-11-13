//! Commands given to the simulation from outside it: mod mode, the MCP server and test
//! harnesses set plot flags and start conversations through [`Sim::apply`], and every call
//! is kept in the world's journal ([`crate::world::World::command_journal`]) with what it
//! did. Mod mode's entries are its edit history: they are never dropped, and a flag set
//! records the value it replaced, for undo. Play inputs (the clock, the player's orders,
//! the interface's messages) go through the same door; the journal keeps only edits, and
//! every command goes to [`Sim::input_log`] when it is on (journal-style saves replay it).

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
    /// The player, through the game's controls and interface.
    Player,
    /// The app itself (its clock, debug steps, the game's own flow outside the scripts).
    App,
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
    /// The simulation's clock goes on by `ms` milliseconds (0: what is due now runs).
    Tick {
        ms: u64,
    },
    /// Player orders (see the `order_*` methods).
    OrderMove {
        who: ObjectId,
        to: [f32; 3],
    },
    OrderAttack {
        who: ObjectId,
        target: ObjectId,
    },
    OrderTalk {
        who: ObjectId,
        target: ObjectId,
    },
    OrderUse {
        who: ObjectId,
        target: ObjectId,
    },
    OrderAbility {
        who: ObjectId,
        ability: i32,
        target: ObjectId,
        item: String,
    },
    OrderAbilityAt {
        who: ObjectId,
        ability: i32,
        at: [f32; 3],
        item: String,
    },
    /// The player picks who they control.
    PlayerSelects {
        who: ObjectId,
    },
    /// The camera came close to the controlled creature (a tutorial waits for it).
    PlayerZoomedIn,
    /// The player picks reply `choice` of the open conversation.
    ChooseReply {
        choice: usize,
    },
    EndConversation,
    SkipCutscene,
    EndCutscene {
        name: String,
    },
    /// A message from the interface's movies (`GUI.*`), or an options message.
    GuiMessage {
        name: String,
        args: Vec<String>,
    },
    ClientOptionMessage {
        name: String,
        args: Vec<String>,
    },
    GuiOpened {
        gui: i32,
    },
    TrainingEvent {
        name: String,
        arg: i32,
    },
    SetTrainingMode {
        mode: i32,
    },
    ConsumeTutorial {
        all: bool,
    },
    AnswerPopup {
        button: i32,
        text: String,
    },
    RequestLevelUp {
        creature: ObjectId,
    },
    SetFaceChoice {
        creature: ObjectId,
        slider: u32,
        value: f32,
    },
    ChargenSoundSet {
        set: String,
    },
    PartyPicker,
    PartyPickerToggle {
        who: ObjectId,
    },
    PartyPickerSubmit,
    CloseStore,
    CloseContainer,
    BeginTravel {
        pin: ObjectId,
    },
    /// What runs before the game is saved.
    Presave,
    /// A debug kill (the app's test steps).
    Kill {
        id: ObjectId,
        killer: ObjectId,
    },
    /// An area transition, as a script's `DoAreaTransition` asks for one (the app's test
    /// steps): performed at the start of the next update.
    Transition {
        area: String,
        waypoint: String,
    },
    /// A cutscene queued to play (the app's test steps).
    QueueCutscene {
        name: String,
    },
    /// Whether the party AI plays the controlled character too (test drivers; the app turns
    /// it off when a game starts).
    SetAutopilot {
        on: bool,
    },
    /// What happens when a script shows the party picker (the app: the player picks).
    SetPartyPickerPolicy {
        policy: crate::sim::PartyPickerPolicy,
    },
}

impl SimCommand {
    /// Whether the world's journal keeps it: edits (plot flags, conversations started from
    /// outside), not the stream of play inputs, which goes to [`Sim::input_log`].
    pub fn is_edit(&self) -> bool {
        matches!(
            self,
            SimCommand::SetPlotFlag { .. } | SimCommand::StartConversation { .. }
        )
    }
}

/// What a command did.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum CommandOutcome {
    /// The flag was set. `replaced` is the stored value it replaced (what to set back to
    /// undo it); `None` for a defined flag, which stores nothing (its plot script answers
    /// for it, so there is nothing to put back).
    /// `changed` is every flag the command set, the plot scripts' own sets included, in
    /// order, each with what it replaced: undo sets them back in reverse.
    PlotFlagSet {
        plot: String,
        replaced: Option<bool>,
        changed: Vec<crate::plot::FlagChange>,
    },
    /// The conversation started: `opened` on the dialog screen, else said in passing.
    ConversationStarted { dialog: String, opened: bool },
    /// Another conversation is open: this one starts when it ends (if its owner is still
    /// there to talk).
    ConversationQueued { dialog: String },
    /// A play input carried out (`ok`: what the method answered, where it answers).
    Done { ok: bool },
    /// Nothing was done.
    Refused { reason: String },
}

/// A play input as the input log keeps it (see [`Sim::input_log`]).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct InputEntry {
    /// Simulation time when it was given, in milliseconds.
    pub t: u64,
    pub source: CommandSource,
    pub command: SimCommand,
}

/// An object named by a command, as it can be found again in another session: object
/// ids change between sessions (add-ins shift them), so its tag, area and template are
/// kept with the id the live call used.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ObjectRef {
    pub id: ObjectId,
    pub tag: String,
    /// The tag of the area it was in.
    pub area: Option<String>,
    /// Its template (resource name).
    pub template: String,
}

/// A command given, and what it did.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct JournalEntry {
    /// Simulation time when it was given, in milliseconds.
    pub t: u64,
    pub source: CommandSource,
    pub command: SimCommand,
    pub outcome: CommandOutcome,
    /// The objects the command named (a conversation's initiator, then its owner).
    #[serde(default)]
    pub objects: Vec<ObjectRef>,
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
                    let owner = self.world.hero.unwrap_or(OBJECT_INVALID);
                    let outer = self.plots.sets.replace(Vec::new());
                    self.set_plot_flag(plot, *flag, *value, *call_script, owner);
                    let changed =
                        std::mem::replace(&mut self.plots.sets, outer).unwrap_or_default();
                    let replaced = changed
                        .iter()
                        .find(|c| c.plot == key && c.flag == *flag)
                        .and_then(|c| c.replaced);
                    if let Some(sets) = self.plots.sets.as_mut() {
                        sets.extend(changed.iter().cloned());
                    }
                    CommandOutcome::PlotFlagSet {
                        plot: key,
                        replaced,
                        changed,
                    }
                }
            }
            SimCommand::StartConversation {
                initiator,
                target,
                dialog,
            } => self.start_commanded_conversation(*initiator, *target, dialog),
            other => CommandOutcome::Done {
                ok: self.play_input(other),
            },
        };
        if let Some(log) = self.input_log.as_mut() {
            log.push(InputEntry {
                t: self.world.time_ms,
                source,
                command: command.clone(),
            });
        }
        if !command.is_edit() {
            return outcome;
        }
        let objects = match &command {
            SimCommand::StartConversation {
                initiator, target, ..
            } => [*initiator, *target]
                .iter()
                .filter_map(|&id| self.object_ref(id))
                .collect(),
            _ => Vec::new(),
        };
        let entry = JournalEntry {
            t: self.world.time_ms,
            source,
            command,
            outcome: outcome.clone(),
            objects,
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

    /// Carries out a play input; what the method answered, where it answers.
    fn play_input(&mut self, c: &SimCommand) -> bool {
        use SimCommand as C;
        match c {
            C::SetPlotFlag { .. } | C::StartConversation { .. } => false,
            C::Tick { ms } => {
                self.tick(*ms);
                true
            }
            C::OrderMove { who, to } => {
                self.order_move(*who, *to);
                true
            }
            C::OrderAttack { who, target } => {
                self.order_attack(*who, *target);
                true
            }
            C::OrderTalk { who, target } => {
                self.order_talk(*who, *target);
                true
            }
            C::OrderUse { who, target } => {
                self.order_use(*who, *target);
                true
            }
            C::OrderAbility {
                who,
                ability,
                target,
                item,
            } => {
                self.order_ability(*who, *ability, *target, item);
                true
            }
            C::OrderAbilityAt {
                who,
                ability,
                at,
                item,
            } => {
                self.order_ability_at(*who, *ability, *at, item);
                true
            }
            C::PlayerSelects { who } => {
                self.player_selects(*who);
                true
            }
            C::PlayerZoomedIn => self.player_zoomed_in(),
            C::ChooseReply { choice } => self.choose_reply(*choice).is_some(),
            C::EndConversation => {
                self.end_conversation();
                true
            }
            C::SkipCutscene => {
                self.skip_cutscene();
                true
            }
            C::EndCutscene { name } => {
                self.end_cutscene(name);
                true
            }
            C::GuiMessage { name, args } => self.gui_message(name, args),
            C::ClientOptionMessage { name, args } => self.client_option_message(name, args),
            C::GuiOpened { gui } => {
                self.gui_opened(*gui);
                true
            }
            C::TrainingEvent { name, arg } => self.training_event(name, *arg),
            C::SetTrainingMode { mode } => {
                self.set_training_mode(*mode);
                true
            }
            C::ConsumeTutorial { all } => {
                self.consume_tutorial(*all);
                true
            }
            C::AnswerPopup { button, text } => self.answer_popup_with_text(*button, text),
            C::RequestLevelUp { creature } => {
                self.request_level_up(*creature);
                true
            }
            C::SetFaceChoice {
                creature,
                slider,
                value,
            } => {
                self.set_face_choice(*creature, *slider, *value);
                true
            }
            C::ChargenSoundSet { set } => self.chargen_sound_set(set),
            C::PartyPicker => {
                self.party_picker();
                true
            }
            C::PartyPickerToggle { who } => self.party_picker_toggle(*who),
            C::PartyPickerSubmit => self.party_picker_submit(),
            C::CloseStore => {
                self.close_store();
                true
            }
            C::CloseContainer => {
                self.close_container();
                true
            }
            C::BeginTravel { pin } => self.begin_travel(*pin),
            C::Presave => {
                self.presave();
                true
            }
            C::Kill { id, killer } => {
                self.kill(*id, *killer);
                true
            }
            C::Transition { area, waypoint } => {
                self.world.transition = Some((area.clone(), waypoint.clone()));
                self.world.transition_at =
                    Some(self.world.time_ms + crate::sim::TRANSITION_DELAY_MS);
                true
            }
            C::QueueCutscene { name } => {
                self.world.cutscenes.push(name.clone());
                true
            }
            C::SetAutopilot { on } => {
                self.world.autopilot = *on;
                true
            }
            C::SetPartyPickerPolicy { policy } => {
                self.party_picker_policy = *policy;
                true
            }
        }
    }

    /// Carries out a play input from the player or the app, answering what its method
    /// answered (see [`Sim::apply`]).
    pub fn input(&mut self, source: CommandSource, command: SimCommand) -> bool {
        matches!(
            self.apply(source, command),
            CommandOutcome::Done { ok: true }
                | CommandOutcome::PlotFlagSet { .. }
                | CommandOutcome::ConversationStarted { .. }
                | CommandOutcome::ConversationQueued { .. }
        )
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
        let waiting = self.world.pending_talks.len();
        if !self.start_conversation(initiator, target, dialog) {
            return refused("the conversation could not be read, or the owner has none");
        }
        if self.world.pending_talks.len() > waiting
            && let Some((_, _, d)) = self.world.pending_talks.last()
        {
            return CommandOutcome::ConversationQueued {
                dialog: d.name.clone(),
            };
        }
        let opened = !was_open && self.talk.as_ref().is_some_and(|c| !c.ended);
        let name = if opened {
            self.talk.as_ref().map(|c| c.dlg.name.clone())
        } else {
            self.ambient
                .iter()
                .find(|a| a.c.owner == target)
                .map(|a| a.c.dlg.name.clone())
        }
        .unwrap_or_else(|| dialog.to_string());
        CommandOutcome::ConversationStarted {
            dialog: name,
            opened,
        }
    }

    /// How an object can be found again in another session.
    fn object_ref(&self, id: ObjectId) -> Option<ObjectRef> {
        let o = self.world.get(id)?;
        Some(ObjectRef {
            id,
            tag: o.tag.clone(),
            area: o
                .area
                .and_then(|a| self.world.get(a))
                .map(|a| a.tag.clone()),
            template: o.template.clone(),
        })
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
        let e = &s.world.command_journal[1];
        assert_eq!(e.source, CommandSource::ModMode);
        // The objects are kept as they can be found again.
        assert_eq!(
            e.objects.iter().map(|o| o.tag.as_str()).collect::<Vec<_>>(),
            ["player", "gone"]
        );
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
