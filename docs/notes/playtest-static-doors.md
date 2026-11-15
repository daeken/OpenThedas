# Dust Town scenery doors: interaction flag

DAOrigins.exe, GOG 1.05a, SHA-256
`2346caee611002d4dff6ecef68eb315b63a37004dd98b39c11a9c26c0b45bfb3`.
Addresses below are virtual addresses. This is a static trace, not an original-game run.

The placeable reader reads `InitialState` into its state word at 0x7506c7,
then reads the `Useable` byte at 0x7506f0. It converts that byte to a boolean
and passes it to 0x69ba70 at 0x750705. That setter writes the object's interaction
byte at +0x42c and updates its interaction registration. Script changes use the
same setter (0x66c176). `Useable` is therefore an initial interaction flag,
independent of the door's locked state; it is not merely editor metadata.

Dust Town's `orzip_static_slum_door` objects have `Useable=0`,
`InitialState=1`, and `AnimContState=1`. They are scenery in the native game.
OpenThedas copied the integer field but left `Object::interactive` at its default
true. Its picking and hover paths consequently offered those doors to the player.

Origins now initializes interaction after loading the template and any placed
instance overrides, including objects created from templates. Scripts can still
change interaction afterwards. DA2 retains its existing initialization.

Save revision 9 repairs the old default on Origins placeables with `Useable=0`.
It preserves other objects and all interaction choices in revision 9 saves. An
explicit script enable in an older save cannot be distinguished from the old
incorrect default and is reset; this is a limitation of the migration.
