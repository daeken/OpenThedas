# DA2 performance ownership and creature processing

This static trace of the user's DA2 analysis executable is followed by a
comparison with OpenThedas. Addresses identify that particular image. Native
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
this gate. The observed record does not save and restore an earlier value of
that bit.

Several other switches in the same operation have separate consumers.

- Object `+0xeb & 0x80` also suppresses the animation-setting paths at
  `0x55f030` and `0x5dcec0`. Those functions' early returns alone do not
  identify the ordinary creature update gate.
- `0x535520` calls `0x5a8900(false)`, and `0x5355d0` calls it with true. This
  writes object `+0xe9 & 4`. Its consumer at `0x5a89a0` has additional
  object/controller guards. Its complete purpose remains unclassified here.
- The component returned by `0x52d500` is stored at creature `+0x7a0` and
  constructed by `0x64ba50`. Take clears its byte `+0x1cc`, and release sets it.
  Update `0x661ad0` returns immediately when that byte is zero. That update
  contains direction/target/animation work. Its full role remains unclassified.
- Object `+0xe9 & 2` is cleared on take and set on release. The separate
  visual update at `0x5a0060` consumes it through virtual `+0x138` at
  `0x5a0158..0x5a016d`. It must not be conflated with creature `+0x794 & 8`.

## Commands can be admitted while processing is held

Script command 533 is registered at `0x527be4` with handler `0x4f0a20`.
The handler resolves the object, copies the static-command argument into the
command record, and calls `0x5cd3b0` with command, front/back choice and add
behavior. The full admission function ends at `0x5cd591`. It contains no
test of the performance-taken bit or the creature processing gate above.

The observed admission decisions follow these rules.

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

Comparator `0x60fe40` checks command type, the static-state field and the
origin field first. When both parameter objects exist it delegates to their
virtual `+8` comparison. A nonzero comparison rejects equality. It does not
compare the executor's elapsed/deadline state or the entire runtime record.
The individual parameter-object comparisons remain to be audited.

The `commands` table supplies different add policies for movement, attack,
and ability commands, including different same-type and different-type
behavior for Use Ability. The script API's front argument alone therefore
does not describe admission.

The queue update at `0x5cd5a0` has its own object `+0xea & 0x10` guard. It
handles the current command, queued work and ready animation replacement,
then calls `0x5c2470`. The ordinary creature path reaches it only after
passing `+0x794 & 8`. Other object paths also call this queue update and
require their own lifecycle audit before sharing a suspension rule.

Commands may be accepted while an actor is held. Its ordinary creature tick
waits for release. Take also clears work under the native static/current
rules. Rejecting all commands during a conversation or merely pausing the
old queue would each omit part of the observed lifecycle. Explicit event
delivery has a separate path through `0x5c5df0 -> 0x5c2010` and requires its own
ownership audit.

## Connection to IsMoving

Command 388 uses wrapper `0x4ec600` and getter `0x52e550`. The getter reads
creature `+0xa30`, which is locomotion component `+0xf0` at creature `+0x940`.
It returns whether this persistent mode is nonzero, including modes 3 and 4.
Setter `0x8a3db0` maintains the mode. Zero is idle, modes 1/2 are
the walk/run choices, mode 3 retains the current speed while requesting a
stop, and mode 4 is entered for a stationary facing change. The walk/run and
soft-stop names are interpretations supported by the callers and field
updates. The integer transitions are directly observed.

Facing update `0x8a3e70` can request mode 4. Stop helper `0x8a60e0` changes
1/2 to 3 and 4 to zero. Update `0x8c5c0b..0x8c5cb8` completes modes 3/4 under
controller and heading checks. Thus queued movement and a displacement in
the last tick are each insufficient representations of this getter.

### Persistent stop and turn completion

The complete update at `0x8c5b60` first requires component byte `+0x75`
and update-record integer `+0x34` to be nonzero. It copies the current
direction from update-record `+0x10` into component `+0xa0`. A nonzero mode
is recorded in the function's activity return before processing; that return
can therefore remain true on the update that clears the mode. `IsMoving`
reads the mode itself, not this return value.

Modes 3 and 4 both run the body-heading step, then clear desired speed
`+0xe0`. The controller update follows before completion is tested. The
observed transitions are:

| State before completion | Condition | Transition |
| --- | --- | --- |
| Mode 3 | Controller is enabled and its `+0x54 & 0x1c` is nonzero | Keep mode 3 |
| Mode 3 | That controller condition is absent | Set mode 0, then pass the retained desired direction through `0x8a3e70` |
| Mode 4 | Desired direction is nonzero and its angle from the current direction is at least `1 - epsilon` degrees | Keep mode 4 |
| Mode 4 | Direction test permits completion, but the enabled controller's linked record exists with nonzero `+0xd0` | Keep mode 4 |
| Mode 4 | Direction and controller tests permit completion | Set mode 0 |

Here epsilon is the float at `0xc2442c`, approximately `1e-6`. The direction
comparison is an **angle in degrees**, not a dot-product tolerance.
`0x8ca5d0` normalizes horizontal vectors, takes the arccosine of their
clamped dot product, and `0x8ca780` multiplies the result by the float
`57.29582595825195` at `0xbe0800`. The ordinary completion tolerance is
therefore about one degree. The meanings of the controller's `+0x54` states
and linked `+0xd0` field are not fully classified by this trace.

The mode-3 call back through `0x8a3e70` matters: a stop can transition
`3 -> 0 -> 4` in the same update when residual facing exceeds the tolerance
and the controller permits it. A zero desired vector is cleared by that
setter; a nonzero desired vector is flattened and normalized. From idle,
the setter enters mode 4 only when a turn is needed and the linked-controller
guard permits it. Stopping a stationary turn through `0x8a60e0`, in contrast,
sets mode 0 directly. A fixed one-tick stop flag or an immediate
clear-on-arrival would each omit these observed states.

The raw mode-switch table at `0x8a3e5c` confirms that mode 3 clears `+0xe4`
without immediately clearing `+0xe0`; mode 0 clears both. The next ordinary
mode-3 update clears `+0xe0` after stepping heading. The speed table at
`0x8a3c40` returns zero for modes 0, 3 and 4; only modes 1/2 perform the
appearance-speed lookup. These are distinct stored speed, requested speed
and controller-completion boundaries, not evidence for another combat-speed
constant.

The full take calls `0x8c3da0` at `0x53d184`, and release calls the same helper
at `0x58601b`. It sets a nonzero locomotion mode to zero, then resets the
linked controller when present. This connects performance ownership to the
getter's actual state and supplies a precise reset boundary for a future
locomotion implementation.

DA2 also has a directly traced body-heading step at `0x8a6110`. Its desired
direction comes from locomotion `+0x90`, and the current direction is in the
update record at `+0x10`. It requires a nonzero mode and desired direction,
with an additional independent-facing choice controlled by `+0xf8`.
`0x8c8780` converts horizontal direction to degrees, and `0x8c8ff0` wraps the
signed heading difference with bounds -180 and 180. For remaining error `e`
and elapsed seconds `dt`, `0x8a6203..0x8a6240` limits the next turn by
`180 * dt / (0.4 - 0.28 * abs(e) / 180)`, clamping to the remaining error.
Differences below the approximately `1e-6` threshold return without rotating.
The resulting direction is normalized before being written back.

Independent reads of float storage confirm milliseconds to seconds at
`0xbe1a40`, inverse 180 at `0xbf3d54`, 0.28 at `0xc237f0`, 0.4
at `0xc23498`, signed 180 at `0xbe0828/0xbe082c`, and epsilon at `0xc2442c`.
Callers include ordinary movement `0x8b11e4` and the mode-3/4 update branch at
`0x8c5be0`, with another caller at `0x8a761b`. This confirms the same scalar yaw
law previously traced for Origins. It does not establish identical route
filtering, controller completion or all movement modes between the games.

The retained `ability_core` script uses IsMoving in at least two materially
different branches. Bytecode `0x11c5` gates a target-dependent follow-up.
Bytecode `0x20c4` selects between a moving additive animation and a queued
stationary animation (113 and 112 respectively). It also reads different
combat-style columns for those choices. An always-false implementation can
therefore affect command scheduling as well as presentation. These call
sites do not establish which branch caused a particular lost fight.

## Source comparison and replay limits

DA2 line handling activates bound world objects. The separate
placement/follower changes landed in #895/#898. Neither
activation nor pose placement implements the processing lifecycle above.
DA2 `AddCommand` appends/prepends directly and does not yet apply all the native
admission/static rules. `IsMoving` still falls back to the unimplemented
return value. A complete fix needs explicit persistent locomotion state and
tests of take, held admission, release, cancellation and save/reload, including
the residual turn after a stop and the controller-dependent completion above.

Retained matched diagnostics contain 38 IsMoving fallbacks in the larger
patrol ambush and 74 in the pre-Ogre replay, all from `ability_core.ncs`.
Both traces contain zero script errors. These counts establish exercised
missing input. Native outcomes and wipe causality remain to be measured. The
retained checkpoint epoch and driver differ from the newly supplied fresh
chain, requiring separate combat-timing reports.

The retained pre-Ogre trace spans `839750..915150` ms. Its IsMoving callers
are Hawke (35), Bethany (28) and Aveline (11). Bethany and Aveline each receive
75 heartbeats at 1000 ms intervals, extending to `914566` and `914372` ms.
Completed Use Ability commands number 35, 27 and 12 respectively. Thus these
actors are not globally starved of script/command processing in that replay.
This does not classify individual ability branches or prove correct cinematic
ownership. It narrows the processing-freeze hypothesis without changing combat
inputs. The old trace predates per-blow attack entries, so it cannot supply
current attack tallies.

These caller counts identify executing script owners. Watched engine-call logs
show all 74 queries occurring during COMMAND_PENDING (35) and querying another
actor. Argument counts are 53 for object 406, five for 448, seven for 449, five
for 450 and four for 540.

The COMMAND_PENDING stack audit identifies that argument as the target variable
initialized from event object 1. At `0x11c5`, the caster must be a follower and
the valid target must be a moving non-follower. Additional ability flags and
range-band checks guard the body. The branch constructs effect type 9 with
engine float 1 set to 0.4. It passes the target, temporary duration type 2,
duration 1.5, creator SELF and ability 0 through `0x2e78` to
Engine_ApplyEffectOnObject at `0x30d7`. The installed effects schema identifies
type 9 as movement speed. The native effect executor remains to be audited.
The animation-selection branch and the target-effect branch provide separate
inputs for a future persistent-locomotion replay.

The supplied fresh_02 chain gives a separate, earlier real-Blightlands control
on main `b5f9b5d9` and candidate `b247acc9`. Both wipe on the team6 route before
Aveline joins, using ordinary potions and zero new developer heals. Bethany and
Carver each receive 100 regular one-second HEARTBEAT2 events on main and 115 on
the candidate. Their completed Use Ability commands number 31/22 on main and
36/17 on the candidate. Hawke completes 11 and 19 respectively. Both traces have
zero script errors. This fresh replay also narrows global processing starvation
while leaving individual ability and damage parity open. Its supplied source
chain includes earlier healplayer use.

Damage, health, chargen inputs and Karl combat are owned by the separate
damage investigation. No balance adjustment or processing heuristic is
introduced by this note.

## Implemented heading slice

DA2 translating creatures now use the existing elapsed-time yaw limiter, including
its saved last-turn timestamp. Ordinary started moves retain their body heading
at arrival. Native completion
`0x675fae -> 0x8a60e0` stops the follower without installing that bearing.
Explicit jumps, placement and non-creature movers keep their facing behavior. Origins follows the
same code it used before this DA2 extension.

Regressions exercise a native 33 ms quarter-turn example through the actual
movement scheduler, a zero-time update, continued turning across a world
round-trip, and a bent route whose arrival bearing differs from the original
order. This implements the scalar body-yaw step during translation. Persistent
locomotion modes, stationary turn completion, performance processing ownership,
the direction filter and complete command admission remain separate work.

The shared scalar helper also retains its existing half-turn convention.
Native `0x8c8780` obtains signed headings from direction vectors, and `0x8c8ff0`
retains both endpoints of the closed interval [-180, 180]. At exactly opposite
directions, the native turn sign can depend on the vector's signed zero. The
scalar facing representation discards that distinction. Exact antiparallel
turn-sign parity therefore belongs to the remaining vector-heading work.
