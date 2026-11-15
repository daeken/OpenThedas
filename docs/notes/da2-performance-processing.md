# DA2 performance ownership and creature processing

This is a static trace of the user's DA2 analysis executable, followed by a
comparison with OpenThedas. No original-game session was run. Addresses identify
that image rather than a portable ABI. Only our own prose is recorded here;
disassembly, resources, checkpoints and drivers remain in external testdata.

## The processing gate

The actor record constructor `0x59c030` calls `0x535520` before its conditional
full take at `0x53d020`. The full take activates the object through virtual
`+0x144`, calls virtual `+0x140`, clears pending work through `0x5ca4c0`, and
marks the object as taken in byte `+0xeb`, mask `0x80`. Its creature-specific
tail resets locomotion and clears creature byte `+0x794`, mask `8`, at
`0x53d1e0`.

The ordinary creature logical update starts at `0x5a02b0`. Its first guards
require object byte `+0xea`, mask `0x10`, and creature byte `+0x794`, mask `8`
(`0x5a02de..0x5a02f2`). Failure returns before the rest of this update. The
guarded body includes a timed script-event section at `0x5a072c..0x5a07d4`
and the command update at `0x5a07da..0x5a07e6`.

Release reaches `0x585b80` through `0x59c140`. Its creature tail resets
locomotion again and sets `+0x794 & 8` at `0x58607c`. Thus there is a concrete
take/update/release gate for ordinary creature processing. Release enables
this gate; the observed record does not save and restore an earlier value of
that bit.

Several other switches occur in the same operation and have separate uses:

- Object `+0xeb & 0x80` also suppresses the animation-setting paths at
  `0x55f030` and `0x5dcec0`. Those functions' early returns alone do not
  identify the ordinary creature update gate.
- `0x535520` calls `0x5a8900(false)`; `0x5355d0` calls it with true. This
  writes object `+0xe9 & 4`. Its consumer at `0x5a89a0` has additional
  object/controller guards. Its complete purpose remains unclassified here.
- The component returned by `0x52d500` is stored at creature `+0x7a0` and
  constructed by `0x64ba50`. Take clears its byte `+0x1cc`; release sets it.
  Update `0x661ad0` returns immediately when that byte is zero. That update
  contains direction/target/animation work; its full role is not established.
- Object `+0xe9 & 2` is cleared on take and set on release. The separate
  visual update at `0x5a0060` consumes it through virtual `+0x138` at
  `0x5a0158..0x5a016d`. It must not be conflated with creature `+0x794 & 8`.

## Commands can be admitted while processing is held

Script command 533 is registered at `0x527be4` with handler `0x4f0a20`.
The handler resolves the object, copies the static-command argument into the
command record, and calls `0x5cd3b0` with command, front/back choice and add
behavior. The full admission function ends at `0x5cd591`; it contains no
test of the performance-taken bit or the creature processing gate above.

Its observed decisions include:

- Reject an absent command or command type zero. A Drive command also has a
  guarded rejection when the first pending command is Use Ability
  (`0x5c2400(0)` reads the pending list, separate from the current command).
- Treat Play Animation as Use Ability for selecting the comparison column.
  Compare the resulting incoming/current types, then query M2DA 94
  (`commands`) using hash `0xed36668a` for different types or `0x7569547a`
  for matching types. An explicit override bypasses this table lookup.
- Apply the resulting replacement policy through `0x5ca4c0`, with a guarded
  adjustment for a current player-origin command and an ExecuteEffect
  exception. Static/current-command state participates in clearing.
- Compare the incoming command with the current one through `0x60fe40`.
  A duplicate can return success without being inserted. Player-origin Use
  Ability has an exception. The insertion helpers are `0x5c47f0` for the
  front and `0x72ce60` for the back.

The `commands` table supplies different add policies for movement, attack,
and ability commands, including different same-type and different-type
behavior for Use Ability. The script API's front argument alone therefore
does not describe admission.

The queue update at `0x5cd5a0` has its own object `+0xea & 0x10` guard. It
handles the current command, queued work and ready animation replacement,
then calls `0x5c2470`. The ordinary creature path reaches it only after
passing `+0x794 & 8`. Other object paths also call this queue update and
require their own lifecycle audit before sharing a suspension rule.

The resulting implementation boundary is ownership plus processing:
commands may be accepted while an actor is held, but its ordinary creature
tick waits for release. Take also clears work under the native static/current
rules. Rejecting all commands during a conversation or merely pausing the
old queue would each omit part of the observed lifecycle. Explicit event
delivery has a separate path through `0x5c5df0 -> 0x5c2010`; no blanket
script-event suspension is established by this trace.

## Connection to IsMoving

Command 388 uses wrapper `0x4ec600` and getter `0x52e550`. The getter reads
creature `+0xa30`, which is locomotion component `+0xf0` at creature `+0x940`.
Setter `0x8a3db0` maintains a persistent mode: zero is idle, modes 1/2 are
the walk/run choices, mode 3 retains the current speed while requesting a
stop, and mode 4 is entered for a stationary facing change. The walk/run and
soft-stop names are interpretations supported by the callers and field
updates; the integer transitions are the directly observed facts.

Facing update `0x8a3e70` can request mode 4. Stop helper `0x8a60e0` changes
1/2 to 3 and 4 to zero. Update `0x8c5c0b..0x8c5cb8` completes modes 3/4 under
controller and heading checks. Thus queued movement and a displacement in
the last tick are each insufficient representations of this getter.

The full take calls `0x8c3da0` at `0x53d184`; release calls the same helper
at `0x58601b`. It sets a nonzero locomotion mode to zero, then resets the
linked controller when present. This connects performance ownership to the
getter's actual state and supplies a precise reset boundary for a future
locomotion implementation.

The retained `ability_core` script uses IsMoving in at least two materially
different branches: bytecode `0x11c5` gates a target-dependent follow-up,
whereas `0x20c4` selects between a moving additive animation and a queued
stationary animation (113 and 112 respectively). It also reads different
combat-style columns for those choices. An always-false implementation can
therefore affect command scheduling as well as presentation. These call
sites do not establish which branch caused a particular lost fight.

## Source comparison and replay limits

At main `9dbc981d`, DA2 line handling activates bound world objects; the
separate placement/follower changes are under review in #895/#898. Neither
activation nor pose placement implements the processing lifecycle above.
`AddCommand` appends/prepends directly and does not yet apply all the native
admission/static rules. `IsMoving` still falls back to the unimplemented
return value. A complete fix needs explicit persistent locomotion state and
tests of take, held admission, release, cancellation and save/reload.

Retained matched diagnostics contain 38 IsMoving fallbacks in the larger
patrol ambush and 74 in the pre-Ogre replay, all from `ability_core.ncs`.
Both traces contain zero script errors. These counts establish exercised
missing input, not native outcomes or a proven cause of the wipes. The
retained checkpoint epoch and driver differ from the newly supplied fresh
chain; their combat timing must be reported separately.

Damage, health, chargen inputs and Karl combat are owned by the separate
damage investigation. No balance adjustment or processing heuristic is
introduced by this note.
