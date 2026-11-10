//! Discovery of a Dragon Age: Origins install and its default resource stack.
//!
//! The layer order below is provisional. It follows the order xoreos uses (core package, then
//! the single-player campaign, each with data, abilities, textures, patches and override), plus
//! DLC packages ordered by the `Priority` attribute in their manifests. It has not yet been
//! checked against what the original executable does; that check belongs to the parity work.

use std::path::{Path, PathBuf};

use crate::vfs::{Vfs, collect_files, is_archive};
use crate::{Error, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TextureQuality {
    #[default]
    High,
    Medium,
}

impl TextureQuality {
    fn dir(self) -> &'static str {
        match self {
            TextureQuality::High => "high",
            TextureQuality::Medium => "medium",
        }
    }
}

/// A DLC package found under `Addins/`.
#[derive(Clone, Debug)]
pub struct AddIn {
    pub uid: String,
    pub priority: i64,
    pub enabled: bool,
    pub dir: PathBuf,
}

#[derive(Clone, Debug)]
pub struct DaoInstall {
    root: PathBuf,
}

impl DaoInstall {
    /// Opens an install root: the directory holding `packages/` and `modules/`.
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().to_path_buf();
        if !root.join("packages").join("core").is_dir() {
            return Err(Error::Malformed {
                path: root,
                reason: "not a Dragon Age: Origins install (no packages/core)".into(),
            });
        }
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// A path relative to the install root, for labels and reports.
    pub fn relative<'a>(&self, path: &'a Path) -> std::borrow::Cow<'a, str> {
        path.strip_prefix(&self.root)
            .unwrap_or(path)
            .to_string_lossy()
    }

    /// Every ERF and RIM file anywhere in the install, sorted by path.
    pub fn all_archives(&self) -> Result<Vec<PathBuf>> {
        let mut files = Vec::new();
        collect_files(&self.root, true, &mut files)?;
        files.retain(|p| is_archive(p));
        files.sort();
        Ok(files)
    }

    /// DLC packages, ordered by manifest priority (lowest first), then UID.
    pub fn addins(&self) -> Result<Vec<AddIn>> {
        let dir = self.root.join("Addins");
        let mut out = Vec::new();
        if !dir.is_dir() {
            return Ok(out);
        }
        let rd = std::fs::read_dir(&dir).map_err(|e| Error::Io {
            path: dir.clone(),
            source: e,
        })?;
        for entry in rd.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let manifest = ["Manifest.xml", "manifest.xml"]
                .iter()
                .map(|m| path.join(m))
                .find(|p| p.is_file());
            let Some(manifest) = manifest else { continue };
            let text = std::fs::read(&manifest).map_err(|e| Error::Io {
                path: manifest.clone(),
                source: e,
            })?;
            let text = String::from_utf8_lossy(&text);
            let uid = xml_attr(&text, "AddInItem", "UID")
                .unwrap_or_else(|| path.file_name().unwrap().to_string_lossy().into_owned());
            let priority = xml_attr(&text, "AddInItem", "Priority")
                .and_then(|p| p.parse().ok())
                .unwrap_or(0);
            let enabled = xml_attr(&text, "AddInItem", "Enabled").as_deref() != Some("0");
            out.push(AddIn {
                uid,
                priority,
                enabled,
                dir: path,
            });
        }
        out.sort_by(|a, b| a.priority.cmp(&b.priority).then_with(|| a.uid.cmp(&b.uid)));
        Ok(out)
    }

    /// The global resource stack: core package, single-player campaign, then enabled DLC.
    /// Area (`env`) and module archives are loaded per area at runtime, so they are not here.
    pub fn global_vfs(&self, quality: TextureQuality) -> Result<Vfs> {
        let mut vfs = Vfs::new();
        self.push_package(&mut vfs, &self.root.join("packages/core"), quality)?;
        self.push_package(&mut vfs, &self.root.join("modules/single player"), quality)?;
        for addin in self.addins()?.into_iter().filter(|a| a.enabled) {
            self.push_package(&mut vfs, &addin.dir.join("core"), quality)?;
            self.push_package(&mut vfs, &addin.dir.join("module"), quality)?;
        }
        Ok(vfs)
    }

    fn push_package(&self, vfs: &mut Vfs, dir: &Path, quality: TextureQuality) -> Result<()> {
        if !dir.is_dir() {
            return Ok(());
        }
        let label = |p: &Path| self.relative(p).into_owned();
        // Loose files first (lowest), as xoreos does.
        for (sub, recursive) in [
            ("data", false),
            ("data/abilities", false),
            ("data/movies", false),
            ("data/talktables", false),
            ("data/cursors", false),
            ("textures", true),
            ("audio", true),
            ("patch", false),
        ] {
            let d = dir.join(sub);
            if d.is_dir() {
                vfs.push_directory(&d, recursive, label(&d))?;
            }
        }
        for sub in [
            "data",
            "data/abilities",
            &format!("textures/{}", quality.dir()),
            "patch",
        ] {
            for archive in archives_in(&dir.join(sub))? {
                let l = label(&archive);
                vfs.push_archive(&archive, l)?;
            }
        }
        let over = dir.join("override");
        if over.is_dir() {
            vfs.push_directory(&over, true, label(&over))?;
        }
        Ok(())
    }
}

fn archives_in(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    if dir.is_dir() {
        collect_files(dir, false, &mut files)?;
    }
    files.retain(|p| is_archive(p));
    files.sort();
    Ok(files)
}

/// Pulls one attribute off the first occurrence of an element. Enough for DLC manifests,
/// which are machine-written and flat; not a general XML parser.
fn xml_attr(text: &str, element: &str, attr: &str) -> Option<String> {
    let start = text.find(&format!("<{element}"))?;
    let rest = &text[start..];
    let end = rest.find('>')?;
    let tag = &rest[..end];
    let needle = format!(" {attr}=\"");
    let a = tag.find(&needle)? + needle.len();
    let len = tag[a..].find('"')?;
    Some(tag[a..a + len].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_manifest_attributes() {
        let xml = r#"<Manifest Type="AddIn"><AddInsList>
            <AddInItem UID="DAO_PRC_STR" Name="DAO_PRC_STR" ExtendedModuleUID="" Priority="100" Enabled="1">"#;
        assert_eq!(
            xml_attr(xml, "AddInItem", "UID").as_deref(),
            Some("DAO_PRC_STR")
        );
        assert_eq!(
            xml_attr(xml, "AddInItem", "Priority").as_deref(),
            Some("100")
        );
        assert_eq!(xml_attr(xml, "AddInItem", "Missing"), None);
        assert_eq!(xml_attr(xml, "Nope", "UID"), None);
    }
}
