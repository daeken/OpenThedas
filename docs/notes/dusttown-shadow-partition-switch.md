# Dust Town's sharp sun-shadow boundary after #991

The original `Static1` pixel shader makes a **hard switch** between two different
shadow estimators. It does not blend the partitions at their common boundary.
For a static caster whose blurred green channel saturates, the original math can
return fully lit immediately before the split and fully shadowed immediately after
it. At ordinary Medium detail that split is 7 m of **camera-forward depth**, not
radial distance from the camera or distance along the ground.

Consequently this investigation preserves the renderer, as requested when the
traced original math also permits the edge. This establishes a shader-level
discontinuity, **not** visual parity with the original at Sera's Dust Town camera.
The supplied original trace is from Commons at High detail, and has a different
caster population. An original Dust Town shadow-target/membership trace would be
needed to establish the magnitude of its edge at that exact scene and camera.

## Evidence and scope

Read-only sources outside the repository:

- `OpenThedas-testdata/reference/original/triage/commons_wall_trace/`:
  `static_pixelshader_9d79_draws.json`, `draws_named.json.gz`, and captured shaders.
  All 149 selected static draws use the same nine shadow vectors and camera.
- The captured pixel shader `9d79dad21fcfe4fa` is byte-for-byte identical to
  `static1.psh.fxo` in the repro install's `shaders.erf`. The captured shadow blur
  `675adc0eebba724c` likewise matches `twopassblurshadows.psh.fxo`. These are MD5
  prefixes identifying the supplied artifacts, not a new executable build ID.
- `DAODecomp/local/disassembly-llvm.txt` and read-only constants from
  `gamedata/bin_ship/DAOrigins.exe`, SHA-256
  `2346caee611002d4dff6ecef68eb315b63a37004dd98b39c11a9c26c0b45bfb3`.
  Addresses below are image VAs, with image base `0x400000`.
- Source comparison is against `1f7f013f`, after fetching and merging
  `origin/main` (already up to date). No original game, Windows VM, or screen was
  used. No executable bytes or disassembly are included here.

Shader token offsets below count 32-bit words from the version token. They make
the original expression reproducible without copying its instruction stream.
Formulas and descriptions are original explanatory prose. **Provisional** marks
claims that the supplied trace does not establish.

## Decoding `g_vShadowData`

Let `P = (world_x, world_y, world_z, 1)`, `C` be the camera position, `F` its
unit forward vector, and `R1`, `R2` the two slice reaches.

| Registers | Meaning in the captured shader | CPU producer / comparison |
| --- | --- | --- |
| c12, c13, c14 | Near-map U, V, depth numerators: dot each row with P | First texture matrix copied at `0x452d93–0x452da3`; `shadow_data` supplies these rows |
| c15 | Near-map homogeneous denominator | Captured `(0,0,0,1)`; ours also affine |
| c16, c17, c18 | Far-map U, V, depth numerators | Second texture matrix copied at `0x452ddb–0x452df1`; same layout in ours |
| c19.xyz | Far-map denominator's XYZ; shader supplies **1** for its W | Zero XYZ in the trace; original c19.w is deliberately excluded from this division |
| c19.w | `R1 - R2`, used for the far fade | Written at `0x452df3–0x452dff`; captured **−20**, matching High's 10/30; ours supplies the same difference |
| c20 | Signed split plane `(-F, R1 + dot(F,C))` | First slot's plane copied at `0x452da5–0x452de2`; ours supplies the same sign and convention |

The plane is constructed from the first camera slice's far plane in
`0x451db0`, especially `0x452073–0x452278`, and normalized by `0x4d44a0`.
Its signed value at the captured camera is **9.999999705 m**; its normal's
length is approximately one. Thus this High-detail capture switches at 10 m.
Taking the reciprocal lengths of the matrices' first three row normals decodes
the fitted light-space spans: approximately **17 × 11 × 10.980667 m** for near,
and **45 × 32 × 34.268126 m** for far. These are fitted projection extents, not
the camera-forward reach distances. They cannot be compared numerically with
the Dust Town repro's projections because the camera, area and caster sets differ.
The original quality setter `0x455731–0x455792` reads constants giving:

| Detail | R1 | R2 | c19.w | Far fade starts | Far fade ends |
| --- | ---: | ---: | ---: | ---: | ---: |
| Low | 5 | 15 | −10 | 12.5 | 15 |
| Medium | 7 | 20 | −13 | 16.75 | 20 |
| High | 10 | 30 | −20 | 25 | 30 |

These ordinary reaches match `shadow::partition_reach`. Slice 2 starts at slice
1's end (`0x4537d1–0x4537df`), also matching ours. The separate camera-state
adjustment below changes the reaches in some states; it does not create an
overlap or a blend band.
The fade columns evaluate this captured Static1 formula at those reaches;
whether Low selects that receiver for a particular material was not traced here.

**Conditional reach adjustment, `0x453740–0x45383b`:** the camera's virtual method
at +0x98 resolves to `0x4656a0`, reading `ICameraInternal +0x138`. This is the
same scalar subsequently exported as `g_vCameraPosition.w`: `0x45983b–0x45984b`
copies it to render-params +0x13c, retained by `0x465f50` and the CameraPosition
usage handler. For finite w above the threshold at `0xae7128`
(approximately 0.400000006), the ordinary reaches become:

```text
shift = (20 - ordinary_R1) * w
R1 = ordinary_R1 + shift
R2 = ordinary_R2 + shift
```

The far base is the square root of the quality setter's stored square
(`0x453761–0x45376c`; helper `0x8e6240` reaches the square-root instruction at
`0x8e627c`). The adjustment uses 20 from `0xbf20d0`, not an extrapolated FOV
or radial-distance rule. The subsequent far-reach bounds do not change these
ordinary finite examples. At Medium, w = 0.5 would give 13.5/26.5 m, with the
same −13 in c19.w; the fade would move to 23.25–26.5 m. The original still
switches sharply at the adjusted R1.

Our fixed reach table and always-zero CameraPosition.w omit this behavior for
nonzero camera states. The scalar starts at zero (`0x465e71`); the game-camera
update explicitly writes zero in modes 4 and 5 (`0x5d772b–0x5d776a`), or writes
`1 - game_camera[+0x24]` on its other kind-2-camera path. The meaning and
activation of that kind-2 state were not traced here. The captured Commons w
is zero, so its 10 m split is established directly. The exact original Dust
Town camera scalar remains **provisional**, rather than assuming that every
original camera state uses the ordinary reach table.

## Decoding the static receiver

Write `sat(x) = min(1,max(0,x))`, `z = dot(F,world-C)`, and `d = R1-z`.
Receiver depths are saturated to [0,1] before comparison. The shader uses these
fixed UV offsets (independent of partition resolution):

| Offset | U | V |
| --- | ---: | ---: |
| A | −0.00141302 | −0.000598593 |
| B | +0.00141838 | −0.00115336 |
| C | −0.000141276 | −0.00139408 |
| D | +0.000517439 | +0.000440816 |

**Far estimator, words 874–983:** sample red at the centre and A, B, C; compare
each red against the saturated receiver depth, with equality lit and no added
receiver-depth bias. `far_raw` is the average of those **four** binary results.
It is neither a single comparison nor hardware comparison filtering. Then:

```text
fade = sat((c19.w - d) / (0.25 * c19.w))
     = sat(4 * (R2-z) / (R2-R1))
far = 1 + fade * (far_raw-1)
```

This fades **to lit** over the last quarter of the second slice's length. For
Medium it is 16.75–20 m, not 15–20 m, and has no influence near the 7 m split.

**Near estimator, words 988–1078:** sample the centre's red and green, plus green
at A, B, C, D. If the centre red is greater than or equal to receiver depth, return
1; otherwise return `sat(0.2 * sum(five_greens))`. There is **no variance,
mean-square subtraction, Chebyshev bound, or light-bleeding reduction**. Calling
this main-pass receiver “VSM” describes neither the captured bytecode nor our
translation. The depth-squared caster channel does not imply a VSM receiver.

**Partition choice, words 857–865 and 1083:** first compute `sat(10000*d)`, then
select far when its negation is greater than or equal to zero. For finite input:

```text
visibility = near if d > 0 else far
```

The multiply by 10000 does **not** create a 0.1 mm interpolation band: its result
is tested by a conditional selection, not used as a lerp weight. At d = 0 the far
partition wins. Both estimators are evaluated before this selection.

**After selection, words 1088–1142:** the shader takes the maximum of visibility
and `1-sat(10*dot(normal,-sun_direction))`. It scales the selected local/sun
diffuse and specular contributions by that value. Its baked-lightmap multiplier
is `1 + (0.15 + 0.35*sat(camera.w))*(visibility_after_normal_floor-1)`. The trace's
camera.w is zero, so the baked term alone dims by at most 15%; that is not a
crossfade between partitions. These operations also run unchanged in Faithful.

`thedas-shader` translates the original shader. Its masked writes, saturation,
dot products, texture operations, and `Cmp` (`>= 0`, including negative zero)
preserve the decisions above. No alternate handwritten Faithful receiver was
found. There is no receiver-selection, ordinary switch-distance, compare-count,
green-average, or far-fade discrepancy to fix in that path.

## Decoding the caster and blur

`PSSMStatic.psh` writes red = normalized depth and green = depth squared.
`PSSM.psh` writes red = depth and green = zero. This difference is preserved by
our material-selected caster programs.

The captured `TwoPassBlurShadows` shader, words 55–109, preserves centre red.
Its loop starts with counter 1 and increments **before** sampling, so the taps
are the centre and **±2, ±3, ±4** steps, not ±1, ±2, ±3. Green becomes:

```text
out_green = sat((sum(seven_greens))^2 / 7)
step = 0.35 / partition_size
```

The captured horizontal and vertical shader constants confirm width 0.35 and
size 1024 at High. The intermediate target is also **G16R16** in this trace:
the previous comment calling its format provisional is superseded for Origins
by this capture. Our hand-written `ShadowBlur` uses the same taps, step,
operation order (positive then negative), squaring, normalization, saturation,
two-pass order, and preservation of centre red. The executable applies the
chain to partition 1 only at Medium/High (`0x4518e0`); ours matches.

For a spatially uniform static caster with normalized depth t, the two passes
give `g1 = sat(7*t^4)` and `g2 = sat(7*g1^2)`. Thus g2 saturates at
`t >= 7^(-3/8)`, approximately **0.482045724**. For t = 0.6:

```text
caster green = 0.36
horizontal green = 0.9072
vertical green = 1
near visibility behind this caster = 1
far visibility behind the corresponding far-map caster = 0
```

At the split the far fade is 1. With a sufficiently sun-facing surface, the
post-selection normal floor does not remove the discontinuity. The original
math therefore **also permits a hard shadow edge there**. The example is a
derived synthetic case, not a sample of an original Dust Town texture.

There is an important trace limit: all **108** caster draws in the supplied
Commons frame use `PSSM.psh`, green zero; **none** use `PSSMStatic.psh`. The rigid
vertex-shader draws (6 near, 12 far) are not proof of static-level casters; props
also use that vertex shader. Skinned draws number 31 near and 59 far. Therefore
this capture cannot establish the blurred green values of Dust Town's level
geometry or its membership in either partition. Explaining Sera's precise edge
amplitude by the synthetic saturation case remains **provisional**.

## Differences and remaining limits

The receiver math above matches. These surrounding differences were found or
confirmed; none justifies replacing the traced hard switch with a new blend:

| Area | Original evidence | Current renderer / limit |
| --- | --- | --- |
| Target storage | Both maps and blur intermediate are `D3DFMT_G16R16` (16-bit UNORM channels) in the captured frame | `Rgba16Float`, including the intermediate. Different precision, range, and channel count; **not** the original storage format. The uniform saturation example survives both formats; exact texture/pixel parity is unestablished |
| Texture-matrix bias | `0x452409–0x452477` builds the D3D9 bias with half a texel added | `shadow::bias` omits that term, while caster VS translation adds a half-pixel raster shift. Exact combined shadow raster/sample alignment is **provisional**; a blanket “wgpu fixes it” claim is not established |
| First camera-slice near | Original default 0.01, read from `0xbf20a4` at `0x4537b9` | Normal camera near 0.05 feeds the slice fitter; influences fitted projection, not the split-plane reach |
| Camera-state reach adjustment | When camera.w > approximately 0.4, add `(20-ordinary_R1)*camera.w` to both reaches before slice setup | Fixed per-detail reaches; matches captured w = 0, differs in adjusted states; original Dust Town state **provisional** |
| CameraPosition.w | Camera +0x138 drives both that adjustment and the lightmap's shadow multiplier; captured w = 0 | Always zero, so positive original camera states' shadow strength is also omitted |
| Light rotation | `0x452ac2` applies an elevation clamp using 0.6 | `light_view` uses normalized input without this adjustment; fitted matrices may differ. Exact effect at this camera **provisional** |
| Empty bounds | `0x453303–0x4533c3` uses identity projection when the caster-bound count is zero | Fits a fallback extruded slice |
| Minimum extent | `0x452824–0x452900` adjusts X/Y minima/maxima; its depth operations do not establish a minimum near/far separation | Raises maxima to enforce at least one metre on all three axes |
| Candidate bounds | Original per-partition Umbra selection, aggregate-owner or individual-node spheres, and separate fit/draw filters | Approximate sphere overlap over retained draws; unsupported/sampled caster materials removed before fitting |
| Object timing | Original queries collect current bounds before fitting | Level fit consumes other passes' previous-frame spheres |
| Runtime sampler state | Supplied per-draw JSON records sampler names and textures, but not the D3D9 sampler-state values | Faithful maps the material state to wgpu sampling, including border fallback when unsupported. Exact runtime filter/address/border parity is **provisional** from these artifacts |
| Temporal projection | 0.35 smoothing, outward whole-metre X/Y rounding, unrounded history | Matches the prior trace; #991's near limit of three times the slice span also matches |
| Original scene population | Commons High frame, no static-caster shader draws | Dust Town Medium repro; same receiver bytecode does not prove same map contents |

The fitting/selection differences are described in more detail by the external
`notes/shadow-box-depth-trace.md` and the earlier
[stability investigation](dusttown-shadow-stability.md). This job does not
silently treat those approximations as traced parity, or expand into a general
shadow-system rewrite. Dragon Age II is outside this Origins trace.

## Verification

A read-only interpreter executed the captured shader's shadow section against
synthetic near/far textures, compared with the formulas above: **42 cases
passed**, covering 7/20 and 10/30 reaches, both sides of the switch, the fade
start/end, beyond the far reach, five distinct green values, saturated greens,
and the centre-depth lit override. This checks decoding, not GPU or scene parity.
External results: `codex-captures/dusttown-shadow-line/math-results.json`.

Two independent hidden/headless captures used the same debug binary built at
`1f7f013f`, the requested `at_leske.json` save, Medium detail, Faithful,
`--camera 387.7,230.4,12.5,270,-30`, `--no-audio`, and the supplied wind/home
HOME fixture. Default output was 1280×720 with 60° vertical FOV, shader time 0,
and frame effects on. Both logs confirm that exact camera, 1096 level draws,
47 saved objects, and the `Static1` shader pair. Neither run advanced simulation
through scripted steps. The external `repro.sh` runs each through the shared
build-slot wrapper.

The images are **pixel-identical**: maximum RGB channel delta **0/255**, mean
absolute RGB delta **0**, and **0%** of pixels changed at thresholds >0/255,
>2/255, and >8/255 in any RGB channel. `comparison.png` labels the first capture,
the unchanged-renderer repeat, and absolute **Delta ×8**. The delta is exactly
zero. All images and metrics are under
`OpenThedas-testdata/codex-captures/dusttown-shadow-line/`; the comparison is also
staged for orchestrator review under `codex-captures/media-staging/`.

This is a repeatability control, **not an after-fix image**. No renderer was
changed, and no disappearance of Sera's line is claimed. The requested-camera
capture alone does not identify the reported moving edge pixel by pixel or
measure its two shadow textures. The matched shader predicts a hard step at
the split whenever the estimators disagree; confirming this precise Dust Town
scene's original caster population and edge amplitude remains **provisional**.

After capture, `origin/main` was fetched and merged again to `ad14ee0c`.
Neither `thedas-render` nor `thedas-shader` changed between that merge and the
captured binary's revision. Repository checks run on that merged source.
No simulation gate was run because this PR changes documentation only.

- `cargo fmt --all` and `cargo fmt --all -- --check`: passed.
- `cargo clippy -j 2 --workspace --all-targets -- -D warnings`: passed.
- `cargo test -j 2 --workspace`: **1350 passed, 0 failed, 22 ignored**, across
  31 test-result summaries including doc tests. The existing ignored
  integration/corpus tests were not enabled. Test threads were capped at two
  and `THEDAS_HIDDEN_WINDOW=1` was set.
- `git diff --cached --check`: passed. The PR contains only this note; no
  renderer, simulation, shader bytecode, game data, or executable-derived
  instruction dumps are included.
