# Origins AddCommand admission (SCR-059)

Static inspection of DAOrigins.exe, SHA-256
`2346caee611002d4dff6ecef68eb315b63a37004dd98b39c11a9c26c0b45bfb3`.
Addresses below are virtual addresses in that build. No original game was run.

Command 533 registers handler 0x66e0e0. It reads object, command, front and
static arguments, then the optional override (default -1). It writes the static
argument into the admitted command and calls 0x73cd10 with the command, front
argument and override. A missing object or command returns false.

The object's waiting list and its active executor are separate. GetCurrentCommand
(0x65bef0 -> 0x73c5f0) reads the executor's command; GetPreviousCommand
(0x65be80 -> 0x73c5e0) reads another stored command. At 0x73ce90, exactly 1
selects insertion at the front of the waiting list (0x73ccd0); other values
append. Neither insertion transfers the active executor back into that list.

Before insertion, the override controls clearing independently of front/back:

| Behavior | Waiting commands | Active command |
| --- | --- | --- |
| 0 / other unrecognized values | Retained | Continues running |
| 1, soft clear | Retained | Cleared unless static |
| 2, hard clear | Nonstatic commands removed | Cleared unless static |
| 3, native conditional behavior | Nonstatic commands removed if the incoming target differs from the creature's combat target | Cleared unless static |

For override -1, 0x73cdef..0x73ce24 looks up merged table 94 by the incoming
command type. Column hash `ed36668a` (`AddBehavior`) applies normally;
`7569547a` applies when the active type equals the incoming type. The initial
behavior is zero if the lookup fails. Thus front insertion does not by itself
mean interrupt, restart or clear.

The clearing helper 0x73c930 visits waiting commands only when asked and removes
those whose static field is zero. It then calls 0x73c730 with result -2 and
without the static override. That helper refuses a static active command, resets
the executor's work and invokes completion through 0x64a510. The completion path
at 0x746900 records the previous command. The cleared command is not requeued.

The simulator retains an active command at vector index zero, with waiting
commands after it. AddCommand therefore inserts after that active front when
clearing leaves it alive. When clearing removes it, COMMAND_COMPLETE carries
COMMAND_FAILED_COMMAND_CLEARED (-2). Reusing a GetCurrentCommand payload admits
a fresh copy without executor timers, movement progress or impact state.
Preparation marks an attack/ability active before COMMAND_PENDING, so a script
can clear it or insert behind it during that callback. A replacement with the
same name and target must not inherit the old command's start deadline or cast.
The scheduler also checks cached fronts/deadlines when a preparation callback
changes another actor's queue, including a deadline already selected that pass.

Scope: Origins admission and start semantics; DA2 keeps its existing path.
The [queue/failure follow-up](origins-command-queue-failure.md) establishes
the growing waiting list, script-visible boundary and event-pump ordering.
Conditional behavior 3 uses the [persistent native combat-target register](origins-combat-target.md).
Adjacent duplicate coalescing and the wider clear/remove APIs are separate
command-queue work, not claims of this fix. Synthetic tests exercise the real
engine dispatch and COMMAND_PENDING script callback, not native execution.
