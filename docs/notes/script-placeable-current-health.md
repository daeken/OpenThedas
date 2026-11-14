# Origins placeable current health (SCR-009)

Read-only static analysis of Origins 1.05 establishes the script setter at
`0x65a250`. It resolves CCObject through `0x5d5be0`, sign-extends the low
16 argument bits, and calls virtual `+0x124` with a second argument of one.
CCPlaceable's vtable `0xb1c084` selects `0x74e8e0`.

The placeable method assigns current member `+0x66c`. For a nonpositive
request it first counts the inventory reached through `+0x678`, using
`0x761220`, inventory virtual `+0x40` and `0x73f670`. An empty inventory
causes `0x69ba70(0)` to clear interaction byte `+0x42c`. Positive writes
retain interaction's existing value. Maximum member `+0x670`, plot and
immortal flags are independent of this write.

`IsDead` at `0x65a3a0` calls `0x695300`, which obtains the integer current
through virtual `+0x120` and compares it with zero. The placeable getter
`0x750f50` reads `+0x66c`. OpenThedas applies this predicate to Origins
placeables and stores current in the existing `Health` field. Explicit
property-7 currents from older OpenThedas saves retain precedence and are
updated alongside the field. The maximum and property base survive each
write. JSON saves and cloning already retain these values.

The shipped placeable script handles a lethal damage effect by queuing
its own DEATH event and setting current to zero. Its death handler selects
state 3 for ordinary door and container controllers. The handler also
halves multi-item stacks and can damage eligible equipment. Surviving
contents remain in the opened container and can be acquired through the
normal container interface. The setter supplies current and interaction
state while the script supplies the death event, opening and item damage.

The install-backed regression executes those shipped scripts with a door
and chest, both with template-only current and an explicit older saved
current. A lethal damage effect must yield current zero, `IsDead` true and
state 3. The chest's stack of eight becomes four, remains accessible, and
can be acquired by the hero. The empty door loses interaction. Death and
state survive JSON reload. The install-independent regressions cover
signed-short boundaries, old property currents, independent maximum/base,
flags, interaction, absence and game/type controls.

Further destruction behavior remains under SCR-009. For nonpositive
currents, `0x74e8e0` also evaluates appearance-table predicate `0x74d1b0`
(table 93, column hash `0x1deaa1ea`). When true it invokes `0x74dba0` for
the previous state and `0x74d350(0)`. Animation completion `0x74d200` and
forced state transitions `0x74f9d0` can request destruction through virtual
`+0x198`. Their effects, physics and scheduling need a separate mapping.
The current setter's creature/base-object virtuals, full native save
lifecycle and DA2 also remain open.
