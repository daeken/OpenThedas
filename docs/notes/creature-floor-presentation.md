# Creature floor presentation (reports 1034, 984, 985, 1105)

Static analysis of DAOrigins.exe; addresses are virtual addresses. Only prose is
included here. The ground-height helper and the moving character controller are
different queries and must not be described as one symmetric floor window.

## Ground-height helper

`0x685430` initially returns the supplied Z when there is no scene or no accepted
hit. `0x73d520` is its area wrapper. It builds a vertical ray at the supplied X/Y,
from **Z + 3.0**, direction **(0, 0, -1)** (`0x6854e7–0x685513`;
double `0xaf0c38 = 3.0`, float `0xae2e00 = -1.0`).

The first query uses scene slot `+0x1c = 0x82e620`: a closest ray hit, **not a
capsule or sweep**. Its maximum ray distance is **FLT_MAX**, loaded from
`0xaf0d18` at `0x82e6ed`. There is no nearby lower-Z cutoff. The mask is
`0x24` (Placeables | StaticGeometry) when `0x67b4d0` returns true, otherwise
`0x02000000` (Walkable). The predicate reads area `+0x658`, then tests its
`+0x7c == 1`; the semantic name of that mode is **provisional**.
The ray's activeGroups argument is all bits (`-1`); the masks above are the
additional low-word groups-mask filter installed by the wrapper.

In the `0x24` branch, the closest hit is rejected when its returned mask includes
`0x01000000` (NonWalkable). If the first query fails or is rejected, a second ray
starts at **Z + 100.0** (`0xaed110`, `0x6855b5–0x6855cd`), again downward,
using **0x24**. Scene slot `+0x20 = 0x82e830` collects all hits, again with
FLT_MAX distance (`0x82e913–0x82e93d`). `0x685644–0x6856b1` discards
NonWalkable hits and picks the smallest **absolute difference from the original
Z**, not necessarily the highest hit. Initial best distance is 65505.0
(`0xb07ecc`). Neither the 3 m lift nor the 100 m fallback is the CCT step height.

`0x73e370` calls the wrapper when its snap argument is set; candidate locations
also reach the helper. `0x73e7f0` first tries a directional offset of half a
pathing cell and calls the wrapper (`0x73e84c–0x73e89b`), then searches further
candidates. These are placement/path-validity searches, not capsule moves.

## Moving character controller

`0x695540` installs position and invokes the creature position-change hook
`0x6b5ef0`. The hook only synchronizes an existing controller and skips the
`+0x420` suspension and `+0x774` recursion flags. It can call `0x6b1790`,
which uses `0x73e370` or `0x73e7f0`, installs the corrected placement, temporarily
shrinks the controller and sets its center above the feet.
The former receives a 10 m horizontal search radius (`0xb0f298`), the latter
1 m; neither number bounds the helper's downward ray.

The capsule is created at `0x6b1ed0–0x6b2206`, through factory `0x821b40`.
Creation installs **Z up**, **0.1 m skin**, **0.5 m step offset**
(`0xb0f28c`), a slope limit of approximately cos(60 degrees), and constrained
capsule climbing. Radius/height depend on the creature's appearance and personal
space; this is not a fixed-radius ground sphere.
PhysX API/descriptor names follow the recovered wrapper vtables and the PhysX 2.x
ABI. The numeric values and calls are verified; the descriptor-field naming and
interpretation of climbing-mode 1 as "constrained" are **provisional ABI mappings**.

The mover `0x6bb890` checks active pathing (`0xa630e0` at `0x6bb911`) and
asks for a proposal before calling `0x6b6880`. That function replaces proposed
vertical displacement with **-9.81 m per call**, irrespective of dt
(`0x6b6a3b–0x6b6a47`, `0xb0f274`), then calls wrapper `0x838240` and
NxController::move. This is capsule movement/contact resolution, not the helper's
point ray. The 9.81 is the actual downward displacement, not a guessed gravity
acceleration multiplied by frame time. An UP collision restores the old center,
retries with step offset zero, then restores **0.5 m**. `0x6b2220` reads the
controller position back, subtracting the center-to-feet offset. Its only direct
caller in the dump is `0x6b6b10`, after the move.

The move passes **0x02011134** as both active groups and the low groups-mask
word (`0x6b6a4b–0x6b6a8e`). Its bits are:

| Bit | Meaning |
| --- | --- |
| 0x4 | Placeables |
| 0x10 | Creature controllers (shape group 4) |
| 0x20 | StaticGeometry |
| 0x100 | Water |
| 0x1000 | Unnamed here; **provisional** |
| 0x10000 | TerrainWall |
| 0x2000000 | Walkable |

It excludes ordinary creature hit/ragdoll shapes (0x2). The capsule-growth overlap
check instead uses `0x11024`; placement check `0x6b1600` uses `0x02011134`.
The wrapper installs AND filters on the scene and forwards the move to PhysX.
These readings agree with the earlier local `creature-collision-trace.md`.

## Why a chair does not justify a short ground window

**Placeables are included**, both by the helper's 0x24 branch/fallback and by the
controller's movement mask. There is no chair/seat category exclusion in these
queries. A seat blocks only when its authored physics shape has a tested mask;
opaque furniture render triangles are not proof of that shape or mask.

`SetPhysicsController(false)` (839) reaches `0x66c01d–0x66c09e`: it writes
the inverse enable value to creature variable key `{0, 0xb}`, destroys the
controller through `0x69edc0` and disables controller interaction. Enabling
recreates it if absent and re-enables interaction. `0x6b1f50–0x6b1f6e`
checks the same variable before creation. Its connection to the script-visible
`PHYSICS_DISABLED` local is **provisional**, so the presentation code does not
use that local as a substitute for native controller state.

There is also no unconditional per-render-frame floor query in ordinary
`CommandPlayAnimation`: command 14 (`0x744870`) schedules action 37
(`0x7a4390`), which starts playback through `0x6aa7e0` and configures animation
motion through `0x7a3370`. The path follower's movement and the ground helper are
not called by that playback path. A stationary sitter therefore does not acquire
a new chair-top root every frame just because furniture is in the collision mask.
The inspected core ambient bytecode queues `CommandPlayAnimation`; it does not
call `SetPhysicsController`. **No universal native "seated" flag or automatic
disable-controller-on-sit rule was established.**

## Presentation implementation and limits

For Origins, `floor_under` uses the **0.5 m step-up allowance and 9.81 m downward
reach of movement**, not the teleport helper's much wider placement search.
The saved simulation position is never written. Stationary scripted animations
keep their authored root; moving commands still ground. Idle standing creatures
also ground because their saved grid Z needs the last physical contact restored
on presentation. Cutscene placement and held-cutscene animations retain their
existing ownership of the root. Dragon Age II retains its previous 0.25 m query.

**Provisional approximation:** this is a vertical point ray through the existing
opaque-render-triangle collider, not a PhysX capsule sweep, group-filtered PHY
query, slope/skin implementation or persistent controller contact. The endpoints
come from verified controller constants, but applying them to a point ray is not
an exact emulation of NxController::move. Preserving stationary scripted-animation
roots is a presentation proxy for the separate native playback/movement paths;
full controller lifetime and animation-driven movement remain unimplemented here.
In particular, this change does not claim to implement the ground helper's
Walkable-first query or 100 m nearest-height fallback.

## Validation

The baseline was built from main `c609d2b7`; the candidate uses the same main
snapshot plus this presentation change. These are replacement-engine observations,
not measurements from running the original game. Captures ran sequentially, hidden,
with `THEDAS_MODS=none`, the supplied saves and reported cameras, `--size 1600x900`,
`--time 0`, and no simulation advance. Each comparison has before, after and
saturating absolute RGB **Delta ×4** panels. A changed pixel has a maximum RGB
channel difference **greater than 2/255**; full-frame counts include all pixels.

| Save | Changed pixels / 1,440,000 | Full-frame change | Result |
| --- | --- | --- | --- |
| 1034 | 96,616 | 6.70944% | Drawn feet rise from sim Z 13.323474 to mesh Z 13.682879 (+0.359405 m); full boots meet the upper step. The selection ring follows the drawn root too. |
| 984 | 2,519 | 0.17493% | Oskias stays seated. His body region (530,365)–(710,555) is pixel-identical, maximum RGB delta 0/255. Differences are on a background creature. |
| 985 | 1,578 | 0.10958% | Hero drawn Z remains 0.5 m. Soles region (740,595)–(840,685) has maximum RGB delta 1/255 and zero pixels over the threshold; the visible full-frame changes are on background creatures. |

The supplied 984 snapshot is in dialogue: the trace identifies Oskias as a
cutscene-owned `pose_17` actor, so that check verifies the existing cutscene bypass.
A supplementary copy clears only the top-level dialogue session and restores
`game_mode` from `mode_before_talk`, leaving Oskias's ordinary animation command
and position intact. Both binaries capture that same copy at the same camera.
Oskias remains seated with the new scripted-root guard active, retaining authored
Z **0.0**; he is not sent through the floor query. The full-frame delta is
**16,538 pixels (1.14847%)**, reflecting the authored-root correction to Oskias
and small background-creature differences. This is a derived probe, not an
unaltered report-984 capture.

Formatting and workspace clippy with warnings denied pass. Debug workspace tests:
**1,379 passed, 0 failed, 22 ignored**, including three new synthetic floor/root
ownership tests. The candidate application build passes. No simulation files or
positions are changed, so the simulation-semantics sweep gate is not applicable.
Capture commands, images and numerical deltas are retained under the testdata
job directory `codex-captures/stairs-feet-floor-window/`; labelled triptychs are
staged for coordinator review, outside the repository and media queue.

## Proving platform ownership (report 1105)

The two visible creatures in the supplied report are the hero (118) and Leske
(313). The platform is a static level draw in `orz500d`, room `orz500d_3046`,
draw 1128, model `dni_livgroom_stair01_0`, mesh chunk
`MshMain_DNI_LivgRoom_stair01_0`. Its `Default` material has render class
`Opaque,Static1`, and its triangles enter `LiveLevel::collider`.

| Creature | Saved X, Y | Sim Z | Report drawn feet-root Z | Platform top / floor query Z | Required rise |
| --- | --- | --- | --- | --- | --- |
| Hero | -8.832403, 56.420140 | -1.689837 | -1.689837 | -1.500004 | 0.189833 m |
| Leske | -7.402170, 52.634762 | -1.664672 | -1.664672 | -1.500004 | 0.164668 m |

Both rises fit the traced 0.5 m step allowance. At those points the underlying
room floor is at -2.017186 and -2.018708 respectively, on model
`dni_livgroom_flor01_0`, chunk `MshMain_DNI_LivgRoom_Flor01_0`, draws 1076
and 1075. `floor_under` selects the raised platform from the static opaque
render-triangle collider. The ARL walkmesh cells (76, 295) and (79, 287) also
store -1.5 m, with walkability value 1. These grid observations describe the
simulation input independently of the presentation ray.

The report's presentation snapshot records retained `bdc140_everd.dlb` animation
layers on both creatures after the conversation has closed. The hero retains
`mh.dg_tn_90_l__0` at 1.150035 seconds, and Leske retains `mh.po_tw_a8__0`
at 3.841973 seconds. `Live::sync` gives a retained cutscene layer ownership of
the authored root, skipping ordinary floor presentation. Thus the report has
both drawn feet-root transforms at their saved Z despite the reachable surface.
The skeleton's animated `root` bone is higher, at -0.908879 for the hero and
-0.871521 for Leske. The feet-root values in the table describe the object
transform used by the model and selection ring.

`CutsceneActors::switch` formerly returned immediately for an unchanged source
key. A conversation can change from a staged line to an unstaged line while
remaining open, retaining its previous layers with source `None`. Closing that
conversation keeps source `None` and used to skip cleanup. Cleanup now runs
on every closed-conversation update, including unchanged source keys. Open
conversation layers retain their lifetime and presentation clock. Releasing
ownership lets the existing floor query place both feet roots on the platform.

The paired reproduction loads the unmodified `save_now.json` and restores the
two transient layers from `presentation.json`, through the staged-to-unstaged
source transition. Its baseline object transforms and animated root-bone
positions reproduce the report's recorded values. This restoration is needed
because retained presentation layers are absent from a simulation save. The
fresh-load comparison independently exercises the simulation save alone.
Captures use a fixed camera and shader time, 1600×900 output, and zero simulation
advance. Temporary probes and local captures remain in the testdata job folder.

The regression checks cover the staged-to-unstaged-to-closed lifecycle for both
creatures, repeated open-line clocks and retained layers, and a synthetic
platform over a lower room floor at the two saved creature heights. Existing
step/drop bounds and scripted-root tests remain in place.

Report 1034 already reaches the Commons stair on the baseline containing PR
1048, with sim Z 13.323474 and drawn feet-root Z 13.682879. Its fresh-load
comparison checks that stair contact alongside the Proving fix. Report 984
checks Oskias's active seated cutscene pose.

With cleanup applied, the hero's drawn feet-root Z is -1.5000038 and Leske's
is -1.5000037. Their animated skeleton root-bone Z values are -0.7073853 and
-0.7071489, respectively. The restored-state before/after comparison changes
122,855 of 1,440,000 pixels (8.53160%) at a maximum-channel RGB threshold
greater than 2/255. The fresh Proving save and Commons stair comparisons are
pixel-identical. Oskias keeps his seated pose and feet-root Z 0.0118341, with
maximum RGB delta 2/255 and zero pixels above the threshold. All comparisons
include a saturating absolute RGB Delta ×4 panel.

Validation on merged main `d6b15225` passes workspace formatting, all-target
workspace clippy with warnings denied, and debug workspace tests with **1,481
passed, 26 ignored, 0 failed**. The new lifecycle regression reproduces the
baseline failure and passes after cleanup. Three added tests cover that
lifecycle, open-line retention and clocks, and the synthetic platform. The
simulation-semantics sweep gate is inapplicable to these presentation-only edits.
