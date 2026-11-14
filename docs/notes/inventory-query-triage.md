# SCR-038 story-gate triage

The complete s01–s13 gate was run against main b5223dc, then refreshed against
main 549d36e after merging the rendering changes. Both comparisons produced the
same differences. Every scenario completed with exit zero, matching ending areas,
and no refused saves; s13 reached Flemeth's hut.

## Cause and controls

A private diagnostic build recorded the arguments, old/new results and random
state of differing inventory queries. The first difference is sys_treasure asking
for a rat's backpack at time 23192: the old result contains its two equipped items
(a natural weapon and armor), while the correct result is empty. Both saved worlds
confirm those items occupy equipment slots 13 and 4, rather than the backpack.
The same mismatch occurs for all 18 rats in s02.

The script's item-material processing helper (NCS offset 0x192e) explicitly selects
backpack for creatures, then iterates that array and calls its material-reroll
helper (0x1a55). That helper consumes Random(7) at 0x1b73 and can change an item's
material. Main wrongly includes worn equipment in that processing; the candidate
correctly excludes it. The script's separate backpack-presence check was also
examined, but changing that check alone did not remove the first divergence.
Removing the erroneous material rerolls preserves worn materials and changes random
consumption. Dog's later reward changes, an additional item is created,
and subsequent object IDs, random loot, combat timing and position-dependent
triggers diverge. These are consequences of the traced backpack filter, not an
alteration to the random generator or combat code.

A private script control changes only that material-processing query to include
worn items again. It restores main's s01–s04 logs and ordered plots, including
Dog's original reward, confirming the first divergence's cause. Later differences
remain with the other filters and slot ordering still enabled; that diagnostic run
is not a substitute for the unmodified production gate below.

A second private run retained the new filters but restored the old array order.
All 13 story logs (excluding diagnostic messages) and ordered plot traces exactly
matched the full candidate. That isolates every reported story-gate difference to
filtering. Nine later saves differ between these two candidates at 153 fields:
array order separately changes which equipped item sys_treasure randomly chooses
to drop, with resulting equipment, inventory, Droppable and handle/queue bookkeeping
differences. For example,
the first affected creature loses its helmet versus its boots. The script's equipped
query, random-index selection and the executable's slot order explain that difference.
Instrumentation and copied game scripts are private validation artifacts only.

## Every gate difference

| Step | Changed log lines | Plot writes main/candidate | Explanation |
| --- | ---: | --- | --- |
| s01 | 0 | 46/46, identical | No change. |
| s02 | 49 | 67/68 | Backpack material processing changes RNG and Dog's reward; adds DOG_MAIN_GIVES_IRONBARK_WEAPON. Remaining changed plot lines are timestamps. |
| s03 | 22 | 13/13, identical | Inherited 2200 ms time shift; same conversation and plot sequence. |
| s04 | 0 | 21/21, identical | Same printed outcome and plot sequence; inherited state differs. |
| s05 | 115 | 67/64 | Shifted IDs, combat/position/timing, one fewer harness potion; no mother-banter true/false pair or entered-chapel write on this route. |
| s06 | 41 | 6/6, identical | Inherited IDs/timing, different Gilmore position and nearby trigger listing; same plot sequence. |
| s07 | 0 | 32/32, identical | Same printed outcome and plot sequence. |
| s08 | 24 | 36/36, identical | Inherited party IDs and time shift; same dialog and plot sequence. |
| s09 | 20 | 62/57, same set | Changed loot/health/combat timing; five fewer repeated basic-combat tutorial writes. Four blood vials still obtained with seed 1. |
| s10 | 4 | 47/47, same set | Party IDs/time shift and plot ordering; same conversations and return area. |
| s11 | 18 | 21/21, identical | Inherited time shift; same Joining conversation and plot sequence. |
| s12 | 4 | 90/90, same set | Party IDs/time/combat shift and plot ordering; same tower entry. |
| s13 | 14 | 167/183 | Different potion use/combat timing; eight extra basic-combat tutorial writes, two extra Alistair ON true/false and RELEASE_HOUNDS true/false pairs, one injury tutorial, one fewer salve tutorial. Beacon cleared and same Flemeth outcome. |

Plot identifiers were resolved to their resource and flag names through the engine
metadata getters, rather than interpreting opaque GUIDs. The old-order control
reproduces all these changes, including all plot additions/removals, while retaining
only the filter behavior as the relevant difference. Repeat/save-reload determinism
checks are identical in all four test saves. Corpus and six origin loads also pass.
