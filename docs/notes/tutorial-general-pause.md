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

## Dalish tutorials-off encounter (2026-11-16)

The second finding is
`playtest-agent/findings/dalish-disabled-tutorial-pauses-combat.md` in the test
data. Its untouched `dalish-combat-stuck.json` is at 35,813 ms in combat in
`bed100ar_forest_clearing`. Training mode, step, expected event, argument and
pause are all zero. There is no active or queued cutscene. The queue holds seven
non-pausing tutorials followed by **Combat Basics**, `tut_combat_basic`, title
264859/body 264857, the only pausing entry. Thus this is the ordinary tutorial
pause, not an unfinished guided inventory or tactical-camera step. The queue
and its pause are serialized, explaining persistence across reloads; a reload
does not repeat the triggering combat event or repair the prior option delivery.

### Original disabled-option path

Static inspection of the same executable hash listed above establishes that
tutorials-off skips **both** a new popup and its pause:

- Profile option `client + 0x98` is registered under `ShowTutorials` at
  `0x5e8b38–0x5e8bbf` (key string `0xb01448`, default true written at
  `0x5e8b4c`). This is the same option checked before guided training at
  `0x692ec1–0x692ed8`.
- Plot-flag notification identifies a tutorial at `0x76014a–0x760154`, then
  reads that profile option through `0x838210` at `0x760156–0x760162`.
  Its false branch at `0x76016d` goes directly to `0x7601a7`, bypassing the
  call to tutorial queuing `0x693f10` at `0x7601a2`.
- The skipped callee would append the popup at `0x694080–0x694087`, inspect
  the plot's pause request at `0x6940c0`, then in explore/combat set `+0x84`
  and call general pause `0x5d6740(true, false)` at `0x6940db–0x6940ec`.
  Disabled tutorials therefore never reach either operation. This trace
  establishes suppression of new notifications; it does not establish that
  changing the option consumes a popup already queued.

### Replacement fault and fix

`Sim::queue_tutorial` already checks `show_tutorials`, and `set_training_mode`
also checks it. The fault was earlier: `input::handler_for` called `present`
first, which consumed the checkbox's `ClientOptions.SetClientOptionMessage`
into the presentation's options map. The bridge then marked the message
`Handled`, so `apply_all`, which takes only `Send` messages, never delivered it
to the simulation. The checkbox could be off while `Sim::show_tutorials` stayed
true. Restore Defaults had the same delivery gap. The app's `option` test step
explicitly sends to both sides and would conceal this bug.

There was also a startup gap: window creation and the initial capture setup
loaded the settings file and attached the UI without `seed_client_options`.
That function was used only when installing another game in the running app.
Starting a new process with `--save` therefore restored the disabled checkbox
but left the simulation's tutorial option at its default. Both initial attach
paths now seed the same journaled options as an in-process load.

The synchronous handler now journals `ClientOptionMessage` for UI-owned option
set/reset messages before acknowledging them. If the simulation is busy, both
updates wait for the existing deferred path. Cancel restores the simulation's
tutorial and combat-auto-pause options alongside difficulty. Existing stuck
saves recover through the general Space unpause command from the first fix;
the popup queue remains available.

`movie_options_reach_the_sim_when_handled_now_or_deferred` exercises the real
bridge acknowledgment/drain paths, tutorial disable, rejected training start,
combat-auto-pause and Restore Defaults. A simulation test checks that a disabled
tutorial plot flag queues no popup or pause, while a new notification after
Restore Defaults does both.

The additional external repro is
`sim-work/repro/pausetutorial-dalish.sh <thedas-app>` in the test data. It uses
the untouched finding save for Space recovery. To repeat the notification, a
derived copy resets only the tutorial queue/pause and Combat Basics flag 0
(`6AB6BC0D36304176B6DB4C5E5DD458CB`); encounter objects, orders and RNG remain
unchanged. It sets that plot flag with tutorials enabled, disabled through the
movie's original `ExternalCommands.SendMessageArgs`, after cancellation, after
Restore Defaults, and after loading a persisted disabled option. All runs are
sequential hidden captures with scratch HOME, no mods/audio and journal-based
PASS/FAIL checks. The derived cases test the option-to-notification boundary,
not a fresh walk from the origin's beginning.
