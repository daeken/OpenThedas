# Origins generic ToString (SCR-031)

Static trace of Origins 1.05, image base `0x400000`; no original-game execution.
This is the generic `ToString(any)` handler at `0x668f30`, separate from
`ObjectToString`'s signed raw-ID formatter.

The variant pop at `0xa2f720` preserves its type through `0xa2db10`. The
handler recognizes only int (3), float (4) and object (6). Ints use the signed
formatter at `0x4c2b10`; floats use `0x4c2b80` with six decimal places. Every
other variant, including strings and resources, receives the fixed cast-failure
diagnostic selected at `0x669058`. These are not string identity conversions.
Legacy numeric rounding and the float helper's bounded output remain unverified.

The object branch (`0x669006`) resolves the raw ID through `0x5d5840`. A missing
object leaves the result empty. For an existing object it calls virtual member
`+0x18`. A nonnull result supplies a wide C string at `+0xbc`, with capacity
at `+0xd0` choosing inline or heap storage. This is the string inside CCObject's
tag wrapper at `+0xb8`: `GetTag` (`0x6596a0`) accesses that same wrapper.
`GetResRef` instead reads the template resource at `+0x1cc`. The C-string copy
stops at the first null; it has no 31-character float-buffer limit.

Primary RTTI vtables for CCCreature, CCPlaceable, CCItem, CCTrigger,
CCSoundObject, CCWayPoint, CProjectile, Store and CCAreaOfEffectObject all
select `0x79af30` at `+0x18`, returning the object itself. These map to the
simulation's creature, placeable, item, trigger, sound, waypoint, projectile,
store and area-of-effect kinds, which therefore return their tag.

CCModule (`0xb01e5c`), CCArea (`0xb0de04`), CCParty (`0xb05e84`),
SharedCharacterStage (`0xb1592c`), MapObject (`0xb22e04`) and MapLocation
(`0xb24814`) instead select `0x4137b0`, the null-returning member. These map
to module, area, party, stage, map and map-pin kinds. Their existing objects
take the fixed invalid-object fallback at `0x669051`, even when they have tags.
That differs from a missing object, which returns an empty string.

DA2 has not been traced and retains its previous conversions. This change
does not alter numeric conversions, object-ID allocation or object lookup.
