# Origins command queue and cleared-command retries

Static inspection of DAOrigins.exe 1.05, SHA-256
`2346caee611002d4dff6ecef68eb315b63a37004dd98b39c11a9c26c0b45bfb3`.
Addresses are VAs, image base `0x400000`. No original game was executed.
Executable and installed-script observations are distinct from the synthetic
regression tests described below.

## Capacity and admission

The first animation report called a ten-entry queue and
`COMMAND_COMPLETE [13,-2]` a queue-full failure. That interpretation was wrong.
Installed `script.ldf` names -2 `COMMAND_FAILED_COMMAND_CLEARED`; there is no
queue-full command result. The installed result constants are:

| Result | Constant |
| --- | --- |
| -1 | `COMMAND_FAILED` |
| -2 | `COMMAND_FAILED_COMMAND_CLEARED` |
| -3 | `COMMAND_FAILED_INVALID_DATA` |
| -4 | `COMMAND_FAILED_INVALID_PATH` |
| -5 | `COMMAND_FAILED_NO_SPACE_IN_MELEE_RING` |
| -6 | `COMMAND_FAILED_NO_LINE_OF_SIGHT` |
| -7 | `COMMAND_FAILED_TARGET_DESTROYED` |
| -8 | `COMMAND_FAILED_DISABLED` |
| -9 | `COMMAND_FAILED_PATH_ACTION_REQUIRED` |
| -10 | `COMMAND_FAILED_TIMEOUT` |
| 1 | `COMMAND_SUCCESSFUL` |
| 2 | `COMMAND_LOOPING` |

`AddCommand` 533 is registered to `0x66e0e0`. Valid object/command arguments
reach `0x73cd10` at `0x66e1af`. A null object/command returns false; successful
admission returns true. Admission selects clearing from the explicit override
or table 94, then inserts at the front (`0x73ccd0`) or back (`0x698c30`) of
the waiting list at object `+0x2cc`. Both paths allocate list nodes. Neither
tests a small capacity, drops the eleventh command, nor sends a capacity failure.

Both call the generic list-size increment `0x7d4f60`.
`0x7d4f84..0x7d4f94` checks the container limit `0x3fffffff`; overflow takes
the C++ length-error path, rather than returning a `COMMAND_FAILED_*` result.
This is an allocation limit, not gameplay admission backpressure. The simulator
already uses a growing vector; no invented limit or overflow retry is added.

The active executor is separate, at object `+0x2dc`. Queue size 724 is
registered at `0x6783f2..0x6783fe` to `0x66d1c0`, which reads the waiting
list's size through `0x7b51e0` at `0x66d1fc`. Index getter `0x73c6d0`
walks that same list and converts the index to unsigned 16 bits at `0x73c6fb`.
`GetCurrentCommand` uses `0x73c5f0`, reading the executor instead. These
boundaries agree with the installed LDF documentation.

## Clearing, subactions and completion delivery

Admission clearing is independent of capacity. `0x73c930` optionally removes
nonstatic waiting commands, then calls `0x73c730(-2, 0, ...)` at `0x73c9c6`
to clear the active command. Soft admission retains waiting entries; hard
admission removes nonstatic waiting entries. Static active commands survive
ordinary admission clearing. See [the admission trace](add-command-front.md).
Removed waiting commands are freed without individual completion callbacks.

`0x745b10` removes executor subactions. If a subaction is executing, it sets
the executor's cancellation flag rather than freeing that running subaction.
After the subaction returns, runner `0x73cb60` checks the flag at
`0x73cc45..0x73cc5a`, forces result -2 and removes the subaction. Other
nonzero results also remove subactions; result 1 can continue through subsequent
subactions. Negative results exit the runner. None of this tests queue capacity.

The enclosing updater `0x73d320` passes nonzero, nonsuccess results through
`0x73c730` at `0x73d42d..0x73d436`. Completion dispatcher `0x742190`
stores the result at component `+0x50` and delivery mode at `+0x4c`, then
invokes the command-specific completion handler. The generic handler
`0x746900` resets the executor, clears its current command and records the
previous command before notification (`0x7469c4..0x7469e5`). It notifies only
when the command's notification field `+0x8` is nonzero.

Marshaller `0x743dd0` creates event 36 (`COMMAND_COMPLETE`): integer 0 is the
command type, integer 1 its unchanged result, object 0 the owner, and object 1
the command target (invalid for a wait). Creator and message recipient are the
owner, not the calling script or command target. A required path action also
supplies object 2.

Delivery mode 1 invokes the owner's selector `0x698960` at `0x743efa`.
Otherwise `0x743f01..0x743f2c` enqueues a zero-delay message through
`0x64ace0`. Admission clearing passes mode 0; command-update completion passes
mode 1. Queued delivery selects the event script at delivery: table 114 can
select `rules_core` before the object's `player_core` fallback. The latter
delegates unhandled events through `HandleEvent` at installed `player_core`
bytecode `0x1198..0x11b2`.

Pump `0x64ae30` repeatedly removes/delivers due messages
(`0x64aec2..0x64aeee`) under a wall-clock processing budget. It does not run
the command updater between messages. Zero-delay messages can run in the same
pump; there is no fixed failure delay, special queue-full deferral or queue
purge here.

## Script retry and simulator repair

Installed `rules_core` handles COMMAND_COMPLETE at bytecode `0x17db`, reads
type/result at `0x17dd..0x181b`, then calls normal AI `0x20df1` at `0x1ca1`.
For attack (1) and use-ability (25) completions, `0x1bec..0x1bff` reads
event object 1 and supplies that target to the AI call. Dropping the target is
therefore a change to the retry input, including after a failed approach.
For an uncontrolled creature outside exploration/combat mode,
`0x20fbb..0x21070` chooses another idle command and admits it through wrapper
`0x160ae`. The selector's wait branches construct a 1- or 2-second wait
(`0x231bf..0x2320d`, `0x233b3..0x233fd`); the wrapper calls AddCommand at
`0x1642d`. This is ordinary idle/retry behavior. It does not interpret -2 as
capacity backpressure or request a special queue purge.

The old simulator called `next_command` before every queued delivery. That
poll starts unstarted commands. In the captured Gilmore conversation the dog
and mother's replacement waits therefore became active between callbacks.
Each callback cleared a newly started wait and sent another same-time -2
callback forever. The ten waits in that capture were a retained tail, not a
capacity limit.

Origins now drains events already due at the current simulation time before
starting another command. Zero-delay failures stay zero-delay; clearing still
records the previous command, retains the tail according to add behavior and
sends one completion to its owner, including native object parameters. Queue
size/index getters now omit the active executor and use unsigned 16-bit
indices. GetCurrentCommand only returns a started command in Origins. DA2 keeps
its existing scheduling and getter behavior.

The same owner/target marshalling now covers ordinary completion, failed
approaches and rejected preparation in Origins; a path-action failure retains
its blocker at object 2. Rejected preparation also records the previous command
before notification. The preexisting failure-result selection and approach
retry delay are unchanged; this repair does not add a delay to cleared-command
callbacks.

Synthetic tests cover two actors retrying cleared waits at one instant, tails
larger than ten entries, replay from a saved event batch, continued clock
advancement, callback result/owner/target and 64 front/back admissions without
dropped commands or invented failures. Getter coverage checks active/waiting
separation, index conversion, reload and completion in both games. The test
callback has a finite guard so the old scheduler fails an assertion instead
of hanging the test process; production has no added event budget.
Additional tests check successful, invalid-path, path-action and preparation
rejection payloads, previous-command retention and DA2's existing behavior.

The complete command lifecycle, interruption overrides, duplicate coalescing
and separate ClearAllCommands API remain outside this bounded repair. The
simulator does not reproduce the pump's wall-clock budget: its event processing
remains deterministic.

## Merged regression and timing differences

Final baseline is main `adf58096`. Three independently started s01-s16 chains
use the same frozen driver, without copying any JSON checkpoint into a chain:
C is unmodified main, A adds only the queue/completion repairs, and B adds
the animation implementation as well. All 48 step processes succeeded. All
three chains passed expected-area, zero-save-refusal, completed-Gilmore and
board-distance checks, and reached camp. Each board distance was 0.0 m.
The final B Tower step reached Flemeth's hut in its first attempt. A also
finished its bounded third-floor standoff; its longer fight is retained in
the evidence rather than skipped by the driver.

The captured active-conversation control still stalls on C until its
45-second watchdog. A and B drain the same-time batch, advance the clock and
finish Gilmore. B passes all four 600-tick determinism cases (siege, Ostagar,
Wilds vials, named Tower midfight), both repeat and save/reload. Formatting,
workspace all-target Clippy with warnings denied, and workspace tests pass:
1,469 passed, 25 ignored, zero failed. The separate completion-focused run
passes 10 tests, including the new success/failure/rejection event cases.

A and B have identical queue/completion repair code. Their remaining simulator
differences are animation execution, loop completion/pose state and float-3
animation speed. This isolates the following differences as animation-timing
cascades; it does not make the resulting optional game state byte-identical.
Plot names and effective values were checked through the game's plot metadata
and ordinary flag reads, including defaults. Raw main C versus B ends with
just the dog-item and genlock-emissary codex differences below. A versus B
ends with nine differences:

| Flags (A → B at camp) | First difference and evidence |
| --- | --- |
| `DOG_MAIN_GIVES_RANDOM_ITEM` (false → true) | s02: both drivers make the same fetch request, but their preceding bark variant and random-item outcome differ. B writes flag 20 at 43,486 ms. Earlier animation callbacks/RNG draws have already diverged. |
| `BHN_MAIN_BATTLE_IN_MAIN_HALL_COMPLETE`, `BHN_MAIN_ENTERED_BLOCKED_PASSAGE`, `BHN_GILMORE_GUARDS_RUN_IN` (false → true) | s05: retimed siege movement/combat and NPC callbacks produce the blocked-passage entry, completed hall battle and guard arrival in B. Both chains finish Gilmore and escape through the father scene. These are actual trigger/callback history differences, not reordered copies of the same writes. |
| `BHN_FATHER_GORE_LEVEL` (false → true) | s07: B records the father's gore-state branch at 535,929 ms in the retimed siege sequence. Both chains reach Ostagar on attempt 1. |
| `PRE_TT_ASKED_ALISTAIR_TRUTH`, `APP_ALISTAIR_DEC_VLOW` (false → true) | s08: the changed timing of Alistair's mage exchange relative to walk/interact gives B an additional optional companion conversation. The recorded replies include the truth question and the approval-lowering reply; the corresponding writes occur at 550,145 and 552,145 ms. A does not run that extra conversation. |
| `COD_CRT_GENLOCK_EMISSARY_UNLOCKED` (true → false) | s13: A/C's Tower encounter/combat history unlocks this codex entry; B's does not. The encounter templates, kill order and RNG history differ after retimed upstream callbacks. This is an optional codex outcome; all three chains kill the ogre, light the beacon and reach Flemeth. |
| `TUT_COMBAT_INJURY_1` (true → false) | s13: A writes the injury tutorial at 4,711,295 ms during its longer fight/recovery history; B does not. The unchanged survival harness records 19 Tower heals for A and 15 for B, with no kill aids on either side. |

Seven additional flag differences are transient. At s05 B has already recorded
`AUTOSS_BHN_FRONT_GATE_AND_GILMORE`, `BHN_MOTHER_TOLD_GO_TO_PANTRY`,
`BHN_MOTHER_DESTINATION_LARDER`, `BHN_GILMORE_ENTERED_MAIN_HALL` and
`BHN_GILMORE_LEAVES_TO_FIGHT`; their stored values agree again at s06.
The enemy-ranks tutorial differs at s07-s09 and the modal-ability tutorial at
s09; both agree again at s10. These follow different callback/checkpoint and
combat/tutorial timing, not missing mandatory progression.

The remaining saved-field differences fall into the same measured cascade:
command/event deadlines and queues; animation and continuing-pose metadata;
RNG, encounters, combat, loot, effects and corpse lifetimes; resulting door,
movement and trigger histories; optional conversations, tutorials, popups,
plots and journal entries; and creation/handle/sequence counters. Spawn and
item differences shift object IDs, including area IDs, so literal ID-indexed
JSON comparisons amplify the difference count. All checkpoint area tags and
required progression checks still agree. No animation duration was shortened.

Reproducible evidence is under the job's external capture directory,
`playanim/queue-blockers/complete-events`: source/binary/input hashes, all
three gate summaries and invariant reports, full checkpoint comparisons,
named flag reads, plot-write times and the animation-only source diffs.
The earlier `resume-20261116-2000` run is retained as a failed intermediate
comparison; its pre-completion-repair Tower timeout is not reported as a pass.
