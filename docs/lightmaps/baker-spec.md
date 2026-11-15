# Lightmap baker spec

What we know about how Dragon Age: Origins' lightmaps were made, and how `thedas-bake`
reproduces it. Sources are the DA:O toolset 1.01's lightmapper (BioWare's `LightMapper.py`
driving `eclipseRay.exe`, a Python 2.5 host for a modified YafaRay 0.1 core, plus
`BakedMapProcessor.exe`, `CreateAtlas.exe` and the level editor plugin `EditorLvl.dll`),
traced statically, and fits against the shipped atlases. No toolset code or bytes are in this
repository; addresses below are virtual addresses in the named binary.

Marks: **[V]** read from the toolset's code; **[F]** found by fitting the shipped atlases;
**[I]** inferred or provisional.

## 0. The big caveat: the shipped atlases are not the 1.01 driver's output

[F] The public driver (`LightMapper.py` 1.0.3, dated 2 December 2009, a month after release)
cannot be what made the shipped atlases:

- its occlusion is five rays per sample, which leaves visible noise; the shipped atlases are
  smooth, and their occlusion is distance-weighted and clamped (§6);
- several shipped lights were baked with a smaller radius than the one they ship with, and
  meshes that cast no baked shadow (MMH 6177 = 0) did not occlude (§6).

Its shadow pass, though, is the shipped one: binary per light (`monochrome`, §2), once the
occlusion both channels share is divided out (§6).

The script's own history says "1.0.3: fixing sunlight direction and shadows". So the toolset's
code tells us the conventions (atlas mapping, the RGBA combination, light falloff, film), and
the shipped data tell us the settings BioWare's own bake used. Both are recorded here.

## 1. The pipeline [V]

For each level the editor writes job files (`EditorLvl.dll` job writers 0x10155220, light XML
0x1015a7d0, scene 0x1015ddf0, atlas job 0x101584b0); the driver renders three passes per
lightmapped mesh part, each to a 24-bit TGA:

| Pass | Job | Lights | Film gamma |
|---|---|---|---|
| Lightmap (RGB) | `job_lm%d.xml` | Baked and Ambient-Baked lights, Negative lights (colour negated) | 1.5 |
| Shadow (→ alpha) | `job_sm%d.xml` | Static and Animated lights, the enabled sun | 1 |
| Occlusion | `job_ao%d.xml` | none | 1.5 |

Then `BakedMapProcessor.exe` (0x407b60) combines the three per part, `CreateAtlas.exe` packs the
parts into room atlases (1024 × 1024 requested, gutter 0, `-halftexel`, inputs capped at
512 × 512), and `TextureProcessor.exe -dxt5 -nomipmap` compresses them.

### Which lights go where (`EditorLvl.dll` 0x1005d1b0, 0x101576f0, 0x1015a7d0)

- LightType: Point 0, Ambient 1, Spot 2. LightEffect: Baked 0, Dynamic 1, Static 2,
  Animated 3, Negative 4. Only lights with *Affects Level* set are baked.
- Baked → RGB job only. Static/Animated → shadow job only. Negative → both (the driver then
  zeroes it in the shadow job). Dynamic → nowhere. Ambient-Baked is the Ambient shape with the
  Baked effect → RGB job. The sun (when enabled) → shadow job only.
- Brightness = |ColorIntensity|; colour is the authored colour, unscaled (negated for Negative).
- In the shadow job the colour is replaced by (0.001, 0, 1) for Static/Animated/sun (constant at
  0x10330e4c) and (0, 1, 0) for Negative; brightness and radius are kept.
- Light Size and Num Samples are written (`LightSize`, `NumShadowSamples`) but the 1.01 driver
  ignores them.
- Sun: direction −D/|D| for the authored direction D; brightness = |SunlightColorMultiplier|.

Shipped room light models (`_ll`) hold the Static lights (every one has IsStatic 6009 = 1) and
each room's runtime ambient. Baked lights are not exported anywhere (OpenThedas-testdata
`notes/baked-lights-survey.md`), so the RGB pass's lights must be recovered by fitting (§6).

### Geometry and targets [V]

- A model is in the bake when its LightmapFlag is set (`EditorLvl` 0x1003d110; the model
  `prp_collision_wall_0` is excluded by name). Every direct mesh child of the model's root is
  scene geometry; the driver never checks MMH 6177 (cast baked shadow) or 6301 (receive).
- [F] But the shipped occlusion does not see meshes with 6177 = 0 (carpets): the shipped bake
  honoured the flag. `thedas-bake` keeps such meshes visible to camera rays and out of shadow
  and occlusion rays.
- Lightmap UVs: TEXCOORD1 when the chunk has it, else TEXCOORD0 (terrain); the driver flips v
  and the camera flips it back. No vertex normals reach the tracer: every face is flat-shaded
  with its winding normal (P1 − P0) × (P2 − P0).
- All geometry gets one white diffuse material (`shinydiffuse`, diffuse 1, no specular,
  Lambert): the bake knows nothing of textures, alpha cut-outs or tints.

## 2. One texel [V]

Images are rendered at the part's size (from the model metadata's LightmapSize, rounded to a
power of two with ties at 1.25×: `EditorLvl` 0x10160f40; see §5).

- **Samples.** One pass of eight primary samples per texel (`eclipseRay` scene defaults,
  0x405d60). Sample s of texel (x, y) sits at (x + (s + ½)/8, y + LP(s + O)), where LP is the
  Larcher–Pillichshammer radical inverse (yafraycore 0x1003d6f2) and
  O = H(y · H(x)), H being FNV-1a over the four little-endian bytes of a 32-bit integer
  (0x1003d50f).
- **Surface.** The camera (`LightmapCamera`, `eclipseRay` 0x40d040) maps the sample to the part's
  triangle containing uv = ((x + dx)/W, (y + dy)/H) through a strip table (0x40dd10, 0x40c8d0),
  interpolates the world position P, and shoots a ray from P + 0.05·N toward −N (N the face
  normal) at the whole scene. The nearest hit is the surface; its normal is faced toward the
  camera (`shinydiffuse` face-forward). No texel is forced to its own triangle.
- **Lightmap pass** (`directlight.dll` integrate 0x10001920, estimateDirect 0x10004150): the sum of
  - each Ambient light's colour, flat (no shadow, no cosine; the host's ambient light,
    `eclipseRay` 0x403d60, answers vtable slot +0x14);
  - each point light's colour × brightness × max(0, 1 − d/R) × |N·L| when L is on the
    surface's side and the shadow ray is clear (`pointlight.dll` 0x10001250: **linear falloff,
    no inverse square**);
  - each spot light likewise, times a smoothstep cone factor (`spotlight.dll` 0x100018a0);
  - each directional light's colour × brightness × |N·L| when clear (`directional.dll`
    0x10001590).
- **Shadow rays**: from P, bias b = 0.0005 (`directlight.dll` 0x10007320), hits counted between
  b and d − b (`scene_t::isShadowed`, yafraycore 0x10022e90). Triangles are hit from both sides.
- **Shadow pass**: the same integrator with `monochrome` set: any positive channel → (1, 1, 1)
  per sample.
- **Occlusion pass**: per sample five rays, cosine-distributed around the faced normal, with
  s1 = vdC(q + k), s2 = Halton₃(q + k), q = 5s + O; direction √s1·N + √(1 − s1)(cos 2πs2·U +
  sin 2πs2·V) with the tangent U = (Ny, −Nx, 0)/|…| (or (±1, 0, 0) for a vertical normal) and
  V = N × U (yafraycore 0x10011280). A ray escapes when nothing lies within 5 (less the
  biases). The pass value is the escaped fraction.
- **Film** (yafraycore `imageFilm_t`): every sample is splatted with a Gaussian of radius 1
  (16 × 16 table of max(0, e^(−6(a² + b²)) − e^(−6))) onto its own texel and, when its offset
  exceeds ½, the texel to its right and/or below (`addSample` 0x1002a0d0, rounding
  ⌊z + 0.4999999999⌋). The texel's value is the weighted mean, clamped at 0, raised to
  1/gamma, then **truncated** to 8 bits (`outTga_t::putPixel` 0x10032540). Texels no sample
  reaches are black.

## 3. Combining and atlasing [V]

- `BakedMapProcessor` (0x407b60): RGB = ⌊lm · ao / 255⌋ and A = ⌊sm · ao / 255⌋ on the 8-bit
  values. If every combined texel is within ±2 of the first, the part is box-filtered down to
  4 × 4.
- `CreateAtlas`: parts sorted by width, greedy rectangle packing, no rotation; pages shrink to
  used powers of two. With `-halftexel` the RML offset and scale (field 3324, copied unchanged
  by `EditorLvl` 0x100d7120 → 0x1008acd0) are o = (x + ½)/A and s = w² / (A (w + 1)) for a
  w-texel part at texel x of an A-texel page. `thedas-bake` inverts this to find each part's
  rectangle and size (`bake::placement`).
- The atlas is compressed to DXT5, so any comparison has a few levels of block noise.

## 4. Runtime use (for reference)

Static1 uses the texel raw: diffuse = Σ(static lights, sun)·lm.a + lm.rgb·lerp(1, S, k) + …
(OpenThedas-testdata `notes/frame-gamma-lightmap-trace.md`). So lm.rgb is the baked light
(gamma 1.5 encoded) times occlusion, and lm.a the static lights' and sun's visibility times
occlusion.

## 5. Sizes [V]

Part size: metadata LightmapSize L divided by the downsample factor (truncated), clamped to the
maximum (2048 by default), then each side rounded to a power of two: a when 3(n − a) ≤ b − n
for a < n ≤ b = 2a, else b; at least 4, at most 2048 (`EditorLvl` 0x1015ea40, 0x10160f40).
Terrain chunks: ChunkExportLightmapTextureSize (default 64, clamped to 512 except lak100d/n).
`thedas-bake` needs none of this for comparison: the shipped RML gives each part's size.

## 6. What the shipped bake did, by fitting [F]

Measured on Castle Cousland's hall (`hrt201d`, atlas `hrt201d_room1400`, every second texel).
`--fit` gathers per texel the surface (as the bake finds it), each static light's
falloff × |cos| × visibility, and occlusion ray hit distances, then fits.

- **RGB lights.** The linear RGB (texel^1.5) of unoccluded texels fits
  ambient + Σ k·colour·max(0, 1 − d/R)·|cos|·visibility over the room's static lights with
  k = 0.85–1.08 for the four lamps beside it: the RGB pass had lights at the static lights'
  positions, colours and radii, brightness 1, linear falloff, gamma 1.5. (The editor puts
  Static lights only in the shadow job, so these are Baked twins of the static lamps.) The
  fireplace fits k ≈ 0.6. The room's baked ambient fits ≈ (0.149, 0.146, 0.199), not the
  runtime ambient (0.225, 0.225, 0.2475). What is left is mostly a blue fill: Baked-only
  lights the game files no longer contain.
- **Alpha *is* binary after all; the curve below is occlusion in disguise.** RGB and alpha
  carry the same occlusion factor, so alpha / green = sm / lm_g, and lm_g follows from the
  fitted RGB model: an occlusion-free estimate of the shadow pass. Over texels one static
  light alone reaches (Cousland room1100/1400/400), that estimate is ≈ 1 (0.9–1.1) at every
  light strength for most lights (lamp_007, lamp_011, fireplaces, bounce_005): the shipped
  shadow pass is the toolset's monochrome one. Some lights (lamp_009, lamp_010, bounce_008)
  step from 0 to 1 well inside their runtime radius: their bake radius was smaller (about
  two thirds) than the exported one.
- **Monochrome with fitted bake radii is now the recipe** (`--shipped`). `--fit` finds each
  shadow-casting light's bake radius as the share of its exported radius that best splits lit
  from dark alpha on the texels it alone reaches (`radius i s` in fit.txt): Cousland's lamps
  0.80–0.90, fireplaces 0.90–0.95; `sun_fake_static` and some window fills are not in the
  shadow pass at all. It also fits the occlusion model on the texels the monochrome pass fully
  lights, where alpha is occlusion alone; the best is still (1 − t/1.5)² weighting,
  clamp(−1 + 2v), unscaled. Against the √min(1, 3S) stand-in, both levels improve on mean
  alpha error: Cousland 13.6 → 10.3 levels (texels within 4 levels 35.4 → 37.3%; RMSE 37.6 →
  39.2), Lake Calenhad 8.3 → 6.4 (70.1 → 73.6%; RMSE 33.2 → 32.7). RGB is unchanged.
- **Former reading (superseded; `--shadow-intensity 3,0.5`): alpha rises with light strength.** Over the static lights, with S = Σ max(0, 1 − d/R)·|cos|·
  visibility (no colour), shipped alpha ÷ occlusion ≈ √min(1, 3S): 0.47 at S = 0.075, 0.73
  at 0.175, 0.96 at 0.325, saturated beyond 0.35 (`--shadow-intensity 3,0.5`). Other falloff
  shapes (quadratic, cos², smoothstep) fit no better. [I] A shadow job without `monochrome`
  whose blue channel (colour (0.001, 0, 1) × brightness) became alpha would behave like this;
  the curve itself is fitted, not traced.
- **Occlusion** is distance-weighted and clamped like the editor's AO settings (MinColour
  −1.5, Exponent 2, MaxRayLength 5, which the 1.01 driver ignores): the best fit so far is
  64 cosine rays out to 1.5, a hit at t occluding (1 − t/1.5)², pass value
  clamp(−1 + 2v, 0, 1) for unoccluded share v, stored as is (`--ao-model 1.5,2,-1,1`). So
  crevices go to exactly 0 in RGB and alpha alike. Meshes with MMH 6177 = 0 (carpets) do not
  occlude.
- **Missing lights.** `--fit --discover N` searches the residual RGB of unoccluded texels for
  point lights (a 2-unit grid over the texels' bounds up to 4 above, radii 3–16, non-negative
  colour per channel, greedy). On room1400 its first four finds are blue fills at radius 6 about
  2 above the floor (colour ≈ (0.05, 0.1, 0.2)), cutting the residual by 20%; `--use-fit` adds
  them to the RGB pass.
- **Shadow-pass membership.** Not every `_ll` light is in the alpha: `--fit` tests each light
  on the texels it alone reaches and leaves it out when they are dark in the shipped alpha
  more than ten to one (`noshadow` in fit.txt). In Cousland that removes `sun_fake_static`
  (5 lit / 3554 dark) and some `window_bounce_*_RT` fills.
- **Coverage.** Shipped parts are covered one texel beyond what the toolset's film reaches
  on every side (`--dilate 1`: an uncovered texel next to covered ones takes their mean).
- **Shadows are hard.** Sampling point lights as spheres (`--light-size`) makes the fit worse
  at every size tried (0.25–1).
- Whole level, each atlas fitted then baked (Cousland, 88 atlases, 19.2M texels): mean error
  RGB 9.4 / 8.2 / 8.8, alpha 10.3 levels with `--shipped` (RMSE 17 / 15 / 16 and 39); 37% of
  texels within 4 levels on every channel. Alpha is the weakest part (window and fake-sun
  fills).
- **Exteriors** (Lake Calenhad `lak100d`, 71 terrain/LOD atlases, 26.3M texels): mean error RGB
  4.0 / 4.1 / 3.8, alpha 6.4 levels with `--shipped`; 74% of texels within 4 levels on every
  channel. The sun
  is in the shadow pass only (its fitted RGB brightness is ≈ 0, as the editor routing says);
  the baked ambient is a dim green, about (0.06, 0.09, 0.06); water surfaces are not part of
  the bake scene (terrain under the lake is sunlit in the shipped alpha).
- With all of the above and the fitted lights, room1400's 876k texels differ by a mean of
  7.4–9.5 levels in RGB and 6.9 in alpha (RMSE 14–17 and 26.5); 47% are within 4 levels on
  every channel. First light was ~14 and ~13.

## 7. Light sets (for the deferred renderer)

`thedas-bake <install> <level> --lightset DIR` merges the per-atlas fits in `DIR/fit-*/fit.txt`
(from `--fit --discover N --out DIR/fit-<atlas>`) into `DIR/<level>.lightset.json`. Because
it is derived from game data, it lives outside the repository, in OpenThedas-testdata
`lightmap/lightsets/`. Fields (linear colours):

- `rooms.<room>.baked_ambient`: the RGB pass's flat ambient for that room's pieces (mean over
  atlases); `runtime_ambient`: the room light model's ambient, when it has one.
- `lights[]`: every level light (the bake's order: non-character points, then the sun) with
  `name`, `kind`, `position` (for the sun, the direction toward it), `radius`, runtime
  `color`; `baked_brightness`: mean fitted RGB-pass brightness where it was seen
  (`null` = not seen); `in_alpha`: whether the shadow pass took it; `shadow_reach`: the share
  of its radius the shadow pass reached (the bake radius; 1 when not fitted).
- `found[]`: baked-only lights recovered from the RGB residual: `position`, `radius`,
  `color` (colour × brightness), and the `atlas` whose fit found them. Fits of neighbouring
  atlases can find the same light twice.

A light's baked contribution is colour × brightness × max(0, 1 − d/radius) × |N·L| (no inverse
square), shadowed, on top of the room's baked ambient; the lightmap stores it with gamma 1.5
and multiplied by occlusion.

## 8. `thedas-bake`

`cargo run -p thedas-bake --release -- <install> <level> [--atlas NAME] [--out DIR]` bakes every
lightmapped part and writes per atlas a PNG (ours | shipped | |difference| × 4, RGB above
alpha) and `report.txt` with per-channel MAE/RMSE/bias. `--fit` instead fits light
brightnesses and room ambients and reports how the shadow and occlusion passes explain
alpha; `--probe X,Y` explains one atlas texel (surface, each light's term, what blocks the
occlusion rays). `--shipped --use-fit fit.txt` is the closest reproduction (the fitted
occlusion, one-texel dilation and the toolset's monochrome shadow pass, plus a fit's lights,
bake radii and ambients). The parts: `--ao-model L,E,LO,P[,HI]`, `--shadow-intensity K,E`,
`--dilate N`, `--use-fit fit.txt` (a `--fit` run's light brightnesses and room ambients),
`--light-size R,N`; `--ao-converged N` and `--ao-length L` vary the toolset's own occlusion.
