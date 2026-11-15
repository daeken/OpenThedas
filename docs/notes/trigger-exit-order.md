# Trigger changes send EXIT before ENTER (SIM-009)

Static DAOrigins.exe trace, 2026-11-15. Image SHA-256:
`2346caee611002d4dff6ecef68eb315b63a37004dd98b39c11a9c26c0b45bfb3`.
Only original prose is recorded here. No DA2 ordering is inferred.

The creature placement helper **0x6ae100** receives the newly collected trigger IDs
and compares them with the creature's prior membership list at **+0xba8**.

The first loop (**0x6ae120–0x6ae1d0**) walks the prior list. Each ID absent from the
new list is resolved to a trigger. The call through virtual slot **+0x17c** at
**0x6ae1c2** passes the enter/leave flag as zero. The trigger wrapper **0x799850**
forwards it to **0x698850**, whose flag selection makes that event type 9, EXIT.

Only after the first loop ends does the second loop begin (**0x6ae1d5–0x6ae295**).
It walks the new list and skips IDs already in the prior list. Its call through the
same virtual slot at **0x6ae276** passes flag one, producing type 8, ENTER. Both
paths use the sender's queued zero-delay mode. Thus an update that leaves one
trigger and enters another queues every EXIT before any ENTER.

OpenThedas previously traversed the new list first, reversing that relationship.
The Origins update now compares the old list first. It preserves relative order
within each existing list and does not emit anything for shared membership. DA2
retains its previous order because this trace concerns Origins only.

A synthetic regression crosses two old and two new triggers while staying inside
a fifth trigger. It checks the complete queue sequence, cached final membership,
no duplicate events on an unchanged membership, and preserved DA2 ordering.

This correction establishes ordering between the two phases only. Native list
ordering, the ENTER guard at **0x6ae235–0x6ae24d**, disabled-event filtering,
membership cache timing, event fields, and trigger geometry are separate work.
