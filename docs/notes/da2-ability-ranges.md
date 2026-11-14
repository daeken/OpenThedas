# DA2 ability range classes

The modern command preparation at 0x640eb7 reads the ability's range class from
column 0xa7fa7789 of ability M2DA 1. At 0x641104 it reads that class's upper
distance (column 0x814f8b88) from combat_ranges, M2DA 301. The subsequent native
distance check also accounts for object bounds. At 0x64111b it calls the caster's
personal-space getter (virtual +0xa0), adding that radius as well as the target
bound to the upper range. This is separate from Origins'
ABI_base Range column and ranges worksheet 134.

Using the Origins column for DA2 answered zero, so the simulation treated casts
as personal/touch-range abilities and walked ranged casters up to the target.
The error also affected approach replanning. The getter and both approach tests
now use the native DA2 class and table. For example, staff attack 5510 has class
6, whose upper distance is 35 metres; an already-in-range mage should not walk
into melee to execute it. Origins retains its existing lookup.

The regression supplies conflicting Origins and DA2 data, verifies the DA2
caster stays put at 20 metres and approaches from 40, and checks personal,
missing and non-finite ranges, and a close-range cast that needs both radii.

The GetDistanceBetween wrapper at 0x4e2a00 calls 0x4db770. With the optional
personal-space flag, the latter subtracts the target's bound and the caster's
virtual +0xa0 personal space at 0x4db803..0x4db820. It does not clamp overlaps to
zero. The simulation now honors that flag for DA2 creature pairs, which lets
the shipped basic-attack selector use its authored close-range threshold.
Unflagged calls, Origins and other object pairs retain their existing behavior.
The regression checks distinct radii, 3D centre distance, overlaps, invalid
objects, area mismatch and Origins isolation. Native closest points for
non-creature bounds are not yet reproduced.

Existing object-bound and movement planning
approximations remain; this does not implement native target-node attachments,
animation timing or projectile flight.
