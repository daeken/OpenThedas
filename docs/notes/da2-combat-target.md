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
Origins now also uses this field, with its separately traced preparation and
clear rules; see [Origins target lifetime](origins-combat-target.md).

Preparation records the target before approach and before `COMMAND_PENDING`;
basic attack selection now also precedes approach. A per-command marker survives
save/reload, so reaching the target does not undo a combat-exit clear that
happened during the walk. The command path still has simplified native
capability/admission checks; this change does not claim a complete native
command-executor port.
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

## Installed-resource replay

In the post-Ogre checkpoint, the first matched changed getter is Bethany's
`rules_core` call at t=861964: invalid on baseline, living hostile 540 with this
repair. The script now validates that target and follows its hostile-target
branch instead of attempting a cluster target and falling through to another
tactic. Preparation also emits basic-attack selection before approach, so the
first event-order difference occurs earlier, at t=860350.

Replaying all 662 baseline requests literally leaves the candidate alive in
combat at t=885350; baseline had wiped at t=900750. These are input-count
endpoints, not matched elapsed times: game-over handling and commands affect
time advancement. Extending the candidate with authored party AI clears the
fight at t=958350. Independently rerunning the original conditional driver
clears it at t=956950. Both candidate runs have zero script errors, without
developer healing, direct kills, activation or plot forcing.

The earlier pre-Ogre checkpoint remains a separate limitation. The conditional
driver still wipes (t=915150 versus baseline t=903150). Its literal-input replay
reaches an ended Innocent Blood conversation that the old input list never
closes, then stalls in combat. This is evidence of changed AI input and a
successful post-Ogre cleanup, not a claim that the entire prologue or its wipe
causes are resolved.

The corrected Chantry driver stops walking to the old Hightown door when
Anders's dialogue transitions inside. Starting from this consistent arrival,
the activation-only branch still wipes at t=1414127. A local integration of
activation and this target repair wins Karl's fight, follows the final Karl
dialogue, returns to Darktown and finishes Anders's explanation at t=1504060.
The hero, Bethany and Varric survive; Anders joins the active followers. A
second run from the preceding Hightown checkpoint reproduces this endpoint
with zero script errors. No developer healing, kill aids, activation, plot
forcing or forced travel is used. DA2 cinematic placement and processing
suspension remain outside both repairs.
