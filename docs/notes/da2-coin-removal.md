# DA2 coin removal and Cavril's chest

The routed Gallows task reaches Cavril, lets Aveline persuade him to hand over the
key, and loots his chest. The coins become money, but Athenril still waits for the
task to finish. Her chest's authored script handles INVENTORY_REMOVED (7) by setting
the task-completion flag (68, unless flag 76 is set). Our coin conversion returned
before queuing any inventory event, so that handler never ran.

DA2's MoveItem registration at 0x527484 points to 0x4ec340, which calls 0x4dc500,
then 0x6cfd20 -> 0x6cf880 -> 0x6cc7b0. The transfer removes from the source through
0x6c71b0 before adding to the destination. Inventory's virtual removal at 0x6cc030
has a specific base-type-17 coin path at 0x6cc0c6; it still produces a successful
removal transaction with the item identity. This is not an add-only currency edit.
The precise native notification dispatch and payload remain untraced.

The DA2 currency path now queues the same source INVENTORY_REMOVED payload as
ordinary transfers. It keeps the existing currency conversion and sends no added
event for the consumed coin. Origins retains its prior behavior. The item identity
in the queued notification refers to the consumed item, as in the existing coin
conversion; the Cavril handler needs only the removal notification.

The synthetic test verifies one source event for a complete coin stack, the money
amount, the absence of an inventory coin, no second event on a repeated loot, and
the unchanged Origins event behavior. No authored scripts or resources are copied
into the repository.
