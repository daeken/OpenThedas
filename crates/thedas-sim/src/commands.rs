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
    /// Sets one of an object's variables (`value`), or removes it (`None`). The module
    /// object's variables are the game's globals.
    SetLocal {
        target: ObjectId,
        name: String,
        value: Option<LocalValue>,
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
}

impl SimCommand {
    /// Whether the world's journal keeps it: edits (plot flags, conversations started from
    /// outside), not the stream of play inputs, which goes to [`Sim::input_log`].
    pub fn is_edit(&self) -> bool {
        matches!(
            self,
            SimCommand::SetPlotFlag { .. }
                | SimCommand::StartConversation { .. }
                | SimCommand::SetLocal { .. }
        )
    }
}

/// A variable's value, of one of the types an object keeps
/// ([`crate::world::Locals`]; locations are not set this way).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum LocalValue {
    Int(i32),
    Float(f32),
    String(String),
    Object(ObjectId),
    Resource(String),
}

impl LocalValue {
    /// The variable's type, as the panels name it.
    pub fn kind(&self) -> &'static str {
        match self {
            LocalValue::Int(_) => "int",
            LocalValue::Float(_) => "float",
            LocalValue::String(_) => "string",
            LocalValue::Object(_) => "object",
            LocalValue::Resource(_) => "resource",
        }
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
    /// The variable was set (or removed). `replaced` is what it held, of the same type
    /// (`None`: it was not there): setting it back undoes it.
    LocalSet { replaced: Option<LocalValue> },
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
            SimCommand::SetLocal {
                target,
                name,
                value,
            } => self.set_commanded_local(*target, name, value.as_ref()),
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
            SimCommand::SetLocal { target, .. } => self.object_ref(*target).into_iter().collect(),
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

    /// [`SimCommand::SetLocal`]: the variable of `value`'s type set, or (`None`) removed.
    fn set_commanded_local(
        &mut self,
        target: ObjectId,
        name: &str,
        value: Option<&LocalValue>,
    ) -> CommandOutcome {
        let Some(o) = self.world.get_mut(target) else {
            return CommandOutcome::Refused {
                reason: "no such object".into(),
            };
        };
        let l = &mut o.locals;
        let name = name.to_string();
        let replaced = match value {
            Some(LocalValue::Int(v)) => l.ints.insert(name, *v).map(LocalValue::Int),
            Some(LocalValue::Float(v)) => l.floats.insert(name, *v).map(LocalValue::Float),
            Some(LocalValue::String(v)) => {
                l.strings.insert(name, v.clone()).map(LocalValue::String)
            }
            Some(LocalValue::Object(v)) => l.objects.insert(name, *v).map(LocalValue::Object),
            Some(LocalValue::Resource(v)) => l
                .resources
                .insert(name, v.clone())
                .map(LocalValue::Resource),
            // Removed: whichever type holds it (the first, should two types share a name).
            None => l
                .ints
                .remove(&name)
                .map(LocalValue::Int)
                .or_else(|| l.floats.remove(&name).map(LocalValue::Float))
                .or_else(|| l.strings.remove(&name).map(LocalValue::String))
                .or_else(|| l.objects.remove(&name).map(LocalValue::Object))
                .or_else(|| l.resources.remove(&name).map(LocalValue::Resource)),
        };
        CommandOutcome::LocalSet { replaced }
    }

    /// Carries out a play input; what the method answered, where it answers.
    fn play_input(&mut self, c: &SimCommand) -> bool {
        use SimCommand as C;
        match c {
            C::SetPlotFlag { .. } | C::StartConversation { .. } | C::SetLocal { .. } => false,
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
    fn variables_are_set_journalled_and_undone() {
        let mut s = sim();
        let m = s.world.spawn(ObjectKind::Module, "module");
        s.world
            .get_mut(m)
            .unwrap()
            .locals
            .ints
            .insert("COUNT".into(), 3);
        let set = |s: &mut Sim, value| {
            s.apply(
                CommandSource::ModMode,
                SimCommand::SetLocal {
                    target: m,
                    name: "COUNT".into(),
                    value,
                },
            )
        };
        let out = set(&mut s, Some(LocalValue::Int(7)));
        assert_eq!(
            out,
            CommandOutcome::LocalSet {
                replaced: Some(LocalValue::Int(3))
            }
        );
        assert_eq!(s.world.get(m).unwrap().locals.ints["COUNT"], 7);
        let e = s.world.command_journal.last().unwrap();
        assert_eq!(e.objects.len(), 1);
        assert_eq!(e.objects[0].tag, "module");
        // Undo: set back what it replaced.
        set(&mut s, Some(LocalValue::Int(3)));
        assert_eq!(s.world.get(m).unwrap().locals.ints["COUNT"], 3);
        // A new variable replaces nothing; removing it gives back its value.
        let out = s.apply(
            CommandSource::ModMode,
            SimCommand::SetLocal {
                target: m,
                name: "NEW".into(),
                value: Some(LocalValue::String("x".into())),
            },
        );
        assert_eq!(out, CommandOutcome::LocalSet { replaced: None });
        let out = s.apply(
            CommandSource::ModMode,
            SimCommand::SetLocal {
                target: m,
                name: "NEW".into(),
                value: None,
            },
        );
        assert_eq!(
            out,
            CommandOutcome::LocalSet {
                replaced: Some(LocalValue::String("x".into()))
            }
        );
        assert!(s.world.get(m).unwrap().locals.strings.is_empty());
        assert_eq!(s.world.command_journal.len(), 4);
        let out = s.apply(
            CommandSource::ModMode,
            SimCommand::SetLocal {
                target: 999_999,
                name: "X".into(),
                value: None,
            },
        );
        assert_eq!(
            out,
            CommandOutcome::Refused {
                reason: "no such object".into()
            }
        );
    }

    #[test]
    fn local_values_serialize_with_their_type() {
        let v = LocalValue::Float(1.5);
        let j = serde_json::to_string(&v).unwrap();
        assert_eq!(j, r#"{"type":"float","value":1.5}"#);
        assert_eq!(serde_json::from_str::<LocalValue>(&j).unwrap(), v);
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
