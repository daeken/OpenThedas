//! Case-insensitive file-system paths.
//!
//! The original game runs on Windows, where file names are case-insensitive, so its data may
//! name a file or folder in a case other than the one on disk: a City Elf save's area names the
//! level `Den201d` while the folder is `env/den201d`. On a case-sensitive volume (Linux, or a
//! case-sensitive APFS volume on a Mac) a plain `Path::join` then misses. Every place that builds
//! a file-system path from a name it did not create itself (game data, scripts, saves, flags, or
//! the install's fixed folder names) goes through [`resolve_ci`].
//!
//! Archive entries need none of this: the resource stack already looks names up
//! case-insensitively (see [`crate::normalize_name`]).

use std::path::{Path, PathBuf};

/// Resolves `rel` under `base`, matching each component case-insensitively (ASCII), as
/// Windows would. `rel` may use `/` or `\` between components; empty components are skipped.
///
/// Each component is tried as given first, so a correctly cased path costs no directory
/// listing (and on a case-insensitive volume that is always the path returned). Only on a miss
/// is the directory listed; of the entries equal to the component ignoring ASCII case, the
/// first by name wins, so the result is the same on every run.
///
/// Returns `None` when a component does not exist in any case, a directory cannot be listed,
/// or `rel` is absolute or has a `.`, `..` or drive (`:`) component. `base` itself is used as
/// given. Never panics.
pub fn resolve_ci(base: &Path, rel: &str) -> Option<PathBuf> {
    let mut cur = base.to_path_buf();
    if rel.starts_with(['/', '\\']) {
        return None;
    }
    for comp in rel.split(['/', '\\']).filter(|c| !c.is_empty()) {
        if comp == "." || comp == ".." || comp.contains(':') {
            return None;
        }
        let exact = cur.join(comp);
        if exact.exists() {
            cur = exact;
            continue;
        }
        let found = std::fs::read_dir(&cur)
            .ok()?
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().into_string().ok()?;
                name.eq_ignore_ascii_case(comp).then_some(name)
            })
            .min()?;
        cur.push(found);
    }
    Some(cur)
}

/// [`resolve_ci`], or `base.join(rel)` as given when nothing matches: for paths a caller only
/// probes (`is_dir`, `is_file`) and reports, where a missing folder is not an error.
pub fn join_ci(base: &Path, rel: &str) -> PathBuf {
    resolve_ci(base, rel).unwrap_or_else(|| base.join(rel))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh, empty directory under `$TMPDIR` (set `TMPDIR` to a case-sensitive volume to
    /// exercise the listing path; on a case-insensitive one the exact probe answers).
    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("thedas-ci-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn resolves_each_component_in_any_case() {
        let root = scratch("any");
        std::fs::create_dir_all(root.join("Packages/Core/Env/den201d")).unwrap();
        std::fs::write(root.join("Packages/Core/Env/Den201D.arl"), b"arl").unwrap();

        let dir = resolve_ci(&root, "packages/core/env/DEN201D").unwrap();
        assert!(dir.is_dir());
        let arl = resolve_ci(&root, "PACKAGES\\core\\ENV\\den201d.ARL").unwrap();
        assert_eq!(std::fs::read(&arl).unwrap(), b"arl");
        // Correct case and doubled separators resolve too.
        assert!(resolve_ci(&root, "Packages//Core/Env/Den201D.arl").is_some());
        // An empty `rel` is `base`.
        assert_eq!(resolve_ci(&root, ""), Some(root.clone()));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn misses_and_bad_components_are_none() {
        let root = scratch("miss");
        std::fs::create_dir_all(root.join("Modules/Single Player")).unwrap();
        assert!(resolve_ci(&root, "modules/single player").is_some());
        assert_eq!(resolve_ci(&root, "modules/singleplayer"), None);
        assert_eq!(resolve_ci(&root, "modules/../modules"), None);
        assert_eq!(resolve_ci(&root, "./modules"), None);
        assert_eq!(resolve_ci(&root, "/modules"), None);
        assert_eq!(resolve_ci(&root, "c:/modules"), None);
        // A file in the middle of the path cannot be listed: no match, no panic.
        std::fs::write(root.join("Modules/readme.txt"), b"x").unwrap();
        assert_eq!(resolve_ci(&root, "modules/README.TXT/inner"), None);
        assert_eq!(resolve_ci(&root.join("nowhere"), "a"), None);
        assert_eq!(join_ci(&root, "Nowhere/x"), root.join("Nowhere/x"));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn several_case_variants_pick_the_exact_one_then_the_first() {
        let root = scratch("variants");
        std::fs::write(root.join("Level.arl"), b"upper").unwrap();
        std::fs::write(root.join("level.arl"), b"lower").unwrap();
        if std::fs::read(root.join("Level.arl")).unwrap() != b"upper" {
            // A case-insensitive volume: both names are one file, nothing to choose.
            std::fs::remove_dir_all(&root).unwrap();
            return;
        }
        let read = |rel: &str| std::fs::read(resolve_ci(&root, rel).unwrap()).unwrap();
        assert_eq!(read("level.arl"), b"lower");
        assert_eq!(read("Level.arl"), b"upper");
        // No exact match: the first name in byte order ("Level" < "level").
        assert_eq!(read("LEVEL.ARL"), b"upper");
        std::fs::remove_dir_all(&root).unwrap();
    }
}
