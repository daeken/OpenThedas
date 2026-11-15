# DA2 post-Ogre wipe: matched trace and AI target gap

This is a diagnosis record, not a combat balance change or a claim that the
prologue now completes. All native analysis was read-only, from the user's local
analysis image; only our understanding is recorded here. The original game was
not run.

## Reproduction

The existing late-prologue checkpoint at t=839750 kills the Ogre at t=855350 and
wipes at t=903150. The surviving party at Ogre death is Hawke, Bethany and
Aveline. The driver uses ordinary attacks, learned abilities and an available
health potion. It does not edit plots, disable creatures, heal through developer
scripts, change difficulty or force a transition.

Watching the three party members and hurlock scripts reproduces the exact
t=903150 result with zero script errors. The entire final saved world is identical
to the earlier uninstrumented replay, including RNG, commands, queue, properties
and plots. Thus the additional engine-call logging has not changed the outcome.
The watched trace contains 76,153 entries over t=839750..903150, including 43,159
watched calls at or after Ogre death.

Repeating the identical watched driver with current main 623396b9 and with the
independent night-map candidate also gives t=903150. All three complete final
worlds are identical. The night-map patch does not change this combat result.

After Ogre death, 117 health-loss entries total 312.971 from Hawke, 95.681 from
Bethany and 115.858 from Aveline. These are health actually removed, including
clamped killing blows, not raw damage rolls. The three shield-hurlock instances
that engage late retain approximately 75.147, 63.398 and 36.945 health. Other
active hurlocks farther away must not be counted as damage dealers merely
because they are alive: source ids in the damage trace identify the participants.

## Accuracy agrees with the traced curve

Native command 861 at 0x4ed090 calls the rating conversion at 0x697c00. The
function reads the optimal rating from the level curve selected by M2DA 371/405,
then applies the below-optimal power curve or the above-optimal half-distance
curve. This is already implemented in `da2_percentage_from_scaled`; no new
accuracy constants are justified by this wipe.

The actual leveled party in this replay differs from the unallocated defaults in
`da2-stat-initialization.md`. At enemy level 3 the optimal attack rating is 68,
the knee is 0.6, and the ceiling is 1. Above the knee:

`fraction = 1 - 0.4 * 0.5^(rating / 68 - 1)`

The watched script receives 0.60405666 for Bethany's rating 69 (29 samples), and
0.6350631 for Hawke's rating 77 (24 samples). Independently evaluating the formula
gives 0.604056626 and 0.635063094, respectively. Against these melee hurlocks,
the script subtracts the target's 10-percentage-point defense bonus, giving
approximately 50.406% and 53.506% accuracy before other situational checks.
Frequent blocks are not evidence that these ratings should be inflated.

The first watched shot also sees its newly engaging target at level zero; the
normal perception script assigns level 3 before the next shot. This timing
observation alone does not establish a native scheduling mismatch and is not
grounds for a new level fallback.

## Confirmed AI interface mismatch

All 321 watched `GetAttackTarget` calls after Ogre death return OBJECT_INVALID,
including queries of actors that are executing modern ability attacks. The
current engine arm only accepts a front command named `CommandAttack`. DA2
converts that command into `CommandUseAbility`, so this getter cannot report its
normal attacks. It also loses the target as soon as the front command finishes.

A separate read-only inspection at t=859950 finds Bethany executing ability
5510 against living hostile 540, with impact due at t=860064. Querying
`GetAttackTarget(413)` then returns OBJECT_INVALID. This confirms the mismatch
during an active enemy-targeted attack, independently of queries made while
commands are being replaced.

That does not match the native getter:

- Shared script dispatcher 0x4e1cf0 handles command 522 by calling 0x4dd1a0.
- Getter 0x4dd1a0 resolves a creature and returns its combat record's stored
  target. It does not inspect the front command's name.
- Legacy attack preparation writes that target at 0x640a12..0x640a18.
- Modern ability preparation has a corresponding write at
  0x641010..0x641016. The surrounding checks preserve some self/party-targeted
  uses; a blanket assignment for every spell would require further care.
- Combat-mode processing includes an explicit clear at
  0x67629c..0x6762a2. This is state with a lifecycle, not just a view of the
  currently executing command.

The authored `rules_core` reads this getter in its target selection, tactic and
movement logic. For example, its follower-target helper at bytecode 0x2f4d5
first queries the existing attack target, validates it, and checks its racial
type before deciding whether to reuse it. Other call sites compare an enemy's
attack target to the acting party member. Returning invalid prevents these
branches from using information the native engine maintains.

This is a proven missing AI input. It is not yet a demonstrated sole cause of
the wipe. Changing targeting may change commands, RNG consumption and the fight
outcome, so a tactical replay with different inputs is not an A/B proof of a
damage-formula defect.

## Next bounded repair

Trace the remaining target-write/admission and clear conditions, then implement
DA2's stored combat target independently of the active command. Cover enemy
attacks, self and party abilities, command completion, combat exit, snapshot
round-trip and Origins isolation. Repeat the same driver and checkpoint on main
and that candidate, checking the first authored AI decision that changes as well
as the final result. Keep the night-map fix in its independent PR.

`IsMoving` is also still unimplemented on this route, but its presence in the
trace does not by itself prove that it caused the loss. Do not tune damage,
health, cooldowns, difficulty or party stats to compensate for either gap.

## Validation of this note

This branch changes documentation only and is based directly on main 623396b9.
Fmt and workspace all-target clippy with warnings denied pass; debug workspace
tests pass (1,170 passed, 16 ignored, zero failures). Its executable sources are
identical to the current-main binary used in the matched replay. The separate
night-map PR's DAO gate has 26 successful runs, no story-log or plot-sequence
differences, and 15 identical complete JSON checkpoints. No combat behavior fix
or original-game parity outcome is claimed by this note.
