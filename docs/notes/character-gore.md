# Origins character gore

## Evidence and ownership

This trace uses the installed Origins 1.05 executable and its shipped resources.
Addresses below are virtual addresses in DAOrigins.exe. Script offsets are byte
offsets in the named `.ncs` from `packages/core/data/designerscripts.rim`.
No extracted assets or executable listings belong in the repository.

Gore is script-readable, saved creature appearance state. The rendering input is
that state. Combat scripts already execute in the headless simulation, so keeping
their Set/Get values supplies the renderer without a second accumulation rule.

| Operation | Evidence | Rule |
| --- | --- | --- |
| Script getter, native 719 | `0x65d5d0`, appearance getter `0x748520` | Resolve a creature and its appearance at creature `+0x7b0`. Return zero for an invalid creature or absent appearance. Otherwise read appearance `+0x180`, masked to zero by ShowGore. |
| Script setter, native 720 | `0x65d650`, setter `0x747c40` | Clamp to 0–1 and write seven appearance values at `+0x180…+0x198` and six at `+0x1b0…+0x1c4`. Refresh the creature's materials through `0x6b73c0`. |
| Material update | `0x6b73c0`, `0x6b1bc0`, parameter name at `0xb0feac` | Write `mml_fGoreThreshold` on the model's material instances. The model-refresh path also handles replacement equipment at `0x6b19b0`. |
| Appearance persistence | `0x748f16…0x748f3d`, `0x74a99f…0x74a9d1` | Save/load appearance field `0x3fc4` through the getter and setter. The save stores the first amount and loading spreads it across the appearance. |
| ShowGore | `0x748520…0x74853f`, `0x838210` | The client option byte at client `+0x94` masks the getter. Stored gore survives turning the option off. |
| Option defaults and refresh | `0x5e8a34…0x5e8a48`, `0x5e1370…0x5e1581` | ShowGore defaults on. An option change refreshes creature material amounts through the masked getter. |

## Build-up and clearing

The damage handler in `creature_core.ncs` reaches its gore branch at `0x459e`
when damage flags contain bit `0x4`. Its increment is `0.001`, replaced by
`0.005` when bit `0x1` is set, otherwise by `0.01` when bit `0x2` is set.
The call at `0x46e9` applies that increment to the damage dealer/creator. These are
per invocation amounts, independent of damage magnitude at this branch.
`player_core.ncs` has the same branch at `0xa614…0xa75f`.

The shared helper at `creature_core.ncs +0x92d7` reads the dealer's appearance
row in M2DA 2. If `bDisableGore` is true it returns. If the current gore is below
`0.6`, it adds the increment, clamps the result to 0–1, and calls the native
setter at `0x93d5`. An amount already at least `0.6` stays there. The corresponding
player helper is `player_core.ncs +0xe14e`. The adjacent `bCanBleed` check gates
the impact VFX on the damage recipient, separately from the dealer amount update. VFX 1015/1016 are separate
from the persistent character texture.

The successful living-creature heal handler halves gore at
`creature_core.ncs +0x654c…0x6597`, also present in `player_core.ncs
+0x4bfb…0x4c46` and other scripts compiled with the heal helper. Placed corpses
with `CREATURE_SPAWN_DEAD` receive a RandomFloat amount at
`creature_core.ncs +0x1019…0x103c`.

Area pre-load exit event 38 clears the party pool with the zero-setting helper
at `area_core.ncs +0x1549`, called from `+0x518`. `player_core.ncs +0x1106b`
also clears gore when its UNEQUIP handler sees slot 4, `INVENTORY_SLOT_CHEST`,
at `+0x10dd5…0x10df1`. The dispatch at `+0x187…0x18f` selects event 12,
`EVENT_TYPE_UNEQUIP`. Both names and values are declared by `script.ldf`.

No elapsed-time fade or unconditional end-of-combat reset was found in these
paths. A separate amount awarded to the damage recipient or specifically for death
blows remains untraced. Those paths must remain provisional. The implementation
lets shipped scripts control all accumulation, healing and clearing.

## Material and shader path

`character.mat.xml`, `face.mat.xml` and `weapon.mat.xml` declare the float
`mml_fGoreThreshold` and the volume texture `mml_tGoreMap`, mapped to `smpGore`.
The character, weapon and cloth authoring default is `Aur_WhiteVol`. The face default
is `Gore_Default`. Appearance material construction installs `Gore_Default`
at `0xa60f00…0xa60f32` and `0xa6746f…0xa674a4`. The installed High texture
archive contains `gore_default.dds`, a 512 × 512 × 4 volume. The material objects inspected
in `materialobjects.erf` provide no gore-map overrides.

The compiled constant tables give the following bindings.

| Pixel shader | Threshold float register | Volume sampler |
| --- | --- | --- |
| `Ch1.psh` | c30.x | s8 |
| `Ch1ArmTnt.psh` | c29.x | s8 |
| `Face0.psh` | c49.x | s8 |
| `Weapon1.psh` | c30.x | s8 |

The texture is a volume whose third coordinate selects progressive coverage.
`Face0.psh` clamps the threshold to 0–0.2 and samples with face UVs.
`Ch1ArmTnt.psh` clamps to 0–0.3, mirrors/projects its UVs using its vertex
inputs, doubles those coordinates and remaps the volume coordinate to
`min(1, 1/3 + level)`. Both multiply their lit colour by the volume's red
channel times `(0.44, 0.91, 1)` plus `(0.56, 0.09, 0)`, bypassing the multiplier
at zero gore. Other shader variants retain their own authored operations.
The faithful renderer executes this shader code through its existing translator.

The new texture fallback substitutes `gore_default.dds` for the character,
face, weapon and cloth authoring placeholders. Explicit material-object textures and
other material defaults retain their bindings. Runtime thresholds are per draw,
so creatures sharing a material can carry different amounts. The threshold is a
per-frame register, including transitions back to zero after healing, toggling
the option or loading another state. Newly equipped parts receive the current
appearance amount during normal scene synchronization.

## HANDOFF: sim

The scalar Origins script interface is implemented in this change. `Object.gore_level`
is saved in replacement-engine saves and defaults to zero for older saves.
`Sim.show_gore` follows the existing client-option message route, defaults on,
and masks the getter and rendered amount. It stays outside game saves.
The renderer reads these values after simulation updates.

Remaining work follows the traced paths below.

- Cutscene action 45, `ActionSetGore`, reads the authored amount from field 5750.
  Its execution at `0x7cfca0` calls the same appearance setter and refreshes the
  model. Its cleanup at `0x7cfd00` sets zero and refreshes again. The action can
  target a creature or a model-only actor, whose threshold is written directly.
  Add this action to the cutscene presentation, preserving its lifetime and zero
  cleanup. Its authored scheduling and cancellation cases need tests.
- Original GFF appearance loading should import field `0x3fc4`. The current
  replacement-engine JSON path carries raw gore. The original GFF writer uses
  the masked getter, so saving with ShowGore off writes zero. That option-off
  persistence distinction remains part of native-save parity work. The original
  appearance loader's additional per-slot values and the meaning of each slot need a separate trace
  before exposing independent body/equipment inputs.
- Verify recipient splashing and death-blow-only gore before adding any new
  simulation rule. The traced dealer branch and the shipped scripts remain
  the implemented source of combat gore.
- DA2's native gore state is untraced. Its existing Set/Get behavior is retained.

## Validation and capture status

Synthetic material tests cover the constant-table register, authored amounts,
per-instance overrides, repeated changes including zero, and volume texture
binding/fallback. Synthetic simulation tests cover native Set/Get, clamping,
invalid objects, ShowGore, persistence and legacy saves.

The deferred G-buffer character shaders still lack gore composition. Adding the
volume sampler and reproducing each shader family's UV and post-lighting colour
operation requires a separate implementation. The faithful path is the review
reference for this change.

Original reference status was checked under `reference/original/triage/character_gore`.
Its README reports that combat could not be reached. Its existing captures have
no bloodied character, so they cannot validate the gore result.

The instrumented Proving replay confirms the dealer target of this helper.
Seven damaging attacks on active Mainar (object 855) increased the hero from
zero to `0.007` while Mainar stayed at zero. The trace records the shipped
`rules_core` calling SetCreatureGoreLevel on the hero after each damage event.
Mainar died and `bdc140cs_everd_victory.cut` started. No gore value was injected
for this capture save.

The saved state was advanced through the victory cutscene before framing the
creature. That cutscene has no action 45. The same after-victory save was rendered
with main `d6b15225` and the candidate. Natural gore is visible across the helmet
and armor. At an absolute RGB difference threshold above 2/255, 8.7905% of full
frame pixels changed and 32.3913% of pixels in the displayed creature crop changed.
The comparison's third panel displays absolute difference amplified ×4.

Validation on Apple M4 with Metal passed workspace Clippy with warnings denied
and 1,480 workspace tests. There were 26 ignored tests and zero failures.

Disabling ShowGore on the same saved creature produced a frame identical to the
baseline at every RGB byte. The amount remains stored while the client masks it.

Explicit custom material-object gore maps retain their existing bindings. Their
precedence relative to the executable's appearance override remains provisional,
since the inspected shipped material objects had no such overrides.

The sequential main-versus-candidate regression gate passed all five steps.
`s04_night`, `s07_father`, `s11_joining`, `s14_lothering` and `s15_board`
each had zero log differences, identical plot sequences and matching final
areas. Their plot event counts were 46, 56, 46, 115 and 13. Neither run refused
a save.
