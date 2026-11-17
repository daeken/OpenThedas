# Very High defaults and optional soft sun shadows

Sera requested this deliberate visual deviation on 2026-11-17: use Very High
by default and improve the shadows even where that departs from the original.

New launcher settings, direct CLI launches, and `Detail::default()` now select
`high`, our existing name for **Very High / original index 3** shadow settings:
two 1024² maps, 10/30 m reaches, and the traced 25–30 m fade. The launcher's
label is “Very High.” The original Options movie's `GraphicsDetailLevel`
default remains **3**, with a test tying that value to the renderer default.
This is an intentional product default, not a claim about the original game's
untraced hardware detection. Existing material technique selection is unchanged.

Explicit saved launcher detail values are retained, including **medium**. An
old saved `medium` is indistinguishable from a player's deliberate Medium
choice, so there is no migration that promotes it. Missing fields get the new
defaults through the existing serde default pattern. Nothing reads or rewrites
the player's settings as a migration. The in-game movie's other existing
video-option limitations remain tracked under RND-110; this job does not add
live material/scene rebuilding when its Graphics Detail slider moves.

## The choice

The launcher offers **Sun shadows (OpenThedas)** beside Renderer and Deferred
lighting, following the existing pattern for engine-specific choices:

- **Soft (improved)** — default ON; saved as `play.soft_shadows: true` and
  passed as `--soft-shadows on`.
- **Faithful** — `play.soft_shadows: false`, `--soft-shadows off`; retains
  the traced hard switch and original near/far filters.

An older launcher file missing `soft_shadows` opts into the improvement, but
an explicit false round-trips unchanged. Renderer selection is independent:
the original-material and deferred renderers both honor this shadow choice.
“Original” describes material rendering, not a guarantee that every separate
OpenThedas improvement is disabled.

## Sampling and trace boundaries

The improved receiver uses sixteen red-depth comparisons per selected map,
weighted as a separable three-texel tent PCF with subtexel interpolation.
The stencil is unrolled, with a single interior/border decision; fully faded
receivers return lit before filtering. The tap weights and fade are unchanged
by these sampling-cost optimizations.
Out-of-map taps explicitly return lit, independent of border-sampler support.
It replaces the original's asymmetric green-coverage near estimator and
four-comparison far estimator with the same depth filter on both maps.
This includes intentionally avoiding the near green channel's traced saturation
behavior; it is not a correction to the original shader's math.

With `d = R1 - camera_forward_depth`, near is used alone at d >= 2 m, far
alone at d <= 0, and both are blended by `smoothstep(0, 2, d)` between them.
At Very High this is the **8–10 m** camera-depth band. The near projection is
never sampled beyond its original slice. The far filter retains exactly
`sat(4*(R2-z)/(R2-R1))`, fading to lit over 25–30 m at Very High and
16.75–20 m at Medium. The grazing-normal floor and material lighting following
the selection are retained.

The existing caster passes, projections, map sizes, blur passes, reach constants,
and allocation count are unchanged. The original G16R16 maps are currently
represented by the engine's existing RGBA16Float textures; this work does not
claim to fix that separate storage difference. Far-map coverage inside the
blend band remains limited by the existing projection/caster fit. White borders
handle missing coverage; this is not a new overlap-caster system.

The D3D translator remains faithful by default. The renderer opts into an
additional runtime branch only for receivers recognized by the named nine-vector
`g_vShadowData`, two named partition samplers, and the split/selection data flow.
There are no shader hashes, fixed instruction offsets, or copied shader programs.
The matcher follows the plane's data flow across independently scheduled fade
math, including terrain and tree shaders that calculate the far estimator first;
it stops at clobbered components or control-flow boundaries.
Unrecognized receivers (including DA2's single-partition shaders) are untouched.
Instanced constants and alpha-to-coverage variants use the same option. The
deferred sun receiver shares the WGSL filter, including its forward-lit overlays.

The option is carried in the spare per-draw fixup W; the constant cache includes
the choice so toggling it cannot leave stale slots on static draws. A read-only
`check_sun_shaders` example checks an external shader archive across backends.
`THEDAS_BENCH_SHADOW_COMPARE=soft` uses the existing capture benchmark's ABBA
pairs without changing the scene, camera, shader clock, maps or render path.

## Verification and captures

Checked on merged main `d6b15225`: fmt and workspace/all-targets clippy with
`-D warnings` pass; workspace tests: **1485 passed, 0 failed, 26 ignored**.
The synthetic GPU fixture checks the blend, exact tent edge weights, white border
and traced fade. Translation tests cover uniform/instanced constants, scheduled
estimators and rejected clobbers. Instanced shaders retain their existing backend
limitations (storage buffers exceed the harness's OpenGL 3.3/ES 3.0 targets).
The ordinary fragment path passes every backend translation. The read-only
archive check passes **532 shaders**, recognizing **all 12 named dual receivers**:
Prop1/punch, Static1/alpha/fast/punch/relief, Terrain1/2/3, branches and fronds.
Metal executes the GPU tests and the captured native material pipelines.

Each view compares old Medium/Faithful, Very High/Faithful and Very High/Soft,
with an absolute RGB **Delta ×8** for each pair. Dust Town uses the #1046 save
and camera `387.7,230.4,12.5,270,-30`; Korcari Wilds uses `dc_20_wilds` and
`939.787,1026.7,10,267.5,-30`. The shader clock is zero, FOV 60°, 1280×720,
MSAA off and frame effects on. The Wilds difference is very small in this shaded
view; its existing foliage/terrain-texture and HUD limitations remain visible.
The staged PNGs/captions are for coordinator review, without publication.

Pixels changed by more than 2/255 in any RGB channel (full-resolution frames):

| View | Medium/Faithful → Very High/Faithful | Very High/Faithful → Soft |
| --- | ---: | ---: |
| Dust Town | 10.249% | 0.635% |
| Korcari Wilds | 0.116% | 0.045% |

The Wilds soft delta is nearly zero at this shaded camera. All six final frames
are pixel-identical to the earlier looped-filter frames: the optimization changes
cost without changing these images.

Apple M4 / Metal, debug build, **Soft minus Faithful at Very High**: 120 ABBA
frames per view, first 16 discarded, leaving 26 four-frame groups. GPU times
sum the timestamped level/object forward receiver passes; the shadow maps and
caster/blur passes are identical in both modes.

| View | Faithful GPU mean | Soft GPU mean | Extra GPU / frame | Paired median extra |
| --- | ---: | ---: | ---: | ---: |
| Dust Town | 4.04 ms | 4.62 ms | **+0.58 ms** | +0.21 ms |
| Korcari Wilds | 18.14 ms | 19.50 ms | **+1.36 ms** | +1.25 ms |

These are observations on the shared host. Headless capture wall means, including
CPU preparation and readback, were 149.20/162.22 ms (Faithful/Soft) in Dust Town
and 379.75/390.22 ms in Wilds; those timings fluctuate heavily with host load.
The initial looped stencil cost +4.26/+10.08 ms of GPU time; unrolling preserved
the filter while reducing the measured cost.

Captures and checks use scratch HOME and hidden/headless rendering. No original
game, player settings, screen, Windows VM or media publisher is used. No simulation
gate was run: this change only affects rendering and player configuration.
