# Combat Basics and the general pause key

Report #1037 was inspected against main `5e95a0e6` (2026-11-15). The local
evidence is `OpenThedas-testdata/triage-reports/1037/`; the earlier guided-training
trace is item 5 in `notes/tapsters-playtest-traces.md` in that data directory.

## Captured state

`save_now.json` has world time 1,382,304 ms, combat game mode 1,
`tutorial_paused=true`, and one queued popup: `tut_combat_basic`, title strref
264859, body strref 264857. Guided training's mode, step, expected event and
argument are all zero; its pause is false. There is **no expected training action**
holding this save. `save_before.json` has no tutorial pause or active training.
The journal's older checkpoint contains inventory training mode 5, step 6,
expected event 1063, but that is not the report's stuck state.

The report's engine replay image shows Combat Basics asking the player to zoom
out for a better combat view and saying that Space pauses for tactical
preparation. It does not instruct the player to dismiss the panel before Space
works. `input.txt` records backward scrolling and Space at 48.900 and 56.023
seconds. Those inputs are available in our app. Space previously toggled only
`Live::paused`; it could never release the independent `tutorial_paused` flag.
Closing the popup emptied its queue and released that flag, explaining the
reported workaround. No movement order or other guided action is required here.

## Original executable evidence

Static inspection only; the original game and VM were not run. Executable:
`DAOrigins.exe`, SHA-256
`2346caee611002d4dff6ecef68eb315b63a37004dd98b39c11a9c26c0b45bfb3`.
Addresses are virtual addresses, image base `0x400000`.

- Ordinary tutorial queuing at `0x6940c0–0x6940ec` checks explore/combat, sets
  tutorial bookkeeping at `+0x84`, and calls the **general pause setter**
  `0x5d6740` with pause=true and user-origin=false. The bookkeeping flag is not
  an additional veto in that setter.
- The general setter's unpause guard, `0x5d6780–0x5d67cf`, checks guided training
  at `+0x88`. Only active guided training requires expected event 1069
  (`TRAINING_UNPAUSE`) and a user-originated request. Otherwise that guard does
  not prevent general unpause. Its accepted training event calls `0x693290`;
  refusal displays the training hint at `0x6931e0`.
- `Tutorials.ConsumeTutorial`, `0x693630`, still offers another way out: an empty
  queue clears `+0x84` at `0x6936d5` and calls the same general setter with
  pause=false at `0x6936fa`.
- Guided movement training remains distinct: `genev_tutorial.ncs` mode 2 begins
  at step 2 expecting movement command 1073 (`0x1d2–0x1e4`), then arms step 9
  expecting unpause 1069 (`0x1ef–0x201`). Its unpause handler ends training with
  `SetTrainingMode(0)` at script offset `0x856`. Thus Space is accepted after
  ordering a move, and refused before it; this is not the state in #1037.

## Replacement and reproduction

`SimCommand::Unpause` journals the player's request, retains the guided-training
guard, and releases the ordinary tutorial pause without consuming the popup.
Space toggles the effective pause, including a popup's pause, so the first press
does not accidentally add a manual pause. Cinematic tutorials retain their
deferred pause until they can be presented.

Three unit tests cover popup retention and journal replay, guided movement's
unpause guard/event, and deferred cinematic pause. The external script
`OpenThedas-testdata/sim-work/repro/pausetutorial.sh <thedas-app>` runs two
sequential hidden, silent, mods-disabled captures in separate scratch HOME
directories. It requires the untouched save to remain frozen and Space to
journal unpause and advance time without consuming the tutorial queue.

## Validation after resuming (2026-11-16)

Merged main `6d66c10e` and built separate main/candidate release app and MCP
binaries in the job's target directory. The hidden-window repro fails on main
with `Space did not journal unpause`. It passes on the merged candidate: the
untouched save remains frozen, Space journals `Unpause` and advances the world
966 ms, and no tutorial-consume command is issued. The captured candidate frame
also shows the pause banner gone while Combat Basics remains visible.

The workspace debug build, `cargo fmt --all -- --check`, and
`cargo clippy --workspace --all-targets -- -D warnings` pass. Workspace tests
pass: 1,394 passed, 25 ignored, zero failed, including all three new tests.
Heavy commands used `cargo-slot.sh`, with Cargo limited to two jobs.

An isolated copy of `gate_round2.sh`, adapted only for job-local paths,
prebuilt binaries and sequential runs, covers s01-s12 and s13 on both variants.
All twelve sweep steps return zero and have zero log differences, identical
plot-flag sequences and ending areas, and no refused saves. The s13 logs match
after normalizing artifact paths and excluding wall-time diagnostics: all 22
trace checkpoints match (last: 550 step calls, 11,919 MCP calls), and both
complete saved states are identical in Flemeth's hut at world time 1,918,259 ms.

Detcheck runs 600 ticks on each of four fixtures: siege start, Ostagar arrival,
Wilds vials and the named tower midfight. All eight variant/fixture checks match
both straight-through repetitions and save/reload halfway. The sixteen complete
saved states (straight runs, halfway saves and reloaded runs) also match between
main and candidate. There are no behavior differences in these gates; elapsed
times and artifact paths vary. The intended difference is exercised by the
Space repro above.

A before/after comparison includes a third panel labeled `Delta ×4`. With any
nonzero RGB-channel difference as the threshold, 5.229% of pixels change: the
pause banner disappears and combat advances while the tutorial stays open.
