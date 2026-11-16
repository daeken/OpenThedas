# Follower formation slots

The DA2 fresh checkpoints made before the arrival-placement fix can retain Bethany
at ordinary follow slot 2 and the next companion at slot 3. This is explained by
the old `area_transition` list, not by removed prologue members: it concatenated
the hero, active followers, and `party_pool()`. Active recruits also occur in the
pool. With two recruits, `[hero, a, b, a, b]` placed them at slots 0/1 and then
overwrote their positions at slots 2/3. Commit `cce90d01` already removed that
duplication, uses the compact arrival table, and sends each member one ENTER.
Loading a historical checkpoint preserves its saved positions; a new transition
uses the corrected arrival path.

The ordinary follow start distance is larger than the displaced slot-2 distance,
so an idle follower already 2.5 m behind the hero need not re-form merely because
the save is loaded or time advances. A stepped harness walk can put it back at
slot 0 immediately. This explains why arrival checkpoints can retain the bad
placement while a later walking checkpoint has ordinary slots 0/1.

## Native slot selection

Both executables use party membership order, before testing whether a follower
can move. Dead, busy, distant, or follow-disabled members do not compact the
remaining slots. If member zero is not leading, it takes the controlled member's
vacated slot. Non-party followers come after the party members.

| Operation | Origins | DA2 |
| --- | --- | --- |
| Find follower and leader in the party, choose slot | `0x631d90`, lookup `0x631850` | `0x5c6f90`, lookup `0x654030` |
| Ordinary member slot | member index minus one | member index minus one |
| Member zero's slot when another member leads | leader index minus one | leader index minus one |
| Escort slot | member count plus escort index minus one | member count plus escort index minus one |
| Follow update calls slot selection | `0x760f68` | `0x5e181c` |
| Slot position / table selection | `0x760810` / `0x7605b0` | `0x5b3060` / `0x5b2e30` |
| Ordinary / compact table | `0xc0d240` / `0xc0d2f0` | `0xd2dfa0` / `0xd2e050` |

DA2's ordinary and compact offsets match Origins' corresponding eleven entries.
The compact flag is also set by DA2's arrival call at `0x47bce5..0x47bd02`.
The table mirroring rule is the same in both executables; it is still not modelled
by the simulation.

`GetFollowerWouldBeLocation` also uses the controlled leader. Origins follows
`0x666650 -> 0x632bb0 -> 0x632640 -> 0x631d90`; DA2 registers command 728 at
`0x52724a` and follows `0x4ec6f0 -> 0x4df260 -> 0x5ce2b0 -> 0x5caaa0 ->
0x5c6f90`. DA2 reads the controlled object at game-session offset `0x84`, and
uses its area and orientation when constructing the returned location.

## Remaining shared-path correction

Stepped walking previously triggered following only for the hero, and its follow
helper read the passed leader's own follower list. A selected companion could not
carry the party. Timed following
enumerated the party after removing the leader, which incorrectly exchanged the
hero's slot with intervening companions when member 2 or 3 led, and omitted
escorts. The script getter always used the hero and treated an escort as slot 0.
These paths now share the native slot assignment and the hero's party and escort
rosters. Eligibility is checked after slot assignment.

The fresh-run Python drivers call `move_hero`, which uses `Sim::walk`; they do not
write follower rosters or formation positions. Their placement shortcut remains
provisional, as do follow timing/retarget details beyond this slot correction,
the absent formation mirror, and the last-slot fallback beyond the table.
The original DA2 arrival screenshots support companions standing beside Hawke,
but their debug-jump route and camera are not a coordinate-level oracle for the
fresh checkpoints.

Batch probes that restore an older world directly into an existing simulation
can retain that simulation's later `next_follow_ms`: `forget_world_caches` does
not reset follow bookkeeping (the existing SIM-023 gap). This can defer following
until the earlier world catches up. The normal test driver `load` first creates
a new simulation and then calls `load_state`, so its timer starts fresh. The
control-switch runtime comparison uses fresh sessions for the same reason. This
timer issue does not explain the duplicated slots in the monotonic fresh chain.
