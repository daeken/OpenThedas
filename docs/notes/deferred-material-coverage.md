# Deferred material inventory

The 2026-11-14 survey uses the fixed Cousland castle, Wilds wolves, Tower of Ishal,
Ostagar arrival and Lothering arrival saves and cameras used for deferred performance
captures. Shader time is 3. The table ranks every forward geometry program observed
in those five scenes before the frond change.

Planned passes include offscreen geometry and inactive tree LODs. Visible instances
pass the faithful renderer's frustum, opacity and LOD checks in the first captured
frame. Instancing can combine several visible instances into one GPU submission.
The counts combine the level and object renderers. Other areas, overlay renderers,
and effects introduced after that frame require further surveys.

| Forward program | Planned passes | Visible instances |
|---|---:|---:|
| SPT_Fronds | 13,885 | 432 |
| SPT_Branch | 3,851 | 94 |
| SPT_LeafMeshes | 3,662 | 53 |
| SPT_LeafCards | 1,516 | 42 |
| Static1 / DAMshOpqe1S | 1,092 | 557 |
| Black / DAMshOpqe1S | 768 | 368 |
| Eye0 / Eyelash0 | 265 | 157 |
| Hair0 / HairAlphaPass2 | 200 | 104 |
| Hair0 / HairAlphaPass3 | 200 | 104 |
| Hair0 / HairAlphaPass4 | 200 | 104 |
| Hair0_Pass1 / HairAlphaPass1 | 200 | 104 |
| Static1_Punch / DAMshPnch1S | 49 | 25 |
| Static1_Alpha / DAMshNorm1SFlat | 38 | 24 |
| Static1_Alpha / DAMshNorm1S | 32 | 14 |
| Prop1 / Prop | 24 | 12 |
| PropAlpha1 / DAMshNorm1S | 19 | 19 |
| Water0 | 18 | 18 |
| Prop1_Punch / PropPunch | 11 | 11 |
| Clouds | 4 | 4 |
| Sky | 4 | 4 |
| Cloth1 / DAMshNorm2S | 3 | 2 |
| SPT_Billboards0 | 3 | 3 |
| SPT_Billboards1 | 3 | 3 |
| Ch1AlphArmTnt / Character_Alpha | 1 | 0 |

Static1, Static1_Punch and Prop1 also have G-buffer draws in this survey. Their
forward rows are cutaway exclusions. Blended materials and all four hair passes
keep their authored colour and ordering. `THEDAS_DEFERRED_LOG_MATERIALS=1` logs
the complete program names, planned forward/G-buffer counts, exclusion reasons,
particle emitter programs, and the first frame's visible geometry counts. The
environment control is read once. Planning counts are emitted once per renderer,
and visible counts once on its first deferred frame.

The same loads contain these forward particle programs. Counts measure planned
emitter passes. Each emitter's live quad count changes during simulation.

| Forward particle program | Emitter passes |
|---|---:|
| VFX0_CS_Addv / VFX0_Addv / DAEmtAddv2S | 208 |
| VFX0_Addv / VFX0_Addv / DAEmtAddv2S | 123 |
| DistortionVFX / DistortionGeom / DistortionMask | 86 |
| VFX0 / VFX0_VolumeTexture / VFXBlendVolTex | 31 |
| DistortionVFX_CS / DistortionGeom / DistortionMask | 10 |
| VFX0_ContactSheet / VFX0 / DAEmtNorm2S | 2 |

Selection and aiming overlays have independent forward renderers. Lava0 and other
shader variants outside these five scenes need additional area captures for counts.

## Tree materials

Roomless, rigid, single-pass `SPT_Fronds`, `SPT_Branch`, `SPT_LeafMeshes` and
`SPT_LeafCards` geometry now shares the G-buffer.
The vertex wrapper calls the existing translated vertex shader. Original wind
matrices, placement scale/yaw, instance slots, camera culling, LOD index ranges and
the D3D9 half-pixel adjustment therefore supply the geometry. Read-only accessors
expose its buffers, bindings and current instance/index ranges.

The fragment material samples the original diffuse texture with its sampler.
Fronds and leaves multiply alpha by `g_fAlphaMultiplier`. The 8-bit Greater comparison reads the
current LOD reference from the same constant slot as the faithful shader. The
existing SPT handling resolves an unnamed/Always state function to Greater. The
rotated vertex normal supplies per-pixel directional and point lighting for fronds.
Leaves use the translated vertex shader's world lighting normal (`TEXCOORD1`),
including the mesh leaf's placement basis. Branches sample `smpNormalMap` at the
translated diffuse UV (`TEXCOORD3`) and transform the signed tangent-space normal
by the three basis rows supplied by that vertex shader (`TEXCOORD0`–`2`). Bark is
opaque, including diffuse texels with zero alpha, as its original state requires.
The RML
`g_vColorMultiplier` supplies ambient light, with zero specular response. The
surface receives the existing sun and point shadows and contributes depth to SSAO.
The shared G-buffer uses the prop material code for these unlightmapped surfaces.

This introduces the deferred renderer's Lambert lighting for these tree families.
The faithful tree materials have their own height-dependent shadow adjustments;
fronds also clamp the final colour. Cutout families retain single-sample binary
coverage at their G-buffer edges under MSAA. Far billboards remain outstanding.

The four tree families also cast into point-light cubes independently of G-buffer
coverage. A separate portable uniform stream runs the same translated vertex shader
with an identity view-projection, preserving wind/placement and the viewing camera's
leaf orientation. Offscreen placements remain eligible, with the same viewing LOD
range and 8-bit cutout reference; faded or inactive placements are absent. Branches
remain opaque. These point casters intentionally include trees whose scene flag
excludes them from faithful sun shadows. Neither that sun path nor its instance
groups/constants change. Exact cache keys include vertex constants, index range,
alpha reference, bounds and immutable geometry/material identities. Wind, LOD,
camera-dependent leaf orientation and removal invalidate affected cubes.
`THEDAS_DEFERRED_TREE_SHADOWS=0` disables planning; it is read once. A renderer-local
`THEDAS_BENCH_DEFERRED_COMPARE=tree-shadows` control compares casting within one load.

Room-assigned, skinned, multipass or incompatible tree pipelines retain forward
rendering. Any runtime-faded tree part sends the renderer's complete family through
the faithful path that frame, preserving its shared instance groups and blended
ordering. Other families keep their planned path. `THEDAS_DEFERRED_FRONDS=0`,
`THEDAS_DEFERRED_BRANCHES=0`, `THEDAS_DEFERRED_MESH_LEAVES=0`, and
`THEDAS_DEFERRED_LEAF_CARDS=0` each keep that family forward from load time. These
environment controls are read once.
`THEDAS_BENCH_DEFERRED_COMPARE=fronds` alternates the planned frond set between
the two paths with a per-renderer override. Benchmark setup must enable frond
planning. The override is restored after capture.
`THEDAS_BENCH_DEFERRED_COMPARE=trees` alternates branches, mesh leaves and leaf cards
together while retaining deferred fronds. It also uses a renderer-local override
and restores the planned mode after capture.

The direct-constant shader wrapper translates through the portable backend checks.
The instanced wrapper retains the faithful tree stream's storage-buffer dependency.
Its backend checks cover HLSL and SPIR-V, with the same older OpenGL limitation as
that stream. Synthetic tests cover both layouts and sampled GPU alpha boundaries,
including opacity multipliers and changes to the LOD reference.
Sampled GPU material tests also check branch normal-map basis orientation and
opaque bark at zero texture alpha, plus both leaf families' exact cutout boundary.

The continued survey moves all 9,029 planned branch/leaf passes (189 visible
instances) into deferred rendering. Planned G-buffer coverage is now 31,226 of
34,360 geometry passes, with 3,134 remaining forward passes. The next programs by
planned count are cutaway Static1 (1,092), Black (768), eyelash Eye0 (265) and the
four hair passes (200 each). The inventory above retains the original baseline
counts for comparison. These counts do not measure GPU submissions after instancing.
