# IsHumanoid (SCR-025)

Static trace of the GOG DAOrigins.exe, image base 0x400000; SHA-256
2346caee611002d4dff6ecef68eb315b63a37004dd98b39c11a9c26c0b45bfb3.

The script handler is 0x666450. It resolves the object, asks virtual slot +0x10
for its creature, and returns false when either is absent. For a creature it
calls 0x6a25a0, which returns false without an appearance object at creature
+0x7b0. Otherwise it reads the appearance's 16-bit type (+0x58) and calls
0x69f030 with that row number.

That helper reads a string from table 2 (`apr_base`), column hash 0xde80b418,
the combat animation tree. It returns false if the table lookup fails. It then
searches the string for `combat_humanoid` through 0x4c0f70, starting at zero,
and returns true precisely when the result is not -1. This is a substring test,
so the resource extension and other suffixes are allowed. The underlying search
0x4c03f0 compares UTF-16 code units and uses the comparison at 0x413800; neither
folds case. It is not a race, model-type or creature-kind classification.

OpenThedas now performs that lookup for creature objects. Invalid objects,
non-creatures, missing tables/rows and non-string cells answer false. Tests cover
the ordinary humanoid tree, a non-humanoid tree, case sensitivity, the substring
rule, empty/missing rows, the 16-bit appearance value and invalid object types.
No table data, worksheet merging or row positions are changed.
