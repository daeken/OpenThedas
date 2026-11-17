# Equipped-item discard and ground placement

Own static reading of the GOG 1.05 DAOrigins.exe LLVM dump. Addresses are virtual
addresses at image base `0x400000`. No executable bytes or disassembly are included.

## UI and simulation path

The inventory movie's confirmed destroy action sends
`GUI.SelectedChar.DropItem` with the item id (`ExternalCommands.SendMessageArgs`).
In the app, UI input passes through `thedas_ui::input::apply` into
`Sim::apply(GuiMessage)`, then `Sim::gui_message` and `inventory_message`.
The headless MCP UI uses the same GUI handler. At main `d6b15225`, including the
reported build `81af4d72`, that message has **no simulation handler**. Equip and
unequip are handled; discard is ignored. There is no existing simulated drop-copy
operation to repair. This explains retained equipment for the inventory message,
but does not establish the provenance of the reported floating copy.

## Executable comparison

- `DropItemMessage`'s name is at `0xb12604`; bindings at `0x7934d9`, `0x79351c`
  and `0x793571` register it. The creature handler is bound at `0x6aa1b0`
  (handler pointer `0x6a72e0`). Its RTTI identifies a creature/member-function
  handler for `DropItemMessage`.
- `0x6a72e0` resolves the supplied item id, sets the player-discard flag `0x400`
  in `0xc371b4`, and calls container removal `0x762100(item, -1)` at `0x6a735b`.
  On success, `0x6a7381..0x6a738f` invokes the removed item's deleting destructor.
  It does **not** spawn a ground item or loot bag.
- Removal `0x762100` uses the item's actual container (`item+0x4e8`) and that
  container's removal virtual. Equipment removal invokes `0x797e20` and,
  for active equipment, `0x6c19f0`: remove item-created effects and base stats,
  recalculate, then immediately send `EVENT_TYPE_UNEQUIP` (12). The creature is
  recipient and creator; object 0 is the original item id, integer 0 its slot.
  This container hook is distinct from explicitly unequipping to the backpack
  (`0x6b7690`); it need not manufacture a temporary backpack copy.
- Inventory removal sender `0x763240` builds `EVENT_TYPE_INVENTORY_REMOVED` (7),
  recipient and creator the possessor, object 0 the item; integer 0 is the
  `0x400` bit (1 for player discard). Party backpack hooks route through the
  party and selected creature; equipped gear belongs to its wearer. The new
  path uses the simulator's existing per-member inventory representation.
- `ITEM_UNEQUIPPED`/`ITEM_LOST` are not script event constants. There is a
  separate training event `EVENT_TYPE_TRAINING_ITEM_UNEQUIPPED` (1070), raised
  by the explicit unequip routine. The removal hook uses `UNEQUIP`, not that
  training routine. Item-lost notification policy inside authored scripts is
  separate from these native events.
- Native corpse bags are a different path: `SpawnBodyBag` (`0x65e3a0` ->
  `0x6a91e0`). `0x6a92b9..0x6a931e` creates/repositions `gen_ip_bodybag` from the
  creature's position; there is no carried-helm/head-height offset. The bag is
  initially hidden (appearance 16); corpse decay later supplies its corpse
  model (`0x6b98e0`). This is not evidence for a native discard sack or a
  native discard height above the floor: native discard leaves no object.

## Requested recoverable drop

The job explicitly requests a recoverable object on the floor. Accordingly,
`Sim::drop_item` intentionally departs from native destruction: it displaces
worn equipment (ending effects and synchronously notifying the wearer), removes
the same id from its owner's inventory, and puts it into a new template-built
`gen_ip_bodybag` container. It queues `INVENTORY_REMOVED` on that owner with
creator=owner, object0=item and int0=1. No new item is created, no stack is split,
and no corpse/body-bag link is assigned to the living creature. The container
can be opened through `placeable_core` and the ordinary loot UI.

The visible container uses `placeable_types.gda` row 126, Sack, Small
(`PLC_SackSmall_01_0`), instead of the hidden corpse-bag appearance. This model
choice is provisional, not a traced native discard appearance. The sack and
contained item receive the walkable navigation floor under the selected
creature's XY. The navigation floor is an approximation of physical ground;
if no floor is available the creature's stored feet height is retained. No
rendering transform or mesh pivot is changed. DA2's previously unhandled
message remains unhandled. Plot items, locked equipment, equipment access,
party ownership and repeated drops are checked before mutation.

Four synthetic equipped-helm tests exercise ownership/effects/events, an elevated
nonzero floor above and below stored feet, refused requests, and another
member's inactive equipment. `tools/repro/dropequipped.sh` is also installed at
external `sim-work/repro/dropequipped.sh`: it sends real MCP inventory messages,
reads the independent `inspect_object.floor_z` navigation sample, checks bag Z and original
id ownership, reloads, and recovers the same item. It raises the creature 1.25 m
for the drop so copying creature Z cannot accidentally pass.

MCP `inspect_object` now includes `floor_z` (null without a walkable sample),
so ground placement can be checked without treating an object position as the
floor or retaining a transient script location handle across a save. Loading the
navigation cache for this diagnostic does not change serialized world state.
