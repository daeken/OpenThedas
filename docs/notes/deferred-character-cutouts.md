# Deferred character cutouts

Origins' Character material has eight Punchthrough semantics, pairing Char1 with Ch1Alph
and its NoTint, MulTint, ArmourSkinTint and emissive variants. CharacterPT enables depth
testing/writing and alpha testing without colour blending. Its shipped alpha comparison is
greater-or-equal 128.

The pixel shaders obtain coverage from diffuse alpha multiplied by the authored opacity
multiplier. This family has no intrinsic shader clip. Its colour, tint, normal, probe and
emissive inputs correspond to the existing Char1 equations represented by our deferred
materials. The optional renderer reuses those equations and the exact sampled comparison
used by its earlier cutouts. The material's diffuse sampler retains its addressing,
filtering, mip and border behavior.

Admission requires a supported single pass, full colour writes, depth testing/writing,
alpha testing and no blending. Missing opacity maps and unsupported geometry retain the
forward path. Dynamic fades are evaluated each frame. Posed point shadows use the same
diffuse coverage, comparison, multiplier, palette and bounds through the independent
cutout-caster plan. The ordinary opaque-caster path excludes these cards.

`THEDAS_DEFERRED_CHARACTER_CUTOUTS=0` disables the new G-buffer and point-shadow coverage.
`THEDAS_DEFERRED_PUNCH=0` independently keeps G-buffer cutouts forward. The existing
objects, cutout-shadow and point-shadow controls retain their behavior.

Asset-free checks cover the recognized family, rejection of blended colour, sampled
8-bit comparisons with the absent shader clip, pose transforms, tint composition and
cube-map invalidation. Shader translation continues to cover Metal, HLSL and GLSL.
