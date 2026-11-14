# Nearest-object type masks (SCR-022)

Read-only static trace of the Origins executable, image base 0x400000.

The shared script handler at 0x671510 covers GetNearestObject (433),
GetNearestObjectByTag (435), GetNearestObjectByGroup (436),
GetNearestObjectByHostility (127), and GetNearestObjectToLocation (434).
The object-based variants pass the requested type mask unchanged to the
area search at 0x6849c0, which applies the predicate at 0x67fec0 to each
candidate, including the source when requested.

At 0x67feec the predicate derives a single type bit from the candidate's
native object-kind byte. At 0x67fefa it intersects that bit with the supplied
mask, rejecting the candidate when the result is zero. There is no
zero-as-all special case in either the handler or that predicate. Thus a
zero mask matches no objects; the all-bits mask matches every eligible kind.
The location-based query already uses a bit intersection in our engine.

Remove the zero-as-all exception from the Origins object-based queries.
Preserve DA2's existing behavior pending its own trace. Regressions cover
the four object-based APIs with zero, creature, placeable, combined,
all-bits and nonmatching masks in both games.

This fixes only the zero-mask part of SCR-022. The native area search walks
an internal spatial list and inserts candidates by distance; our ID-based
tie order is still provisional. Other native eligibility predicates and
tag comparison are outside this change.
