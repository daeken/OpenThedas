# SetCreatureMana current clamp (SCR-053)

Read-only static trace of the GOG Origins executable, image base 0x400000.

`SetCreatureMana` (0x66cd00) resolves a creature, obtains its property container,
selects property 8 and dispatches the integer input to the current-value setter
at virtual slot +0x2c. Non-creatures and missing properties are left alone.

The depletable property's vtable is 0xb210bc. Its current setter, 0x79c8d0,
reads its total through virtual +0x0c and the lower bound from member +0x78,
then passes the requested current, total and lower bound to 0x79c2d0. For finite
values this is `min(max(requested, lower), total)`. The total includes modifiers.
The first comparison selects the requested value or the lower bound. The
second selects the total whenever it is at most that result or unordered.
A total below the lower bound is returned directly. Thus a negative total
and a zero lower bound yield that negative total. An unordered total is
also returned. This corrects the earlier note's claim that the ceiling was
raised to the floor.

The lower bound is not universally zero. The depletable constructor 0x79ca30
initializes it to zero at 0x79ca60; clone 0x79c7d0 copies it. The virtual +0x30
setter, 0x79c490, writes it. The creature immortality setter 0x6ad130 uses that
slot specifically on property 7 (health), setting one for an immortal creature
and zero otherwise. SetImmortal's handler 0x659c50 reaches it through the
creature's virtual +0x16c (the corresponding vtable entry is 0xb105f0).
This is a health floor, not a mana floor.

The static callers of the floor setter and the writes in the depletable
implementation provide no path that changes property 8's zero floor. The
property serialization at 0x79ca80 does not load a lower-bound field either.
Mana's initialized zero floor therefore survives both construction and load.
Origins now checks that property 8 already exists, matching the handler's
checks at 0x66cd77..0x66cd90, and applies the traced floor-then-ceiling
selection. This corrects the previous creation of an absent property and
the zero result for negative totals. Tests exercise modded bounds, negative
inputs and totals, fractional modifiers, integer limits, absent properties,
non-creatures and the independent dead, active and immortal flags.

The separate health immortality floor is now stored independently of the
flag and shared by current writes and reclamps; see the
[creature health trace](script-health-current-floor.md). The remaining
status and lifecycle questions stay under SCR-026.

This trace establishes Origins behavior only. It does not establish DA2's
property implementation, whose existing path is retained.
