# Origins trap difficulty getters

Own interpretation of the read-only GOG 1.05 executable trace.

Native 411 (`GetTrapDetectionDifficulty`) and 413
(`GetTrapDisarmDifficulty`) register the shared handler `0x668a20` at
`0x6774da` and `0x677503`. It resolves the object through `0x5d5be0` and
dispatches by native ID. Its placeable getter (object virtual `+0x24`)
returns the signed integer at `+0x644` for 411 and `+0x648` for 413
(`0x668c7b`–`0x668ca5`). Other object kinds or unresolved objects return
the accumulator's initial zero. There is no active/dead/trap-type check.

The placeable serializer binds those members to GFF IDs `0x3f84` and
`0x3f85` (`0x74d7f3`, `0x750041`). The installed GFF3 placeable resources
name those integer fields TrapDetection and TrapDisarm. The simulator
already imports template and overriding instance integers into Object.fields,
so the handlers read those existing fields directly, with zero defaults.
No parsing, scaling, clamping or save schema changes are introduced.

Three `engine::tests::trap_difficulty_` tests cover independent signed values,
integer extremes, missing values, inactive/dead placeables, other object kinds,
invalid IDs and unchanged DA2 dispatch.
