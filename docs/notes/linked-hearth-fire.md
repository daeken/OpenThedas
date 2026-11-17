# Linked hearth fire (#1097)

Report 1097, build 81af4d72, reproduces from `save_now.json` in Beraht's
Shop (`bdc110ar_berahts_shop`, layout `orz102d`). This is a particle topology
error, not a missing placeable attachment or a Z-up conversion.

## Placement and offset chain

The marked wall torch has two independent sibling placements in
`orz102d_0.rml`; the VFX is not attached to a placeable or model hook.
Positions below are metres, Z up, read from the installed resources and
checked against the report's presentation snapshot.

| Term | Value |
| --- | --- |
| Room translation | (-0.988619, -13.640385, 2.105620), identity rotation |
| Torch model | `prp_dw_torch01_0`, placement 311 / `orz102d_8n_0` |
| Torch room-local translation | (-3.002281, 20.147892, -0.781588), scale 1 |
| Torch world origin | (-3.990900, 6.507507, 1.324031) |
| VFX model | `fxe_fire_small_p`, placement 505 / `orz102d_e1_0` |
| VFX room-local translation | (-2.773438, 20.080770, 0.298744), scale 1.5 |
| VFX world origin | (-3.762057, 6.440386, 2.404364) |
| Model chain | `GOB` -> `fire_dummy` -> `Flame`; both parent transforms identity |
| Emitter local translation | (0, 0, -0.026781), identity rotation |
| Emitter world origin | (-3.762057, 6.440386, 2.364192) |

The VFX placement rotates about Z, leaving its emission axis upright. Thus
`2.105620 + 0.298744 + 1.5 * -0.026781 = 2.364192`. There is no additional
hook term and no Y/Z swizzle. Other siblings are `Glow`, `FireMeat_4`, and
`Distortion_8`. `Flame` uses `fx_firetorchfb_add`, a 4-row, 8-column sheet,
orientation 3, and **LinkParticlesTogether = true** (field 6033).

## Cause and executable trace

The parser already retained that flag, but `EmitterState::quads` ignored it.
It generated an independent, centered sprite for every flame particle. At
scale 1.5 its provisional ordinary-sprite half-height was approximately
`0.5 * 1.5^2 * 1 = 1.125 m`, extending a newborn flame below the bowl.

DAOrigins.exe uses `ObjectNodeEmitterSpriteLinked<SpriteParticle>` (vtable
0xafadac), with a distinct normal rendering path at **0x5850e0**. It forms
segments through particles in birth order, averages adjacent transverse
axes (**0x58559b–0x585625**), and terminates the final segment at the emitter
position (**0x585704–0x58574e**). There is no longitudinal `P ± A` sprite
extent. Its width comes from the first particle's Y age-map size
(**0x5852c9–0x585302**), without the ordinary sprite's owner-scale-squared
factor. Its flipbook clock is shared by the ribbon (**0x5851bf–0x585203**).
Expiry preserves chain order (**0x584c7d–0x584cab**).

The missing term was the terminal emitter endpoint; the extra term was the
ordinary sprite's longitudinal half-size. Moving the whole VFX upward would
hide that error while breaking its authored origin.

## Change and validation

Linked emitters now generate that ribbon topology and shared phase, preserving
birth order when particles expire. Ordinary sprites keep their existing path.
A synthetic regression covers a translated/scaled emitter, a deliberately
large longitudinal key, continuous joints, UV endpoints, and ordered expiry.

Captures under `codex-captures/conv-gradient-hearth-fire/` compare the same
camera and clock: `1097-before.png` / `1097-linked.png`; another shop torch
(`shop-other-*`), Castle Cousland's hearth (`castle-hearth-*`) and wall torch
(`castle-torch-*`). The marked fire's detached flame below the bracket is
removed. The other fixtures are checked visually for attachment, with
three-panel comparisons including amplified pixel deltas staged for review.

Trace limits: this was static executable analysis and replacement-engine
captures, not an original-game run. Linked UV subdivision/chain breaks and
spinning side axes remain provisional/unimplemented. Spawn RNG and ordinary
sprite scale approximations predate this change and remain provisional.
