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

/// A level's own resources, mounted on top of a stack while its area is loaded.
#[derive(Clone, Debug)]
pub struct LevelMount {
    pub level: String,
    /// The `env` directory the level was found in.
    pub env_dir: PathBuf,
    /// The stack height before mounting; [`LevelMount::unmount`] truncates back to it.
    pub mark: usize,
    /// Loose files mounted (the `.arl` and `.opf`).
    pub files: Vec<PathBuf>,
    /// Archives mounted, in push order: the level rim, then room rims, then `.gpu.rim`s.
    pub archives: Vec<PathBuf>,
}

impl LevelMount {
    pub fn unmount(self, vfs: &mut Vfs) {
        vfs.truncate(self.mark);
    }
}

impl DaoInstall {
    /// The `env` directories that can hold levels, in search order: the core package, then
    /// enabled DLC packages by priority (highest first). Provisional, like the global order.
    pub fn env_dirs(&self) -> Result<Vec<PathBuf>> {
        let mut dirs = vec![self.root.join("packages/core/env")];
        let mut addins: Vec<AddIn> = self.addins()?.into_iter().filter(|a| a.enabled).collect();
        addins.reverse();
        for a in addins {
            dirs.push(a.dir.join("core/env"));
            dirs.push(a.dir.join("module/env"));
        }
        Ok(dirs)
    }

    /// Mounts a level's resources on top of `vfs`: its loose `.arl`/`.opf`, then every archive
    /// in `env/<level>/` (the level rim with room layouts, each room's rim with baked meshes
    /// and probes, and the `.gpu.rim`s with lightmap atlases). Layer labels are paths relative
    /// to the install, so provenance shows exactly which level archive supplied a resource.
    pub fn mount_level(&self, vfs: &mut Vfs, level: &str) -> Result<LevelMount> {
        if level.is_empty()
            || level.contains(['/', '\\', ':'])
            || level.starts_with('.')
            || level.contains("..")
        {
            return Err(Error::NotFound(format!("level {level:?}")));
        }
        let env_dir = self
            .env_dirs()?
            .into_iter()
            .find(|d| d.join(format!("{level}.arl")).is_file() || d.join(level).is_dir())
            .ok_or_else(|| Error::NotFound(format!("level {level}")))?;
        let mark = vfs.mark();
        let files: Vec<PathBuf> = ["arl", "opf"]
            .iter()
            .map(|ext| env_dir.join(format!("{level}.{ext}")))
            .filter(|p| p.is_file())
            .collect();
        let label = |p: &Path| self.relative(p).into_owned();
        vfs.push_files(&files, label(&env_dir.join(level)));
        let mut archives = archives_in(&env_dir.join(level))?;
        // Plain rims first, then GPU rims; each group sorted by name.
        archives.sort_by_key(|p| {
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().to_ascii_lowercase());
            let gpu = name.as_deref().is_some_and(|n| n.ends_with(".gpu.rim"));
            (gpu, name)
        });
        for a in &archives {
            if let Err(e) = vfs.push_archive(a, label(a)) {
                vfs.truncate(mark);
                return Err(e);
            }
        }
        Ok(LevelMount {
            level: level.to_string(),
            env_dir,
            mark,
            files,
            archives,
        })
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

    /// A minimal ERF V2.0 archive with the given entries.
    fn erf_v20(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let utf16 = |s: &str, n: usize| {
            let mut v: Vec<u8> = s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
            v.resize(n, 0);
            v
        };
        let mut out = utf16("ERF V2.0", 16);
        out.extend((entries.len() as u32).to_le_bytes());
        out.extend(109u32.to_le_bytes());
        out.extend(1u32.to_le_bytes());
        out.extend(0xFFFF_FFFFu32.to_le_bytes());
        let mut offset = 32 + 72 * entries.len();
        for (name, data) in entries {
            out.extend(utf16(name, 64));
            out.extend((offset as u32).to_le_bytes());
            out.extend((data.len() as u32).to_le_bytes());
            offset += data.len();
        }
        for (_, data) in entries {
            out.extend_from_slice(data);
        }
        out
    }

    #[test]
    fn mounts_and_unmounts_a_level() {
        let root = std::env::temp_dir().join(format!("thedas-install-{}", std::process::id()));
        let env = root.join("packages/core/env");
        std::fs::create_dir_all(env.join("lvl001d")).unwrap();
        std::fs::create_dir_all(root.join("packages/core/data")).unwrap();
        std::fs::write(
            root.join("packages/core/data/global.erf"),
            erf_v20(&[("shared.dds", b"global"), ("model.mmh", b"m")]),
        )
        .unwrap();
        std::fs::write(env.join("lvl001d.arl"), b"arl").unwrap();
        std::fs::write(
            env.join("lvl001d/lvl001d.rim"),
            erf_v20(&[("lvl001d_0.rml", b"rml")]),
        )
        .unwrap();
        std::fs::write(
            env.join("lvl001d/lvl001d_0.gpu.rim"),
            erf_v20(&[("lvl001d_room000.dds", b"lm"), ("shared.dds", b"level")]),
        )
        .unwrap();

        let install = DaoInstall::open(&root).unwrap();
        let mut vfs = install.global_vfs(TextureQuality::High).unwrap();
        let before = vfs.layers().len();
        assert!(vfs.lookup("lvl001d_0.rml").is_none());

        let mount = install.mount_level(&mut vfs, "lvl001d").unwrap();
        assert_eq!(mount.archives.len(), 2);
        assert!(mount.archives[1].ends_with("lvl001d_0.gpu.rim"));
        let (data, prov) = vfs.read("lvl001d_0.rml").unwrap();
        assert_eq!(data, b"rml");
        assert_eq!(prov.layer_label, "packages/core/env/lvl001d/lvl001d.rim");
        assert_eq!(vfs.read("lvl001d.arl").unwrap().0, b"arl");
        assert_eq!(vfs.read("shared.dds").unwrap().0, b"level");
        assert_eq!(vfs.read("model.mmh").unwrap().0, b"m");

        mount.unmount(&mut vfs);
        assert_eq!(vfs.layers().len(), before);
        assert_eq!(vfs.read("shared.dds").unwrap().0, b"global");
        assert!(vfs.lookup("lvl001d_room000.dds").is_none());

        assert!(install.mount_level(&mut vfs, "missing").is_err());
        assert!(install.mount_level(&mut vfs, "../data").is_err());
        assert_eq!(vfs.layers().len(), before);
    }

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
