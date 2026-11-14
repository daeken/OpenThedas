# ActivateSoundByTag (SCR-036)

Static trace of the GOG DAOrigins.exe, image base 0x400000; SHA-256
2346caee611002d4dff6ecef68eb315b63a37004dd98b39c11a9c26c0b45bfb3.

The handler at 0x65d0c0 reads the tag and converts its integer argument to a
boolean with a nonzero test (0x65d15e..0x65d16a). It gets the current area and
calls its sound activation routine, 0x67c230. That routine walks every entry
of the area's sound-object list (+0x694/+0x698), resolves a sound object through
0x5d59b0, and compares its tag (+0xb8) with the requested tag at 0x67c2a4.

The comparison is 0x4c0de0, which calls the length-aware comparison 0x414170;
that calls 0x413800. The latter compares UTF-16 code units directly. No stage
folds case, so otherwise identical tags with different capitalization do not
match. Every matching object is activated, rather than just the first one.

For each match, 0x7c2190 changes the enabled byte (+0x4a0), calling 0x7c20f0
when enabling or 0x7c1ff0 when disabling; an unchanged byte does nothing.
OpenThedas stores the switch as the sound object's active flag. The app already
exports inactive sound tags to its audio snapshot and uses them to control the
area sound emitters; the previous source comment saying there was no audio
consumer was stale.

The implementation now compares tags exactly. Its regression fixture checks
two matching emitters, a tag differing only in case, an unrelated tag, an
emitter in another area, and nonzero integer enablement. No audio or UI source
files change.
