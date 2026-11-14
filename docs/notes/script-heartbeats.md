# Origins scripted heartbeat lookup (SCR-068)

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
write the two float members directly. These traces establish the remaining
scheduling differences in SCR-068. OpenThedas uses a minimum 100 ms
interval and a proactively queued future event, integer milliseconds and
an empty payload. Reinitialization can add another event while the first
waits, and termination leaves the earlier future event queued. The native
event manager's delivery order and already-queued event disposal still
need tracing before replacing that scheduling model. DA2's handlers also
need their own trace.
