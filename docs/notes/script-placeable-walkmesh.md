# Placeable health writes and walkmeshes

Origins `SetPlaceableHealth` reaches 0x74e8e0. On a nonpositive signed-short
write it checks table 93, column hash 0x1deaa1ea, with the unsigned low
16 bits of appearance. Any nonzero integer enables this destruction path;
missing cells disable it. This is separate from `IsDestroyable`. Four
shipped rows enable it: the cadaver/table/fence barricades and destructible
large fence gate (227, 367, 368, 404). Ordinary doors and chests do not.

Before assigning current health, it calls 0x74dba0 for the OLD state with
operation 1. That routine rejects an out-of-range unsigned-short state, an
original physics group containing bit 1 unless the appearance type byte
(column 0x5010edc3) is 1, and already nonpositive current on a destructive
appearance. Repeated dead writes therefore cannot erase another object's
later pathing-cell writes. Positive writes do not restore pathing or physics.

The resource loader 0x7500e0 combines the appearance's ModelName with the
ASCII template's original Appearance integer as `ModelName_Appearance.pwk`.
The table index truncates to 16 bits; the filename's number does not. Its
root GFF4 list 3016 holds grids in state order; each state has NAVI field
3110 with dimensions, cell size/base, data and heights (3086–3093).
Absent grids retain their state index. This call is not a visual effect.

Operation 1 (0xa46cc0) visits every source cell center, rotates/translates
it by the object's placement and maps it to the area's grid. Source cell
kinds do not mask this visit. Only destination kinds 7/8 become kind 1;
other kinds, heights and surface materials survive. Coordinates use the
native half-cell adjustment and truncation. Operation 0 is different: it
copies source kinds into destination cells below 8. It remains unimplemented
with the ordinary state-change walkmesh installation path.

The following 0x74d350(0) writes physics group **1**, including on repeated
dead writes, through the model's physics object. It does not clear Active.
The PhysicsObject vtable (RTTI, 0xb2724c) resolves group getter +0x50 to
0x839080 and setter +0x58 to 0x8398a0. The getter takes the first nonzero
actor mask; actor getter 0x81e6c0 unions shape masks. The setter reaches
actor 0x81e5f0 and replaces each shape's group-mask low word, zeroing its
other words. Bit 1 is Items, so a group-1 object no longer belongs to the
Placeables/StaticGeometry sight mask. Saved initial group (+0x6d8) supplies
the earlier walkmesh gate; it is not overwritten by this runtime override.

The implementation reads original actor masks from PHY collision objects,
keeps the runtime group separately and saves sparse pathing writes by area.
Layout resources stay shared and immutable. Reload/restore drops derived
area-grid caches and reconstructs them from the saved writes; another area
using the same layout is unaffected. The existing active-placeable sight
cache includes only objects whose runtime group intersects the sight mask.

Animation completion 0x74d200 remains separate: after checking current and
the appearance predicate, it changes byte +0x18d and requests destruction
after 3000 ms. That byte targets opacity: 0x695a60 reads the target, while
0x695a90 approaches it from the current opacity using the client update
delta divided by 500 and clamps the result to [0, 1]. The immediate-opacity
path is 0x695c20; completion uses 0x695c00 instead. This is separate from
Active and must not be replaced by immediate hiding. No timer, animation
completion or object removal is inferred from this health write. The
state-controller animation/completion path, fading, state-change operation
0, full physics and DA2 remain open under SCR-009.

Validation includes synthetic transformed grids, null state entries and
malformed-grid rejection, plus all four real destructive appearances on
isolated grids. The install-backed regression checks positive writes,
first lethal writes, unsigned-short state lookup, full filename numbers,
repeat-dead writes after another grid write, independent layout instances,
snapshot reload and the absence of health-triggered destruction scheduling.
No game resources or executable material are included in this note or tests.
