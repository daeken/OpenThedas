# DA2 worn item effects

The shared worn-property reader used Origins' `itemprps` table. DA2's native
`item_properties` table (M2DA 12001) has another schema, so those effects were
absent even when the correct weapon and armor were equipped. This includes the
fake prologue's Champion armor health, attack, physical damage and armor inputs.
The creature's base-health initializer and incoming-damage normalization are
separate from these missing equipment modifiers.

## Native initialization

The item initialization loop at 0x57c8f0 calls 0x56ef50 for each property. Its
switch on column 0x12a02374 behaves as follows: type zero, restriction type three
and type four do not build a worn effect; type two takes a distinct on-hit-wrapper
path; type one and values above four use the effect in column 0x3ba07a8a. A zero
effect ID produces none. Socketed sub-items recurse in the native implementation;
the simulation's existing item model has no socketed sub-items.

The native builder assigns the source item as creator, sets the internal lifetime
to four, and evaluates parameters through 0x54d7d0. The duration getter at
0x4e840d reads that same field. Lifetime four is distinct from
script permanent, whose LDF value is three. The parameter defaults are columns
0xa3630beb / 0xba783aaa for the two integers and 0x861984a9 / 0x9f02b5e8 for the
two floats. Column 0x36001b03 selects which one is scaled. The level curve,
item-type/material factors and optional float rounding are the same as the
[item-power query](da2-item-power.md).

The selected integer parameter is multiplied by the scaled power, then native
0x54dad7 / 0x54daf4 adds one half and truncates it into the effect field. The public
power query returns a float product. Integer row defaults remain exact integers;
the effect conversion retains a wider intermediate product so the API return
type cannot discard low bits before the integer field is assigned. The selected float parameter
uses the optional signed rounding already implemented by the power evaluator.
All other parameters retain their row defaults. The effect is attached to the
wearer's controller at 0x56f0e7, and normal APPLY_EFFECT / REMOVE_EFFECT scripts
own its actual stat changes.

The implementation builds these effects through the existing equip, add-property,
remove-property and unequip paths. Removal reconstructs the old effect using the
removed entry's power, even after that entry has left the live item list. It does
not add another derived stat bonus or bake item values into property bases.

## Replacing a template's inventory

Native LoadItemsFromTemplate (340) is registered at 0x527035, through 0x512560 and
0x503d80 to 0x598a10. It destroys equipped items first, regardless of the replace-
inventory argument. With that argument set, it then destroys backpack items.
The equipment and backpack loops at 0x598ab5 / 0x598aff use the same destruction
virtual method as DestroyObject's 0x4e57e2 call. This removal ends worn effects
before their source items disappear.

DA2 now follows that destruction path before loading replacement items. Clearing
maps and removing objects directly would leave their old modifiers active, so
that cleanup is part of enabling the effect lifecycle. Origins retains its prior
reader and replacement path.

The existing integer instance-power model and missing rune recursion remain
limitations. On-hit wrappers are not applied as flat worn bonuses. Old snapshots
that never acquired the effects retain their stored state; use a fresh run or
ordinary re-equipping to establish them. This change does not replay inventory
or spawn callbacks during load, or invent bonuses outside the authored tables.

## Validation

Compared merged source `86da3d7a` with main `f4a5463a`. fmt and clippy
(`--workspace --all-targets -- -D warnings`) pass. Workspace tests:
1270 passed, 18 ignored, zero failed. The three synthetic
regressions cover effect construction, property edits and unequipping, and
replacement with either backpack policy, including an equipped item absent from
an old snapshot's inventory. Documented DAO gate (s04, s07, s11, s14, s15 plus s13): all 12 executions succeed, zero log/plot
differences, 7 identical checkpoint JSONs. Four 600-tick DAO scenarios per
binary are identical on repeat and through save/reload.

The independent effects patch retains the main baseline's weapon. A separate
source archive combines this patch with the staged-preset fix (`0d8a0601`),
with 1271 workspace tests passing and 18 ignored.
All workspace release packages are cleaned between source roots. Frozen binary
hashes and actual installed-game equipment/stat queries are retained with the
external evidence.

| Fresh Hawke total | Main | Effects only | Both fixes |
| --- | ---: | ---: | ---: |
| Max health (7) | 210.000 | 210.000 | 245.000 |
| Attack (13) | 132.200 | 132.200 | 220.200 |
| Physical damage bonus (17) | 0.000 | 0.000 | 3.000 |
| Armor (26) | 0.000 | 86.000 | 155.000 |

The shipped APPLY_EFFECT scripts produce these totals; the patch does not bake
bonuses into base properties. Every full honest run reaches the real Blightlands
without cheat healing or forced kills, with zero script errors, ordinary starter gear,
and no worn effects whose creator is absent from current equipment. This checks
that fake Champion modifiers end during the authored gear reset. At real arrival,
the effects-only and combined runs both have maximum HP 160, attack 65.6,
physical-damage bonus zero and armor 86. Their raw modifier map is exactly
`{26: 86}`; the fake armor's +35 HP, +88 attack and +3 physical bonus are gone.

| Fresh probe | Main | Effects only | Both fixes |
| --- | ---: | ---: | ---: |
| Initial six enemies observed defeated (seconds) | 214.5 | 214.5 | 164.2 |
| Mean 3110/3210 damage to those six | 5.0972 (81 events) | 5.0972 (81 events) | 14.6815 (28 events) |

The preset-only fix in [PR #939](https://github.com/daeken/OpenThedas/pull/939)
measured 132.2 seconds and mean damage 12.6058 to the same initial six enemies.
Thus adding the worn effects improves the recorded damage mean but does not
monotonically improve this driver's clear time. Full honest real-area arrival is
1390.066 game seconds on main, 831.966 with the preset alone, 1390.066 with effects
alone and 838.866 with both. These are measurements of this scripted route;
original-game combat-time parity is not established.

All probe comparisons use the same frozen supplied `drive_da2.py` (SHA-256
`5a2a52bf1d94051d50b4fc6eec5f9f2d5415845277b2151baa2a0d6bd1eba7ae`)
as `wave1b.py`'s drive module. Aggregate probe damage can include later-wave
targets while initial ranged enemies live, so the table filters the initial six
IDs. Blocked positive-damage events count. The old 189-second passive-driver
sample ended with enemies alive and is not the matched baseline above.

The Tower story replay wipes on the first floor for all five seeds on both
binaries; its script still exits zero and saves under the misleading
`13_flemeth_hut` name. This is baseline parity, not a successful Tower story clear.

Extra DA2 600-tick repeat and save/reload results, including fresh snapshots
containing the new item effects:

- effects base-da2-detcheck.log: `DET pro_real_start.json twice: identical | save+reload: [('/next_effect_id', '2', '3')]`
- effects base-fresh-detcheck.log: `DET base_01_pro000ar_blightlands_fake.json twice: identical | save+reload: identical`
- effects fix-da2-detcheck.log: `DET pro_real_start.json twice: identical | save+reload: [('/next_effect_id', '2', '3')]`
- effects fix-fresh-detcheck.log: `DET fix_01_pro000ar_blightlands_fake.json twice: identical | save+reload: identical`
- combined fix-fresh-detcheck.log: `DET fix_01_pro000ar_blightlands_fake.json twice: identical | save+reload: identical`

Any baseline reload difference is retained explicitly in the external snapshots;
it is not silently ignored or called an identical reload. The comparison note
records the observed field differences alongside the DAO checks.

Own-renderer media has Before, After and Delta ×4 panels. At a threshold greater
than 3/255 in any RGB channel, the effects-only pair changes
0.0000% of frame pixels; the combined pair changes
1.9965%. The effects-only same-pose frames are pixel-identical; the corrected armor
stat does not change the equipment appearance in this capture. All traces, snapshots, hashes and
media statistics remain outside the repository in the damage job evidence.
