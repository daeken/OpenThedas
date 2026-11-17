# DA2 desktop platform and presentation-only script calls

Read-only tracing of the DA2 analysis image identifies these registrations:

| Command | Registration | Handler | Script result |
| --- | --- | --- | --- |
| GetPlatform (694) | 0x52868c | 0x4ebfd0 | Integer 1 |
| StopAdditiveAnimation (932) | 0x5268ba | 0x4f08f0 | Void |
| DoPlayerHitEffect (936) | 0x528887 | 0x4ecfc0 | Void |

`GetPlatform` pushes literal 1; this is the desktop platform expected by the
DA2 scripts, including when the replacement engine runs on macOS.

`StopAdditiveAnimation` resolves the object, obtains its model, checks an
additive channel's state through `0x4eeb20`, then calls the model's animation
stop/fade path. It returns no script value. `DoPlayerHitEffect` ignores
nonpositive effect IDs and forwards positive IDs plus the step parameter to
`0x533640`, which looks up table 3100 and constructs a presentation effect.
It also returns no script value. These native presentation effects are not
combat damage or persistent simulation effects.

The simulation acknowledges both void calls as no-ops, matching its existing
`PlayAdditiveAnimation` boundary. This does not implement additive channels or
screen hit effects. All three handlers are DA2-only, leaving Origins' previous
dispatch unchanged. The script-facing regression checks literal platform 1,
valid/invalid animation targets, several hit-effect IDs and an unchanged
simulation snapshot.

## Matched validation

Compared baseline `9bf64597` with candidate `f6c05c43`.
Formatting and strict workspace/all-target clippy pass. Workspace tests: 1463 passed, 25 ignored, zero failures. Debug assertions are enabled; debug symbols are disabled for workspace checks.

All seven paired Origins scenarios pass (14 executions): identical logs and plot sequences, eight identical checkpoint JSONs, no game over; Tower reaches Flemeth's hut. Four 600-tick fixtures pass repeated-run and save/reload determinism. Both full corpus runs pass with identical statistics. Diagnostic examples are also identical.

The matched `da2-story-check --aid none` runs use seed 1, complete counters, zero script errors, zero developer heals and zero kill aids.

- `main-9bf64597`: fake_prologue 33 ms, real_prologue 210166 ms; first game over 310798 ms in encounter [10008].
- `stubs4`: fake_prologue 33 ms, real_prologue 210166 ms; first game over 310798 ms in encounter [10008].

| Fight | Baseline ms / result | Candidate ms / result |
| --- | --- | --- |
| 1: pro000ar_blightlands_fake, encounters [187, 10000, 10001, 10002] | 208400 / area_transition | 208400 / area_transition |
| 2: pro000ar_blightlands_real, encounters [187] | 1100 / combat_ended | 1100 / combat_ended |
| 3: pro000ar_blightlands_real, encounters [10003] | 23200 / combat_ended | 23200 / combat_ended |
| 4: pro000ar_blightlands_real, encounters [10008] | 51100 / game_over | 51100 / game_over |

Party totals below are damage / impact count; observed differences are reported without changing HP, damage, equipment or difficulty.

| Fight | Actor | Baseline | Candidate |
| --- | --- | --- | --- |
| 1 | player | 3374.698246 / 358 | 3374.698246 / 358 |
| 2 | player | 0.000000 / 0 | 0.000000 / 0 |
| 2 | gen00fl_carver | 0.000000 / 0 | 0.000000 / 0 |
| 2 | gen00fl_bethany | 0.000000 / 0 | 0.000000 / 0 |
| 3 | player | 229.085861 / 31 | 229.085861 / 31 |
| 3 | gen00fl_carver | 92.859954 / 14 | 92.859954 / 14 |
| 3 | gen00fl_bethany | 278.054185 / 17 | 278.054185 / 17 |
| 4 | player | 496.612649 / 78 | 496.612649 / 78 |
| 4 | gen00fl_carver | 310.366518 / 52 | 310.366518 / 52 |
| 4 | gen00fl_bethany | 183.497895 / 22 | 183.497895 / 22 |

Full verdicts retain per-combatant ability totals, impact spacing, health, encounter teams and unimplemented-call counts in this job's local evidence.

Unimplemented calls in the matched story drop from 262 to zero for GetPlatform, 305 to zero for StopAdditiveAnimation, and 163 to zero for DoPlayerHitEffect. All per-fight metrics remain identical.
