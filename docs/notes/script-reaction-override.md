# Origins creature reaction overrides

Read-only trace of the Origins 1.05 executable. `SetReactionOverride` (471,
`0x65bb70`) and `ResetReaction` (669, `0x65bbe0`) use the same CCObject
lookup `0x5d5be0` as the [base-health handlers](script-base-health.md).
The setter writes 1 at `+0x3b8` and the full integer at `+0x3bc`; reset
clears both. Failed lookups do not mutate anything. Neither handler changes
`GroupId`, calls an update, or sends an event. Stage, map and map-pin casts
remain untraced and are not admitted.

`IsObjectHostile` (468, `0x65ba20`) calls `0x64bb40`, which resolves two
creatures and uses `0x64b900`. For different groups, if either override flag
is exactly 1, the pair is hostile iff either raw override value is exactly
1, even when it belongs to the side without the enabled flag. Otherwise
the function uses the group table. The native sight path at `0x6b8873`
reaches the same check via `0x6a2b80`. OpenThedas shares the rule between
live queries and perception's precomputed creature data, retaining existing
same-group and self behavior and the normal next-tick perception event.
Explicit cached `IsHostile` still takes precedence in `RequestTarget`.
DA2 and the group-table accessors are unchanged.

Native construction zeroes both fields. Its named-field loader at
`0x69b0d6`–`0x69b12a` reads bytes named `GroupOverridden` and
`GroupForceHsti`. OpenThedas retains them in its existing object field map;
missing keys (including after reset) represent zero. Its JSON snapshots
preserve the runtime integer, not a conversion to a native save format.
