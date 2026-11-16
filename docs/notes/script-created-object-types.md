# Origins script-created object types

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

Origins script dispatch now restricts types to those two conversion ranges.
Successful calls retain template loading, placement, active-state and script
override handling. The general `Sim::create_from_template` API remains available
to other engine callers, including mod mode. DA2 retains its existing dispatch
until its own factory has been traced.

The regression uses synthetic creature/placeable GFF templates. It verifies
that unsupported types allocate no object or event, that canonical and
noncanonical supported types still load their templates, and that DA2 retains
its item, trigger, waypoint and unknown-type mappings.

This is a type-dispatch correction. Creature pools, native template-load failure,
invalid locations, trigger geometry and spawn-event timing remain separate work.
In particular, the replacement still creates an object for a missing template;
native factory cleanup at 0x686388..0x6863a3 instead returns null after deleting
the failed allocation. This change does not claim to fix that difference.

Static inspection used DAOrigins.exe SHA-256
`2346caee611002d4dff6ecef68eb315b63a37004dd98b39c11a9c26c0b45bfb3`,
at image base 0x400000. No original executable or VM was run. Native excerpts
and analysis drivers remain outside the repository; this note contains original
prose and trace anchors only.
