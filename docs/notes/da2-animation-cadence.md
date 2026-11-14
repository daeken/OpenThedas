# DA2 animation cadence and supply containers

This records a read-only trace of the DA2 executable and shipped resources. No
original-game session was run. Addresses identify the traced executable, not a
portable ABI.

## Modern ability timing

The modern ability executor at `0x6d7ec0` reads the ability's Animation column
(`0x43c8d895`), resolves its action through M2DA 0 (`0x7a398af9`), and queues
animation task 37. Its processor is `0x6d8860`. Combat style M2DA 300 supplies the
combat tree (`0xb4214584`); creature appearance M2DA 2 supplies a fallback combat
tree (`0xde80b418`). This implements those two tree sources and the existing
ANB action resolver, not the entire native blend graph or attachment system.

GAD duration, tree/GAD playback speed and EVT impact/release events 7/9 determine
the impact deadline. EVT 41 determines the cancellable marker. Native animation
event handler `0x542a70` records that marker in task integer 10; event 42 resets
the task's ready state. The task processor at `0x6d95e1` waits another 100 ms and
`0x69e390` sends event 112 once. Its creator is the caster, object slots are
caster/target, and integer slots are command type, 1, ability, 1.

The shipped rules_core handler forwards this notification to the next AI round.
The native command consumer at `0x5cd5a0` replaces a ready animation when another
command is waiting. An unqueued animation continues to its end. The simulation
therefore saves three separate deadlines/states: impact, readiness and end.
Readiness does not itself repeat an attack or finish an unqueued command.
Interrupting before impact suppresses that command's pending impact.

M2DA 3000 tuning is resolved by `0x8c0dd0` / `0x8bf0c0`: generic, tree, action,
then tree/action overlays. Loader `0x8a1bb0` fills eight nonnegative floats:

| Record offset | Column hash | Use in this implementation |
| --- | --- | --- |
| 0x00 | 0x4dcadd9b | retained in overlay only |
| 0x04 | 0x311c2dfc | retained in overlay only |
| 0x08 | 0xb4283d52 | playback rate minimum |
| 0x0c | 0xdef2da69 | playback rate maximum |
| 0x10 | 0x21732cb1 | end-tail offset |
| 0x14 | 0x4d28f736 | retained in overlay only (task integer 9) |
| 0x18 | 0xc3930717 | cancellable deadline override (task integer 10) |
| 0x1c | 0x82cc8766 | retained in overlay only |

The stack record starts at `esp+0xe4` in `0x6d8860`: the tail at `esp+0xf4`
feeds `0x6d8af5`, while `esp+0xfc` feeds `0x6d8bc1`. The adjacent task-integer-9
field must not be treated as the tail. Modern executor `0x6d8448` supplies the
additional `1 + property42/100` playback factor. Random rate ranges use the
simulation's saved RNG. Full task-loop/blend behavior, multiple impact markers,
event-42 resets and native random-number parity remain unfinished. End is kept
at least as late as the selected impact in the single-impact headless model.

The MCP attack helper also observes completed-command history, so an authored
AI chain targeting the same creature does not make the helper wait its entire
10-second bound after the requested swing has already completed.

## Supply corpse interaction

The prologue's supply corpses have StateController 5. Looking up the Origins
appearance-based controller fails for them and falls back to USE (5), which
placeable_core does not treat as opening a container. Consequently the authored
health potion remains in the corpse.

Native loader `0x8c73e0` selects registry M2DA 11100, reads TableIndex hash
`0x8f291c52` for the controller, and loads that table's state/action records.
Controller 5 selects M2DA 11004. State 0 has action 11 (open inventory); state 1
has action 3 (unlock) leading to state 0. A transition of -1 is absent, not a
new state. The DA2 lookup now follows this chain; Origins keeps its existing
appearance lookup. Native GetPlaceableAction (`0x4e9820`) returns a cached
cursor action; our use command resolves the default action directly. Alternate
cursor/action selection remains outside this change.

## Remaining difficulty gaps

Ordinary ranged ability impacts still land at release. Script-created shots
still use the existing fixed 20 m/s deadline. Native table-4 initial speed,
acceleration and bounds differ: common staff shots start at 50 m/s, accelerate
at 10, and clamp to 15–100; Arcane Bolt is 60 m/s. Flight applies to both sides,
so its absence is not by itself proof of a one-sided enemy advantage. Native
per-update homing, gravity, alternate collision sweeps and lifespan must be
reconstructed before replacing it. No constant native timestep or damage buff
is inferred from these tables.
