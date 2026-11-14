# Origins event script selection (SCR-065)

Static trace of Origins 1.05, image base `0x400000`; no original-game execution.
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

The remaining concrete discrepancy is earlier in selection. `Sim::deliver`
currently chooses only its explicit script override or the object's script;
it never consults table 114. Consequently, on this native path an event's
table script can run even when the object has no script, and can supersede
both the explicit override and object script. Our silent return is correct
only after no applicable script remains. This note does not claim complete
event-routing parity, DA2 behavior, or a trace of all direct-dispatch callers.
