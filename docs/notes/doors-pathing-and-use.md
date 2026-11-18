# Origins doors, scenery collision and use approach

Static comparison with the GOG 1.05a DAOrigins.exe, SHA-256
`2346caee611002d4dff6ecef68eb315b63a37004dd98b39c11a9c26c0b45bfb3`.
Addresses are virtual addresses. The original game was not run.

The controller path is independent of path planning. `0x6b6880` moves the
creature's controller through `0x838240` with mask `0x2011134`; this includes
Placeables and StaticGeometry. NonWalkable alone is not its movement filter.
The controller subsequently supplies the creature's position (`0x6b2220`).
The downward controller displacement also keeps a walking body on the floor.
The capsule descriptor's step offset is 0.5 m; this does not justify raising
the grid planner's existing 0.6 m limit. The retained hideout stair samples
have ordinary 0.25–0.375 m increments.

Door state changes restore the original PHY group when the state controller
marks the state blocking; otherwise they use group 1 (`0x74e850`, `0x74bcc0`,
`0x74d350`). PWK clearing and installation are separate operations. The
locked footprint's kind-8 cells are obstacles even though they are not
ordinary walkable cells. They must remain associated with the door when
locating the path action that opens or unlocks it. Searching a virtual open
doorway must never clear another object's kind-8 cells.

The simulation previously located a blocking door by inspecting only retained
waypoints. A straightened leg can cross the entire leaf between its ends.
Door identification now samples every leg and retains the continuous prefix
before the first closed doorway. Command movement walks that prefix before
reporting the path action; the tools' stepped walk uses the same identification.
Planning and locomotion share the existing rule allowing an actor already in
a doorway to leave on its own side. A leaf ray also constrains movement from
such a coarse cell, so releasing its centre cannot let a stale route cross
the closed shape.

Scenery physics was absent from locomotion: movement slid against other
creatures, but not placed boxes or level geometry. The simulation now derives
cached obstacle cells from the existing PHY collision. Both planning and
movement use them. Placed objects load with the controller mask separately
from sight collision; the layout currently reuses its sight shapes together
with the pathing grid. This remains a conservative grid approximation: horizontal
body sections at 0.5 and 1 m, with at least half-cell coverage, are not a full
PhysX capsule sweep or an exact implementation of its sliding response.
Area-transition volumes remain triggers. Closed doors remain path actions.
Cache invalidation includes the grid, placement, active flags and runtime
physics group; changing areas or restoring a save drops object-specific caches.

Attack execution has its own obstruction test. `0x7a8909` calls `0x67c750`,
which queries geometry using mask `0x24`; failure returns -1 at `0x7a8965`.
Being within melee range therefore does not authorize a blow through a box.
The helper uses vfunc +0x1b0 (`0x6a1a30(1)` for creatures), which resolves
the animated `head` attachment; `0x67c610` adds each height to its endpoint.
The simulation provisionally substitutes authored HEIGHT/HEIGHT_F for these
attachments, for creature targets. The fixed 1.8 m perception ray would pass
above the hideout dwarves' head-high crates. Placeable
bashing retains its existing path until target-shape exclusion is modelled.

Use is a compound action. The script builder `0x66f770` makes command 31;
its final use executor is `0x7a78f0`, registered at `0x7aba42`. The slot
movement executor `0x7aa560` asks the target's virtual slot resolver at
`+0x98`; CCPlaceable's resolver `0x74f240` calls `0x74dfb0`, which transforms
the current PWK rectangle. It is not a creature appearance radius. The final
USE event is emitted at `0x7a7af8` after that movement stage.

The simulation incorrectly indexed apr_base with a placeable appearance.
For example, Proving door appearance 82 collided with a creature-table row
whose personal space is 3 m. Player and script use commands now approach
the current footprint. A missing PWK falls back to the rest PHY bounds,
then the object origin. The actor's capsule plus the existing 0.6 m gap is
still a provisional use tolerance; the full native slot resolver is not
claimed as recovered.

Follower recovery is explicit in `0x760dc0`. After the ordinary start and
eligibility tests, `0x76101e` asks whether the leader perceives the follower.
The unseen branch at `0x7610d7` recomputes the compact formation slot,
clears actions (`0x73c930`) and places the follower (`0x73d080`). Origins
now follows that branch for eligible followers; busy, follow-disabled and
fighting followers retain their orders. Formation placement and the exact
perception model retain the approximations documented elsewhere.
Follow command ownership and the next update now belong to the saved world:
restoring an earlier world must not retain a later world's timer. This also
makes an active follow move and its update phase survive save/reload.
Completion, failure, clearing and explicit replacement release follow ownership;
a new manual or script move must not inherit eligibility for the old move's warp.

Arrival must preserve the locomotion result. A repaired route can end away
from the original request, and collision can extend a move's deadline while
the update advances movers. Completion now respects that extension and
keeps the body's actual route end, including its grounded height after an
approach on stairs. A hero displacement over 2 m in one tick
is recorded on the simulation trace and application log, with game time,
elapsed time, areas and the front command.

Collision retries must preserve route corners too. The Tower gate exposed
body contact discarding a covered waypoint around a wall, then retrying an
unwalkable shortcut forever. Origins now sweeps bodies along the individual
route legs, retains required corners, and rebuilds a blocked remainder when
body sliding has pushed the actor behind a previously passed corner. Grid
boundary ordering distinguishes a segment through a free side cell from an
actual diagonal corner crossing, independently of update length. A compound
approach repaired to the actor's own feet fails rather than polling every
millisecond while its target stays out of reach. These remain grid stand-ins
for native path progress and continuous controller collision.

For the reported spontaneous hideout jump, the retained #1109 journal starts
at game time 3340325 ms and replays to 3489297 ms on the initial main baseline
without a displacement over 2 m per input. It cannot establish the cause of
an earlier jump. The session text log has no position history; the report
checkpoint replaced that earlier interval. The arrival repairs are tested
independently and are not presented as proof of that historical event.

Door presentation reads `State`, chooses the state-controller transition in
`PlaceableSwings`, and passes its elapsed simulation time into `Live::clip`.
The diagonal state pose handles first sighting and save loads. PR #891 is
still open: its simulation-owned playback record is absent from this base.
The existing state/transition tests verify that bridge. An audible transition
with correct state and clip selection still needs rendering investigation;
this change does not duplicate the animation-clock work in job
`0061-enemy-anim-rate`.

## Reproduction and validation

The baseline is main `d6b15225`. Installed-save probes use the retained
Proving, Commons and Beraht hideout saves. The probe isolates unrelated actors
and scripts, while restoring the actual target door script for USE. No game
files or personal saves are included here. A stock Commons door already
blocked the baseline route through its PWK; the sampled Proving and hideout
doors did not. Their PHY shapes include NonWalkable as well as Placeables,
so the missed route-leg intersection explains those reproductions; the
controller mask correction is independently covered by a synthetic test.

`sim-work/repro/doors.sh` reports A–G separately. D tests the existing pose
bridge; E tests independent route-arrival hazards. Neither means that its
intermittent historical incident has been reproduced. Optional external
save and journal inputs are documented at the top of that script. The
`doors_probe` example also accepts `--use-fixture`, which exports a scratch
save with a real Proving door and a walkable starting point 3.5 m away for
headless app testing. In the app, the baseline USE opened that door remotely;
the candidate approached first and visibly opened it by three seconds.
The Proving controller also has `Open_2` (state 4), not just `Open` (state 2).

Retained journal replay measured stair grounding as follows:

| Journal | Main maximum floor error | Candidate maximum floor error | Candidate ticks over 0.08 m |
| --- | ---: | ---: | ---: |
| #1108, 240 ticks | 0.391 m | 0 m | 0 |
| #1109, 5,451 ticks | 0.555 m | 0 m | 0 |

Neither journal contains a hero jump over 2 m per input on either build.
The renderer already projects some walking poses onto the floor; the sampled
stairs image is consequently almost unchanged despite the simulation fix.

The default five-scenario gate completes in the same areas, with no save
refusals. The first three scenarios are identical. In Lothering, the arrival
conversation shifts from s15 into s14, moving its three plot writes between
steps. The remaining final sparse-storage difference is an explicit false
`ORPHAN_SHOUTS`; reading all four affected flags through MCP with
`call_script=false` confirms equal effective values. This is a timing change
requiring sim review, not a claim of identical logs. Four 600-tick determinism
scenarios agree across repeated runs and save/reload.

The additional Tower gate reaches Flemeth's hut on both builds and lights
the beacon. Final simulation times are 1979312 ms (main) and 1994577 ms
(candidate); the harness uses 23 and 17 cheat heals respectively, with no
kill aids. Five effective flags differ: injury and salve tutorials,
`ALISTAIR_EVENT_ON`, the Aegis codex entry, and `PRE_BEACON_REACHED`.
These are consistent with changed combat/loot and follower-trigger timing;
they require sim review rather than an identical-gate claim. The installed
`pre440tr_beacon_reached` script specifically sets flag 11 only for Alistair
entering outside combat. It is distinct from beacon USE's flag 2,
`PRE_BEACON_LIT`, which is true on both final saves. This separates the
follower proximity difference from completing the beacon objective.
