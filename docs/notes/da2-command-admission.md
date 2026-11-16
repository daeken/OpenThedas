# DA2 script command replacement and static admission

`AddCommand` command 533 reaches `0x4f0a20`. The wrapper decodes object,
command, front, static and optional add behavior (-1), writes static to the
command, and calls `0x5cd3b0`.

## Native ordinary-script path

The admission routine rejects invalid command type zero. Drive (33) is
refused when the first waiting command is UseAbility (25). It reads table 94
using the incoming type: column `0xed36668a` for differing types, or
`0x7569547a` for the same type. For that comparison only, PlayAnimation (14)
and UseAbility (25) share a type. An explicit behavior bypasses the lookup.

The native behavior dispatch at `0x5cd514..0x5cd538` handles soft clear (1)
and hard clear (2) through `0x5ca4c0`. Both discard waiting commands whose
static field is zero. Contrary to the independently traced Origins routine,
DA2 soft clear also prunes these waiting commands. Static waiting commands
survive either path.

The active executor at object `+0x2b8` is separate. The command comparison
`0x60fe40` compares type, static state, issuing flag and argument data, not
executor deadlines. The simulation also excludes computed object-movement
goals, runtime routes and dialogue stall time from that argument comparison.
Equal ordinary active commands retain their executor and
return success without another insertion. Otherwise the clear helper
`0x5c6210` checks static state and the current task's interruption flag.
Soft clear cannot clear a static active command; hard clear permits it. Both
also test task byte +4. The generic task constructor initializes it to zero,
but 0x5ef260 writes the caller's protection argument. Modern animation task 37
is explicitly enqueued with protection 1 at 0x6d861e..0x6d8629. Ordinary add
behavior therefore retains a running ability animation, even after readiness.

Readiness uses separate state. Event 41 at 0x542ade supplies argument integer
10; 0x6d95e1..0x6d95fc sets task +8 once elapsed time reaches that value plus
100 ms. The dispatcher at 0x5cd5a0 sees a single ready task and a waiting
command, then at 0x5cd6c4 calls the clear helper with result 1 and overrides
for both task protection and static state. This is a successful handoff,
independent of soft/hard admission. The simulation retains its existing
ready-animation completion path and now places new requests in the waiting
slice instead of suspending the protected active command.

At `0x5cd556`, only front value 1 inserts at the waiting-list front; all other
values append. Surviving active commands stay outside that list. Cancelled
commands report COMMAND_FAILED_COMMAND_CLEARED (-2); queued payloads start
with fresh execution state even when copied from GetCurrentCommand.
Only the started vector front is active: an older snapshot's paused command
farther down the vector is still waiting, even if it retained a deadline.

## Scope

This implements ordinary script-issued admission through AddCommand. The
native player-issued flag at command `+0x10`, its notification path, its
hard-to-soft downgrade and the equal player-issued UseAbility exception are
not represented by the current script-command data model. Native command
priority, scheduler timing and additional task kinds' protection remain outside
this bounded repair. The represented ordinary movement/wait path is
unprotected; the represented ability animation uses task 37's traced
protection and separate successful ready handoff.
Origins retains its independent admission implementation.

Regression controls exercise soft/hard replacement, active and waiting
statics, waiting-front versus back (including front value 2), equal active
payloads despite executor state, default same-type metadata, protected animation
before and after readiness, successful ready handoff, invalid objects and
snapshot reload.

Matched encounter-10008 measurements and Origins regression results follow
after the merged candidate completes its checks. No HP, damage or difficulty
values are retuned.
