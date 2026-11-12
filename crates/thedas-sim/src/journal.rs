//! The journal: quests and the codex, as journal.gfx shows them.
//!
//! The executable keeps the journal as lists of entries it describes with named attributes
//! (`Journal.PlotList`, `CompletedPlots`, `PlotGroups`, `CodexGroups`, `ActivePlot`,
//! `CodexUpdated`; an entry's `Updated`, `PlotGroup`, `Completed`, `ParentPlot`; a group's
//! `Priority`, `ExpandedInCurrentQuests`, `ExpandedInCompletedQuests`; a codex group's
//! `CodexList`, an entry's `Descriptions` and `Locked`: UTF-16 names at 0x713d52 to 0x714aa0
//! in DAOrigins.exe). What goes in them is built here from the plots:
//!
//! - A quest is a plot of type 1 (or 12, a Chanter's Board job) under a parent (a quest
//!   group, type 14, or another quest: a sub-quest) once one of its flags with journal text
//!   (a string that is not empty) is
//!   set by a script (a flag the plot file starts set does not count). It shows the text of
//!   the last such flag set, and is completed once one of its ending flags is set.
//! - A codex group is a `plottypes.gda` row marked as codex (`IsCodex`); its entries are the
//!   plots of that type, locked until one of their flags is set, showing the text of every
//!   flag set.
//!
//! Provisional: the executable's plot manager, which decides what the journal holds, is not
//! traced; the rules above are read from the plot files and journal.gfx.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::plot::{DEFINED_FLAGS, PlotInfo};
use crate::sim::Sim;

/// The quest the player marked as active (its plot resource).
pub const MSG_SET_ACTIVE: &str = "Journal.SetActiveQuest";
/// The player read a codex entry (its plot resource), or all of them.
pub const MSG_MARK_CODEX_READ: &str = "Journal.MarkCodexAsRead";
pub const MSG_MARK_ALL_CODEX_READ: &str = "Journal.MarkAllCodexAsRead";
/// A movie wrote one of the journal's attributes (the attribute and the value): journal.gfx
/// clears a quest's `Journal.PlotList.N.Updated` (or `CompletedPlots`) when the player reads
/// it, and for every quest with "Mark All as Read". The interface passes these on.
pub const MSG_SET_ATTRIBUTE: &str = "Journal.SetAttribute";
/// The HUD's notification for a kind of alert was dismissed or faded (the alert type).
pub const MSG_HIDE_ALERT: &str = "Party.HideAlert";
pub const MSG_HIDE_UBER_ALERT: &str = "Party.HideUberAlert";

/// notifications.gfx's alert types (`ALERT_QUEST`, `ALERT_CODEX`).
pub const ALERT_QUEST: i32 = 1;
pub const ALERT_CODEX: i32 = 2;

/// Quests, Chanter's Board jobs and quest groups (`plottypes.gda` rows).
const KIND_QUEST: i32 = 1;
const KIND_CHANTERS: i32 = 12;
const KIND_GROUP: i32 = 14;
/// `plottypes.gda`'s M2DA id (journal.gfx's `TABLE_PLOT_TYPES`) and its columns: the name
/// string and whether the type is a codex category.
const TABLE_PLOT_TYPES: i32 = 108;
const HASH_TYPE_NAME: u32 = 0xa422_7c06;
const HASH_IS_CODEX: u32 = 0xeb84_ee4d;

/// popuplayer.gfx's tutorial buttons: Next (`0`) takes the one shown off the queue, Close
/// (`1`) all of them (DAOrigins.exe's TutorialControl handler, 0x693630, loops while the
/// message's flag is set).
pub const MSG_CONSUME_TUTORIAL: &str = "Tutorials.ConsumeTutorial";

/// `plottypes.gda`'s column saying a type is a tutorial (13 controls, 15 plot), as
/// DAOrigins.exe reads it (0x75bcf0).
const HASH_IS_TUTORIAL: u32 = 0x12e4_743c;

/// A tutorial popup waiting to show (`Tutorials.TutorialList.N`): its title (the plot's
/// name), its text (the flag's journal text) and the codex entry its Codex button opens
/// (the plot's resource name, field 3). DAOrigins.exe's TutorialControl builds one
/// (0x693390) when a flag of a tutorial plot is set (0x76014d, 0x693f10).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Tutorial {
    pub resref: String,
    pub title: Option<u32>,
    pub text: Option<u32>,
    pub pauses: bool,
}

/// A quest's latest journal entry: the flag whose text it shows and when it was set (a
/// running count).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct JournalMark {
    pub flag: i32,
    pub seq: u64,
    /// Not yet read in the journal (its `Updated`).
    #[serde(default)]
    pub unread: bool,
}

/// A HUD notification (notifications.gfx's `Party.Alerts.N`): a quest's new journal entry or
/// a codex entry unlocked, one per type.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Alert {
    /// [`ALERT_QUEST`] or [`ALERT_CODEX`].
    pub kind: i32,
    /// The plot resource (`QuestResRef`).
    pub resref: String,
    /// The plot's name (`QuestName`, a talk-table string).
    pub name: Option<u32>,
    /// The quest is done (`QuestCompleted`).
    pub completed: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct QuestView {
    /// The plot resource (`plt_...`): journal.gfx names quests by it.
    pub resref: String,
    pub name: Option<u32>,
    /// The text of its latest entry.
    pub description: Option<u32>,
    pub completed: bool,
    /// Its quest group's plot resource (the top quest's, for a sub-quest).
    pub group: String,
    /// The quest it belongs under, for a sub-quest.
    pub parent: Option<String>,
    pub seq: u64,
    /// A new entry not yet read (`Updated`).
    pub updated: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct QuestGroupView {
    pub resref: String,
    pub name: Option<u32>,
    pub priority: i32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CodexEntryView {
    pub resref: String,
    pub name: Option<u32>,
    /// The text of each flag set, in flag order (none: locked).
    pub texts: Vec<u32>,
    /// Unlocked or added to and not yet read (`Updated`).
    pub updated: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CodexGroupView {
    /// The plot type (`plottypes.gda` row).
    pub kind: i32,
    pub name: Option<u32>,
    pub entries: Vec<CodexEntryView>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct JournalView {
    /// Groups with a quest in them, by priority (highest first), then resource.
    pub groups: Vec<QuestGroupView>,
    /// Quests in progress, latest entry first.
    pub active: Vec<QuestView>,
    pub completed: Vec<QuestView>,
    pub codex: Vec<CodexGroupView>,
    pub active_quest: Option<String>,
    /// A codex entry is unread (`Journal.CodexUpdated`).
    pub codex_updated: bool,
}

impl Sim {
    /// Notes a quest's new journal entry when one of its flags with journal text is set
    /// (called as flags are set).
    /// Whether a plot type is a tutorial's.
    fn is_tutorial_kind(&self, kind: i32) -> bool {
        self.world
            .tables()
            .get(TABLE_PLOT_TYPES, "")
            .is_some_and(|t| {
                matches!(
                    t.cell_by_hash(HASH_IS_TUTORIAL, i64::from(kind)),
                    Some(thedas_formats::gda::Cell::Int(1))
                )
            })
    }

    /// Queues a tutorial plot's popup for a flag just set: one per plot at a time (a plot
    /// already queued is skipped, 0x694032), pausing the game when the plot asks (0x6940c0).
    /// Provisional: the game's Show Tutorials option is taken to be on, and a flag with no
    /// text queues nothing.
    pub(crate) fn queue_tutorial(&mut self, info: &PlotInfo, flag: i32) {
        if flag >= DEFINED_FLAGS || !self.is_tutorial_kind(info.kind) {
            return;
        }
        let Some(text) = info
            .journal
            .get(&flag)
            .copied()
            .filter(|&t| self.has_text(t))
        else {
            return;
        };
        let resref = info.resref.trim_start_matches("plt_").to_string();
        if self.world.tutorials.iter().any(|t| t.resref == resref) {
            return;
        }
        self.world.tutorials.push(Tutorial {
            resref,
            title: info.name_strref,
            text: Some(text),
            pauses: info.pauses,
        });
        if info.pauses {
            self.world.tutorial_paused = true;
        }
    }

    /// Takes the tutorial shown off the queue, or all of them; the game resumes once none is
    /// left if a tutorial paused it.
    pub fn consume_tutorial(&mut self, all: bool) {
        loop {
            if self.world.tutorials.is_empty() {
                break;
            }
            self.world.tutorials.remove(0);
            if !all {
                break;
            }
        }
        if self.world.tutorials.is_empty() {
            self.world.tutorial_paused = false;
        }
    }

    pub(crate) fn note_journal_flag(&mut self, info: &PlotInfo, flag: i32) {
        if flag >= DEFINED_FLAGS || !info.journal.get(&flag).is_some_and(|&t| self.has_text(t)) {
            return;
        }
        self.world.journal_seq += 1;
        let seq = self.world.journal_seq;
        self.world.journal.insert(
            info.guid.clone(),
            JournalMark {
                flag,
                seq,
                unread: true,
            },
        );
        // The HUD's notification: a codex entry, or a quest the journal lists. There is one
        // per type, naming the latest plot (notifications.gfx hides them by type,
        // `Party.HideAlert <type>`). Provisional: inferred from that message.
        let kind = if self.is_codex_kind(info.kind) {
            ALERT_CODEX
        } else if matches!(info.kind, KIND_QUEST | KIND_CHANTERS) && info.parent.is_some() {
            ALERT_QUEST
        } else {
            return;
        };
        let completed = kind == ALERT_QUEST && info.finals.iter().any(|&f| self.flag_on(info, f));
        self.world.alerts.retain(|a| a.kind != kind);
        self.world.alerts.push(Alert {
            kind,
            resref: info.resref.clone(),
            name: info.name_strref,
            completed,
        });
    }

    /// Whether a plot type is a codex category (`plottypes.gda`'s `IsCodex`).
    fn is_codex_kind(&self, kind: i32) -> bool {
        self.world
            .tables()
            .get(TABLE_PLOT_TYPES, "")
            .is_some_and(|t| {
                matches!(
                    t.cell_by_hash(HASH_IS_CODEX, i64::from(kind)),
                    Some(thedas_formats::gda::Cell::Int(1))
                )
            })
    }

    /// Marks a plot's journal entry read (its plot resource).
    pub fn mark_journal_read(&mut self, resref: &str) -> bool {
        let Some(info) = self.plot_info(resref) else {
            return false;
        };
        match self.world.journal.get_mut(&info.guid) {
            Some(m) => {
                m.unread = false;
                true
            }
            None => false,
        }
    }

    /// Whether a journal text says something: many flags name a string that is empty.
    fn has_text(&self, strref: u32) -> bool {
        self.string(strref).is_some_and(|t| !t.trim().is_empty())
    }

    /// Whether a script has set a plot's flag. A flag a plot file starts set makes no journal
    /// entry (a Chanter's Board posting starts set, with the board's text): the journal holds
    /// what happened. Provisional.
    fn flag_on(&self, info: &PlotInfo, flag: i32) -> bool {
        self.script_set_plot_flag(&info.guid, flag)
    }

    /// The journal as it stands.
    pub fn journal(&mut self) -> JournalView {
        let vfs = self.vfs.clone();
        let all = self.plots.all(&vfs);
        let by_res: BTreeMap<String, Arc<PlotInfo>> = all
            .iter()
            .map(|p| (p.resref.trim_start_matches("plt_").to_string(), p.clone()))
            .collect();
        let find = |r: &str| by_res.get(r.trim_start_matches("plt_")).cloned();
        let mut view = JournalView {
            active_quest: self.world.active_quest.clone(),
            ..Default::default()
        };
        let mut groups: BTreeMap<String, QuestGroupView> = BTreeMap::new();
        for p in all
            .iter()
            .filter(|p| matches!(p.kind, KIND_QUEST | KIND_CHANTERS))
        {
            let Some(parent) = p.parent.as_deref().and_then(find) else {
                continue;
            };
            // The group: up the parents until one is a quest group.
            let mut top = parent.clone();
            let mut depth = 0;
            while top.kind != KIND_GROUP && depth < 8 {
                match top.parent.as_deref().and_then(find) {
                    Some(up) => top = up,
                    None => break,
                }
                depth += 1;
            }
            if top.kind != KIND_GROUP {
                continue;
            }
            let set: Vec<i32> = p
                .journal
                .iter()
                .filter(|(f, t)| **f < DEFINED_FLAGS && self.flag_on(p, **f) && self.has_text(**t))
                .map(|(f, _)| *f)
                .collect();
            if set.is_empty() {
                continue;
            }
            let mark = self
                .world
                .journal
                .get(&p.guid)
                .filter(|m| set.contains(&m.flag));
            let flag = mark.map_or(*set.last().unwrap_or(&0), |m| m.flag);
            let quest = QuestView {
                resref: p.resref.clone(),
                name: p.name_strref,
                description: p.journal.get(&flag).copied(),
                completed: p.finals.iter().any(|&f| self.flag_on(p, f)),
                group: top.resref.clone(),
                parent: (parent.kind != KIND_GROUP).then(|| parent.resref.clone()),
                seq: mark.map_or(0, |m| m.seq),
                updated: mark.is_some_and(|m| m.unread),
            };
            groups.entry(top.resref.clone()).or_insert(QuestGroupView {
                resref: top.resref.clone(),
                name: top.name_strref,
                priority: top.priority,
            });
            if quest.completed {
                view.completed.push(quest);
            } else {
                view.active.push(quest);
            }
        }
        // A sub-quest's parent quest is listed too, as the heading it sits under, though it
        // has no entry of its own yet: journal.gfx places a sub-quest under its parent's
        // line and drops it (and an empty group) when the parent is not listed. The Dalish
        // opening's "Humans Near the Camp" sits under plt_bed000pt_main, which writes no
        // entry. Provisional: the exe's own listing rule is not traced.
        for completed in [false, true] {
            let mut i = 0;
            loop {
                let list = if completed {
                    &view.completed
                } else {
                    &view.active
                };
                let Some(q) = list.get(i) else { break };
                i += 1;
                let Some(parent) = q.parent.clone() else {
                    continue;
                };
                let listed = view
                    .active
                    .iter()
                    .chain(view.completed.iter())
                    .any(|x| x.resref == parent);
                if listed {
                    continue;
                }
                let Some(info) = find(&parent) else { continue };
                let grand = info
                    .parent
                    .as_deref()
                    .and_then(find)
                    .filter(|g| g.kind != KIND_GROUP)
                    .map(|g| g.resref.clone());
                let heading = QuestView {
                    resref: info.resref.clone(),
                    name: info.name_strref,
                    description: None,
                    completed,
                    group: q.group.clone(),
                    parent: grand,
                    seq: q.seq,
                    updated: false,
                };
                if completed {
                    view.completed.push(heading);
                } else {
                    view.active.push(heading);
                }
            }
        }
        for list in [&mut view.active, &mut view.completed] {
            list.sort_by(|a, b| b.seq.cmp(&a.seq).then(a.resref.cmp(&b.resref)));
        }
        view.groups = groups.into_values().collect();
        view.groups
            .sort_by(|a, b| b.priority.cmp(&a.priority).then(a.resref.cmp(&b.resref)));
        // The codex: each codex type with its plots.
        if let Some(types) = self.world.tables().get(TABLE_PLOT_TYPES, "") {
            for &kind in &types.order {
                let is_codex = matches!(
                    types.cell_by_hash(HASH_IS_CODEX, kind),
                    Some(thedas_formats::gda::Cell::Int(1))
                );
                if !is_codex {
                    continue;
                }
                let name = match types.cell_by_hash(HASH_TYPE_NAME, kind) {
                    Some(thedas_formats::gda::Cell::Int(v)) if *v > 0 => Some(*v as u32),
                    _ => None,
                };
                let entries: Vec<CodexEntryView> = all
                    .iter()
                    .filter(|p| i64::from(p.kind) == kind)
                    .map(|p| CodexEntryView {
                        resref: p.resref.clone(),
                        name: p.name_strref,
                        updated: self.world.journal.get(&p.guid).is_some_and(|m| m.unread),
                        texts: p
                            .journal
                            .iter()
                            .filter(|(f, t)| {
                                **f < DEFINED_FLAGS && self.flag_on(p, **f) && self.has_text(**t)
                            })
                            .map(|(_, t)| *t)
                            .collect(),
                    })
                    .collect();
                if !entries.is_empty() {
                    view.codex.push(CodexGroupView {
                        kind: kind as i32,
                        name,
                        entries,
                    });
                }
            }
        }
        view.codex_updated = view
            .codex
            .iter()
            .any(|g| g.entries.iter().any(|e| e.updated && !e.texts.is_empty()));
        view
    }

    /// journal.gfx's and notifications.gfx's messages: marking a quest active (its plot
    /// resource; map markers follow it in the original), reading quests (the movie clears
    /// their `Updated`) and codex entries, and dismissing the HUD's notifications.
    pub(crate) fn journal_message(&mut self, name: &str, args: &[String]) -> bool {
        match name {
            MSG_SET_ACTIVE => {
                let r = args.first().map(|a| a.trim()).unwrap_or_default();
                self.world.active_quest = (!r.is_empty()).then(|| r.to_string());
                true
            }
            MSG_MARK_CODEX_READ => {
                let r = args.first().map(|a| a.trim()).unwrap_or_default();
                self.mark_journal_read(r);
                true
            }
            MSG_MARK_ALL_CODEX_READ => {
                let codex: Vec<String> = self
                    .journal()
                    .codex
                    .into_iter()
                    .flat_map(|g| g.entries)
                    .filter(|e| e.updated)
                    .map(|e| e.resref)
                    .collect();
                for r in codex {
                    self.mark_journal_read(&r);
                }
                true
            }
            MSG_SET_ATTRIBUTE => {
                // `Journal.PlotList.N.Updated` (or `CompletedPlots`) set false: the quest at
                // that place in the list the movie was given is read. Other attributes the
                // movies write (selections, expanded groups) stay the interface's.
                let key = args.first().map(|a| a.trim()).unwrap_or_default();
                let cleared = matches!(args.get(1).map(|v| v.trim()), Some("0" | "false" | ""));
                let parts: Vec<&str> = key.split('.').collect();
                if let [
                    "Journal",
                    list @ ("PlotList" | "CompletedPlots"),
                    i,
                    "Updated",
                ] = parts.as_slice()
                {
                    if cleared {
                        let view = self.journal();
                        let quests = if *list == "PlotList" {
                            view.active
                        } else {
                            view.completed
                        };
                        if let Some(q) = i.parse::<usize>().ok().and_then(|i| quests.get(i)) {
                            let r = q.resref.clone();
                            self.mark_journal_read(&r);
                        }
                    }
                }
                true
            }
            MSG_HIDE_ALERT => {
                let kind = args.first().and_then(|a| a.trim().parse::<f64>().ok());
                match kind {
                    Some(k) => self.world.alerts.retain(|a| f64::from(a.kind) != k),
                    None => self.world.alerts.clear(),
                }
                true
            }
            MSG_HIDE_UBER_ALERT => true,
            MSG_CONSUME_TUTORIAL => {
                let all = args
                    .first()
                    .and_then(|a| a.trim().parse::<f64>().ok())
                    .is_some_and(|v| v != 0.0);
                self.consume_tutorial(all);
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use thedas_formats::gda::Cell;
    use thedas_script::EngineTable;
    use thedas_script::vm::OBJECT_INVALID;
    use thedas_vfs::Vfs;

    use super::*;

    fn plot(resref: &str, kind: i32, parent: Option<&str>, journal: &[(i32, u32)]) -> PlotInfo {
        PlotInfo {
            guid: resref.to_ascii_uppercase(),
            resref: format!("plt_{resref}"),
            name_strref: Some(100 + resref.len() as u32),
            journal: journal.iter().copied().collect(),
            kind,
            parent: parent.map(str::to_string),
            ..Default::default()
        }
    }

    #[test]
    fn tutorial_flags_queue_popups_that_next_and_close_take_off() {
        let mut s = Sim::new(Arc::new(Vfs::new()), Arc::new(EngineTable::default()), 1);
        s.plots
            .insert(plot("tut_explore", 13, None, &[(0, 10), (1, 11)]));
        let mut combat = plot("tut_combat_basic", 13, None, &[(0, 20)]);
        combat.pauses = true;
        s.plots.insert(combat);
        s.plots
            .insert(plot("lot100pt_bears", KIND_QUEST, None, &[(0, 30)]));
        s.world.tables().insert(
            "#108",
            &["#eb84ee4d", "#12e4743c"],
            vec![
                (1, vec![Cell::Int(0), Cell::Int(0)]),
                (13, vec![Cell::Int(1), Cell::Int(1)]),
            ],
        );
        let _ = s.strings.set(
            [10, 11, 20, 30]
                .into_iter()
                .map(|t| (t, format!("text {t}")))
                .collect(),
        );
        let o = OBJECT_INVALID;
        s.set_plot_flag("lot100pt_bears", 0, true, false, o);
        assert!(s.world.tutorials.is_empty(), "a quest is no tutorial");
        s.set_plot_flag("tut_explore", 0, true, false, o);
        // One per plot at a time; a flag already set queues nothing again.
        s.set_plot_flag("tut_explore", 1, true, false, o);
        s.set_plot_flag("tut_explore", 0, true, false, o);
        s.set_plot_flag("tut_combat_basic", 0, true, false, o);
        let queued: Vec<(&str, Option<u32>)> = s
            .world
            .tutorials
            .iter()
            .map(|t| (t.resref.as_str(), t.text))
            .collect();
        assert_eq!(
            queued,
            [("tut_explore", Some(10)), ("tut_combat_basic", Some(20))]
        );
        assert!(s.world.tutorial_paused);
        // Next takes the first off; the pause holds while any is left.
        assert!(s.journal_message(MSG_CONSUME_TUTORIAL, &["0".into()]));
        assert_eq!(s.world.tutorials.len(), 1);
        assert!(s.world.tutorial_paused);
        s.set_plot_flag("tut_explore", 1, false, false, o);
        s.set_plot_flag("tut_explore", 1, true, false, o);
        // Close takes them all off and the game resumes.
        assert!(s.journal_message(MSG_CONSUME_TUTORIAL, &["1".into()]));
        assert!(s.world.tutorials.is_empty());
        assert!(!s.world.tutorial_paused);
    }

    #[test]
    fn quests_show_their_latest_entry_under_their_group_and_the_codex_unlocks() {
        let mut s = Sim::new(Arc::new(Vfs::new()), Arc::new(EngineTable::default()), 1);
        let mut group = plot("lot_plot_group", KIND_GROUP, None, &[]);
        group.priority = 2;
        s.plots.insert(group);
        let mut bears = plot(
            "lot100pt_bears",
            KIND_QUEST,
            Some("lot_plot_group"),
            &[(0, 500), (1, 501), (2, 502), (3, 503), (300, 504)],
        );
        bears.finals.insert(2);
        s.plots.insert(bears);
        s.plots.insert(plot(
            "lot100pt_bears_sub",
            KIND_QUEST,
            Some("lot100pt_bears"),
            &[(0, 600)],
        ));
        // No parent: bookkeeping, not a quest.
        s.plots
            .insert(plot("lot000pt_ambient", KIND_QUEST, None, &[(0, 700)]));
        s.plots
            .insert(plot("cod_crt_ogre", 3, None, &[(0, 800), (1, 801)]));
        s.plots.insert(plot("cod_crt_wolf", 3, None, &[(0, 900)]));
        s.world.tables().insert(
            "#108",
            &["#a4227c06", "#eb84ee4d"],
            vec![
                (1, vec![Cell::Int(0), Cell::Int(0)]),
                (3, vec![Cell::Int(296632), Cell::Int(1)]),
            ],
        );
        let _ = s.strings.set(
            [500, 501, 502, 504, 600, 700, 800, 801, 900]
                .into_iter()
                .map(|t| (t, format!("entry {t}")))
                .chain([(503, String::new())])
                .collect(),
        );
        assert_eq!(s.journal().active, Vec::new());

        let o = OBJECT_INVALID;
        // A sub-quest's entry alone lists its parent too, as its heading (no entry of its own).
        s.set_plot_flag("lot100pt_bears_sub", 0, true, false, o);
        let j = s.journal();
        let names: Vec<(&str, Option<u32>, Option<&str>)> = j
            .active
            .iter()
            .map(|q| (q.resref.as_str(), q.description, q.parent.as_deref()))
            .collect();
        assert_eq!(
            names,
            [
                ("plt_lot100pt_bears", None, None),
                (
                    "plt_lot100pt_bears_sub",
                    Some(600),
                    Some("plt_lot100pt_bears")
                ),
            ]
        );
        assert_eq!(j.active[0].group, "plt_lot_plot_group");
        assert_eq!(j.groups.len(), 1);
        s.set_plot_flag("lot100pt_bears_sub", 0, false, false, o);
        s.set_plot_flag("lot100pt_bears", 1, true, false, o);
        s.set_plot_flag("lot100pt_bears", 0, true, false, o);
        s.set_plot_flag("lot100pt_bears_sub", 0, true, false, o);
        s.set_plot_flag("lot000pt_ambient", 0, true, false, o);
        s.set_plot_flag("cod_crt_ogre", 1, true, false, o);
        // A flag whose text is empty makes no entry, nor does a defined flag (never stored).
        s.set_plot_flag("lot100pt_bears", 3, true, false, o);
        s.set_plot_flag("lot100pt_bears", 300, true, false, o);
        let j = s.journal();
        assert_eq!(j.groups.len(), 1);
        assert_eq!(j.groups[0].resref, "plt_lot_plot_group");
        assert_eq!(j.groups[0].priority, 2);
        let names: Vec<&str> = j.active.iter().map(|q| q.resref.as_str()).collect();
        assert_eq!(names, ["plt_lot100pt_bears_sub", "plt_lot100pt_bears"]);
        // The entry set last, not the highest flag.
        assert_eq!(j.active[1].description, Some(500));
        assert_eq!(j.active[1].parent, None);
        assert_eq!(j.active[0].parent.as_deref(), Some("plt_lot100pt_bears"));
        assert_eq!(j.active[0].group, "plt_lot_plot_group");
        assert_eq!(j.codex.len(), 1);
        assert_eq!(j.codex[0].name, Some(296632));
        let ogre = j.codex[0]
            .entries
            .iter()
            .find(|e| e.resref == "plt_cod_crt_ogre");
        assert_eq!(ogre.map(|e| e.texts.clone()), Some(vec![801]));
        let wolf = j.codex[0]
            .entries
            .iter()
            .find(|e| e.resref == "plt_cod_crt_wolf");
        assert_eq!(wolf.map(|e| e.texts.is_empty()), Some(true), "locked");

        // An ending flag completes it.
        s.set_plot_flag("lot100pt_bears", 2, true, false, o);
        let j = s.journal();
        assert_eq!(j.completed.len(), 1);
        assert_eq!(j.completed[0].description, Some(502));
        assert!(s.journal_message(MSG_SET_ACTIVE, &["plt_lot100pt_bears_sub".into()]));
        assert_eq!(
            s.journal().active_quest.as_deref(),
            Some("plt_lot100pt_bears_sub")
        );

        // New entries are unread and raise the HUD's notifications, one per type naming the
        // latest plot: the quests' ("bears", now completed) and the codex entry's, not the
        // bookkeeping plot's.
        let j = s.journal();
        assert!(j.completed[0].updated && j.active[0].updated);
        assert!(j.codex_updated);
        let alerts: Vec<(i32, &str, bool)> = s
            .world
            .alerts
            .iter()
            .map(|a| (a.kind, a.resref.as_str(), a.completed))
            .collect();
        assert_eq!(
            alerts,
            [
                (ALERT_CODEX, "plt_cod_crt_ogre", false),
                (ALERT_QUEST, "plt_lot100pt_bears", true),
            ]
        );
        // The movie clears a quest's `Updated` as it is read (its place in the list).
        assert!(s.journal_message(
            MSG_SET_ATTRIBUTE,
            &["Journal.CompletedPlots.0.Updated".into(), "false".into()]
        ));
        assert!(!s.journal().completed[0].updated);
        assert!(s.journal().active[0].updated, "the others stay new");
        assert!(s.journal_message(MSG_MARK_CODEX_READ, &["plt_cod_crt_ogre".into()]));
        assert!(!s.journal().codex_updated);
        // Dismissing a kind of notification drops those.
        assert!(s.journal_message(MSG_HIDE_ALERT, &[ALERT_QUEST.to_string()]));
        assert_eq!(s.world.alerts.len(), 1);
        assert_eq!(s.world.alerts[0].kind, ALERT_CODEX);
    }
}
