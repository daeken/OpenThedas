# DA2 live cluster candidate indexes

The native `GetClusterCenter` wrapper (`0x51b160 -> 0x51a4b0`) constructs
its candidate vector from the ordered perception map at creature `+0xb4c`.
At `0x51a670..0x51a69d`, each entry is resolved as a creature; an absent
creature or nonzero dead predicate is skipped before the vector append.
The dead virtual at `+0x1d8` is `0x52e810`, reading the DA2 death byte.

The simulation already uses this death predicate, but kept a dead entry in
the candidate vector with an exclusion flag. Its normal perception refresh
removes dead actors; this construction still matters for retained entries
between refreshes and for restored perception lists. That changes indexes used by
the clustering algorithm. Its final best-index test intentionally excludes
index zero: a corpse followed by a sole living hostile made that hostile
selectable, although native construction would give it index zero and
return an invalid location.

DA2 now discards dead candidates during construction. The algorithm's live
order, distance cache, first-index exclusion, ally scoring and random draws
are unchanged. Origins keeps its independently traced health-based predicate
and existing construction path.

The script-facing regression exercises the failure with a dead first entry
and a sole living hostile, then makes the first entry live as a control and
obtains the second creature's location. Existing coverage retains dead/health
cross-products for both games, native DA2 span metadata and upgrades.

No HP, damage or difficulty values are retuned.

## Matched validation

Compared baseline `baa3966d` with candidate `d4b5ac03`.
Formatting and strict workspace/all-target clippy pass. Workspace tests: 1445 passed, 25 ignored, zero failures. Debug assertions are enabled; debug symbols are disabled for workspace checks.

All seven paired Origins scenarios pass (14 executions): identical logs and plot sequences, eight identical checkpoint JSONs, no game over; Tower reaches Flemeth's hut. Four 600-tick fixtures pass repeated-run and save/reload determinism. Both full corpus runs pass with identical statistics. Diagnostic examples are also identical.

The matched `da2-story-check --aid none` runs use seed 1, complete counters, zero script errors, zero developer heals and zero kill aids.

- `main-baa3966d`: fake_prologue 33 ms, real_prologue 210166 ms; first game over 310798 ms in encounter [10008].
- `cluster4`: fake_prologue 33 ms, real_prologue 210166 ms; first game over 310798 ms in encounter [10008].

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
