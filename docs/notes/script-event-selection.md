# Origins event script selection (SCR-065)

Static trace of Origins 1.05, image base `0x400000`, executable SHA-256
`2346caee611002d4dff6ecef68eb315b63a37004dd98b39c11a9c26c0b45bfb3`;
no original-game execution.
This describes the CCObject event-dispatch path, not every module/area sender
or the script-side `HandleEvent` path (SCR-011).

`0x698960` receives an event pointer and chooses its script. At `0x6989bd`
it requests M2DA table 114 (`engineevents`); the lookup at `0x6989d4` uses
the `Script` column hash `0xf333fe09` and the event's type as the row ID.
A nonempty result goes directly to execution. If that lookup yields no
script, `0x6989ea` checks the event's explicit-script flag at `+0x40`,
choosing its script at `+0x2c` when set, or the object's script at `+0x26c`
otherwise. The selected script is passed to `0x6985f0` at `0x698a0e`.

`0x6985f0` first checks the object's disabled-event set through `0x698490`.
If the event is enabled but the selected script is empty, `0x69865d` takes
the same disposal branch as a disabled event (`0x69863e`): destroy the
event, clear its pointer and return. That branch runs no script and emits
no diagnostic. The old SCR-065 concern about silently dropping an event
with no selected script therefore describes native behavior.

The creature constructor installs the secondary interface at `+0x70`
(`0x6b9f59`, vtable `0xb1035c`); the placeable constructor installs its
corresponding interface at `0x750d61` (vtable `0xb1bf5c`). Both tables select
`0x69a130` at slot `+0x28`. That method adjusts back to the primary object
and forwards the event to `0x698960`. These two concrete paths establish
the scope of the replacement correction.

Queued delivery reaches the same selector. The generic message dispatcher
`0x64a8c0` resolves the target's CCObject and calls primary virtual `+0xc8`.
The creature implementation `0x6a25c0` and placeable implementation
`0x74d030` forward message kind 10 to `0x698a50`. Its branch at `0x698afd`
calls `0x698960` at `0x698b11`.

`Sim::deliver` now follows this precedence for Origins creatures and
placeables: a nonempty table-114 `Script` cell, then an explicit override
when present, then the object's own script. A table script can therefore
handle an otherwise scriptless object and supersede either fallback.
An explicitly selected empty script suppresses the object fallback and
silently consumes the event. String terminators are removed; whitespace
does not turn a nonempty table script into a fallback. Selection happens
at delivery rather than when the event is queued.

Synthetic regressions execute scripts and verify the selected script,
scriptless objects, empty and missing table values, explicit-empty
suppression, queued delivery, and preservation of DA2 and module/area
routes. Disabled-event sets, other object routes, module/area senders,
script-side `HandleEvent`, and
all direct-dispatch callers remain outside this bounded fix.

## Complete-world comparison

The full s01–s16 comparison of main `d87590e5` and runtime head `636e792f`
has 32 successful exits, matching ending areas, sixteen matching story logs,
sixteen matching ordered plot-write traces and sixteen matching command
traces. All 8,895 health inspections and 44 before/after heal pairs agree.
Four 600-tick Origins determinism checks agree on repeat and save/reload,
and all six origins load and advance 60 ticks.

Of 35 complete saves, sixteen agree byte for byte. The other nineteen differ
only in `world.next_handle`, which advances less often with table dispatch.
Every compared save has an empty live-handle map. Complete recursive
comparison establishes equality of every other saved field.

An identical-input event probe isolates the allocation difference. Main
delivers COMMAND_COMPLETE through `player_core` and then `rules_core`,
allocating one extra transient event handle. The corrected dispatcher selects
`rules_core` directly. A second probe uses the Lothering Doomsayer's object
script, which delegates through `creature_core` to `rules_core`. Direct table
dispatch removes two transient allocations and one read-only plot diagnostic
for that single event. Every other saved field agrees in both probes.

Eleven engine-log files differ. A strict comparison accounts for 360 entries
whose only difference is a transient engine-handle number. It also accounts
for 12 fewer read-only Doomsayer plot diagnostics in s15 and 49 fewer in s16.
The timestamp multiset of removed diagnostics exactly matches that creature's
completed animation commands, including multiplicity. All remaining engine-log
text and metadata agree.

The complete inherited final plot maps agree at every step.

| Step | Stored entries on each side | Final values |
| --- | ---: | --- |
| s01 | 41 | Equal |
| s02 | 66 | Equal |
| s03 | 75 | Equal |
| s04 | 84 | Equal |
| s05 | 108 | Equal |
| s06 | 109 | Equal |
| s07 | 130 | Equal |
| s08 | 149 | Equal |
| s09 | 157 | Equal |
| s10 | 175 | Equal |
| s11 | 189 | Equal |
| s12 | 232 | Equal |
| s13 | 262 | Equal |
| s14 | 297 | Equal |
| s15 | 303 | Equal |
| s16 | 329 | Equal |

Sixteen installed-table event probes exercise scriptless creatures and
placeables, explicit overrides and ITEM_ONHIT/COMMAND_COMPLETE delivery.
The corpus passes in 619 seconds with 563,678 entries, 10,652 archives,
155,719 GFF round trips and all 774 typed placeable walkmeshes. The existing
5,219 undeclared-action diagnostics remain nonfatal. Human Noble and Dwarf Commoner hidden
character-creation click-throughs reach their verified names and openings.
Their staged comparisons include absolute RGB Delta ×4 panels with a
maximum-channel threshold greater than 2/255. Human Noble changes thirteen
pixels, or 0.001411%, and Dwarf Commoner is pixel-identical. Both deltas are
essentially zero.

Separate DA2 repeats and four paired complete saves agree. Both binaries'
DA2 save/reload checks differ at `next_effect_id`, from 2 to 3. The standalone
Tower control on exact main exits 3 after 2,403 seconds when its server hits
the 4 GiB physical-footprint watchdog, with the last checkpoint at 300 steps
and 6,886 calls. Candidate standalone execution is deferred after that baseline
failure. The full-chain Tower scenarios complete on both binaries.

Private evidence is retained under
`codex-mods-target/gaps/scr065-event-table-review/`, including the strict
complete-save/log classification, identical-input delegation probes,
final plot maps, binary and harness hashes, captures and control outcomes.
