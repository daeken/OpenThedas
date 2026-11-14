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
implement node attachment or projectile motion. If an old save
or a script has no captured context, impact retains the previous zero result;
native context allocation defaults remain untraced. Item identities currently
use the simulation's item-tag command representation.

## Default attacks and script selection

DA2's CommandAttack constructor at 0x4dda50 creates CommandUseAbility (25),
integer 0 default ability 1 and object 0 the target. The forced attack-result
argument belongs only to its unused legacy fallback. Both script constructors
and player orders now use this modern path.

Ability 1 is a placeholder, not a concrete damaging swing. Native preparation at
0x640d6d sends CHOOSE_BASIC_ATTACK (111) synchronously to the creature's script:
creator/caster, objects caster/target/invalid, vector 0 the target point, and
integer 2 command boolean 2. The ordinary constructor leaves that boolean false.
CommandResult is cleared first. The shipped ability_core script chooses the
actual attack from the combat range, arc and combo tables and writes its ID to
CommandResult. Native 0x640e34 writes that ID back into command integer 0; a
result of 1 takes the no-action branch. The player-input path at 0x47fb83 uses
the same event. No custom combo cycle or replacement damage formula is needed.

The simulation retains its existing approach planner, asks for selection once
in reach, stores the chosen ability and target point on the command, then uses
the modern START/IMPACT pipeline above. A script can prepend an animation during
selection; its command must survive removal of the placeholder. A saved concrete
attack is not selected again on reload. Origins' constructor remains unchanged.
Completing the existing approach also marks a modern default attack as arrived;
without that handoff it can repeat a zero-distance approach indefinitely instead
of entering selection. The regression test exercises walking into the selection
script and retaining the command it prepends.

The modern default-attack path also retains the legacy executor's existing
turn-to-target behavior before script selection. Without this handoff, a
close attack can execute while the actor still faces the last movement heading;
the authored narrow attack arc then excludes its own target. This uses the
existing instantaneous turn approximation, not a newly inferred native turn rate.

A normal checkpoint probe now selects 3210 (two-handed medium front) from a
request for default ability 1. Its scripted impact deals 8.7422, 4.3711 and 2.1855
damage across three targets, with no script errors. The earlier placeholder-only
probe dealt no hero damage. These are engine execution checks, not a difficulty
comparison with a running original game.

Remaining approximations: approach/turn scheduling and target attachments are
not a full native movement controller; alternate combo input has no command
boolean representation; completion of a no-action choice uses the existing
simulation command-completion delay. DA2 animation timing still falls back when
its modern animation data is not resolved. Projectile release still lands at
once, and native flight/collision/lifespan remain separate work.
