# Origins MoveAllItems

Own prose from the read-only GOG 1.05 executable trace; DA2 dispatch is unchanged.

Native 350 enters at `0x666020`. Both arguments are resolved through the
inventory-interface lookup `0x5d5c20`; a missing interface makes the call inert.
It calls the destination interface's bulk routine `0x763570` with the source ID.
That routine obtains each side's default inventory through virtual `+0x18`.
Equipment inventories are separate and are not included in this operation.

The helper at `0x762e90` snapshots the source iterator and sends the resulting
items to `0x762d60`. Each goes through `0x762c40` / `0x762990`, including source
removal permission and destination admission checks, followed by ordinary
remove/add operations. An already-correct destination/location is a successful
no-op. Failure for one item does not stop later items.

For the creature's composite backpack (`0xb1b974`), the iterator at `0x740530`
includes both ordinary and plot inventories. The admission check `0x73f600`
routes plot items and base-item 17 coin outside the ordinary capacity check.
Ordinary admission `0x7ca890` checks the slot count and maximum (`-1` means
unlimited); a compatible non-full stack can still admit an item. The ordinary
backpack removal virtual is `0x722ae0`, which always allows removal. In
particular, an item's Irremovable flag does not block moving a backpack item.
Both active and inactive equipment are excluded, regardless of Droppable.

OpenThedas snapshots carried item IDs in inventory order, excludes both equipment
maps, checks the existing maximum-inventory-size state, and uses `give_item`
for stack merging, recipes, coin conversion and inventory events. It does not
move the source's separate money balance. Invalid possessors and self transfers
are inert. Creature, placeable and store inventories are supported.

The simulator still keeps party backpacks per creature rather than as the
original shared inventory. This operation uses the given object's existing
inventory representation. Its fallback creature capacity is the same
provisional starting size used by GetMaxInventorySize; placeables and stores
default to unlimited unless a maximum is explicitly stored. Existing transfer
event timing, recipe and currency behavior are retained.

`inventory::tests::bulk_` covers script dispatch, order and events, both weapon
sets, plot and irremovable backpack items, retained money, stack merging at
capacity, per-item failures, plot/coin admission, invalid owners, self transfer,
and unchanged DA2 dispatch. No asset parsing changes are made.
