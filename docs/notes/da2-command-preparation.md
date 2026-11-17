# DA2 command preparation callback timing

The waiting-list consumer `0x5ca5a0` removes its first entry before calling the
command manager. The manager's virtual entry at `0x60ff30` stores the incoming
command in the active executor at object `+0x2b8` (`0x60ff53..0x60ff5d`), then
calls its type-specific preparer at `0x60ff7a`.

For modern ability 25, the registry at `0x682a05` selects `0x640c40`. Its basic
selector event 111 therefore runs with that command already active, before
approach. If an approach or facing task is needed, it precedes callback task
50, enqueued protected at `0x641544`. Task 50 dispatches through `0x692390` to
the command-type callback registry; type 25 selects `0x6b6ab0`. This callback
sends COMMAND_PENDING (35) at `0x6b6c0e`, checks its result at `0x6b6cd8`, then
enqueues protected ability task 8 at `0x6b6d6f`. The cast-start event is inside
that task, at `0x6d80f4`. There is no additional fixed delay established by this
trace. The existing impact, readiness and endpoint intervals are retained.

The simulation previously left the command unstarted during both selection
and pending callbacks. The DA2 queue getters consequently counted it as
waiting, while GetCurrentCommand also exposed the unstarted front as current.
A callback could also replace the
front, after which the old name/target comparison allowed the old executor's
cast or deadline to be attached to the replacement.

Preparation now claims the active front before either callback, and the DA2
current getter excludes unstarted waiting entries. Selection of
placeholder ability 1 has no task yet and can be cleared through ordinary
admission. The later pending phase has the existing ability task barrier.
Callbacks can add waiting entries without suspending the preparer. After
callback execution, the full command snapshot determines whether the same
executor remains, including its started state. Target preparation's saved
marker is retained in that snapshot.

DA2 callbacks also invalidate other actors' cached fronts. Scheduling now
revalidates those snapshots and the previously selected deadline just as the
Origins path already does. Rescanning uses DA2's effective readiness deadline,
including ready handoff, rather than discarding it in favor of the animation
endpoint. Origins keeps its existing preparation order and fast path.

Own synthetic scripts exercise both callback queue views, current-command
visibility, insertion during preparation, an equal-payload replacement, and
callbacks replacing an earlier or later actor's front. These are engine tests,
not original-game captures. No HP, damage, equipment, difficulty or authored
animation interval is changed. The full native task scheduler and unrelated
command recipes remain separate boundaries.
