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

## Matched validation

Compared baseline `8860ebb8` with candidate `e4d2de35`.
Formatting and strict workspace/all-target clippy pass. Workspace tests: 1542 passed, 28 ignored, zero failures. Debug assertions are enabled; debug symbols are disabled for workspace checks.

All seven paired Origins scenarios pass (14 executions): identical logs and plot sequences, eight identical checkpoint JSONs, no game over; Tower reaches Flemeth's hut. Four 600-tick fixtures pass repeated-run and save/reload determinism. Both full corpus runs pass with identical statistics. Diagnostic examples are also identical.

The matched `da2-story-check --aid none` runs use seed 1, complete counters, zero script errors, zero developer heals and zero kill aids.

- `main-8860ebb8`: fake_prologue 33 ms, real_prologue 208566 ms; first game over 294330 ms in encounter [10008].
- `prep5`: fake_prologue 33 ms, real_prologue 208566 ms; first game over 294330 ms in encounter [10008].

| Fight | Baseline ms / result | Candidate ms / result |
| --- | --- | --- |
| pro000ar_blightlands_fake, encounters [187, 10000, 10001, 10002] (fight 1 / 1) | 206800 / area_transition | 206800 / area_transition |
| pro000ar_blightlands_real, encounters [187] (fight 2 / 2) | 1100 / combat_ended | 1100 / combat_ended |
| pro000ar_blightlands_real, encounters [10003] (fight 3 / 3) | 22600 / combat_ended | 22600 / combat_ended |
| pro000ar_blightlands_real, encounters [10008] (fight 4 / 4) | 36500 / game_over | 36500 / game_over |

Party totals below are damage / impact count; observed differences are reported without changing HP, damage, equipment or difficulty.

| Fight | Actor | Baseline | Candidate |
| --- | --- | --- | --- |
| 1 / 1; encounters [187, 10000, 10001, 10002] | player | 3763.897029 / 365 | 3763.897029 / 365 |
| 2 / 2; encounters [187] | player | 0.000000 / 0 | 0.000000 / 0 |
| 2 / 2; encounters [187] | gen00fl_carver | 0.000000 / 0 | 0.000000 / 0 |
| 2 / 2; encounters [187] | gen00fl_bethany | 0.000000 / 0 | 0.000000 / 0 |
| 3 / 3; encounters [10003] | player | 254.303524 / 37 | 254.303524 / 37 |
| 3 / 3; encounters [10003] | gen00fl_carver | 185.153630 / 25 | 185.153630 / 25 |
| 3 / 3; encounters [10003] | gen00fl_bethany | 160.542845 / 13 | 160.542845 / 13 |
| 4 / 4; encounters [10008] | player | 443.900789 / 66 | 443.900789 / 66 |
| 4 / 4; encounters [10008] | gen00fl_carver | 214.514928 / 24 | 214.514928 / 24 |
| 4 / 4; encounters [10008] | gen00fl_bethany | 158.832486 / 10 | 158.832486 / 10 |

Fight numbers above are baseline / candidate. Different encounter groupings appear as separate rows. Full-story durations reflect carried party health and progression, not equal-start isolated fights.

Full verdicts retain per-combatant ability totals, impact spacing, health, encounter teams and unimplemented-call counts in this job's local evidence.
