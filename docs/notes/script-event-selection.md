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
routes. No saved state or table content changes. Disabled-event sets,
other object routes, module/area senders, script-side `HandleEvent`, and
all direct-dispatch callers remain outside this bounded fix.
