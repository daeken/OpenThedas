# Input / UI stream

The stream that owned everything between the player and the simulation: the interface
movies (Scaleform `.gfx`) and their engine bindings, mouse, keyboard and controller input,
cursors, screens, options, saves screen plumbing, tutorials and popups. Written 2026-11-14
by the Claude session that ran it (worktree `~/projects/OpenThedas-wt/input`, branches
`m1-*`).

## What it owns

- **Interface bindings** (`crates/thedas-ui`): the bridge between the movies and the
  simulation. `binding.rs` (the values every movie reads: `sim_values_for`, `Presentation`),
  `input.rs` (messages and queries the movies send: `apply`, `query`), `protocol.rs` (the
  `ExternalCommands`/`fscommand` host API: GetValue, GetNumMembers, GetNumber, SendMessage),
  `options.rs` (player settings), `chains.rs` (ability lists), `appearance.rs` (character
  creation sliders), `keys.rs` (key names/designer tags), `avm1.rs` (AVM1 prelude helpers that
  patch movie behaviour). The player itself is `crates/thedas-gfx` (our own Scaleform player;
  Ruffle is gone since #818) and belongs to the Scaleform/Codex stream; changes there go
  through it.
- **App side** (`crates/thedas-app`): `ui.rs` (`UiLayer`: screens, popups, floaties, loading
  screens, death screen, saves), `screens.rs` (which screen is up: `wanted()`), `pad.rs` +
  `padplay.rs` + `focusnav.rs` (the controller), `cursor.rs` (mouse cursors), `report.rs`
  and the report marking in `main.rs`, `chargen_stage.rs` (character creation camera),
  `journal.rs` (journal saves: crash-safe resume).
- **Gap list of record**: `docs/gaps/ui.md` (ids `UI-NNN`; last used UI-219) and
  `docs/KNOWN_GAPS.md`.
- Training tutorials, popups, GUI_OPENED events, store/container/chest screens, the options
  screen (incl. Anti-Aliasing slider), credits, death screen, start menu, resume offer, Mod
  Mode button, issue-report marking UI.

## State at handoff (2026-11-14)

Everything I built is merged to `main`; no open PRs of mine. Last merged: #707 (stale aim
drop), #712 (CreatureCanUseItem argument order), #717/#722 (picking tuning, moot since Ruffle
went), #734 (Anti-Aliasing slider). Codex-stream review requests #834/#851/#852/#859/#860 were
answered OK. No parked branches that matter (`m1-*` branches on origin are all merged).
`~/projects/OpenThedas-wt/input` is a scratch worktree: delete or reuse it freely.

Open questions I did not close:
- deferred reported a "Please enter a name for the dog:" popup on its headless runs
  (`01_cousland_start.json`); I could not reproduce it (my runs reach the HUD with no popup).
  It is a script `ShowPopup`, not our code. If it comes back, get the exact command, commit and
  app log, and check the text-popup focus path (`focus_dialog_field`, `GUIPopupLayer.InputText`).
- Chargen: Sera reported the taken skill/talent not showing as taken. I reproduced the real
  click flow and it matched QA's captures of the original (the movie's white "learn" glow
  plays for about half a second and then settles). Unconfirmed on Sera's build; ask for a
  screenshot if it recurs.
- Unverified: the exe's default and storage of the anti-aliasing setting (`AntialiasingLevel`
  ini key; we keep our own `[VideoOptions] AntialiasIndex`).

## How to build, run and verify

Hard rules on Sera's Mac (they exist because they bit us):
- Control ports **7960-7969 only** for this stream; `lsof -nP -iTCP:PORT -sTCP:LISTEN` first.
  **Kill by pid only, never by pattern, and never a parent shell pid** (killing a parent
  dropped the session once). Check `lsof -a -p PID -d cwd` before killing.
- **No speaker output**: `--no-audio` / `THEDAS_NO_AUDIO=1`.
- **Hidden window** for every run: `THEDAS_HIDDEN_WINDOW=1` with `--control`.
- Anything that could trigger an issue report (F8, both sticks, Report Issue) must run with a
  **scratch HOME** (the real `~/.openthedas` holds a live upload webhook). `--control` runs are
  also offline by `upload::online()`.
- Builds at `nice -n 19 cargo ... -j 2`, one heavy build at a time. Check `df -h` first.
- Never commit game data; only QA runs the original game.

Build and check (the merge loop): fetch, merge `origin/main` by exact SHA, then
`cargo fmt --all --check`, `cargo clippy --all-targets --locked -j 2 -- -D warnings`,
`cargo test --locked -j 2` (debug), and merge only if `origin/main` is still the tested SHA.
Main moves faster than a ~20 minute cycle: ask the coordinator for a short merge hold early.

Driving the app (see also `OpenThedas-testdata/ui/for-qa/recipes.md`, the per-feature
recipes QA grades from):
- `OpenThedas-testdata/ui/blockload/live.sh BIN SAVE PORT LOG` starts a hidden-window app on
  a save with a scratch HOME and a control port (`SIZE=1280x720`). Saves there:
  `01_cousland_start_main.json`, `ce_home.json` (city elf home: a chest, the father),
  `dc_02b_store.json` (Olinda's store), `s01_alienage.json` (journal), `sc_tower4.json`.
- Control client: `OpenThedas-testdata/triage/firsthour3/g.py PORT OUTDIR step...` (steps:
  `shot:NAME`, `st`, `goto:X,Y`, `tut`, `conv:...`, any app input step). Input steps:
  `run MS`, `click X,Y` (physical pixels: twice the 1280x720 view), `rclick`, `rpress`/`rrelease`,
  `hover`, `leave`, `key i|c|j|/|f1`, `gamemenu` (the real Escape key; `key escape` is a
  text-field key only), `pad a|press rt|stick-left x,y ms`, `call MOVIE PATH args`,
  `setvalue`, `asktext`/`askchoice`, `trainingmode N`, `shift down|up`.
- Headless captures: `OpenThedas-testdata/qa/cap.sh NAME --new-game --no-audio --steps "..."`
  (set `APP=binary`). The chargen click-through used as a gate:
  `OpenThedas-testdata/triage/convcam/bin/convcap.sh dcommon NAME "run 8000" --size 1280x720`
  (expect the name "Faren" and screens `{Cutscene}`).
- Debugging the movies: `RUST_LOG=thedas_ui=debug` logs every engine query the movies make
  (`query CreatureCanUseItem [...]`); `THEDAS_TOKEN_LOG=1` logs every value pushed to a movie;
  `call MOVIE _global.thedas_tree_list` lists the visible clips with bounds; movie bytecode
  was disassembled to `OpenThedas-testdata/ui/covagent/ctl/*.dis` (and `ui/extra`). The
  disassembler script is `OpenThedas-testdata/scripts/ui/avm1dis.py`.
- Exe tracing: the disassembly lives at `OpenThedas-testdata/disasm/daorigins.asm`
  (imagebase 0x400000; ask rendering for the helper). Per-trace notes are in
  `OpenThedas-testdata/notes/` (`handoff-ui.md`, `selection-rings-trace.md`,
  `alerts-trace.md`, `journal-saves-plan.md`).

## Key design decisions (the non-obvious ones)

- **The UI reads sim state and never writes it.** Inputs reach the sim only through
  `Sim::input`/`Sim::apply` (#639). Values for movies come from `sim_values_for(sim, shown)`
  every frame; presentation-only state (tab filters, store trades, item filters, options,
  hover) lives in `Presentation`, not the sim.
- **Movies read tables by row position**: tables 3, 5, 92, 97, 98, 109, 138, 187, 208-213 and
  more are indexed by row order, and the exe keeps merged worksheets sorted ascending by row
  id. `Table::order` must stay ascending (#560/#564); chargen broke when it was not.
- **Movie messages have quirks that were worked out from bytecode**: argument order
  (`CreatureCanUseItem` sends creature then item), which message takes one item from a chest
  (`GUI.SelectedChar.AcquireItem`; `Container.AcquireItem` is put-into), `GetNumMembers`
  skips list members with value 0 (the anti-aliasing list needed a special case), list-valued
  tokens need the list's own token to carry a value or the movie's observer never fires.
- **Multiply blend** is not supported by compositing into a transparent texture: the New Quest
  splash, death screen blood and loading-screen stains are drawn in `MultiplyPass`es (hide
  clips, draw the multiplied ones over a white backdrop, multiply onto the frame; `above`
  clips follow). Reuse that for any movie with a multiply layer.
- **Controller**: `pad.rs` is a data table (`BINDINGS`: context, button, edge, action);
  `padplay.rs` does the behaviour against a `Host` trait (testable). Contexts: Field, Picker,
  Screen, Conversation, Cutscene. A held target is kept only if the player picked it. A is
  never "use" while a tutorial waits (it opens the screen the tutorial asks for). RT+A swaps
  weapon sets (every D-pad direction and stick click is taken in the field).
- **Escape / B** on a store, chest, job board or world map presses that screen's own Close
  (`padplay::press_back`), not the game menu.
- **Cursors** follow the exe's rule (0x70f600/0x70cda0): movies' own cursor when the
  interface has the mouse, else by what is under it; `cursor.rs` documents the order.
- **Issue reports** (F8 / both sticks / menu): marking state machine in `main.rs`
  (`Marking::{Off, Offered, On}`) with pad support; reports upload only outside automated runs.
- **Provisional marking**: anything not traced in the exe is commented "Provisional" and has
  a gap line in `docs/gaps/ui.md`. Keep that discipline.

## Gaps and what is next

`docs/gaps/ui.md` is the list (about 180 items). A PR that fixes an item deletes its line in
the same PR and names the id; new gaps take the next free number (UI-220); keep the count in
`docs/KNOWN_GAPS.md` equal to `grep -c "^- \*\*UI-" docs/gaps/ui.md`. Highest value left: the
radial menu (UI-180, large), button glyphs/prompts for the pad (UI-172), key rebinding
(UI-059), video/control options still not applied (UI-055/056/057), effect icons and
summons on the HUD (UI-089/017), crafting and the remaining unbuilt screens.

## What a newcomer must not break

- The chargen click-through (gate above) and the first-hour flows in `recipes.md`.
- Table row order (see above) and the bridge's list conventions (GetNumMembers, list tokens).
- The pad contexts: tutorials must be dismissable by A, B/Escape must always get the player
  out of a screen, a held stick must not cancel a script door use (`walk` in `padplay.rs`).
- Never write sim state from the UI; never add a second player path that bypasses
  `Sim::input`.
- The scratch-HOME rule for anything touching reports.
