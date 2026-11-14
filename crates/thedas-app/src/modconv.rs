//! Mod mode's conversation launcher: any conversation in the game started out of order, on
//! whoever owns it here (or on the inspected object), with every line's condition laid out
//! (the plot flags it reads, whether it could be used now) and those flags set or cleared
//! from the list. Everything goes through the simulation's commands ([`Sim::apply`] with
//! [`CommandSource::ModMode`]), so it is kept in the world's command journal, which is
//! also this panel's history and what its undo sets back.
//!
//! Nothing here runs per frame: the panel asks for work through [`ConvAction`]s, which
//! the app carries out with [`carry_out`] when a button is pressed.

use thedas_script::vm::{OBJECT_INVALID, ObjectId};
use thedas_sim::Sim;
use thedas_sim::commands::{CommandOutcome, CommandSource, LocalValue, SimCommand};
use thedas_sim::dialog::NodeCondition;
use thedas_sim::world::ObjectKind;

/// The most conversations the list shows at once (the filter narrows it).
const LIST_MAX: usize = 400;

/// What the conversation panel asks the app to do.
#[derive(Clone, Debug, PartialEq)]
pub enum ConvAction {
    /// Fill the list of conversations from the game's files.
    List,
    /// Pick a conversation: find who owns it here and lay out its conditions.
    Choose(String),
    /// Lay the picked conversation's conditions out again, as owner `owner` would have it.
    Refresh,
    /// Start the picked conversation between the leader and the chosen owner.
    Start,
    /// Start it at this line instead (out of order: its condition not asked).
    StartAt(usize),
    /// Offer every creature here as its speaker, not only those whose conversation it is.
    AnySpeaker,
    /// Set (or clear) a plot flag.
    SetFlag {
        plot: String,
        flag: i32,
        value: bool,
    },
    /// Undo history entry `n` (an index into [`Convs::history`]).
    Undo(usize),
}

/// One of mod mode's commands as the history shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct Edit {
    pub text: String,
    /// The commands that undo it, in order (flags and variables set back to what they
    /// held); empty when it cannot be undone (a conversation started, a defined flag, a
    /// command refused).
    pub undo: Vec<SimCommand>,
}

/// Which of the conversation's lines the panel lists.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Show {
    /// Its starts, and the lines whose condition reads a flag.
    #[default]
    Conditional,
    Starts,
    All,
}

impl Show {
    fn keeps(self, l: &NodeCondition) -> bool {
        match self {
            Show::Conditional => l.start || !l.reads.is_empty() || !l.sets.is_empty(),
            Show::Starts => l.start,
            Show::All => true,
        }
    }
}

/// The conversation panel's state.
#[derive(Default)]
pub struct Convs {
    pub open: bool,
    /// Every conversation's resource name (without `.dlg`), once listed.
    pub names: Option<Vec<String>>,
    filter: String,
    /// The conversation picked.
    pub dialog: Option<String>,
    /// Who could own it: the objects here whose conversation it is, then the inspected
    /// object, with their tags.
    pub owners: Vec<(ObjectId, String)>,
    pub owner: Option<ObjectId>,
    /// Its lines' conditions as they stand, when laid out.
    pub lines: Option<Vec<NodeCondition>>,
    /// Flags set from the panel run their plot's script, as a script's `WR_SetPlotFlag`
    /// with its last argument true does.
    pub call_script: bool,
    /// Which lines the list shows.
    show: Show,
    /// What the last command did.
    pub status: String,
    /// Mod mode's commands this session, oldest first.
    pub history: Vec<Edit>,
}

impl Convs {
    /// The panel, when open.
    pub fn window(&mut self, ctx: &egui::Context, actions: &mut Vec<ConvAction>) {
        if !self.open {
            return;
        }
        if self.names.is_none() && !actions.contains(&ConvAction::List) {
            actions.push(ConvAction::List);
        }
        let mut open = self.open;
        egui::Window::new("Conversations")
            .open(&mut open)
            .default_pos([420.0, 16.0])
            .default_width(560.0)
            .default_height(640.0)
            .show(ctx, |ui| self.contents(ui, actions));
        self.open = open;
    }

    fn contents(&mut self, ui: &mut egui::Ui, actions: &mut Vec<ConvAction>) {
        ui.horizontal(|ui| {
            ui.label("Find");
            ui.text_edit_singleline(&mut self.filter);
        });
        let needle = self.filter.to_ascii_lowercase();
        let shown: Vec<&String> = self
            .names
            .iter()
            .flatten()
            .filter(|n| needle.is_empty() || n.contains(&needle))
            .take(LIST_MAX)
            .collect();
        egui::ScrollArea::vertical()
            .id_salt("dialogs")
            .max_height(140.0)
            .show(ui, |ui| {
                for n in &shown {
                    let picked = self.dialog.as_deref() == Some(n.as_str());
                    if ui.selectable_label(picked, n.as_str()).clicked() && !picked {
                        actions.push(ConvAction::Choose((*n).clone()));
                    }
                }
                if shown.len() == LIST_MAX {
                    ui.weak("… type to narrow the list");
                }
            });
        ui.separator();
        let Some(dialog) = self.dialog.clone() else {
            ui.label("Pick a conversation.");
            self.history_list(ui, actions);
            return;
        };
        ui.horizontal(|ui| {
            ui.strong(&dialog);
            ui.label("owner");
            let label = self
                .owners
                .iter()
                .find(|(id, _)| Some(*id) == self.owner)
                .map_or("nobody".to_string(), |(id, t)| format!("{t} ({id})"));
            egui::ComboBox::from_id_salt("owner")
                .selected_text(label)
                .show_ui(ui, |ui| {
                    for (id, tag) in &self.owners {
                        if ui
                            .selectable_label(self.owner == Some(*id), format!("{tag} ({id})"))
                            .clicked()
                        {
                            self.owner = Some(*id);
                            actions.push(ConvAction::Refresh);
                        }
                    }
                    if ui
                        .selectable_label(self.owner.is_none(), "nobody")
                        .clicked()
                    {
                        self.owner = None;
                        actions.push(ConvAction::Refresh);
                    }
                });
        });
        ui.horizontal(|ui| {
            if ui
                .button("Start")
                .on_hover_text("The leader talks to the owner, with this conversation")
                .clicked()
            {
                actions.push(ConvAction::Start);
            }
            if ui
                .button("Any speaker")
                .on_hover_text("Offer every creature here as the one speaking")
                .clicked()
            {
                actions.push(ConvAction::AnySpeaker);
            }
            if ui.button("Check again").clicked() {
                actions.push(ConvAction::Refresh);
            }
            ui.checkbox(&mut self.call_script, "flags run their plot script");
        });
        ui.horizontal(|ui| {
            ui.label("show");
            ui.selectable_value(&mut self.show, Show::Conditional, "starts and conditions");
            ui.selectable_value(&mut self.show, Show::Starts, "starts");
            ui.selectable_value(&mut self.show, Show::All, "every line");
        });
        if !self.status.is_empty() {
            ui.label(&self.status);
        }
        ui.separator();
        match &self.lines {
            None => {
                ui.label("The conversation could not be read.");
            }
            Some(lines) => {
                let mut flips = Vec::new();
                egui::ScrollArea::vertical()
                    .id_salt("lines")
                    .max_height(320.0)
                    .show(ui, |ui| {
                        let shown = lines.iter().filter(|l| self.show.keeps(l));
                        for l in shown {
                            line_row(ui, l, &mut flips);
                        }
                    });
                actions.extend(flips);
            }
        }
        self.history_list(ui, actions);
    }

    fn history_list(&self, ui: &mut egui::Ui, actions: &mut Vec<ConvAction>) {
        if self.history.is_empty() {
            return;
        }
        ui.separator();
        ui.label("History (this session)");
        egui::ScrollArea::vertical()
            .id_salt("history")
            .max_height(120.0)
            .stick_to_bottom(true)
            .show(ui, |ui| {
                for (n, e) in self.history.iter().enumerate() {
                    ui.horizontal(|ui| {
                        if !e.undo.is_empty() && ui.small_button("Undo").clicked() {
                            actions.push(ConvAction::Undo(n));
                        }
                        ui.label(&e.text);
                    });
                }
            });
    }
}

/// One line of the conversation: what kind it is, whether it could be used, its text, and
/// the flags its condition reads, each with a button that flips it.
fn line_row(ui: &mut egui::Ui, l: &NodeCondition, flips: &mut Vec<ConvAction>) {
    let kind = match (l.start, l.reply) {
        (true, _) => "start",
        (_, true) => "reply",
        _ => "line",
    };
    let mark = if l.usable { "+" } else { "-" };
    let mut text = l.text.replace('\n', " ");
    if text.chars().count() > 90 {
        text = text.chars().take(90).collect::<String>() + "…";
    }
    if text.is_empty() {
        text = "(no text)".into();
    }
    let head = egui::RichText::new(format!("{mark} {kind} {}: {text}", l.node));
    ui.horizontal(|ui| {
        ui.label(if l.usable { head } else { head.weak() });
        if !l.reply
            && ui
                .small_button("Start here")
                .on_hover_text("Begin the conversation at this line, whatever its condition")
                .clicked()
        {
            flips.push(ConvAction::StartAt(l.node));
        }
    });
    for r in &l.reads {
        ui.horizontal(|ui| {
            ui.add_space(24.0);
            let name = r.name.clone().unwrap_or_else(|| format!("flag {}", r.flag));
            let plot = r.resref.clone().unwrap_or_else(|| r.plot.clone());
            ui.monospace(format!("{plot}: {name} = {}", r.value));
            if r.defined {
                ui.weak("(defined: its plot script answers)");
            } else if ui
                .small_button(if r.value { "clear" } else { "set" })
                .clicked()
            {
                flips.push(ConvAction::SetFlag {
                    plot: r.plot.clone(),
                    flag: r.flag,
                    value: !r.value,
                });
            }
        });
    }
    for s in &l.sets {
        ui.horizontal(|ui| {
            ui.add_space(24.0);
            ui.weak(format!(
                "its condition would set {} flag {} = {}",
                s.plot, s.flag, s.value
            ));
        });
    }
}

/// Carries out what the panel asked for, on the simulation. `selected` is the inspected
/// object, offered as an owner.
pub fn carry_out(sim: &mut Sim, convs: &mut Convs, a: ConvAction, selected: Option<ObjectId>) {
    match a {
        ConvAction::List => convs.names = Some(dialog_names(sim)),
        ConvAction::Choose(dialog) => {
            convs.owners = owners(sim, &dialog, selected);
            convs.owner = convs.owners.first().map(|o| o.0);
            convs.dialog = Some(dialog);
            convs.status.clear();
            refresh(sim, convs);
        }
        ConvAction::Refresh => refresh(sim, convs),
        ConvAction::AnySpeaker => {
            let any = every_creature(sim);
            for o in any {
                if !convs.owners.iter().any(|x| x.0 == o.0) {
                    convs.owners.push(o);
                }
            }
            convs.status = format!("{} possible speakers", convs.owners.len());
        }
        ConvAction::Start | ConvAction::StartAt(_) => {
            let start = match a {
                ConvAction::StartAt(n) => Some(n),
                _ => None,
            };
            let Some(dialog) = convs.dialog.clone() else {
                return;
            };
            let Some(initiator) = sim.world.controlled().or(sim.world.hero) else {
                convs.status = "No leader to start it.".into();
                return;
            };
            let target = convs.owner.unwrap_or(OBJECT_INVALID);
            let outcome = sim.apply(
                CommandSource::ModMode,
                SimCommand::StartConversation {
                    initiator,
                    target,
                    dialog: dialog.clone(),
                    start,
                },
            );
            convs.status = describe(&outcome);
            let at = start.map_or(String::new(), |n| format!(" at line {n}"));
            convs.history.push(Edit {
                text: format!("start {dialog}{at}: {}", convs.status),
                undo: Vec::new(),
            });
        }
        ConvAction::SetFlag { plot, flag, value } => {
            let edit = set_flag(sim, &plot, flag, value, convs.call_script);
            convs.status = edit.text.clone();
            convs.history.push(edit);
            refresh(sim, convs);
        }
        ConvAction::Undo(n) => {
            let Some(e) = convs.history.get_mut(n) else {
                return;
            };
            let undo = std::mem::take(&mut e.undo);
            let what = e.text.clone();
            for c in undo {
                sim.apply(CommandSource::ModMode, c);
            }
            convs.status = format!("undone: {what}");
            convs.history.push(Edit {
                text: convs.status.clone(),
                undo: Vec::new(),
            });
            refresh(sim, convs);
        }
    }
}

/// The picked conversation's conditions, as its owner would have them now.
fn refresh(sim: &mut Sim, convs: &mut Convs) {
    if let Some(dialog) = convs.dialog.as_deref() {
        convs.lines = sim.conversation_conditions(convs.owner.unwrap_or(OBJECT_INVALID), dialog);
    }
}

/// Sets a flag as mod mode's command, and what the history shows of it.
pub fn set_flag(sim: &mut Sim, plot: &str, flag: i32, value: bool, call_script: bool) -> Edit {
    let outcome = sim.apply(
        CommandSource::ModMode,
        SimCommand::SetPlotFlag {
            plot: plot.to_string(),
            flag,
            value,
            call_script,
        },
    );
    let undo = match &outcome {
        CommandOutcome::PlotFlagSet { changed, .. } => changed
            .iter()
            .rev()
            .filter_map(|c| {
                // Set back as it was stored, without running the plot script again.
                c.replaced.map(|r| SimCommand::SetPlotFlag {
                    plot: c.plot.clone(),
                    flag: c.flag,
                    value: r,
                    call_script: false,
                })
            })
            .collect(),
        _ => Vec::new(),
    };
    let name = sim
        .plot_info(plot)
        .map(|p| {
            let f = p
                .flags
                .get(&flag)
                .cloned()
                .unwrap_or_else(|| flag.to_string());
            format!("{}: {f}", p.resref)
        })
        .unwrap_or_else(|| format!("{plot}: {flag}"));
    Edit {
        text: format!(
            "{} {name}: {}",
            if value { "set" } else { "clear" },
            describe(&outcome)
        ),
        undo,
    }
}

/// A variable's value as the panels write it.
pub fn show_value(v: &LocalValue) -> String {
    match v {
        LocalValue::Int(x) => x.to_string(),
        LocalValue::Float(x) => format!("{x}"),
        LocalValue::String(x) => format!("{x:?}"),
        LocalValue::Object(x) => format!("object {x}"),
        LocalValue::Resource(x) => x.clone(),
    }
}

/// Sets (or, with `None`, removes) one of `target`'s variables as mod mode's command, and
/// what the history shows of it.
pub fn set_local(sim: &mut Sim, target: ObjectId, name: &str, value: Option<LocalValue>) -> Edit {
    let tag = sim
        .world
        .get(target)
        .map_or_else(|| target.to_string(), |o| o.tag.clone());
    let what = match &value {
        Some(v) => format!("{tag}.{name} = {}", show_value(v)),
        None => format!("remove {tag}.{name}"),
    };
    let outcome = sim.apply(
        CommandSource::ModMode,
        SimCommand::SetLocal {
            target,
            name: name.to_string(),
            value,
        },
    );
    let undo = match &outcome {
        CommandOutcome::LocalSet { replaced } => vec![SimCommand::SetLocal {
            target,
            name: name.to_string(),
            value: replaced.clone(),
        }],
        _ => Vec::new(),
    };
    Edit {
        text: format!("{what}: {}", describe(&outcome)),
        undo,
    }
}

/// Moves, makes or removes an object as mod mode's command (`what` says which, for the
/// history), with what undoes it: a move moves it back, a made object is removed; a
/// removal cannot be undone.
pub fn object_edit(sim: &mut Sim, command: SimCommand, what: String) -> Edit {
    let target = match &command {
        SimCommand::MoveObject { target, .. } => Some(*target),
        _ => None,
    };
    let outcome = sim.apply(CommandSource::ModMode, command);
    let undo = match (&outcome, target) {
        (CommandOutcome::ObjectMoved { from, from_facing }, Some(target)) => {
            vec![SimCommand::MoveObject {
                target,
                position: *from,
                facing: *from_facing,
            }]
        }
        (CommandOutcome::ObjectCreated { id }, _) => {
            vec![SimCommand::DestroyObject { target: *id }]
        }
        _ => Vec::new(),
    };
    Edit {
        text: format!("{what}: {}", describe(&outcome)),
        undo,
    }
}

/// A command's outcome in a few words.
pub fn describe(o: &CommandOutcome) -> String {
    match o {
        CommandOutcome::PlotFlagSet { changed, .. } => {
            // A flag set to what it already held is in `changed` too; it changed nothing.
            let n = changed
                .iter()
                .filter(|c| c.replaced != Some(c.value))
                .count();
            match n {
                0 => "nothing changed".into(),
                1 => "1 flag changed".into(),
                n => format!("{n} flags changed (its script set more)"),
            }
        }
        CommandOutcome::ConversationStarted { dialog, opened } => {
            if *opened {
                format!("{dialog} opened")
            } else {
                format!("{dialog} said in passing")
            }
        }
        CommandOutcome::ConversationQueued { dialog } => {
            format!("{dialog} starts when the open conversation ends")
        }
        CommandOutcome::LocalSet { replaced } => match replaced {
            Some(v) => format!("was {}", show_value(v)),
            None => "was not set".into(),
        },
        CommandOutcome::ObjectMoved { from, .. } => {
            format!("moved from {:.2}, {:.2}, {:.2}", from[0], from[1], from[2])
        }
        CommandOutcome::ObjectCreated { id } => format!("made object {id}"),
        CommandOutcome::ObjectDestroyed => "removed".into(),
        CommandOutcome::Done { ok } => if *ok { "done" } else { "did nothing" }.into(),
        CommandOutcome::Refused { reason } => format!("refused: {reason}"),
    }
}

/// Every conversation in the game's files, by resource name without `.dlg`, sorted.
fn dialog_names(sim: &Sim) -> Vec<String> {
    let mut v: Vec<String> = sim
        .vfs
        .names()
        .into_iter()
        .filter_map(|n| {
            let l = n.to_ascii_lowercase();
            l.strip_suffix(".dlg").map(str::to_string)
        })
        .collect();
    v.sort();
    v.dedup();
    v
}

/// Who could own conversation `dialog` here: the creatures and placeables in the current
/// area whose own conversation it is (living ones first), then `selected`.
/// Every active, living creature in the current area but the hero, by tag.
fn every_creature(sim: &Sim) -> Vec<(ObjectId, String)> {
    let w = &sim.world;
    let mut v: Vec<(ObjectId, String)> = w
        .objects
        .values()
        .filter(|o| {
            o.area.is_some()
                && o.area == w.area
                && o.kind == ObjectKind::Creature
                && o.active
                && !o.dead
                && Some(o.id) != w.hero
        })
        .map(|o| (o.id, o.tag.clone()))
        .collect();
    v.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));
    v
}

fn owners(sim: &Sim, dialog: &str, selected: Option<ObjectId>) -> Vec<(ObjectId, String)> {
    let w = &sim.world;
    let mut v: Vec<(bool, ObjectId, String)> = w
        .objects
        .values()
        .filter(|o| {
            o.area.is_some()
                && o.area == w.area
                && matches!(o.kind, ObjectKind::Creature | ObjectKind::Placeable)
                && o.dialog
                    .as_deref()
                    .is_some_and(|d| d.trim_end_matches(".dlg").eq_ignore_ascii_case(dialog))
        })
        .map(|o| (!o.active || o.dead, o.id, o.tag.clone()))
        .collect();
    v.sort();
    let mut out: Vec<(ObjectId, String)> = v.into_iter().map(|(_, id, t)| (id, t)).collect();
    if let Some(s) = selected
        && !out.iter().any(|o| o.0 == s)
        && let Some(o) = w.get(s)
    {
        out.push((s, o.tag.clone()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use thedas_sim::plot::FlagChange;

    fn line(start: bool, reads: usize) -> NodeCondition {
        NodeCondition {
            node: 0,
            start,
            reply: false,
            text: String::new(),
            usable: true,
            reads: (0..reads)
                .map(|i| thedas_sim::plot::FlagRead {
                    plot: "G".into(),
                    resref: None,
                    flag: i as i32,
                    name: None,
                    value: false,
                    defined: false,
                })
                .collect(),
            sets: Vec::new(),
        }
    }

    #[test]
    fn the_list_keeps_starts_and_conditioned_lines() {
        let (start, plain, gated) = (line(true, 0), line(false, 0), line(false, 2));
        assert!(Show::Conditional.keeps(&start));
        assert!(!Show::Conditional.keeps(&plain));
        assert!(Show::Conditional.keeps(&gated));
        assert!(!Show::Starts.keeps(&gated));
        assert!(Show::All.keeps(&plain));
    }

    #[test]
    fn outcomes_read_plainly() {
        let set = CommandOutcome::PlotFlagSet {
            plot: "G".into(),
            replaced: Some(false),
            changed: vec![
                FlagChange {
                    plot: "G".into(),
                    flag: 3,
                    value: true,
                    replaced: Some(false),
                },
                FlagChange {
                    plot: "H".into(),
                    flag: 1,
                    value: true,
                    replaced: None,
                },
            ],
        };
        assert_eq!(describe(&set), "2 flags changed (its script set more)");
        let again = CommandOutcome::PlotFlagSet {
            plot: "G".into(),
            replaced: Some(true),
            changed: vec![FlagChange {
                plot: "G".into(),
                flag: 3,
                value: true,
                replaced: Some(true),
            }],
        };
        assert_eq!(describe(&again), "nothing changed");
        assert_eq!(
            describe(&CommandOutcome::ConversationQueued {
                dialog: "alistair_main".into()
            }),
            "alistair_main starts when the open conversation ends"
        );
        assert_eq!(
            describe(&CommandOutcome::Refused {
                reason: "no plot x".into()
            }),
            "refused: no plot x"
        );
    }
}
