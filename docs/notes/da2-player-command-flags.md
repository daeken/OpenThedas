# DA2 player-issued combat commands

Read-only tracing of the retained analysis image; no VM or original-game run.

The ordinary attack input at `0x47fa90` creates modern command 25 and calls
`0x5d2330` at `0x47fcad`. Direct ability input reaches the same constructor at
`0x4800d9`. This constructor writes the player-issued field at command `+0x10`
to one, keeps static state zero, and calls ordinary admission with the supplied
front value and default add behavior. Both combat input sites pass front zero.
The basic-attack selector boolean at argument index 2 is a separate field.

Admission `0x5cd3b0` rejects invalid/blocked Drive requests first. For a flagged
command, `0x5cd403..0x5cd464` synchronously dispatches event 94 with object zero
equal to the command owner and integer zero equal to the command type. The
installed interface names this event PLAYER_COMMAND_ADDED. It precedes active
command inspection and table-94 lookup, so a script callback can change what
the subsequent admission sees.

At `0x5cd4da..0x5cd4f8`, hard clear becomes soft clear when the active command
is player-issued, except for incoming type 32. That exception is retained as a
numeric native type; its purpose is not inferred. Payload comparison at
`0x60fe40` includes issuing state. The equal-command suppression at
`0x5cd53d..0x5cd554` permits another equal flagged ability/animation to queue.
Ordinary equal script commands retain the existing executor without insertion.

The simulation saves `Command.da2_player_issued` independently from the basic
selector option. Missing historical fields deserialize as false. Script
constructors remain unflagged; copying an existing flagged command retains its
payload, as the native AddCommand wrapper only overwrites static state.
DA2 attack and ability orders now enter admission through this traced path.
Origins keeps its existing player-order behavior. The separate movement input
and the synthetic talk/use approach helpers are outside this combat-input
repair; they are not assumed to use the same native constructor.

Regression controls cover issuing-state payload inequality, equal script and
player commands, animation/ability aliases, the hard-clear downgrade and type-32
exception, synchronous callback arguments and pre-insertion waiting count,
callback queue mutation, saved flags and older records. Task protection and
preparation callback timing are separate follow-up changes. No HP, damage,
equipment or difficulty values change.

## Matched validation

Compared baseline `8c9932c6` with candidate `29dabe4b`.
Formatting and strict workspace/all-target clippy pass. Workspace tests: 1509 passed, 27 ignored, zero failures. Debug assertions are enabled; debug symbols are disabled for workspace checks.

All seven paired Origins scenarios pass (14 executions): identical logs and plot sequences, eight identical checkpoint JSONs, no game over; Tower reaches Flemeth's hut. Four 600-tick fixtures pass repeated-run and save/reload determinism. Both full corpus runs pass with identical statistics. Diagnostic examples are also identical.

The matched `da2-story-check --aid none` runs use seed 1, complete counters, zero script errors, zero developer heals and zero kill aids.

- `main-8c9932c6`: fake_prologue 33 ms, real_prologue 208566 ms; first game over 294330 ms in encounter [10008].
- `player5`: fake_prologue 33 ms, real_prologue 212766 ms; first game over 313596 ms in encounter [10008].

| Fight | Baseline ms / result | Candidate ms / result |
| --- | --- | --- |
| pro000ar_blightlands_fake, encounters [187, 10000, 10001, 10002] (fight 1 / 1) | 206800 / area_transition | 211000 / area_transition |
| pro000ar_blightlands_real, encounters [187] (fight 2 / —) | 1100 / combat_ended | — |
| pro000ar_blightlands_real, encounters [10003] (fight 3 / —) | 22600 / combat_ended | — |
| pro000ar_blightlands_real, encounters [10008] (fight 4 / 3) | 36500 / game_over | 53900 / game_over |
| pro000ar_blightlands_real, encounters [187, 10003] (fight — / 2) | — | 25931 / combat_ended |

Party totals below are damage / impact count; observed differences are reported without changing HP, damage, equipment or difficulty.

| Fight | Actor | Baseline | Candidate |
| --- | --- | --- | --- |
| 1 / 1; encounters [187, 10000, 10001, 10002] | player | 3763.897029 / 365 | 3866.827777 / 385 |
| 2 / —; encounters [187] | player | 0.000000 / 0 | — |
| 2 / —; encounters [187] | gen00fl_carver | 0.000000 / 0 | — |
| 2 / —; encounters [187] | gen00fl_bethany | 0.000000 / 0 | — |
| 3 / —; encounters [10003] | player | 254.303524 / 37 | — |
| 3 / —; encounters [10003] | gen00fl_carver | 185.153630 / 25 | — |
| 3 / —; encounters [10003] | gen00fl_bethany | 160.542845 / 13 | — |
| 4 / 3; encounters [10008] | player | 443.900789 / 66 | 779.917321 / 89 |
| 4 / 3; encounters [10008] | gen00fl_carver | 214.514928 / 24 | 195.308149 / 37 |
| 4 / 3; encounters [10008] | gen00fl_bethany | 158.832486 / 10 | 411.818036 / 40 |
| — / 2; encounters [187, 10003] | player | — | 187.243857 / 31 |
| — / 2; encounters [187, 10003] | gen00fl_carver | — | 145.889137 / 22 |
| — / 2; encounters [187, 10003] | gen00fl_bethany | — | 266.867005 / 17 |

Fight numbers above are baseline / candidate. Different encounter groupings appear as separate rows. Full-story durations reflect carried party health and progression, not equal-start isolated fights.

Full verdicts retain per-combatant ability totals, impact spacing, health, encounter teams and unimplemented-call counts in this job's local evidence.
