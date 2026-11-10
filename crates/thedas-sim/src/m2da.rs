//! Merged 2DA tables (M2DA), as `GetM2DAInt` and friends read them.
//!
//! `m2da_base.gda` lists each table id with a label and a worksheet prefix (`1, Abilities,
//! ABI_`). A table is the union of every GDA resource whose name starts with the prefix
//! (`abi_base.gda`, `abi_dlc.gda`, ...), indexed by the first column (`ID`). Provisional: when
//! two worksheets define the same row id, the one that sorts later by name wins; scripts can
//! also name a worksheet directly (the optional `s2DA` argument), which reads just that file.

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
            if !self.rows.contains_key(&id) {
                self.order.push(id);
            }
            self.rows.insert(id, cells);
        }
        for r in self.rows.values_mut() {
            r.resize(width, Cell::Empty);
        }
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
        let mut prefixes = HashMap::new();
        if let Ok((data, _)) = vfs.read("m2da_base.gda") {
            if let Ok(g) = Gda::parse(&data) {
                let ws = g.column("Worksheet").unwrap_or(2);
                for row in &g.rows {
                    if let (Some(Cell::Int(id)), Some(Cell::String(p))) = (row.first(), row.get(ws))
                    {
                        prefixes.insert(
                            *id as i32,
                            p.trim_matches(|c: char| c == '\0' || c.is_whitespace())
                                .to_ascii_lowercase(),
                        );
                    }
                }
            }
        }
        let mut gdas: Vec<String> = vfs
            .names()
            .into_iter()
            .filter(|n| n.ends_with(".gda"))
            .collect();
        gdas.sort();
        Tables {
            prefixes,
            gdas,
            loaded: Mutex::new(HashMap::new()),
            vfs: Some(vfs),
        }
    }

    /// Puts a ready-made table under a worksheet name (as `get(_, name)` finds it), for tests.
    #[cfg(test)]
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
            for name in self.gdas.iter().filter(|n| n.starts_with(prefix.as_str())) {
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
