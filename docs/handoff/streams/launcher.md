# Launcher stream: game launcher, setup wizard, store downloads, launcher accessibility

Thread: "Build a game launcher and setup wizard" (project thread cmsg_01MDS9yYeQSHJXoFNZwuNeXt6tKi9amXBgu7pDKw9kRHgv), started 2026-11-14 00:10 UTC. Branch used: `claude/project-thread-hi5uzj` (all merged).

## What I own

Four crates and modules, all about getting a player from "I own Dragon Age" to "it is running":

- `crates/thedas-launcher`: no UI. Finds game installs (Steam on every OS, GOG, EA app, Wine/CrossOver/Whisky/Heroic/Lutris/Bottles prefixes, usual folders on every drive), reads `launcher.json` (installs, play options, `skip_launcher`, `wizard_done`), edits `mods.txt` while keeping comments and blank lines, and builds store links. Pure functions, tested with synthetic folders.
- `crates/thedas-app/src/launcher.rs` (+ `launcher_download.rs`, `launcher_control.rs`): the egui launcher window and first-run wizard. Pages: Welcome, Search, Keys (DLC keys), Play, Games, Mods, Settings, Download. `launcher_mods.rs` (the "What your mods do" section) was written by the mods stream and then hooked into the Mods page; it follows my conventions.
- `crates/thedas-store`: downloading the game from the player's own Steam or GOG account inside the launcher.
- `docs/launcher.md` (design and how-tos) and `docs/launcher-qa/` (rerunnable regression scripts).

PRs I landed: #657 launcher, wizard and emblem icon; #668 Steam and GOG downloads; #709 AccessKit accessibility plus the scripting hook; #735 keyboard and screen-reader fixes plus the first-run regression scripts. I also reviewed the mods stream's #732.

## Current state

Everything above is on `main`. No branch or PR of mine is open or parked. The mods stream has since changed `launcher_mods.rs` (stale-report fixes) on its own; I did not review that.

Not done, in priority order:
1. The Mac run of `docs/launcher-qa/run.sh first-run|keyboard`, and a check that macOS exposes the accessibility tree (System Events or Accessibility Inspector). I started a Remote Control session for it and stopped it when usage was cut. Both scripts pass under Xvfb on Linux only.
2. VoiceOver and Narrator listening checks. A script cannot judge these; a person has to.
3. Windows UI Automation check in the Windows VM (QA owns the VM).
4. A real Steam or GOG sign-in and download. Nothing past the sign-in screens, the catalogue read and unit tests has ever run against a real account. GOG's download-link step (`secure_link`) is a guess from public descriptions and is marked provisional in `gog.rs`.

## Rules Sera set for this stream (do not relax without her word)

- Credentials are never saved, and only used for the download. No refresh tokens kept (ask first if that ever changes). Passwords, Steam Guard codes, GOG sign-in codes and every token live in memory only: `Secret` (zeroed on drop, redacted in `Debug`) or the store library's session object, and are dropped when the download ends. Nothing secret is logged: `report.rs` caps reqwest, hyper, h2, rustls and steamroom logs at warn, and typed text is never logged by the script hook (only the field name and the length).
- steamroom-client's `TokenStore`, `ManifestCache` and `DepotConfig` are never used (they write to disk). Only the game's own files are written.
- GOG's public Galaxy client id and secret pair is committed in `gog.rs` by Sera's explicit decision (a public OAuth pair; the one exception to "no key material in the repo"). Nothing else of that kind goes in.
- Never reorder Sera's mod order. The launcher's mod editor only writes `mods.txt` on Save and keeps `mods.txt.bak`.
- The launcher never changes the game's files. It reads them where they are.
- No game data in the repo. The QA scripts use stand-in folders only.

## How to build, run, test, verify

```
cargo build -p thedas-app
thedas-app --launcher                       # show the launcher even if skip_launcher is set
cargo test -p thedas-launcher -p thedas-store -p thedas-app
```

Without `--install` or `THEDAS_INSTALL` and with no headless flag, `thedas-app` shows the launcher; Play re-spawns the exe with `--install <root>` plus the play options and `THEDAS_KEYS`. One winit EventLoop per process is why it re-spawns instead of continuing.

Regression scripts (the main way to verify):
```
docs/launcher-qa/run.sh first-run    # wizard end to end on stand-in installs, then a second start on what it saved
docs/launcher-qa/run.sh keyboard     # Tab order, focus after page changes
```
`run.sh` makes a scratch HOME, sets `THEDAS_LAUNCHER_CONFIG` (scratch settings file), `THEDAS_LAUNCHER_DRY_RUN=1` (Play reports the command instead of starting the game) and `THEDAS_MODS=none`, and exits non-zero when a step or `expect` fails. They need a window: on a headless Linux box wrap in `xvfb-run -a` (needs libxkbcommon-x11). Windows: set the same env vars and run `thedas-app --launcher-script docs\launcher-qa\first-run.steps`. Each run writes screenshots to `$OUT`.

Driving it by hand: `thedas-app --launcher --launcher-steps "click Search this computer; wait; tree; expect heading Found; shot x.png; quit"` or `--launcher --control 127.0.0.1:8020` (JSON lines: click, type, key, choose, wait, run, tree, status, expect, mkinstall, screenshot, quit). Full list in the header of `launcher_control.rs`. QA's launcher ports are 8020-8029. File dialogs cannot be driven, so `choose <path>` answers the next one. Offscreen page screenshots: `thedas-app --launcher-capture out.png --launcher-page <page> --size 1920x1360 --launcher-scale 2` (reads the settings of whatever HOME is set; use a scratch HOME).

## Key files and design decisions

- `launcher.rs`: `Ui` holds all state; pages draw in `draw()`; clicks push `Action`s that `act()` runs after the frame (file dialogs, spawning, saving). `Host for Ui` is what scripts read (`status`). `shot()` renders a PNG offscreen.
- Layout trick: egui panels must be created before the central panel to take space, but Tab order is registration order. So `draw()` reserves a fixed-height footer panel (`FOOTER_HEIGHT`), draws the page, then draws the footer's controls into that rect afterwards. Result: Tab goes header, page, footer. The nav tabs are drawn left to right after a spacer sized from the last frame's measured width (a right-to-left layout reversed their reading order).
- Focus: a page change that was not a click on a nav tab, and the end of a search, move focus to the new page's first control (`Memory::move_focus(Next)` with nothing focused picks the first widget that registers afterwards). The Steam Guard field requests focus once when asked for.
- Accessibility: egui-winit's `accesskit` feature gives UIA on Windows, VoiceOver on macOS, AT-SPI on Linux. Page titles use `heading()`, status lines use `live()` (polite or assertive). Buttons whose text repeats get a fuller name through `named()` ("Remove Dragon Age: Origins (Steam, This computer)"). Text fields are labelled by the text beside them (`labelled_by`). The window is created hidden until AccessKit is attached (its adapter must exist before first show).
- Scripts read the same tree a screen reader does, so a control that loses its name or order fails them. That is deliberate; keep new controls named.
- `thedas-store`: runs on its own thread with its own tokio runtime; the launcher talks to it through a `Job` (Events out, Replies in). Steam uses steamroom 0.3 (QR or password plus Guard, PICS, depot keys, CDN). GOG is written from scratch against content-system v2: browser sign-in with a pasted code (the app never sees the password), builds, zlib JSON depots, chunk md5 checks. `safe_relative()` guards every path a store hands us (no `..`, drive letters, control characters). Downloads resume into the same folder.
- Platform wrinkles already handled: case-insensitive disks (tests compare lowercased paths), Wine drive mapping, CrossOver bottles with GOG keys, Windows exe icon needs `windres` on the build box.

## Known gaps and traps

- Picking a folder on the wizard's Search page adds it and jumps to the Keys step. That is the current behaviour (and it now keeps the search's ticked installs); you may want it to stay on Search.
- On launch, focus is on nothing; the first Tab goes to the first control. Only page changes move focus.
- The Search page puts focus on Back while a search is running (nothing else focusable yet), then moves it to the results.
- The DLC passwords step: Origins passwords come from the player's own exe at run time; DA2 needs a keys file. The search fallback from #623 was never tested on a real install.
- Not done: game configuration the in-game options do not cover, a launcher entry in the pause menu, a macOS app bundle using `openthedas.icns`, controller navigation of the launcher.

## What a newcomer must not break

- Never write a credential, token or code to disk, a log, a screenshot, a report or a script file. If you add a store call, check what its client library caches or logs.
- Keep every control named and in reading order; run `run.sh keyboard` after any layout change.
- `THEDAS_LAUNCHER_CONFIG`, `THEDAS_LAUNCHER_DRY_RUN` and `THEDAS_MODS=none` are what keep QA runs from touching a player's settings and mods. Do not run launcher scripts against a real HOME.
- Play must still re-spawn the exe with `--install` and the keys env var; the game side expects exactly that.
- `launcher.json` that does not parse is renamed to `.bad`, never deleted.
