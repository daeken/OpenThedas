# DA2 ability cast context

DA2's modern ability executor at 0x6d7ec0 creates START event 32 at 0x6d80f4.
Its creator is the caster, objects 0/1/2 are caster/item/target, integer 0 is the
ability and integer 1 is the caster's pending command result (+0x2e4). Vector 0
is the horizontal normalized direction from the caster to the command's target
point, with +X for a zero direction.

`SetAbilityResult` is not just presentation. Its native handler 0x4dd360 reads the
current command and obtains the active ability context through 0x52d2f0. It copies
integer 0 (ability), object 0 (target), object 1 (item), vector 0 (target point),
the creature's current command result and the supplied projectile target node.
IMPACT event 31 at 0x6d8715 uses those saved values: objects caster/item/target,
integers ability/result, and vector 0 the saved target point. Later changes to the
creature's pending result do not change that impact.

The simulation now preserves this DA2 context in world snapshots and supplies
the native START/IMPACT result and vector. Origins keeps its existing event path.
The stored projectile node is available for future flight work; it does not
implement node attachment, combo selection or projectile motion. If an old save
or a script has no captured context, impact retains the previous zero result;
native context allocation defaults remain untraced. Item identities currently
use the simulation's item-tag command representation.
