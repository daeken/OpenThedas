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

## Validation and route continuation

The regression covers DA2 selection, invalid tags, Origins isolation, preserved
secondary map, snapshot round-trip, pin-index travel and close/arrival persistence.
The local installed-resource replay starts at the existing Darktown exit at
t=1303942. Invoking the movie's own night-button method changes the binding to
type 7 and `MAP_NIGHT_1280X1024`; the active Hightown night pin has full-list index
3. Its normal travel event reaches `dae120ar_hightown_nt` at t=1304942. Selection
leaves object data, pin statuses, plots and RNG identical; GUI message dispatch
also drains already-due events, advancing handle bookkeeping without world time.

Walking to Anders and accepting entry at the Chantry reaches `and121ar_chantry`
at t=1314315. Karl's dialogue sets Tranquility flag 7 at t=1316415. The authored
plot script finds all five members of team 104000 and changes their groups, but
they remain inactive, so the fight does not begin. The continuation stops there
at t=1326515 with no script errors. It does not claim quest completion or force
the team's activation. Plot area-state actions and cinematic activation remain
separate work.

On main 623396b9 plus this patch: fmt and workspace all-target clippy with warnings
denied pass; debug workspace tests pass (1,171 passed, 16 ignored). The DAO s01-s13
gate has 26 successful runs, identical story logs and plot sequences, and 15
identical JSON checkpoints. Four DAO 600-tick repeat/reload checks and a DA2
600-tick repeat/reload check are identical.
