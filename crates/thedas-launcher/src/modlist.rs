//! Editing the mod list (`mods.txt`, read by [`thedas_vfs::mods::ModList`]) without
//! disturbing it: comments and blank lines stay where they are, a mod turned off becomes a
//! comment the engine skips (`#off: <line>`), and the order changes only when the player moves
//! a mod. Nothing is written until [`ModFile::save`], whose first write keeps the old list as
//! `mods.txt.bak`.

use std::path::{Path, PathBuf};

use thedas_vfs::mods::ModList;

/// The comment prefix of a mod that is turned off.
pub const OFF: &str = "#off: ";

#[derive(Clone, Debug, PartialEq, Eq)]
enum Line {
    Mod { text: String, on: bool },
    Other(String),
}

/// One mod of the list, for showing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModRow {
    /// The line as written (a path, maybe `archive.zip!/folder`).
    pub text: String,
    pub on: bool,
    /// The file or folder it names on disk.
    pub path: PathBuf,
    pub exists: bool,
}

#[derive(Clone, Debug)]
pub struct ModFile {
    pub path: PathBuf,
    lines: Vec<Line>,
    crlf: bool,
    saved: Vec<Line>,
}

impl ModFile {
    /// Reads the list; a missing file is an empty list.
    pub fn load(path: &Path) -> ModFile {
        let text = std::fs::read(path)
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_default();
        ModFile::parse(path, &text)
    }

    pub fn parse(path: &Path, text: &str) -> ModFile {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let lines: Vec<Line> = text
            .lines()
            .map(|raw| {
                let raw = raw.trim_end_matches('\r');
                let t = raw.trim();
                if let Some(rest) = t.strip_prefix(OFF.trim_end()) {
                    let rest = rest.trim();
                    if !rest.is_empty() {
                        return Line::Mod {
                            text: rest.to_string(),
                            on: false,
                        };
                    }
                }
                if t.is_empty() || t.starts_with('#') {
                    Line::Other(raw.to_string())
                } else {
                    Line::Mod {
                        text: t.to_string(),
                        on: true,
                    }
                }
            })
            .collect();
        ModFile {
            path: path.to_path_buf(),
            crlf: text.contains("\r\n"),
            saved: lines.clone(),
            lines,
        }
    }

    /// The mods, lowest priority first.
    pub fn rows(&self) -> Vec<ModRow> {
        let base = self.path.parent().unwrap_or(Path::new("."));
        self.lines
            .iter()
            .filter_map(|l| match l {
                Line::Mod { text, on } => {
                    let path = ModList::parse(text, base)
                        .entries
                        .into_iter()
                        .next()
                        .map(|e| e.path)
                        .unwrap_or_else(|| PathBuf::from(text));
                    Some(ModRow {
                        text: text.clone(),
                        on: *on,
                        exists: path.exists(),
                        path,
                    })
                }
                Line::Other(_) => None,
            })
            .collect()
    }

    /// The line index of mod `n`.
    fn line_of(&self, n: usize) -> Option<usize> {
        self.lines
            .iter()
            .enumerate()
            .filter(|(_, l)| matches!(l, Line::Mod { .. }))
            .nth(n)
            .map(|(i, _)| i)
    }

    pub fn set_on(&mut self, n: usize, value: bool) {
        if let Some(Line::Mod { on, .. }) = self.line_of(n).and_then(|i| self.lines.get_mut(i)) {
            *on = value;
        }
    }

    /// Swaps mod `n` with the mod before it (lower priority); other lines stay put.
    pub fn move_up(&mut self, n: usize) {
        if n == 0 {
            return;
        }
        if let (Some(a), Some(b)) = (self.line_of(n - 1), self.line_of(n)) {
            self.lines.swap(a, b);
        }
    }

    /// Swaps mod `n` with the mod after it (higher priority).
    pub fn move_down(&mut self, n: usize) {
        self.move_up(n + 1);
    }

    pub fn remove(&mut self, n: usize) {
        if let Some(i) = self.line_of(n) {
            self.lines.remove(i);
        }
    }

    /// Adds a mod at the end: above every other, as a new download usually wants.
    pub fn add(&mut self, entry: &str) {
        let entry = entry.trim();
        if entry.is_empty() || entry.starts_with('#') || entry.contains(['\n', '\r']) {
            return;
        }
        self.lines.push(Line::Mod {
            text: entry.to_string(),
            on: true,
        });
    }

    /// Whether there are changes not yet saved.
    pub fn dirty(&self) -> bool {
        self.lines != self.saved
    }

    pub fn revert(&mut self) {
        self.lines = self.saved.clone();
    }

    pub fn to_text(&self) -> String {
        let nl = if self.crlf { "\r\n" } else { "\n" };
        let mut out = String::new();
        for l in &self.lines {
            match l {
                Line::Mod { text, on: true } => out.push_str(text),
                Line::Mod { text, on: false } => {
                    out.push_str(OFF);
                    out.push_str(text);
                }
                Line::Other(raw) => out.push_str(raw),
            }
            out.push_str(nl);
        }
        out
    }

    /// Writes the list. The first save keeps the file it replaces as `mods.txt.bak`.
    pub fn save(&mut self) -> std::io::Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut bak = self.path.clone().into_os_string();
        bak.push(".bak");
        let bak = PathBuf::from(bak);
        if self.path.is_file() && !bak.exists() {
            std::fs::copy(&self.path, &bak)?;
        }
        let mut tmp = self.path.clone().into_os_string();
        tmp.push(".tmp");
        let tmp = PathBuf::from(tmp);
        std::fs::write(&tmp, self.to_text())?;
        std::fs::rename(&tmp, &self.path)?;
        self.saved = self.lines.clone();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdir::TestDir;

    const LIST: &str = "# Later lines win.\n/mods/a\n\n# faces\n/mods/b.zip!/Pearly White\n#off: /mods/c\n/mods/d\n";

    #[test]
    fn untouched_list_writes_back_the_same() {
        let f = ModFile::parse(Path::new("/x/mods.txt"), LIST);
        assert_eq!(f.to_text(), LIST);
        assert!(!f.dirty());
        let rows = f.rows();
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[1].text, "/mods/b.zip!/Pearly White");
        assert_eq!(rows[1].path, PathBuf::from("/mods/b.zip"));
        assert!(!rows[2].on);
    }

    #[test]
    fn edits_only_move_what_the_player_moved() {
        let mut f = ModFile::parse(Path::new("/x/mods.txt"), LIST);
        f.set_on(2, true);
        f.set_on(0, false);
        f.move_up(1);
        assert!(f.dirty());
        assert_eq!(
            f.to_text(),
            "# Later lines win.\n/mods/b.zip!/Pearly White\n\n# faces\n#off: /mods/a\n/mods/c\n/mods/d\n"
        );
        f.move_down(3); // the last: nothing after it
        f.move_up(0); // the first: nothing before it
        f.remove(9);
        f.add("  ");
        f.add("# not a mod");
        f.add("evil\n/injected");
        assert_eq!(f.rows().len(), 4);
        f.add("/mods/e");
        assert_eq!(f.rows().last().unwrap().text, "/mods/e");
        f.revert();
        assert_eq!(f.to_text(), LIST);
    }

    #[test]
    fn engine_reads_off_lines_as_comments() {
        let base = Path::new("/x");
        let list = ModList::parse(LIST, base);
        assert_eq!(list.entries.len(), 3);
    }

    #[test]
    fn crlf_and_relative_paths() {
        let f = ModFile::parse(Path::new("/x/mods.txt"), "a\r\n#c\r\n");
        assert_eq!(f.to_text(), "a\r\n#c\r\n");
        assert_eq!(f.rows()[0].path, PathBuf::from("/x/a"));
    }

    #[test]
    fn save_keeps_a_backup_once() {
        let t = TestDir::new("modlist");
        let path = t.path().join("mods.txt");
        std::fs::write(&path, LIST).unwrap();
        let mut f = ModFile::load(&path);
        f.set_on(0, false);
        f.save().unwrap();
        assert!(!f.dirty());
        assert_eq!(
            std::fs::read_to_string(t.path().join("mods.txt.bak")).unwrap(),
            LIST
        );
        f.set_on(0, true);
        f.save().unwrap();
        assert_eq!(
            std::fs::read_to_string(t.path().join("mods.txt.bak")).unwrap(),
            LIST
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), LIST);
        // A missing list saves as a new file, with no backup.
        let mut g = ModFile::load(&t.path().join("new/mods.txt"));
        g.add("/m");
        g.save().unwrap();
        assert!(!t.path().join("new/mods.txt.bak").exists());
    }
}
