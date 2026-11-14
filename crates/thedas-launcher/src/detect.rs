//! Finding the player's Dragon Age installs: Steam libraries, GOG and EA registry keys,
//! common install folders on every drive, and the same inside Wine prefixes (CrossOver and
//! Whisky bottles on macOS; Wine, Lutris, Heroic and Bottles on Linux).
//!
//! Every candidate is checked as [`thedas_vfs::DaoInstall::open`] checks an install (it must
//! hold `packages/core`), and the game is told by its campaign module. Nothing is written.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use thedas_vfs::{DaoInstall, Game, resolve_ci};

use crate::registry::{self, Hive, Registry, WineRegistry};
use crate::vdf;

/// Steam app IDs: Dragon Age: Origins - Ultimate Edition, Dragon Age: Origins (the original
/// release, still owned by early buyers), Dragon Age II.
pub const STEAM_ORIGINS_UE: u32 = 47810;
pub const STEAM_ORIGINS: u32 = 17450;
pub const STEAM_DA2: u32 = 1238040;
/// GOG product ID of Dragon Age: Origins - Ultimate Edition (DA2 is not sold on GOG).
pub const GOG_ORIGINS: &str = "1949616134";

/// Where an install was found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    Steam,
    Gog,
    /// The EA app, Origin or a retail disc (the BioWare registry key).
    Ea,
    /// A folder named like the game in a usual place.
    Folder,
    /// Picked by the player.
    Manual,
}

impl Source {
    pub fn label(&self) -> &'static str {
        match self {
            Source::Steam => "Steam",
            Source::Gog => "GOG",
            Source::Ea => "EA app / retail",
            Source::Folder => "Found on disk",
            Source::Manual => "Added by hand",
        }
    }
}

/// One install found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub game: Game,
    /// The install root (holding `packages/` and `modules/`).
    pub root: PathBuf,
    pub source: Source,
    /// Which system it is in: "This computer", or a Wine prefix ("CrossOver bottle Steam").
    pub place: String,
}

/// What a checked install holds, for the launcher to show.
#[derive(Clone, Debug, PartialEq)]
pub struct Inspected {
    pub game: Game,
    pub root: PathBuf,
    /// Origins' `DAOrigins.exe` version (1.04, 1.05), when the exe is there.
    pub version: Option<f64>,
    /// The original executable (`bin_ship/DAOrigins.exe` or `bin_ship/DragonAge2.exe`), which
    /// Origins' DLC passwords are read from.
    pub exe: Option<PathBuf>,
    /// DLC add-ins the install has.
    pub addins: usize,
    /// Voice-over languages (`packages/core/audio/vo/<language>`), sorted.
    pub languages: Vec<String>,
}

/// The game at `root`, when it is an install.
pub fn game_at(root: &Path) -> Option<Game> {
    DaoInstall::open(root).ok().map(|i| i.game())
}

/// Looks an install over.
pub fn inspect(root: &Path) -> Option<Inspected> {
    let install = DaoInstall::open(root).ok()?;
    let game = install.game();
    let exe_name = match game {
        Game::Origins => "bin_ship/DAOrigins.exe",
        Game::DragonAge2 => "bin_ship/DragonAge2.exe",
    };
    let mut languages: Vec<String> = resolve_ci(root, "packages/core/audio/vo")
        .and_then(|d| std::fs::read_dir(d).ok())
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.path().is_dir())
                .map(|e| e.file_name().to_string_lossy().to_ascii_lowercase())
                .collect()
        })
        .unwrap_or_default();
    languages.sort();
    Some(Inspected {
        game,
        root: root.to_path_buf(),
        version: install.game_version(),
        exe: resolve_ci(root, exe_name).filter(|p| p.is_file()),
        addins: install.addins().map(|a| a.len()).unwrap_or(0),
        languages,
    })
}

/// The install a path the player picked belongs to: the path itself, a folder above it (they
/// picked the exe or `bin_ship`), or a folder just inside it (they picked the folder holding
/// the game's folder).
pub fn install_root(picked: &Path) -> Option<PathBuf> {
    let start = if picked.is_file() {
        picked.parent()?
    } else {
        picked
    };
    for dir in start.ancestors().take(4) {
        if game_at(dir).is_some() {
            return Some(dir.to_path_buf());
        }
    }
    let mut kids: Vec<PathBuf> = std::fs::read_dir(start)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    kids.sort();
    kids.into_iter().find(|k| game_at(k).is_some())
}

/// The operating system searched, which decides the places looked at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Os {
    Windows,
    MacOs,
    Linux,
}

impl Os {
    pub fn current() -> Os {
        if cfg!(windows) {
            Os::Windows
        } else if cfg!(target_os = "macos") {
            Os::MacOs
        } else {
            Os::Linux
        }
    }
}

/// A Windows file system: the machine itself on Windows, or a Wine prefix.
pub struct WinRoot {
    /// "This computer", or the prefix's name.
    pub place: String,
    pub registry: Box<dyn Registry>,
    /// Drive letters (lower case) and the folders they are.
    pub drives: Vec<(char, PathBuf)>,
}

impl WinRoot {
    /// A Wine prefix: `drive_c` is C:, and `dosdevices/<x>:` links give the other drives.
    pub fn wine(prefix: &Path, place: String) -> Option<WinRoot> {
        let c = prefix.join("drive_c");
        if !c.is_dir() {
            return None;
        }
        let mut drives = vec![('c', c)];
        if let Ok(rd) = std::fs::read_dir(prefix.join("dosdevices")) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().to_ascii_lowercase();
                let mut ch = name.chars();
                let (Some(letter), Some(':'), None) = (ch.next(), ch.next(), ch.next()) else {
                    continue;
                };
                // z: is the whole host file system; searching it is the host search's job.
                if !letter.is_ascii_lowercase() || letter == 'c' || letter == 'z' {
                    continue;
                }
                if e.path().is_dir() {
                    drives.push((letter, e.path()));
                }
            }
        }
        drives.sort_by_key(|(l, _)| *l);
        Some(WinRoot {
            place,
            registry: Box::new(WineRegistry::load(prefix)),
            drives,
        })
    }

    /// The machine's own drives and registry.
    pub fn native() -> WinRoot {
        let drives = ('c'..='z')
            .map(|l| (l, PathBuf::from(format!("{}:\\", l.to_ascii_uppercase()))))
            .filter(|(_, p)| p.is_dir())
            .collect();
        WinRoot {
            place: "This computer".into(),
            registry: registry::native(),
            drives,
        }
    }

    /// A Windows path (`C:\GOG Games\X`) as a path on this machine, matched case-insensitively
    /// in a prefix.
    pub fn map(&self, win: &str) -> Option<PathBuf> {
        let win = win.trim().trim_matches('"');
        let mut chars = win.chars();
        let letter = chars.next()?.to_ascii_lowercase();
        if chars.next() != Some(':') {
            return None;
        }
        let rest = win.get(2..)?.trim_start_matches(['\\', '/']);
        let (_, drive) = self.drives.iter().find(|(l, _)| *l == letter)?;
        if rest.is_empty() {
            return Some(drive.clone());
        }
        Some(resolve_ci(drive, rest).unwrap_or_else(|| drive.join(rest.replace('\\', "/"))))
    }
}

/// Collects candidates, dropping repeats of one folder.
#[derive(Default)]
struct Hits {
    seen: HashSet<PathBuf>,
    found: Vec<Found>,
}

impl Hits {
    fn offer(&mut self, root: &Path, source: Source, place: &str) {
        let Some(game) = game_at(root) else { return };
        let key = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        if !self.seen.insert(key) {
            return;
        }
        self.found.push(Found {
            game,
            root: root.to_path_buf(),
            source,
            place: place.to_string(),
        });
    }
}

/// Whether a folder's name looks like one of the games'.
fn named_like_game(name: &str) -> bool {
    let n = name.to_ascii_lowercase().replace(['_', '-', '.'], " ");
    n.contains("dragon age") || n.contains("dragonage")
}

/// Folders, relative to a drive's root, that games are usually installed in.
const WIN_BASES: &[&str] = &[
    "",
    "Games",
    "Program Files (x86)",
    "Program Files",
    "Program Files (x86)/Steam/steamapps/common",
    "Program Files/Steam/steamapps/common",
    "SteamLibrary/steamapps/common",
    "Steam/steamapps/common",
    "GOG Games",
    "GOG Galaxy/Games",
    "Program Files (x86)/GOG Galaxy/Games",
    "Program Files (x86)/GOG.com",
    "Program Files (x86)/Origin Games",
    "Program Files/Origin Games",
    "Program Files/EA Games",
    "Program Files (x86)/EA Games",
    "EA Games",
    "Program Files (x86)/BioWare",
    "Program Files/BioWare",
];

/// Children of `base` named like a game, and the folders just inside those (some installers
/// nest the game one level down).
fn scan_base(base: &Path, source: Source, place: &str, hits: &mut Hits) {
    let Ok(rd) = std::fs::read_dir(base) else {
        return;
    };
    let mut kids: Vec<PathBuf> = rd
        .flatten()
        .filter(|e| named_like_game(&e.file_name().to_string_lossy()))
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    kids.sort();
    for kid in kids {
        if game_at(&kid).is_some() {
            hits.offer(&kid, source.clone(), place);
        } else if let Some(inner) = install_root(&kid) {
            hits.offer(&inner, source.clone(), place);
        }
    }
}

/// The libraries a Steam folder lists (`steamapps/libraryfolders.vdf`, both its current form
/// and the old one where each numbered key is a path), the Steam folder first. `map` turns a
/// path written in the file into one on this machine.
pub fn steam_libraries(steam: &Path, map: &dyn Fn(&str) -> Option<PathBuf>) -> Vec<PathBuf> {
    let mut libs = vec![steam.to_path_buf()];
    let vdf_path = resolve_ci(steam, "steamapps/libraryfolders.vdf")
        .or_else(|| resolve_ci(steam, "config/libraryfolders.vdf"));
    let text = vdf_path
        .and_then(|p| std::fs::read(p).ok())
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default();
    if let Some(top) = vdf::parse(&text) {
        if let Some(lf) = top.get("libraryfolders") {
            for (k, v) in lf.pairs() {
                if !k.chars().all(|c| c.is_ascii_digit()) {
                    continue;
                }
                let path = match v {
                    vdf::Vdf::Str(s) => Some(s.as_str()),
                    vdf::Vdf::Block(_) => v.get("path").and_then(|p| p.str()),
                };
                if let Some(p) = path.and_then(map) {
                    if !libs.contains(&p) {
                        libs.push(p);
                    }
                }
            }
        }
    }
    libs
}

/// The folder of Steam app `id` in a library, from its `appmanifest_<id>.acf`.
pub fn steam_app_dir(library: &Path, id: u32) -> Option<PathBuf> {
    let acf = resolve_ci(library, &format!("steamapps/appmanifest_{id}.acf"))?;
    let text = String::from_utf8_lossy(&std::fs::read(acf).ok()?).into_owned();
    let top = vdf::parse(&text)?;
    let dir = top.get("AppState")?.get("installdir")?.str()?.to_string();
    if dir.is_empty() || dir.contains("..") || dir.contains(['/', '\\', ':']) {
        return None;
    }
    resolve_ci(library, &format!("steamapps/common/{dir}"))
}

fn search_steam(steam: &Path, map: &dyn Fn(&str) -> Option<PathBuf>, place: &str, hits: &mut Hits) {
    for lib in steam_libraries(steam, map) {
        for id in [STEAM_ORIGINS_UE, STEAM_ORIGINS, STEAM_DA2] {
            if let Some(dir) = steam_app_dir(&lib, id) {
                hits.offer(&dir, Source::Steam, place);
            }
        }
        if let Some(common) = resolve_ci(&lib, "steamapps/common") {
            scan_base(&common, Source::Steam, place, hits);
        }
    }
}

/// Searches one Windows file system: its registry, its Steam, and the usual folders.
pub fn search_windows(root: &WinRoot, hits_out: &mut Vec<Found>) {
    let mut hits = Hits::default();
    for f in hits_out.iter() {
        hits.seen
            .insert(std::fs::canonicalize(&f.root).unwrap_or_else(|_| f.root.clone()));
    }
    let reg = root.registry.as_ref();
    let place = root.place.as_str();
    let map = |s: &str| root.map(s);

    // Steam: where the registry says, and its default folders.
    let mut steams: Vec<PathBuf> = [
        reg.string(Hive::CurrentUser, r"Software\Valve\Steam", "SteamPath"),
        registry::software(reg, Hive::LocalMachine, r"Valve\Steam", "InstallPath"),
    ]
    .into_iter()
    .flatten()
    .filter_map(|s| root.map(&s))
    .collect();
    for d in [r"C:\Program Files (x86)\Steam", r"C:\Program Files\Steam"] {
        if let Some(p) = root.map(d) {
            steams.push(p);
        }
    }
    let mut seen_steam = HashSet::new();
    for s in steams {
        let key = std::fs::canonicalize(&s).unwrap_or_else(|_| s.clone());
        if s.is_dir() && seen_steam.insert(key) {
            search_steam(&s, &map, place, &mut hits);
        }
    }

    // GOG Galaxy and GOG's offline installers both write this key. Provisional: from
    // install reports and other tools, not traced in the installer.
    for name in ["path", "exe"] {
        if let Some(p) = registry::software(
            reg,
            Hive::LocalMachine,
            &format!(r"GOG.com\Games\{GOG_ORIGINS}"),
            name,
        )
        .and_then(|s| root.map(&s))
        {
            let p = if p.is_file() {
                p.parent().map(Path::to_path_buf)
            } else {
                Some(p)
            };
            if let Some(r) = p.and_then(|p| install_root(&p)) {
                hits.offer(&r, Source::Gog, place);
            }
        }
    }

    // The BioWare key the retail, Origin and EA app installers write. Provisional: the value
    // names come from install reports, not from tracing the installers.
    for game_key in [
        r"BioWare\Dragon Age",
        r"BioWare\Dragon Age 2",
        r"BioWare\Dragon Age II",
    ] {
        for name in ["Path", "Install Dir", "InstallDir"] {
            if let Some(r) = registry::software(reg, Hive::LocalMachine, game_key, name)
                .and_then(|s| root.map(&s))
                .and_then(|p| install_root(&p))
            {
                hits.offer(&r, Source::Ea, place);
            }
        }
    }

    for (_, drive) in &root.drives {
        for base in WIN_BASES {
            let dir = if base.is_empty() {
                Some(drive.clone())
            } else {
                resolve_ci(drive, base)
            };
            if let Some(dir) = dir {
                scan_base(&dir, Source::Folder, place, &mut hits);
            }
        }
    }
    hits_out.extend(hits.found);
}

/// Wine prefixes under the player's home folder, with a name to show for each.
pub fn wine_prefixes(home: &Path, os: Os) -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    let mut children = |dir: PathBuf, label: &str| {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            return;
        };
        let mut v: Vec<_> = rd.flatten().map(|e| e.path()).collect();
        v.sort();
        for p in v {
            if p.join("drive_c").is_dir() {
                let name = p
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                out.push((p, format!("{label} {name}")));
            }
        }
    };
    match os {
        Os::MacOs => {
            children(
                home.join("Library/Application Support/CrossOver/Bottles"),
                "CrossOver bottle",
            );
            children(
                home.join("Library/Containers/com.isaacmarovitz.Whisky/Bottles"),
                "Whisky bottle",
            );
            children(home.join("Games/Heroic/Prefixes/default"), "Heroic prefix");
            children(home.join("Games/Heroic/Prefixes"), "Heroic prefix");
        }
        Os::Linux => {
            children(home.join(".local/share/bottles/bottles"), "Bottles");
            children(
                home.join(".var/app/com.usebottles.bottles/data/bottles/bottles"),
                "Bottles",
            );
            children(home.join("Games/Heroic/Prefixes/default"), "Heroic prefix");
            children(home.join("Games/Heroic/Prefixes"), "Heroic prefix");
            children(home.join("Games"), "Lutris prefix");
        }
        Os::Windows => {}
    }
    if os != Os::Windows && home.join(".wine/drive_c").is_dir() {
        out.push((home.join(".wine"), "Wine prefix ~/.wine".into()));
    }
    out
}

/// Native Steam folders on macOS and Linux.
fn host_steams(home: &Path, os: Os) -> Vec<PathBuf> {
    let rel: &[&str] = match os {
        Os::MacOs => &["Library/Application Support/Steam"],
        Os::Linux => &[
            ".steam/steam",
            ".steam/root",
            ".local/share/Steam",
            ".var/app/com.valvesoftware.Steam/.local/share/Steam",
            "snap/steam/common/.local/share/Steam",
        ],
        Os::Windows => &[],
    };
    rel.iter()
        .map(|r| home.join(r))
        .filter(|p| p.is_dir())
        .collect()
}

/// Finds a folder holding `packages/core` inside `dir`, at most `depth` folders down, looking
/// at no more than `budget` folders (for app bundles of the old Mac release, which keep the
/// game deep in a wrapper). Provisional: the bundle's layout is not checked against a copy.
fn find_inside(dir: &Path, depth: usize, budget: &mut usize) -> Option<PathBuf> {
    if *budget == 0 {
        return None;
    }
    *budget -= 1;
    if game_at(dir).is_some() {
        return Some(dir.to_path_buf());
    }
    if depth == 0 {
        return None;
    }
    let mut kids: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.path())
        .collect();
    kids.sort();
    kids.into_iter()
        .find_map(|k| find_inside(&k, depth - 1, budget))
}

/// Everywhere this machine could hold an install: its own folders and Steam, and every Wine
/// prefix found. `home` is the player's home folder.
pub fn search(os: Os, home: Option<&Path>) -> Vec<Found> {
    let mut found = Vec::new();
    if os == Os::Windows {
        search_windows(&WinRoot::native(), &mut found);
        return found;
    }
    let Some(home) = home else { return found };
    let mut hits = Hits::default();
    let place = "This computer";
    let native = |s: &str| {
        let p = PathBuf::from(s);
        p.is_absolute().then_some(p)
    };
    for steam in host_steams(home, os) {
        search_steam(&steam, &native, place, &mut hits);
    }
    for base in [
        home.join("Games"),
        home.join("Games/Heroic"),
        home.join("GOG Games"),
    ] {
        scan_base(&base, Source::Folder, place, &mut hits);
    }
    if os == Os::MacOs {
        for apps in [PathBuf::from("/Applications"), home.join("Applications")] {
            let Ok(rd) = std::fs::read_dir(&apps) else {
                continue;
            };
            let mut bundles: Vec<PathBuf> = rd
                .flatten()
                .filter(|e| {
                    let n = e.file_name().to_string_lossy().to_string();
                    n.ends_with(".app") && named_like_game(&n)
                })
                .map(|e| e.path())
                .collect();
            bundles.sort();
            for b in bundles {
                if let Some(r) = find_inside(&b, 8, &mut 4000) {
                    hits.offer(&r, Source::Folder, place);
                }
            }
        }
    }
    found.extend(hits.found);
    for (prefix, label) in wine_prefixes(home, os) {
        if let Some(root) = WinRoot::wine(&prefix, label) {
            search_windows(&root, &mut found);
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdir::TestDir;

    /// A synthetic install: just the folders the checks look at.
    fn fake_install(root: &Path, game: Game) {
        std::fs::create_dir_all(root.join("packages/core/audio/vo/en-us")).unwrap();
        std::fs::create_dir_all(root.join("packages/core/audio/vo/fr-fr")).unwrap();
        std::fs::create_dir_all(root.join("bin_ship")).unwrap();
        let module = match game {
            Game::Origins => "modules/single player",
            Game::DragonAge2 => "modules/campaign_base",
        };
        std::fs::create_dir_all(root.join(module)).unwrap();
    }

    #[test]
    fn root_from_inside_or_above() {
        let t = TestDir::new("root-pick");
        let game = t.path().join("Dragon Age");
        fake_install(&game, Game::Origins);
        std::fs::write(game.join("bin_ship/DAOrigins.exe"), b"MZ").unwrap();
        assert_eq!(install_root(&game).as_deref(), Some(game.as_path()));
        assert_eq!(
            install_root(&game.join("bin_ship")).as_deref(),
            Some(game.as_path())
        );
        assert_eq!(
            install_root(&game.join("bin_ship/DAOrigins.exe")).as_deref(),
            Some(game.as_path())
        );
        assert_eq!(install_root(t.path()).as_deref(), Some(game.as_path()));
        assert_eq!(install_root(&t.path().join("missing")), None);
        let i = inspect(&game).unwrap();
        assert_eq!(i.game, Game::Origins);
        assert_eq!(i.languages, vec!["en-us".to_string(), "fr-fr".to_string()]);
        assert!(i.exe.is_some());
    }

    #[test]
    fn steam_library_and_manifest() {
        let t = TestDir::new("steam");
        let steam = t.path().join("Steam");
        let lib2 = t.path().join("Lib2");
        std::fs::create_dir_all(steam.join("steamapps")).unwrap();
        std::fs::create_dir_all(lib2.join("steamapps")).unwrap();
        std::fs::write(
            steam.join("steamapps/libraryfolders.vdf"),
            format!(
                "\"libraryfolders\"\n{{\n \"0\" {{ \"path\" \"{}\" }}\n \"1\" {{ \"path\" \"{}\" }}\n}}\n",
                steam.display(),
                lib2.display()
            ),
        )
        .unwrap();
        std::fs::write(
            lib2.join("steamapps/appmanifest_47810.acf"),
            "\"AppState\"\n{\n \"appid\" \"47810\"\n \"installdir\" \"Odd Name\"\n}\n",
        )
        .unwrap();
        fake_install(&lib2.join("steamapps/common/Odd Name"), Game::Origins);
        fake_install(
            &steam.join("steamapps/common/Dragon Age II"),
            Game::DragonAge2,
        );
        let mut hits = Hits::default();
        let native = |s: &str| Some(PathBuf::from(s));
        search_steam(&steam, &native, "here", &mut hits);
        let mut got: Vec<(Game, String)> = hits
            .found
            .iter()
            .map(|f| {
                (
                    f.game,
                    f.root.file_name().unwrap().to_string_lossy().into_owned(),
                )
            })
            .collect();
        got.sort_by(|a, b| a.1.cmp(&b.1));
        assert_eq!(
            got,
            vec![
                (Game::DragonAge2, "Dragon Age II".to_string()),
                (Game::Origins, "Odd Name".to_string()),
            ]
        );
        assert!(hits.found.iter().all(|f| f.source == Source::Steam));
    }

    #[test]
    fn manifest_cannot_escape_the_library() {
        let t = TestDir::new("steam-escape");
        std::fs::create_dir_all(t.path().join("steamapps")).unwrap();
        std::fs::write(
            t.path().join("steamapps/appmanifest_47810.acf"),
            "\"AppState\" { \"installdir\" \"../../etc\" }",
        )
        .unwrap();
        assert_eq!(steam_app_dir(t.path(), STEAM_ORIGINS_UE), None);
    }

    #[test]
    fn crossover_bottle_with_gog_key() {
        let t = TestDir::new("bottle");
        let home = t.path();
        let bottle = home.join("Library/Application Support/CrossOver/Bottles/GOG");
        let game = bottle.join("drive_c/Somewhere Odd/DAO");
        fake_install(&game, Game::Origins);
        std::fs::write(
            bottle.join("system.reg"),
            "WINE REGISTRY Version 2\n\n[Software\\\\Wow6432Node\\\\GOG.com\\\\Games\\\\1949616134] 1\n\"path\"=\"C:\\\\somewhere odd\\\\dao\"\n",
        )
        .unwrap();
        // And one in a usual folder of the same bottle, found without a key.
        fake_install(
            &bottle.join("drive_c/GOG Games/Dragon Age Ultimate"),
            Game::Origins,
        );
        let found = search(Os::MacOs, Some(home));
        assert_eq!(found.len(), 2, "{found:?}");
        assert_eq!(found[0].source, Source::Gog);
        assert_eq!(found[0].place, "CrossOver bottle GOG");
        // A case-insensitive disk (the default on macOS) keeps the registry's casing.
        let root = found[0].root.to_string_lossy().to_lowercase();
        assert!(root.ends_with("dao"), "{root}");
        assert_eq!(found[1].source, Source::Folder);
    }

    #[test]
    fn linux_steam_and_dedup() {
        let t = TestDir::new("linux");
        let home = t.path();
        let steam = home.join(".local/share/Steam");
        std::fs::create_dir_all(steam.join("steamapps")).unwrap();
        fake_install(
            &steam.join("steamapps/common/Dragon Age Ultimate Edition"),
            Game::Origins,
        );
        // The same Steam through the usual symlink must not list the game twice.
        #[cfg(unix)]
        {
            std::fs::create_dir_all(home.join(".steam")).unwrap();
            std::os::unix::fs::symlink(&steam, home.join(".steam/steam")).unwrap();
        }
        let found = search(Os::Linux, Some(home));
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].game, Game::Origins);
    }

    #[test]
    fn wine_drive_mapping() {
        let t = TestDir::new("drives");
        std::fs::create_dir_all(t.path().join("drive_c/Program Files")).unwrap();
        let root = WinRoot::wine(t.path(), "x".into()).unwrap();
        // On a case-insensitive disk the path keeps the casing it was asked with.
        let lower = |p: Option<PathBuf>| p.map(|p| p.to_string_lossy().to_lowercase());
        assert_eq!(
            lower(root.map(r"c:\program files")),
            lower(Some(t.path().join("drive_c/Program Files")))
        );
        assert_eq!(root.map("D:\\x"), None);
        assert_eq!(root.map("relative"), None);
        assert_eq!(root.map(""), None);
    }
}
