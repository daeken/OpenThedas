# DA2 fake-prologue weapon inputs

The fresh prologue's quick-start path equipped the ordinary starting warrior,
despite the area staging the fake-story character presets. This is an input error
before combat, rather than evidence for changing hurlock HP or damage balance.
This note records clean-room understanding of the local analysis image and shipped
scripts. No executable material or game resources are included.

## Damage and health path

Abilities 3110 and 3210 dispatch through `ability_core` to `dae_basic_attacks`.
The latter's event-1006 handler calls its base-damage helper at bytecode 0x44b4.
For a rank-100 PC, this reads the equipped hand's first type-5 item property and
requests its scaled power, then adds half of the primary attribute above ten:

`base damage = weapon damage + 0.5 * (primary attribute - 10)`

The ability multiplier, hit/critical result, outgoing damage bonuses and target
mitigation are applied by the shipped script. The optional base-damage variance
is 0.9–1.1; the initial basic-attack call disables that optional variation. The
critical helper is 0x4933, outgoing modifiers 0x4bc0, mitigation 0x50f3 and NPC
health normalization 0x635b. The latter uses M2DA 196's level coefficient divided
by M2DA 174's rank coefficient and M2DA 211's difficulty coefficient, all from
column 0xc2017dc8. At level nine, critter rank and normal difficulty, the multiplier
is approximately 0.578704. Thus an NPC's visible HP and a PC's visible HP do not
represent the same unnormalized damage budget.

Native `GetItemPropertyPower` (784) runs through 0x4defb0, 0x4ebcf0 and 0x54dbd0.
For these swords it evaluates the level quadratic, multiplies the item's base
power and item-type damage factor, then applies the property parameter and any
requested rounding. The existing implementation follows that path; see
[the item-power note](da2-item-power.md). Both swords' damage property is 5100
with base power one. The level-two starter sword evaluates to five damage; the
level-fifteen Champion's Warblade evaluates to twenty-five. For these inputs the
curve reduces to rounded `(5.4 * item level + 5) * 0.28571`: approximately 4.51
and 24.57 before the native rounding. Hawke's stored
strength base is 23, but the script requests its total: the warrior passive adds
three, yielding 26 and an eight-point contribution. Raw damage therefore changes
from 13 to 33. Blocked attacks can still produce damage: helper 0x4933 multiplies
them by the difficulty's block-damage coefficient (one half on normal).
Physical mitigation uses armor rating (property 26) and the physical-resistance
modifier (44); property 17 is an outgoing physical-damage bonus. Variation,
critical results, blocks and lethal-hit caps explain why the aggregate trace mean
is not the weapon's advertised damage.

The monster branch of `creature_core`'s spawn scaling explicitly initializes
base HP to 100 at bytecode 0xed49. The fake hurlocks are critter rank, and their
perception handler assigns level nine from the level-ten party leader and the
area's bounds. This is not a missing-health fallback. PC health instead has its
native derived maximum; the supplied Hawke has 210 HP. Native property totals at
0x5ae3b0 combine the base, derivation and modifiers. Neither the health initializer
nor its damage normalization is changed here. The PC-health derivation at
0x5bd900 adds five times total constitution. Hawke's base 50, constitution 22
(base 20 plus the warrior passive's two) and passive HP bonus 50 give the observed
maximum 210. See also
[stat initialization](da2-stat-initialization.md).

That accounts for the supplied starter state, not every intended equipment
bonus. A separate mismatch is the worn-item reader: it still reads Origins'
`itemprps`, whereas DA2's `item_properties` supplies the fake Champion armor's
health, attack and armor effects. The native item-effect initialization at
0x56ef50 builds those effects and scales their parameters through 0x54d7d0.
That correction is kept separate from this preset-selection change. These
gear-only measurements therefore do not claim complete equipped-stat parity.

## Preset selection

The native selector at 0x4974c0 enumerates `chargen_dae` (M2DA 374), reads its
`CreatureTag` column and locates the staged creatures in the current area.
The row index encodes gender, race, class and background/variant. For example,
1310 is the ordinary male human warrior; 1311 is the fake-story counterpart.
The selector's later cleanup at 0x4977a0 hides the staged lineup.

`sys_chargen`'s background-selection event stores the variant, rescales the class
and calls its appearance/equipment helper at 0x55b6. That helper reads the selected
M2DA 374 template and calls `LoadItemsFromTemplate` at 0x577c. Ending creation with
exit zero resets the choice to the generic warrior and starter gear. Exit one
keeps the selected preset. Previously quick start always took that generic exit.

Quick start now chooses the first authored preset whose staged creature is in
the creation creature's current area, sends the normal gender/race/class/variant
events and keeps that choice on exit. It does not hard-code a fake-prologue
weapon, level or damage bonus. An area's ordinary staged presets still select
ordinary gear. Manual creation, level-up and Origins retain their existing paths;
the old quick-start fallback remains available when no staged preset is found.
The story also has its own reset: `pro000pt_main`'s fake-to-real setup at 0x1fd4
passes variant zero to its character-rebuild helper, whose appearance helper
loads the ordinary equipment at 0x179d4. That authored reset remains responsible
for discarding the fake story's equipment before the real prologue.

This event replay is provisional: the full native frontend's character-transfer
sequence has not been established. It preserves the shipped script's equipment
selection without claiming complete character-selector UI parity. Existing saves
keep their equipped items; use an honest fresh run to measure this correction.

## Validation

Compared merged source `019ae5e1` with main `f4a5463a`. fmt and clippy
(`--workspace --all-targets -- -D warnings`) pass. Workspace tests:
1268 passed, 18 ignored, zero failed. The current-area
selection regression also checks ordinary female mage selection and level-up.

Both sides use the frozen supplied `drive_da2.py`, SHA-256
`5a2a52bf1d94051d50b4fc6eec5f9f2d5415845277b2151baa2a0d6bd1eba7ae`,
as the supplied `wave1b.py` probe's drive module. Fresh checkpoints come from
normal load, opening dialogue and eight further ticks. No healing or kill assists
were used; both full honest runs reach the real Blightlands with zero script
errors and restore the ordinary starter sword. All local release packages were
cleaned between source roots, then binary hashes and actual weapon powers were
checked; a stale-library candidate was rejected before these measurements.

| Fresh checkpoint measurement | Main | Preset fix |
| --- | ---: | ---: |
| First six initial enemies observed defeated (seconds) | 214.5 | 132.2 |
| Mean 3110/3210 damage to those six | 5.0972 (81 events) | 12.6058 (32 events) |
| Supplied bounded probe aggregate mean | 5.0199 (170 events) | 12.2849 (60 events) |
| Honest fresh arrival in real Blightlands (game seconds) | 1390.066 | 831.966 |

The aggregate can include later-wave targets while initial ranged enemies remain
alive. Initial-six timing and damage therefore use their object IDs separately.
Blocked positive-damage events count in these means. The earlier 189-second
sample used a passive driver and ended with enemies alive; it was a bounded
sample, not a measured wave clear, and is not the matched baseline above.
The supplied already-created snapshot keeps its stored starter equipment;
its complete probe end states are identical across this fix.

The Tower story replay wipes on the first floor for all five seeds on both binaries; its script still exits zero and saves under the misleading 13_flemeth_hut name. This is baseline parity, not a successful Tower story clear.

The documented DAO gate (s04, s07, s11, s14, s15 plus s13) passes all 12 executions with zero log/plot differences
and 7 identical checkpoint JSONs. Four 600-tick DAO scenarios per binary
are identical on repeat and through save/reload. Extra DA2 check: `DET pro_real_start.json twice: identical | save+reload: [('/next_effect_id', '2', '3')]; DET pro_real_start.json twice: identical | save+reload: [('/next_effect_id', '2', '3')]`.
The DA2 normalized-world difference, if shown, is present on both sides and is
kept explicit rather than treating it as a clean reload result.

Staged own-renderer media compares the same fresh-checkpoint procedure with a
third Delta ×4 panel: 1.9965% of frame pixels differ by more
than 3/255 in any RGB channel. Evidence is retained outside the repository under
the 2026-11-15 damage job, including probe traces, full honest saves, gates,
determinism snapshots and binary hashes.
