# Launcher and setup wizard

Sera's ask (2026-11-14): a launcher that configures the games, handles mods, and on first run
finds an existing Dragon Age: Origins install, or helps the player get one from Steam or GOG.

## Shape

- `thedas-store` (library, no GPU): downloading the game from Steam or GOG with the
  player's own account (see "Getting the game").
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

Two ways, side by side on the welcome page ("Don't have the game?"):

- **Download here** (`thedas-store`, the launcher's Download page): the player signs in to
  their own Steam or GOG account and the launcher downloads the Windows game's files into a
  folder (default `~/Games/<game>`, which the search looks in), then adds it as an install.
  The files are all the engine needs, so on macOS and Linux no Steam client, Galaxy, Wine or
  CrossOver is involved. Sera asked for this on 2026-11-14.
- **The store's own client**: `steam://install/<id>` (Steam's install dialog), Galaxy's
  `goggalaxy://openGameView/1949616134` on Windows, or the store pages; then search again.

### Credentials are never saved

Sera's rule: credentials are never saved, and only used for the download. The page says so
on every view. In the code:

- A typed password, a Steam Guard code and the GOG sign-in code move out of their text fields
  into `thedas_store::Secret` (zeroed on drop, `[redacted]` in `Debug`) when they are used;
  egui's state for those fields (its undo history) is wiped at the same moment.
- Steam's and GOG's tokens live in memory, inside the download's session, and are dropped
  with it. No refresh token is kept after the download (ask Sera before changing that).
- Nothing is written to disk except the game's files: steamroom-client's `TokenStore`,
  `ManifestCache` and `DepotConfig` are not used.
- The HTTP and Steam libraries (`reqwest`, `hyper`, `h2`, `rustls`, `steamroom`) log only
  warnings, whatever `RUST_LOG` says (`report.rs`), so no URL or header with a token reaches
  the console or a report's log. `thedas-store` itself never logs a credential.

### Steam

Through `steamroom` / `steamroom-client` (MIT OR Apache-2.0), Steam's own client protocol:
QR sign-in (scanned in the Steam mobile app, no password typed) or account name and password
followed by a Steam Guard code or a confirmation in the app. Product info names the depots:
those for Windows (or any OS) with a public manifest, in the chosen language or none (English
when the language has no depot), skipping depots another app provides; DLC depots the account
can't open are skipped. Each depot's key, then its manifest (every path checked to stay inside
the folder, links refused), then `DepotJob` fetches, decrypts and checks each chunk. Origins
downloads app 47810 (Ultimate Edition, all DLC), DA2 app 1238040.

### GOG

Origins only (GOG does not sell DA2). Provisional: written from public descriptions of GOG's
content system and checked against GOG's public build and file lists for Origins (32,418
files, 38.8 GB in English); the signed download addresses need an account and have not been
seen working yet.

- Sign-in happens on GOG's own page in the browser, so the launcher never sees the password.
  GOG ends on `embed.gog.com/on_login_success?origin=client&code=...`; the player pastes that
  address and the launcher takes the code. GOG Galaxy's public client id and secret are in
  the repository for this (Sera's decision, 2026-11-14; the one exception to "no key material
  in the repository").
- `content-system.gog.com/products/1949616134/os/windows/builds?generation=2` lists builds; the
  newest public generation-2 build's manifest (zlib JSON) names depots by language (`*` for
  all). Each depot's file list is `gog-cdn-fastly.gog.com/content-system/v2/meta/aa/bb/<hash>`.
- `secure_link` signs download addresses for the account (renewed every 15 minutes or when
  refused; the access token is renewed with the in-memory refresh token on a long download).
  Each chunk is fetched, its compressed MD5 checked, inflated, and its MD5 checked again.
- Files land as `<name>.part` and are renamed when whole; a file already there at the right
  size is kept, so starting again picks up where it stopped. Four files and twelve chunks at
  a time.

Later, not built: unpacking GOG's offline installer directly (an Inno Setup reader) for
players who already downloaded it.

## Icon

Sera's emblem (`crates/thedas-app/assets/icon/`): `emblem-1024.png` is the source,
`emblem-256.png` is embedded for the window icon, the macOS Dock icon (set at run time, as the
game is not started from an app bundle) and the launcher's header. `openthedas.ico` becomes the
Windows executable's icon through `build.rs` when a resource compiler is on the build machine
(`x86_64-w64-mingw32-windres` from binutils-mingw-w64 for the cross builds; without one the
build warns and goes on). `openthedas.icns` is for a future macOS app bundle.

## Accessibility and scripting

The launcher's controls are exposed through AccessKit (egui's accessibility tree): UI
Automation on Windows, VoiceOver on macOS, AT-SPI on Linux. Buttons whose text repeats per game
or install ("Remove", "Download from Steam") carry a fuller accessible name ("Remove Dragon
Age: Origins (Steam, This computer)", "Download Dragon Age II from Steam"); text fields are
labelled by the text beside them.

Scripts drive the launcher through the same tree, clicking controls by name
(`launcher_control.rs` has the full list):

- `thedas-app --launcher --launcher-steps "click Search this computer; wait; tree; click
  Continue; shot out.png; quit"`: steps in order; `tree` and `status` print JSON; a failed step
  ends the run with a non-zero exit.
- `thedas-app --launcher --control 127.0.0.1:8020`: JSON requests a line at a time
  (`{"cmd":"click","name":"Continue"}`, `type`, `key`, `choose`, `wait`, `run`, `tree`,
  `status`, `screenshot`, `quit`). `--control` without `--launcher` still drives the game.
- File and folder dialogs can't be driven, so `choose <path>` answers the next one:
  `choose C:\Games\Dragon Age; click Pick the game's folder myself…`.
- `thedas-app --launcher-script file.steps`: the same steps from a file, one or more to a line,
  `#` comment lines ignored, `${NAME}` replaced by an environment variable.
- `expect <what>` fails the step unless the page is as said (it retries for four seconds):
  `page`, `heading`, `text`, `no text`, `focus`, `enabled`/`disabled`, `installs`, `message`,
  `launch`. `mkinstall <folder> [da2]` makes a stand-in install (folders only) to find or
  pick. `key Shift+Tab` and other modifier combinations work.
- `THEDAS_LAUNCHER_CONFIG=<file>` points the launcher at a scratch settings file, and
  `THEDAS_LAUNCHER_DRY_RUN=1` makes Play report the command it would run (`launch` in `status`)
  instead of starting the game.
- Typed text never reaches a log line (a step logs only the field and the length), and a
  password field reads back as dots.

### Regression scripts for QA

`docs/launcher-qa/` holds scripts to rerun after any launcher change. `run.sh` plays one in a
scratch home with a scratch settings file and no mods, so a player's own setup is never read
or changed, and exits non-zero when a step or an `expect` fails:

```
docs/launcher-qa/run.sh first-run   # wizard start to finish on stand-in installs, then a
                                    # second start on what it saved; all four tabs; Play
docs/launcher-qa/run.sh keyboard    # keyboard only: Tab order, focus after page changes
```

`run.sh <script> [thedas-app]` uses `target/debug/thedas-app` or `$THEDAS_APP`. On a Linux
box with no display, wrap it in `xvfb-run -a`. On Windows, set `HOME`/`USERPROFILE`,
`THEDAS_LAUNCHER_CONFIG`, `THEDAS_LAUNCHER_DRY_RUN`, `THEDAS_MODS=none`, `SCRATCH` and `OUT`
(see `run.sh`) and run `thedas-app --launcher-script docs\launcher-qa\first-run.steps`.
Screenshots of each stage go to `$OUT`. The scripts read the page through the same
accessibility tree a screen reader does, so a control that loses its name or order fails them.
They do not stand in for hearing it in VoiceOver or Narrator: that needs a person.

Keyboard behaviour they pin down: Tab goes header, page, footer (Back before the primary
button, the tabs in reading order); a page change that is not a click on a tab moves focus to
the new page's first control; a finished search moves focus to the results; page titles are
headings, the footer message and the search status are live regions, and the Steam Guard field
takes focus when asked for.

## Screenshots and checks

`thedas-app --launcher-capture out.png --launcher-page <page> --size 1920x1360
--launcher-scale 2` draws a page (`welcome`, `search`, `keys`, `play`, `games`, `mods`,
`settings`, `download`, `download-gog`, `download-progress`) offscreen and exits. It reads the settings of whatever `HOME` is set, so a scratch
`HOME` with synthetic installs shows the pages without touching the player's.

## Slices

1. Detection, settings, mod list editor, store links (`thedas-launcher`), with tests. Built.
2. The wizard and launcher window in `thedas-app`. Built.
3. Downloading from Steam and GOG in the launcher (`thedas-store`). Built; the GOG download
   itself is untested without an account.
4. Next: game configuration the in-game options screen does not cover; a launcher entry in
   the pause menu; a macOS app bundle using `openthedas.icns`; controller navigation of the
   launcher.
