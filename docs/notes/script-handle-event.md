# Origins HandleEvent (SCR-011)

Static trace of Origins 1.05 at image base `0x400000`. The original game was
never executed for this trace.

The resource overload at `0x6586d0` pops the event and script resource. It
converts the resource to text and takes the portion before its first period
through `0x658770`–`0x6587b6`. It then resolves the current script's
`OBJECT_SELF` through `0x5d5840` at `0x6587bb`–`0x6587c5`.

The resolved object's virtual member `+0x18` selects its CCObject interface.
When present, an empty explicit resource selects that object's script member
at `+0x26c` (`0x6587ea`–`0x6587fd`). The call at `0x65880e` passes the event
and selected script directly to `0x6985f0`. This bypasses the per-event
table selection at `0x698960`, tracked separately under SCR-065.

The same handler has explicit area and module branches. Area objects use
virtual member `+0x08`, default script `+0x38c`, and execution through
`0x67bfb0` (`0x658818`–`0x65884c`). Module objects use virtual member `+0x1c`,
default script `+0x1f8`, and execution through `0x5fe840`
(`0x658853`–`0x658886`). All three branches execute on the resolved caller.
The simulation event's previous delivery target supplies no recipient here.

The CCObject interface covers creature, placeable, item, trigger, sound,
waypoint, projectile, store and area-of-effect kinds. The RTTI/vtable mapping
is recorded in the generic ToString trace. Party, stage, map and map-pin
objects have a null CCObject getter and neither the area nor module type,
so this handler disposes of their event. An unresolved caller also takes the
disposal branch at `0x65888d`. A selected empty script runs no script.

The implementation supplies this selection and caller context for Origins.
The regression runs a hand-written NCS fixture which prints an integer
from its current event. It covers all fifteen simulation kinds, scriptless
objects, empty/default/explicit resource selection, suffix removal, a
different prior event target, invalid callers and invalid event handles.
Calls complete synchronously and retain the event payload. Both games are
covered, with DA2 retaining its previous behavior pending its own trace.

The native string overload at `0x6588e0` also falls back to the caller's script
but has a separate string path. It remains in the unanswered-function
inventory. Disabled-event sets and registered event listeners belong to the
broader dispatch model and remain outside this selection fix.
