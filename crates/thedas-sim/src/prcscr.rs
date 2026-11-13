//! The scripts add-ins run when an area list loads (`PRCSCR_*.gda`): how downloadable content
//! hooks into the campaign (bonus items, Shale's merchants, the camp's extra followers).
//!
//! DAOrigins.exe opens every `.gda` whose name starts with `PRCSCR_` as one set of rows
//! (0x6fba00, called as an area list loads, 0x6fbff0). A row names an area list (column
//! 0xf41e4134) and a script (column 0xf333fe09); the rows whose area list is `any` or the
//! loaded list's own name give the scripts to run, in order. `mem000_additem` is left out
//! unless the module is `Single Player`. The module's update then runs each once per load of
//! the list (0x5fe200 -> 0x6fa700, a flag on the list; 0x6fa480), after that update's
//! events (0x64b010): a plain script run with the hero as `OBJECT_SELF` and no event.
//!
//! Provisional: the order the tables are read in is the resource manager's, not traced;
//! here it is the prefix set's (`Tables::prefix_worksheets`). The exe also runs them again when add-ins are refreshed while
//! playing (0x6031c0), which the simulation does not do. The module's area tree names a
//! list `al_<name>`, cut to 27 characters, so a cut name matches by prefix.

use std::sync::Arc;

use thedas_formats::gda::Cell;

use crate::sim::Sim;

const COL_AREA_LIST: u32 = 0xf41e_4134;
const COL_SCRIPT: u32 = 0xf333_fe09;

/// The longest an area tree's list name gets (`al_` and 24 characters).
const TREE_NAME_MAX: usize = 27;

/// The rows of one `PRCSCR_*` worksheet: (area list, script), lower case, in file order.
pub fn rows(t: &crate::m2da::Table) -> Vec<(String, String)> {
    let text = |c: Option<&Cell>| match c {
        Some(Cell::String(s)) => crate::engine::clean(s).to_ascii_lowercase(),
        _ => String::new(),
    };
    t.order
        .iter()
        .map(|&id| {
            (
                text(t.cell_by_hash(COL_AREA_LIST, id)),
                text(t.cell_by_hash(COL_SCRIPT, id)),
            )
        })
        .filter(|(l, s)| !l.is_empty() && !s.is_empty())
        .collect()
}

/// Whether a row's area list is the one named `tree` in the module's area tree.
pub fn names_list(row: &str, tree: &str) -> bool {
    let tree = tree.to_ascii_lowercase();
    let name = tree.strip_prefix("al_").unwrap_or(&tree);
    row == name || (tree.len() >= TREE_NAME_MAX && row.starts_with(name))
}

impl Sim {
    /// Every row of the `PRCSCR_` prefix set (see `Tables::prefix_worksheets`: each
    /// worksheet read on its own, in the set's order), read once.
    fn prcscr_rows(&mut self) -> Arc<Vec<(String, String)>> {
        if let Some(r) = &self.prcscr {
            return r.clone();
        }
        let out: Vec<(String, String)> = self
            .world
            .tables()
            .prefix_worksheets("PRCSCR_")
            .iter()
            .flat_map(|(_, t)| rows(t))
            .collect();
        let out = Arc::new(out);
        self.prcscr = Some(out.clone());
        out
    }

    /// The area list the hero's area belongs to, as the module's area tree names it.
    fn hero_area_list(&self) -> Option<String> {
        let hero = self.world.get(self.world.hero?)?;
        let area = self.world.get(hero.area?)?;
        self.module_info
            .areas
            .iter()
            .find(|a| a.resref.eq_ignore_ascii_case(&area.template))
            .and_then(|a| a.area_list.clone())
    }

    /// Runs the add-ins' scripts for the hero's area list if it has been loaded since they
    /// last ran: on arriving in an area of another list, and after a load.
    pub(crate) fn run_area_list_scripts(&mut self) {
        let Some(list) = self.hero_area_list() else {
            return;
        };
        if self.prcscr_ran.as_deref() == Some(list.as_str()) {
            return;
        }
        self.prcscr_ran = Some(list.clone());
        let Some(hero) = self.world.hero else {
            return;
        };
        let single_player = self.module_info.name.eq_ignore_ascii_case("single player");
        let scripts: Vec<String> = self
            .prcscr_rows()
            .iter()
            .filter(|(l, s)| {
                (l == "any" || names_list(l, &list)) && (single_player || s != "mem000_additem")
            })
            .map(|(_, s)| s.clone())
            .collect();
        for s in scripts {
            self.run_script(&s, hero, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use thedas_script::vm::ObjectId;

    #[test]
    fn the_scripts_run_once_each_time_an_area_list_loads() {
        use crate::sim::AreaEntry;
        use crate::world::ObjectKind;
        let table = Arc::new(thedas_script::EngineTable::default());
        let mut s = Sim::new(Arc::new(thedas_vfs::Vfs::new()), table, 1);
        s.module_info.name = "single player".into();
        let entry = |resref: &str, list: &str| AreaEntry {
            resref: resref.into(),
            layout: None,
            area_list: Some(list.into()),
        };
        s.module_info.areas = vec![
            entry("den200ar_market", "al_den02al_den_market"),
            entry("den220ar_noble_tavern", "al_den02al_den_market"),
            entry("lot100ar_lothering", "al_lot100ar_lothering"),
        ];
        s.prcscr = Some(Arc::new(vec![
            ("any".into(), "dlc_any".into()),
            ("den02al_den_market".into(), "dlc_market".into()),
            ("any".into(), "mem000_additem".into()),
        ]));
        let area = |s: &mut Sim, template: &str| {
            let id = s.world.spawn(ObjectKind::Area, template);
            s.world.get_mut(id).unwrap().template = template.into();
            id
        };
        let (market, tavern, lothering) = (
            area(&mut s, "den200ar_market"),
            area(&mut s, "den220ar_noble_tavern"),
            area(&mut s, "lot100ar_lothering"),
        );
        let hero = s.world.spawn(ObjectKind::Creature, "player");
        s.world.hero = Some(hero);
        s.world.trace.keep = true;
        // The scripts are not in the (empty) resource stack: each run is noted as missing.
        let mut ran = |s: &mut Sim, to: ObjectId| -> Vec<String> {
            s.world.get_mut(hero).unwrap().area = Some(to);
            s.world.trace.entries.clear();
            s.tick(100);
            s.tick(100);
            s.world
                .trace
                .entries
                .iter()
                .filter_map(|e| match e {
                    crate::trace::TraceEntry::Missing { what, .. } => {
                        what.strip_prefix("script ").map(str::to_string)
                    }
                    _ => None,
                })
                .collect()
        };
        assert_eq!(
            ran(&mut s, market),
            ["dlc_any", "dlc_market", "mem000_additem"]
        );
        assert!(ran(&mut s, tavern).is_empty(), "the same list");
        assert_eq!(ran(&mut s, lothering), ["dlc_any", "mem000_additem"]);
        assert_eq!(ran(&mut s, tavern).len(), 3, "the list was loaded again");
        // A load loads the list again.
        let save = s.save_game();
        s.load_game(save).unwrap();
        s.world.trace.keep = true;
        assert_eq!(ran(&mut s, tavern).len(), 3);
        // Another module leaves mem000_additem out.
        s.module_info.name = "dao_prc_gib".into();
        assert_eq!(ran(&mut s, lothering), ["dlc_any"]);
    }

    #[test]
    fn rows_match_a_list_by_its_name_without_the_prefix() {
        assert!(names_list("den02al_den_market", "al_den02al_den_market"));
        assert!(names_list(
            "urn230ar_the_gauntlet",
            "al_urn230ar_the_gauntlet"
        ));
        assert!(!names_list(
            "den02al_den_market",
            "al_den03al_den_elven_aliena"
        ));
        // The tree cuts names to 27 characters.
        assert!(names_list(
            "orz01al_orzammar_entrance",
            "al_orz01al_orzammar_entranc"
        ));
        assert!(!names_list(
            "orz01al_orzammar",
            "al_orz01al_orzammar_entranc"
        ));
        // A short list name is not a prefix test.
        assert!(!names_list(
            "cam100ar_camp_plains2",
            "al_cam100ar_camp_plains"
        ));
    }
}
