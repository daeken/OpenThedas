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

## Fronds

Roomless, rigid, single-pass `SPT_Fronds/SPT_Fronds` geometry now shares the G-buffer.
The vertex wrapper calls the existing translated vertex shader. Original wind
matrices, placement scale/yaw, instance slots, camera culling, LOD index ranges and
the D3D9 half-pixel adjustment therefore supply the geometry. Read-only accessors
expose its buffers, bindings and current instance/index ranges. The faithful draw
implementation continues to use its existing entry point.

The fragment material samples the original diffuse texture with its sampler and
multiplies alpha by `g_fAlphaMultiplier`. The 8-bit Greater comparison reads the
current LOD reference from the same constant slot as the faithful shader. The
existing SPT handling resolves an unnamed/Always state function to Greater. The
rotated vertex normal supplies per-pixel directional and point lighting. The RML
`g_vColorMultiplier` supplies ambient light, with zero specular response. The
surface receives the existing sun and point shadows and contributes depth to SSAO.
The shared G-buffer uses the prop material code for these unlightmapped surfaces.

This introduces the deferred renderer's Lambert lighting for fronds. The faithful
frond material has its own height-dependent shadow adjustment and final colour
clamp. Fronds retain single-sample binary coverage at their G-buffer edges under
MSAA. Their own point-shadow caster coverage remains to be implemented. Branches,
leaves and far billboards remain the next foliage materials in the inventory.

Room-assigned, skinned, multipass or incompatible frond pipelines retain forward
rendering. Any runtime-faded frond sends the renderer's complete frond set through
the faithful path that frame, preserving its shared instance groups and blended
ordering. `THEDAS_DEFERRED_FRONDS=0` keeps this family forward from load time.
`THEDAS_BENCH_DEFERRED_COMPARE=fronds` alternates the planned frond set between
the two paths with a per-renderer override. Benchmark setup must enable frond
planning. The override is restored after capture.

The direct-constant shader wrapper translates through the portable backend checks.
The instanced wrapper retains the faithful tree stream's storage-buffer dependency.
Its backend checks cover HLSL and SPIR-V, with the same older OpenGL limitation as
that stream. Synthetic tests cover both layouts and sampled GPU alpha boundaries,
including opacity multipliers and changes to the LOD reference.
