# DA2 basic-attack animation movement

The real-prologue audit found `ABILITY_CAST_START` and `ABILITY_CAST_IMPACT`
for Hawke's 3310 without any resulting health loss. The shipped
`dae_basic_attacks` script does reach its ordinary impact handler. It queries
creatures within 2.5 metres of the caster, then checks the attack arc; at the
recorded impacts the query contains Hawke and Bethany but no hostile. Changing
the damage multiplier cannot repair that missing recipient.

## Native movement path

Read-only inspection of the player's DA2 analysis image establishes:

- Modern preparation at 0x640e43 reads ability column 0x650e5503. Bit 0 becomes
  task boolean 7 and bit 1 becomes task boolean 8 (0x64151d..0x641534). The
  modern executor copies them to animation task 37 at 0x6d85b7..0x6d85e2.
- Task 37's processor at 0x6d8860 supplies an animation-controller movement
  request, through 0x52ec70. Animation locomotion is separate from the cast's
  impact event and its remaining tail.
- The GAD resource, rather than the ANI pose, supplies this motion. Function
  0x90f5c0 checks GAD field 4008. Function 0x911b50 finds `gob_translation`
  in field 4005, resolves either a string or its DA2 hash, and samples the flat
  field-4004 key list through 0x90c900. Its key stride is four floats: seconds
  and XYZ. Function 0x911d10 rotates that displacement by the initial transform,
  scales it, and adds it to the movement origin.
- At 0x6d8f64..0x6d9003, a range class above close range uses the forward
  endpoint to scale travel to the target distance minus the close-range
  minimum and the two objects' bounds. The minimum is column 0x6e4fa70c of
  combat_range row 1, separate from the class's admission maximum.

3310 is the long-range two-handed attack: range class 3, animation 198,
action `attk_lng_rng_v1`. The humanoid two-handed tree resolves it to
`longrangeattack`. Its admitted GAD motion advances about 4.482 metres before
holding that endpoint. Our executor had read its duration and impact marker
but discarded the entire movement curve, so a valid long-range cast could
play in place and hit nobody. Medium-range attacks use the same mechanism.

## Simulation repair and limits

DA2 now samples the admitted GAD translation on the same playback clock as
the cast, honoring the ability's movement bit and native range scaling. Its
initial transform, curve, scale and clock survive snapshots. Movement runs
before the impact script; changing the command's deadline to its animation
tail does not restart the curve. Completing, interrupting, replacing or
canceling the command stops its motion. No extra damage recipient, attack
bonus, health adjustment or ability-specific 3310 override is introduced.

Translation uses the existing headless body sliding, walkable-grid clipping
and closed-door collision approximations, with ordinary trigger crossings.
This is not a complete native PhysX controller or animation blend graph.
Root rotation, native controller blending, arbitrary target-node bounds,
vertical steps and exact collision response remain separate work. The
existing turn-to-target approximation supplies the initial orientation.
Origins neither creates nor advances this DA2 motion state; its absent
optional save field remains omitted.

Synthetic regressions cover the admitted flat GAD curve, timed interpolation,
the close-range endpoint, motion before impact, save/reload halfway through
the lunge, continued animation tail, interruption and Origins isolation.
Installed-resource replay and regression-gate results are recorded with the PR.
