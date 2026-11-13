//! Discovery of a Dragon Age: Origins install and its default resource stack.
//!
//! The order of the packages follows the priorities the original executable registers its
//! folders with (lowest number wins there; traced, see `docs/gaps/formats.md` FMT-006):
//! `packages/core/override` in the player's documents folder (0) and in the install (1), add-ins
//! (9000 and below), the player's own copies of the campaign and core folders (20000 to
//! 20007), the campaign (50000s), core (100000s). The order inside one package (loose files,
//! archives, patch, override) is still xoreos's and provisional, as is the order between
//! add-ins (by the `Priority` attribute in their manifests).

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
    /// Whether the add-in belongs to the base campaign, and so whether its `module` folder
    /// joins the base campaign's resources (its `core` folder always does).
    ///
    /// The original registers add-in folders in two places (traced by the mods stream):
    /// - 0x5f4610, at start-up and when add-ins are enabled or disabled (it logs
    ///   `Enabled AddIn: %s.`), registers every enabled add-in's `core` folders with no
    ///   comparison against a campaign;
    /// - 0x5ef460, at the end of a campaign load, registers `module` folders, and skips an
    ///   add-in whose campaign match fails (the flag test at 0x5ef6f0, on to 0x5f0269).
    ///
    /// Provisional: that the string matched against the campaign is the manifest's
    /// `ExtendedModuleUID` is inferred from the shipped manifests (the stand-alone campaigns
    /// leave it empty, and their `module` folders reuse base names such as
    /// `default_player.utc`), not read in the executable; and 0x5f4610 is gated by flag
    /// bytes of which one (+0x16c) is not identified. An add-in with no such attribute
    /// counts as not extending the base campaign.
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
        addins_in(&join_ci(&self.root, "Addins"))
    }

    /// The install's own resource stack: core package, single-player campaign, then enabled
    /// DLC, with the install's `packages/core/override` above them all as the stack's overlay
    /// (the original's priority 1: it beats add-ins and a level's archives too).
    /// Every enabled add-in's `core` folder is mounted, as the original registers them
    /// whatever campaign plays; its `module` folder only when the add-in extends the base
    /// campaign ([`AddIn::extends_base_campaign`], provisional), so a stand-alone DLC
    /// campaign's own module resources cannot replace the base game's. Add-ins in the
    /// player's documents folder follow the same rule.
    /// Area (`env`) and module archives are loaded per area at runtime, so they are not here.
    pub fn global_vfs(&self, quality: TextureQuality) -> Result<Vfs> {
        self.stack(quality, None)
    }

    /// The stack a game session reads through: [`DaoInstall::global_vfs`] with the player's
    /// own folders from the original's documents folder ([`crate::mods::original_user_dir`])
    /// in their places, and the player's mods on top ([`crate::mods::mount_default`]). Tools
    /// that measure the install itself (the corpus) use `global_vfs`. With mods switched off
    /// (`THEDAS_MODS=none`) this is `global_vfs`.
    pub fn play_vfs(&self, quality: TextureQuality) -> Result<Vfs> {
        if crate::mods::default_list_file().is_none() {
            return self.global_vfs(quality);
        }
        let user = crate::mods::original_user_dir(self.game());
        let mut vfs = self.stack(quality, user.as_deref())?;
        crate::mods::mount_default(&mut vfs, user.as_deref());
        crate::mods::log_player_layers(&vfs);
        Ok(vfs)
    }

    /// Add-ins installed in the player's documents folder (`AddIns/` under `user`), in
    /// [`DaoInstall::addins`]' order. The original installs `.dazip` packages there.
    pub fn user_addins(&self, user: &Path) -> Result<Vec<AddIn>> {
        addins_in(&join_ci(user, "AddIns"))
    }

    /// Builds the stack, lowest layer first. `user` is the original's folder in the player's
    /// documents; its layers go where the original's priorities put them: the player's core
    /// `patch` and `data` and campaign folders above the install's campaign and below every
    /// add-in, and the player's add-ins among the install's.
    fn stack(&self, quality: TextureQuality, user: Option<&Path>) -> Result<Vfs> {
        let campaign = format!("modules/{}", self.game().campaign_module());
        let mut vfs = Vfs::new();
        let core = join_ci(&self.root, "packages/core");
        let install = |p: &Path| self.relative(p).into_owned();
        // Layers from the player's folder are labelled `user:<path under it>`, so tools can
        // tell them from the install's and a label never carries the player's home path.
        let player = |p: &Path| {
            let rel = user.and_then(|u| p.strip_prefix(u).ok()).unwrap_or(p);
            let rel = rel
                .to_string_lossy()
                .replace(std::path::MAIN_SEPARATOR, "/");
            format!("{}{rel}", crate::mods::USER_LABEL)
        };
        push_package(&mut vfs, &core, quality, false, &install)?;
        push_package(
            &mut vfs,
            &join_ci(&self.root, &campaign),
            quality,
            true,
            &install,
        )?;
        if let Some(user) = user {
            let user_core = join_ci(user, "packages/core");
            push_package(&mut vfs, &user_core, quality, false, &player)?;
            push_package(&mut vfs, &join_ci(user, &campaign), quality, true, &player)?;
        }
        let mut addins = self.addins()?;
        if let Some(user) = user {
            addins.extend(self.user_addins(user)?);
            sort_addins(&mut addins);
        }
        for addin in addins.into_iter().filter(|a| a.enabled) {
            let label: &dyn Fn(&Path) -> String = if addin.dir.starts_with(&self.root) {
                &install
            } else {
                &player
            };
            push_package(&mut vfs, &join_ci(&addin.dir, "core"), quality, true, label)?;
            if addin.extends_base_campaign() {
                push_package(
                    &mut vfs,
                    &join_ci(&addin.dir, "module"),
                    quality,
                    true,
                    label,
                )?;
            }
        }
        let over = join_ci(&core, "override");
        if over.is_dir() {
            let mut overlay = Vfs::new();
            overlay.push_directory(&over, true, self.relative(&over).into_owned())?;
            vfs.set_overlay(overlay);
        }
        Ok(vfs)
    }
}

/// Pushes one package folder's layers. `with_override` also pushes its `override` folder
/// on top; the core package's is the stack's overlay instead.
fn push_package(
    vfs: &mut Vfs,
    dir: &Path,
    quality: TextureQuality,
    with_override: bool,
    label: &dyn Fn(&Path) -> String,
) -> Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
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
    if with_override && over.is_dir() {
        vfs.push_directory(&over, true, label(&over))?;
    }
    Ok(())
}

fn sort_addins(addins: &mut [AddIn]) {
    addins.sort_by(|a, b| a.priority.cmp(&b.priority).then_with(|| a.uid.cmp(&b.uid)));
}

/// The add-ins in one `AddIns` folder, ordered by manifest priority (lowest first), then UID.
fn addins_in(dir: &Path) -> Result<Vec<AddIn>> {
    let mut out = Vec::new();
    if !dir.is_dir() {
        return Ok(out);
    }
    let rd = std::fs::read_dir(dir).map_err(|e| Error::Io {
        path: dir.to_path_buf(),
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
    sort_addins(&mut out);
    Ok(out)
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

    /// Every enabled add-in mounts its `core` folder; only one that extends the base
    /// campaign mounts its `module` folder, so another campaign's module cannot replace
    /// base resources.
    #[test]
    fn only_base_campaign_addins_mount_their_module_folder() {
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
        // Its own campaign, at the highest priority: `module` stays out, `core` is mounted.
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
            &erf_v20(&[("own_core.uti", b"own core")]),
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
        assert_eq!(vfs.read("own_core.uti").unwrap().0, b"own core");
        assert_eq!(vfs.read("sp_core.uti").unwrap().0, b"sp core");
        assert_eq!(vfs.read("sword.uti").unwrap().0, b"dlc sword");
        assert!(vfs.lookup("sp_only.utc").is_some());
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// The original's priorities: core override beats add-ins, add-ins beat the player's own
    /// campaign and core folders, those beat the install's campaign, which beats core.
    #[test]
    fn override_and_user_folders_sit_where_the_original_puts_them() {
        let base = std::env::temp_dir().join(format!("thedas-userdirs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (root, user) = (base.join("install"), base.join("docs"));
        let write = |dir: &Path, rel: &str, bytes: &[u8]| {
            let p = dir.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, bytes).unwrap();
        };
        let manifest = |uid: &str| {
            format!(
                r#"<AddInItem UID="{uid}" ExtendedModuleUID="Single Player" Priority="5" Enabled="1">"#
            )
        };
        let names = ["a", "b", "c", "d", "e", "f", "g"];
        // The source ranked r (0 highest) provides the first r + 1 names, so the winner of
        // each name is the source of that rank.
        let erf = |rank: usize, tag: &str| {
            let f: Vec<(String, &[u8])> = names[..=rank]
                .iter()
                .map(|n| (format!("{n}.txt"), tag.as_bytes()))
                .collect();
            let refs: Vec<(&str, &[u8])> = f.iter().map(|(n, d)| (n.as_str(), *d)).collect();
            erf_v20(&refs)
        };
        write(&root, "packages/core/data/core.erf", &erf(6, "core"));
        write(
            &root,
            "modules/single player/data/camp.erf",
            &erf(5, "campaign"),
        );
        write(&user, "packages/core/data/ucore.erf", &erf(4, "user core"));
        for n in &names[..=3] {
            let rel = format!("modules/Single Player/override/{n}.txt");
            write(&user, &rel, b"user campaign");
        }
        write(&root, "Addins/x/Manifest.xml", manifest("X").as_bytes());
        write(&root, "Addins/x/core/data/x.erf", &erf(2, "addin"));
        write(&user, "AddIns/y/Manifest.xml", manifest("Y").as_bytes());
        write(&user, "AddIns/y/core/data/y.erf", &erf(1, "user addin"));
        write(&root, "packages/core/override/sub/a.txt", b"core override");

        let install = DaoInstall::open(&root).unwrap();
        let read = |vfs: &Vfs, n: &str| String::from_utf8(vfs.read(n).unwrap().0).unwrap();
        let vfs = install.stack(TextureQuality::High, Some(&user)).unwrap();
        let got: Vec<String> = names
            .iter()
            .map(|n| read(&vfs, &format!("{n}.txt")))
            .collect();
        assert_eq!(
            got,
            [
                "core override",
                "user addin",
                "addin",
                "user campaign",
                "user core",
                "campaign",
                "core"
            ]
        );
        assert_eq!(vfs.hits("a.txt").len(), 7);
        // The player's layers are named apart from the install's, without the home path
        // (lowercased here: on a case-insensitive volume a folder keeps the case asked for).
        let mine: Vec<(String, usize)> = crate::mods::player_layers(&vfs)
            .into_iter()
            .filter(|l| l.files > 0)
            .map(|l| (l.label.to_ascii_lowercase(), l.files))
            .collect();
        assert_eq!(
            mine,
            [
                ("user:packages/core/data/ucore.erf".to_string(), 5),
                ("user:modules/single player/override".to_string(), 4),
                ("user:addins/y/core/data/y.erf".to_string(), 2),
            ]
        );
        // Without the player's folders: the install alone, override still on top.
        let vfs = install.global_vfs(TextureQuality::High).unwrap();
        assert_eq!(read(&vfs, "a.txt"), "core override");
        assert_eq!(read(&vfs, "b.txt"), "addin");
        assert_eq!(read(&vfs, "d.txt"), "campaign");
        assert_eq!(read(&vfs, "g.txt"), "core");
        // A level mounted later does not hide the override folder.
        assert_eq!(vfs.overlay().map(|o| o.layers().len()), Some(1));
        std::fs::remove_dir_all(&base).unwrap();
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
