# Origins combat stance overrides the walk/run speed column

Static trace of DAOrigins.exe for the combat-walk portion of SIM-005.

The move executor passes the run flag into locomotion at 0x7a93d0. The locomotion
setter 0xa639c0 maps false to mode 1 (walk) and true to mode 2 (run).

The target-speed function 0xa63120 checks the combat-stance byte +0xf9 before
choosing either ordinary gait column. When the byte is set, 0xa6313d..0xa63151
selects combat column 0x0dc3ebb2 (or alternate column 0x03186054 under +0xf8).
Modes 1 and 2 both dispatch to the table lookup at 0xa63174. Thus asking to walk
does not select the ordinary walk-speed column while combat stance is set.
Creature combat-state update 0x6a7f30 passes the state into 0xa631d0 at 0x6a7f97,
which sets locomotion byte +0xf9.

OpenThedas already selected the normal combat column in `move_speed`, but
`command_speed` bypassed it for a walk command. Origins walk commands now use
`move_speed` while their owner is in combat. Ordinary walking keeps its existing
walk-column selection. DA2 is unchanged because this trace covers Origins only.

The synthetic appearance table in the speed regression gives walk 1.54, run 6.0,
and combat 4.66 m/s. It verifies both combat gait choices, both ordinary gait
choices, and preservation of the existing DA2 walk behavior.

This does not implement acceleration, the alternate-stance flag, or the follow
catch-up bonus. Those remaining parts of SIM-005 stay open.
