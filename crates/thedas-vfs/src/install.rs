//! Discovery of a Dragon Age: Origins install and its default resource stack.
//!
//! The layer order below is provisional. It follows the order xoreos uses (core package, then
//! the single-player campaign, each with data, abilities, textures, patches and override), plus
//! DLC packages ordered by the `Priority` attribute in their manifests. It has not yet been
//! checked against what the original executable does; that check belongs to the parity work.

use std::path::{Path, PathBuf};

use crate::path::{join_ci, resolve_ci};
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
    /// The manifest's `ExtendedModuleUID`: the campaign this add-in adds to (`Single Player`
    /// for the base game's own DLC, `DAO_PRC_EP_1` for Awakening's), or empty or absent for
    /// an add-in that is a campaign of its own.
    pub extends: Option<String>,
    pub dir: PathBuf,
}

impl AddIn {
    /// Whether the add-in belongs to the base campaign, and so whether any of its folders
    /// (`core` and `module`) join the base campaign's resources.
    ///
    /// The original registers add-in folders at the end of a campaign load (0x5ef460, traced
    /// by the mods stream): an add-in record whose campaign match fails (the flag test at
    /// 0x5ef6f0) is skipped whole (0x5f0269), registering no folder at all.
    ///
    /// Provisional: that the string matched against the campaign is the manifest's
    /// `ExtendedModuleUID` is inferred from the shipped manifests (the stand-alone campaigns
    /// leave it empty, and their folders reuse base names such as `default_player.utc`), not
    /// read in the executable; a second registration function near 0x5f4b5a is unread. An
    /// add-in with no such attribute counts as not extending the base campaign.
    pub fn extends_base_campaign(&self) -> bool {
        self.extends
            .as_deref()
            .is_some_and(|m| m.trim().eq_ignore_ascii_case(BASE_CAMPAIGN_UID))
    }
}

/// The base campaign's module UID, as add-in manifests name it.
const BASE_CAMPAIGN_UID: &str = "Single Player";

/// Which game an install holds. Both use the same folder layout; what differs here is the
/// base campaign's module folder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Game {
    Origins,
    DragonAge2,
}

impl Game {
    /// The base campaign's folder under `modules/`.
    pub fn campaign_module(self) -> &'static str {
        match self {
            Game::Origins => "single player",
            Game::DragonAge2 => "campaign_base",
        }
    }
}

#[derive(Clone, Debug)]
pub struct DaoInstall {
    root: PathBuf,
}

impl DaoInstall {
    /// Opens an install root: the directory holding `packages/` and `modules/`.
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().to_path_buf();
        if !join_ci(&root, "packages/core").is_dir() {
            return Err(Error::Malformed {
                path: root,
                reason: "not a Dragon Age install (no packages/core)".into(),
            });
        }
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The game this install holds: Dragon Age II when it has `modules/campaign_base`,
    /// Dragon Age: Origins otherwise.
    pub fn game(&self) -> Game {
        let da2 = format!("modules/{}", Game::DragonAge2.campaign_module());
        if join_ci(&self.root, &da2).is_dir() {
            Game::DragonAge2
        } else {
            Game::Origins
        }
    }

    /// A path relative to the install root, for labels and reports. Always uses `/` as the
    /// separator, so labels read the same on every platform.
    pub fn relative<'a>(&self, path: &'a Path) -> std::borrow::Cow<'a, str> {
        let rel = path
            .strip_prefix(&self.root)
            .unwrap_or(path)
            .to_string_lossy();
        if std::path::MAIN_SEPARATOR == '/' {
            rel
        } else {
            rel.replace(std::path::MAIN_SEPARATOR, "/").into()
        }
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
        let dir = join_ci(&self.root, "Addins");
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
            let manifest = resolve_ci(&path, "manifest.xml").filter(|p| p.is_file());
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
            let extends = xml_attr(&text, "AddInItem", "ExtendedModuleUID");
            out.push(AddIn {
                uid,
                priority,
                enabled,
                extends,
                dir: path,
            });
        }
        out.sort_by(|a, b| a.priority.cmp(&b.priority).then_with(|| a.uid.cmp(&b.uid)));
        Ok(out)
    }

    /// The global resource stack: core package, single-player campaign, then enabled DLC.
    /// An enabled add-in's `core` and `module` folders are mounted only when the add-in
    /// extends the base campaign ([`AddIn::extends_base_campaign`], provisional); an add-in
    /// for another campaign contributes nothing, as in the original, so a stand-alone DLC
    /// campaign's own resources cannot replace the base game's.
    /// Area (`env`) and module archives are loaded per area at runtime, so they are not here.
    pub fn global_vfs(&self, quality: TextureQuality) -> Result<Vfs> {
        let mut vfs = Vfs::new();
        self.push_package(&mut vfs, &join_ci(&self.root, "packages/core"), quality)?;
        self.push_package(
            &mut vfs,
            &join_ci(
                &self.root,
                &format!("modules/{}", self.game().campaign_module()),
            ),
            quality,
        )?;
        for addin in self.addins()?.into_iter().filter(|a| a.enabled) {
            if addin.extends_base_campaign() {
                self.push_package(&mut vfs, &join_ci(&addin.dir, "core"), quality)?;
                self.push_package(&mut vfs, &join_ci(&addin.dir, "module"), quality)?;
            }
        }
        Ok(vfs)
    }

    /// [`DaoInstall::global_vfs`] plus this player's mods as the stack's overlay
    /// ([`crate::mods::mount_default`]): what a game session reads through. Tools that measure
    /// the install itself (the corpus) use `global_vfs`.
    pub fn play_vfs(&self, quality: TextureQuality) -> Result<Vfs> {
        let mut vfs = self.global_vfs(quality)?;
        crate::mods::mount_default(&mut vfs);
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
            let d = join_ci(dir, sub);
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
            for archive in archives_in(&join_ci(dir, sub))? {
                let l = label(&archive);
                vfs.push_archive(&archive, l)?;
            }
        }
        let over = join_ci(dir, "override");
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
        let mut dirs = vec![join_ci(&self.root, "packages/core/env")];
        let mut addins: Vec<AddIn> = self.addins()?.into_iter().filter(|a| a.enabled).collect();
        addins.reverse();
        for a in addins {
            dirs.push(join_ci(&a.dir, "core/env"));
            dirs.push(join_ci(&a.dir, "module/env"));
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
        // Data names levels in any case (a City Elf area's layout is `Den201d`, the folder
        // `den201d`), as Windows matched them; so the files and folder are matched likewise.
        let file = |dir: &Path, ext: &str| {
            resolve_ci(dir, &format!("{level}.{ext}")).filter(|p| p.is_file())
        };
        let level_dir = |dir: &Path| resolve_ci(dir, level).filter(|p| p.is_dir());
        let env_dir = self
            .env_dirs()?
            .into_iter()
            .find(|d| file(d, "arl").is_some() || level_dir(d).is_some())
            .ok_or_else(|| Error::NotFound(format!("level {level}")))?;
        let mark = vfs.mark();
        let files: Vec<PathBuf> = ["arl", "opf"]
            .iter()
            .filter_map(|ext| file(&env_dir, ext))
            .collect();
        let dir = level_dir(&env_dir).unwrap_or_else(|| env_dir.join(level));
        let label = |p: &Path| self.relative(p).into_owned();
        vfs.push_files(&files, label(&dir));
        let mut archives = archives_in(&dir)?;
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

    /// Folder and file names in another case than the data uses, as the Windows original
    /// accepts. Passes on case-sensitive and case-insensitive volumes alike (run the tests with
    /// `TMPDIR` on a case-sensitive volume to exercise the listing path).
    #[test]
    fn finds_folders_levels_and_manifests_in_any_case() {
        let root = std::env::temp_dir().join(format!("thedas-install-ci-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let env = root.join("Packages/Core/ENV");
        std::fs::create_dir_all(env.join("den201d")).unwrap();
        std::fs::create_dir_all(root.join("Packages/Core/Data")).unwrap();
        std::fs::write(
            root.join("Packages/Core/Data/global.erf"),
            erf_v20(&[("shared.dds", b"global")]),
        )
        .unwrap();
        std::fs::write(env.join("den201d.ARL"), b"arl").unwrap();
        std::fs::write(env.join("Den201D.opf"), b"opf").unwrap();
        std::fs::write(
            env.join("den201d/den201d.rim"),
            erf_v20(&[("den201d_0.rml", b"rml")]),
        )
        .unwrap();
        let addin = root.join("ADDINS/Some_DLC");
        std::fs::create_dir_all(addin.join("Core/Env")).unwrap();
        std::fs::write(
            addin.join("MANIFEST.XML"),
            br#"<AddInItem UID="some_dlc" Priority="5" Enabled="1">"#,
        )
        .unwrap();

        let install = DaoInstall::open(&root).unwrap();
        let addins = install.addins().unwrap();
        assert_eq!(addins.len(), 1);
        assert_eq!(
            (addins[0].uid.as_str(), addins[0].priority),
            ("some_dlc", 5)
        );
        // Core's env, then the DLC's core/env (found) and module/env (absent).
        let dirs = install.env_dirs().unwrap();
        assert_eq!(dirs.len(), 3);
        assert!(dirs[0].is_dir() && dirs[1].is_dir() && !dirs[2].exists());

        let mut vfs = install.global_vfs(TextureQuality::High).unwrap();
        assert_eq!(vfs.read("shared.dds").unwrap().0, b"global");
        let mount = install.mount_level(&mut vfs, "Den201d").unwrap();
        assert_eq!(mount.files.len(), 2);
        assert_eq!(mount.archives.len(), 1);
        assert_eq!(vfs.read("den201d.arl").unwrap().0, b"arl");
        assert_eq!(vfs.read("den201d.opf").unwrap().0, b"opf");
        assert_eq!(vfs.read("den201d_0.rml").unwrap().0, b"rml");
        mount.unmount(&mut vfs);
        assert!(install.mount_level(&mut vfs, "DEN201D").is_ok());
        assert!(install.mount_level(&mut vfs, "den201").is_err());
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// An add-in for another campaign mounts nothing, neither `core` nor `module`, so it
    /// cannot replace base resources; an add-in that extends the base campaign mounts both.
    #[test]
    fn only_base_campaign_addins_mount_their_folders() {
        let root = std::env::temp_dir().join(format!("thedas-addins-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let write = |rel: &str, bytes: &[u8]| {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, bytes).unwrap();
        };
        let manifest = |uid: &str, extends: Option<&str>, priority: u32| {
            let extends = extends.map_or(String::new(), |m| format!(r#" ExtendedModuleUID="{m}""#));
            format!(
                r#"<Manifest Type="AddIn"><AddInsList><AddInItem UID="{uid}"{extends} Priority="{priority}" Enabled="1">"#
            )
        };
        write(
            "packages/core/data/base.erf",
            &erf_v20(&[("hero.utc", b"base"), ("sword.uti", b"base")]),
        );
        // Its own campaign, at the highest priority: `module` and `core` both stay out.
        write(
            "Addins/own/Manifest.xml",
            manifest("OWN", Some(""), 900).as_bytes(),
        );
        write(
            "Addins/own/module/data/own.erf",
            &erf_v20(&[("hero.utc", b"own campaign"), ("own_only.utc", b"x")]),
        );
        write(
            "Addins/own/core/data/own_core.erf",
            &erf_v20(&[("own_core.uti", b"own core"), ("hero.utc", b"own core")]),
        );
        // Another campaign's add-in, and one whose manifest names none.
        write(
            "Addins/ep/Manifest.xml",
            manifest("EP", Some("DAO_PRC_EP_1"), 800).as_bytes(),
        );
        write(
            "Addins/ep/module/data/ep.erf",
            &erf_v20(&[("hero.utc", b"expansion")]),
        );
        write(
            "Addins/bare/Manifest.xml",
            manifest("BARE", None, 700).as_bytes(),
        );
        write(
            "Addins/bare/module/data/bare.erf",
            &erf_v20(&[("hero.utc", b"bare")]),
        );
        // The base campaign's own add-in, in another case: both its folders are mounted.
        write(
            "Addins/sp/Manifest.xml",
            manifest("SP", Some("single player"), 10).as_bytes(),
        );
        write(
            "Addins/sp/module/data/sp.erf",
            &erf_v20(&[("sword.uti", b"dlc sword"), ("sp_only.utc", b"y")]),
        );
        write(
            "Addins/sp/core/data/sp_core.erf",
            &erf_v20(&[("sp_core.uti", b"sp core")]),
        );

        let install = DaoInstall::open(&root).unwrap();
        let addins = install.addins().unwrap();
        let extends: Vec<(&str, bool)> = addins
            .iter()
            .map(|a| (a.uid.as_str(), a.extends_base_campaign()))
            .collect();
        assert_eq!(
            extends,
            [("SP", true), ("BARE", false), ("EP", false), ("OWN", false)]
        );
        let vfs = install.global_vfs(TextureQuality::High).unwrap();
        assert_eq!(vfs.read("hero.utc").unwrap().0, b"base");
        assert!(vfs.lookup("own_only.utc").is_none());
        assert!(vfs.lookup("own_core.uti").is_none());
        assert_eq!(vfs.read("sp_core.uti").unwrap().0, b"sp core");
        assert_eq!(vfs.read("sword.uti").unwrap().0, b"dlc sword");
        assert!(vfs.lookup("sp_only.utc").is_some());
        std::fs::remove_dir_all(&root).unwrap();
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

    #[test]
    fn a_dragon_age_2_install_mounts_campaign_base() {
        let root = std::env::temp_dir().join(format!("thedas-install-da2-{}", std::process::id()));
        std::fs::create_dir_all(root.join("packages/core/data")).unwrap();
        let origins = DaoInstall::open(&root).unwrap();
        assert_eq!(origins.game(), Game::Origins);
        assert_eq!(origins.game().campaign_module(), "single player");

        let data = root.join("modules/campaign_base/data");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("campaign.txt"), b"da2").unwrap();
        let install = DaoInstall::open(&root).unwrap();
        assert_eq!(install.game(), Game::DragonAge2);
        let vfs = install.global_vfs(TextureQuality::High).unwrap();
        let (bytes, prov) = vfs.read("campaign.txt").unwrap();
        assert_eq!(bytes, b"da2");
        assert_eq!(prov.layer_label, "modules/campaign_base/data");
        std::fs::remove_dir_all(&root).ok();
    }
}
