# Origins derived attribute contributions

Static trace of Origins 1.05, `DAOrigins.exe` SHA-256
`2346caee611002d4dff6ecef68eb315b63a37004dd98b39c11a9c26c0b45bfb3`.
Addresses below are VAs with image base `0x400000`. No original-game execution
is claimed by this trace.

The property's total getter (`0x79c760`) combines its base, derivation and
modifiers. Derivation dispatch (`0x79c5b0`, table `0xc0f604`) selects these rules:

| Property | Native function | Attribute contribution |
|---|---|---|
| Attack | `0x79cb30` | `0.5*max(Strength-10, 0) + 0.5*max(Dexterity-10, 0)` |
| Defense | `0x79cc90` | `max(Dexterity-10, 0)` |
| Armour penetration | `0x79cd40` | `max(Cunning-10, 0)/7` |
| Spellpower | `0x79cdf0` | `max(Magic-10, 0)` |
| Health | `0x79cea0` | `5*max(Constitution-10, 0)` |
| Mana/stamina | `0x79cf50` | `5*max(Willpower-10, 0)` |

Each function compares the excess with zero and retains zero for a nonpositive
excess. Attack floors the two contributions independently. The two resistance
functions (`0x79d000`, `0x79d0f0`) instead retain signed excesses.

Previously `derived.rs` used signed excesses for all these derivations. With
Strength 8, Dexterity 14, attack base 60 and an explicit attack modifier −10,
it returned 51; the native formula gives 52. The regression also covers negative
attribute modifiers, the other clamped properties, and signed resistance terms.
Explicit property bases and modifiers still contribute normally. DA2 returns
through its separate derivation method before these Origins rules.

This matters during ordinary Tower combat even when the starting attributes
are at least ten. The shipped `rules_core` ApplyStun path (NCS offsets
`0xe56d..0xe585`) subtracts 1000 from Dexterity's modifier. A diagnostic Tower
replay observes defense queries for stunned Alistair at time 1355968 with
Dexterity excess −994, and the mage at 1356500 with excess −996. The corrected
derivation contributes zero and preserves base defense; the old signed term
erases it. The regression includes this −1000 modifier case. The paired Tower
gate therefore changes combat timing, while both builds reach Flemeth's hut;
serial repeats reproduce each build's respective path.

For the traced Human Noble warrior / Tower genlock snapshot, all six attributes
are at least ten. This correction leaves that pair's attack/damage inputs
unchanged. The attack and damage arithmetic itself resides in shipped
`rules_core.ncs` combat helpers; the engine supplies property and item totals.

Equipped base stats are native modifiers: `0x71b3c0` follows itemstatdefines'
linked-property column `0xd6505aa1`, multiplies the itemstats cell by its
materialrules multiplier, and adds it via `0x71b2d0 -> 0x71a800`. Equip calls it
with +1 at `0x6c17e4`, unequip with −1 at `0x6c1a5d`. Our query-time gear sum
therefore supplies the same armour/defense terms for this pair. The item-stat
script getter (`0x665670 -> 0x753ce0`) performs the same base/material product.

The selected weapons' attribute-damage style is also traced. `0x6a3c30` reads
combat-state +0x5c. `0x6b8dd9` installs the gear-tree selection from `0x7c3500`
through `0x7c3600`: OW/WS map to SINGLE (1), DW to DUAL (2), TW to TWOHANDED (3).
The two-handed test (`0xa664c0`) checks whether item equippable slots equal the
main-only mask 1 (`0xa66440`, column `0xd5b0ae22`). The selected sword and mace
allow either hand; the shield selects WS and an empty off hand selects OW.
Both therefore take the script's single-weapon attribute-damage branch. Other
weapon/appearance combinations remain outside this pair's parity claim.
