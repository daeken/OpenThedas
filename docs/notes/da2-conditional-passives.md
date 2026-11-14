# DA2 combat styles and conditional passive stats

`GetWeaponStyle` command 354 dispatches through 0x4f5370, case 0x162 at
0x4f54ca, to 0x4dd1e0 and 0x52ec60. It returns the id of the selected DA2
`combat_style` record, through the creature's component at +0xa94. The +0x24
accessor at 0x6eb1f0 is shared machine code; neighboring functions do not identify
the component. These ids are not Origins' three weapon styles, despite the
legacy constants still declared in DA2's script header.

Selection at 0x55ff60 prefers a valid positive package override (M2DA 163,
column 0x9ebe3727). Otherwise it examines the first present hand, main then
offhand, and reads the item's combat style (M2DA 12000, 0x93b942d7). If neither
selects a valid record, the learned style ability (M2DA 300, 0x5d74e7ea) is
searched in table order after the default row. Otherwise the default is zero.
The original installs the selected record at 0x58502b through 0x758470.
OpenThedas derives the selection from these inputs on query.

The registry loader at 0x5cd984 reads the row's ID (0x66fbc936), then stores
that value as both registry key and the record's first integer at 0x5cd9b8.
The learned-ability fallback has a native quirk: 0x5600f8 selects the scan index,
not that row ID. An absent registry key yields the default zero record. The
shipped learned styles 1 through 5 have matching indices and IDs; sparse NPC
styles have no learned ability. Tests preserve the distinction for authored
sparse tables.

Real prologue data selects style 3 for Hawke/Carver's item 1001, style 5 for
Bethany's item 1200, and NPC styles 100/101 through hurlock packages 20100/20101.
Styles 2 and 10 share learned ability 202000; fallback reaches 2 first, while an
authored item can explicitly select 10.

The modifier-state selector 0x52f7c0 maps style 1 to bit 8, 2/5/10 to bit 16,
3 to bit 4, and 4 to bit 2. Other styles produce neutral 255. A conditional
modifier matches only a recognized single-bit state, never neutral 255. Rows
with mask zero always apply. Shipped modifier row 102050 adds 10 to property 15
only in the two-handed state (mask 4).

The ten modifier-row ids are themselves read through the upgraded integer
getter (0x56e067 -> 0x6a1300). The property query and script getter now share that
resolver. Learned upgrades can therefore change which modifier row a passive
uses, while property bases remain unchanged across grants, removals and reloads.

The previously unexplained callers at 0x57b8ca/0x57b8f8 and
0x57bb39/0x57bb69 bracket GFF property serialization: remove the current state,
apply neutral 255, serialize, then restore. They introduce no further gameplay
states. Native style changes use the remove-old/apply-new loop at 0x56e240.

The shipped conditional row affects critical chance, not resource maxima.
General native preservation of resource fractions when arbitrary modded passive
maxima change remains outside this query-derived implementation.
