# Origins engine-structure equality (SCR-074)

Static trace of DAOrigins.exe, GOG 1.05(a), image base `0x400000`, SHA-256
`2346caee611002d4dff6ecef68eb315b63a37004dd98b39c11a9c26c0b45bfb3`.
The read-only LLVM dump and function index under DAODecomp supplied the
instruction evidence; no original-game process was run.

The opcode table at `0xa31c24` sends both EQUAL (`0x0b`) and NEQUAL (`0x0c`)
to `0xa30a85`. A scalar comparison has one cell; script-struct comparison
(`0x24`) reads its byte size and compares the constituent cells from the top
down, stopping on inequality. Engine cells (`0x10` through `0x19`) take the
branch at `0xa30b9d`. It passes the engine type index and both structure
pointers through the VM host's virtual member `+0x1c` at `0xa30be0`.
`0xa30c3c` chooses the equality result or its inverse for the two opcodes.

The game VM receives the host with vtable `0xb0030c` at `0x5dbe18` through
`0x5dbe2f`. Its comparison member is `0x652cf0`. That dispatcher rejects
either null pointer, then selects the following rules:

- Event (type 0): `0x69bba0` compares the type, creator and target at
  `+0x20`, `+0x24`, `+0x28`, then calls the parameter comparator `0xa5a3a0`.
- Location (type 1): `0x7abcb0` compares area at `+0x20` and two three-float
  vectors, orientation and position, via `0x4d4eb0`. Comparisons are exact;
  signed zeros match and NaNs do not. The simulation represents orientation
  by its existing Z-facing field; this work does not add X/Y orientation.
- Effect and item property (types 3 and 4): `0x652d35` compares only the
  persistent effect ID at `+0x20`. Payload changes and application context
  do not affect this identity. Two independently constructed valid effects
  with identical parameters still differ. The sim already preserves IDs
  through copies and setters; this change uses those IDs instead of handles.
- Command (type 2) calls `0x7420a0`, comparing its header and optional
  polymorphic payload; player (type 5) compares pointers. Command payload
  reconstruction and the unmodelled item-property/player values are outside
  this fix. Their previous host-handle comparisons remain in place.

The event parameter comparator is not ordinary vector equality. It first
checks whether each typed list is allocated. When both lists exist, it
compares elements only if their counts match; different populated counts
skip the element loop and continue with the next type. This is visible at
`0xa5a400`/`0xa5a402`, `0xa5a4cc`/`0xa5a4ce`, and the analogous later
branches. The implementation preserves that behavior, using an empty sim
list for an unallocated native list. It does not model an allocated but
empty list separately; existing setters only grow lists.

The list fields are integers (`+0x04`), floats (`+0x08`), resource identifiers
(`+0x0c`), object IDs (`+0x10`), strings (`+0x14`), vectors (`+0x18`) and
quaternions (`+0x1c`). Floats and vector components compare numerically and
exactly; strings compare case-sensitively through `0x4c0e50`.
The sim stores resource names rather than native identifiers, so their
existing case-insensitive resource-name convention supplies this comparison.
This is not a claim of parity for identifier/hash collisions.

Origins' SetEventLocation at `0x6585f0` copies only the location's position
into the event vector list through `0xa5a9b0` (`0x65864e`–`0x658674`).
GetEventLocation at `0x658470` reads that vector, derives area from the event
creator, and supplies zero orientation. Consequently, equality compares
only the positions of the sim's event-location entries. This change does
not alter the existing setters/getters or unify their storage with DA2's
separate vector parameters. Quaternion parameters are not modelled.

The VM's `None` engine handle means a default value, including RSADD and
missing host returns. Native RSADD allocates a default structure through
host member `+0x24` (`0x652d60`), rather than storing a null pointer. The sim
therefore resolves `None` to its default event, location or zero-ID effect
for comparison. Populated handles that are stale or point at another data
type compare unequal. This does not reinterpret a stale handle as a default.

The VM now delegates both scalar engine cells and engine members of script
structs to a host comparison hook. Other hosts retain handle equality by
default. Origins' ScriptHost supplies the three rules above; DA2 retains
its previous handle comparisons pending a separate native trace. The tests
include real VM execution of hand-written NCS fixtures using sim getters
and a no-op event setter, plus per-type field changes, native list-count
behavior, NaNs, signed zero, default and stale values, and DA2 preservation.
