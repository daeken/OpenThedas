//! Job boards: the Chantry board and the guild boxes (`ShowChantersGUI`, movie
//! `chanters.gfx`).
//!
//! Provisional, inferred from the data and the movie's bytecode:
//! - `jobboards.gda` lists the boards (column 4 is the header's string id), and
//!   `jobboardplots.gda` the jobs: a plot, the board showing it, the plot flag that puts it
//!   on the board (`VisibleFlag`) and the flag set when the player takes it (`AcceptedFlag`).
//! - A job shows while its visible flag is set. Its title is the plot's name and its text
//!   the journal text of the visible flag.
//! - The movie counts `Journal.ChanterBoard.ChanterEntries`, reads each entry's `PlotName`,
//!   `JournalText`, `PlotResRef` and `Accepted`, sends `Journal.AcceptChantersQuest <index>`
//!   when the player takes a job and `Journal.ClearChanterBoard` when it closes. Taking a job
//!   sets its accepted flag (running the plot's script).
//!
//! Open question: jobs are posted by scripts setting their visible flags (a guild box's
//! plot script, or a finished job posting the next), but nothing in the data posts the first
//! Lothering job (`BANDITS2_CHANTER_BOARD`), although the area marks the board as having one.
//! The original engine may post some jobs itself (its `Journal` handles the movie's messages;
//! the posting rule is not recovered); until that is known, only flagged jobs show.
//!
//! The board's contents are kept in the world while it is open, so the interface reads them
//! without touching the simulation.

use serde::{Deserialize, Serialize};
use thedas_formats::gda::Cell;

use crate::sim::Sim;

/// `jobboardplots.gda` column hashes. Only some of the names are known (`ID`, `BoardID`,
/// `VisibleFlag`, `AcceptedFlag` hash to these); the plot column's name is not.
const COL_PLOT: u32 = 0x67cf_4c46;
const COL_BOARD: u32 = 0xe244_63ff;
const COL_VISIBLE: u32 = 0x9623_699d;
const COL_ACCEPTED: u32 = 0x8617_98a2;
/// `jobboards.gda`: the board header's string id (the fourth column).
const COL_HEADER: u32 = 0x46e2_5c69;

/// The message the movie sends when the player takes a job.
pub const MSG_ACCEPT: &str = "Journal.AcceptChantersQuest";
/// The message the movie sends when it closes.
pub const MSG_CLEAR: &str = "Journal.ClearChanterBoard";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BoardJob {
    /// The plot's resource name.
    pub plot: String,
    pub name_strref: Option<u32>,
    pub text_strref: Option<u32>,
    pub accepted_flag: i32,
    pub accepted: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChanterBoard {
    pub board: i32,
    pub header_strref: Option<u32>,
    pub jobs: Vec<BoardJob>,
}

fn cell_int(c: Option<&Cell>) -> Option<i64> {
    match c {
        Some(Cell::Int(v)) => Some(*v),
        Some(Cell::String(s)) => s.trim_end_matches('\0').trim().parse().ok(),
        _ => None,
    }
}

impl Sim {
    /// `ShowChantersGUI`: opens a board with the jobs it shows now.
    pub fn open_chanter_board(&mut self, board: i32) {
        let tables = self.world.tables();
        let header_strref = tables
            .get(0, "jobboards")
            .and_then(|t| cell_int(t.cell_by_hash(COL_HEADER, board as i64)))
            .and_then(|v| u32::try_from(v).ok());
        let mut rows = Vec::new();
        if let Some(t) = tables.get(0, "jobboardplots") {
            for &id in &t.order {
                if cell_int(t.cell_by_hash(COL_BOARD, id)) != Some(board as i64) {
                    continue;
                }
                let plot = match t.cell_by_hash(COL_PLOT, id) {
                    Some(Cell::String(s)) => s.trim_end_matches('\0').trim().to_string(),
                    _ => continue,
                };
                let visible = cell_int(t.cell_by_hash(COL_VISIBLE, id)).unwrap_or(-1) as i32;
                let accepted = cell_int(t.cell_by_hash(COL_ACCEPTED, id)).unwrap_or(-1) as i32;
                rows.push((plot, visible, accepted));
            }
        }
        let owner = self.world.hero.unwrap_or(thedas_script::vm::OBJECT_INVALID);
        let mut jobs = Vec::new();
        for (plot, visible, accepted_flag) in rows {
            if visible < 0 || !self.get_plot_flag(&plot, visible, false, owner) {
                continue;
            }
            let info = self.plot_info(&plot);
            let accepted =
                accepted_flag >= 0 && self.get_plot_flag(&plot, accepted_flag, false, owner);
            jobs.push(BoardJob {
                name_strref: info.as_ref().and_then(|p| p.name_strref),
                text_strref: info.as_ref().and_then(|p| p.journal.get(&visible).copied()),
                plot,
                accepted_flag,
                accepted,
            });
        }
        self.world.chanter_board = Some(ChanterBoard {
            board,
            header_strref,
            jobs,
        });
    }

    /// The player takes the job at `index` on the open board. Returns false when there is
    /// no such job or it was already taken.
    pub fn accept_board_job(&mut self, index: usize) -> bool {
        let Some(job) = self
            .world
            .chanter_board
            .as_ref()
            .and_then(|b| b.jobs.get(index))
            .cloned()
        else {
            return false;
        };
        if job.accepted || job.accepted_flag < 0 {
            return false;
        }
        let owner = self.world.hero.unwrap_or(thedas_script::vm::OBJECT_INVALID);
        self.set_plot_flag(&job.plot, job.accepted_flag, true, true, owner);
        if let Some(j) = self
            .world
            .chanter_board
            .as_mut()
            .and_then(|b| b.jobs.get_mut(index))
        {
            j.accepted = true;
        }
        true
    }

    /// A message from a game screen (`SendMessage`/`SendMessageArgs`) taken as player input.
    /// Returns whether the simulation acted on it.
    pub fn gui_message(&mut self, name: &str, args: &[String]) -> bool {
        match name {
            MSG_ACCEPT => {
                let i = args.first().and_then(|a| a.trim().parse::<f64>().ok());
                match i {
                    Some(i) if i >= 0.0 => self.accept_board_job(i as usize),
                    _ => false,
                }
            }
            MSG_CLEAR => self.world.chanter_board.take().is_some(),
            crate::inventory::MSG_EQUIP
            | crate::inventory::MSG_UNEQUIP
            | crate::inventory::MSG_NEXT_CHARACTER
            | crate::inventory::MSG_PREVIOUS_CHARACTER => self.inventory_message(name, args),
            n if n.ends_with(crate::quickbar::MSG_SET_QUICKSLOT) => {
                self.quickbar_message(name, args)
            }
            _ => self.world_map_message(name, args),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use thedas_script::EngineTable;
    use thedas_vfs::Vfs;

    use super::*;

    #[test]
    fn accepting_marks_the_job_once_and_clearing_closes() {
        let mut s = Sim::new(Arc::new(Vfs::new()), Arc::new(EngineTable::default()), 1);
        s.world.chanter_board = Some(ChanterBoard {
            board: 3,
            header_strref: None,
            jobs: vec![BoardJob {
                plot: "bears".into(),
                name_strref: None,
                text_strref: None,
                accepted_flag: 2,
                accepted: false,
            }],
        });
        assert!(!s.gui_message(MSG_ACCEPT, &["4".into()]), "no such job");
        assert!(s.gui_message(MSG_ACCEPT, &["0".into()]));
        assert!(
            s.world
                .plots
                .get(&("BEARS".into(), 2))
                .copied()
                .unwrap_or(false)
        );
        assert!(s.world.chanter_board.as_ref().unwrap().jobs[0].accepted);
        assert!(!s.gui_message(MSG_ACCEPT, &["0".into()]), "already taken");
        assert!(s.gui_message(MSG_CLEAR, &[]));
        assert!(s.world.chanter_board.is_none());
    }

    #[test]
    fn an_empty_install_has_an_empty_board() {
        let mut s = Sim::new(Arc::new(Vfs::new()), Arc::new(EngineTable::default()), 1);
        s.open_chanter_board(3);
        let b = s.world.chanter_board.as_ref().unwrap();
        assert_eq!((b.board, b.jobs.len()), (3, 0));
    }
}
