# DA2 world-map tabs

The shipped DA2 world-map movie's city-night button calls
`WorldMap.SetPrimaryMapByID(7)`. It obtains the tag from M2DA 171, column
0xaffee486, plays the authored GUI sound and sends
`WorldMap.WorldMapSetPrimaryMapByTag` with that tag. City day is row 4 and the
Free Marches is row 1. These are authored map resources, not a swap between the
script's primary and secondary maps.

Read-only analysis of the user's DA2 image found the message registration at
0x4c2125/0x4c215f, message factory 0x435b40, type accessor 0x41eb00 and screen
subscription 0x6ae100. Handler 0x6ac6a0 looks up the first string through the
module's tag registry (0x5b9160, occurrence zero). A failed lookup leaves the
screen unchanged. A successful lookup calls 0x6a66e0: the map object is resolved
through 0x5a6f40 and the screen's primary binding is updated and refreshed.
The script `SetWorldMapPrimary` wrapper at 0x4ebab0 calls the same
0x6a66e0 setter. Selection updates the primary map; it does not change the
secondary map, plot flags or start travel.

The simulator resolves DA2 map tags to map objects and replaces the primary map.
UI bindings and travel use `shown_maps()`, and MCP inspection reads the updated
primary map. A subsequent pin index refers to the selected map's full pin list,
including hidden pins. The
module's ordinary travel event still controls the transition. Selection persists
through closing the screen and ordinary world snapshot serialization until a
script or another tab selects a different map. Origins does not handle this
DA2-only message and keeps its existing temporary swap behavior.

This fixes selection and pin routing. It does not fix the movie's existing
placeholder/template rendering, localization or full world-map presentation.
