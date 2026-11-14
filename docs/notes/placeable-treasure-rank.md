# GetPlaceableTreasureRank (SCR-088)

Static trace of the GOG DAOrigins.exe, image base 0x400000; SHA-256
2346caee611002d4dff6ecef68eb315b63a37004dd98b39c11a9c26c0b45bfb3.

The script handler, 0x65f220, initializes its result to zero, resolves a
placeable through 0x5d5b40, and reads its 32-bit member at +0x6bc when present
(0x65f256). A missing object or another object type leaves the result zero.

The placeable constructor initializes this member to zero (0x74ef94). The
binary resource reader reads field 0x3f7d into it (0x74ff4b..0x74ff5e). The
named serializer reads `Rank` into that same member at 0x750982..0x75099b;
the next member, +0x6c0, is separately named `TreasureCategory`. Therefore
the existing use of the object's Rank was correct, rather than a guessed
stand-in for some different treasure-rank property.

OpenThedas already loads template and instance Rank into Object::rank, with
instances taking precedence. This change supplies the missing type guard:
only placeables return their Rank. Regression cases cover zero, positive and
negative ranks, creature/item objects with nonzero ranks, and an invalid ID.
No object layout, save format or treasure-generation policy changes.
