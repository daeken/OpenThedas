# Inventory script lookups (SCR-039)

Static trace of the GOG DAOrigins.exe, image base 0x400000; SHA-256
2346caee611002d4dff6ecef68eb315b63a37004dd98b39c11a9c26c0b45bfb3.

## CountItemsByTag

Handler 0x666210 resolves the possessor through 0x5d5c20 and calls its inventory
interface's virtual +0xc at 0x6662ae. A creature's interface (vtable 0xb10334)
uses 0x6a2ea0, summing its carried and equipped inventories through 0x7615f0.
Other possessors use the forwarding implementation 0x7630d0, which obtains the
inventory through virtual +0x18 and tail-calls that same helper at 0x7630ea.
An invalid possessor yields zero.

The helper 0x7615f0 iterates items. At 0x761657 it compares the item tag through
0x4c0de0 (the exact, case-sensitive string comparison). A matching item contributes
one unless 0x753c60 says it stacks. That predicate requires DefaultAmmo to be false
and BITM_base's StackSize column (table 6, hash 0x39974122, accessor 0x753070)
to exceed one. The named serializer binds DefaultAmmo to item +0x5d0 at 0x754db4.
For a stackable match the helper adds the protected count at item +0x5f0
(0x76166b..0x761676); otherwise it adds one at 0x76167a. The accumulator is a
32-bit integer. A nonstacking item does not contribute an arbitrary StackSize field.

OpenThedas uses the existing base-item stack limit and stored stack count, excluding
default ammunition. Its inventory vector already includes equipped items, including
inactive weapon sets, so those must not be counted a second time through slot maps.

## GetItemEquipSlot

Handler 0x665410 resolves an item through 0x5d5870, obtains its owner at +0x4a8,
resolves that owner as a creature through 0x5d58c0, and asks its equipment inventory
at creature +0xbe0 for the item descriptor through 0x7612e0. It returns descriptor
+0x8 (slot), defaulting to 255 for invalid inputs. The weapon set is a separate
descriptor member, not part of the returned slot number.

The equipment inventory is constructed by 0x798c60 and installed at creature +0xbe0
by 0x6b443e..0x6b445e. Its vtable 0xb1fb8c has virtual +0x4c = 0x797c80,
the lookup dispatched by 0x7612e0. That function first checks the active slot map
at inventory +0x24. If not found, it traverses the weapon-set collection at +0x44
(0x797cb7..0x797d00), using 0x7f4ee0 to obtain each set's slot. Thus the inactive
set also answers the same slot number. OpenThedas now searches both equipment maps
and requires an item held by a creature.

Both corrections apply to Origins. DA2 retains its previous answers pending a DA2
trace. Tests cover multiple stacks, exact tags, nonstacking/default-ammunition
exceptions, invalid and separate holders, both weapon sets before/after switching,
unequipping, and the retained DA2 path. No table data, row order, stack merging,
equipment mutation, or save layout changes.
