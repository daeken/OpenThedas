# Origins ring, weapon and play-time queries

Own prose interpretation of the read-only executable. All new dispatch is
Origins-only. No parsing or resource-format changes are made.

`GetMeleeRingPosition` (606, 0x66cac0) resolves a creature, bounds the unsigned
index by its ring vector, and returns the ring record's first three floats.
Invalid creatures, negative indices, and absent rings return zero. The result
is a ring CENTER, not a slot on its circumference.

Ring construction 0x6ac2a0 creates the primary ring and optional consecutive
`meleering1`..`meleering9` records for the appearance. It stops when a table has
no row for that appearance. The primary ring's forward offset is appearance
table 2 column 0xed58b0c9, clamped to [-0.5, 0.5], multiplied by 0.6. Updating
its center at 0x6a3060 adds that offset along the creature's forward vector.
Sim computes this center from its current position/facing. Extra-ring centers
can be anchored to animated bone nodes (record +0x2c); Sim has no animated
skeleton and currently returns the owner's position for those centers. This
is an explicit boss-ring approximation, independent of the existing melee
slot arbitration approximation.

`GetWeaponsUnsheathedStatus` (160) uses shared handler 0x668a20, creature branch
0x668b41, predicate 0x7c34c0. It returns false without a creature/controller.
Controller modes -1, 0 and 2 read the main-hand flag; 1 and 3 require both
hand flags; other modes return false. Sim retains explicit hand flags in
object fields, deriving the ordinary single/pair controller mode from existing
equipment when no mode is stored. Its previous combat-based attachment choice
remains the fallback for snapshots without those flags.

The paired script commands 158/159 share builder 0x66fb30: internal command 39,
payload false/true respectively. Executor 0x7a26d0 chooses draw versus sheath;
without its animation path it toggles both controller flags through 0x7c33d0
at 0x7a28e8. Sim now retains the explicit choice at command completion, and the
UI reads it and invalidates its attachment cache. The existing half-second
command timing remains provisional; animation event timing is not reconstructed.
Combat transitions return control to the existing automatic combat policy.
Cutscene hand flags are still presentation-owned and are not imported into Sim's
controller; the query during those cutscene overrides remains a limitation.

`GetPlayTime` (830, 0x666b40 -> 0x69d340) returns -1 without a player context,
otherwise accumulated integer playthrough seconds. The executable samples an
external timer and adds its integer delta to the player's saved counter. Sim
must remain deterministic: its current implementation uses saved simulation
milliseconds divided by 1000 with a loaded-module context. Wall time spent
paused/in menus, and the native save's separate historical play-time counter,
are not reconstructed. This is a deterministic approximation, not an exact
wall-clock replacement.
