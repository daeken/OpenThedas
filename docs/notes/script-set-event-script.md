# Origins SetEventScript (SCR-065)

Static trace of Origins 1.05, image base `0x400000`, executable SHA-256
`2346caee611002d4dff6ecef68eb315b63a37004dd98b39c11a9c26c0b45bfb3`.
The evidence comes from static executable inspection.

The action-122 handler at `0x658aa0` ends at the aligned return `0x658c43`.
It copies the supplied string through `0x4c2510` at `0x658b43`, searches
forward for a UTF-16 period from position zero through `0x4c1000` at
`0x658b55`, then takes the preceding prefix through `0x4c30f0` at
`0x658b64`. The search delegates to `0x4c03f0`, which scans forward by
UTF-16 units. The prefix helper clamps its unsigned count to the string's
stored length, so a missing period keeps the entire string.

Assignment preserves case and whitespace. `Mixed.part.NCS` becomes `Mixed`,
`script.nss` becomes `script`, `.script` becomes empty, and an unextended
name keeps its text. A period in a directory component also ends the prefix.

After resolving the explicit object through `0x5d5840` at `0x658b91`, the
handler calls its primary virtual member `+0x18`. The nine physical kinds
listed in the CCObject cast trace return their CCObject interface. The
handler assigns its script at `+0x26c` and sets dirty byte `+0x288` to one
at `0x658bc4`. [CCObject cast trace](script-tostring.md)

Objects with a null CCObject interface take explicit native-kind branches.
Kind 4 obtains an area through virtual member `+0x08` and assigns its script
at `+0x38c`. Kind 3 obtains a module through `+0x1c` and assigns `+0x1f8`.
These native kind bytes have their own numbering. Party, stage, map and
map-pin objects reach the cleanup path without assignment. An unresolved
object also takes that path.

The replacement applies the traced prefix and recipient checks to Origins.
The regression covers all fifteen object kinds in both games, independent
caller and target objects, case, whitespace, multiple extensions, leading
and trailing periods, directory periods, Unicode names, empty assignment,
missing IDs and a save/reload of each assignment on template-free objects.

Native dirty-byte consumers and template-script restoration require further
lifecycle tracing. The current JSON representation encodes an empty script
as `None`, and the body-bag repair path can supply a template script during
load. The bounded assignment correction leaves that existing lifecycle
question open. DA2's setter requires its own executable trace.
