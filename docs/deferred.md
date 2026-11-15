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
- Depth-disabled selection rings and the aiming reticle retain their ground-overlay
  blending over terrain. In deferred frames, the colour before these marks is restored
  over the G-buffer's character material code. This includes any nearer forward occluder
  and preserves each MSAA sample. Forward-only characters still draw after the marks.
  `THEDAS_DEFERRED_GROUND_MASK=0` disables this restoration for comparisons. Single-sample
  targets need `COPY_SRC` usage; targets without it retain the unmasked overlay. The
  mask follows the G-buffer's single-sample character coverage, including under MSAA.
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

Point-shadow cubes retain their previous contents when their exact inputs are unchanged.
Each slot compares its light position/radius and the eligible casters in reach: geometry
buffers, transform, posed bounds, bone palette values, opacity test/clip, and immutable
material texture/sampler handles. Tree keys also include wind/placement vertex constants,
leaf orientation, LOD index range and alpha reference. A changed slot redraws all six
faces; removed/faded casters and newly empty slots clear correctly. Camera movement
can reuse a cube unless it changes slot selection or a tree's LOD/orientation.
Resource reloads replace the renderer and its cache.
`THEDAS_DEFERRED_SHADOW_REUSE=0` redraws every active cube for comparison. Animated or moving
casters still invalidate every cube they reach; stationary capture gains do not predict
the same gain during motion.

For same-process ABBA captures, combine `THEDAS_BENCH_FRAMES=120`,
`THEDAS_BENCH_DUMP=1` and `THEDAS_BENCH_SHADOW_COMPARE=reuse|faithful`. The reuse
control forces every active cube to redraw while both modes retain exact input
bookkeeping; the other comparison alternates faithful/deferred rendering. Overrides are
per renderer and restored afterwards, without mutating environment variables while
worker threads exist. Discard warm-up frames and pair adjacent opposite modes.

1. Shadows exactly as now (the faithful pass places and draws the sun partitions).
2. **G-buffer pass** over the deferred draws, writing the shared depth buffer:

   | target | format | contents |
   |---|---|---|
   | 0 | `Rgba8Unorm` | albedo rgb; alpha material code: Static1 (0 or a spare code for tint), Black (32/255), prop (64/255), Low terrain (128/255), character (192/255), other terrain (1) |
   | 1 | `Rgba8Unorm` | specular mask rgb (`mml_tSpecularMask`), specular power code (its alpha) |
   | 2 | `Rgba16Float` | normal (octahedral xy, world space, from `mml_tNormalMap` through the mesh's tangent frame, with derivative fallback), room index, specular falloff (`mml_vFalloffParams`, as Static1's vertex shader works it out) |
   | 3 | `Rgba16Float` | fused baked ambient/reflection/emission RGB and baked visibility; separate-pass modes retain lightmap or probe irradiance RGB; floats preserve HDR probes |

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
   through walls into rooms the original never lit. A conservative 16-by-9 screen grid
   projects each light sphere's world AABB, with one pixel of margin for reconstruction
   and tile edges. Bounds crossing the eye plane use every tile. Six ascending 32-bit
   masks per tile select candidates; exact radius, room and material-domain tests still
   run per pixel, in the original accumulation order. Two padded vec4s per tile take
   4,608 bytes in a portable uniform buffer; there are no storage buffers or candidate
   count truncations. The `wrap` debug view expands tile bounds to twice the radius.
   `THEDAS_DEFERRED_TILED_LIGHTS=0` puts every light in every tile for comparisons.
4. Roomless rigid `SPT_Fronds`, `SPT_Branch`, `SPT_LeafMeshes` and `SPT_LeafCards`
   draws join the G-buffer using their existing translated
   wind/placement vertex shader, instance groups and LOD alpha references. Their authored
   ambient and world normals feed the shared per-pixel lighting, including branch normal
   maps. Leaves retain the original 8-bit LOD cutout rule; bark remains opaque. These
   surfaces receive runtime shadows and contribute to SSAO depth. These four families
   also cast point shadows, including when their colour stays forward.
   `THEDAS_DEFERRED_FRONDS=0`, `THEDAS_DEFERRED_BRANCHES=0`,
   `THEDAS_DEFERRED_MESH_LEAVES=0` and `THEDAS_DEFERRED_LEAF_CARDS=0` each keep that
   family forward. The material survey and remaining coverage
   are recorded in [the inventory](notes/deferred-material-coverage.md).
5. Eyelashes and all four compatible hair passes use the shared deferred lights and
   shadows in their existing forward phases, preserving sampled coverage, depth tests,
   culling and source-alpha blending. Hair retains its packed channels, noise tint and
   two strand lobes; degenerate strand vectors use a finite tangent fallback. Runtime
   fades and incompatible inputs keep the complete family on its faithful shaders.
   `THEDAS_DEFERRED_FORWARD_LIGHTING=0` disables this planning; it is read once.
   [The material note](notes/deferred-material-coverage.md#ordered-character-lighting)
   describes admission and remaining coverage. Other materials draw with their original
   shaders over that colour and depth. All renderers first draw their remaining opaque
   geometry. Blended and faded geometry, ordered hair families and particles then draw
   against the completed opaque depth, before the MSAA
   resolve and frame effects. This keeps a forward object from overwriting particles or
   water in front of it. Draw order within each renderer stays authored; this is not a
   global transparency sort or order-independent transparency. Distortion still uses its
   later shared mask. `THEDAS_DEFERRED_LATE_TRANSPARENCY=0` restores the earlier per-renderer
   order for comparison. Faithful frames keep their existing order.

   API callers of `render_main_with` finish their opaque overlays with `render_over`, then
   call `finish_transparency` on the level and each overlay, using the shared frame/depth
   views, before `resolve`. It is a no-op without pending work. `capture_with` does this
   automatically; a standalone renderer needs no split.

Debug views in place of the lit frame: `THEDAS_DEFERRED_VIEW=albedo|normal|lightmap|
visibility|diffuse|specular|no-rooms|flat|wrap|shadows|sun-shadows|ao` (see `deferred::debug_view`).

With baked lighting, the G-buffer's HDR lightmap RGB stores the sum of baked ambient
colour, probe reflection and emission. This reuses the material samples and tangent frame
from the first geometry pass and removes the second geometry pass. The baked visibility
alpha still masks direct lights in the lighting pass. The probe cubemap, reflection
multiplier, full-resolution normals, specular mask/power/falloff, terrain sun visibility,
terrain additive tint and character emission retain their existing equations. Faces and
eyes still receive no cubemap reflection. Bounce retains the stored albedo's UNORM
clipping and the lightmap's half-float precision. This reorders ambient shading and
colour writes. Captures have small mean differences and larger localized differences;
use the comparison control when reviewing image changes.

`THEDAS_DEFERRED_FUSED_REFLECTIONS=0` retains the separate reflection pass for comparison.
Probe/SSAO mode and debug views always retain it: AO needs the completed depth buffer and
must affect bounce independently of reflection. Fusion is selected for the whole shared
G-buffer, including object contributors, on each frame; renderer switches, target resizes
and shadow updates do not reuse stale reflection pixels.

Nonzero Static1 material tints apply to the complete fused lighting sum before fog:
`clamp(colour / (1 - tint.rgb), 0, 1) * (1 - tint.a)`. This includes bounce,
direct diffuse/specular and cubemap reflection. Spare albedo material codes select a
4 KiB uniform table shared by level and object contributors; albedo and specular masks
retain their existing precision and the G-buffer formats and geometry passes stay the
same. Untinted materials retain the existing deferred HDR equation. The palette
supports 250 distinct transforms; an overflowing shared frame renders faithfully.
Singular or nonfinite tints retain their faithful draw. Probe/SSAO, diagnostic and
separate-reflection modes also retain tinted draws on their faithful shaders: applying
the nonlinear clamp to bounce and reflection separately would be incorrect. Switching
back to fused baked lighting restores their G-buffer coverage. Runtime fades keep the
existing faithful handoff. `THEDAS_DEFERRED_STATIC_TINT=0` restores the previous omission;
this comparison switch is read once.

The G-buffer uses authored mesh tangent frames where complete, with screen derivatives
as a fallback (see the controls below).

**Deferred draws**: the level's own opaque draws with Static1's shaders
(`Static1.psh`, or `Static1_Fast.psh` at Low; no alpha test, no blending), not
the far SpeedTree billboards; and Origins terrain (`Terrain2.vsh` + `Terrain1.psh`, or
`Terrain3` at Low detail).

Supported cutaway meshes use the G-buffer while fully opaque, including Static1,
Static1 punch and props. Any runtime opacity below one hands colour back to the
faithful forward phase and removes the mesh from the point-caster set. Opacity zero
draws nothing; returning to one restores deferred coverage and invalidates affected
shadow cubes. `THEDAS_DEFERRED_CUTAWAY=0` restores the blanket cutaway exclusion from
both the G-buffer and the independent cutout-caster plan. The control is read once.

Rigid, single-pass Origins `Black` geometry also shares deferred depth. Its dedicated
material remains constant opaque black under lighting, fog, probes and diagnostic
views, and has no reflection redraw or texture sampling. Normal/depth data remains
available to AO. It respects the scene's point-casting flag and runtime fades.
`THEDAS_DEFERRED_BLACK=0` keeps it forward; this control is also read once. The current
single-sample G-buffer limitation still applies to its silhouette under MSAA.

Single-pass `Static1_Punch` also enters the G-buffer, retaining diffuse alpha times
`mml_fOpacityMultiplier`, the shader's extra 0.3 clip, and its authored comparison/reference
on rounded saturated 8-bit alpha. Its diffuse sampler retains addressing, min/mag/mip
filtering and border fallback. The same coverage masks reflections; holes write neither
G-buffer values nor depth, so SSAO and later forward geometry see through them. Missing
opacity maps keep the faithful fallback. `THEDAS_DEFERRED_PUNCH=0` keeps these draws
forward. Alpha coverage is binary at the G-buffer's single sample, including with MSAA.
Cutout point shadows use the independent textured-caster plan; these cards never enter
the opaque cube-shadow pipeline. Prop punch and unsupported blended materials retain
their original forward equations. Compatible eyelash/hair colour uses the ordered
lighting path described above.

Single-pass, depth-writing `Char1` / `Ch1Alph` character cutouts also share the G-buffer
when their pass alpha-tests without blending. The ordinary, NoTint, MulTint, ArmourTint
and corresponding emissive variants retain the existing character probe, tint and lighting
equations. Coverage uses diffuse alpha times the authored opacity multiplier and the
pass's rounded 8-bit comparison/reference. This family has no additional shader clip.
Its G-buffer coverage is single-sampled and binary, including under MSAA.
The same sampled coverage supplies posed point-shadow casters. Missing opacity maps,
multi-pass materials, runtime fades and unsupported palettes retain the forward fallback.
`THEDAS_DEFERRED_CHARACTER_CUTOUTS=0` disables both additions for comparison.

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
indices, depth, sun visibility and the point-shadow atlas. Their environment and
emissive terms are added afterwards. Objects interpolate their assigned SH probe's light
from posed vertex normals; normal maps affect direct lighting. Placeables also
take the brightest ambient node, while characters take no ambient nodes. Character-domain
point lights affect character materials, level-domain lights affect the others, and emitter
lights affect both. Object point lights use their own visibility; only their directional
sun takes the sun mask. Static1's grazing-angle shadow floor is confined to level materials.
Probe bounce does not take the level lightmap's sun mask. Character
diffuse and specular tint opacities remain independent, including the separate skin channel.

Opaque `Face1/Face0` heads and `Eye0` eyes also join this path. Faces retain morphs and
the four-weight pose, independently blend aged diffuse and normals, add expression and
brow/stubble normal detail, and compose skin tint, tattoos, facial hair and the other
three diffuse tint channels. Regional face roughness and diffuse alpha control a single
Blinn specular lobe; this approximates the original skin BRDF and Beckmann lookup.
Face probe irradiance takes its authored ambient multiplier. Eyes retain iris tint,
base and cornea normals and both authored specular powers/masks; the one-lobe G-buffer
blends the normals/powers by their masks rather than evaluating two separate highlights.
Neither family receives the ordinary cubemap reflection. They share character lights,
room indices, SSAO depth and posed point-shadow geometry with the bodies.
`THEDAS_DEFERRED_FACES=0` keeps both families forward for comparisons.

This is our deferred material approximation, not shader parity: character rim lighting,
skin scattering, gore/heraldry and remote-material effects are not represented yet. Hair,
unsupported alpha-tested and blended materials, lower-detail Char2 and other unsupported families
still draw forward; the known cutouts described below can supply point-shadow coverage. Tangent
frames use the authored mesh basis where complete. Probe assignments remain those made at
scene load.
The float lightmap target may change baked-image rounding slightly.

Terrain's G-buffer material decodes MaskV's three layer indices with point sampling and
blends their palette texels using MaskA/MaskA2's eight weights. The material supplies atlas
dimensions, padding, UV scales and each layer's specular colour/power. Explicit gradients
keep tile wrapping from choosing the wrong mip. Terrain uses TEXCOORD0 for its lightmap,
normal-map palette at Medium/High, vertex normals and no specular at Low, and the original
vertex-interpolated specular distance factor. As with Static1, its tangent frame uses the
raw mesh tangents and binormals where complete, falling back to screen derivatives.

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
  (`deferred::emitter_light`, constants tuned by eye). Its colour also takes the
  particle texture's tint (its alpha-weighted mean colour over a small mip, brightest channel
  one; `deferred::texture_tint`), since some effects, lightning for one, carry their colour
  in the texture rather than the particles. Two kinds give none: light-shaft and
  lit-dust effects (`fxe_day_beam`, `fxe_day_sunshine`, `fxe_day_dust_shine`, matched by
  name), which show light rather than give it, and any emitter within 1.5 m of a level static
  light, a torch or brazier the level already lights with its own lamp, or within 3 m of a
  visual effect's own light (effects carry authored lights their animations drive, #971). Before this, the
  Cousland hall camera was 40% brighter than the faithful frame (mean 69.3 against 49.6; 41.2
  after). `THEDAS_DEFERRED_EMITTER_LIGHTS=0` turns emitter lights off. Lights without a room reach every
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
- Point and VFX lights: up to sixteen cube shadow slots, allocated to dynamic lights by brightness
  times radius over distance from the camera. Each is six 512-square `R32Float` distance
  maps with a 0.35 m near plane to leave the flame's enclosing torch bowl out. Opaque
  eligible deferred meshes cast from both faces, including geometry outside the camera's
  view. Each cube face conservatively culls posed world spheres against all six clip
  planes before allocating/uploading draw uniforms. Face seams and near/far crossings
  retain a millimetre margin, and unknown bounds stay in every face.
  `THEDAS_DEFERRED_SHADOW_CULL=0` restores six submissions per in-range caster for
  comparisons. The lighting pass compares distances with a bilinear 2-by-2 filter, a 4 cm normal
  offset and 2 cm distance bias. Supported skinned objects and terrain cast too; forward
  materials do not, except for the trees and cutouts below. Static lights keep baked
  visibility. Six atlas sheets hold one 512-square tile per light, with one render pass
  per dirty sheet (at most six per frame). Each pass clears only dirty colour tiles;
  clean cubes retain their distances. A shared scratch depth sheet is cleared per pass.
  The filter clamps its four taps to the selected tile before adding its atlas origin,
  so adjacent lights cannot bleed into one another at face edges or corners. The CPU
  packs that origin into spare light-uniform fields, avoiding per-pixel tile division.
  `THEDAS_DEFERRED_SHADOW_SLOTS=4` retains the previous light budget for comparisons;
  integer values clamp to 1–16, with sixteen as the default. This control is read once.
  The sixteen-slot atlas is six 2048-square distance sheets (96 MiB) and one 2048-square
  scratch depth sheet (16 MiB). Four slots use six 1024-square sheets and 4 MiB depth
  (28 MiB total, versus the previous 48 MiB). Face matrices occupy 6 KiB of uniforms,
  within the portable 16 KiB binding limit. No G-buffer attachment is added.
  `THEDAS_DEFERRED_POINT_SHADOWS=0` disables both rendering and sampling these maps for
  captures and cost comparisons; it does not change the faithful path.
- Tree point casters: roomless rigid single-pass fronds, branches, mesh leaves and
  leaf cards reuse their translated wind/placement shader through a private uniform
  stream. An identity view-projection returns deformed world positions, then each
  cube face projects them. The viewing camera still supplies leaf orientation and
  distance LOD; its frustum and instance groups do not limit casting. Cutout families
  sample the authored diffuse map/sampler and use the current 8-bit Greater alpha
  reference. Bark stays opaque, including at zero diffuse alpha. Faded placements
  and inactive LODs cast nothing. Both faces block point lights. The scene's tree
  no-cast flag belongs to the faithful sun path; these point casters are independent.
  Far billboards remain unsupported. `THEDAS_DEFERRED_TREE_SHADOWS=0` disables this
  addition; the environment control is read once. The renderer-local
  `THEDAS_BENCH_DEFERRED_COMPARE=tree-shadows` control alternates casting within one
  load and restores the planned mode afterwards. Enable tree-shadow planning for it.
  Switching this coverage invalidates affected cubes, so its timings include those
  redraws. The `faithful` comparison retains steady deferred shadow reuse.
- Forward cutouts: known Origins `Static1_Punch`, `Prop1_Punch`, alpha-tested `Ch1Alph`,
  `Eye0/Eyelash0`, and the
  colour-disabled `Hair0_Pass1` depth core also cast into point-light cubes, independently
  of G-buffer coverage and `THEDAS_DEFERRED_OBJECTS`. Geometry retains UVs, morphs and the
  current four-weight pose. The opacity map, sampler address/filter/mip states, opacity
  multiplier, comparison and 8-bit reference are retained. Static1_Punch additionally
  clips diffuse alpha times opacity below 0.3; hair takes opacity from the packed
  texture's blue channel and admits only its depth core's `>= 255` coverage. All visible
  hair passes remain forward and are not suppressed. Eyelashes take diffuse alpha.
  Authored prop punch and eyelash passes also blend colour while writing tested depth.
  Their ordinary source-alpha blend is accepted for point-shadow coverage, while their
  colour passes stay forward. Prop punch retains its additional 0.3 shader clip; eyelash
  coverage retains the authored reference (normally 20). Other blend equations, missing
  tests, disabled depth/write, and unknown material families do not qualify. The cube
  stores binary occlusion after the tests, not fractional transmission through surviving
  texels. `THEDAS_DEFERRED_BLENDED_CUTOUTS=0` restores the earlier exclusion of blends.
  Casts are deduplicated per scene draw; explicit no-cast flags, current posed bounds,
  fade-outs and cutaway exclusions still apply. Missing opacity maps do not become
  rectangular casters. `THEDAS_DEFERRED_CUTOUT_SHADOWS=0` disables just these additions
  for comparison. Other material families and tree billboards remain unsupported;
  this does not add forward cutouts to SSAO or change the faithful sun shadows.
- With baked lighting on, static lights and static geometry still depend on `lm.a`.
  The experimental lightmaps-off mode below lets static lights use the point-shadow pool.

## Lightmap modes

The deferred path's treatment of the baked lightmaps is chosen once per process:
`thedas-app --lighting baked|hybrid|realtime`, the launcher's "Deferred lighting" choice,
`thedas_render::set_baked_lighting` before the first renderer is made, or
`THEDAS_DEFERRED_BAKED_LIGHTING` (`0` realtime, `bounce`/`hybrid` hybrid). The faithful
path ignores it.

- `baked` (default): lightmap RGB is the baked bounce, alpha the baked direct visibility,
  as Static1 uses them.
- `hybrid`: the lightmaps keep only their bounce. Static point lights compete with the
  dynamic ones for the sixteen cube shadow slots; a static light holding one takes its
  cube's visibility in place of the baked mask (and, like a dynamic light, not the sun's
  mask). Where a sun cascade covers a point, the sun takes the cascade's visibility alone.
  Lights without a cube and the sun outside the cascades keep the baked mask, so nothing
  leaks where runtime visibility is unknown. Characters and placeables now shadow the
  level's own torches and the sun's light the same way the lightmaps shadowed static
  geometry. The bounce stays baked: it does not move with doors, characters or spells.
- `realtime`: below.

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
  uses its own cube shadow if it wins one of the sixteen slots. Static point lights now
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
solution. Deferred surfaces contribute to AO; supported forward cutouts and tree
families also cast point shadows. Other forward families and far tree billboards
remain missing from those occluders. Eligible characters, placeables and terrain contribute
to both, and their reflections also stop using baked visibility in this mode. The small point-shadow
budget, missing occluders, screen edges and coarse room probes can cause light leaks or
ambient discontinuities. The original baked image remains the default while those are
addressed. Moving the remaining material families into the G-buffer is still
the next coverage step.

### Fitted light sets (experimental)

`THEDAS_DEFERRED_LIGHTSET=<file or directory>` gives the lightmaps-off mode a level's light set:
what the shipped lightmaps say the original bake's lights were, fitted by `thedas-bake` (see
`docs/lightmaps/baker-spec.md` §7; the sets are derived from game data and live outside the
repository, in OpenThedas-testdata `lightmap/lightsets/<level>.lightset.json`). It does nothing
when unset or in the `baked` mode; `hybrid` takes only the shadow reaches.

- Level surfaces (not objects or characters) take their room's baked ambient in place of the
  probe, encoded as the lightmaps store light (linear^(1/1.5)), uniform over the hemisphere.
- The set's baked-only fill lights join the light list after the shadow slots are chosen
  (never shadowed), nearest first into what room it has left. Each belongs to the room its
  fitted atlas lights. The lighting pass marks them by colour w = 2: the bake's linear
  falloff max(0, 1 − d/R), no highlight, level surfaces only. They are summed linear and
  encoded with the room's ambient as the bake encoded its texels:
  `(ambient^1.5 + Σ fills)^(1/1.5)`, then the ambient occlusion as before.
- **Shadow reach.** The bake's shadow pass reached only `shadow_reach` of a light's radius,
  and not at all for lights it left out (`in_alpha` false), so the shipped alpha masks static
  light beyond that. Wherever runtime visibility replaces the lightmap's mask (`realtime`, and
  `hybrid`'s slotted static lights), a set light lights only within `shadow_reach × R`. The
  reach rides in the light record's domain code as a fraction below 0.5
  (`domain + 0.4 · (1 − reach)`), which every domain test still reads correctly.
- **Occlusion.** With lightmaps off the level's point lights take the SSAO term too, as the
  bake multiplied its alpha (static lights) by its occlusion pass (specular by at least 0.2).

On the Cousland hall camera, against the `baked` frame, `realtime` differs by a mean of 20.4
levels per channel (−18 bias) and `realtime` with the light set by 10.6 (+8.6); on two Lake
Calenhad terrain views 6.1–6.5 and 0.5. With the reach, encoding and occlusion above
(same binary, Cousland): 21.4 without the set, 11.4 with it before the reach, 10.4 with it.
`hybrid` with the set's reaches stays within 1.9 of `baked` (1.8 without them).

Tried and left out: the static lights' baked twins (the RGB pass had a light at every static
light's place, at its fitted brightness) as fills, unshadowed or sharing the static light's
cube: both made the Cousland frame further from `baked` (11.9, 12.1), as the fits' brightness
outliers and the twins' missing bake shadows overshoot. Fill visibility is still missing.

## Real-time bounce

`fs_bounce` gathers one bounce of light at quarter resolution before the lighting pass, which
adds it to the ambient term (times the albedo in fused frames). `THEDAS_DEFERRED_BOUNCE=0`
turns it off; `THEDAS_DEFERRED_VIEW=bounce` shows it alone.

- **Shadowed point lights:** each cube texel is the first surface the light reaches, so it
  is lit; twelve texels per light (two per face, jittered per pixel; neighbouring bounce
  pixels take alternate faces, which the upsample's 2 × 2 average combines) become small
  emitters at that point, with a normal from neighbouring texels and the light's colour and
  falloff. Each stands for its solid angle times its distance squared, and relights a
  receiver by both cosines over `π r²` (`r²` clamped near the patch). Bounce surfaces reflect
  a fixed grey (0.4): the cubes hold no albedo. Lights reach receivers within twice their
  radius and only in rooms they reach.
- **Sun (screen-space):** eight points of a 3 m hemisphere around the receiver, each the
  surface the screen shows there, lit by the sun through its cascades and reflecting its own
  G-buffer albedo (so a green carpet tints nearby stone). The sun's maps can't be used the
  way the cubes are: they see a roof, not the sunlit floor under a window. Off-screen
  surfaces are missed, so the bounce fades at the frame's edges.

Which lights bounce depends on the lightmap mode: with lightmaps (`baked`, `hybrid`) only
the dynamic lights (emitters, spells), whose bounce the lightmaps never held; with none
(`realtime`) the static lights and the sun too. No receiver-to-sender visibility test yet,
no temporal accumulation (the per-pixel jitter is averaged by the depth-aware 2 × 2
upsample only), and one bounce. Cost: on an M4 with the Cousland hall in `realtime` mode,
the first half-resolution version (24 texels a light, 16 sun points) added about 74 ms of
GPU time; this quarter-resolution, interleaved version adds about 3.6 ms there (lighting stage
4.6 → 8.2 ms with `THEDAS_RENDER_PROFILE=1`) and looks the same in captures.

## Measuring frame cost

For fixed-scene save captures (`--save ...`), `THEDAS_BENCH_FRAMES=120 THEDAS_BENCH_DUMP=1`
reports the headless capture loop's CPU/submission/GPU-wait/readback cost, excluding
scene load and PNG encoding. `THEDAS_BENCH_DEFERRED_COMPARE=all` alternates full-candidate
and optimized frames in ABBA order within one loaded scene. `tiles` or `shadows` isolates
one optimization; `tile-frusta` compares projected-box tiles with additional sphere/tile
side-plane rejection; `reflections` compares the separate and fused reflection paths;
`light-masks` compares per-light room/domain checks with transposed membership masks;
`opaque-gbuffer` compares the general G-buffer pipeline with discard-free opaque
pipelines; `lighting` compares all three new optimizations together;
`faithful` alternates the faithful path with optimized deferred. `specialized-gbuffer`
compares the general shaders with rigid Static1/terrain variants; `terrain-layers`
compares the eight terrain-layer comparisons with direct palette selection;
`depth-lights` compares screen tiles with additional view-depth masks;
`early-light-reject` compares early squared-distance rejection;
`pass-cost` switches these four controls together.
`forward-lighting` compares faithful and shared deferred lighting for the planned
eyelash/hair receivers, preserving their forward geometry phases in both cases.
Start with `--renderer deferred` so planning is already complete, keep `--time`, camera,
resolution and AA fixed, and discard the first twenty frames before comparing adjacent
pairs. `bench comparison modes` labels control frames 0 and test frames 1. Overrides
are per renderer, without mutating environment variables while other threads run, and
are cleared after the benchmark. These measurements are not isolated GPU timings or
whole-gameplay FPS; shared-machine load can still affect absolute values.

Rendering environment controls are read once per process. Set them before starting the
engine; paired benchmarks use renderer overrides instead of changing the environment.

G-buffer surfaces and their separate reflection pass use the mesh's declared `TANGENT0`
and `BINORMAL0`. The deferred-only vertex stream decodes the authored formats, skins both
vectors with the same four weights as positions/normals, then applies the world's linear
transform. As in Static1/Prop1, the full basis is interpolated without independently
normalizing or orthogonalizing its columns; mirrored handedness and vector lengths survive.
Normal maps retain their alpha/green XY packing and reconstructed Z. Missing, nonfinite,
zero or coplanar frames retain the previous UV/position derivative frame. An interpolated
validity marker also keeps triangles with invalid vertex frames on the fallback, except
within a 0.0001 interpolation tolerance at a valid edge; posed/interpolated degeneracy is
checked again per fragment. `THEDAS_DEFERRED_MESH_TANGENTS=0` retains the derivative frame
for all these surfaces. The control is read once during deferred stream planning. Hair's
authored strand tangent and translated tree bases keep their existing handling. Faithful
vertex streams and shader inputs are unchanged. The deferred stream grows from 72 to 100
bytes per vertex; G-buffer attachment formats and storage stay unchanged. The derivative
control retains the old equations, but the changed shader interface can alter rounding;
it is not a pixel-exact copy of an older binary.

Rigid Static1 and terrain draws use dedicated G-buffer entry points, sharing the same
material equations while allowing unrelated face/eye/tint branches to be removed. Their
vertex entry point reads positions/normals directly, omitting the identity bone palette.
Skinned draws retain the general vertex path. Cutouts retain their alpha test, and draw
order is unchanged. `THEDAS_DEFERRED_SPECIALIZED_GBUFFER=0` restores the general shaders.
These variants run only with fused baked reflections: probe/AO and diagnostic modes
retain the general vertex path so their separate reflection pass matches G-buffer depth.
Terrain's MaskV decoding selects its sole possible layer directly, retaining the same
0.0625 tolerance and the gaps between codes. Specular blending still accumulates all
eight authored weights in order. `THEDAS_DEFERRED_DIRECT_TERRAIN_LAYERS=0` restores the
eight comparisons for layer weight/scale selection.

Sixteen four-metre view-depth slices further intersect the screen/room/domain light
masks. The final slice extends to infinity; spheres crossing boundaries reach both
slices, with a reconstruction margin, and invalid bounds reach every slice. The screen
and depth masks together occupy 5,120 bytes of portable uniform storage. `wrap` expands
both masks to twice the radius. `THEDAS_DEFERRED_DEPTH_LIGHTS=0` omits the depth
intersection. `THEDAS_DEFERRED_EARLY_LIGHT_REJECT=0` omits the conservative squared-radius
and back-face tests before full light evaluation. Back-face rejection uses the
unnormalized light vector, retaining a tangent-plane margin of 0.00001 times the
light's reach for floating-point rounding. The original distance and normalized-normal
cutoffs remain. Light accumulation order and shadow equations are preserved.

Ordinary baked frames use a lighting entry point with constant mode choices. It
omits AO derivatives, diagnostic branches and per-light room/domain checks, loads
the intersected screen/depth/membership masks as two vectors before the light
loop, and visits only words containing current light indices. A zero RGB specular
mask also skips highlight normalization and exponentiation. Light indices still
accumulate in ascending order with the same diffuse, falloff and shadow equations.
Probe/AO, debug views, separate reflections, and disabled membership/depth/early
rejection controls select the general lighting pipeline on that frame.
`THEDAS_DEFERRED_SPECIALIZED_LIGHTING=0` keeps the general pipeline for comparisons;
the control is read once. `THEDAS_BENCH_DEFERRED_COMPARE=specialized-lighting`
alternates the pipelines in one process and restores the renderer's policy after
capture.

G-buffer draw uniforms retain their allocation and refresh only visible, unfaded
draws. A draw re-entering the view or returning to full opacity refreshes its slot
before drawing. Point shadows retain their independent world and skin streams, so
offscreen casters still update. Changed slots upload in one buffer write.

## Capture profiling

`THEDAS_RENDER_PROFILE=1` records CPU stage durations for offscreen world captures and
requests optional GPU timestamps from the adapter. When timestamps are
available, it also reports durations on the actual G-buffer, lighting, reflection,
level-forward, object-forward and delayed-transparency render passes. Repeated object
or transparency stages are separate passes, with their own timestamp pair.
CPU stages cover shadow preparation/encoding, object overlays,
post-processing and pixel copies too. Unsupported adapters still report CPU durations.
The existing `THEDAS_BENCH_FRAMES` and same-process
ABBA path comparison can collect warmed samples for each renderer.

CPU encoding, submission/wait/pixel unpacking and GPU execution are reported separately.
Per-pass GPU intervals can overlap on pipelined GPUs and can overlap CPU work.
Keep each interval separate when interpreting the frame budget.
The reflection interval is absent when its work is fused into the G-buffer. The GPU
samples cover these passes. Shadows, uploads, ground and post-processing still have
CPU stage timings rather than individual GPU queries.
Profiling adds query resources, timestamp writes and a diagnostic readback per capture.
Use ordinary paired captures with profiling disabled for the relative frame-cost target.

`THEDAS_RENDER_PROFILE_SERIAL=1`, together with profiling, submits and waits between
stages. This prevents an earlier pass's fragments from overlapping a later pass's vertex
timestamp, making the individual lighting and forward intervals easier to interpret.
It also prints `render-profile-serial ... submit_wait_ms=...` for each boundary, including
the shadow, ground and post stages. These wall waits include submission/driver overhead;
they are not GPU durations. CPU stage times exclude these waits. Serialization perturbs
scheduling and adds overhead, so it is a diagnostic budget, not a gameplay or benchmark
mode. Compare ordinary same-process captures to establish an optimization's benefit.

`THEDAS_DEFERRED_TIGHT_TILES=0` retains projected-box light tiles as a comparison. The
default also tests each candidate sphere against the tile's four side planes, with a
one-pixel margin for reconstruction and rounding. This rejects false candidates when
the enclosing box crosses the camera plane; spheres containing the eye still reach all
tiles. Light indices, accumulation order, room/domain filtering and shader equations
are unchanged. The `wrap` view scales both broad and narrow phases to twice the radius.

`THEDAS_DEFERRED_LIGHT_MASKS=0` retains per-light room/domain checks. The default
transposes those memberships into six light words per room and per domain, intersects
them with the tile mask, then iterates the surviving indices in the same ascending
order. Unassigned/overflow rooms and the `no-rooms` view use the unrestricted row.
The 8,256-byte uniform block fits portable limits and is uploaded only when membership
changes.

`THEDAS_DEFERRED_OPAQUE_GBUFFER=0` retains the general alpha-capable G-buffer pipeline
for every draw. The default uses a separate fragment entry point with no reachable
discard for draws without an alpha test. Material sampling and G-buffer outputs are
shared with the cutout pipeline. Cutouts retain authored comparisons, opacity and clip;
faded draws retain forward rendering. Draw order and depth tests are unchanged.

`THEDAS_DEFERRED_SUN_EARLY_EXIT=0` retains every original sun-filter tap for
comparisons. The default omits map reads when the final grazing-angle floor clamps
visibility to one, and omits the four near-map green taps when the centre depth
already selects full visibility. Shadowed near receivers and the far PCF retain
their complete filters, white-border rule, distance fade and grazing floor.
The shared G-buffer, separate reflection and forward lighting use this same
function. The control is read once; the per-renderer
`THEDAS_BENCH_DEFERRED_COMPARE=sun-early-exit` override alternates it in one capture
and restores the configured policy afterwards.

## Order of work

1. Switch plus G-buffer and lighting for static level geometry (this document's first PR).
2. Sun shadows in the lighting pass; terrain into the G-buffer.
3. VFX and spell lights; point light shadow pool.
4. Characters and placeables: the shared G-buffer and skinned vertex path are in place;
   extend the remaining material families and cutout casters next.
5. Lightmaps off: the Static1 probe/SSAO comparison mode is in place; extend coverage and
   visibility before making it a player-facing option. Then TAA and other post work.
