# Known gaps: rendering

[Back to the index](../KNOWN_GAPS.md)

Everything drawn: world, lighting, characters, animation, vegetation, effects, post-processing, shaders and the formats behind them.

## Part 1: the owning stream's list

Written by the stream that works on this area. Entries marked traced were read from the original executable; trace notes live outside the repository with the test data.

Each item: original / ours / evidence / size / dependencies. Sizes: S < 1 day, M 1-3 days, L a week or more.

### Vegetation
1. **SpeedTree geometry.** Original: SpeedTreeRT, statically linked, builds branches, fronds and leaf cards from the .spt parameters at load, draws them with the spt_branch/fronds/leafcards/leafmesh shaders, with LOD fades to billboards. Ours: billboards only (#311, two crossed quads per tree). Evidence: notes/speedtree.md; parser exists (#204). Size L+. Needs the SDK read as a format reference only (no code copied), plus checking the exe's vertex packing.
2. **Grass and bushes** (gra_*, bsh_* .spt). Original: leaf-card clumps. Ours: nothing (no billboard image). Size M once (1) exists, or S as an interim of crossed cards from the leaf texcoords.
3. **Tree wind and leaf motion.** Original: g_amWindMatrices, SpeedWind. Ours: static. Size M. Needs (1).
4. **Tree per-instance colour** (RML 3375), billboard lighting from the normal_billboards atlas, and LOD transitions. Ours: none. Size S-M.

### Lighting and materials
5. **Day exterior terrain too dark and green** (Ostagar, alienage). Against the original (reference/original/alienage_load.png vs our ce_01_alienage.json at 1024x768, High detail, the same view), static meshes (buildings, stairs, fences) match closely; the terrain (cobbles, dirt mound: Terrain2/Terrain1) is the outlier. The sunlit mound averages (107, 77, 52) in the original and (75, 63, 41) in ours; the near cobbles average (62, 50, 33) and (56, 57, 34). Ours is darker and green-shifted. Ruled out: fog (density 0.008, negligible near), frame effects (bloom, identity HSV), g_vCameraPosition.w (0 or 1 makes no visible difference), missing lightmaps/masks (all bound), sRGB decoding. Suspects: Terrain1's palette tile selection (MaskV one-hot thresholds with point sampling; the texldd gradients), the sun term on terrain, or ambient c4/light D in Terrain2's COLOR1. Ostagar's ground is the same terrain path. Size M. Depends: none.
6. **Weapon and shield tints.** HN shield wood near black: T3_SHD_HRN ch0 colour (0.10, 0.08, 0.08) times the mask. Original: grained wood, gold rim (wine cg7). Weapon tint field order or brightening untraced. Size S-M (exe trace).
7. **Armour rim/specular too hot** (Dwarf Noble, Dalish), shield specular. QA medium. Size M (specular/env terms per shader).
8. **Dalish clearing lit sepia** instead of green; Dalish hunters' faces black against bright sky. Likely light/probe selection for that level. Size M.
9. **Environment cubemaps**: now the probe .dds (#302), but the binding is inferred, not traced. Size S to verify.
10. **Light rules.** Light groups (ARL 3800) not mirrored; object-carried lights reach every room; level geometry keeps an older light-pick rule (the traced rule lost the Cousland sun patches). Evidence: notes/character-lights.md. Size M.
11. **Sun occlusion visibility** per character (c3 · v eased towards 0.1 when occluded) is taken as 1. Size S-M.
12. **Conversation/cinematic lighting rigs**: speakers lack the original's warm key light. Unknown mechanism (probably cutscene light rigs). Size M.
13. **Water**: Water0 draws (#302), but WaterFogParameters is unfilled; refraction and reflection are only the probe cube; ripple animation unchecked. Size S-M.
14. **Missing texture fallback** = parameter default (#300); provisional, the exe's fallback isn't traced. Size S.
15. **Stand-in draws**: any material outside the original-shader filter draws with the stand-in. The renderer's stats list the programs; each stand-in draw is a gap. Size S per shader.

### Shadows
16. **Sun shadow maps** (PSSM, 2 partitions) exist. Unverified against the original: bias, resolution per detail level, which objects cast. Faded objects still cast shadows (triage tracing the exe's +0x190 read). Size S-M.
17. **No point/spot light shadows.** The original may have none either; verify. Size S to verify.

### Sky, atmosphere, post
18. **Sky scattering and colour** vs the original (Ostagar navy sky vs golden). Needs the reference shots. Size M.
19. **Frame effects**: bloom, HSV matrix and da_fade done. Not done: depth of field (oC1 focal-plane output dropped everywhere), radial/other .ffx effects per level; check each level's effect list. Size M.
20. **Fog**: per-level fog done. Water fog and height fog parameters partly unfilled. Size S.

### Effects / VFX
21. **Particle emitters**: sprite emitters done. Not done: mesh-particle emitters, physics emitters, the VFX0 Chunky/VolTex semantics (partial), emitter target/attraction, wind influence, emitters starting at the effect's start time (they start at scene build), and VFX_base columns beyond ModelName (attachment node, scale, type). Size M-L.
22. **Creature/ability VFX**: effects applied to creatures (spell hits, buffs, auras, weapon trails, blood/gore decals) aren't drawn; only location effects are (#295). Needs sim to expose applied visual effects per creature (it may already). Size L.
23. **Remote material effects** (mml_vRMPSettings: freeze, stone, burning skin overlays) unfilled. Size M.
24. **Gore** (mml_fGoreThreshold, gore maps): unfilled, so no blood on armour. Size S-M.
25. **Decals and projected textures** (blood pools, scorch marks, selection rings). Selection and use rings are a 2D overlay without depth, yellow for hostiles where the original uses red ground decals. Size M.

### Animation presentation
26. **Placeable state animations**: poses and swings done (#286). The exe's playback isn't traced; other controllers (containers, cages, traps, bridges) use the same path but are unchecked. Size S.
27. **Creature animation**: no blending between clips (snaps), variant choice (attack variants, idles) untraced, no facial animation/lipsync for VO, no look-at/head tracking, no IK foot placement. Conversation actors don't sit/animate (QA known). Size L overall.
28. **Cloth/hair physics** (does the exe use PhysX cloth for capes and hair?): none. Size L; verify what the original does.
29. **Orzammar "see-through guard"**: a commoner's tunic and head vanish where she overlaps guard 2 (notes/handoff-render.md). Unresolved. Size S-M.

### Characters
30. **Hair**: chargen hm_cps_p01 dark strands; hair alpha passes approximate. Size S-M.
31. **Skin tone/tint** on NPC hands and shields (residual). Size S.
32. **Morph LOD and head LODs**: always LOD0. Size S.

### Level content
33. **Level-wide layout** now loads (#302). Not drawn: terrain LOD/low-LOD maps (Terrain3), horizon models' LODs, ost000t_v_* "v" meshes (check), the ARL "Umbra" occlusion data. Size M.
34. **Level-layout doors and placeables** (not sim objects) never pose. Size S.

### Performance
35. **Frame time** ~17.5 ms headless in the Wilds; GPU-bound per a CPU profile (CPU mostly idle). Need per-pass GPU timestamps, then instancing for trees and repeated props, culling by room/Umbra, and fewer per-frame constant uploads. Size M.
36. **Area preloading** (requested by the maintainer). The model loader is now stack-safe for worker threads (#289). Size L.

### Interface overlap (rendering side of UI)
37. **Paperdoll/portrait lighting** and the inventory character view: done with the light rig. Portrait framing per race unchecked. Size S.

The three big unbuilt systems: SpeedTree geometry (1), creature VFX (22) and animation blending/facial (27). Most of the rest are S-M fixes that can run in parallel.

## Part 2: code inventory

Found by reading the code at main on 2026-11-12: provisional markers, stubs, ignored inputs and features with no code. "Original: not traced" means nobody has yet checked what the original does. Line numbers drift; search for the quoted names if a line has moved.

Scope: `crates/thedas-render`, `crates/thedas-shader`, the rendering-facing readers in `crates/thedas-formats`, and the parts of `crates/thedas-app` (plus the presentation no-ops in `thedas-sim` and `thedas-ui`) that feed the renderer. Line numbers are against `main` at the time of writing.

### World and terrain

- **RND-001** **Terrain sectors always load their most detailed layout.** Original: not traced. Ours: when a room has no `<room>.rml`, the loader takes `<room>_0.rml`. The other per-sector LOD layouts are never loaded or switched by distance. Evidence: `crates/thedas-render/src/scene.rs:1714`. Size: M. Depends: none. Kind: missing.
- **RND-002** **No mesh LOD selection.** Original: not traced. Ours: every model draws its full mesh at every distance. The renderer has no LOD switching or distance fade code. Evidence: `crates/thedas-render/src/scene.rs:1229`. Size: M. Depends: none. Kind: missing.
- **RND-003** **Occlusion culling (Umbra) is not mirrored.** Original: Umbra visibility per frame (`Camera::setFrustum` 0x466e61, `resolveVisibility` 0x467095). Ours: only a view-frustum sphere test, widened by `VIEW_MARGIN`, with an OpenGL-style near plane. Evidence: `crates/thedas-render/src/d3d_gpu.rs:554`. Size: L. Depends: none. Kind: provisional.
- **RND-004** **Culling bounds are our own spheres.** Original: Umbra `OBBModel` boxes per object (0x466efb, 0x467bcd). Ours: a bounding sphere around each mesh's vertex box. Skinned draws use a sphere carried by every palette matrix. Evidence: `crates/thedas-render/src/d3d_gpu.rs:690`. Size: S. Depends: RND-003. Kind: provisional.
- **RND-005** **Room and model transform rules are inferred from the data.** Original: not traced. Ours: `world = room * T * R * S * node chain`, and model nodes use `parent * T * R`. Both come from the stored bounding spheres. Evidence: `crates/thedas-render/src/scene.rs:1609`, `crates/thedas-formats/src/model.rs:9`. Size: S. Depends: none. Kind: provisional.
- **RND-006** **Field labels for level, room, model and mesh are identified from the data.** Original: not traced. Ours: GFF4 labels and chunk header meanings for ARL/RML/MMH/MSH are guesses checked against the data, not against the exe. Evidence: `crates/thedas-formats/src/level.rs:8`, `crates/thedas-formats/src/mesh.rs:6`, `crates/thedas-formats/src/model.rs:17`. Size: M. Depends: none. Kind: provisional.
- **RND-007** **Only triangle-list mesh chunks load.** Original: not traced. Ours: a chunk whose primitive type is not 0 is logged and skipped. Evidence: `crates/thedas-render/src/scene.rs:611`, `crates/thedas-formats/src/mesh.rs:239`. Size: S. Depends: none. Kind: provisional.
- **RND-008** **An object's room is the smallest box that holds it.** Original: the object's room comes from its `vfunc+0x6c` (not traced). Ours: the smallest room box that holds the position. Light reach and probe choice depend on this room. Evidence: `crates/thedas-render/src/scene.rs:1578`. Size: S. Depends: none. Kind: provisional.
- **RND-009** **A material whose semantic is missing falls back to its first semantic.** Original: the exe's lookup for a missing semantic is not traced. Ours: the material's first semantic is used (sky clouds, lava `Diffuse`). Evidence: `crates/thedas-render/src/scene.rs:816`. Size: S. Depends: none. Kind: provisional.
- **RND-010** **Technique selection is fixed at shader model 3 and its ordering is inferred.** Original: `Logic` blocks are chosen by the device's shader model and detail. Ours: `select(.., 3, detail)` always, with the ordering inferred from `static.mat` and `eye.mat`. The SM2 techniques are never chosen. Evidence: `crates/thedas-formats/src/material.rs:437`, `crates/thedas-render/src/scene.rs:73`. Size: S. Depends: none. Kind: provisional.
- **RND-011** **Water has no engine-made reflection or refraction inputs.** Original: water. Ours: `Water0.vsh` materials are drawn with their translated shaders. A sampler whose usage is not shadow, environment map or lightmap gets its material texture, its default, or black. No reflection or frame-copy target is produced. Evidence: `crates/thedas-render/src/d3d.rs:363`, `crates/thedas-render/src/d3d.rs:1113`. Size: M. Depends: RND-107. Kind: unverified.
- **RND-012** **Rivers, lakes and shore effects have no dedicated handling.** Original: rivers, lakes, shore. Ours: no code found beyond the generic `Water0` material draw. Evidence: `crates/thedas-render/src/d3d.rs:349`. Size: M. Depends: RND-011. Kind: missing.
- **RND-013** **Weather (rain, snow) is not drawn.** Original: weather (rain, snow). Ours: no code found. Evidence: `crates/thedas-render/src/scene.rs:1606`. Size: M. Depends: none. Kind: missing.
- **RND-014** **Script atmosphere changes are ignored.** Original: `SetAtmosphere` and `SetAtmosphereRGB` change the atmosphere at run time. Ours: both are accepted as no-ops, and the atmosphere stays as the layout loaded it. Evidence: `crates/thedas-sim/src/engine.rs:180`. Size: M. Depends: none. Kind: missing.
- **RND-015** **Time-of-day variants are not modelled.** Original: time-of-day and atmosphere variants. Ours: no code found. One atmosphere per level, from its `ATMO` block. Evidence: `crates/thedas-render/src/atmosphere.rs:1`. Size: M. Depends: RND-014. Kind: missing.
- **RND-016** **The atmosphere ignores the tactical camera.** Original: `t` comes from the camera each frame (0 is the tactical view). Ours: `t` is fixed at 1, the game camera, so the fog never uses its tactical intensity. Evidence: `crates/thedas-render/src/atmosphere.rs:46`. Size: S. Depends: none. Kind: provisional.
- **RND-017** **The shader clock never pauses.** Original: the graphics clock stops while its pause flag (`+0xca`) is set. Ours: `ApplicationTime` always runs, and the pause flag is not traced. Evidence: `crates/thedas-render/src/atmosphere.rs:47`. Size: S. Depends: none. Kind: provisional.
- **RND-018** **A celestial body scale of 0 gives a zero projection.** Original: the engine would divide by the scale. Ours: a scale of 0 gives a zero projection. Evidence: `crates/thedas-render/src/atmosphere.rs:113`. Size: S. Depends: none. Kind: provisional.
- **RND-019** **Layout sun normalization is assumed.** Original: the engine uploads the stored sun vector. Ours: the vector is normalized, and a black or directionless sun is dropped. Evidence: `crates/thedas-formats/src/level.rs:261`. Size: S. Depends: none. Kind: provisional.
- **RND-020** **Camera collision uses the drawn triangles.** Original: a 0.25 m PhysX capsule sweep (0x6490b0) against physics shapes (mask 0x1024). Ours: a sweep against the opaque render triangles, both faces. Cut-out and blended draws are not solid. Evidence: `crates/thedas-render/src/collide.rs:7`, `crates/thedas-app/src/live.rs:316`. Size: M. Depends: none. Kind: provisional.
- **RND-021** **Camera handedness is unverified.** Original: not traced. Ours: a right-handed, Z-up view with wgpu depth. It has not been checked against the original. Evidence: `crates/thedas-render/src/camera.rs:74`. Size: S. Depends: none. Kind: unverified.
- **RND-022** **D3D cull winding is unverified.** Original: `CULL_CW` and `CULL_CCW` by screen-space winding. Ours: a constant mapping to wgpu front and back faces, still to be checked against captures. Evidence: `crates/thedas-render/src/d3d_gpu.rs:30`. Size: S. Depends: RND-021. Kind: unverified.
- **RND-023** **Destructible placeables are not drawn.** Original: destructibles. Ours: no code found. Evidence: `crates/thedas-render/src/scene.rs:1847`. Size: M. Depends: none. Kind: missing.
- **RND-024** **Placeable health has no visual effect.** Original: `SetPlaceableHealth` drives a placeable's state. Ours: it is a presentation no-op. Evidence: `crates/thedas-sim/src/engine.rs:190`. Size: S. Depends: RND-023. Kind: missing.

### Lighting and shadows

- **RND-025** **Level geometry lights do not follow the traced rule.** Original: the per-object pass (0x4654e0 → 0x464310 → 0x463e40) uses domains other than 1, the RML 3164 room test, `R − d` ranking and the D/E slots. Ours: level draws keep an older rule: three lights by brightness times cos² attenuation, no room test and no D/E lights. Evidence: `crates/thedas-render/src/d3d.rs:487`. Size: M. Depends: none. Kind: provisional.
- **RND-026** **Level geometry gets a room ambient term.** Original: 0x464310 without the `prop` flag skips the ambient list (0x4644eb). Ours: level geometry gets its room's summed ambient lights, or the average over all rooms, because the Highever hall otherwise draws too dark. Evidence: `crates/thedas-render/src/d3d_gpu.rs:1279`. Size: S. Depends: RND-025. Kind: provisional.
- **RND-027** **Light groups and other light-list tests are not mirrored.** Original: a `CanBeOccluded` light reaches only the groups in ARL 3800. The D/E lists also run a hidden-owner test, and objects can have a second room. Ours: none of these three. Evidence: `crates/thedas-render/src/d3d.rs:472`. Size: M. Depends: none. Kind: provisional.
- **RND-028** **Unknown light `Version` values use version 0's formula.** Original: the exe leaves the last value. Ours: any version other than 1 uses version 0's formula. Evidence: `crates/thedas-render/src/d3d.rs:443`. Size: S. Depends: none. Kind: provisional.
- **RND-029** **Animated and flickering lights are not modelled.** Original: lights scale their influence by an intensity that animates (flicker). Ours: the intensity is always 1, and there is no light animation code. Evidence: `crates/thedas-render/src/d3d.rs:442`. Size: M. Depends: none. Kind: missing.
- **RND-030** **Spot lights are not supported.** Original: spot lights. Ours: no code found. Only `nplt` point lights and `nalt` ambient lights are read. Evidence: `crates/thedas-formats/src/model.rs:49`. Size: M. Depends: none. Kind: missing.
- **RND-031** **Sun occlusion for characters is not traced.** Original: a per-node visibility eases towards 0.1 when the sun is occluded from a character (0x46a520, layout 3148). Ours: the visibility is always 1. Evidence: `crates/thedas-render/src/d3d.rs:49`. Size: M. Depends: none. Kind: provisional.
- **RND-032** **Lights carried by placed objects reach every room.** Original: not traced. Ours: a light carried by a placed object has no room, so it reaches every room. Evidence: `crates/thedas-render/src/scene.rs:234`. Size: S. Depends: RND-008. Kind: provisional.
- **RND-033** **Moving-object light re-pick is approximate.** Original: lights are re-picked per render node every visible frame (0x45bdb0). Ours: one bound for all of an object's meshes. The object keeps the probe it was placed with. Evidence: `crates/thedas-render/src/d3d_gpu.rs:2150`. Size: S. Depends: none. Kind: provisional.
- **RND-034** **Probe choice is partly unknown.** Original: the light manager's probe choice is not known. Ours: the room's own archive, else the level's, else the first probe. Placed objects take the nearest reaching probe, while level draws keep their room's probe. Evidence: `crates/thedas-render/src/scene.rs:1188`, `crates/thedas-render/src/scene.rs:1204`. Size: S. Depends: RND-025. Kind: provisional.
- **RND-035** **A guessed irradiance is used when a level has no probe.** Original: not traced. Ours: a uniform irradiance of 0.35. Evidence: `crates/thedas-render/src/d3d.rs:679`. Size: S. Depends: none. Kind: provisional.
- **RND-036** **The environment cubemap binding is inferred from file names.** Original: the `EnvironmentMap` binding is not traced. Ours: the draw's probe `.mtx` stem plus `.dds`, inferred from the file names. Evidence: `crates/thedas-render/src/d3d.rs:1086`. Size: S. Depends: none. Kind: provisional.
- **RND-037** **`IrradianceParams` is bound empty.** Original: not traced. Ours: the constant gets no values, so its registers read zero. Evidence: `crates/thedas-render/src/d3d.rs:877`. Size: S. Depends: none. Kind: provisional.
- **RND-038** **Shadow box reach towards the sun is guessed.** Original: the slot's 0x6b0 term and the box's starting extent (not traced). Ours: a fixed `SUN_REACH` and our own starting extent. Evidence: `crates/thedas-render/src/shadow.rs:154`. Size: S. Depends: none. Kind: provisional.
- **RND-039** **The shadow light elevation clamp is not applied.** Original: the engine clamps the light's elevation (0x452ac2). Ours: no clamp. Evidence: `crates/thedas-render/src/shadow.rs:31`. Size: S. Depends: none. Kind: provisional.
- **RND-040** **Second shadow partition start and texel bias are approximate.** Original: not traced. Ours: the second partition starts where the first ends, and the bias drops D3D9's half-texel offset. Evidence: `crates/thedas-render/src/shadow.rs:30`. Size: S. Depends: none. Kind: provisional.
- **RND-041** **Shadow partitions are fitted a frame late.** Original: not traced. Ours: the level pass fits the partitions to the object casters as they stood last frame. Evidence: `crates/thedas-render/src/d3d_gpu.rs:212`. Size: S. Depends: none. Kind: provisional.
- **RND-042** **The shadow blur's temporary target format is guessed.** Original: not traced. Ours: the same format as the partition. Evidence: `crates/thedas-render/src/d3d_gpu.rs:320`. Size: S. Depends: none. Kind: provisional.
- **RND-043** **Alpha-tested shadow casters are dropped.** Original: every caster draws its `PSSM` semantic. Ours: a caster whose `PSSM` program samples any texture is skipped, so cut-out foliage, fences and hair cast no sun shadow. Evidence: `crates/thedas-render/src/d3d_gpu.rs:1502`. Size: M. Depends: none. Kind: bug.
- **RND-044** **Fading objects still cast shadows.** Original: fading objects never cast. Ours: caster selection does not check the draw's opacity, so a creature faded by the camera still casts. Evidence: `crates/thedas-render/src/shadow.rs:21`, `crates/thedas-render/src/d3d_gpu.rs:1490`. Size: S. Depends: none. Kind: bug.
- **RND-045** **No shadow fade by object opacity.** Original: shadow fade by opacity. Ours: no code found. Casters draw at full strength whatever the object's opacity. Evidence: `crates/thedas-render/src/d3d_gpu.rs:1880`. Size: S. Depends: RND-044. Kind: missing.
- **RND-046** **Point and spot lights cast no shadows.** Original: point/spot shadows. Ours: no code found. Only the two sun partitions exist. Evidence: `crates/thedas-render/src/shadow.rs:1`. Size: L. Depends: RND-030. Kind: missing.
- **RND-047** **Interiors have no shadows at all.** Original: not traced. Ours: shadow casters and partitions exist only under a sun, so interiors draw with `NO_SHADOW_DATA`. Evidence: `crates/thedas-render/src/gpu.rs:599`. Size: M. Depends: RND-046. Kind: unverified.
- **RND-048** **The base ambient for the fallback detail mode is not used.** Original: 0x458eb0 writes (0.2, 0.2, 0.2) in fallback mode 0. Ours: always zero, because mode 0 is never selected. Evidence: `crates/thedas-render/src/d3d.rs:652`. Size: S. Depends: RND-110. Kind: provisional.
- **RND-049** **Light rigs are placed at the feet.** Original: the paperdoll and chargen rigs are placed by the paperdoll scene (0x7bbb30, 0x7f4037). Ours: the rig is placed at the creature's feet, turned to face the camera. Evidence: `crates/thedas-app/src/live.rs:1387`. Size: S. Depends: none. Kind: provisional.

### Characters

- **RND-050** **Face bone offsets are not applied.** Original: a head morph gives face bones an offset. Ours: `MorphNode::offset` is read but never applied to the skeleton or the meshes. Evidence: `crates/thedas-formats/src/morph.rs:57`. Size: M. Depends: none. Kind: missing.
- **RND-051** **The face morph target blend is inferred.** Original: not traced. Ours: `base + Σ wᵢ·tᵢ`, with each family's neutral target carrying minus the sum of the others. It matches the shipped morphs to about 1 cm at worst. Evidence: `crates/thedas-formats/src/morph.rs:317`. Size: S. Depends: none. Kind: provisional.
- **RND-052** **Face tint slot composition is inferred.** Original: the morph rebuild job (not traced). Ours: the slot-to-entry mapping is read from the presets, and the intensity opacities are left as they are. Evidence: `crates/thedas-formats/src/morph.rs:575`. Size: S. Depends: none. Kind: provisional.
- **RND-053** **The preset base face is picked by name.** Original: not traced. Ours: the base face is named after the preset's first two letters (`hm_pcc_b01.mor`). Evidence: `crates/thedas-render/src/scene.rs:931`. Size: S. Depends: none. Kind: provisional.
- **RND-054** **The body skin tint composition is inferred.** Original: not traced. Ours: the face's skin tint (slot 0) fills the skin entries of `*ArmourSkinTint` materials only. Evidence: `crates/thedas-render/src/scene.rs:1479`. Size: S. Depends: none. Kind: provisional.
- **RND-055** **Tattoos are not put on the face.** Original: tattoos. Ours: the choice is kept on the creature but not drawn. Evidence: `crates/thedas-sim/src/appearance.rs:10`. Size: M. Depends: none. Kind: missing.
- **RND-056** **There is no player toggle to hide helmets.** Original: hide helmet. Ours: a worn helmet always hides the hair and beard (provisional), and no code was found for a player toggle. Evidence: `crates/thedas-ui/src/models.rs:49`, `crates/thedas-render/src/scene.rs:1953`. Size: S. Depends: none. Kind: missing.
- **RND-057** **Cloaks and cloth are not simulated.** Original: cloth simulation. Ours: the `Cloth1.vsh` shader is drawn skinned, and no cloth simulation code was found. Evidence: `crates/thedas-render/src/d3d.rs:355`. Size: L. Depends: none. Kind: missing.
- **RND-058** **Physics bones and ragdolls are not simulated.** Original: physics bones and ragdoll. Ours: no code found. `.phy` shapes are read for picking and camera fade only. Evidence: `crates/thedas-formats/src/phy.rs:1`. Size: L. Depends: none. Kind: missing.
- **RND-059** **Blood, gore and dirt are not drawn.** Original: blood and gore decals, dirt, and the gore level. Ours: no code found. `SetCreatureGoreLevel` is a no-op. Evidence: `crates/thedas-sim/src/engine.rs:200`. Size: M. Depends: none. Kind: missing.
- **RND-060** **Weapon draw and sheathe are not animated.** Original: weapons are drawn and sheathed with `COMMAND_UNSHEATHEWEAPONS` and `COMMAND_SHEATHEWEAPONS` animations. Ours: weapon models jump between their hand and back nodes by combat state. Evidence: `crates/thedas-ui/src/models.rs:566`. Size: M. Depends: RND-064. Kind: provisional.
- **RND-061** **The camera fade uses rest-pose shapes.** Original: the physics scene follows the animated skeleton. Ours: the creature `.phy` shapes are placed by the bones' rest pose. Which shapes the sphere query meets is not traced. Evidence: `crates/thedas-app/src/camfade.rs:24`, `crates/thedas-app/src/camfade.rs:96`. Size: S. Depends: none. Kind: provisional.
- **RND-062** **Faded opaque blending is untraced.** Original: how a faded opaque pass blends is not traced. Ours: a `SrcAlpha`/`InvSrcAlpha` pipeline variant, drawn after the other opaque draws. Evidence: `crates/thedas-render/src/d3d_gpu.rs:1878`, `crates/thedas-render/src/gpu.rs:1291`. Size: S. Depends: none. Kind: provisional.
- **RND-063** **Head and eye location comes from model names.** Original: not traced. Ours: a creature's face is found from `_uem_` or `_uhm_` in the model name, for picking and portraits. Evidence: `crates/thedas-app/src/live.rs:1000`. Size: S. Depends: none. Kind: provisional.

### Animation

- **RND-064** **Animation layers have no weights or crossfade.** Original: the exe's blend layers and weights, and its transitions. Ours: each layer overwrites the bones it has tracks for. A new clip snaps in with no transition blend. Evidence: `crates/thedas-render/src/skeleton.rs:114`. Size: L. Depends: none. Kind: provisional.
- **RND-065** **The translation key convention is unknown.** Original: not traced. Ours: translations apply as offsets from the clip's first key on top of the rest translation, because the root keys lack the hip height. Evidence: `crates/thedas-render/src/skeleton.rs:7`. Size: M. Depends: none. Kind: provisional.
- **RND-066** **Additive animation is not supported.** Original: additive layers. Ours: no code found. Evidence: `crates/thedas-render/src/skeleton.rs:117`. Size: M. Depends: RND-064. Kind: missing.
- **RND-067** **No IK (foot placement, look-at).** Original: IK; `SetLookAtEnabled` exists. Ours: no code found, and `SetLookAtEnabled` is a no-op. Evidence: `crates/thedas-sim/src/engine.rs:195`. Size: L. Depends: none. Kind: missing.
- **RND-068** **Root motion is not used.** Original: root motion. Ours: no code found. Creatures are placed from simulation positions. Evidence: `crates/thedas-render/src/animate.rs:184`. Size: M. Depends: RND-065. Kind: missing.
- **RND-069** **Facial animation morph tracks are skipped.** Original: facial animation. Ours: `.ani` tracks other than `_rotation` and `_translation` (morph weights) are skipped at parse. Evidence: `crates/thedas-formats/src/anim.rs:319`. Size: L. Depends: none. Kind: missing.
- **RND-070** **Lip sync is not drawn.** Original: lip sync. Ours: no code found. Evidence: `crates/thedas-formats/src/anim.rs:319`. Size: L. Depends: RND-069. Kind: missing.
- **RND-071** **Gesture overlays use the plain layer overwrite.** Original: gesture overlays. Ours: a cutscene's head or gesture clip overwrites its bones, with no mask weights. Evidence: `crates/thedas-render/src/skeleton.rs:110`. Size: M. Depends: RND-064. Kind: provisional.
- **RND-072** **Animation tree fields are partly unread.** Original: not traced. Ours: field 4027 is read as play-once (inferred), 4028-4030 are not interpreted, and event kinds are inferred. Evidence: `crates/thedas-formats/src/anim.rs:9`, `crates/thedas-formats/src/anim.rs:136`. Size: S. Depends: none. Kind: provisional.
- **RND-073** **Attack and gesture styles are inferred.** Original: the exe's choice of style is not traced. Ours: the style follows the trees' names, and attacks without a duration use a fixed default. Evidence: `crates/thedas-ui/src/models.rs:651`, `crates/thedas-ui/src/models.rs:675`. Size: S. Depends: none. Kind: provisional.
- **RND-074** **Placeable state animation is limited to rigid nodes.** Original: placeable state animation. Ours: rigid mesh nodes follow the model's animation (a door's leaf). Skinned placeable state changes have no code beyond the shared clip path. Evidence: `crates/thedas-render/src/scene.rs:1902`. Size: S. Depends: none. Kind: unverified.

### Vegetation

- **RND-075** **SpeedTree geometry is not built.** Original: SpeedTreeRT builds the trunks, branches, fronds and leaves at run time, and shows billboards only far away. Ours: every tree is two crossed quads from the level's billboard atlas at all distances, drawn with `prop.mat` `Punch`. Evidence: `crates/thedas-render/src/scene.rs:686`. Size: L. Depends: none. Kind: provisional.
- **RND-076** **Trees without a billboard are not drawn.** Original: grass and bushes are SpeedTrees. Ours: trees with no billboard image are left out, so grass and bushes are missing. Evidence: `crates/thedas-render/src/scene.rs:693`. Size: L. Depends: RND-075. Kind: missing.
- **RND-077** **SpeedTree wind is not applied.** Original: wind. Ours: the `.spt` wind sections are parsed, and no renderer code uses them. Evidence: `crates/thedas-formats/src/speedtree.rs:13`. Size: M. Depends: RND-075. Kind: missing.
- **RND-078** **SpeedTree LOD and leaf clusters are unused.** Original: LOD transitions. Ours: the LOD and leaf-cluster sections are parsed and unused. Evidence: `crates/thedas-formats/src/speedtree.rs:489`. Size: M. Depends: RND-075. Kind: missing.
- **RND-079** **The SpeedTree format layout comes from the SDK.** Original: not traced. Ours: token numbers, nesting and defaults follow the SpeedTree 4 SDK reader. Evidence: `crates/thedas-formats/src/speedtree.rs:18`. Size: S. Depends: none. Kind: provisional.
- **RND-080** **Billboards cast no shadow.** Original: not traced. Ours: billboards are given `cast_shadow: Some(false)`. Evidence: `crates/thedas-render/src/scene.rs:758`. Size: S. Depends: RND-075. Kind: provisional.

### Effects and particles

- **RND-081** **Particle seeds are per emitter.** Original: two global `rand` seeds (0xc31a6c, 0xc31994) shared by all emitters. Ours: each emitter is seeded from its placement. Evidence: `crates/thedas-render/src/particles.rs:35`. Size: S. Depends: none. Kind: provisional.
- **RND-082** **The particle quad size factor is guessed.** Original: the owner's vfunc 0xe0 value (`+0x5c`) is applied twice to the size and once to the bounds. Ours: 1/√2 times the placement scale. Evidence: `crates/thedas-render/src/particles.rs:37`. Size: S. Depends: none. Kind: provisional.
- **RND-083** **Particle orientation modes are approximate.** Original: not traced. Ours: mode 0's camera vectors are taken as down and right, mode 3 uses velocity, and mode 4 draws as mode 0. Evidence: `crates/thedas-render/src/particles.rs:40`. Size: S. Depends: none. Kind: provisional.
- **RND-084** **Spawn volume directions are approximate.** Original: 0x4ee200's Euler order. Ours: an approximation of the sphere direction and of using normals as the initial direction. Evidence: `crates/thedas-render/src/particles.rs:43`. Size: S. Depends: none. Kind: provisional.
- **RND-085** **Several emitter behaviours are not mirrored.** Original: targets and attraction, movement spread, particle inheritance, birth per metre, wind, collisions and physics. Ours: none of these are mirrored. Evidence: `crates/thedas-render/src/particles.rs:45`. Size: L. Depends: none. Kind: missing.
- **RND-086** **Mesh-particle and physics emitters are skipped.** Original: mesh particles and physics emitters. Ours: they are left out at load. Evidence: `crates/thedas-render/src/scene.rs:1427`. Size: M. Depends: none. Kind: missing.
- **RND-087** **Ribbons, trails, path following and splats are not drawn.** Original: ribbons, trails and splats. Ours: `follow_path`, `link_particles`, `splat`, `inherit_velocity` and `update_only_when_visible` are parsed and unused. Evidence: `crates/thedas-formats/src/emitter.rs:185`. Size: M. Depends: none. Kind: missing.
- **RND-088** **Lightning effects are not drawn.** Original: lightning. Ours: no code found. Evidence: `crates/thedas-render/src/particles.rs:1`. Size: M. Depends: RND-087. Kind: missing.
- **RND-089** **`Chunky` and `VolTex` VFX shaders are not drawn.** Original: `VFX.mat` semantics. Ours: only the eight sprite vertex shaders are in the filter. Evidence: `crates/thedas-render/src/d3d.rs:334`. Size: M. Depends: none. Kind: missing.
- **RND-090** **Emitters draw only their material's first pass.** Original: not traced. Ours: an emitter draws only the first pass of its material. Evidence: `crates/thedas-render/src/d3d_gpu.rs:1572`. Size: S. Depends: none. Kind: provisional.
- **RND-091** **Particles are not sorted.** Original: not traced. Ours: particles are drawn last in emitter order with no depth sort, within or across emitters. Evidence: `crates/thedas-render/src/gpu.rs:1327`. Size: M. Depends: none. Kind: unverified.
- **RND-092** **The source of the flipbook grid is untraced.** Original: who fills `mml_vContactSheetParams` is not traced. Ours: the emitter's columns and rows. Evidence: `crates/thedas-render/src/scene.rs:189`. Size: S. Depends: none. Kind: provisional.
- **RND-093** **The route of placement user parameters is untraced.** Original: `UPAR` is stored on the object (0xa29d43). How it reaches the draw is not traced. Ours: it becomes the VFX colour multiplier. Evidence: `crates/thedas-render/src/scene.rs:194`. Size: S. Depends: none. Kind: provisional.
- **RND-094** **Visual effects on creatures and items are not drawn.** Original: spell VFX attachment, crust and shader effects on characters, weapon glows. Ours: `ApplyEffectVisualEffect` is a no-op, and item visual effects are kept but not drawn. Only location VFX models are drawn. Evidence: `crates/thedas-sim/src/engine.rs:179`, `crates/thedas-sim/src/engine.rs:2016`. Size: L. Depends: none. Kind: missing.
- **RND-095** **Instant location VFX are dropped.** Original: not traced. Ours: a location visual effect with an instant duration is not kept, so it is never drawn. Evidence: `crates/thedas-sim/src/engine.rs:2449`. Size: M. Depends: none. Kind: bug.
- **RND-096** **Location VFX rows are only partly read.** Original: `VFX_base.gda` columns (type, attachment, scale). Ours: only `ModelName` is used. Evidence: `crates/thedas-ui/src/models.rs:983`. Size: S. Depends: RND-094. Kind: provisional.
- **RND-097** **Decals and projected textures are not drawn.** Original: decals and projected textures. Ours: no code found. Evidence: `crates/thedas-render/src/scene.rs:1229`. Size: M. Depends: none. Kind: missing.
- **RND-098** **AoE indicators and lasting areas are not drawn.** Original: AoE indicators. Ours: no world-drawn persistent-area visuals. The aiming reticle is a plain outline (see RND-128). Evidence: `crates/thedas-sim/src/engine.rs:262`. Size: M. Depends: RND-094. Kind: missing.
- **RND-099** **The stand-in skips effect materials.** Original: effect materials are drawn. Ours: `vfx*`, distortion and vfx-class materials resolve to no draw under the stand-in. Evidence: `crates/thedas-render/src/material.rs:128`. Size: S. Depends: RND-107. Kind: missing.
- **RND-100** **Particles are capped at 256 per emitter.** Original: 256 alive. Ours: 256 per emitter. The step and count functions are mirrored. Evidence: `crates/thedas-render/src/particles.rs:56`. Size: S. Depends: none. Kind: unverified.

### Post-processing

- **RND-101** **Depth of field (`da_dof`) is not drawn.** Original: `da_dof` (layer 5). Ours: not drawn. The shaders' depth-of-field output `oC1` is dropped and `FocalPlane` is fixed at (1, 0, 0, 0). Evidence: `crates/thedas-render/src/post.rs:7`, `crates/thedas-render/src/d3d.rs:829`. Size: M. Depends: none. Kind: missing.
- **RND-102** **Frame effect order within a layer is untraced.** Original: the effect manager's order (not traced). Ours: a stable sort by layer that keeps the list order. Evidence: `crates/thedas-render/src/post.rs:14`. Size: S. Depends: none. Kind: provisional.
- **RND-103** **The back buffer alpha is assumed opaque.** Original: the back buffer format is not traced. Ours: the frame's alpha is taken as 1 (`X8R8G8B8`). Evidence: `crates/thedas-render/src/post.rs:63`. Size: S. Depends: none. Kind: provisional.
- **RND-104** **The fade's `minlod` mapping is guessed.** Original: not traced. Ours: `minlod` is read as the detail level counted from Low (0). Evidence: `crates/thedas-render/src/post.rs:208`. Size: S. Depends: none. Kind: provisional.
- **RND-105** **A missing fade mask draws white.** Original: `Default_Missing`. Ours: a 1×1 white image. Evidence: `crates/thedas-render/src/post.rs:269`. Size: S. Depends: none. Kind: provisional.
- **RND-106** **Script frame-buffer effects are ignored.** Original: `FB_SetEffectResource` and `FB_SetEffectEnabled` drive frame effects (desaturation at death, scripted fades and colour curves). Ours: both are no-ops, and only the layout's own effect list runs. Evidence: `crates/thedas-sim/src/engine.rs:182`. Size: M. Depends: none. Kind: missing.
- **RND-107** **Distortion, heat haze and refraction are not drawn.** Original: distortion and heat haze. Ours: distortion materials are skipped by the stand-in and are not in the original-shader filter. No frame copy is offered to materials. Evidence: `crates/thedas-render/src/material.rs:128`. Size: M. Depends: RND-011. Kind: missing.
- **RND-108** **The gamma option is not applied.** Original: the Gamma option sets the device gamma ramp (0x53ab10 → `SetGammaRamp`). Ours: no ramp is applied. It is the identity only at the default 0.50, and the option changes nothing. Evidence: `crates/thedas-render/src/post.rs:58`, `crates/thedas-ui/src/options.rs:13`. Size: S. Depends: RND-110. Kind: missing.
- **RND-109** **No anti-aliasing.** Original: anti-aliasing. Ours: every pass uses `sample_count: 1`, and no code was found for MSAA or any other AA. Evidence: `crates/thedas-render/src/gpu.rs:1137`. Size: M. Depends: none. Kind: missing.

### Shaders and materials

- **RND-110** **Video options do nothing.** Original: `VideoOptions` (Graphics Detail, frame-buffer effects, VSync, full screen, texture pack, gamma) take effect. Ours: the options screen keeps them but changes nothing. Detail and frame effects come only from the command line. Evidence: `crates/thedas-ui/src/options.rs:13`, `crates/thedas-app/src/main.rs:187`. Size: M. Depends: none. Kind: missing.
- **RND-111** **The default detail level is guessed.** Original: chosen by hardware detection (not traced). ClientOptions defaults `GraphicsDetailLevel` to 3. Ours: Medium by default. The script call `GetGraphicsDetailLevel` always returns 1. Evidence: `crates/thedas-render/src/scene.rs:250`, `crates/thedas-sim/src/engine.rs:1548`. Size: S. Depends: RND-110. Kind: provisional.
- **RND-112** **Shader families outside the filter use the stand-in.** Original: every material draws its own D3D9 shaders. Ours: by default only Static1, Black, the object, terrain, sky, lava and water vertex shaders and eight VFX sprite shaders are translated. Every other family falls back to the diffuse-times-lightmap stand-in or is skipped. Evidence: `crates/thedas-render/src/d3d.rs:326`, `crates/thedas-app/src/main.rs:290`. Size: L. Depends: none. Kind: missing.
- **RND-113** **The stand-in shader is not the original look.** Original: the original look. Ours: diffuse × lightmap with a guessed lightmap RGB scale (2.0), alpha weight (0.6) and unlit level (0.8). Both faces are drawn and there are no lights. Evidence: `crates/thedas-render/src/standin.wgsl:1`, `crates/thedas-render/src/gpu.rs:404`, `crates/thedas-render/src/material.rs:164`. Size: M. Depends: RND-112. Kind: provisional.
- **RND-114** **Several D3D9 opcodes are not translated.** Original: D3D9 SM2/3. Ours: `texld` in a vertex shader, predication and `setp`, `call`/`callnz`, `ret` inside control flow, relative destinations and some misc registers return `Unsupported`. A shader that uses one falls back to the stand-in. Evidence: `crates/thedas-shader/src/translate.rs:507`, `crates/thedas-shader/src/translate.rs:742`, `crates/thedas-shader/src/translate.rs:845`. Size: M. Depends: none. Kind: missing.
- **RND-115** **Edge-case instruction semantics are guessed.** Original: driver-defined. Ours: `mova` rounds halves away from zero, and `nrm` of a zero vector gives inf/NaN. Evidence: `crates/thedas-shader/src/translate.rs:642`, `crates/thedas-shader/src/translate.rs:713`. Size: S. Depends: none. Kind: provisional.
- **RND-116** **Fixed-function fog and point size are dropped.** Original: D3D9 applies `oFog` and point size. Ours: `oFog` and `oPts` are written to private variables and discarded, and `DS_FOG*` states are not mapped. Evidence: `crates/thedas-shader/src/translate.rs:385`. Size: S. Depends: none. Kind: unverified.
- **RND-117** **Only one colour target is kept.** Original: MRT outputs `oC1+` (depth of field). Ours: `color_targets: Some(1)`, and the other outputs are dropped. Evidence: `crates/thedas-render/src/d3d.rs:144`, `crates/thedas-shader/src/translate.rs:1283`. Size: S. Depends: RND-101. Kind: missing.
- **RND-118** **Several render states are not mapped.** Original: D3D9 render states. Ours: stencil, depth bias and slope scale, separate alpha blend, blend factor constant, fill mode and alpha-to-coverage are not mapped. Alpha blends with the colour factors. Evidence: `crates/thedas-render/src/d3d_gpu.rs:1119`, `crates/thedas-render/src/d3d_gpu.rs:1167`. Size: M. Depends: none. Kind: missing.
- **RND-119** **sRGB states are parsed and unused.** Original: `DS_SRGBWRITEENABLE` and `SAMP_SRGBTEXTURE`. Ours: both are parsed, and the pipelines ignore them. Evidence: `crates/thedas-formats/src/state.rs:372`, `crates/thedas-formats/src/state.rs:408`. Size: S. Depends: none. Kind: missing.
- **RND-120** **Anisotropic filtering is not honoured on the original-shader path.** Original: `TEXFILT_ANISOTROPIC`. Ours: it maps to linear with no `anisotropy_clamp`. The stand-in uses 8×. Mip LOD bias and max mip level are not mapped. Evidence: `crates/thedas-render/src/d3d_gpu.rs:898`, `crates/thedas-render/src/gpu.rs:650`. Size: S. Depends: none. Kind: missing.
- **RND-121** **Depth "equal" is relaxed to "less or equal".** Original: `CMP_EQUAL`. Ours: `LessEqual`, because separately translated passes may not reach bit-identical depths. Evidence: `crates/thedas-render/src/d3d_gpu.rs:1156`. Size: S. Depends: none. Kind: provisional.
- **RND-122** **Border colours are approximated.** Original: any `SAMP_BORDERCOLOR`. Ours: the nearest of transparent black, opaque black or opaque white. Without the device feature, clamp to edge. Evidence: `crates/thedas-render/src/d3d_gpu.rs:883`. Size: S. Depends: none. Kind: provisional.
- **RND-123** **State-file defaults and loading are untraced.** Original: the engine's state loader (not traced). Ours: D3D9 defaults where a file is silent, stage numbers read as pixel sampler registers, and an unknown blend factor keeps the default. Evidence: `crates/thedas-formats/src/state.rs:18`, `crates/thedas-formats/src/state.rs:310`. Size: S. Depends: none. Kind: provisional.
- **RND-124** **Matrix register layouts are inferred.** Original: not traced. Ours: `*Trans` usages are written as rows and plain usages as the transpose. `MatrixRows`/`MatrixColumns` are read from Static1's bytecode, and `bsm_bones` as 3x4 rows. Evidence: `crates/thedas-render/src/d3d.rs:43`, `crates/thedas-render/src/d3d.rs:808`, `crates/thedas-render/src/d3d.rs:850`. Size: S. Depends: none. Kind: provisional.
- **RND-125** **The missing-texture fallback is untraced.** Original: the exe's fallback for a texture that fails to load (not traced). Ours: the parameter's default (for example `Default_Missing`), else black. A sampler with no matching parameter reads black. Evidence: `crates/thedas-render/src/d3d.rs:1033`, `crates/thedas-render/src/d3d_gpu.rs:1219`. Size: S. Depends: none. Kind: provisional.
- **RND-126** **Material `Matrix4x4f` layout is guessed.** Original: not traced. Ours: four registers, one row each, in the order written. Evidence: `crates/thedas-formats/src/material.rs:196`. Size: S. Depends: none. Kind: provisional.
- **RND-127** **Transparent draws are not depth-sorted.** Original: not traced. Ours: blended and multi-pass draws keep scene order, with no back-to-front sort. Evidence: `crates/thedas-render/src/gpu.rs:868`. Size: M. Depends: none. Kind: unverified.

### In-world UI

- **RND-128** **The aiming reticle is a plain outline.** Original: the game's own reticle art. Ours: a plain coloured outline (circle, cone or ring). Evidence: `crates/thedas-app/src/reticle.rs:4`. Size: S. Depends: none. Kind: provisional.
- **RND-129** **No selection circles or target rings under creatures.** Original: selection circles and target rings. Ours: no code found. Evidence: `crates/thedas-app/src/reticle.rs:1`. Size: M. Depends: none. Kind: missing.
- **RND-130** **Floating text placement is inferred.** Original: not traced. Ours: floatylayer.gfx message floaties are driven, with the head height, linger time and stacking inferred. Evidence: `crates/thedas-app/src/ui.rs:1243`. Size: S. Depends: none. Kind: provisional.
- **RND-131** **The minimap shows every creature.** Original: not traced. Ours: every living creature in the area is shown within the map radius. Evidence: `crates/thedas-app/src/ui.rs:1241`. Size: S. Depends: none. Kind: provisional.
- **RND-132** **There is no area-map fog of war.** Original: area-map fog of war (`mapmask`, made by the engine). Ours: the mask is clear, so the whole map shows as explored. Exploration is not tracked. Evidence: `crates/thedas-app/src/ui.rs:408`. Size: M. Depends: none. Kind: missing.
- **RND-133** **Portraits are not lit by their own rig.** Original: `portraits.gda` light columns give portraits their own rig. Ours: the scene's light, brightened by a guessed `PORTRAIT_GAIN` (1.8). The camera orbit uses only some columns. Evidence: `crates/thedas-app/src/live.rs:1049`, `crates/thedas-app/src/live.rs:1279`. Size: S. Depends: none. Kind: provisional.
- **RND-134** **Paperdoll camera presets are unused.** Original: `GUIPaperdoll` camera presets in its own scene. Ours: a level camera framing the member's height, with coverage from drawing over black and over white. Evidence: `crates/thedas-app/src/live.rs:1426`. Size: S. Depends: none. Kind: provisional.
- **RND-135** **The chargen camera blend and field of view are guessed.** Original: interpolation 0x5b0db0 (not traced). Ours: a linear blend, with the field of view taken as vertical. Evidence: `crates/thedas-app/src/chargen_stage.rs:23`, `crates/thedas-app/src/chargen_stage.rs:142`. Size: S. Depends: none. Kind: provisional.
- **RND-136** **Creature picking uses guessed cylinders.** Original: not traced. Ours: creatures are picked as upright cylinders 1.9 m tall. Evidence: `crates/thedas-app/src/live.rs:1059`. Size: S. Depends: none. Kind: provisional.

### Formats

- **RND-137** **DXT2 and DXT4 are treated as unpremultiplied.** Original: DXT2 and DXT4 are premultiplied. Ours: they are read as DXT3 and DXT5 with no un-premultiply. Evidence: `crates/thedas-formats/src/dds.rs:125`. Size: S. Depends: none. Kind: provisional.
- **RND-138** **Some DDS layouts are rejected.** Original: D3D9 texture formats. Ours: any FourCC other than DXT1-5 and any RGB layout other than 8888, X888, 888, 4444 or L8 is rejected (for example R5G6B5, A1R5G5B5, A8L8, V8U8 and float formats). Evidence: `crates/thedas-formats/src/dds.rs:127`. Size: S. Depends: none. Kind: unverified.
- **RND-139** **Small compressed mips are dropped.** Original: the full mip chain. Ours: a compressed texture's chain stops at the first level that is not a whole number of 4×4 blocks. Evidence: `crates/thedas-render/src/gpu.rs:161`. Size: S. Depends: none. Kind: bug.
- **RND-140** **Texture type mismatches draw a solid colour.** Original: not traced. Ours: a texture whose kind (2D, cube, volume) does not match the sampler's is replaced by a solid colour. Evidence: `crates/thedas-render/src/gpu.rs:278`. Size: S. Depends: none. Kind: provisional.
- **RND-141** **The probe's spherical-harmonic data is ignored.** Original: not traced. Ours: `.mtx` lines after the twelve matrix rows are ignored. Evidence: `crates/thedas-formats/src/probe.rs:6`. Size: S. Depends: none. Kind: unverified.
- **RND-142** **The `.phy` capsule axis is inferred.** Original: not traced. Ours: the axis and origin are read from the models' bounding boxes. Evidence: `crates/thedas-formats/src/phy.rs:17`. Size: S. Depends: none. Kind: provisional.
- **RND-143** **Frame effect labels are inferred.** Original: not traced. Ours: 3026, 5520, 5521, 5522-5524 are identified from the data. Entries with no id or a non-numeric value are skipped. Evidence: `crates/thedas-formats/src/level.rs:231`. Size: S. Depends: none. Kind: provisional.
- **RND-144** **Some atmosphere fields are unread.** Original: the loader (0x5d00ce) does not read 22525 or 22527. Ours: they are not read either, and `g` keeps preset 0's −0.5. Evidence: `crates/thedas-formats/src/level.rs:189`, `crates/thedas-render/src/atmosphere.rs:23`. Size: S. Depends: none. Kind: unverified.
- **RND-145** **Morph and stage fields are read from shipped files.** Original: not traced. Ours: the morph model-list slots and the stage camera fields are named from the data or from xoreos. Evidence: `crates/thedas-formats/src/morph.rs:43`, `crates/thedas-formats/src/stage.rs:21`. Size: S. Depends: none. Kind: provisional.
- **RND-146** **Bink movies depend on an external ffmpeg.** Original: Bink movies with audio. Ours: an `ffmpeg` on the PATH decodes the frames. Without it the movies are skipped. They play silent, and `DisableIntroMovies` is not read. Evidence: `crates/thedas-app/src/intro.rs:5`. Size: L. Depends: none. Kind: provisional.
- **RND-147** **The `.are` layout field meaning is inferred.** Original: not traced. Ours: the `Layout` field is read as the env layout name. Evidence: `crates/thedas-formats/src/area.rs:5`. Size: S. Depends: none. Kind: provisional.

### Platform and performance

- **RND-148** **No texture streaming or memory budget.** Original: texture streaming and a memory budget. Ours: no code found. The texture, mesh and pipeline caches only grow until the resource stack changes. Evidence: `crates/thedas-render/src/gpu.rs:238`, `crates/thedas-render/src/gpu.rs:429`. Size: M. Depends: none. Kind: missing.
- **RND-149** **The texture pack option is ignored.** Original: the `TexturePack` option (texture detail). Ours: it is ignored. Evidence: `crates/thedas-ui/src/options.rs:58`. Size: S. Depends: RND-110. Kind: missing.
- **RND-150** **Window mode does not follow the options.** Original: `FullScreen` defaults to 1, and `ScreenResIndex` picks the resolution. Ours: a window of the command-line logical size. No fullscreen code was found, and the resolution list is unused. Evidence: `crates/thedas-app/src/main.rs:788`, `crates/thedas-ui/src/options.rs:54`. Size: M. Depends: RND-110. Kind: missing.
- **RND-151** **VSync is always on.** Original: the `UseVSync` option defaults to 0. Ours: `AutoVsync` always, with no frame limiter or pacing control. Evidence: `crates/thedas-app/src/main.rs:804`. Size: S. Depends: RND-110. Kind: missing.
- **RND-152** **No GPU timing instrumentation.** Original: not traced. Ours: CPU-side counts in `RenderStats` only. Every pass sets `timestamp_writes: None`, and no frame-time profiler was found. Evidence: `crates/thedas-render/src/gpu.rs:1287`, `crates/thedas-render/src/gpu.rs:465`. Size: S. Depends: none. Kind: missing.
- **RND-153** **Constants are rebuilt per draw.** Original: not traced. Ours: one constant slot per draw, with per-frame constants refilled for every draw in view. There is no instancing for repeated meshes. Evidence: `crates/thedas-render/src/d3d_gpu.rs:2008`. Size: M. Depends: none. Kind: unverified.
- **RND-154** **DA2 rendering formats are not supported.** Original: DA2 rendering formats. Ours: no code found in the renderer. Only the ERF V3.0 container is recognised in `thedas-vfs`. Evidence: `crates/thedas-vfs/src/erf.rs:8`. Size: L. Depends: none. Kind: missing.

### Checked and looks complete

- Static1 level geometry with lightmap atlases and the translated original shaders (`crates/thedas-render/src/d3d.rs:304`).
- Terrain drawn with `Terrain2`/`Terrain3` by detail (`crates/thedas-render/src/d3d.rs:358`).
- The sky dome and celestial body projection, and the cloud layer's registers (`crates/thedas-render/src/atmosphere.rs:90`).
- Fog registers from the level's `ATMO` (`crates/thedas-render/src/atmosphere.rs:59`).
- Point-light selection for characters and placeables (`pick_lights`, influence, room reach) (`crates/thedas-render/src/d3d.rs:475`).
- Irradiance probes read and bound as `g_mLightParams` (`crates/thedas-formats/src/probe.rs:1`).
- The sun as a directional light from the layout (3122/3150/3152/3149) (`crates/thedas-formats/src/level.rs:249`).
- Sun shadow partitions: sizes, reaches, light view, box smoothing and the variance blur at Medium and High (`crates/thedas-render/src/shadow.rs:1`).
- CPU skinning palettes and GPU skinning through `bsm_bones` (`crates/thedas-render/src/skeleton.rs:211`).
- Packed quaternion decoding of `.ani` rotations (`crates/thedas-formats/src/anim.rs:224`).
- Equipment tints into `mml_vTintMaskColours` in the exe's order (`crates/thedas-formats/src/tint.rs:14`).
- Head morph vertex replacement, material overrides and hair and beard hiding under a helmet (`crates/thedas-render/src/scene.rs:1322`).
- Attachments hung from bone nodes (weapons, shields) (`crates/thedas-render/src/scene.rs:1921`).
- Sprite particle stepping, birth, motion, spin, age map and quad building (`crates/thedas-render/src/particles.rs:6`).
- Bloom (`da_bloom`) with the 7-tap and 5-tap blurs (`crates/thedas-render/src/post.rs:29`).
- Colour grading (`da_hsvmatrix` with the `ColorCurve` ramp) (`crates/thedas-render/src/post.rs:19`).
- Radial blur and distortion of `da_fade` (`crates/thedas-render/src/post.rs:46`).
- The camera fade of creatures the camera is inside (rate, radius, target 0.05) (`crates/thedas-app/src/camfade.rs:1`).
- Uncompressed and BC1-3 DDS for 2D, cube and volume textures (`crates/thedas-formats/src/dds.rs:1`).
- Offscreen capture and the control channel's screenshot (`crates/thedas-render/src/gpu.rs:1361`, `crates/thedas-app/src/main.rs:1797`).
- Window resize and lost-surface reconfigure (`crates/thedas-app/src/main.rs:2012`).
- Backend choice via wgpu from the environment, any of Vulkan, Metal, DX12 or GL (`crates/thedas-render/src/gpu.rs:86`).
- The minimap picture and creature icons fed to minimap.gfx and map.gfx (`crates/thedas-app/src/ui.rs:1472`).
- Party portraits and paperdoll rendered offscreen (`crates/thedas-app/src/live.rs:1282`).
