# Origins MoveToObject range and completion

Statically traced in DAOrigins.exe; only the interpretation is recorded here.

The builder at 0x66f260 retains minimum and maximum separately, raises the maximum to
the minimum at 0x66f312, and stores maximum in native float slot 0 and minimum in
slot 1. The executor normalizes negative bounds and their ordering at 0x7a2270.

The executor 0x7aa200 has two distinct completion checks:

- On first execution, 0x7aa440 calls 0x7a3090 against the target position. A horizontal
  distance between max(0, minimum - 0.05) and maximum + 0.05 succeeds immediately
  (0x7aa47b returns 1), subject to the optional line-of-sight check. No walk starts.
- Otherwise 0x7aa4b9 calls 0x7a6ca0. For the ordinary zero native float-4 case, that
  calls 0x7a31c0 with both bounds: the goal is minimum metres from the target toward
  the mover, in the horizontal plane. It goes through 0x73e370 with search radius
  max(0.5, maximum - minimum), check off and snap on. The chosen goal is stored in
  native vector slot 1.
- Native float slot 2 is maximum - minimum (0x7aa382..0x7aa3d5). The ongoing walk
  delegates to 0x7a9130. At 0x7a9622..0x7a9654 it tests horizontal distance to the
  stored goal, minimum zero and maximum float slot 2. With executor flags 7, an
  out-of-range update returns in-progress (0); entry into that goal tolerance returns
  success (1) and stops locomotion at 0x7a976a. It does not intentionally continue to
  the inner bound after this test succeeds.

Along a straight approach to an unchanged goal this permits stopping at the outer
bound. Around a corner, the goal-centered disc and the target-centered disc differ.
OpenThedas previously tested only the latter during every walking update and also
walked commands that started inside range. The saved command location now retains
the ordinary Origins locomotion goal; completion uses the range width around it.
DA2 and old saved commands without a maximum retain their existing approximation.

Still provisional: the nonzero float-4 line-of-sight goal search at 0x7a5450,
0x73e7f0's failed-safe-search fallback, moving-target goal updates at 0x7a50f0,
capsule-contact completion, and simulation tick granularity. The safe search keeps
the existing grid and body approximations.

## Why the previous gate delayed the Lothering introduction

The original #883 comparison was against main 0e3c29a3. A watched replay of its s13
checkpoint identifies the first differing completion precisely:

- Script `pre420tr_ambush`, caller trigger 5238 of the same tag, constructs a move at
  script offset 0x381 and adds it at 0x38e. Actor 5167 (`pre420cr_ambush`, template
  `hurlock_mace`) moves toward waypoint 5247 (`pre420wp_ambush`).
- At simulation time 1646363 the affected command has run=1, minimum 0.369507,
  use-original-movement=1 and maximum 2.7955196. The script derives minimum from
  0.5 times RandomFloat and maximum from minimum + 0.5 + 2 times RandomFloat;
  it queues further moves as other party members enter the trigger.
- This command completes at 1647382 on the old branch versus 1647682 on its main
  baseline. Its shorter stop changes subsequent combat, RNG, timings and positions;
  the gate's s14 hero checkpoint shifts from (256.3138,149.96753) to
  (249.93356,165.86203). The s15 harness then walks toward the same board from those
  different starting points.
- The introduction is owned by trigger `lot100tr_party_talk`, centered at
  (267.82733,154.11592). Its script handles ENTER (8), checks the party's
  `CAMP_EVENT_BEFORE_LOTHERING` flag and `IsFollower` on the entering creature,
  then starts `party_camp.dlg`. It does not issue MoveToObject or wait for the
  ambush command's completion or perception. On the old branch that walk misses the
  trigger and leaves the flag set; camp entry later consumes it, selecting the
  Lothering conversation instead of the nightmare conversation.

Thus a shorter walk is not itself evidence of a range bug. The correction above is
justified by the two executable completion paths, independently of whether a changed
gate route enters that story trigger. The renewed gate must report its actual story
flags and dialogue ordering rather than assuming that those are restored.
