# DA2 AI priority queries and script overrides

`GetAILevel`, command 221, is registered at `0x5267ae` to `0x4e5b10`.
It resolves a base game object and returns its signed byte at `+0x30c`;
an invalid object returns -1. This is AI scheduling priority, independent of
experience level and difficulty settings.

## Assignment and overrides

The constructor at `0x5dca6c` initializes the byte to -1. The setter virtual
at `0x52c7d0` writes it. `SetAILevel` reaches `0x4dbdf0`: -1 clears the manual
lock and invokes automatic assignment; every other integer asks the manager
at `0x662c80` to assign a bucket, then sets the manual lock. The manager
accepts only 0 through 4, refuses changes while locked, and routes -1 to
removal. Consequently an invalid non--1 request locks the current value,
and a second forced assignment cannot change it until -1 is passed.
Non-creature objects inherit an empty automatic-assignment virtual at
`0x52c7f0`, so unlocking them leaves their previous byte unchanged.

The creature override at `0x539ac0` supplies values without a script setter:

- The controlled creature gets 4, including before the death check.
- A creature in combat uses helper `0x60dbc0` with base 2, capped at 3.
- Otherwise either of the player's follower membership maps gives 3.
- Otherwise a creature in the controlled character's area uses the helper
  with base 1; another area or no controlled area gives -1.

The helper branches on death (`0x52e810`). Dead creatures get base minus one
and skip both living bonuses. Living creatures add the player-group flag
(group identifier 1, recalculated at `0x52f180`) and add one when model state
is **zero**. It clamps the result to 0 through 4. The model query `0x5a88e0`
returns model `+0x178` or 255 for an absent model.

## Simulation boundary

The getter evaluates automatic assignment from current simulation state.
Manual assignments and the lock use saved `AILevel` and `AILevelLocked`
fields. Older snapshots' `AILevel` values retain their override interpretation;
-1 remains automatic. A stored override is read as a signed byte. Unlocking a
non-creature explicitly clears the lock while retaining its stored value.

Simulated creatures use ordinary loaded model state zero. Native appearance
and fade transitions, absent models, and player-group inheritance through an
owner at `+0x920` are not represented. Activation and parking are independent
of the native model-state query and are not used as substitutes for it.
The unit helper exercises both model branches, but the automatic simulation
path is bounded to the ordinary-model case. Encounter 10008 contains ordinary
loaded creatures, with no summoned/owned actor. Native scheduler bucket timing
is outside this change; assignment here supplies the script-visible value.
Origins retains its previous setter and unimplemented getter.

## Coverage and validation

Regressions call the script getter/setter and cover automatic values without
a setter, invalid objects, controlled-character changes, followers, combat,
group, death, area removal, signed-byte reads, invalid requests, manual locks,
non-creature unlock and snapshot reload. A helper control distinguishes the
model branches and verifies death bypasses both living bonuses.

Matched story measurements and Origins regression results follow after the
merged candidate completes its checks.

No HP, damage, difficulty or AI frequency values are retuned.
