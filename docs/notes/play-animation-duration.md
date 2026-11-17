# CommandPlayAnimation duration (Origins)

The former 500 ms queue deadline made scripts resume before an emote finished,
repeated long ambient animations twice a second, and reported ordinary success
for continuing loops.
The queue now uses the animation tables and the creature's own appearance tree,
and reports COMMAND_LOOPING when it leaves a negative-count loop playing.
This changes Origins only; Dragon Age II's executor has not been traced here.

## Native trace

Read-only evidence: DAOrigins.exe's LLVM dump in DAODecomp/local, executor
0x7a4390, builder 0x66fc20, chain-length helper 0x6957a0 and remaining-time helper
0x7a1ae0, subaction runner 0x73cb60 and command completion 0x73d3ac–0x73d436.
Pinned executable SHA-256: 2346caee611002d4dff6ecef68eb315b63a37004dd98b39c11a9c26c0b45bfb3;
image base 0x400000, addresses here are VAs (RVA = VA minus image base).
These notes describe the behavior; no executable bytes or recovered
disassembly are included.

The builder's script arguments are nAnimation, nLoops (default 0), bPlayNext
(default 0), bBlendIn (default 1). Its native storage is different from our
script argument vector: animation is integer 0, loops integer 2, play-next
boolean 4, blend mode integer 7. Executor state is integer 3. Boolean 0 records
initialization. Float 3 is an optional playback-speed override; it is exposed
through SetCommandFloat/GetCommandFloat at that index.

At initialization, boolean 4 (bPlayNext) == 1 or native integer 4 (an explicit
next-animation override) != 0 selects the staged path. Integer and boolean
arrays are separate: 0xa5a220 reads integers, 0xa5a280 reads flags. The loop
count is integer 2, not the override. The constructor and command start handler
(0x66fc20 -> 0x744870) leave the integer-4 override unset; a nonzero nLoops
alone therefore does not select staged playback. That
path reads the initial animation's raw length through 0x694e50. Otherwise it
reads the chain length through 0x6957a0. A positive float 3 divides this first
duration; zero and negative values leave it alone. A looping initial animation
multiplies that duration by nLoops and enters state 1. A non-looping initial
animation starts in state 0.

0x6957a0 returns one raw length when its input loops. Otherwise its ordinary
path adds at most five non-looping rows, stopping before the first looping
successor. The successor is M2DA 0's next column (hash 0xffc372e7), read by
0x5ca6b0. The fallback loop test is bit 0 of flags (0x12a02374, 0x695390);
the action column is 0x7a398af9. The helper also has event-dependent alternatives
using animation event kinds 31 and 32 and creature-selected successors. Those
alternatives are not implemented by this change.

At 0x7a4646 onward, completion checks the remaining duration: [command+0x14]
minus elapsed time [command+0x0c] (zero when the stored duration is nonpositive).
The blend tail is subtracted before the state changes. State 2 completes; state
1 with a negative loop count returns COMMAND_LOOPING (2). A finite state 1 completes unless
the staged path was selected, in which case it plays the next row at speed 1,
waits for that row's chain length, and enters state 2 (0x7a4711–0x7a4767).

State 0 resolves the initial row's next animation. Without the staged path it
completes after the initial chain. With that path, a looping successor waits
for its raw length times nLoops and enters state 1; a non-looping successor
waits for its raw length and enters state 2 (0x7a478b–0x7a4887). Thus loops can
apply to an entering animation's following loop, not only the requested row.
A loop count of zero skips the loop; it does not mean one repetition.

COMMAND_LOOPING does **not** block the queue forever. The subaction runner
(0x73cc99 -> 0x73cc5e) removes nonzero results, and the enclosing command update
(0x73d3ac–0x73d436) passes result 2 through command completion. `script.ldf`
names 1 COMMAND_SUCCESSFUL and 2 COMMAND_LOOPING. A directly requested negative
loop therefore releases the command at the first scheduler tick; an entering
animation with bPlayNext and a negative count releases it when its successor
loop starts. Without bPlayNext, an entering command waits for its initial chain
and completes normally; a directly requested loop still counts nLoops.

The completion-event marshaller (0x743dd0, particularly 0x743e95–0x743ea3)
copies the stored command result into event integer 1 without remapping it.
COMMAND_LOOPING therefore reaches the script as result 2.

Raw length is 0x694e50 -> 0xa5f7a0 -> 0xa38ed0: .gad field 4009 times 1000.
It does not divide by the tree-node or .gad speed. The shared combat animation
reader computes both raw and playback lengths; this command deliberately uses
its raw value, applying only the explicit command speed to the initial stage.
The shared reader's nearest-millisecond rounding is retained.

## Implementation and limits

The animation reader and cache are shared with ability timing. Appearance
table 2, column 0x500976ec supplies the creature's own tree, including sitting,
working and emotes absent from combat_humanoid. Appearance is truncated to 16
bits. Its own tree is searched first, then the existing weapon/style/combat
trees for explicitly requested combat actions. That priority is provisional;
this trace did not recover the animation component's entire tree stack.

Negative loop counts retain the continuing loop as saved presentation state
after reporting COMMAND_LOOPING and releasing the queue. Starting another
non-wait command clears that pose; waits keep it. This pose replacement policy
is provisional beyond the traced command's own successor selection.
Missing animations have no invented duration
and complete on the scheduler's minimum 1 ms tick. Unknown successors contribute
zero rather than discarding the known part of a chain. The five-row bound also
prevents malformed next cycles from hanging the simulation.

Presentation now uses the recorded command start instead of subtracting 500 ms
from its deadline, repeats table-marked loops and applies the command speed.
It still shows the requested finite action rather than advancing
through all transition/loop/end actions. Component-specific event-driven next
selection, blend-tail timing, native animation interrupts, same-animation
continuation, and initialization failure result codes remain outside this change.
Neither combat effect expiry nor quickbar settling was changed.

Unit tests cover transitions, finite/zero/negative loops, bounded cycles, missing
resources, appearance changes, the float-3 override, completion and save/reload,
DA2's previous duration, and authored .anb/.gad resources through the shared
reader/cache (including raw versus playback speed).

## Regression review against da95f9dd

Real resources give greeting 606 a duration of 8,667 ms (4,333 with float-3 speed
2), and chair enter 910 with two loops 5,400 ms without play-next versus 18,433
with it. These replace 500 ms deadlines, so ambient callbacks, random draws and
later combat/movement can occur in a different order. Four 600-tick determinism
cases passed repeat and save/reload comparisons for both binaries.

The original reference gate's night, father, Joining and Lothering steps retained identical
ordered plot writes and effective final flags. The board step differs by seven
flags: its candidate checkpoint cannot route to the board and does not advance
time on retries. Main also rejects that move from the same checkpoint. The
extended chained candidate run exposed a cleared-command retry cycle at Gilmore's
first reply: player_core/rules_core continually requeue CommandWait after
cleared-command failures, result -2, for dog and mother at one instant. A control retaining an
active conversation reproduces the cycle in both main and candidate. These
state-dependent blockers required sim review before landing; restoring invented
animation deadlines would hide them. The [queue follow-up](origins-command-queue-failure.md)
corrects the initial queue-full diagnosis, separates event dispatch from command
starts, and traces the native waiting/executor boundary. Its validation follows
the current merged main rather than this historical baseline. Path routing,
effect-expiry and quickbar logic remain outside this animation change.

The independent frozen Tower input stalls in main's event/mover loop. Candidate
reaches Flemeth's hut on its second attempt (6,348 steps, 135,372 calls; the
existing harness supplies 42 heals). This is a divergent comparison, not a clean
Tower parity result. Detailed input hashes, traces and captures accompany the
job report. Temporary diagnostic code was removed and the production binary
rebuilt.

## Final merged validation

The queue/completion follow-up now passes the full fresh s01-s16 chain on
main `adf58096`, including Gilmore, the Tower, the board (0.0 m) and camp.
Unmodified main and the repair-only control also pass all 16 steps, with
zero save refusals across all 48 processes. Four determinism cases pass repeat
and save/reload comparisons. Workspace checks pass with 1,469 tests passed,
25 ignored and zero failures.

The [queue trace and regression review](origins-command-queue-failure.md)
maps every remaining plot difference and the saved-state difference classes.
Against unmodified main the final plot differences are the dog's random-item
flag and the genlock-emissary codex unlock. The repair-only animation control
isolates nine final timing-cascade flags, including optional dialogue and
combat/tutorial outcomes. These are functional effects of changed callback,
RNG and combat timing, not a claim of identical saved worlds. Animation
deadlines and the implementation limits described above are unchanged.
