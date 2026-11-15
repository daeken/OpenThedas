# Origins trigger event fields (SIM-009)

Static DAOrigins.exe trace, 2026-11-15. Image SHA-256:
`2346caee611002d4dff6ecef68eb315b63a37004dd98b39c11a9c26c0b45bfb3`.
Only original prose is recorded here. No DA2 behavior
is inferred.

The creature placement path compares cached and newly collected trigger IDs through
**0x6ae100**. For each transition, it calls the receiving trigger's virtual slot
+0x17c (**0x799850**). The caller passes enter/leave, creature ID, creature ID,
integer zero and a final flag of zero (**0x6ae19f–0x6ae1c2** for EXIT,
**0x6ae24f–0x6ae276** for ENTER).

The trigger wrapper forwards those arguments to **0x698850**. That sender selects
event type **8** for ENTER and **9** for EXIT and checks whether the event is
enabled. Its event fields are:

- creator = creature, at event+0x24 (**0x6988f3**);
- target = creature, at event+0x28 (**0x6988f6**);
- integer slot 0 = zero, through **0xa5b710**;
- object slot 0 = the receiving trigger ID, through **0xa5b950**
  (**0x6988fe–0x698903**).

The meaning of the two named object fields is independently established by
`GetEventCreator` (**0x6582a0**, reads +0x24 at **0x6582d5**) and `GetEventTarget`
(**0x659030**, reads +0x28 at **0x659052**).

With the final flag zero, the sender queues a zero-delay message to the trigger
through **0x64ace0** (**0x69890f–0x698924**). The creature is the message's other
party. The **message receiver is the trigger**, while the **event target is the
creature**; these are different fields.

OpenThedas already sets the creator correctly. Origins ENTER/EXIT now explicitly
sets target to the creature, integer slot 0 to zero and object slot 0 to the trigger.
Previously object slot 0 held the creature and delivery filled the absent target
with the trigger. Explicit target assignment matters because `Sim::deliver` fills
an absent event target with its receiver.

The regression moves a creature into and out of a trigger and delivers both events
to a synthetic script. The script reads creator, target, object slot 0, integer slot
0 and event type, storing them on OBJECT_SELF. This verifies the script-visible
fields after delivery and preserves the existing DA2 behavior. No save schema
changes.

This correction is independent of authored trigger coordinates. IsInTrigger cache
timing, the additional ENTER guard, enabled-event filtering and EXIT-before-ENTER
ordering remain separate work. The membership geometry and transition order are
unchanged by this event-field correction.
