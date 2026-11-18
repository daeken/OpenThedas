# DA2 persistent sphere geometry

Dragon Age II's persistent-area loader at `0x541400` reads M2DA 96. It
compares the integer shape column `0x4de70e62` with 1 and passes the
float radius from `0xad5fb9f8` to `0x52caf0`, which stores it for
the sphere. The regular creature membership check
(`0x55e723`, `0x55e8f3` and `0x52c9c0`) tests the 3D distance
against the sphere radius plus the creature's personal space.

The installed worksheet uses that numeric schema for the persistent
auras attached to prologue hurlocks (rows 400005, 400009, 400021 and
400036). The Origins reader expects a string shape, so these were
created with `AoeShape::None` and never acquired members. A new DA2
sphere reads the numeric row and retains the existing event and
lifetime handling. Loading a historical DA2 snapshot recovers the
sphere from its saved row. The next scan computes membership using the
saved positions. The save repair does not reissue the creation event.

This change covers shape 1 and the stationary membership test. The
native swept test for a moving sphere, shapes 2 and 3, and the
script-facing area flags and ability query are separate paths. The
Origins path continues to read the string geometry used by its data.
