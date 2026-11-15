# DA2 stored combat target

The post-Ogre trace in `da2-post-ogre-diagnosis.md` found a living Bethany
executing a modern attack against a living hostile while `GetAttackTarget`
returned invalid. The old implementation only recognized a front
`CommandAttack`; DA2 replaces that command with `CommandUseAbility`.

## Native lifecycle

Command 522 dispatches through 0x4e1cf0 to 0x4dd1a0. The getter resolves a
creature and reads the target in its combat record. It does not consult the
command queue or validate the target's existence, life or hostility.

The preparation table initialized by 0x6829a0 assigns legacy attack preparation
to 0x6409b0 and modern ability preparation to 0x640c40. They write the same
stored target at 0x640a12..0x640a18 and 0x641010..0x641016. Modern preparation
first selects a concrete basic attack when given placeholder ability 1; a
selection of 1 is a no-action, before the target write. It also rejects missing
ability/target records before that write. Later script-side `COMMAND_PENDING`
failure is a separate phase, not a rollback of preparation.

The modern write preserves self casts. It also preserves casts where both
objects belong to the party **pool**, checked by 0x5c2ce0 against the party's
pool list. This is not a hostility test, an ability-category test, or just the
active-party list. GetPartyPoolList's 0x520b80 -> 0x51ae00 reads the same list;
GetPartyList's 0x520c70 -> 0x51aeb0 reads the different active list. Other admitted
casts replace the target, including a ground cast's invalid object id.
SetFollowerState's 0x5f3280 adds/updates pool membership through 0x5f10e0 for
active, available, unavailable, suspended and locked-active states. In
particular, unavailable recruits remain in this pool; the simulator's narrower
travel pool is not an exact substitute. Adding the hero also enters this pool
(0x5f11d6..0x5f11e2 updates the hero reference after insertion).

The completion table assigns modern abilities to 0x676160. Its
0x676288..0x6762a2 clear runs only outside combat for an active party member or
non-party follower. The cached membership flag read by 0x52e6d0 is populated at
0x5a05bc..0x5a05ed from the active-member cache and non-party-follower list.
The active-member cache is set at 0x5a0098..0x5a00cb using the same membership
lookup as IsFollower (0x4ea1f0 -> 0x5c2e40), not the preparation pool lookup.
Completion during combat preserves the target.

SetCombatState dispatches through 0x4e8b60 to 0x583c70. A transition from combat
to exploration invokes 0x579dd0, which clears the target immediately. Repeating
the current state does not invoke the transition. Emptying a queue by itself is
not a general target clear; modern ability completion still has its conditional
outside-combat clear.

## Engine change and scope

DA2 now stores the prepared id in `CombatData`, reads it through the script
getter, preserves self/pool casts, and applies the native completion and
combat-exit clears. Legacy DA2 attack preparation writes the same field. The
existing death transition also leaves no combat target when leaving combat.
No target validity filter is added to the getter: authored scripts own that
decision. Old snapshots default to no stored target; nothing invents historical
preparation when loading an old save. An absent target is omitted from JSON.
Origins keeps its existing queue-based getter and does not populate the field.

Preparation uses the simulator's existing command-start/approach path. That path
still has simplified capability/admission checks and turns/walks at its existing
points; this change does not claim a complete native command-executor port.
The presentation's `attack_target` accessor remains the view of the currently
drawn legacy swing, distinct from the script API's persistent target.

Synthetic regressions exercise the actual script getter/setter with selected
modern abilities, self/active/reserve casts, other friendly targets, ground
casts, placeholder/missing records, pending failure, completion and cancellation,
combat exit, repeated states, stale/dead target ids, old/save round trips and
Origins isolation. Installed-resource A/B replay must report the first changed
authored decision and the outcome, without treating this missing AI input as a
proven sole cause of any wipe. No damage, health, cooldown, difficulty or stat
adjustment belongs in this repair.
