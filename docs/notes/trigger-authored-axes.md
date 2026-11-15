# Trigger outlines keep their authored axes (SIM-009)

Static DAOrigins.exe trace, 2026-11-15. The executable image is the same pinned image
identified in the external `path-follower-trace.md`; only original prose is recorded here.

## Loader and membership test

The CCTrigger loader **0x7993c0** reads the GFF `Geometry` list at
**0x79964a–0x799676**. For each entry it reads `XPoint`, `YPoint`, `ZPoint`, then adds
the trigger's position directly to that vector (**0x79976f–0x7997e8**). There is no
facing rotation in that conversion. The final virtual call at **0x799808–0x79980f**
is slot +0x64; on the ordinary trigger vtable **0xb1fef4**, that slot is the empty
**0x5bdcf0** function. It does not rotate the loaded vertices afterward.

The polygon membership method **0x799130** is slot +0x180 on that vtable and the
five derived trigger vtables. It tests indexed edges in XY and returns the odd/even
intersection result (**0x799160–0x799345**). It never reads the point's Z component,
the vertices' Z components, or the trigger's facing. This supports the existing XY
height behavior; adding a vertical extent would introduce a new discrepancy.

The creature placement path **0x6b2f70** calls the area membership collector
**0x73efe0** (**0x6b3004–0x6b3012**). That collector tests objects through their
+0x180 membership method (**0x73f075–0x73f086**). Thus this polygon predicate is
used by the movement/placement membership path, not merely by an editor query.

## Correction and limits

OpenThedas stores the authored relative vertices. Its Origins trigger query now
subtracts the trigger position from the tested point and applies the XY polygon test
directly. It previously also rotated the point by negative trigger facing, effectively
rotating the authored outline in world space.

For example, a trigger at (10,20) with vertices spanning local x=1..3 and y=−0.5..0.5
contains (12,20) regardless of its facing. At facing 90 degrees, the previous query
instead accepted (10,22). ENTER/EXIT collection and the current `IsInTrigger` query
use the corrected coordinates. DA2 keeps its previous rotation because this is an
Origins-only trace. No save fields change.

The native `IsInTrigger` handler **0x66da90** actually queries the creature's cached
membership list at +0xba8 (**0x66dadb–0x66daea**); OpenThedas still performs a
geometric query there. This change does not claim to recover that cache's timing.
Edge-boundary comparisons, movement sampling frequency (SIM-008), and event
creator/payload rules also remain separate work. The regression tests use interior
points away from boundaries, verify that height alone causes no EXIT, and preserve
the existing DA2 query behavior.
