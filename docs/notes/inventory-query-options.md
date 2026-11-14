# GetItemsInInventory options (SCR-038)

Static trace of the GOG DAOrigins.exe, image base 0x400000; SHA-256
2346caee611002d4dff6ecef68eb315b63a37004dd98b39c11a9c26c0b45bfb3.

Handler 0x672410 reads the owner, options, base-item type and tag, and optionally
the fifth argument (ignore plot items) at 0x6724f2. It resolves the owner through
0x5d5840, then selects by object kind. The branches are creature at 0x672527,
placeable at 0x67283a, and store at 0x672923. Other kinds return an empty array.

For a creature:

- Options greater than zero include the equipment inventory at creature +0xbe0
  (0x67255c). Options exactly two return after equipment (0x672655). Thus zero or
  negative selects backpack, one selects all, two selects equipped, and other
  positive values select all. These are comparisons, not a bit mask.
- Equipment comes first. Its iterator is made by 0x797740 -> 0x797510, traversing
  active slots, weapon set zero, then weapon set one. Advance 0x7975f0 exhausts
  each child before proceeding. The inactive set is included; switching sets moves
  weapons between the active map and the set maps. Slot maps use ordered keys
  (see 0x7f4f60). OpenThedas represents these as active and inactive maps, each
  ordered by slot, with equipped items also referenced by its inventory vector.
- Base type zero means any. Otherwise the item's unsigned base-item member at
  +0x464 must equal the requested integer (0x6725b3..0x6725dc; the backpack loops
  repeat this check). A nonempty tag must match exactly through 0x4c0de0
  (0x6725ff, 0x672711, 0x6727ee).
- The backpack is at +0xbf0. With ignore-plot nonzero, 0x67266b selects only its
  ordinary-item child at inventory +0x8. Otherwise it iterates the whole backpack
  at 0x67275d. The whole iterator 0x740530 appends ordinary items first (+0x8,
  0x740566), then plot items (+0x18, 0x740582). Insertion 0x7409c4 routes an item
  with its Plot byte set (+0x2c4) into the plot child. The flag does not remove
  plot equipment, since equipment was already appended before this choice.

Placeables use their container at +0x678; stores use their inventory at +0x444.
Both branches iterate it and apply only the exact tag check (0x6728d7 / 0x6729bc).
They ignore options, base type, and the ignore-plot flag. Filtering them like a
creature would introduce a difference from the original.

OpenThedas now implements these branches and filter rules for Origins. Equipment
is not duplicated through the backpack vector. Tests assert array order as well
as membership, including mixed plot/ordinary items, both weapon sets, case-only
tags, base-type filtering, invalid inputs, nonstandard option values, and container
exceptions. DA2 retains its previous behavior pending its own trace. This does not
change inventory ownership, mutation, table data, or table row order; party-wide
inventory ownership remains the separate SCR-042 gap.
