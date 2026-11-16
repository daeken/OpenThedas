# Origins equipment restriction natives

Own interpretation of the read-only GOG 1.05 executable trace. No parsing changes
are needed: the flags use the existing serialized integer-field maps.

Native 375, `SetItemIrremovable` (`0x6654b0`), resolves an item through `0x5d5870`,
normalizes its integer argument to a boolean, and calls `0x752bf0`. That writes
the item's protected boolean at `+0x630` and notifies its current container when
one exists. Native 376, `IsItemIrremovable` (`0x66bd70`), reads that same boolean,
defaulting to false for an invalid item. The flag is separate from Plot.

Equipment admission at `0x7970d0` calls the creature's fitting check `0x6af630`.
That check rejects replacement of an occupied slot when its occupant has the
Irremovable flag (`0x6af686`). It also checks the opposite hand for two-handed
conflicts. The simulator now protects occupied slots, including the inactive
weapon set, before replacement mutates equipment or sends events. Existing item
fitting and two-handed conflict approximations remain outside this change.

The direct native unequip routine `0x6b7690` moves the item to the ordinary
backpack via `0x762c40`. The equipment inventory's removal virtual is `0x7612b0`,
which accepts a non-null item without consulting Irremovable. Therefore the
script's direct UnequipItem retains that behavior; the player-facing equip and
unequip operations enforce the item flag, as does the inventory movie through
the item's reflected Irremovable attribute. Backpack transfers remain allowed.

Native 391, `SetCanChangeEquipment` (`0x665c80`), resolves only a creature and
writes its protected CanChangeEquipment attribute at `+0x5f4` using virtual
`+0x54`. This is a creature attribute, not an inventory-interface method. Its
reflection binding at `0x6b4d81` names it CanChangeEquipment. The serializer
reads/writes GFF field `0x404f` (`0x6b2c1d`, `0x6bf285`). It is reflected to the
inventory screen and used by player-facing equip/unequip operations. Scripted
equipment manipulation is kept separate from those screen controls.

The native restriction checker `0x6ae8b0` also consults this attribute together
with the creature's rules component and a special item ability exemption. The
rules-component predicate is not fully traced, so this change does not invent a
blanket script-equipping prohibition from the attribute alone. Origins' UI used
to report equipment access unconditionally and derive item Irremovable from
Plot; it now exposes both stored native flags. DA2 retains its previous UI and
dispatch behavior.

Five `inventory::tests::equipment_flags_` tests exercise native dispatch and
boolean normalization, flag/Plot independence, serialization, locked occupants,
cross-member player transfers, disabled equipment access, explicit script
unequip, inactive gear, invalid kinds and DA2. One binding test verifies the
Origins attributes and retained DA2 values.
