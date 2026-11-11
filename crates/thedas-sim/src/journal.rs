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

use crate::plot::PlotInfo;
use crate::sim::Sim;

/// The quest the player marked as active (its plot resource).
pub const MSG_SET_ACTIVE: &str = "Journal.SetActiveQuest";
/// The player read a codex entry, or all of them; nothing is marked as new here.
pub const MSG_MARK_CODEX_READ: &str = "Journal.MarkCodexAsRead";
pub const MSG_MARK_ALL_CODEX_READ: &str = "Journal.MarkAllCodexAsRead";

/// Quests, Chanter's Board jobs and quest groups (`plottypes.gda` rows).
const KIND_QUEST: i32 = 1;
const KIND_CHANTERS: i32 = 12;
const KIND_GROUP: i32 = 14;
/// `plottypes.gda`'s M2DA id (journal.gfx's `TABLE_PLOT_TYPES`) and its columns: the name
/// string and whether the type is a codex category.
const TABLE_PLOT_TYPES: i32 = 108;
const HASH_TYPE_NAME: u32 = 0xa422_7c06;
const HASH_IS_CODEX: u32 = 0xeb84_ee4d;

/// A quest's latest journal entry: the flag whose text it shows and when it was set (a
/// running count).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct JournalMark {
    pub flag: i32,
    pub seq: u64,
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
}

impl Sim {
    /// Notes a quest's new journal entry when one of its flags with journal text is set
    /// (called as flags are set).
    pub(crate) fn note_journal_flag(&mut self, info: &PlotInfo, flag: i32) {
        if flag >= 256 || !info.journal.get(&flag).is_some_and(|&t| self.has_text(t)) {
            return;
        }
        self.world.journal_seq += 1;
        let seq = self.world.journal_seq;
        self.world
            .journal
            .insert(info.guid.clone(), JournalMark { flag, seq });
    }

    /// Whether a journal text says something: many flags name a string that is empty.
    fn has_text(&self, strref: u32) -> bool {
        self.string(strref).is_some_and(|t| !t.trim().is_empty())
    }

    /// Whether a script has set a plot's flag. A flag a plot file starts set makes no journal
    /// entry (a Chanter's Board posting starts set, with the board's text): the journal holds
    /// what happened. Provisional.
    fn flag_on(&self, info: &PlotInfo, flag: i32) -> bool {
        self.world
            .plots
            .get(&(info.guid.clone(), flag))
            .copied()
            .unwrap_or(false)
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
                .filter(|(f, t)| **f < 256 && self.flag_on(p, **f) && self.has_text(**t))
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
                        texts: p
                            .journal
                            .iter()
                            .filter(|(f, t)| {
                                **f < 256 && self.flag_on(p, **f) && self.has_text(**t)
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
        view
    }

    /// journal.gfx's messages: marking a quest active (its plot resource; map markers follow
    /// it in the original), and reading codex entries (nothing is marked as new here).
    pub(crate) fn journal_message(&mut self, name: &str, args: &[String]) -> bool {
        match name {
            MSG_SET_ACTIVE => {
                let r = args.first().map(|a| a.trim()).unwrap_or_default();
                self.world.active_quest = (!r.is_empty()).then(|| r.to_string());
                true
            }
            MSG_MARK_CODEX_READ | MSG_MARK_ALL_CODEX_READ => true,
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
    fn quests_show_their_latest_entry_under_their_group_and_the_codex_unlocks() {
        let mut s = Sim::new(Arc::new(Vfs::new()), Arc::new(EngineTable::default()), 1);
        let mut group = plot("lot_plot_group", KIND_GROUP, None, &[]);
        group.priority = 2;
        s.plots.insert(group);
        let mut bears = plot(
            "lot100pt_bears",
            KIND_QUEST,
            Some("lot_plot_group"),
            &[(0, 500), (1, 501), (2, 502), (3, 503)],
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
            [500, 501, 502, 600, 700, 800, 801, 900]
                .into_iter()
                .map(|t| (t, format!("entry {t}")))
                .chain([(503, String::new())])
                .collect(),
        );
        assert_eq!(s.journal().active, Vec::new());

        let o = OBJECT_INVALID;
        s.set_plot_flag("lot100pt_bears", 1, true, false, o);
        s.set_plot_flag("lot100pt_bears", 0, true, false, o);
        s.set_plot_flag("lot100pt_bears_sub", 0, true, false, o);
        s.set_plot_flag("lot000pt_ambient", 0, true, false, o);
        s.set_plot_flag("cod_crt_ogre", 1, true, false, o);
        // A flag whose text is empty makes no entry.
        s.set_plot_flag("lot100pt_bears", 3, true, false, o);
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
    }
}
