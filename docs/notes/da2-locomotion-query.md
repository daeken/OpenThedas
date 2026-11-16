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

Validation and matched story-check measurements are recorded after the final
candidate build. No health, damage, difficulty, ability cooldown or class-stat
constants are changed.
