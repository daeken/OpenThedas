# Arrival placement and the Lothering introduction

Static executable and shipped-script trace, plus isolated headless probes, 2026-11-15.
Addresses below refer to DAOrigins.exe. Game data and probe output remain outside this
repository in OpenThedas-testdata/codex-captures/sim.

## Why the introduction moved to camp in the #878 gate

The renewed comparison against main d87f9ef2 ended s14's highway-bandit fight at different
hero positions:

| Checkpoint | Hero position, metres |
| --- | --- |
| Main | (251.24857, 148.81361, 7.8125) |
| Arrival branch | (247.70212, 175.70909, 7.8125) |

Both runs first arrive in Lothering through `wmw_lot_south`, at
(252.55663, 119.14624, 7.853201), facing 357.5 degrees. After the arrival scripts and
bandit cutscene have run, both heroes are at (252.54123, 109.89848, 7.84654). The different
s14 endpoints therefore arise during the intervening fight, after the changed follower
placement and earlier combat/RNG history; they are not two different hero transition
targets.

s15 asks the harness to walk directly to the Chanter's Board, at approximately
(346.4626, 199.41118, 1.17261). Main finds a grid path. The branch checkpoint has no grid
path and `route_inner` returns `Routed::Straight`; `Sim::walk` then samples a direct line.
The command-path probe likewise has 198 waypoints for main's checkpoint and an empty
path for the branch checkpoint. The latter line bypasses both `lot100tr_party_talk` and
`lot100tr_talk_warning`. This is the existing SIM-012 fallback, not a changed pathfinder.

The party-talk trigger's outline is near x=264.31..271.30, y=150.82..157.32. Its shipped
script handles ENTER (8), requires `CAMP_EVENT_BEFORE_LOTHERING` and a controlled creator,
and starts `party_camp.dlg`. Main's watched ENTER has creator 118, the hero. The branch
does not enter it. Consequently the flag is still pending when camp selects its
conversation: the Lothering introduction takes priority over the nightmare. The optional
warning is absent for the same geometric reason; it has its own trigger.

Two controls isolate the cause:

- For each fixed s14 save, running s15 with either main or the arrival binary produces
  byte-identical sampled states and watched traces. Save loading does not call `arrive`.
- Swapping only the hero's saved position reverses whether the party-talk ENTER occurs.
  Main's state at the branch position misses it; the branch state at main's position
  enters it. The party/RNG/plot state is otherwise retained.

## What the executable does, and what remains uncertain

Transition placement 0x6c73c0 gives the controlled member the transition target. Followers
use 0x631d90's slot index and 0x7605b0 with flag 1, selecting the compact table at
0xc0d2f0. The outer 20 m search is 0x73e370. These facts support the arrival change; they
do not establish that the original would reproduce our subsequent fight or choose our
grid path. Formation mirroring, the formation helper's path adjustment, the physics
standable approximation and the native pathfinder remain separate parity limits.

The native path-start helper 0x73ea80 tries a direct reachability test and then a path
search. After a failed search it can attempt a physics-tested direct path, but only when
the endpoints' absolute height difference is at most 0.5 m (0x73ee88..0x73eead, constant
0xb1b7b4). Failed path statuses 5 and 8 reach 0x7a9880 in the move executor 0x7a9130,
which stops locomotion and returns -4 through 0x7a3190. There is no unconditional walk
through the failed route. The branch checkpoint's board goal is about 6.64 m lower, so
our unconditional fallback is inconsistent with this traced height guard.

The evidence does not justify changing arrival coordinates or forcing the introduction.
The confirmed correction belongs to SIM-012: distinguish this failed path from missing
navigation data and stop the move instead of inventing the tall direct descent. The
original's exact route from these synthetic post-combat coordinates is unverified.
