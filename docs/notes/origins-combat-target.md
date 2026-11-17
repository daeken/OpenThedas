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
Old snapshots without this field cannot reconstruct historical preparation;
no last-enemy or nearest-hostile heuristic belongs in the getter.
