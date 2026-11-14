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

- `thedas_render::RenderPath` (`Faithful` | `Deferred`) on `AreaRenderer::path`. The level
  and its eligible objects share a G-buffer; remaining materials, rings and effects draw
  forward over its depth.
- `thedas-app --renderer faithful|deferred` (default `faithful`), the launcher's
  "Renderer" choice, **F11** in game to flip between them at once, and the step
  `renderer faithful|deferred|toggle` for `--steps` and `--control` runs.
- When the GPU refuses the deferred pipelines (`gpu::checked`), the level stays faithful
  and says so in the log once.

Faithful loads skip deferred planning entirely, including stream conversion and texture
lookups. Renderers constructed for deferred prepare it at load; the first F11 switch from
faithful prepares the existing object scene and reloads only the level's CPU scene to plan
its draws. That first switch can pause for scene loading and GPU setup; subsequent switches
reuse the plan. Forward-only rings and effects, portraits and paperdolls do not request one.
API callers use `AreaRenderer::new_cached_with_path`, or call `prepare_deferred` with the
original scene before selecting `path = RenderPath::Deferred`.

## Frame (deferred path)

1. Shadows exactly as now (the faithful pass places and draws the sun partitions).
2. **G-buffer pass** over the deferred draws, writing the shared depth buffer:

   | target | format | contents |
   |---|---|---|
   | 0 | `Rgba8Unorm` | albedo rgb; alpha material code: Static1 (0), prop (64/255), Low terrain (128/255), character (192/255), other terrain (1) |
   | 1 | `Rgba8Unorm` | specular mask rgb (`mml_tSpecularMask`), specular power code (its alpha) |
   | 2 | `Rgba16Float` | normal (octahedral xy, world space, from `mml_tNormalMap` through a derivative-built tangent frame), room index, specular falloff (`mml_vFalloffParams`, as Static1's vertex shader works it out) |
   | 3 | `Rgba16Float` | lightmap RGB and baked visibility, or object probe irradiance and visibility 1; floats preserve HDR probes |

   Position comes back from depth. 24 bytes a pixel (32 under wgpu's attachment accounting),
   within the portable limit.
3. **Lighting pass**, one full-screen triangle into the frame: Static1's sum per pixel
   (`frame-gamma-lightmap-trace.md`): `albedo · (lm.rgb + lm.a · Σ(sun, point lights)) +
   specular`, its `cos²(π/2 · d/R)` falloff and Blinn-Phong highlight, then the level's fog.
   One deliberate difference: Static1's falloff wraps past the radius (it rises again to
   full at 2R), and the original hands a light to whole objects whose bounding sphere it
   reaches, so large floors catch light from beyond its radius. That is most of the
   faithful Cousland hall floor's brightness; per pixel it draws hard rings, so the deferred
   falloff ends at the radius (`THEDAS_DEFERRED_VIEW=wrap` shows the other choice).
   Point lights come from a uniform array (192, which every backend can bind) holding the
   level lights whose reach is in view (the scene's and the added ones, i.e. cutscene
   lights; nearest first past 192), each with a bit set of the rooms it reaches
   (`AreaScene::light_rooms`), tested against the pixel's room: no 3-light cap, no leaking
   through walls into rooms the original never lit. Tiled or clustered culling replaces
   the per-pixel loop over all of them when light counts call for it.
4. Everything not deferred draws forward with its original shaders over that colour and
   depth: blended, alpha-tested and multi-pass materials, water, sky, trees,
   unsupported character materials, cut-away meshes and particles. Then frame effects as now.

Debug views in place of the lit frame: `THEDAS_DEFERRED_VIEW=albedo|normal|lightmap|
visibility|diffuse|specular|no-rooms|flat|wrap|shadows|sun-shadows|ao` (see `deferred::debug_view`).

The environment reflection Static1 adds (the probe cubemap times
`mml_fSpecularReflectionMult`, the specular mask and falloff) has no room in the G-buffer
(wgpu's 32 bytes a pixel): a second draw of the deferred meshes adds it over the lit frame.

Static1's material tint and the meshes' own tangents remain to come (the G-buffer builds
a tangent frame from screen derivatives).

**Deferred draws**: the level's own opaque draws with Static1's shaders
(`Static1.psh`, or `Static1_Fast.psh` at Low; no alpha test, no blending), not
SpeedTree or cut away; and Origins terrain (`Terrain2.vsh` + `Terrain1.psh`, or
`Terrain3` at Low detail).

**Objects and characters**: opaque, single-pass `Prop1`, `Weapon1`, `Cloth1`, and
`Char1` with `Ch1`, its NoTint, MulTint and ArmourTint variants, and their emissive
variants. Static1 draws in object renderers also qualify. `THEDAS_DEFERRED_OBJECTS=0`
keeps the new coverage forward for comparison. Portrait and paperdoll renderers keep
their original path. Fade-outs return to the faithful blended draw for that frame.

The deferred vertex stream retains morph positions and carries all four blend weights
and indices. A vertex uniform palette (up to Origins' 67 affine bones) poses positions
and normals on the GPU; point shadows use exactly the same stream, palette and transforms.
Missing palette rows stay at identity. Malformed streams and oversized palettes stay
forward. Posed bounds select point-shadow casters, including objects outside the camera;
meshes explicitly marked not to cast shadows and faded objects do not enter those maps.
The faithful renderer exposes only read-only palette and bound accessors to this path.

The level and object renderers contribute before a single lighting pass, sharing room
indices, depth, sun visibility and the four point-shadow slots. Their environment and
emissive terms are added afterwards. Objects interpolate their assigned SH probe's light
from posed vertex normals; normal maps affect direct lighting. Placeables also
take the brightest ambient node, while characters take no ambient nodes. Character-domain
point lights affect character materials, level-domain lights affect the others, and emitter
lights affect both. Object point lights use their own visibility; only their directional
sun takes the sun mask. Static1's grazing-angle shadow floor is confined to level materials.
Probe bounce does not take the level lightmap's sun mask. Character
diffuse and specular tint opacities remain independent, including the separate skin channel.

This is our deferred material approximation, not shader parity: character rim lighting,
gore/heraldry and remote-material effects are not represented yet. Faces, eyes, hair,
alpha-tested and blended materials, lower-detail Char2 and other unsupported families
still draw forward; they also remain absent from deferred point-shadow casters. Tangent
frames still come from derivatives. Probe assignments remain those made at scene load.
The float lightmap target may change baked-image rounding slightly.

Terrain's G-buffer material decodes MaskV's three layer indices with point sampling and
blends their palette texels using MaskA/MaskA2's eight weights. The material supplies atlas
dimensions, padding, UV scales and each layer's specular colour/power. Explicit gradients
keep tile wrapping from choosing the wrong mip. Terrain uses TEXCOORD0 for its lightmap,
normal-map palette at Medium/High, vertex normals and no specular at Low, and the original
vertex-interpolated specular distance factor. As with Static1, its tangent frame comes from
screen derivatives rather than the raw mesh tangents.

The lighting keeps Terrain1's `lm.a²` static diffuse mask (Terrain3 at Low uses `lm.a` once) and
`0.75 * runtime_visibility * lm.a + 0.25` specular mask; bounce uses the same 15% shadow
blend as Static1. Low uses Terrain3's shadow filter without the grazing-angle floor.
Terrain reflections use this visibility too, via shared read-only sun
bindings, and the material's additive tint is added with the reflection pass. Point lights
are evaluated per pixel with the deferred radius cutoff, so illumination near fires can
differ from the faithful per-vertex single A/D lights. `THEDAS_DEFERRED_TERRAIN=0` keeps
terrain forward for before/after captures; `albedo`, `normal`, `lightmap` and `sun-shadows`
debug views now include the ground. Terrain also joins the point-shadow caster list.

## Lights from areas and from effects

- Area lights: `AreaScene::lights` (`_ll` light models: point lights with colour, radius,
  intensity, room) plus `set_extra_lights` (cutscene lights). Ambient light nodes keep
  lighting placeables only, as in the original.
- Glowing emitters (done): every particle emitter drawn additively (torch and brazier
  flames, embers, spell effects), in the level's, its objects' and the visual effects'
  renderers, gives the deferred pass a point light each frame at its particles' glow-
  weighted centre, its colour their hue and its strength rising with the light they show
  (`deferred::emitter_light`, constants tuned by eye). Lights without a room reach every
  room; room boxes do not reliably identify the floor of a multi-storey area.
  `THEDAS_DEFERRED_LOG_LIGHTS=1` logs a frame's lights and shadow slots. Original idea
  below kept for what is left:
- Spell and VFX lights (later step): each live emitter of a prop's effect becomes a point
  light at the emitter, its colour the particles' current colour, scaled by how many are
  alive against the emitter's peak, its radius from the emitter's spread. Light nodes inside
  VFX models, where a model has them, are used as they are.

## Shadows

- Sun: the faithful path's two partitions, sampled in the lighting pass (same matrices,
  targets, casters, quality and temporal box smoothing). Static1's five-tap near filter
  reads red depth and blurred green coverage; the far filter compares four depths and
  fades to lit over the final quarter of its reach. Its grazing-angle floor is retained.
  Outside either map is lit. `THEDAS_DEFERRED_SUN_SHADOWS=0` disables sampling for comparison;
  `THEDAS_DEFERRED_VIEW=sun-shadows` shows visibility on deferred surfaces.
  The baked direct visibility `lm.a` still applies once to static diffuse (and with a 0.2
  floor to specular), multiplied by runtime visibility as in Static1. Baked RGB bounce
  takes only `lerp(1, visibility, 0.15)`, the original exploration-camera blend; neither
  the whole lightmap nor dynamic emitter lights are multiplied by a second sun mask.
  World reconstruction undoes the G-buffer vertex shader's half-pixel fix-up.
- Point and VFX lights: four cube shadow slots, allocated to dynamic lights by brightness
  times radius over distance from the camera. Each is six 512-square `R32Float` distance
  maps with a 0.35 m near plane to leave the flame's enclosing torch bowl out. Opaque
  eligible deferred meshes cast from both faces, including geometry outside the camera's
  view. The lighting pass compares distances with a bilinear 2-by-2 filter, a 4 cm normal
  offset and 2 cm distance bias. Supported skinned objects and terrain cast too; forward
  materials and trees do not. Static lights keep baked visibility. At most 24 face passes per frame.
  `THEDAS_DEFERRED_POINT_SHADOWS=0` disables both rendering and sampling these maps for
  captures and cost comparisons; it does not change the faithful path.
- With baked lighting on, static lights and static geometry still depend on `lm.a`.
  The experimental lightmaps-off mode below lets static lights use the point-shadow pool.

## Real-time lighting in place of lightmaps (experimental)

Lightmap rgb is the baked bounce and the baked-only lights; `lm.a` the baked visibility.
`THEDAS_DEFERRED_BAKED_LIGHTING=0` replaces lightmaps on opaque level Static1 and terrain
surfaces. Baked lighting stays on by default; the faithful path ignores the
switch. Choose it before loading an area. F11 still switches between faithful and deferred.

- Ambient RGB comes from each draw's assigned irradiance probe (`AreaScene::probes`),
  evaluating its three quadratic forms at the world-space mapped normal. The level's
  existing room-probe assignment is retained. Missing probes use the same provisional
  `[0.35, 0.35, 0.35]` irradiance fallback as the faithful objects. Negative SH lobes clamp
  to zero. The fourth G-buffer target uses `Rgba16Float` in this mode so bright probes
  survive above one; the four targets still meet the portable 32-byte attachment budget.
- Baked visibility becomes one. The sun uses the existing partitions; each point light
  uses its own cube shadow if it wins one of the four slots. Static point lights now
  compete with dynamic ones for those slots, and no point light takes the sun's mask.
  Other point lights remain unshadowed, with the existing room reach and radius cutoff.
- A deterministic twelve-sample world-space hemisphere provides screen-space ambient
  occlusion over a 0.8 m radius. It uses depth-derived geometric normals and affects only
  probe diffuse, with metric bias and range rejection to avoid flat-surface acne and
  distant foreground halos. Sky and offscreen samples are open. There is no noise rotation,
  temporal filter or extra render target.
  `THEDAS_DEFERRED_SSAO=0` disables it for comparison; `THEDAS_DEFERRED_VIEW=ao` shows it.
  In this mode `lightmap` shows probe irradiance and `visibility` shows white.

This is a first comparison mode, not a complete replacement for baked lighting. Probes are
the original baked irradiance and may contain direct illumination; they are not a new GI
solution. Only deferred surfaces contribute to AO and point shadows, so unsupported
forward materials and trees are missing from those occluders. Eligible characters,
placeables and terrain contribute
to both, and its reflections also stop using baked visibility in this mode. The small point-shadow
budget, missing occluders, screen edges and coarse room probes can cause light leaks or
ambient discontinuities. The original baked image remains the default while those are
addressed. Moving the remaining material families into the G-buffer is still
the next coverage step.

## Order of work

1. Switch plus G-buffer and lighting for static level geometry (this document's first PR).
2. Sun shadows in the lighting pass; terrain into the G-buffer.
3. VFX and spell lights; point light shadow pool.
4. Characters and placeables: the shared G-buffer and skinned vertex path are in place;
   extend the remaining material families and cutout casters next.
5. Lightmaps off: the Static1 probe/SSAO comparison mode is in place; extend coverage and
   visibility before making it a player-facing option. Then TAA and other post work.
