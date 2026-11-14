# Launcher and setup wizard

Sera's ask (2026-11-14): a launcher that configures the games, handles mods, and on first run
finds an existing Dragon Age: Origins install, or helps the player get one from Steam or GOG.

## Shape

- `thedas-launcher` (library, no GPU): install detection, the launcher's settings file, the
  mod list editor and the store links. Everything in it is plain file and registry reading, so
  it is unit tested with synthetic folders.
- `thedas-app` shows the launcher (an egui window, the same egui mod mode uses) when it is
  started with no install: no `--install`, no `THEDAS_INSTALL`, and none of the headless or
  test flags. `--launcher` opens it on purpose. Every existing command line keeps working
  unchanged; `play.command`, the harness and QA recipes never see the launcher.
- **Play** starts the game as a new process of the same executable with `--install` and the
  chosen options, then the launcher closes. winit allows one event loop per process, and the
  update handover already starts the game the same way.

## Settings

`launcher.json` in the player's data folder (`~/.openthedas`, `Documents\OpenThedas` on
Windows): the installs the player kept, which one plays, a keys file per install, and the
play options (window size, graphics detail, frame effects, intro movies, voice language,
sound). "Skip the launcher" plays the chosen install at once when the game is started with no
arguments; `--launcher` brings the launcher back.

## Finding installs

Each place is looked at, each candidate is checked (`packages/core` must exist; the game is
Origins or DA2 by `modules/campaign_base`), and duplicates are dropped by canonical path.

| Where | How |
| --- | --- |
| Steam | `libraryfolders.vdf` in the Steam folder lists the libraries; `appmanifest_<id>.acf` names the game's folder. Origins: 47810 (Ultimate Edition), 17450 (original). DA2: 1238040. |
| GOG | Registry `SOFTWARE\WOW6432Node\GOG.com\Games\1949616134`, value `path`. |
| EA app / retail | Registry `SOFTWARE\WOW6432Node\BioWare\Dragon Age` (and `Dragon Age 2`), values `Path` and `Install Dir`. Provisional: from install reports, not traced. |
| Common folders | Children named like "Dragon Age" under Program Files, Program Files (x86), `GOG Games`, `GOG Galaxy\Games`, `Origin Games`, `EA Games`, `Games`, on every drive. |
| Wine prefixes | CrossOver bottles (`~/Library/Application Support/CrossOver/Bottles`), Whisky bottles, Heroic, Lutris and Bottles prefixes, `~/.wine`: the prefix's `system.reg`/`user.reg` are read as the registry, `C:` maps to `drive_c`, and the same searches run inside. |
| Linux Steam | `~/.steam/steam`, `~/.local/share/Steam`, the Flatpak's folder; Proton games live in the normal libraries. |
| Picked by hand | Any folder or file inside an install (the exe, `bin_ship`) is walked up to its root; a folder holding an install one level down is walked down. |

## DLC keys

The player can point each install at an `erfpasswords.txt`-format keys file. The launcher
passes it as `THEDAS_KEYS` (the engine reads `--keys`, then `THEDAS_KEYS`, then `keys.txt` in
the data folder). Origins does not need one: its DLC passwords come from the player's own
`DAOrigins.exe`. DA2's encrypted DLC needs it. No key material is ever in the repository.

## Mods

The mod page edits `mods.txt`, the list the engine already reads. Lines the launcher does not
understand (comments, blank lines) stay where they are. A mod turned off becomes
`#off: <line>`, which the engine skips as a comment. The order only changes when the player
moves a mod; adding a mod puts it at the end (highest priority). Nothing is written until
Save, and the first save keeps the old list as `mods.txt.bak`.

## Getting the game

Handing off to the store's own client, never downloading game files ourselves:

- Steam: `steam://install/<id>` opens Steam's install dialog when the player owns it; the
  store page otherwise. On macOS and Linux, Steam installs the Windows game only through
  Proton or a CrossOver bottle, and the launcher says so.
- GOG: `goggalaxy://openGameView/1949616134` for Galaxy on Windows, the store page otherwise.
  On macOS the GOG offline installer runs in CrossOver; the wizard then finds the bottle.

Later, not built: unpacking GOG's offline installer directly (an Inno Setup reader) so a Mac
needs no Wine at all, and `steamcmd` with `+@sSteamCmdForcePlatformType windows` as a
Valve-sanctioned download path on macOS and Linux.

## Icon

Sera's emblem (`crates/thedas-app/assets/icon/`): `emblem-1024.png` is the source,
`emblem-256.png` is embedded for the window icon, the macOS Dock icon (set at run time, as the
game is not started from an app bundle) and the launcher's header. `openthedas.ico` becomes the
Windows executable's icon through `build.rs` when a resource compiler is on the build machine
(`x86_64-w64-mingw32-windres` from binutils-mingw-w64 for the cross builds; without one the
build warns and goes on). `openthedas.icns` is for a future macOS app bundle.

## Screenshots and checks

`thedas-app --launcher-capture out.png --launcher-page <page> --size 1920x1360
--launcher-scale 2` draws a page (`welcome`, `search`, `keys`, `play`, `games`, `mods`,
`settings`) offscreen and exits. It reads the settings of whatever `HOME` is set, so a scratch
`HOME` with synthetic installs shows the pages without touching the player's.

## Slices

1. Detection, settings, mod list editor, store links (`thedas-launcher`), with tests. Built.
2. The wizard and launcher window in `thedas-app`. Built.
3. Next: game configuration the in-game options screen does not cover; a launcher entry in
   the pause menu; a macOS app bundle using `openthedas.icns`; controller navigation of the
   launcher.
