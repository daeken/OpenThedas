# Origins cluster health exclusion (SCR-024)

Static Origins 1.05 trace. `GetClusterCenter` calls the candidate creature's
virtual slot `+0x1d0` at `0x671d6a..0x671d79` before scoring a center and
again at `0x671de7..0x671df8` before admitting another member. A result of
one excludes that candidate.

CCCreature's vtable at `0xb10484` selects `0x6a3660` for this slot. That
method checks the creature's property container, invokes current-health
virtual `+0x120`, and returns one when the resulting signed integer is at
most zero. A missing container returns one directly.

The current-health virtual is `0x6ab4b0`. It obtains property 7 and calls
its current-value getter at `+0x24`. A missing property produces zero.
The value is narrowed to a 32-bit float before conversion through
`0x8e34f0`. Its SSE2 path truncates toward zero and yields integer
indefinite for NaN, infinities and overflow. Consequently an admitted
candidate has current health at least one and below 2147483648. This
predicate also covers fractional positive health below one.

OpenThedas now applies that predicate to both Origins center and member
selection, keeping each candidate's position in the distance table. DA2's
predicate remains untraced. Native perception-list limits also remain
under SCR-024. The historical x87 conversion fallback needs separate
verification.
