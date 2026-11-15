# Origins movement-speed fallback (SIM-006)

Static tracing confirms the existing 5.0 m/s fallback. No runtime change is needed.

Target-speed function 0xa63120 first initializes its local lookup output from the
5.0 constant at 0xae7308. For walk and run modes (1 and 2), its jump table selects
the appearance-table lookup at 0xa63174. The table wrapper 0xa2cc80 returns false
at 0xa2ccac if the requested table is unavailable; it does not write the output
parameter on that path. Therefore the initialized 5.0 survives that failed lookup.
At 0xa6319d the function multiplies the resulting base by the speed multiplier.

This is a lookup fallback, not the speed for every locomotion mode. Modes 0, 3
and 4 explicitly select zero at 0xa63170; a mode above 4 retains the initialized
fallback. A valid walk/run appearance row normally supplies the base instead.

OpenThedas' `DEFAULT_SPEED` is already 5.0, used when appearance speed is unavailable
and then multiplied by `speed_multiplier`. The existing synthetic appearance-table
test verifies the default for an object without an appearance speed. SIM-006 can
leave the provisional inventory; the comments and count are updated accordingly.

The table implementation's treatment of every invalid row or cell value was not
traced in this audit. Acceleration, alternate stance and the rest of the partial
locomotion model remain tracked by SIM-005 and the neighboring movement gaps.
