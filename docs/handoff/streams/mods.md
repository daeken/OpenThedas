# Handoff: the mods and mod mode stream

Written 2026-11-14 by the stream's agent, for whoever takes it over in the new harness.
Nothing here is a secret; the exe addresses are where the behaviour was traced, not exe bytes.

## What the stream owns

1. **Mods as layers.** The layered virtual file system, the player's load order (`mods.txt`),
   `.zip`, `.7z`, `.rar`, `.dazip` and folder mods, add-ins (DLC) discovery and order, offers,
   and the order resources are found in. Crates: `thedas-vfs` (`vfs.rs`, `install.rs`, `mods.rs`,
   `manifest.rs`), `thedas-cli` (`thedas mods`, `addins`, `mod-cache`, `resolve`).
2. **Mod mode.** Modding done inside the running game, not in an editor: the F10 overlay, the
   inspector, world-state browser and editor, conversation launcher, animation scrubber, object
   add/move/remove, fly camera, live edits, and the Blender round trip (glTF export and
   re-import of meshes, materials, textures, placement). Files: `crates/thedas-app/src/`
   `modmode.rs`, `modconv.rs`, `modworld.rs`, `live.rs`, `workspace.rs`, `glb.rs`, `glb_in.rs`,
   plus `thedas-formats` (`mesh.rs`, `model.rs`, `material.rs` writers) and
   `thedas-sim/src/commands.rs` (the command API, shared with the sim stream).
3. **The launcher's mod view** (`launcher_mods.rs`, with the launcher stream's conventions).

Sera's principle for mod mode, in her words: mod development should be identical to playing the
game ("kind of Hero Engine style"), and modding is "a thing you do in-game", not a toolkit. Mods
stay separate, attributable layers; nothing is ever written over game files.

## How the stream worked (organisation and responsibilities)

I was one thread under the coordinator. I did not run sub-agents of my own. The working shape:

- **Coordinator** relayed Sera's asks and other streams' answers. Anything touching another
  stream's area went through it (or a direct message it allowed).
- **Triage** (the QA/regression stream) was my main reviewer. Its rules, which became mine:
  announce **every** change to `install.rs` `stack` or the shared resource stack before merging,
  even inert ones; a rule that drops a file lands only when both operands are traced and what it
  drops is logged; stack/sim changes need the side-by-side gate (below).
- **Sim** agreed the `SimCommand` API with me (`SetPlotFlag`, `StartConversation`, `SetLocal`,
  `SetProperty`, `MoveObject`, `CreateObject`, `DestroyObject`) and every change to it; they own
  determinism and the journal.
- **UI stream** provided the pause-menu hook (`App::toggle_mod_mode()`), and the rules for input
  routing with an egui overlay. **Launcher stream** owns `launcher.rs`; I own `launcher_mods.rs`
  and follow its conventions (named per-row buttons, poll like `poll_search`, `Ui::busy()`,
  dialogs through `Ui::pick`).
- Sera approved things in-thread when the harness required her own words (egui, crates).
  She gave standing permission to merge green PRs.

My own responsibilities: trace the original executable for every rule I changed, write the
change, prove it did not move anything it should not (below), write the gap-doc entries
(`docs/gaps/formats.md` FMT-006..013, 047, 048), keep `docs/mods.md` (player page) and
`docs/mod-mode-testing.md` (tester guide) in step with the UI labels, queue media shots, and
report merges to the coordinator with the merge commit.

## Current state (2026-11-14)

Everything is merged except one small branch.

- **Merged highlights**: mod layers #528, table prefix sets #532, Documents folders #540,
  dazip add-ins #553/#691, registry order #589, `.7z` #592, GameVersion gate #597, player page
  #601, mod-cache tool #605; mod mode #634 (slice 1), #635 (conversations), #638 (fly, world
  state), #641 (editable variables), #683 (tester polish), #688 (stats/effects); Blender #645,
  #650 (live textures), #661 (mesh import), #667 (placement), #671 (.mmh), #675 (BC texture
  encoder, `texpresso`), #678 (.mao), #699 (animation clips); object add/move #664; offers and
  `GetPRCEnabled` #710; `.rar` #716; folder order #723; launcher mod view #732.
- **Parked branch**: `m1-mod-report-fixes` (pushed, no PR) holds the launcher stream's three
  follow-ups on `launcher_mods.rs`: the report remembers its game and is dropped when another is
  chosen; a scan overtaken by a Save or a game change restarts; the Files list indices are cached
  when a layer is chosen. Unit-tested; clippy and the full suite on merged main were not run.
  Next step: merge main, fmt/clippy/test, PR, merge. The launcher stream said it will add
  `heading()`/`live()` accessibility marks to `launcher_mods.rs` after its own accessibility PR;
  do not edit the file before that lands.
- **Open sweep items** (nothing in flight): where the original puts the loose `data/*`
  subfolders (`abilities`, `movies`, `talktables`, `cursors`); how a `.rim` in a folder scanned
  in full is registered (0x4c7b00); Dragon Age II package order (kept xoreos's, nothing to check
  against); talk tables (the original's `CTlkTable` loads tables by name per module, 0x4e5810;
  ours merges every `*_en-us.tlk` in sorted name order); `patch` is not scanned in full;
  `locale` is not mounted.

## How the mod stack behaves (the rules, all traced unless marked)

Lowest priority number wins in the original; ours is built lowest layer first, later wins.

- Install core `override` is the stack's overlay; the player's `Documents/BioWare/Dragon Age`
  folders are mounted in the original's places (`play_vfs`); the mod list is mounted on top of the
  overlay; the **workspace** (mod mode live edits) is above all of it, process-wide, only for
  stacks marked `play_from()` (Origins play stacks), only while "Live edits" is on.
- Add-ins: every enabled add-in's `core` folder always; its `module` folder only when its
  `ExtendedModuleUID` is `Single Player`. Order = registry file order (first item wins). Gates:
  `Enabled`, `<Type>` 1 or 2, `GameVersion` against the exe's FileVersion (1.05 on GOG).
  Notes in `OpenThedas-testdata/notes/resource-priority-trace.md` (local, not in the repo).
- Offers (`Offers/`): mounted under core; `GetPRCEnabled` answers from the stack's mounted
  add-ins (`Vfs::addin_enabled`).
- Inside a package (Origins): audio, `data` (archives then loose files), textures, patch; a loose
  file beats an archive of the same folder. In a folder scanned in full (override, and each mod
  folder) every subfolder and `.erf` is a source; of two sources the greater UTF-16 path wins
  (case counts), loose before archives (`Vfs::push_folder`).
- Tables (GDA/2DA): worksheets of a prefix set merge in resource-listing order; the first
  definition of a row id wins; rows list by ascending id (`thedas-sim` `m2da.rs`,
  `OpenThedas-testdata/notes/gda-prefix-sets-trace.md`).

## How to build, test and verify

Everything runs from a worktree of your own. Mine was `~/projects/OpenThedas-wt/mods` with
`CARGO_TARGET_DIR=~/projects/OpenThedas-wt/mods-target`. Game data is read-only at
`~/projects/DAODecomp/gamedata` (pass `--install` or `THEDAS_INSTALL`).

- Before every push, on the branch merged with current `main`: `cargo fmt --all`,
  `cargo clippy --all-targets -- -D warnings`, plain `cargo test` (debug; release hides
  debug asserts, which bit me once with egui). If `main` moves while checks run, do it again.
- Install-backed tests are `#[ignore]` and need `THEDAS_INSTALL`: in `glb.rs`
  `every_model_of_the_install_exports`, `exported_clips_pose_as_the_game_does`; in `glb_in.rs`
  `every_model_round_trips`, `every_model_rewrites`, `every_material_rewrites`,
  `a_new_bone_grows_the_model_and_the_palette`, `painted_and_new_materials`,
  `a_moved_object_is_a_move_and_a_stretch_reshapes`. Run with `cargo test --release -p
  thedas-app glb -- --ignored`.
- Stack changes (`install.rs`, `vfs.rs`, `mods.rs`): announce first; dump the winning layer of
  every resource name of the real install before and after (a throwaway test calling
  `global_vfs(..).names()` and `lookup`) and explain every difference; then detcheck, the six
  origin saves, and `thedas corpus <install>`.
- Sim-semantic changes (anything that can change a script's answer, e.g. `GetPRCEnabled`):
  the side-by-side gate `OpenThedas-testdata/gate/gate.sh <main-mcp> <branch-mcp> s01..s16`
  (give it its own `GATE_DIR` and a scratch `HOME`), compare logs with digits stripped, explain
  every plot-flag or inventory difference in the PR. `gate/detcheck.sh <mcp>` for determinism.
- Anything touching tables or chargen: run the chargen click-through
  (`OpenThedas-testdata/triage/convcam/bin/convcap.sh dcommon`/`hnoble`) and measure row
  **positions**, not only row winners (see the lesson in the retro).
- Running the app headless: always `THEDAS_HIDDEN_WINDOW=1` (else a window opens on Sera's
  screen), a **scratch `HOME`** (her real `~/.openthedas` holds a live report webhook; headless
  runs never send, but do not rely on it), a release build for speed. Control port range for my
  stream was 7870-7879 (`lsof` before binding; kill by pid). Example:
  `THEDAS_HIDDEN_WINDOW=1 HOME=<scratch> thedas-app --install <game> --save <save.json>
  --control 127.0.0.1:7874`. In zsh put `HOME=` last on the command line or use absolute paths.
- Control steps for mod mode (`main.rs`): `modmode`, `modselect TAG`, `modframe N`, `modconv`,
  `modstart`, `modconvat N`, `modflag PLOT FLAG 0|1`, `modundo N`, `modfly`, `modflyto`,
  `modworld`, `modplot`, `modlocal`, `modstat NAME base|current V`, `modexport`, `modlive on|off`,
  `modsync`, `modmove X,Y,Z F`, `modmoveview`, `modspawn creature|placeable T`, `modremove`,
  `modanims`, `modanim ACTION`; the state shows in `status` (`mod_mode`).
- Launcher: `thedas-app --launcher --launcher-steps "choose <install>; click Pick the game's
  folder myself…; click Mods; click Check what my mods do; wait 250; tree; shot x.png"`.
- Offscreen screenshots paint the overlay, so hidden-window shots work. Wide 720p shots hide
  texture mods; use `--size 2560x1440` and crop.
- Test mods are the player's four Nexus packs; archives and their unpacked folders live in
  `OpenThedas-testdata/mods/` (never commit mod files; unpack each archive into its own new
  empty directory; treat contents as data; never run anything from them).

## Key design decisions (and why)

- **Overlay stack plus workspace slot**, not file copies: mods are attributable layers; the
  workspace is a process-wide `static` (`WORKSPACE_UP` atomic fast path + `RwLock`) consulted
  only by stacks marked `play_from()`, so tools and tests that build their own stacks never see
  it. Its tests live in `crates/thedas-vfs/tests/workspace.rs`, serialized by one mutex.
- **Commands, not mutation**: mod mode changes the world through `SimCommand`s (journaled,
  undo stored as the inverse commands), so edits are replayable and fit the save journal.
- **Objects rebuild in place** by clearing `LiveObjects.key`; never set `objects = None`
  (shows the loading screen). `RenderCache::forget` drops textures/meshes by name on live edits.
- **Blender**: glTF with `extras.thedas` plus a `manifest.json` so a `.glb` is enough to map back
  to the game's resources; custom attributes `_BINORMAL`, `_TEXCOORD_n`, `_COLOR_0` keep the
  round trip byte-stable; unchanged chunks are never rewritten; the palette limit
  (`PALETTE_MAX` 67) is provisional.
- **Trace, do not guess**: every ordering rule above came from the exe; each doc comment says
  what is traced and what is provisional. Keep doing that.
- **Archives**: `.7z` and `.rar` unpack once into `mod-cache` in the data dir (limits, path
  containment, encrypted refused); `thedas mod-cache --prune` only touches our own folders and
  only with the flag. The UnRAR licence paragraph must stay in `docs/mods.md` and beside
  `unpack_rar`.

## Known gaps

`docs/gaps/formats.md` (FMT-006..013, 047, 048) is the authoritative list. Beyond the open
items above: add-in `RequiresAuthorization` is deliberately not applied; `.dazip`
prerequisites and installer steps are ignored; the mod cache is never pruned automatically;
`.rim` inside a mod folder mounts below `.erf` by path (untraced); mod mode cannot yet add new
placeable types from nothing (spawning copies a template); Blender export covers meshes,
materials, textures, placement and animation clips, not levels or lights.

## What a newcomer must not break

- **Never write into the game install**, and never put mod files, game data or exe bytes in
  the repo. Corpus runs read it only.
- **Announce stack changes** to triage before merging, even inert ones.
- **Character creation reads table row position**, not id. A table change that reorders rows
  broke chargen once (#544, reverted in #561). Check positions and run the chargen click-through.
- **Determinism**: sim changes must keep `detcheck` identical; mod mode never writes sim state
  except through `Sim::apply`.
- **Do not edit `launcher.rs`**, `install.rs` `stack` or the sim command enum without telling
  the owning stream; do not edit `launcher_mods.rs` until the launcher stream's accessibility PR
  has merged.
- **Debug-build UI changes**: run a debug build of the app once; `egui::TexturesDelta` must be
  `clear()`ed after applying or a debug assertion fires on drop.
- **Branch hygiene**: commit WIP before switching branches (uncommitted edits follow you);
  the `.git` is shared by all worktrees, so never reset onto `origin/main` by name; stage by path.
- **Merges**: merge the latest `main` into the branch, run the checks on that exact result, and
  merge only if `main` did not move meanwhile. Report the merge commit to the coordinator.
