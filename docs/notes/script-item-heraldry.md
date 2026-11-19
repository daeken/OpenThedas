# Origins item heraldry natives

Read-only trace of the GOG Origins executable. LDF 171 (`GetItemHeraldry`,
`0x65d770`) resolves an item through `0x5d5870` and returns the unsigned byte at
`+0x46c`, or zero if the lookup fails. LDF 172 (`SetItemHeraldry`, `0x65d7d0`)
stores the low byte of the integer argument in the same field. This applies
whether or not the item has an owner; it is not a boolean or a clamped integer.

The item constructor calls `0xa66270` on `item+0x440`; it initializes the byte
at component `+0x2c` to zero. Its copy constructor (`0xa66320`) copies the byte.
The named `Heraldry` input (`0x75759c`) and GFF field `0x3f6d` (write `0x754596`,
read `0x7587d3`) use the same field. The simulator keeps it in the existing saved
item integer map, which already loads named integer fields from item templates.
The original stack comparison (`0x755a70`) does not compare this byte; no stack
rules are changed.

The native setter then looks up the owning creature and, if it has an appearance
object, performs a slot/appearance update (`0x7480d0`, `0x747d90`, `0x6c1400`).
This change does not implement that update or render heraldry. DA2 dispatch is
unchanged.
