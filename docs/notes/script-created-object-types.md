# Script-created object types (SCR-019)

## Origins

`CreateObject` is script function 306. In the GOG Origins executable, registration
at 0x675e58..0x675e64 selects handler 0x669550. The handler converts its integer
type bit to an internal kind using logarithm, division by the logarithm of 2,
truncation and addition of one (0x6696a3..0x6696c7). Ordinary positive inputs
32..63 select creature kind 6; 512..1023 select placeable kind 10. The canonical
script constants are 32 and 512. Treating every noncanonical value as an unknown
enum would discard part of this conversion.

These ranges describe the ordinary SSE2 conversion path through
0x8e34f9..0x8e350b. Boundary rounding in the historical x87 fallback has not
been verified.

The creation request reaches 0x687830 at 0x669705. It first checks a creature
pool indexed by template. Without a matching pool, 0x68793b calls factory
0x6861f0. That factory has two branches: kind 6 constructs a creature through
0x6b9ed0, and kind 10 constructs a placeable through 0x750ce0. Other kinds return
null at 0x6863a6. This path cannot construct an item, trigger or waypoint, and it
does not substitute a placeable for an unknown kind. The script handler returns
an invalid object after a failed creation.

## Dragon Age II

The shipped action table also declares `CreateObject` as 306. Registration at
`0x52671f` selects wrapper `0x511a00`, which calls `0x503b30`. That handler
loads the signed type as an integer, computes `ln(type) / ln(2)` and applies
its truncating conversion (`0xb2a680`, ordinary SSE2 at `0xb2a695`) before
adding one. The same positive ranges select creature (32..63) and placeable
(512..1023). The historical x87 integer conversion is not verified.

The request enters `0x59f9b0`. It looks up the template in the area's pool
before inspecting the kind: a matching pool can return an existing creature
without going through the factory. A miss calls `0x59ecf0`, which accepts only
kind 6 (constructor `0x5955e0`, `CCCreature`) and 10 (`0x5ef640`,
`CCPlaceable`). All other kinds return null at `0x59ee6a`. The outer handler
leaves its failure result at `-1` after a null return.

## Replacement scope

Both games now use the traced non-pooled dispatch. The simulation does not
model pool lookup or reuse, and rejection outside these ranges describes its
unpooled path only. The general `Sim::create_from_template` API and other
callers are unchanged, as are successful calls' placement, active state,
script override and event handling. Synthetic creature and placeable GFFs
verify that canonical and noncanonical types load the right template and that
unsupported types allocate no object, ID or event.

This is not a template/load, location or spawn-event fix. For example, the
replacement still creates an object for a missing template; native factory
failure cleanup (Origins `0x686388..0x6863a3`, DA2 `0x59ee4b..0x59ee53`)
deletes a failed allocation and returns null. Invalid locations, DA2's
appearance-copy argument, trigger geometry and spawn-event timing are not
changed.

Static inspection used DAOrigins.exe SHA-256
`2346caee611002d4dff6ecef68eb315b63a37004dd98b39c11a9c26c0b45bfb3`,
and the DA2 analysis image at base `0x400000`. No original executable or VM
was run. Native excerpts
and analysis drivers remain outside the repository; this note contains original
prose and trace anchors only.
