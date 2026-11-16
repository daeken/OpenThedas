# DA2 waiting commands and animation readiness

The fresh real-prologue probe on main `d87590e5` finds Carver reaching melee
range and dealing six basic impacts before dying. His animation readiness
notifications execute, validate his current hostile and forward to the next
authored AI round. That round exits at `rules_core` bytecode
`0x240d1..0x2418e` because `GetCommandQueueSize` reports one. The counted entry
is the current swing. His next basic starts at the full animation endpoint.

## Native queue boundary

Read-only tracing of the player's DA2 analysis image establishes the following
separate storage and script interfaces.

- Registration `0x527c5f` binds command 724 to `0x4fd460`. The handler resolves
  the object and reads its waiting-list count at object `+0x2b0`.
- `GetCommandByIndex`, command 536, runs through `0x4e7800 -> 0x5c2400`.
  It iterates the waiting list at object `+0x298`. The index is consumed as an
  unsigned 16-bit value.
- `GetCurrentCommand`, command 539, runs through `0x4e7750 -> 0x5a8d40`.
  It reads the active executor through object `+0x2b8`, separately from the
  waiting-list count and index operation.

The shipped `script.ldf` independently describes the active command as outside
the queue. Its description of index zero identifies the first waiting command.

The modern animation task's ready event reaches `rules_core` event 112.
The script validates event object 1, sets integer 10, changes the event to
COMMAND_COMPLETE and schedules its own handler. For a living follower in
combat, the ensuing AI round admits another command when its waiting queue is
empty. Counting the active animation prevents that authored chain.

## Simulation change

The simulation stores the active executor at the front of its command vector.
Its existing deadline distinguishes a started front from waiting entries.
DA2's queue size and index getters now expose the slice after that started
front. An unstarted front remains waiting. A ready animation remains active
until completion or replacement. Index lookup retains the native 16-bit
conversion. Origins uses its existing command-vector view.

The deadline already survives snapshots, so older in-flight records and reloads
retain this boundary. Regression coverage exercises empty and invalid objects,
an unstarted command, a running command, multiple waiting commands, index
bounds and conversion, readiness, reload, completion and Origins behavior.
The repair concerns the two waiting-queue getters. The broader native command
admission and preparation lifecycle remains provisional.

Installed-resource cadence, combat outcomes and regression checks are recorded
with the PR and in the damage-investigation report.
