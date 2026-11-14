# Scripted heartbeat scheduling triage

SCR-068's interval/elapsed/payload change was compared with exact main
`9d53c12f`, including the creature guards and placeable current-health work.
All sixteen scenarios succeed on both binaries, with the same ending areas,
including Flemeth's hut and party camp. Save refusals and script-error logs
are zero. This is a simulation change with intentional gate differences.

The first changed engine work is Gilmore's HEARTBEAT2: main delivers at
12,351 ms, the candidate at 12,365 ms. Main schedules exact future multiples
of the requested interval; the candidate emits after a creature update
exceeds it and delivers on a later pump. In s02 the watched replay reproduces
each side's complete saved state exactly. It records 63/70 delivered beats.
The first changed attack entry is at 30,333 ms on the candidate, while the
next main entry is at 30,352 ms. Targets subsequently differ. Main kills rat
474 at 32,487 ms and its death script consumes blood-pool random numbers;
the candidate has not killed it then. That changes later damage rolls,
dialogue variants and timing. The Dog reward remains the same piece of wood.

## Story trace differences

Ignoring timestamps, plot-event multiplicities match in thirteen scenarios.
The complete exceptions are:

| Scenario | Main/candidate writes | Difference |
| --- | ---: | --- |
| s05 siege | 64/66 | The entrance-hall trigger's flag-16 true/false pair happens here on the candidate. |
| s06 hall | 6/4 | Main reaches that same entrance-hall trigger here. |
| s13 beacon | 183/172 | Seven fewer repeated basic-combat tutorial writes; one fewer Alistair flag-0 and flag-15 true/false pair; injury tutorial absent and salve tutorial present. |

The other scenario write counts are respectively 46, 68, 13, 21, 32, 36,
57, 47, 21, 90, 98, 13 and 62. Some of those sequences reorder or have
different timestamps. These are trigger/combat/banter/tutorial effects of
the changed cadence; both runs complete the beacon, rescue and camp chain.
Health audits contain 9,511/9,426 inspections and 44/43 developer-heal pairs,
consistent with the different fights. They are not claimed identical.

Eighteen of the 35 saved files differ. Besides the new clock members,
differences include event due times/payloads/sequences, AI counters/targets,
health and command timing, RNG, corpse/bag creation, item rolls and ids,
later area object ids, trigger membership, banter and tutorial state. Raw
id-keyed comparison greatly overcounts semantic changes once allocation ids
diverge; the private classification report retains the full paths.

## Controls and other checks

A private build can restore only the old heartbeat scheduler. On the same
sixteen scenarios it reproduces every story log, ordered plot trace and all
35 complete saves from main exactly, with zero saved-field differences.
No control instrumentation or game scripts are committed.

A second private control suppresses only the new float payload. It reproduces
every candidate story log and ordered plot trace across all sixteen scenarios.
Thirty-four complete saves match; the only difference in the remaining save
is the float-slot list on one queued heartbeat. Thus scheduling accounts for
the gameplay differences in this sweep; none requires the payload change.

Four repeat/save-reload checks and six origin saves plus 60 ticks pass.
The fresh corpus passes 563,678 entries, 10,652 archives and 155,719 GFF
round trips, with zero archive/read failures or encrypted skips. Its JSON
is byte-identical to the previously validated corpus report. Only the known
5,219 nonfatal undeclared-action diagnostics remain.

Seven isolated compiled-script interval fixtures per binary cover 21
setters and 42 timed updates, restarts, payloads, reloads and termination.
The ordinary regressions also cover strict equality, disabled/nonfinite
intervals, stopped/inactive/parked/dead creatures, legacy pending events,
snapshot continuation, invalid types and DA2 controls.

A fresh full triage after merging interface PRs #851/#859 uses exact main
`0b42da74`. All 35 main saves and all 35 candidate saves reproduce their
earlier counterparts exactly. The same classifications and controls apply.
Final workspace checks pass 1,164 tests, zero failures and 16 ignored.

Evidence is private under `codex-mods-target/gaps/scr068-heartbeat-scheduling/`
and `gaps/scr068-heartbeat-scheduling-after859/`; the respective exact main
binaries are under `gaps/main-9d53c12f/bin/` and `gaps/main-0b42da74/bin/`.
