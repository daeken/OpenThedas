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

Synthetic regressions cover passive/base-item filtering, selected integer versus
float parameter behavior, creator and lifetime, add/remove/unequip, and replacement
with and without discarding the backpack. Installed-game stat queries, probe
measurements, the DAO gate and determinism results will be recorded after the
compiled verification run.
