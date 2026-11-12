# Rendering gaps versus the original (rendering stream, 2026-11-12)

Each item: original / ours / evidence / size / dependencies. Sizes: S < 1 day, M 1-3 days, L a week or more.

## Vegetation
1. **SpeedTree geometry.** Original: SpeedTreeRT, statically linked, builds branches, fronds and leaf cards from the .spt parameters at load, draws them with the spt_branch/fronds/leafcards/leafmesh shaders, with LOD fades to billboards. Ours: billboards only (#311, two crossed quads per tree). Evidence: notes/speedtree.md; parser exists (#204). Size L+. Needs the SDK read as a format reference only (no code copied), plus checking the exe's vertex packing.
2. **Grass and bushes** (gra_*, bsh_* .spt). Original: leaf-card clumps. Ours: nothing (no billboard image). Size M once (1) exists, or S as an interim of crossed cards from the leaf texcoords.
3. **Tree wind and leaf motion.** Original: g_amWindMatrices, SpeedWind. Ours: static. Size M. Needs (1).
4. **Tree per-instance colour** (RML 3375), billboard lighting from the normal_billboards atlas, and LOD transitions. Ours: none. Size S-M.

## Lighting and materials
5. **Day exteriors too dark** (Ostagar, alienage). Terrain1 shade = lightmap·lerp(1, shadow, 0.35·g_vCameraPosition.w + 0.15) + ambient c4 + light D. We write camera.w = 1; the exe's value isn't traced. Evidence: qa/run20/camp.png, qa/run22 grades; reference shots coming (reference/original/). Size S-M once shots exist.
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

## Shadows
16. **Sun shadow maps** (PSSM, 2 partitions) exist. Unverified against the original: bias, resolution per detail level, which objects cast. Faded objects still cast shadows (triage tracing the exe's +0x190 read). Size S-M.
17. **No point/spot light shadows.** The original may have none either; verify. Size S to verify.

## Sky, atmosphere, post
18. **Sky scattering and colour** vs the original (Ostagar navy sky vs golden). Needs the reference shots. Size M.
19. **Frame effects**: bloom, HSV matrix and da_fade done. Not done: depth of field (oC1 focal-plane output dropped everywhere), radial/other .ffx effects per level; check each level's effect list. Size M.
20. **Fog**: per-level fog done. Water fog and height fog parameters partly unfilled. Size S.

## Effects / VFX
21. **Particle emitters**: sprite emitters done. Not done: mesh-particle emitters, physics emitters, the VFX0 Chunky/VolTex semantics (partial), emitter target/attraction, wind influence, emitters starting at the effect's start time (they start at scene build), and VFX_base columns beyond ModelName (attachment node, scale, type). Size M-L.
22. **Creature/ability VFX**: effects applied to creatures (spell hits, buffs, auras, weapon trails, blood/gore decals) aren't drawn; only location effects are (#295). Needs sim to expose applied visual effects per creature (it may already). Size L.
23. **Remote material effects** (mml_vRMPSettings: freeze, stone, burning skin overlays) unfilled. Size M.
24. **Gore** (mml_fGoreThreshold, gore maps): unfilled, so no blood on armour. Size S-M.
25. **Decals and projected textures** (blood pools, scorch marks, selection rings). Selection and use rings are a 2D overlay without depth, yellow for hostiles where the original uses red ground decals. Size M.

## Animation presentation
26. **Placeable state animations**: poses and swings done (#286). The exe's playback isn't traced; other controllers (containers, cages, traps, bridges) use the same path but are unchecked. Size S.
27. **Creature animation**: no blending between clips (snaps), variant choice (attack variants, idles) untraced, no facial animation/lipsync for VO, no look-at/head tracking, no IK foot placement. Conversation actors don't sit/animate (QA known). Size L overall.
28. **Cloth/hair physics** (does the exe use PhysX cloth for capes and hair?): none. Size L; verify what the original does.
29. **Orzammar "see-through guard"**: a commoner's tunic and head vanish where she overlaps guard 2 (notes/handoff-render.md). Unresolved. Size S-M.

## Characters
30. **Hair**: chargen hm_cps_p01 dark strands; hair alpha passes approximate. Size S-M.
31. **Skin tone/tint** on NPC hands and shields (residual). Size S.
32. **Morph LOD and head LODs**: always LOD0. Size S.

## Level content
33. **Level-wide layout** now loads (#302). Not drawn: terrain LOD/low-LOD maps (Terrain3), horizon models' LODs, ost000t_v_* "v" meshes (check), the ARL "Umbra" occlusion data. Size M.
34. **Level-layout doors and placeables** (not sim objects) never pose. Size S.

## Performance
35. **Frame time** ~17.5 ms headless in the Wilds; GPU-bound per a CPU profile (CPU mostly idle). Need per-pass GPU timestamps, then instancing for trees and repeated props, culling by room/Umbra, and fewer per-frame constant uploads. Size M.
36. **Area preloading** (Sera's ask). The model loader is now stack-safe for worker threads (#289). Size L.

## Interface overlap (rendering side of UI)
37. **Paperdoll/portrait lighting** and the inventory character view: done with the light rig. Portrait framing per race unchecked. Size S.

The three big unbuilt systems: SpeedTree geometry (1), creature VFX (22) and animation blending/facial (27). Most of the rest are S-M fixes that can run in parallel.
