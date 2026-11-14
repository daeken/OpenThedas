# Origins explicitly assigned object names (SCR-029)

Static trace of Origins 1.05, image base `0x400000`; no original-game execution.

`SetName` at `0x66ba00` resolves a scriptable object through `0x5d5be0` and
writes its localized-name property through `0x4d1bb0`. That setter copies the
supplied string and marks the property resolved and explicitly assigned. It
does so for an empty string too; there is no branch restoring the template.
`GetName` at `0x66bba0` reads that same property through `0x4d1eb0`. A resolved
value bypasses the talk-table resolver at `0x4d1d50`, so an explicitly assigned
empty name stays empty.

The simulation represents this with `Object::name = Some("")`. Its existing
display-name and save paths already preserve that distinction from `None`,
which permits a template or tag fallback. Only the Origins setter changes;
DA2's empty-name behavior remains untraced and keeps its earlier fallback.

This fixes empty-name assignment, not every aspect of native names. The
handler's player-name context update is separate from the object's localized
name. Its token consumers and exact scriptable-kind eligibility are outside
this change. SCR-029 retains the separate appearance/base-appearance gap.
