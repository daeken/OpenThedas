//! Reading string values from a Windows registry: the real one on Windows, or a Wine prefix's
//! text copy (`system.reg` for `HKEY_LOCAL_MACHINE`, `user.reg` for `HKEY_CURRENT_USER`),
//! which is how a CrossOver bottle keeps it.

use std::collections::HashMap;
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Hive {
    LocalMachine,
    CurrentUser,
}

/// String values by key path (`SOFTWARE\GOG.com\Games\1949616134`) and value name.
pub trait Registry: Send + Sync {
    fn string(&self, hive: Hive, key: &str, name: &str) -> Option<String>;
}

/// [`Registry::string`] under `SOFTWARE\WOW6432Node\<rest>` (where a 64-bit Windows keeps a
/// 32-bit installer's keys), then `SOFTWARE\<rest>`.
pub fn software(reg: &dyn Registry, hive: Hive, rest: &str, name: &str) -> Option<String> {
    reg.string(hive, &format!(r"SOFTWARE\WOW6432Node\{rest}"), name)
        .or_else(|| reg.string(hive, &format!(r"SOFTWARE\{rest}"), name))
        .filter(|s| !s.trim().is_empty())
}

/// No registry (macOS and Linux outside a Wine prefix).
pub struct NoRegistry;

impl Registry for NoRegistry {
    fn string(&self, _: Hive, _: &str, _: &str) -> Option<String> {
        None
    }
}

/// A Wine prefix's registry files, parsed for their string values.
#[derive(Default)]
pub struct WineRegistry {
    /// (hive, lower-cased key, lower-cased name) to value.
    values: HashMap<(Hive, String, String), String>,
}

impl WineRegistry {
    /// Reads `system.reg` and `user.reg` in the prefix folder; missing files read as empty.
    pub fn load(prefix: &Path) -> Self {
        let mut r = WineRegistry::default();
        for (file, hive) in [
            ("system.reg", Hive::LocalMachine),
            ("user.reg", Hive::CurrentUser),
        ] {
            // Registry files of a prefix with many programs can be tens of megabytes; one
            // over 256 MB is not a registry.
            let p = prefix.join(file);
            if std::fs::metadata(&p).is_ok_and(|m| m.len() < 256 << 20) {
                if let Ok(bytes) = std::fs::read(&p) {
                    r.add(hive, &String::from_utf8_lossy(&bytes));
                }
            }
        }
        r
    }

    /// Adds the string values of one file's text.
    pub fn add(&mut self, hive: Hive, text: &str) {
        let mut key: Option<String> = None;
        for line in text.lines() {
            let line = line.trim_end_matches('\r');
            if let Some(rest) = line.strip_prefix('[') {
                key = rest
                    .rfind(']')
                    .map(|end| unescape(&rest[..end]).to_ascii_lowercase());
                continue;
            }
            let Some(k) = &key else { continue };
            let Some((name, value)) = split_value(line) else {
                continue;
            };
            self.values
                .insert((hive, k.clone(), name.to_ascii_lowercase()), value);
        }
    }
}

impl Registry for WineRegistry {
    fn string(&self, hive: Hive, key: &str, name: &str) -> Option<String> {
        self.values
            .get(&(hive, key.to_ascii_lowercase(), name.to_ascii_lowercase()))
            .cloned()
    }
}

/// `"name"="value"`, `@="value"` or `"name"=str(2):"value"`; other kinds of value are not
/// strings and give `None`.
fn split_value(line: &str) -> Option<(String, String)> {
    let (name, rest) = if let Some(rest) = line.strip_prefix('@') {
        (String::new(), rest)
    } else {
        let body = line.strip_prefix('"')?;
        let end = closing_quote(body)?;
        (unescape(&body[..end]), body.get(end + 1..)?)
    };
    let rest = rest.strip_prefix('=')?;
    let rest = match rest.strip_prefix("str(") {
        Some(r) => r.split_once("):")?.1,
        None => rest,
    };
    let body = rest.strip_prefix('"')?;
    let end = closing_quote(body)?;
    Some((name, unescape(&body[..end])))
}

/// The byte index of the first unescaped `"`.
fn closing_quote(s: &str) -> Option<usize> {
    let mut escaped = false;
    for (i, c) in s.char_indices() {
        match c {
            _ if escaped => escaped = false,
            '\\' => escaped = true,
            '"' => return Some(i),
            _ => {}
        }
    }
    None
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('0') => {}
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

/// The machine's own registry.
#[cfg(windows)]
pub struct WindowsRegistry;

#[cfg(windows)]
impl Registry for WindowsRegistry {
    fn string(&self, hive: Hive, key: &str, name: &str) -> Option<String> {
        use winreg::RegKey;
        use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_64KEY};
        let root = RegKey::predef(match hive {
            Hive::LocalMachine => HKEY_LOCAL_MACHINE,
            Hive::CurrentUser => HKEY_CURRENT_USER,
        });
        root.open_subkey_with_flags(key, KEY_READ | KEY_WOW64_64KEY)
            .ok()?
            .get_value::<String, _>(name)
            .ok()
    }
}

/// The registry of the machine this runs on: the real one on Windows, none elsewhere.
pub fn native() -> Box<dyn Registry> {
    #[cfg(windows)]
    {
        Box::new(WindowsRegistry)
    }
    #[cfg(not(windows))]
    {
        Box::new(NoRegistry)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SYSTEM: &str = r#"WINE REGISTRY Version 2
;; All keys relative to \\Machine

[Software\\Wow6432Node\\GOG.com\\Games\\1949616134] 1700000000
#time=1da0000000000
"gameName"="Dragon Age: Origins - Ultimate Edition"
"path"="C:\\GOG Games\\Dragon Age Ultimate Edition"
"quoted"="say \"hi\""
"num"=dword:00000001
"expand"=str(2):"%ProgramFiles%\\X"
@="default"
"#;

    #[test]
    fn reads_wine_strings() {
        let mut r = WineRegistry::default();
        r.add(Hive::LocalMachine, SYSTEM);
        assert_eq!(
            software(&r, Hive::LocalMachine, r"GOG.com\Games\1949616134", "PATH").as_deref(),
            Some(r"C:\GOG Games\Dragon Age Ultimate Edition")
        );
        let key = r"Software\Wow6432Node\GOG.com\Games\1949616134";
        assert_eq!(
            r.string(Hive::LocalMachine, key, "quoted").as_deref(),
            Some("say \"hi\"")
        );
        assert_eq!(r.string(Hive::LocalMachine, key, "num"), None);
        assert_eq!(
            r.string(Hive::LocalMachine, key, "expand").as_deref(),
            Some(r"%ProgramFiles%\X")
        );
        assert_eq!(
            r.string(Hive::LocalMachine, key, "").as_deref(),
            Some("default")
        );
        assert_eq!(r.string(Hive::CurrentUser, key, "path"), None);
    }

    #[test]
    fn junk_does_not_panic() {
        let mut r = WineRegistry::default();
        for t in [
            "[",
            "[a",
            "\"",
            "\"a\"=",
            "\"a\"=\"",
            "@=str(",
            "[k]\n\"\\",
            "[k]\n@=\"\\",
        ] {
            r.add(Hive::LocalMachine, t);
        }
    }
}
