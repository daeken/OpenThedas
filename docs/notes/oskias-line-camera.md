# Oskias line timing and camera handoff

Report 984 uses `bdc130_oskias`. On the all-first-replies route, node 17 is
the bartender's line and references `41277.cut`. Its authored length is
3.216664 seconds. The English voice sample for strref 41277 is 2.976 seconds.
Node 18 follows through an empty reply and has its own inline cinematics.

## Executable trace

The evidence is the read-only DAOrigins.exe dump in
`DAODecomp/local/disassembly-llvm.txt`, from the GOG 1.05a build with SHA-256
`2346caee611002d4dff6ecef68eb315b63a37004dd98b39c11a9c26c0b45bfb3`.
Addresses are virtual addresses with image base `0x00400000`. This is static
analysis. The runtime measurements below come from OpenThedas.

- Line setup obtains the referenced or inline cut through `0x7f0960` at
  `0x68e53c`, attaches its camera at `0x68e553`, places its stage at
  `0x68e584`, and starts playback at `0x68e5a0`.
- The ordinary cinematic timer comes from the cut's length at `+0x8`, read
  at `0x68e5ee–0x68e5ff`. The special branch at `0x68e5c4–0x68e5e3` checks
  cut field 5018 and line field 12216. Node 17 has field 12216 clear and
  takes the ordinary length branch.
- Setup subtracts 0.3 seconds and clamps at zero at `0x68ebfc–0x68ec3d`.
  Timer expiry calls `0x7f1a40` at `0x68e210` to prepare responses. This
  makes responses available shortly before the authored end.
- The cinematic update tests completion through `0x7cd290` at
  `0x68de74`. Its automatic empty-response path is reached after that
  completion test, through `0x68ded1–0x68df57`. It then ticks the current
  cut at `0x68df79`. Automatic continuation follows cut completion.
- The completion helper accepts terminal state 6. With field 5018 enabled,
  it also accepts the timeline-end flag at `+0x18` with the counter
  at `+0xe8` clear. The tick sets that end flag at `0x7e4397–0x7e43ac`
  when its frame reaches the stored frame bound.
- Replacing the line releases the old cut through `0x7e10a0` at
  `0x68e42f`, destroys it, and clears the stored cut pointer before
  obtaining the next line's cut. The old cut therefore stops controlling
  the camera during the next line.

## Reproduction

The supplied `oskias_line_camera_log.py` reports simulation `time_ms`.
`Sim::tick` freezes that clock while a full cutscene plays. Consequently its
roughly 0.1-second difference around node 17 measures the unpaused world time
around the cut. The cut's `elapsed_ms` records its playback time. Capture
overhead and the live simulation's step cap affect wall-clock measurements.

A hidden-window run with silent voice processing reproduced node 18 while
`41277.cut` was still active. The UI used the shorter voice duration to take
the empty reply. The remaining cut continued to supply the bartender camera
and actor poses. The unvoiced run waited until the cut ended because its
reading time exceeded the cut length.

The UI presentation clock advances by the frame delta, while `Live::step`
caps each simulation step at 100 ms. Slow frames therefore increase the
overlap between a line advanced by voice time and the preceding cut's camera.

The supplied pad logger can stop progressing when its single A press skips a
line before the responses become ready. The capture driver follows the same
first-reply route and selects reply zero directly for actual choices. It
leaves empty replies to the normal UI continuation path.

## Fix and regression coverage

The UI records whether the current line owns the playing referenced cut.
Automatic continuation waits until that ownership ends. The query matches
the resource and the line that queued it, and includes final lines still
being presented after the conversation walker marks them ended.

The UI regression reproduces the shorter-voice failure and presentation
time advancing well ahead of the cut clock. It also exercises an unvoiced
line, a later line in another line's cut, reply choices, and explicit skip.
The query test checks final lines, a later line sharing the resource name,
a different resource, and a cut started by a script. The new UI assertion
failed with the original `Line::played` implementation and passed with the
completion guard.

Hidden-window all-first-reply observations on the fresh baseline saw node 18
while `41277.cut` remained active. The fixed run first showed node 18 after
the cut ended. The logger's world-clock differences were 0 ms and 32 ms
respectively, illustrating the stopped-clock measurement.
