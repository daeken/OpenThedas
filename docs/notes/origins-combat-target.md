# Origins combat target and idle recovery

Static inspection of DAOrigins.exe, SHA-256
`2346caee611002d4dff6ecef68eb315b63a37004dd98b39c11a9c26c0b45bfb3`.
Addresses are virtual addresses in that build. The original game was not run.

`GetAttackTarget` (522) is registered at 0x6780b5 with handler 0x655000.
The handler resolves a creature and reads its combat record's target
(0x655063..0x65506d). It neither consults the command queue nor tests whether
the target is alive. The script owns target validation.

Attack preparation at 0x742fd0 resolves the command's target, then stores its
id at 0x743032..0x743038, before constructing the approach subcommands.
Consequently an approach failure must not erase the prepared target. Queue
admission alone does not prepare a new target. Conditional AddCommand behavior
3 also compares against this stored target (0x73ce54..0x73ce60).

Ability preparation at 0x743f50 resolves the ability and validates the target
before the write at 0x7441fe..0x744204. Self casts preserve the target
(comparison at 0x7441f8). Unlike DA2's preparation, this write does not have
the additional party-pool exemption. A ground cast can write invalid.

Generic completion at 0x746900 records the previous command and delivers
COMMAND_COMPLETE without clearing the combat target. Ability completion at
0x746d16..0x746d32 clears the target for a party/follower creature outside
combat; it preserves it during combat. Combat-state transitions at 0x6ae0a0
call the exit helper 0x6abbf0, which clears it at 0x6abc1c. Repeating the same
state does not enter that helper. Clearing the command queue is not a combat
exit.

The shipped rules_core partial AI is selected for a controlled creature at
bytecode 0x21f41..0x21f9d. Its helper 0x24242 reads GetAttackTarget at 0x24271,
validates it, and can issue CommandAttack at 0x2435f or 0x24456. An invalid
target can leave a controlled creature without another attack. Using the
front command as the getter therefore removes an AI input exactly when
COMMAND_COMPLETE needs it. Full AI and temporary wait commands can also
observe the missing input, though their other selection paths can mask it.

The simulator's presentation accessor remains separate: it reports the
currently drawn swing. Script GetAttackTarget must report the persistent
prepared id, including stale or dead ids, until a native lifecycle clear.
Revision 12 migrates an older active attack's explicit target while its owner
is in combat, and marks that command prepared. An explicit saved register is
preserved, and an active attack outside combat keeps its cleared register.
Old idle/wait snapshots cannot reconstruct historical preparation; no
last-enemy or nearest-hostile heuristic belongs in the getter. Ability-only
history (especially a self cast's previous enemy) is likewise not invented.

## Installed-resource replay

The build-81af4d72 Proving report (#1106) supplies the arena, equipment, health,
scripts and RNG. One initial ordinary player attack starts the comparison;
there is no further input, autopilot, healing or stat adjustment. Per-combatant
100 ms samples and command-ending traces are recorded by the external
`sim-work/repro/combatidle.sh` testdata harness.

The first changed watched call is rules_core GetAttackTarget(118) at
t=2759802, immediately after its successful attack completion: invalid on
baseline, living hostile Mainar (855) with the fix. The script validates that
target and immediately queues another attack. Baseline waits for an unrelated
incoming attack event to supply an enemy again at t=2763001.

Over 30 seconds, baseline has one creature idle beyond its authored 1 s wait,
with a longest sampled gap of 3.1 s and its living hostile at most 1.9 m away.
The fix has none and no sampled idle gap during combat. Both runs have zero
script errors and end with the hero alive and Mainar dead; combat ends after
19.6 s versus 8.9 s. All observed command endings in this duel are successful;
the separate synthetic wall regression exercises an actual invalid-path end.

The five-step gate preserves all plot sequences and final areas. Lothering's
standard driver consumes two additional health poultices, and its following
board step ends 1099 ms later. A watched repeat first diverges at t=2108819:
GetAttackTarget(bandit leader 5808) returns the prepared player target (124)
instead of invalid. The watched repeat reproduces the same two potion uses.
Tower of Ishal reaches Flemeth's hut in 604 versus 576 step calls (12661 versus
11902 driver calls), with the standard driver's 23 versus 16 cheat heals and
no kill aids. These combat-decision changes require sim review; unchanged plot
sequences do not imply identical combat. The two canonical 600-tick detchecks
and the Proving checkpoint are identical both on repeat and through reload.
