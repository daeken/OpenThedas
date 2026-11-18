# DA2 task protection beyond ability animation

The clear helper at `0x5c6210` checks active-command static state separately
from the first executor task's byte `+4`. Neither soft nor hard ordinary
admission overrides a protected task (`0x5c6267..0x5c6277`). The generic task
constructor zeroes that byte, but `0x5ef260` overwrites it from the fourth
enqueue argument. Using the constructor default alone loses the real barrier.

The command preparation registry at `0x6829a0` and task dispatch registry at
`0x6db600` connect these paths. Relevant represented task recipes are:

| Command | Task / enqueue site | Protection |
| --- | --- | --- |
| PlayAnimation 14 | animation 37, `0x61d394` | inverse of native boolean 6 |
| StartConversation 18 | approach, then dialogue 12, `0x656559` | 1 on the dialogue task |
| UseObject 31 | approach, then use 21, `0x656a6c` | 1 on the use task |
| JumpToObject 9 | jump 32, `0x6109a9` | 1 |
| JumpToLocation 10 | jump 33, `0x610a49` | 1 |
| Turn 41 | facing 42, `0x6101e3` | 1 |
| SwitchWeaponSet 40 | switch 22 / continuation 48, `0x610dc6`, `0x610e1f` | 1 |
| Legacy attack 1 | post-approach attack 2, `0x698563` | 1 |

PlayAnimation's script wrapper is registered at `0x527dd2`, enters
`0x4e7e30`, then calls constructor `0x4de010`. It writes integer arguments
0/2/7 and booleans 3/11. It leaves boolean 6 at zero. Thus script-created
animations have protected task 37; the readiness word at task `+8` is a
different field. An animation becoming ready does not make ordinary admission
able to interrupt it. Successful ready handoff has its own override.

Movement task handlers `0x6c8960` and `0x6c9170` clear byte `+4` after setup
at `0x6c8b43` / `0x6c93fa`. Wait task 36 (`0x692980`) also releases it while
initializing its duration at `0x6929ad`. The existing movement/wait phase must
remain interruptible. The simulation uses the active movement origin to
distinguish approach from the protected terminal action. Unknown/unrepresented
task recipes keep their prior behavior. Placeholder ability 1 selects its
concrete ability before constructing tasks, so selection has no task barrier.
Full native task scheduling, native
animation durations for non-ability commands, and preparation callback phases
are separate boundaries; no durations or combat values are changed here.

The admission barrier now covers these represented action phases as well as
the already protected ability animation. Regressions exercise soft and hard
clear, readiness independent of protection, waiting-copy pruning, endpoint
completion, movement/wait release and snapshot persistence. Origins admission
is independent and unchanged.

## Matched validation

Compared baseline `8860ebb8` with candidate `1af2c303`.
Formatting and strict workspace/all-target clippy pass. Workspace tests: 1540 passed, 28 ignored, zero failures. Debug assertions are enabled; debug symbols are disabled for workspace checks.

All seven paired Origins scenarios pass (14 executions): identical logs and plot sequences, eight identical checkpoint JSONs, no game over; Tower reaches Flemeth's hut. Four 600-tick fixtures pass repeated-run and save/reload determinism. Both full corpus runs pass with identical statistics. Diagnostic examples are also identical.

The matched `da2-story-check --aid none` runs use seed 1, complete counters, zero script errors, zero developer heals and zero kill aids.

- `main-8860ebb8`: fake_prologue 33 ms, real_prologue 208566 ms; first game over 294330 ms in encounter [10008].
- `task5`: fake_prologue 33 ms, real_prologue 208566 ms; first game over 294330 ms in encounter [10008].

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
