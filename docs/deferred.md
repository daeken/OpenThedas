# The deferred renderer (optional, ours)

OpenThedas draws the original's way by default: every material through its own D3D9 shaders,
translated (`thedas-render/src/d3d.rs`), with the original's lighting limits (three static
point lights and two others per object, baked lightmaps, two sun shadow partitions). That
path is the reference and stays untouched by this work.

The deferred renderer is a second way to draw the same scene data, chosen at run time. It
aims past the original: per-pixel lighting from every light in reach, lights that move and
cast shadows (spell effects included), and in time real-time lighting in place of the baked
lightmaps. It is not parity work; where the two differ, the faithful path is right.

## Switching

- `thedas_render::RenderPath` (`Faithful` | `Deferred`) on `AreaRenderer::path`. Only the
  renderer of the level's own meshes acts on it; objects, rings and effects still draw
  forward over its depth, as they do now.
- `thedas-app --renderer faithful|deferred` (default `faithful`), the launcher's
  "Renderer" choice, **F11** in game to flip between them at once, and the step
  `renderer faithful|deferred|toggle` for `--steps` and `--control` runs.
- When the GPU refuses the deferred pipelines (`gpu::checked`), the level stays faithful
  and says so in the log once.

## Frame (deferred path)

1. Shadows exactly as now (the faithful pass places and draws the sun partitions).
2. **G-buffer pass** over the deferred draws, writing the shared depth buffer:

   | target | format | contents |
   |---|---|---|
   | 0 | `Rgba8Unorm` | albedo rgb (`mml_tDiffuse`), unused |
   | 1 | `Rgba8Unorm` | specular mask rgb (`mml_tSpecularMask`), specular power code (its alpha) |
   | 2 | `Rgba16Float` | normal (octahedral xy, world space, from `mml_tNormalMap` through a derivative-built tangent frame), room index, specular falloff (`mml_vFalloffParams`, as Static1's vertex shader works it out) |
   | 3 | `Rgba8Unorm` | lightmap texel (rgb baked light, a: the baked visibility Static1 multiplies the sun and lights by) |

   Position comes back from depth. 20 bytes a pixel, under every backend's limit.
3. **Lighting pass**, one full-screen triangle into the frame: Static1's sum per pixel
   (`frame-gamma-lightmap-trace.md`): `albedo · (lm.rgb + lm.a · Σ(sun, point lights)) +
   specular`, its `cos²(π/2 · d/R)` falloff and Blinn-Phong highlight, then the level's fog.
   Point lights come from a storage buffer holding **every** level light (the scene's and
   the added ones, i.e. cutscene lights), each with a bit set of the rooms it reaches
   (`AreaScene::light_rooms`), tested against the pixel's room: no 3-light cap, no leaking
   through walls into rooms the original never lit.
4. Everything not deferred draws forward with its original shaders over that colour and
   depth: blended and multi-pass materials, water, sky, terrain, trees, skinned meshes,
   cut-away meshes, particles. Then the frame effects as now.

**Deferred draws** (first cut): the level's opaque and alpha-tested D3D draws whose material
is `static.mat`, not skinned, not SpeedTree, not cut away. Terrain (`terrain.mat`), then
characters and placeables, move over in later steps.

## Lights from areas and from effects

- Area lights: `AreaScene::lights` (`_ll` light models: point lights with colour, radius,
  intensity, room) plus `set_extra_lights` (cutscene lights). Ambient light nodes keep
  lighting placeables only, as in the original.
- Spell and VFX lights (later step): each live emitter of a prop's effect becomes a point
  light at the emitter, its colour the particles' current colour, scaled by how many are
  alive against the emitter's peak, its radius from the emitter's spread. Light nodes inside
  VFX models, where a model has them, are used as they are.

## Shadows (later steps)

- Sun: the faithful path's two partitions, sampled in the lighting pass (same matrices,
  `ShadowTargets::frame`), so both paths agree on where shadows fall.
- Point and VFX lights: cube shadow maps for the few lights that matter most this frame
  (screen size times intensity), from a fixed pool; the rest unshadowed.
- With both in, `lm.a` is no longer needed as the visibility term.

## Real-time lighting in place of lightmaps (later steps)

Lightmap rgb is the baked bounce and the baked-only lights; `lm.a` the baked visibility.
A "baked lighting" switch under the deferred path turns lightmaps off: visibility then comes
from the shadow maps, the bounce from the level's irradiance probes (`AreaScene::probes`) and
screen-space ambient occlusion. Lightmaps stay the default until that looks at least as good.

## Order of work

1. Switch plus G-buffer and lighting for static level geometry (this document's first PR).
2. Sun shadows in the lighting pass; terrain into the G-buffer.
3. VFX and spell lights; point light shadow pool.
4. Characters and placeables into the G-buffer (skinned vertex path).
5. Lightmaps off: probes and SSAO; then TAA and other post work.
