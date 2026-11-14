# Origins base-object maximum health (SCR-026)

Read-only static Origins 1.05 analysis. This covers the base-object paths of
`Deprecated_GetMaxHealth` and `SetMaxHealth`, including failed lookups. It
does not establish the placeable's separate health storage or lifecycle.

The getter at `0x65a2c0` initializes its answer to -1, resolves a CCObject
through `0x5d5be0` and, on success, calls virtual `+0x128`. The setter at
`0x65a330` performs the same lookup, narrows its input to signed 16 bits
and calls virtual `+0x12c`. A failed lookup leaves the getter at -1 and
the setter does nothing.

The lookup calls the resolved game object's virtual `+0x18`. The module,
area and party vtables select `0x4137b0`, which returns null. Their other
vtable slots therefore must not be interpreted as CCObject health methods.
Missing object IDs likewise fail the lookup.

Items, triggers (including the trap, transition and audio subclasses),
sounds (including pannable sounds), waypoints, projectiles, stores and
area effects instead return the CCObject through `0x79af30`. All select
base getter `0x4e13d0` and setter `0x455850`. The getter returns -1. The
setter consumes its argument without reading or writing the object.
Neither consults a `Health` or `MaxHealth` field or property 7; neither
creates a property when absent. Dead, active and immortal flags have no
effect on these answers.

The vtables checked are item `0xb1c61c`, trigger `0xb1fef4`, sound
`0xb2208c`, waypoint `0xb227cc`, projectile `0xb23bb4`, store `0xb242d4`
and area effect `0xb23854`. Module `0xb01e5c`, area `0xb0de04` and party
`0xb05e84` have the rejecting cast described above. Creature and placeable
health virtuals are different implementations.

OpenThedas now returns -1 and ignores the setter for these traced Origins
kinds and missing IDs. Regression coverage includes misleading fields,
absent and existing health properties, modifiers, integer limits, flags,
save/reload and preserved DA2 behavior. Stage, map and map-pin paths remain
untraced. This slice does not change creature or placeable handling, the
save format, tables or resource ordering. The remaining health/status
questions stay under SCR-026.
