# Dust Town sun-shadow stability

The executable stabilizes the fitted shadow projection in whole light-space metres.
It does not snap the centre to the current map's texel size. Origins' partition finalizer
at 0x453240 first interpolates the previous fitted bounds towards this frame's bounds
by 0.35 (0x453266; constant at 0xbf2088). With casters present, 0x453309–0x4533b9 passes
the X/Y maxima through ceil (0x8e51d0) and minima through floor (0x8e6110), before
constructing the orthographic projection at 0x4524d0. The two math helpers were followed
through their positive/negative and fractional branches to distinguish ceil/floor from
round-to-nearest or truncation.

Depth is passed through without that rounding. The box copied to the previous-frame
slot at 0x4533fa is the smoothed, **unrounded** box. Rounding that history instead would
make small camera movements stall at integer bounds. The existing 0.35 smoothing is
already correct; the missing outward rounding let the projected shadow texels slide
over fixed geometry every frame.

Faithful and deferred consume the same `D3dPass` sun partitions and texture matrices.
The level's sun is fixed when the scene is loaded. Faithful point-light picks are stored
per draw and are repicked for moving objects, based on their world bounds and rooms,
not the camera. Deferred's additional point-shadow pool is separate from these sun
partitions. A static-scene camera walk therefore isolates the shared sun fitting from
object motion and light-selection churn.

Other provisional items in `shadow.rs` (caster reach towards the sun, partition overlap,
the light-elevation clamp and one-frame-late object bounds) are independent of this
correction. This trace concerns Origins; Dragon Age II's own shadow setup is untraced.

## Reproduction

Build `cargo build -j 4 -p thedas-app --example shadow_walk`, using the job's build-slot
wrapper and target directory. The example loads an area through the normal resource
stack and shader translator, then uses the real renderer's offscreen captures. It opens
no window. Optional `--save` reads the area's creatures/placeables through the normal
model resolver, holds their rest poses, and runs no simulation ticks during the walk.
For example:

```sh
shadow_walk --install "$THEDAS_INSTALL" --out captures/faithful \
  --area bdc200ar_slums --camera 389,240,10,99,-14 \
  --step 0.01,0,0 --frames 120 --renderer faithful
```

Add `--save slums.json` for the slums save's frozen creatures/placeables. Repeat on the
base and branch binaries. Add `--unshadowed` to suppress runtime casters as a
lighting/motion control (sun casters in Faithful; sun and point casters in Deferred).
Use `--renderer deferred` for the other renderer;
`THEDAS_DEFERRED_VIEW=sun-shadows` isolates its sampled sun visibility. All sequences
freeze shader time at three seconds, disable frame effects and antialiasing, and warm
the same camera for 30 frames before walking. Each capture records its exact camera in
`cameras.csv`. These choices remove postprocess and animation noise from the comparison.
`shadow-data.csv` records the actual GPU-bound texture rows and partition size without
advancing the shadow fit. `shadow_walk_metrics.py` reports the displacement of 27 fixed
world anchors in shadow texels, plus image deltas. With the two unshadowed control folders
supplied, it measures temporal deltas of the signed control-minus-frame RGB residual
(the runtime shadow contribution), including the percentage of pixels changing by more than
8/255 in any channel. That image metric still includes the shadow's ordinary screen
motion during the walk; the matrix metric isolates the moving shadow grid itself.

For a separate particle-motion check, use `--time-step 0.016666666666666666`. This
advances the particles/shader clock at 60 Hz after the warmup; `emitter-lights.csv`
records the derived light centres, radii and colours. The ordinary zero-time-step run
isolates cascade fitting from animated particle-light motion.

## Measured scope

The frozen-save replay uses `dc_01_slums.json`, 47 objects in rest poses, camera
`387.7,226,11,90,-24`, and 40 frames moving 0.01 m along world X per frame, at 640×360.
This is a scripted camera walk; it does not reproduce the player's animation, camera
collision/fading, or a cutscene. The original game was not run.

On that replay the shadow-grid motion over 27 fixed world anchors changes as follows:

| Metric | Before | After |
| --- | ---: | ---: |
| Stationary partition updates (<0.001 texel) | 0% | 97.436% |
| Mean displacement (texels/frame) | 0.22548 | 0.33223 |
| 95th percentile displacement | 0.35126 | 0 |
| Maximum displacement | 0.35203 | 29.21772 |

The larger maximum is the original's coarse whole-metre bound transition, not a promise
of continuously fixed texels. The replay's visible Faithful shadow-residual temporal
delta is essentially unchanged: mean absolute RGB delta 0.257145 → 0.257200 on a 0–255
scale; mean pixels changing by >8/255 in any channel 0.283431% → 0.283509%. Ordinary
screen motion is included in that residual metric. The matching pixel comparisons
must therefore be described as essentially zero, rather than evidence that Sera's
reported large jumps are resolved. This corrects a traced fitting discrepancy; the
exact reported playtest route/save/active renderer still needs confirmation.

Deferred's corresponding residual also stays essentially unchanged: mean absolute RGB
delta 0.602285 → 0.602306; mean pixels changing by >8/255 1.823640% → 1.823751%.
Across the 40 paired scene frames, Deferred changes no pixels by >2/255 (maximum 1/255).
Faithful's mean paired changed-pixel percentage at that threshold is 0.0000217%
(maximum channel difference 6/255). Independent unshadowed control captures differ by
at most 5/255, so the tiny scene deltas do not establish an improvement above capture
variation.

Deferred endpoint probes at the start and end of that frozen walk each report 91 lights
in view and exactly the same ordered 16 point-shadow lights (positions and radii). This
does not show a selection discontinuity at those endpoints; it is not an all-frames or
advancing-particle-time guarantee.

The advancing-particle replay (same 40-frame walk, 1/60 second per frame) moves the
nearby torch emitter 51's derived light centre by a mean 0.007567 m and maximum
0.018393 m per frame, spanning 0.076483 m vertically. The CSV intentionally logs all
emitters for diagnosis; the actual renderer selects only additive glowing materials.
The CSV alone therefore cannot establish pool membership. An actual per-frame
selected-light trace would be needed to establish animated pool
churn. Particle-light motion is a useful follow-up if the reported session used
Deferred. No speculative point-light policy change is included here.
