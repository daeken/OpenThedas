//! Merged 2DA tables (M2DA), as `GetM2DAInt` and friends read them.
//!
//! `m2da_base.gda`, and any other GDA whose name starts with `m2da_`, lists each table id with
//! a label and a worksheet prefix (`1, Abilities, ABI_`). A table is the union of every GDA resource whose name starts with the prefix
//! (`abi_base.gda`, `abi_dlc.gda`, ...), indexed by the first column (`ID`). Scripts can also
//! name a worksheet directly (the optional `s2DA` argument), which reads just that file.
//!
//! A row id belongs to the first worksheet that defines it: the original skips a row whose id
//! the set already holds (`0x4ec910` returns -1 and `0x4eca10` moves on), within one worksheet
//! and across worksheets alike.
//!
//! The order of the worksheets is the order of the original's resource listing (`0x4c56b0`):
//! its resource locations from the lowest priority number up, so a worksheet in a
//! higher-priority folder (an add-in's, a mod's) comes before the base game's. That much is
//! traced and ours follows it by stack layer. Inside one location the original lists resources
//! in the order of a hash container keyed by name (CRC-32 of the UTF-16 name, MSVC
//! `hash_map` bucket order), which is not reproduced yet. Provisional: among worksheets served
//! from the same layer the later name comes first, which keeps the result this loader gave
//! before rows stopped overwriting each other (the shipped AI package tables, `aip_archer` and
//! `aip_archer_*`, share row ids inside one archive, so this choice decides them).

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

use thedas_formats::Gda;
use thedas_formats::gda::Cell;
use thedas_vfs::Vfs;

#[derive(Debug, Default)]
pub struct Table {
    /// Column name hash -> column index.
    pub columns: HashMap<u32, usize>,
    /// Row id -> cells.
    pub rows: BTreeMap<i64, Vec<Cell>>,
    /// Row ids in file order (for `GetM2DARowIdFromRowIndex`).
    pub order: Vec<i64>,
}

impl Table {
    fn merge(&mut self, gda: &Gda) {
        let mut map = Vec::new();
        for (i, c) in gda.columns.iter().enumerate() {
            let n = self.columns.len();
            let at = *self.columns.entry(c.hash).or_insert(n);
            map.push((i, at));
        }
        let width = self.columns.len();
        for row in &gda.rows {
            let id = match row.first() {
                Some(Cell::Int(v)) => *v,
                _ => continue,
            };
            let mut cells = vec![Cell::Empty; width];
            for (i, at) in &map {
                if let Some(c) = row.get(*i) {
                    cells[*at] = c.clone();
                }
            }
            // The first definition of a row id stands.
            if self.rows.contains_key(&id) {
                continue;
            }
            self.order.push(id);
            self.rows.insert(id, cells);
        }
        for r in self.rows.values_mut() {
            r.resize(width, Cell::Empty);
        }
    }

    /// Whether the table has no rows.
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Whether the table has a row with this id.
    pub fn has_row(&self, row: i64) -> bool {
        self.rows.contains_key(&row)
    }

    pub fn cell(&self, column: &str, row: i64) -> Option<&Cell> {
        let c = *self
            .columns
            .get(&thedas_formats::gda::column_hash(column))?;
        self.rows.get(&row)?.get(c)
    }

    /// The id of the first row whose `column` is the string `value` (ignoring case and any
    /// trailing NUL).
    pub fn find(&self, column: &str, value: &str) -> Option<i64> {
        let c = *self
            .columns
            .get(&thedas_formats::gda::column_hash(column))?;
        self.order.iter().copied().find(|id| {
            matches!(self.rows.get(id).and_then(|r| r.get(c)),
                Some(Cell::String(s)) if s.trim_end_matches('\0').trim().eq_ignore_ascii_case(value))
        })
    }

    pub fn cell_by_hash(&self, hash: u32, row: i64) -> Option<&Cell> {
        let c = *self.columns.get(&hash)?;
        self.rows.get(&row)?.get(c)
    }
}

/// The prefix of the GDAs that list the merged tables (`m2da_base.gda` and any other).
const TABLE_LIST_PREFIX: &str = "m2da_";

/// The names in `gdas` (lowercase resource names) that start with `prefix`, in `gdas`' order.
fn prefix_set<'a>(gdas: &'a [String], prefix: &str) -> Vec<&'a str> {
    let prefix = prefix.to_ascii_lowercase();
    gdas.iter()
        .map(String::as_str)
        .filter(|n| n.starts_with(&prefix))
        .collect()
}

/// Orders the worksheets of a set as their rows take precedence: the one the resource stack
/// serves from a higher layer first (traced), then the later name first (provisional; see the
/// module documentation).
fn in_set_order<'a>(vfs: &Vfs, mut names: Vec<&'a str>) -> Vec<&'a str> {
    names.sort_by_cached_key(|n| {
        let layer = vfs.lookup(n).map_or(0, |p| p.layer);
        std::cmp::Reverse((layer, n.to_string()))
    });
    names
}

/// Adds one table-list worksheet's rows (table id, label, worksheet prefix) to `prefixes`. An
/// id an earlier worksheet (or row) gave keeps its prefix, as rows do in any prefix set.
fn add_prefixes(prefixes: &mut HashMap<i32, String>, list: &Gda) {
    let ws = list.column("Worksheet").unwrap_or(2);
    for row in &list.rows {
        if let (Some(Cell::Int(id)), Some(Cell::String(p))) = (row.first(), row.get(ws)) {
            let p = p.trim_matches(|c: char| c == '\0' || c.is_whitespace());
            prefixes
                .entry(*id as i32)
                .or_insert_with(|| p.to_ascii_lowercase());
        }
    }
}

/// Every M2DA table, built lazily from the resource stack.
pub struct Tables {
    /// Table id -> worksheet prefix (lowercase).
    prefixes: HashMap<i32, String>,
    /// Resource names of every GDA in the stack, sorted.
    gdas: Vec<String>,
    loaded: Mutex<HashMap<String, std::sync::Arc<Table>>>,
    vfs: Option<std::sync::Arc<Vfs>>,
}

impl Tables {
    pub fn empty() -> Self {
        Tables {
            prefixes: HashMap::new(),
            gdas: Vec::new(),
            loaded: Mutex::new(HashMap::new()),
            vfs: None,
        }
    }

    pub fn new(vfs: std::sync::Arc<Vfs>) -> Self {
        let mut gdas: Vec<String> = vfs
            .names()
            .into_iter()
            .filter(|n| n.ends_with(".gda"))
            .collect();
        gdas.sort();
        // The list of tables is itself a prefix set: the original opens every GDA whose name
        // starts with `M2DA_` (0x4e0cf8), so an add-in or mod adds tables with its own
        // `m2da_*.gda` beside `m2da_base.gda`.
        let mut prefixes = HashMap::new();
        for name in in_set_order(&vfs, prefix_set(&gdas, TABLE_LIST_PREFIX)) {
            if let Ok((data, _)) = vfs.read(name) {
                if let Ok(g) = Gda::parse(&data) {
                    add_prefixes(&mut prefixes, &g);
                }
            }
        }
        Tables {
            prefixes,
            gdas,
            loaded: Mutex::new(HashMap::new()),
            vfs: Some(vfs),
        }
    }

    /// The names of every GDA in the stack whose name starts with `prefix` (any case): a
    /// prefix set, as the original opens them (`0x4ec7a0`; `M2DA_` for the table list,
    /// `PRCSCR_` for the add-ins' start-up scripts). In the order their rows take precedence
    /// (see the module documentation): higher layer first, then the later name first.
    pub fn prefix_set(&self, prefix: &str) -> Vec<&str> {
        let names = prefix_set(&self.gdas, prefix);
        match &self.vfs {
            Some(vfs) => in_set_order(vfs, names),
            None => names,
        }
    }

    /// Each worksheet of a prefix set as its own table, in [`Tables::prefix_set`] order, for
    /// sets whose files are read one by one rather than merged by row id. A worksheet that
    /// cannot be read or parsed is left out.
    pub fn prefix_worksheets(&self, prefix: &str) -> Vec<(String, std::sync::Arc<Table>)> {
        self.prefix_set(prefix)
            .into_iter()
            .filter_map(|name| Some((name.to_string(), self.get(0, name)?)))
            .collect()
    }

    /// Puts a ready-made table under a worksheet name (as `get(_, name)` finds it, and
    /// `#<id>` as `get(id, "")` does), for tests.
    pub fn insert(&self, worksheet: &str, columns: &[&str], rows: Vec<(i64, Vec<Cell>)>) {
        let mut t = Table::default();
        for (i, c) in columns.iter().enumerate() {
            // `#1234abcd` names a column by its hash (for columns whose names are unknown).
            let h = match c.strip_prefix('#') {
                Some(x) => u32::from_str_radix(x, 16).unwrap_or(0),
                None => thedas_formats::gda::column_hash(c),
            };
            t.columns.insert(h, i);
        }
        for (id, cells) in rows {
            t.order.push(id);
            t.rows.insert(id, cells);
        }
        if let Ok(mut l) = self.loaded.lock() {
            l.insert(worksheet.to_ascii_lowercase(), std::sync::Arc::new(t));
        }
    }

    /// The table for an id, or for an explicit worksheet name when `worksheet` is non-empty.
    pub fn get(&self, id: i32, worksheet: &str) -> Option<std::sync::Arc<Table>> {
        let key = if worksheet.is_empty() {
            format!("#{id}")
        } else {
            worksheet.to_ascii_lowercase()
        };
        if let Some(t) = self.loaded.lock().ok()?.get(&key) {
            return Some(t.clone());
        }
        let vfs = self.vfs.as_ref()?;
        let mut table = Table::default();
        if worksheet.is_empty() {
            let prefix = self.prefixes.get(&id)?;
            for name in in_set_order(vfs, prefix_set(&self.gdas, prefix)) {
                if let Ok((data, _)) = vfs.read(name) {
                    if let Ok(g) = Gda::parse(&data) {
                        table.merge(&g);
                    }
                }
            }
        } else {
            let name = if worksheet.ends_with(".gda") {
                key.clone()
            } else {
                format!("{key}.gda")
            };
            let (data, _) = vfs.read(&name).ok()?;
            table.merge(&Gda::parse(&data).ok()?);
        }
        let t = std::sync::Arc::new(table);
        self.loaded.lock().ok()?.insert(key, t.clone());
        Some(t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use thedas_formats::gda::{Column, column_hash};

    fn list(rows: &[(i64, &str)]) -> Gda {
        Gda {
            version: thedas_formats::FourCC(*b"V0.2"),
            columns: ["ID", "Label", "Worksheet"]
                .iter()
                .map(|n| Column {
                    hash: column_hash(n),
                    kind: None,
                })
                .collect(),
            rows: rows
                .iter()
                .map(|(id, p)| {
                    vec![
                        Cell::Int(*id),
                        Cell::String("label".into()),
                        Cell::String(p.to_string()),
                    ]
                })
                .collect(),
        }
    }

    #[test]
    fn every_table_list_worksheet_adds_tables() {
        let gdas: Vec<String> = [
            "abi_base.gda",
            "m2da_base.gda",
            "m2da_base_mymod.gda",
            "m2dax.gda",
            "prcscr_one.gda",
            "prcscr_two.gda",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(
            prefix_set(&gdas, "M2DA_"),
            ["m2da_base.gda", "m2da_base_mymod.gda"]
        );
        assert_eq!(
            prefix_set(&gdas, "PRCSCR_"),
            ["prcscr_one.gda", "prcscr_two.gda"]
        );
        assert!(prefix_set(&gdas, "none_").is_empty());

        let mut prefixes = HashMap::new();
        add_prefixes(&mut prefixes, &list(&[(1, "ABI_\0"), (2, " ITM_ ")]));
        add_prefixes(&mut prefixes, &list(&[(2, "ITX_"), (9000, "MYMOD_")]));
        assert_eq!(prefixes[&1], "abi_");
        // The first worksheet to give an id keeps it, and new ids are added.
        assert_eq!(prefixes[&2], "itm_");
        assert_eq!(prefixes[&9000], "mymod_");
        assert_eq!(prefixes.len(), 3);
    }

    #[test]
    fn the_first_definition_of_a_row_stands() {
        let mut t = Table::default();
        // Twice in one worksheet, then again in a later one.
        t.merge(&list(&[(5, "first"), (5, "same sheet"), (6, "six")]));
        t.merge(&list(&[(5, "later sheet"), (7, "seven")]));
        let text = |id: i64| match t.cell("Worksheet", id) {
            Some(Cell::String(s)) => s.clone(),
            other => panic!("{other:?}"),
        };
        assert_eq!(text(5), "first");
        assert_eq!(text(6), "six");
        assert_eq!(text(7), "seven");
        assert_eq!(t.order, [5, 6, 7]);
    }

    #[test]
    fn worksheets_from_higher_layers_come_first() {
        let base = std::env::temp_dir().join(format!("thedas-m2da-order-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let write = |rel: &str| {
            let p = base.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b"x").unwrap();
        };
        write("core/aip_archer.gda");
        write("core/aip_archer_fire.gda");
        write("core/aip_archer_base.gda");
        write("addin/aip_archer_dlc.gda");
        write("mod/aip_archer_fire.gda");
        let mut vfs = Vfs::new();
        vfs.push_directory(base.join("core"), false, "core")
            .unwrap();
        vfs.push_directory(base.join("addin"), false, "addin")
            .unwrap();
        let mut overlay = Vfs::new();
        overlay
            .push_directory(base.join("mod"), false, "mod:m")
            .unwrap();
        vfs.set_overlay(overlay);
        let tables = Tables::new(std::sync::Arc::new(vfs));
        assert_eq!(
            tables.prefix_set("AIP_ARCHER"),
            [
                "aip_archer_fire.gda",
                "aip_archer_dlc.gda",
                "aip_archer_base.gda",
                "aip_archer.gda"
            ]
        );
        std::fs::remove_dir_all(&base).unwrap();
    }
}
