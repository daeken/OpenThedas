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
queue-full command result. Other failures are generic -1, invalid data -3,
invalid path -4, no melee-ring space -5, no line of sight -6, destroyed target
-7, disabled -8, required path action -9, and timeout -10.

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

Synthetic tests cover two actors retrying cleared waits at one instant, tails
larger than ten entries, replay from a saved event batch, continued clock
advancement, callback result/owner/target and 64 front/back admissions without
dropped commands or invented failures. Getter coverage checks active/waiting
separation, index conversion, reload and completion in both games. The test
callback has a finite guard so the old scheduler fails an assertion instead
of hanging the test process; production has no added event budget.

The complete command lifecycle, interruption overrides, duplicate coalescing
and separate ClearAllCommands API remain outside this bounded repair. The
simulator does not reproduce the pump's wall-clock budget: its event processing
remains deterministic.
