# Origins scripted heartbeat (SCR-068)

Read-only static analysis of Origins 1.05 establishes the object lookup
used by `InitHeartbeat` and `EndHeartbeat`.

The initialization handler `0x666380` pops an object and float interval.
It resolves the object through `0x5d58c0`, which requires the object-manager
entry and its CCObject type before invoking creature cast virtual `+0x10`.
The handler calls `0x69f440` only when the resulting creature exists.
That method stores the requested float interval at `+0xc98` and resets
the elapsed accumulator at `+0xc9c` to zero.

`EndHeartbeat` at `0x666400` uses the same creature lookup and calls
`0x69f460` on success. That method zeros both members. A failed lookup
returns through the handler's success path with object state untouched.

OpenThedas now applies the creature guard to both Origins handlers.
Non-creatures retain existing fields and queued events, including
misleading `HeartbeatMs` fields in older snapshots. Invalid IDs also
return without queuing an event. A regression checks all fifteen object
kinds, existing interval fields, queued events, JSON reload and invalid
IDs, alongside controls for both games.

The native update at `0x6be889` first requires a positive interval, adds
the update delta to elapsed, and checks whether elapsed strictly exceeds
the interval. On that update it queues event 81, fills float slot zero
with elapsed, then resets elapsed to zero. Initialization and termination
write the two float members directly. There is no minimum interval, catch-up
loop or remainder carried into the next beat. Zero, negative and unordered
intervals do not advance. Positive infinity never reaches its threshold.

The caller is creature update virtual `+0x15c`, `0x6be4b0`. It returns for
an inactive object (`+0x2c5 == 0`) or absent client mode. A mode-4 actor
filter can also return early. The heartbeat block is outside the dead-creature
and ordinary AI branches: death alone does not stop this clock. Its delta
comes from the object's timer, converted from integer milliseconds through
the float constant 0.001. The timer's full time-scaling policy and the mode-4
filter remain separate work.

The client update `0x641f90` invokes message-manager update `0x64b010`.
That routine pumps due messages through `0x64af00` **before** visiting active
objects and calling virtual `+0x15c`. The heartbeat sender calls the queued
send `0x64ace0` with zero delay, sender and receiver both the creature, and
message kind 10 (not a priority). Thus its script runs on a later pump,
not inline with the creature update. `0x64ae30` repeatedly takes due nodes
through `0x64a8c0`, within a work budget. Neither setter accesses that queue:
ending or restarting a clock cannot cancel a heartbeat already emitted.
Script-created HEARTBEAT2 events do not automatically start another clock.

OpenThedas now keeps the float interval and elapsed on each creature. The
ordinary tick pumps events first, then advances the clock once, queuing at
most one event for a later pump. Inactive and parked creatures do not advance.
Reinitialization replaces the interval and clears elapsed; termination clears
the clock. DA2 retains its earlier future-event implementation pending a trace.

Our JSON snapshots retain elapsed for deterministic continuation. The original
reader/writer (`0x6bf4f5` / `0x6b2e2e`) stores interval under field `0x405b`;
those references do not persist elapsed. Interval is encoded as float bits in
our snapshots so even NaN and infinity reload without changing their meaning.
An older creature's `HeartbeatMs` field is converted on its next update.
Already queued old events are preserved and delivered once, without rearming:
old snapshots cannot distinguish engine beats from script-created events.

Object visits currently follow deterministic object-id order, once per tick.
The native manager uses a rotating work-budgeted list, and its finer update
cadence, timer scaling, mode filters and DA2 remain open under SCR-068. This
change does not claim to reproduce that global scheduler.
