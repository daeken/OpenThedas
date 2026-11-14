//! Discovery of a Dragon Age: Origins install and its default resource stack.
//!
//! The order of the packages follows the priorities the original executable registers its
//! folders with (lowest number wins there; traced, see `docs/gaps/formats.md` FMT-006):
//! `packages/core/override` in the player's documents folder (0) and in the install (1), add-ins
//! (9000 and below), the player's own copies of the campaign and core folders (20000 to
//! 20007), the campaign (50000s), core (100000s). The order inside one package (loose files,
//! archives, patch, override) follows the Origins source priorities below; Dragon Age II
//! retains xoreos's order. Add-ins follow their registry's order (see `sort_addins`).

use std::path::{Path, PathBuf};

use crate::path::{join_ci, resolve_ci};
use crate::vfs::{Vfs, collect_files, container_key, is_archive};
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
    /// Its place in the registry (`AddIns.xml`) that named it, counted from the first item;
    /// `None` for an add-in known only from its folder's manifest.
    pub rank: Option<usize>,
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

/// The base campaign's module folder of the install at `root` (`single player` for Dragon
/// Age: Origins, `campaign_base` for Dragon Age II): what the tools load when no module is
/// named. An install that does not open counts as Origins.
pub fn campaign_module(root: &Path) -> &'static str {
    DaoInstall::open(root)
        .map(|i| i.game())
        .unwrap_or(Game::Origins)
        .campaign_module()
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

    /// The number the original holds a registry item's `GameVersion` against: the first two
    /// dot-separated parts of its executable's `FileVersion` string, read as a number
    /// (`1.05.13263.0` gives 1.05). Traced: the gate at 0x5ede19 asks its platform object
    /// (`Eclipse::PlatformWin32`, virtual slot +0x5c, function 0x4142c0) for the version, and
    /// that function reads the `\StringFileInfo\<lang>\FileVersion` string of its own module's
    /// version resource; the gate then cuts it at the second `.` (0x5ede33..0x5ede5b).
    ///
    /// `None`, which skips the gate, when the install has no `bin_ship/DAOrigins.exe` or the
    /// string cannot be found in it, and for Dragon Age II: without the executable there is
    /// nothing to hold an add-in against, and dropping add-ins on a guess would be worse.
    pub fn game_version(&self) -> Option<f64> {
        if self.game() != Game::Origins {
            return None;
        }
        let exe = resolve_ci(&self.root, "bin_ship/DAOrigins.exe").filter(|p| p.is_file())?;
        let bytes = std::fs::read(exe).ok()?;
        let version = file_version_string(&bytes)?;
        let mut parts = version.splitn(3, '.');
        let two = match (parts.next(), parts.next()) {
            (Some(major), Some(minor)) => format!("{major}.{minor}"),
            (Some(major), None) => major.to_string(),
            _ => return None,
        };
        Some(leading_number(&two))
    }

    /// DLC packages, in the order the stack takes them (lowest first; see `sort_addins`).
    ///
    /// The original keeps one registry of add-ins, `AddIns.xml`, and many shipped add-ins (the
    /// promotional items) have no manifest of their own in their folder. So an add-in is
    /// known from the install's copy of the registry when it has one (GOG's installer keeps
    /// it at `__support/userdocs/AddIns.xml` and copies it to the player's documents), and
    /// from its folder's `Manifest.xml` otherwise.
    pub fn addins(&self) -> Result<Vec<AddIn>> {
        self.addins_with(&[])
    }

    /// [`DaoInstall::addins`] with other copies of the registry consulted first (the one in
    /// the player's documents, `Settings/AddIns.xml`): the first registry to name an add-in
    /// describes it, enabled or not.
    fn addins_with(&self, registries: &[PathBuf]) -> Result<Vec<AddIn>> {
        let dir = join_ci(&self.root, "Addins");
        let mut out: Vec<AddIn> = Vec::new();
        let install_registry = join_ci(&self.root, "__support/userdocs/AddIns.xml");
        let game_version = self.game_version();
        for registry in registries.iter().chain([&install_registry]) {
            let Ok(bytes) = std::fs::read(registry) else {
                continue;
            };
            for (item, left_out) in registry_items(&String::from_utf8_lossy(&bytes), game_version) {
                let known = out.iter().any(|a| a.uid.eq_ignore_ascii_case(&item.uid));
                let folder = resolve_ci(&dir, &item.uid).filter(|d| d.is_dir());
                if let (false, Some(folder)) = (known, folder) {
                    if let Some(why) = left_out {
                        log_left_out(&item.uid, &why);
                    }
                    out.push(AddIn {
                        dir: folder,
                        rank: Some(out.len()),
                        ..item
                    });
                }
            }
        }
        for addin in addins_in(&dir)? {
            let known = |a: &AddIn| a.dir == addin.dir || a.uid.eq_ignore_ascii_case(&addin.uid);
            if !out.iter().any(known) {
                out.push(addin);
            }
        }
        sort_addins(&mut out);
        Ok(out)
    }

    /// Offers (`Offers/`): the store's stand-ins for DLC, each an add-in-shaped package with
    /// `core` and `module` folders, in the order the original's records hold them (the
    /// registry's first item first).
    ///
    /// Traced (the offer manager, `OfferManager.cpp`): `LoadOffers` (from 0x4a3f8c) reads
    /// `offers.xml` from the player's settings folder (`OPTIONS:`; GOG's installer keeps a copy
    /// at `__support/userdocs/Offers.xml`, as it does `AddIns.xml`), one record per
    /// `OfferItem`, and looks for each one's folder under `OFFERS:`. Its `GameVersion` is
    /// held against the game's as an add-in's is (0x497570 sets record byte +0x1a8). Then:
    /// - 0x498dc0 registers every offer's `core` folder (and its `textures/high` or `medium`)
    ///   at priority 199997, with no campaign test, when +0x1a8 is set;
    /// - 0x499a30, called with the add-in `module` registration when a campaign loads (with
    ///   1, at 0x5fdf0e) and unloads (with 0, at 0x5fc235), registers the `module` folder's
    ///   campaign aliases (`CAMP_DATA`, `CAMP_ENV`, `CAMP_AUDIO_SOUND`, `CAMP_AUDIO_VO`,
    ///   `CAMP_LOCALE`, `CAMP_PATCH`, `CAMP_OVERRIDE`) of each offer whose
    ///   `ExtendedModuleUID` matches the campaign and whose +0x1a8 is set, walking the records
    ///   from last to first with priorities from 199998 down by 7; record byte +0x1a6 only
    ///   remembers that it did (cleared on unload).
    ///
    /// Nothing asks whether the player owns the DLC an offer sells: the offer's own scripts do
    /// (`GetPRCEnabled`, see [`Vfs::addin_enabled`]). Lowest number wins, so offers lie below
    /// the core package and supply only names nothing else has.
    ///
    /// Provisional: offers no registry names are taken from their folders' `manifest.xml`,
    /// after the registry's (as for add-ins); the `OFFERS:` alias is taken to be the install's
    /// `Offers` folder only; the record's `Type` (0, 1 or 2 register `core`; every shipped
    /// offer has 1) is not applied.
    pub fn offers(&self) -> Result<Vec<AddIn>> {
        self.offers_with(&[])
    }

    /// [`DaoInstall::offers`] with other copies of the registry consulted first (the
    /// player's `Settings/Offers.xml`).
    fn offers_with(&self, registries: &[PathBuf]) -> Result<Vec<AddIn>> {
        let dir = join_ci(&self.root, "Offers");
        let mut out: Vec<AddIn> = Vec::new();
        if !dir.is_dir() {
            return Ok(out);
        }
        let install_registry = join_ci(&self.root, "__support/userdocs/Offers.xml");
        let game_version = self.game_version();
        let known = |out: &[AddIn], uid: &str| out.iter().any(|a| a.uid.eq_ignore_ascii_case(uid));
        for registry in registries.iter().chain([&install_registry]) {
            let Ok(bytes) = std::fs::read(registry) else {
                continue;
            };
            let text = String::from_utf8_lossy(&bytes);
            for (item, left_out) in items_of(&text, "OfferItem", game_version) {
                let folder = resolve_ci(&dir, &item.uid).filter(|d| d.is_dir());
                if let (false, Some(folder)) = (known(&out, &item.uid), folder) {
                    if let Some(why) = left_out {
                        log_left_out(&item.uid, &why);
                    }
                    out.push(AddIn {
                        dir: folder,
                        rank: Some(out.len()),
                        ..item
                    });
                }
            }
        }
        let mut folders: Vec<PathBuf> = std::fs::read_dir(&dir)
            .map_err(|e| Error::Io {
                path: dir.clone(),
                source: e,
            })?
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        folders.sort();
        for folder in folders {
            if out.iter().any(|a| a.dir == folder) {
                continue;
            }
            let manifest = resolve_ci(&folder, "manifest.xml").filter(|p| p.is_file());
            let Some(text) = manifest.and_then(|m| std::fs::read(m).ok()) else {
                continue;
            };
            let text = String::from_utf8_lossy(&text);
            if let Some((item, left_out)) = items_of(&text, "OfferItem", game_version)
                .into_iter()
                .next()
            {
                if known(&out, &item.uid) {
                    continue;
                }
                if let Some(why) = left_out {
                    log_left_out(&item.uid, &why);
                }
                out.push(AddIn {
                    dir: folder,
                    rank: None,
                    ..item
                });
            }
        }
        Ok(out)
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
        self.stack(quality, None, Vec::new())
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
        let packaged = crate::mods::default_list_addins();
        let mut vfs = self.stack(quality, user.as_deref(), packaged)?;
        crate::mods::mount_default(&mut vfs, user.as_deref());
        crate::mods::log_player_layers(&vfs);
        // Mod mode's live edits (the workspace) apply to Origins' play stacks only.
        if self.game() == Game::Origins {
            vfs.play_from();
        }
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
    /// add-in, and the player's add-ins among the install's. `packaged` are add-ins unpacked
    /// from `.dazip` files of the mod list ([`crate::mods::unpack_dazip`]); they join the
    /// others by manifest priority, as if installed.
    fn stack(
        &self,
        quality: TextureQuality,
        user: Option<&Path>,
        packaged: Vec<AddIn>,
    ) -> Result<Vfs> {
        let game = self.game();
        let campaign = format!("modules/{}", game.campaign_module());
        let mut vfs = Vfs::new();
        let core = join_ci(&self.root, "packages/core");
        let install = |p: &Path| self.relative(p).into_owned();
        // Offers first: their priorities (199997 and up) are under the core package's. The
        // registry's first offer is pushed last, so it wins between offers.
        let offer_registries: Vec<PathBuf> = user
            .map(|u| join_ci(u, "Settings/Offers.xml"))
            .into_iter()
            .collect();
        for offer in self.offers_with(&offer_registries)?.iter().rev() {
            if !offer.enabled {
                continue;
            }
            if offer.extends_base_campaign() {
                push_package(
                    &mut vfs,
                    &join_ci(&offer.dir, "module"),
                    quality,
                    game,
                    true,
                    &install,
                )?;
            }
            // 0x498dc0 registers `core`'s data, patch and textures, no override.
            push_package(
                &mut vfs,
                &join_ci(&offer.dir, "core"),
                quality,
                game,
                false,
                &install,
            )?;
        }
        // Layers from the player's folder are labelled `user:<path under it>`, so tools can
        // tell them from the install's and a label never carries the player's home path.
        let player = |p: &Path| {
            let rel = user.and_then(|u| p.strip_prefix(u).ok()).unwrap_or(p);
            let rel = rel
                .to_string_lossy()
                .replace(std::path::MAIN_SEPARATOR, "/");
            format!("{}{rel}", crate::mods::USER_LABEL)
        };
        push_package(&mut vfs, &core, quality, game, false, &install)?;
        push_package(
            &mut vfs,
            &join_ci(&self.root, &campaign),
            quality,
            game,
            true,
            &install,
        )?;
        if let Some(user) = user {
            let user_core = join_ci(user, "packages/core");
            push_package(&mut vfs, &user_core, quality, game, false, &player)?;
            push_package(
                &mut vfs,
                &join_ci(user, &campaign),
                quality,
                game,
                true,
                &player,
            )?;
        }
        let registries: Vec<PathBuf> = user
            .map(|u| join_ci(u, "Settings/AddIns.xml"))
            .into_iter()
            .collect();
        let mut addins = self.addins_with(&registries)?;
        if let Some(user) = user {
            addins.extend(self.user_addins(user)?);
        }
        addins.extend(packaged);
        sort_addins(&mut addins);
        let from_user = |p: &Path| user.is_some_and(|u| p.starts_with(u));
        let addins: Vec<AddIn> = addins.into_iter().filter(|a| a.enabled).collect();
        vfs.set_addins(addins.iter().map(|a| a.uid.clone()).collect());
        for addin in addins {
            let label: &dyn Fn(&Path) -> String = if addin.dir.starts_with(&self.root) {
                &install
            } else if from_user(&addin.dir) {
                &player
            } else {
                &crate::mods::cache_label
            };
            push_package(
                &mut vfs,
                &join_ci(&addin.dir, "core"),
                quality,
                game,
                true,
                label,
            )?;
            if addin.extends_base_campaign() {
                push_package(
                    &mut vfs,
                    &join_ci(&addin.dir, "module"),
                    quality,
                    game,
                    true,
                    label,
                )?;
            }
        }
        let over = join_ci(&core, "override");
        if over.is_dir() {
            let mut overlay = Vfs::new();
            let label = |p: &Path| self.relative(p).into_owned();
            for problem in overlay.push_folder(&over, &label)? {
                log::warn!("install override folder: {problem}");
            }
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
    game: Game,
    with_override: bool,
    label: &dyn Fn(&Path) -> String,
) -> Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    if game == Game::Origins {
        push_origins_package(vfs, dir, quality, label)?;
    } else {
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
    }
    let over = join_ci(dir, "override");
    if with_override && over.is_dir() {
        for problem in vfs.push_folder(&over, label)? {
            log::warn!("override folder: {problem}");
        }
    }
    Ok(())
}

/// One package folder's layers in the original's order (Dragon Age: Origins), lowest first.
///
/// Traced (priorities in `notes/resource-priority-trace.md`; the folder indexer 0x4cf690 and
/// the container order 0x4e6660): the package's folders are sources at their own priority
/// numbers, lowest number winning: `override`, then `patch`, `locale`, `env`, `textures`,
/// `data`, then audio. A folder registered to hold archives (`data`, `patch`, `textures`...)
/// contributes its loose files as a source of rank 1 and each `.erf` in it as a source of a
/// higher rank at the same priority, and of two sources of one priority the lower rank wins:
/// a loose file beats an archive of the same folder, and archives of one folder go by path,
/// the greater path winning. So here: audio, then `data` (its archives, then its loose
/// files, including every subfolder's loose files in source-path order),
/// then `textures`, then `patch`.
///
/// The `data` source is the rank-1 directory factory (0x4d0150). Its scan registers each
/// child directory with the parent's priority (0x4cf89c..0x4cf977), recursively; the greater
/// UTF-16 folder path wins, just as in an override folder. The separate final flag controls
/// change notifications (0x4cfdd0), not whether subfolders are scanned.
///
/// Provisional: `.rim` archives here are still eagerly mounted; the original records their
/// names for explicit loading instead (0x4c7b00). Nested `.erf` discovery and the original's
/// `toolset`/`resmetrics` scan exclusions are not yet reproduced by this package loader.
fn push_origins_package(
    vfs: &mut Vfs,
    dir: &Path,
    quality: TextureQuality,
    label: &dyn Fn(&Path) -> String,
) -> Result<()> {
    let loose = |vfs: &mut Vfs, sub: &str, recursive: bool| -> Result<()> {
        let d = join_ci(dir, sub);
        if d.is_dir() {
            vfs.push_directory(&d, recursive, label(&d))?;
        }
        Ok(())
    };
    let archives = |vfs: &mut Vfs, sub: &str| -> Result<()> {
        for archive in archives_in(&join_ci(dir, sub))? {
            let l = label(&archive);
            vfs.push_archive(&archive, l)?;
        }
        Ok(())
    };
    loose(vfs, "audio", true)?;
    archives(vfs, "data")?;
    archives(vfs, "data/abilities")?;
    let data = join_ci(dir, "data");
    if data.is_dir() {
        let mut files = Vec::new();
        collect_files(&data, true, &mut files)?;
        let mut folders: Vec<PathBuf> = files
            .iter()
            .filter_map(|p| p.parent().map(Path::to_path_buf))
            .chain([data.clone()])
            .collect();
        folders.sort_by_cached_key(|p| container_key(&data, p, true));
        folders.dedup();
        // Keep each folder a separate source, so GDA worksheet listing also follows the
        // folder precedence, including when two differently named worksheets share row ids.
        for folder in folders {
            vfs.push_directory(&folder, false, label(&folder))?;
        }
    }
    archives(vfs, &format!("textures/{}", quality.dir()))?;
    loose(vfs, "textures", true)?;
    archives(vfs, "patch")?;
    loose(vfs, "patch", false)?;
    Ok(())
}

/// Orders add-ins as the stack takes them, lowest first.
///
/// The original keeps its add-in records in the order its registry lists them: the parser
/// appends each item to the record vector (`push_back` at 0x5f8b70, called from 0x5fa6da) and
/// nothing sorts it; `Priority` is copied to the record (+0x154) and read once, for a
/// `Format="0"` item (0x5f4074), never to order records. Both registration functions walk
/// the vector from its last record to its first, handing out priority numbers from 9000
/// downwards, lowest number winning: so **the first item in the registry beats every later
/// one**. Ours: registry add-ins in reverse registry order, so the first is pushed last.
///
/// Provisional: add-ins no registry names (a folder with only a manifest, one from the
/// player's `AddIns` folder, one unpacked from a `.dazip`) go below every registry add-in,
/// ordered among themselves by manifest `Priority` then UID. The original would have them in
/// its registry, appended when installed, which is also below the shipped ones.
fn sort_addins(addins: &mut [AddIn]) {
    addins.sort_by(|a, b| match (a.rank, b.rank) {
        (Some(x), Some(y)) => y.cmp(&x),
        (None, Some(_)) => std::cmp::Ordering::Less,
        (Some(_), None) => std::cmp::Ordering::Greater,
        (None, None) => a.priority.cmp(&b.priority).then_with(|| a.uid.cmp(&b.uid)),
    });
}

/// Every `AddInItem` of a manifest (or registry), in file order, as written (no gates
/// applied, no folders).
pub(crate) fn manifest_items(text: &str) -> Vec<AddIn> {
    registry_items(text, None)
        .into_iter()
        .map(|(mut a, _)| {
            a.enabled = true;
            a
        })
        .collect()
}

/// An add-in described by a manifest's first `AddInItem`. `fallback_uid` names it when the
/// manifest gives no UID.
pub(crate) fn addin_from_manifest(text: &str, dir: PathBuf, fallback_uid: String) -> AddIn {
    item_from(text, "AddInItem", dir, fallback_uid)
}

/// An add-in or offer described by the first `element` (`AddInItem` or `OfferItem`) of a
/// manifest or registry.
fn item_from(text: &str, element: &str, dir: PathBuf, fallback_uid: String) -> AddIn {
    AddIn {
        uid: xml_attr(text, element, "UID").unwrap_or(fallback_uid),
        priority: xml_attr(text, element, "Priority")
            .and_then(|p| p.parse().ok())
            .unwrap_or(0),
        enabled: xml_attr(text, element, "Enabled").as_deref() != Some("0"),
        extends: xml_attr(text, element, "ExtendedModuleUID"),
        dir,
        rank: None,
    }
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
        let fallback = path.file_name().unwrap().to_string_lossy().into_owned();
        out.push(addin_from_manifest(
            &String::from_utf8_lossy(&text),
            path,
            fallback,
        ));
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
    /// enabled add-ins in the stack's order (the one that wins first). With mods on (as
    /// [`DaoInstall::play_vfs`] decides), the player's add-ins join them: those in the
    /// original's documents folder and those packaged in the mod list's `.dazip` files, so
    /// a mod's new areas are found. Provisional, like the global order.
    pub fn env_dirs(&self) -> Result<Vec<PathBuf>> {
        let mut player = Vec::new();
        if crate::mods::default_list_file().is_some() {
            if let Some(user) = crate::mods::original_user_dir(self.game()) {
                player.extend(self.user_addins(&user).unwrap_or_default());
            }
            player.extend(crate::mods::default_list_addins());
        }
        self.env_dirs_with(player)
    }

    /// [`DaoInstall::env_dirs`] with `player`'s add-ins among the install's.
    pub fn env_dirs_with(&self, player: Vec<AddIn>) -> Result<Vec<PathBuf>> {
        let mut dirs = vec![join_ci(&self.root, "packages/core/env")];
        let mut addins: Vec<AddIn> = self.addins()?;
        addins.extend(player);
        let mut addins: Vec<AddIn> = addins.into_iter().filter(|a| a.enabled).collect();
        sort_addins(&mut addins);
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

/// The `AddInItem`s of an `AddIns.xml` registry, in file order, without their folders. An
/// item with no `UID` is skipped.
///
/// `enabled` is what the original's registration tests, as far as the registry decides it
/// (traced: the parser at 0x5faa34 and the record fill at 0x5f3b21..0x5f3f6f): the `Enabled`
/// attribute must be non-zero (record byte +0x15d), and the `<Type>` child must be 1 or 2
/// (record dword +0x150; 0x5f4610 registers an add-in's folders for nothing else, and an item
/// without the element reads as 0). `State` is copied to the record (+0x170) but neither
/// registration function tests it, so it is ignored here too.
///
/// Not applied, on purpose: `RequiresAuthorization` (+0x15e) needs the record's "authorized"
/// byte (+0x15f), which the original sets from its online entitlement check; we have none,
/// and an add-in on the player's disk is taken as theirs (GOG's registry carries no such
/// attribute).
///
/// The `<GameVersion>` child is a gate too (record byte +0x177: set at 0x5ede12, cleared at
/// 0x5edea4 when the number read from the item's `GameVersion` is greater than the number read
/// from the running game's version): an add-in made for a newer game is not registered.
/// `game_version` is the game's side of that comparison ([`DaoInstall::game_version`]); `None`
/// skips the gate.
///
/// Each item comes with the reason it is left out, when it is.
fn registry_items(text: &str, game_version: Option<f64>) -> Vec<(AddIn, Option<String>)> {
    items_of(text, "AddInItem", game_version)
}

/// [`registry_items`] for items of either registry: `AddInItem` (add-ins, with the `Type`
/// gate) or `OfferItem` (offers, which have no `Enabled` attribute and whose `Type` decides
/// only which folders the original registers, not whether: see [`DaoInstall::offers`]).
fn items_of(text: &str, element: &str, game_version: Option<f64>) -> Vec<(AddIn, Option<String>)> {
    let open = format!("<{element}");
    let close_tag = format!("</{element}>");
    let offers = element == "OfferItem";
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(&open) {
        rest = &rest[start..];
        let end = rest.find('>').map_or(rest.len(), |e| e + 1);
        let tag = &rest[..end];
        rest = &rest[end..];
        // The item's children run to its closing tag, or to the next item, whichever is first.
        let body = if tag.ends_with("/>") {
            ""
        } else {
            let close = rest.find(&close_tag).unwrap_or(rest.len());
            &rest[..rest.find(&open).map_or(close, |next| close.min(next))]
        };
        let kind = body.find("<Type>").and_then(|a| {
            let value = &body[a + "<Type>".len()..];
            value[..value.find('<')?].trim().parse::<i64>().ok()
        });
        let needs = body.find("<GameVersion>").map_or(0.0, |a| {
            let value = &body[a + "<GameVersion>".len()..];
            leading_number(&value[..value.find('<').unwrap_or(value.len())])
        });
        let mut addin = item_from(tag, element, PathBuf::new(), String::new());
        let why = if !addin.enabled {
            Some("its registry entry says Enabled=\"0\"".to_string())
        } else if !offers && !matches!(kind, Some(1 | 2)) {
            Some(match kind {
                Some(k) => format!("its registry entry has Type {k}, not 1 or 2"),
                None => "its registry entry has no Type".to_string(),
            })
        } else {
            game_version
                .filter(|game| needs > *game)
                .map(|game| format!("it asks for game version {needs}, and this game is {game}"))
        };
        addin.enabled = why.is_none();
        if !addin.uid.is_empty() {
            out.push((addin, why));
        }
    }
    out
}

/// Says once per process that an add-in on disk is not mounted, and why.
fn log_left_out(uid: &str, why: &str) {
    static SAID: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
    let Ok(mut said) = SAID.lock() else { return };
    if !said.iter().any(|u| u == uid) {
        said.push(uid.to_string());
        log::info!("add-ins: {uid} is installed but not mounted: {why}");
    }
}

/// The `FileVersion` string of a Windows executable's version resource (`1.05.13263.0`),
/// found by its UTF-16 key: the value follows the key and its padding. `None` when there is
/// no such key or no text after it.
fn file_version_string(exe: &[u8]) -> Option<String> {
    let key: Vec<u8> = "FileVersion\0"
        .encode_utf16()
        .flat_map(|u| u.to_le_bytes())
        .collect();
    let mut from = 0;
    while let Some(at) = exe[from..].windows(key.len()).position(|w| w == key) {
        let mut i = from + at + key.len();
        // The value is aligned to four bytes; padding is zero.
        while exe.get(i..i + 2) == Some(&[0, 0]) && i < from + at + key.len() + 4 {
            i += 2;
        }
        let units: Vec<u16> = exe[i..]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .take_while(|&u| u != 0)
            .take(64)
            .collect();
        let text = String::from_utf16_lossy(&units);
        if text.starts_with(|c: char| c.is_ascii_digit()) {
            return Some(text);
        }
        from += at + key.len();
    }
    None
}

/// The number a C `wcstod` would read off the start of a string: leading spaces, an optional
/// sign, digits with at most one point; 0 when there are no digits there (`V1.03` is 0).
fn leading_number(s: &str) -> f64 {
    let s = s.trim_start();
    let mut end = 0;
    let mut seen_point = false;
    let mut digits = 0;
    for (i, c) in s.char_indices() {
        match c {
            '+' | '-' if i == 0 => {}
            '0'..='9' => digits += 1,
            '.' if !seen_point => seen_point = true,
            _ => break,
        }
        end = i + c.len_utf8();
    }
    if digits == 0 {
        return 0.0;
    }
    s[..end].trim_end_matches('.').parse().unwrap_or(0.0)
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

    /// Inside one package the original's order: `patch` over `textures` over `data`, and in
    /// each folder the loose files over its archives, the greater archive path winning.
    #[test]
    fn a_package_is_ordered_as_the_original_orders_it() {
        let root = std::env::temp_dir().join(format!("thedas-package-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let write = |rel: &str, bytes: &[u8]| {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, bytes).unwrap();
        };
        let core = "packages/core";
        write(
            &format!("{core}/data/d1.erf"),
            &erf_v20(&[
                ("a.gda", b"data d1"),
                ("b.gda", b"data d1"),
                ("c.gda", b"data d1"),
            ]),
        );
        write(
            &format!("{core}/data/d2.erf"),
            &erf_v20(&[("a.gda", b"data d2")]),
        );
        write(&format!("{core}/data/b.gda"), b"data loose");
        write(&format!("{core}/data/c.gda"), b"data loose");
        write(
            &format!("{core}/patch/p0.erf"),
            &erf_v20(&[("c.gda", b"patch p0"), ("d.gda", b"patch p0")]),
        );
        write(&format!("{core}/patch/c.gda"), b"patch loose");
        let install = DaoInstall::open(&root).unwrap();
        let vfs = install.global_vfs(TextureQuality::High).unwrap();
        let read = |n: &str| String::from_utf8(vfs.read(n).unwrap().0).unwrap();
        assert_eq!(read("a.gda"), "data d2", "the greater archive path wins");
        assert_eq!(
            read("b.gda"),
            "data loose",
            "loose beats the folder's archives"
        );
        assert_eq!(
            read("c.gda"),
            "patch loose",
            "patch beats data; loose beats archive"
        );
        assert_eq!(read("d.gda"), "patch p0");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn data_subfolders_share_the_data_priority_and_use_source_path_order() {
        let root = std::env::temp_dir().join(format!("thedas-data-folders-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let data = root.join("packages/core/data");
        for sub in ["", "cursors", "talktables", "Z", "a/deep"] {
            std::fs::create_dir_all(data.join(sub)).unwrap();
            std::fs::write(data.join(sub).join("shared.txt"), sub.as_bytes()).unwrap();
        }
        std::fs::write(data.join("a/deep/only_deep.txt"), b"nested").unwrap();
        let textures = root.join("packages/core/textures");
        std::fs::create_dir_all(&textures).unwrap();
        std::fs::write(textures.join("only_deep.txt"), b"textures").unwrap();
        let vfs = DaoInstall::open(&root)
            .unwrap()
            .global_vfs(TextureQuality::High)
            .unwrap();
        assert_eq!(vfs.read("shared.txt").unwrap().0, b"talktables");
        assert_eq!(vfs.read("only_deep.txt").unwrap().0, b"textures");
        assert!(
            vfs.hits("only_deep.txt")
                .iter()
                .any(|p| p.file.ends_with("a/deep/only_deep.txt"))
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// An override folder as the original scans it: every subfolder is a source of its own
    /// and every `.erf` an archive, all at one priority; loose files beat archives, and of two
    /// folders or two archives the one with the greater path (case counts) wins.
    #[test]
    fn an_override_folder_is_ordered_as_the_original_orders_it() {
        let root = std::env::temp_dir().join(format!("thedas-override-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let write = |rel: &str, bytes: &[u8]| {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, bytes).unwrap();
        };
        write(
            "packages/core/data/core.erf",
            &erf_v20(&[("core.gda", b"core")]),
        );
        let over = "packages/core/override";
        // Loose files: the root's, and two subfolders' (`b` is greater than `a`).
        write(&format!("{over}/both.gda"), b"root loose");
        write(&format!("{over}/a/both.gda"), b"a loose");
        write(&format!("{over}/b/both.gda"), b"b loose");
        write(&format!("{over}/a/only_a.gda"), b"a only");
        // Archives: one in the root, one in each subfolder. An archive never beats a loose
        // file, whatever its path; between archives the greater path wins.
        write(
            &format!("{over}/0.erf"),
            &erf_v20(&[
                ("both.gda", b"root erf"),
                ("arch.gda", b"root erf"),
                ("core.gda", b"root erf"),
            ]),
        );
        write(
            &format!("{over}/a/a.erf"),
            &erf_v20(&[("arch.gda", b"a erf"), ("only_erf_a.gda", b"a erf")]),
        );
        write(
            &format!("{over}/b/b.erf"),
            &erf_v20(&[("arch.gda", b"b erf")]),
        );
        // Case counts: `Z` is below `a` as UTF-16 code units (a case-blind order puts it above).
        write(
            &format!("{over}/Z/up.erf"),
            &erf_v20(&[("case.gda", b"Z erf")]),
        );
        write(
            &format!("{over}/a/low.erf"),
            &erf_v20(&[("case.gda", b"a erf")]),
        );
        let install = DaoInstall::open(&root).unwrap();
        let vfs = install.global_vfs(TextureQuality::High).unwrap();
        let read = |n: &str| String::from_utf8(vfs.read(n).unwrap().0).unwrap();
        // Subfolders beat the root, the greater subfolder beats the lesser, loose beats erf.
        assert_eq!(read("both.gda"), "b loose");
        assert_eq!(read("only_a.gda"), "a only");
        // Archives: the greater path wins (`\\b\\b.erf` over `\\a\\a.erf` over `\\0.erf`, since
        // the root's `\zzz.erf` is below every subfolder's `\x\...`).
        assert_eq!(read("arch.gda"), "b erf");
        assert_eq!(read("only_erf_a.gda"), "a erf");
        // An override archive beats the core package's.
        assert_eq!(read("core.gda"), "root erf");
        // Case counts: `\a\low.erf` is above `\Z\up.erf`.
        assert_eq!(read("case.gda"), "a erf");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn scanned_rims_wait_for_an_explicit_load() {
        let root = std::env::temp_dir().join(format!("thedas-scanned-rim-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let over = root.join("override");
        std::fs::create_dir_all(over.join("nested")).unwrap();
        std::fs::write(
            over.join("neighbor.erf"),
            erf_v20(&[("shared.gda", b"erf")]),
        )
        .unwrap();
        let rim = over.join("nested/later.rim");
        std::fs::write(
            &rim,
            erf_v20(&[("shared.gda", b"rim"), ("only_rim.gda", b"rim only")]),
        )
        .unwrap();
        let mut vfs = Vfs::new();
        let label = |p: &Path| {
            p.strip_prefix(&root)
                .unwrap()
                .to_string_lossy()
                .into_owned()
        };
        assert!(vfs.push_folder(&over, &label).unwrap().is_empty());
        assert_eq!(vfs.read("shared.gda").unwrap().0, b"erf");
        assert!(vfs.lookup("only_rim.gda").is_none());
        vfs.push_archive(&rim, label(&rim)).unwrap();
        assert_eq!(vfs.read("only_rim.gda").unwrap().0, b"rim only");
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// Offers sit under the core package: each registry offer's `core`, and its `module`
    /// when it sells base-campaign content, supplies only names nothing else has; the
    /// registry's first offer wins between offers; one made for a newer game, or with no
    /// folder, is left out; an unlisted offer folder is known from its manifest. The stack
    /// also records which add-ins it mounts, for `GetPRCEnabled`.
    #[test]
    fn offers_lie_under_core_and_addins_are_recorded() {
        let root = std::env::temp_dir().join(format!("thedas-offers-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let write = |rel: &str, bytes: &[u8]| {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, bytes).unwrap();
        };
        write(
            "packages/core/data/core.erf",
            &erf_v20(&[("shared.gda", b"core")]),
        );
        write(
            "Addins/dlc/Manifest.xml",
            br#"<AddInItem UID="DAO_PRC_DLC" ExtendedModuleUID="Single Player" Enabled="1">"#,
        );
        write(
            "Addins/dlc/core/data/dlc.erf",
            &erf_v20(&[("dlc.uti", b"dlc")]),
        );
        let item = |uid: &str, extends: &str, version: &str| {
            format!(
                r#"<OfferItem UID="{uid}" ExtendedModuleUID="{extends}" Priority="1"><GameVersion>{version}</GameVersion><Type>1</Type></OfferItem>"#
            )
        };
        write(
            "__support/userdocs/Offers.xml",
            format!(
                "<OfferList>{}{}{}{}</OfferList>",
                item("O_FIRST", "Single Player", "1.0"),
                item("O_SECOND", "Single Player", "1.0"),
                item("O_NEW", "Single Player", "9.0"),
                item("O_MISSING", "Single Player", "1.0"),
            )
            .as_bytes(),
        );
        write(
            "Offers/o_first/core/data/f.erf",
            &erf_v20(&[("shared.gda", b"first"), ("both.gda", b"first")]),
        );
        write(
            "Offers/o_first/module/data/fm.erf",
            &erf_v20(&[("first_seller.ncs", b"first module")]),
        );
        write(
            "Offers/O_Second/core/data/s.erf",
            &erf_v20(&[("both.gda", b"second"), ("second.gda", b"second")]),
        );
        write(
            "Offers/o_new/core/data/n.erf",
            &erf_v20(&[("new.gda", b"x")]),
        );
        // Another campaign's offer, known only from its folder: `core` only.
        write(
            "Offers/o_ep/manifest.xml",
            item("O_EP", "DAO_PRC_EP_1", "1.0").as_bytes(),
        );
        write(
            "Offers/o_ep/core/data/e.erf",
            &erf_v20(&[("ep.gda", b"ep")]),
        );
        write(
            "Offers/o_ep/module/data/em.erf",
            &erf_v20(&[("ep_seller.ncs", b"x")]),
        );
        // The game's version, as `game_version` reads it.
        let exe: Vec<u8> = "FileVersion\u{0}1.05.13263.0\u{0}"
            .encode_utf16()
            .flat_map(|u| u.to_le_bytes())
            .collect();
        write("bin_ship/DAOrigins.exe", &exe);

        let install = DaoInstall::open(&root).unwrap();
        let offers: Vec<(String, bool, Option<usize>)> = install
            .offers()
            .unwrap()
            .into_iter()
            .map(|o| (o.uid, o.enabled, o.rank))
            .collect();
        assert_eq!(
            offers,
            [
                ("O_FIRST".into(), true, Some(0)),
                ("O_SECOND".into(), true, Some(1)),
                ("O_NEW".into(), false, Some(2)),
                ("O_EP".into(), true, None),
            ]
        );
        let vfs = install.global_vfs(TextureQuality::High).unwrap();
        assert_eq!(vfs.read("shared.gda").unwrap().0, b"core");
        assert_eq!(vfs.read("both.gda").unwrap().0, b"first");
        assert_eq!(vfs.read("second.gda").unwrap().0, b"second");
        assert_eq!(vfs.read("first_seller.ncs").unwrap().0, b"first module");
        assert_eq!(vfs.read("ep.gda").unwrap().0, b"ep");
        assert!(vfs.lookup("ep_seller.ncs").is_none());
        assert!(vfs.lookup("new.gda").is_none());
        assert!(vfs.addin_enabled("dao_prc_dlc"));
        assert!(!vfs.addin_enabled("O_FIRST"));
        assert!(!vfs.addin_enabled("DAO_PRC_OTHER"));
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
        let vfs = install
            .stack(TextureQuality::High, Some(&user), Vec::new())
            .unwrap();
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

    /// An add-in handed over from an unpacked package joins the stack like an installed one:
    /// `core` always, `module` when it extends the base campaign, placed by its priority.
    #[test]
    fn packaged_addins_join_the_installed_ones() {
        let base = std::env::temp_dir().join(format!("thedas-packaged-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let root = base.join("install");
        let write = |rel: &str, bytes: &[u8]| {
            let p = base.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, bytes).unwrap();
        };
        write(
            "install/packages/core/data/core.erf",
            &erf_v20(&[("sword.uti", b"core"), ("shield.uti", b"core")]),
        );
        write(
            "install/Addins/x/Manifest.xml",
            br#"<AddInItem UID="X" ExtendedModuleUID="Single Player" Priority="50" Enabled="1">"#,
        );
        write(
            "install/Addins/x/core/data/x.erf",
            &erf_v20(&[("sword.uti", b"installed"), ("shield.uti", b"installed")]),
        );
        write("cache/pkg/core/data/sword.uti", b"packaged");
        write("cache/pkg/module/data/area.are", b"packaged module");
        let packaged = |priority: i64, extends: &str| AddIn {
            uid: "PKG".into(),
            priority,
            enabled: true,
            extends: Some(extends.into()),
            dir: base.join("cache/pkg"),
            rank: None,
        };
        let install = DaoInstall::open(&root).unwrap();
        let read = |vfs: &Vfs, n: &str| String::from_utf8(vfs.read(n).unwrap().0).unwrap();

        let stack = |a: AddIn| install.stack(TextureQuality::High, None, vec![a]).unwrap();
        let vfs = stack(packaged(60, "Single Player"));
        assert_eq!(read(&vfs, "sword.uti"), "packaged");
        assert_eq!(read(&vfs, "shield.uti"), "installed");
        assert_eq!(read(&vfs, "area.are"), "packaged module");
        // A lower priority than the installed add-in's: under it.
        let vfs = stack(packaged(40, "Single Player"));
        assert_eq!(read(&vfs, "sword.uti"), "installed");
        // Another campaign's: `core` still joins, `module` stays out.
        let vfs = stack(packaged(60, "Some Other Campaign"));
        assert_eq!(read(&vfs, "sword.uti"), "packaged");
        assert!(vfs.lookup("area.are").is_none());
        std::fs::remove_dir_all(&base).unwrap();
    }

    /// Add-ins the registry names are found without a manifest of their own; the registry in
    /// the player's documents is asked before the install's, and a folder's manifest only
    /// for add-ins no registry names.
    #[test]
    fn the_registry_names_addins_that_have_no_manifest() {
        let base = std::env::temp_dir().join(format!("thedas-registry-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let root = base.join("install");
        let write = |rel: &str, bytes: &[u8]| {
            let p = base.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, bytes).unwrap();
        };
        write(
            "install/packages/core/data/core.erf",
            &erf_v20(&[("a.uti", b"core")]),
        );
        // Two add-ins with no manifest, one with, one the registry names but is not there.
        write(
            "install/Addins/promo_one/core/data/p1.erf",
            &erf_v20(&[("p1.uti", b"promo one"), ("shared.uti", b"promo one")]),
        );
        write(
            "install/Addins/promo_one/module/data/p1m.erf",
            &erf_v20(&[("p1.plo", b"plot")]),
        );
        write(
            "install/Addins/Promo_Two/core/data/p2.erf",
            &erf_v20(&[("p2.uti", b"promo two"), ("shared.uti", b"promo two")]),
        );
        write(
            "install/Addins/Promo_Two/module/data/p2m.erf",
            &erf_v20(&[("p2.plo", b"plot")]),
        );
        write(
            "install/Addins/with_manifest/core/data/m.erf",
            &erf_v20(&[("m.uti", b"manifest"), ("shared.uti", b"manifest")]),
        );
        // Named by the registry but switched off, with no type, and with a type the original
        // registers nothing for: none of them is mounted.
        for (dir, name) in [
            ("switched_off", "off"),
            ("no_type", "n"),
            ("type_three", "t"),
            ("too_new", "new"),
        ] {
            let rel = format!("install/Addins/{dir}/core/data/{name}.erf");
            write(&rel, &erf_v20(&[(&format!("{name}.uti"), b"x")]));
        }
        write(
            "install/Addins/with_manifest/Manifest.xml",
            br#"<AddInItem UID="WITH_MANIFEST" ExtendedModuleUID="Single Player" Priority="9" Enabled="1">"#,
        );
        // The game's own version, for the `GameVersion` gate.
        write(
            "install/bin_ship/DAOrigins.exe",
            &exe_with_version("1.05.13263.0"),
        );
        write(
            "install/__support/userdocs/AddIns.xml",
            br#"<?xml version="1.0"?><AddInsList>
  <AddInItem UID="PROMO_ONE" Name="One" ExtendedModuleUID="Single Player" Priority="1" Enabled="1" State="0">
    <Title DefaultText="One"><en-us><![CDATA[One > Two]]></en-us></Title>
    <GameVersion>V1.03</GameVersion>
    <Type>1</Type>
  </AddInItem>
  <AddInItem UID="PROMO_TWO" ExtendedModuleUID="Another Campaign" Priority="5" Enabled="1" >
    <Type> 2 </Type>
  </AddInItem>
  <AddInItem UID="NOT_INSTALLED" ExtendedModuleUID="Single Player" Priority="2" Enabled="1">
    <Type>1</Type>
  </AddInItem>
  <AddInItem UID="WITH_MANIFEST" ExtendedModuleUID="Single Player" Priority="3" Enabled="1" RequiresAuthorization="1">
    <Type>1</Type>
  </AddInItem>
  <AddInItem UID="SWITCHED_OFF" ExtendedModuleUID="Single Player" Priority="4" Enabled="0">
    <Type>1</Type>
  </AddInItem>
  <AddInItem UID="NO_TYPE" ExtendedModuleUID="Single Player" Priority="4" Enabled="1">
  </AddInItem>
  <AddInItem UID="TOO_NEW" ExtendedModuleUID="Single Player" Priority="4" Enabled="1">
    <GameVersion>1.06</GameVersion>
    <Type>1</Type>
  </AddInItem>
  <AddInItem UID="TYPE_THREE" ExtendedModuleUID="Single Player" Priority="4" Enabled="1">
    <Type>3</Type>
  </AddInItem>
</AddInsList>"#,
        );
        let install = DaoInstall::open(&root).unwrap();
        let all = install.addins().unwrap();
        let off: Vec<&str> = all
            .iter()
            .filter(|a| !a.enabled)
            .map(|a| a.uid.as_str())
            .collect();
        // Lowest in the stack first: the registry's order backwards.
        assert_eq!(off, ["TYPE_THREE", "TOO_NEW", "NO_TYPE", "SWITCHED_OFF"]);
        let list: Vec<(String, i64)> = all
            .iter()
            .filter(|a| a.enabled)
            .map(|a| (a.uid.clone(), a.priority))
            .collect();
        // The registry's priority for the one that also has a manifest (3, not 9), and the
        // registry's first item last, so on top: `Priority` does not order them.
        assert_eq!(
            list,
            [
                ("WITH_MANIFEST".to_string(), 3),
                ("PROMO_TWO".to_string(), 5),
                ("PROMO_ONE".to_string(), 1)
            ]
        );
        let vfs = install.global_vfs(TextureQuality::High).unwrap();
        assert_eq!(vfs.read("p1.uti").unwrap().0, b"promo one");
        assert!(vfs.lookup("p1.plo").is_some());
        // The registry's first item owns a file three add-ins provide.
        assert_eq!(vfs.read("shared.uti").unwrap().0, b"promo one");
        // Another campaign's: `core` only.
        assert_eq!(vfs.read("p2.uti").unwrap().0, b"promo two");
        assert!(vfs.lookup("p2.plo").is_none());
        // `RequiresAuthorization` does not keep an add-in out, nor does `State`.
        assert_eq!(vfs.read("m.uti").unwrap().0, b"manifest");
        for gone in ["off.uti", "n.uti", "t.uti", "new.uti"] {
            assert!(vfs.lookup(gone).is_none(), "{gone}");
        }

        // The player's own registry comes first: here it switches one add-in off.
        write(
            "docs/Settings/AddIns.xml",
            br#"<AddInsList><AddInItem UID="promo_one" ExtendedModuleUID="Single Player" Priority="1" Enabled="0"><Type>1</Type></AddInItem></AddInsList>"#,
        );
        let vfs = install
            .stack(TextureQuality::High, Some(&base.join("docs")), Vec::new())
            .unwrap();
        assert!(vfs.lookup("p1.uti").is_none());
        assert!(vfs.lookup("p2.uti").is_some());
        // An empty registry there changes nothing.
        write("docs/Settings/AddIns.xml", b"<AddInsList />");
        let vfs = install
            .stack(TextureQuality::High, Some(&base.join("docs")), Vec::new())
            .unwrap();
        assert!(vfs.lookup("p1.uti").is_some());
        std::fs::remove_dir_all(&base).unwrap();
    }

    /// A stand-in for an executable: bytes with a version resource's `FileVersion` entry.
    fn exe_with_version(version: &str) -> Vec<u8> {
        let utf16 =
            |s: &str| -> Vec<u8> { s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect() };
        let mut out = b"MZ junk before".to_vec();
        out.extend(utf16("\\VarFileInfo\\Translation\0"));
        out.extend(utf16("FileVersion\0"));
        out.extend([0, 0]);
        out.extend(utf16(version));
        out.extend([0, 0, 0, 0]);
        out.extend(utf16("InternalName\0"));
        out
    }

    #[test]
    fn the_game_version_is_the_executables_file_version() {
        assert_eq!(
            file_version_string(&exe_with_version("1.05.13263.0")).as_deref(),
            Some("1.05.13263.0")
        );
        assert_eq!(file_version_string(b"no version here"), None);
        let root = std::env::temp_dir().join(format!("thedas-exever-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("packages/core")).unwrap();
        let install = DaoInstall::open(&root).unwrap();
        // No executable: no number, and so no gate.
        assert_eq!(install.game_version(), None);
        std::fs::create_dir_all(root.join("Bin_Ship")).unwrap();
        std::fs::write(
            root.join("Bin_Ship/daorigins.EXE"),
            exe_with_version("1.05.13263.0"),
        )
        .unwrap();
        assert_eq!(install.game_version(), Some(1.05));
        std::fs::write(root.join("Bin_Ship/daorigins.EXE"), exe_with_version("1.0")).unwrap();
        assert_eq!(install.game_version(), Some(1.0));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn reads_a_leading_number_as_the_c_library_would() {
        for (text, n) in [
            ("1.04", 1.04),
            (" 1", 1.0),
            ("1.0.3", 1.0),
            ("V1.03", 0.0),
            ("", 0.0),
            ("1.", 1.0),
            ("-2.5x", -2.5),
            (".5", 0.5),
        ] {
            assert_eq!(leading_number(text), n, "{text:?}");
        }
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
        assert_eq!(campaign_module(&root), "campaign_base");
        assert_eq!(campaign_module(&root.join("missing")), "single player");
        let vfs = install.global_vfs(TextureQuality::High).unwrap();
        let (bytes, prov) = vfs.read("campaign.txt").unwrap();
        assert_eq!(bytes, b"da2");
        assert_eq!(prov.layer_label, "modules/campaign_base/data");
        std::fs::remove_dir_all(&root).ok();
    }
}
