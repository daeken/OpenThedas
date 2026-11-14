# Origins creature maximum-health query (SCR-026)

Static Origins 1.05 trace. This change covers the creature path of
`Deprecated_GetMaxHealth`.

The script handler at `0x65a2c0` obtains a CCObject through `0x5d5be0` and
calls virtual slot `+0x128`. CCCreature's vtable at `0xb10484` selects
`0x6ab530`. That method obtains the creature's property container, checks
that property 7 exists, and calls the property's total getter at virtual
slot `+0x0c`. Missing containers or properties produce zero.

The total is narrowed to a 32-bit float and converted through `0x8e34f0`.
Its SSE2 path truncates toward zero and produces integer indefinite
(`i32::MIN`) for NaN, infinity or an out-of-range value. The historical
x87 fallback remains outside this implementation's parity claim.

OpenThedas now uses its existing bounded property-total calculation for
Origins creatures. This includes derivation, equipment and effect modifiers.
The query requires a stored health property, matching the native existence
check. It reads the live total on each call and adds no saved state.

The other virtual implementations and DA2's handler still need tracing
through construction and loading. Their existing paths remain provisional
under SCR-026. The creature setter and its separate current floor are
covered by the [health-current trace](script-health-current-floor.md).
