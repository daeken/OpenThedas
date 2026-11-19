# Issuing Orders: mouse button and completion event

Checked against the DA:O PC install, without running the original game.

## Verified game data

`packages/core/data/talktables/core_en-us.tlk` contains these strings. The IDs do
not run in left/right order:

| ID | Text |
| --- | --- |
| 399131 | right-click |
| 399132 | left-click |
| 399133 | right-clicking |
| 399134 | left-clicking |
| 399135 | Right-clicking |
| 399136 | Left-clicking |
| 399137 | right |
| 399138 | left |
| 330711 | Left-click |
| 330712 | Right-click |

The orders lesson's title is 398581; its instruction is 398582. That instruction
uses `<RightClickDefaultCapped/>`, rather than the uncapped spelling in the bug
brief. With default, unswapped buttons it resolves to:

> While paused, you can issue orders to your character. Right-click on your desired destination.

DAOrigins.exe's designer-tag resolver at 0x51d620 reads the `SwapMouseButtons`
option at client options +0x350 (getter 0x838210). The CRC-32 of the UTF-16LE tag
`<RightClickDefaultCapped/>` is 0xad658054; its case starts at 0x51e3c0. The
unswapped branch selects 330712; the swapped branch selects 330711. The uncapped
tag's CRC is 0x4fbe9dfd, case 0x51dcd5: unswapped 399131, swapped 399132.
The same check of the other mouse-tag cases agrees with `keys::TAGS`. There is
no inverted TLK mapping to fix.

The original's ground-order release handler, 0x70c9a0, checks the pending press
at 0xc35e77 and a valid ground point (+0xd4/+0xd0), issues the move through
0x70be80, then calls 0x693290 with event 0x431 (1073) and argument 0.
`script.ldf` names 1073 `EVENT_TYPE_TRAINING_MOVEMENT_COMMAND_ISSUED`.
`trainingsteps.gda` row 2 names the first orders step and supplies TLK 398582/398581.
In `genev_tutorial`, mode 2's `TRAINING_BEGIN` branch calls
`BeginTrainingStep(2, 1073, 0)` (script offset 0x1d2); its 1073 branch calls
`BeginTrainingStep(9, 1069, 0)` (0x1ef). Row 9 supplies TLK 398940, the Space
instruction, and 1069 is `TRAINING_UNPAUSE`; that branch ends training.
`keymap.gda` exposes keyboard actions and middle-button camera pan, but contains
no left/right mouse binding for the move order. Its defaults cannot establish a
second ground-order button.

## OpenThedas path and fix

`App::left_button` already sent a world pick to `Live::click`. For
`Pick::Ground`, that sends `OrderMove` (which queues `CommandMoveToLocation`),
then `TrainingEvent { name: "MOVEMENT_COMMAND_ISSUED", arg: 0 }`.
`training.rs` delivers that event synchronously to `genev_tutorial`, which
changes the step. The lesson remains paused until its next, Space/unpause step.

`App::right_button` only orbited the camera or cancelled targeting. In addition,
the control harness's `rclick` and `rpress`/`rrelease` bypassed that handler and
sent mouse events only to the movies. Thus both real and harness ground clicks
failed to do what the verified right-click instruction requested.

DA:O right-button taps now use the existing ground-order path on release.
Dragging keeps camera orbit and suppresses the move, including a drag returning
to its starting pixel. Movie presses retain their movie release; cancelling an
aim cannot also move. The window control steps use the same right-button
handler. Existing left-click selection and movement are retained. DA2's native
right-button behavior is unchanged.

## Provisional and remaining gaps

- The original's exact left-button behavior over empty ground is not established
  by the TLK or the traced move handler. In particular, this investigation does
  not establish a second left-button move binding. Retaining OpenThedas's
  existing left-click move is a compatibility choice required by this job.
- Right-drag camera look already existed. Its original drag threshold has not
  been traced; the new tap detector permits 4 pixels of jitter and latches a
  drag once exceeded. Relative device motion also counts when the cursor is
  confined.
- UI-057 now honors the committed Swap Mouse Buttons setting in Origins window/control input. Physical left/right exchange the existing logical actions; a press and release keep the same mapping. The resolved designer mouse tags use that setting too. The movie/world dispatch is provisional; the trace above verifies the names, not every original mouse action. Pad input and offscreen movie steps retain their logical button roles, and DA2 is unchanged.
- This fix adds ground orders only. Right-click world-object interactions and
  selection modifiers still need a separate input-binding trace. The offscreen
  `--capture --steps` movie-only right-button path is also separate from the
  window/control path tested here.

## Reproduce and check

Inspect the installed resources with `thedas dump --install INSTALL
core_en-us.tlk`, `thedas dump --install INSTALL trainingsteps.gda`,
`thedas dump --install INSTALL keymap.gda`, and `thedas script disasm INSTALL
genev_tutorial`. Keep extracted game data outside the repository.

`tools/triage/check_orders_tutorial.py BIN SAVE OUT PORT` loads an exploration
save in a hidden window, starts the shipped orders lesson, checks the resolved
TLK instruction, and verifies that its named button queues a move and advances
the script to the Space step. It also checks drag/menu suppression and existing
left-click movement. `THEDAS_INSTALL` is required. On the shared Mac, run it
through `cargo-slot.sh` and `scripts/capped.sh`, using a free assigned port.

## Recovery validation (2026-11-16)

The interrupted work was reviewed and updated by merging fetched `origin/main`
(`baa3966d`, a clean fast-forward). `cargo fmt --all` and Clippy across all
workspace targets with warnings denied passed. Debug `cargo test --workspace`
passed 1,448 tests, with 25 ignored and no failures (31 result summaries).
The three added mouse/tag tests passed.

The hidden-window Mage Harrowing check passed: the instruction's tag resolves
to Right-click, a ground tap queues `CommandMoveToLocation` and advances to
step 9/event 1069, Space ends training, drag/menu presses do not advance the
lesson, and left-click replaces the earlier move with a different destination.
The recovered driver's first run incorrectly expected `GetTlkString` to expand
designer tags; it now checks the raw instruction and calls
`ReplaceDesignerTags` for its button tag, matching the movies' two-stage lookup.

The unmodified shared UI regression driver passed 15/15 on rerun. The first
captured run passed 14/15: its fixed exit-menu click opened Options instead of
returning to the start menu. An unchanged-main exit-only comparison with an
intervening screenshot reached the start menu, and the branch rerun without
optional captures passed the exact original driver. This is a transient menu
timing observation, not an established input regression; no engine or shared
driver change was made for it. All engine runs used hidden windows and port
8036, sequentially under `cargo-slot.sh`/`capped.sh`.
