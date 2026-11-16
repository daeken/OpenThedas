# DA2 persistent locomotion and IsMoving

Read-only tracing uses the user's local DA2 analysis image. No original-game
session was run. Executable material and shipped scripts remain in external
testdata; addresses below identify that image.

Command 388 is registered at `0x52694d` to wrapper `0x4ec600`. The wrapper
resolves a creature, defaults to false on failure, and calls `0x52e550`.
That getter tests creature `+0xa30` for nonzero. This is locomotion component
`+0xf0`, independent of both the command queue and task-37 animation motion.

Setter `0x8a3db0` retains modes 0 (idle), 1/2 (walk/run), 3 (stopping), and
4 (stationary turning). `0x8a4030` selects 1/2. Stop `0x8a60e0` converts
1/2 to 3 and 4 to zero. Facing setter `0x8a3e70` enters 4 from idle when its
normalized desired direction differs by at least approximately one degree.
The ordinary update `0x8c5b60` rotates modes 3/4 before testing completion.
Without a controller blocker, mode 3 clears then reapplies the retained
facing, potentially entering 4; mode 4 clears within the same angle tolerance.
The scalar elapsed-time rotation law is recorded in
`da2-performance-processing.md` and already used by ordinary DA2 walks.
Native `SetFacing` reaches this desired-direction setter through `0x4db160`.
The noninstant creature branch of `SetFacingObject` uses the same path through
`0x4db0f0`. The instant branch directly sets orientation.

The simulation now retains a DA2-only locomotion mode, desired facing and
update timestamp in snapshots. Movement admission starts walking/running;
arrival and interrupted approaches request a stop. Stopping retains the last
movement direction, allowing the remaining turn to finish while translation
has ceased. An unstarted queued move does not itself make IsMoving true.
Stationary facing requests can make the getter true with no commands at all.
Performance take/release resets the mode at the existing binding boundaries.
Task-37 displacement is deliberately not treated as a locomotion-mode write.
Origins neither creates this state nor changes its getter fallback.

## Scope of the headless controller

The native completion tests also inspect the enabled controller's `+0x54 &
0x1c` and its linked record's `+0xd0`. The headless collision/slide model has
no equivalent retained native controller state. This implementation follows
the traced unblocked branch; it does not claim to reconstruct those physical
controller blockers, blend graphs, or performance processing/admission gates.
The latter remain separately described in `da2-performance-processing.md`.
Older snapshots without locomotion state begin idle; a subsequent movement or
facing request creates the state. Full native save import is separate.

## Validation

Merged source `bac4f227` is compared with main `765648d0`. The untouched
`da2-story-check --aid none` gate and a separate observational replay have the
same milestones, combat metrics, counters and real-arrival checkpoint JSONs.
Only wall-clock fields differ between each instrumented/uninstrumented pair.
Neither build reaches Karl; both first wipe in encounter 10008, Team 6.

| Fight observed by the story driver | Main | Candidate |
| --- | --- | --- |
| Fake prologue, through area transition | 206.7 s | 208.4 s |
| Trailing Ogre observation after transition | 1.1 s | 1.1 s |
| First real encounter 10003 | 23.0 s, clear | 23.2 s, clear |
| Emissary encounter 10008 | 36.1 s, wipe | 50.0 s, wipe |

The real-prologue milestone occurs at 208466 / 210166 ms and first game over
at 293898 / 309698 ms. All four controls have zero script errors, developer
heals and kill aids. IsMoving disappears from the candidate's unimplemented
counter (main: 338). In an exercised fake-wave query at 3300 ms, ability_core's
query of the moving hurlock returns true rather than the default false.

In the emissary interval, actual health removed by Hawke changes from
367.752041 in 56 impacts to 596.194033 in 75; Carver from 191.481519 in 20 to
190.773570 in 41; Bethany from 204.265792 in 12 to 494.434747 in 39. These
are complete driver-defined fight intervals with changed commands, targets and
RNG progression, not matched native damage-roll experiments. Median basic
cast-start gaps remain 672 ms for Hawke/Carver, 570 ms for Bethany and 1600 ms
for the emissary. Several archers retain the traced 2330 ms cadence; shorter
intervals on other archers require the separate command-admission audit.
No health, damage, difficulty, ability cooldown or class-stat constants change.

Formatting and strict workspace/all-target clippy pass. Debug workspace tests:
1386 passed, 22 ignored, zero failed. Release MCP, CLI and app builds pass.
Six serial Origins scenarios per binary have zero log or plot-sequence diffs
and seven identical complete checkpoints, all without game over. The Tower
fixture reaches Flemeth's hut on both builds. Four 600-tick Origins fixtures
pass repeat and save/reload determinism. Origins corpus: all 32864 files pass,
with the existing 5219 undeclared-action diagnostics.

An isolated same-input facing request advances exactly 100 ms with fixed
translation. Baseline faces 250 degrees immediately; the candidate retains a
turn at 198.618286 degrees and IsMoving is true. The heading law advances on
its actual intervening simulation updates. One fixed renderer/camera produces
the staged before/after/delta triptych: absolute RGB delta multiplied by four,
4.42284% of scene pixels above an any-channel threshold of 2/255. This is a
facing/query control, not original-game combat parity evidence.
