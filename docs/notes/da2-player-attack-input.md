# DA2 player attack selection input

The retained Sundermount patrol diagnostic ends with Hawke and Varric alive and
two previously encountered enemies outside their perceived-hostile lists. A
short watched continuation against archer 6824 exposes an earlier failed input.
At capsule distance 10.6663 metres, each ordinary attack request reaches
`ability_core` and selects `CommandResult` 1. The script also queries IsMoving
on Hawke and queues stationary animation 112. Three requests advance 100 ms
each, leave Hawke in place and deal zero damage. An ordinary walk to the archer
lets the next request select attack 3110 and deal damage.

This narrows that diagnostic's stall to the basic-attack selector and player
approach. The retained processing trace separately shows continuing commands and
heartbeats. The source checkpoint predates the supplied fresh chain, so its
outcome needs a separate label from fresh-chain progression.

## Native player and script paths

The native UI handler at `0x6023f0`, registered in dispatch slot `+0x58` at
`0x683707`, obtains the controlled actor and calls `0x47fa90` at `0x602445`.
It passes zero as that function's third argument. The path builds modern command
25 with integer 0 set to the default basic ability and object 0 set to the target.
At `0x47fc51..0x47fc5e`, it sets native command boolean 2 to the result of
comparing the third argument with zero. Thus this ordinary player path stores
true. A separate branch can ask the selector synchronously before admission and
sets this boolean according to its own third argument.

Native modern-command preparation `0x640c40` handles placeholder ability 1 by
sending event 111. At `0x640dff..0x640e10`, it reads command boolean 2 through
`0x8a83b0` and copies its value into event integer 2. The selection result replaces
command integer 0 at `0x640e34`. Result 1 takes the no-action completion path.

The script `CommandAttack` constructor at `0x4dda50` also builds command 25 from
the positive default ability. It writes its ability and object parameters while
leaving boolean 2 at its constructor default. The player-order entry point and
the script constructor therefore supply different selector inputs.

## Installed-script control

Two same-checkpoint event probes vary only event integer 2. With zero, the shipped
selector returns 1 for the retained archer at 10.6663 metres. With one, it returns
concrete attack 3310. Both use the same caster, target, facing and remaining event
arguments. The script's range/arc/combo tables choose that ability.

In `ability_core`, the helper beginning at bytecode `0x1ab3` receives this event
option. Its range and fallback branches admit the controlled actor into choices
that otherwise serve actors controlled by AI. This explains why a direct player
request can require a concrete attack choice before the existing approach planner
gets to run. Deferring selection until arrival would change the native phase order.

## Engine representation

`Command.da2_basic_attack_fallback` represents native boolean 2 for these basic
attacks. `order_attack` enables it for DA2 player orders. The script constructor
has a false default, and selection forwards the saved flag into event integer 2.
The flag belongs to command parameters and survives snapshot round trips.
Older command records deserialize it as false, preserving their recorded input.

The existing script continues to choose the concrete ability. The existing
approach planner then handles its range. Regression coverage exercises event
delivery through the player-order entry point, the script constructor, parameter
persistence, older records and the Origins order representation. Installed-resource
replays must measure the resulting approach and combat outcomes independently of
the scalar heading change in `da2-performance-processing.md`.

Persistent locomotion modes and performance take/release processing remain open.
The evidence here identifies a missing player selector input. Further fresh-chain
replays are needed to establish its contribution to individual wipe outcomes.
