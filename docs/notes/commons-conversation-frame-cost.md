# Commons conversation frame cost (#1033)

The conversation camera's stage and line-cut resources were already cached. The
steady frame cost is dominated by rendering and synchronization, rather than by
repeated parsing. This change removes two smaller sources of repeated allocation:
world tag searches compare ASCII case without allocating lowercase copies, and
shader constant lookup borrows its material usage string.

The conversation exit timer is a separate issue, handled by #1079. No timing or
conversation state changes are part of this work.

## Method

Local fixtures: report 1033's `save_before.json` and `save_now.json`. The report
requests a 1600×900 logical window at scale 2: its actual framebuffer is
3200×1800. It used Medium, faithful rendering and 4× MSAA. The tests use the same
Apple M4/Metal machine and release builds, with hidden windows, audio disabled,
no mods and a fixed shader/presentation clock.

The capture benchmark uses the window's live render/finish/UI path into a reused
offscreen target, with 33⅓ ms simulation/presentation steps. Each case starts
from its fixture; the first 30 frames are excluded from steady statistics.
Simulation is stopped when the selected screen stops it. The cases are the
first Kasch line, its next reply, free play at the report camera, journal,
skills, and a conversation after opening and closing journal and skills.

Temporary instrumentation records inclusive wall times and main-thread CPU
times for simulation, posing, camera, light picking, constants, rendering,
submission, waiting, and Scaleform updates/compositing. Nested times overlap;
posing also uses worker threads. Encoder GPU timestamps bracket world geometry,
post-processing and UI were instrumented for optional encoder timestamps, but
this adapter exposes pass timestamps only. The existing render profiler supplies
those; separate serial captures fence the remaining stages. The diagnostic waits
for each frame to complete. Its elapsed frame time is a serialized frame
budget, **not** a direct prediction of the presented frame rate: gameplay can
overlap CPU work and GPU work, and surface acquisition/presentation adds its own
timing.

Both old allocation paths can be selected independently in the same release
profiling binary. This permits paired before/after measurements without changing
scene data, instrumentation, shaders or build configuration. The temporary
switches, timers and benchmark loop are not included in the product change.
Raw logs, binary hashes, command lines, instrumentation patches and native-size
captures are retained locally under
`OpenThedas-testdata/codex-captures/commons-conversation-frame-cost/`.
The profiled base is `45e01895`; `instrumentation-4.patch`,
`frame_profile-4.rs`, `run_pairs.py` and `diagnostics.py` retain the exact
measurement setup. The final validation merge includes upstream `adf58096`
(the OpenGL-only MSAA availability change).

This was a shared, busy machine. Wall-time outliers must be interpreted alongside
main-thread CPU time and GPU timestamps. The earlier interrupted runs are kept
as evidence, but are not used as before/after optimization pairs.

The hidden control window has no drawable surface on this Mac: its screenshot
reports `presented: false`, and its ordinary update frames encode no world draw.
Consequently its title-bar FPS is not a rendered-frame measurement. The
offscreen benchmark is necessary to measure the complete frame while respecting
the hidden-window constraint. Control-screen measurements independently verify
the CPU update path.

The conversation control check starts from `save_before`, asserts
`{Conversation}` at both ends, and records 1,604 update frames (2.34 ms median,
with no world encoding). The earlier `save_now` control sequence supplies the
HUD, Journal and Skills samples; its attempt to restart Kasch did not open a
conversation, so that last phase is excluded.

## Measurements

### Frame budget

Each run has 200 frames, with 170 retained after warmup. The following are the
original allocation paths (`paired-before`), in milliseconds:

| Case | Mean | Median | p95 | Max |
| --- | ---: | ---: | ---: | ---: |
| Conversation | 74.28 | 68.06 | 112.47 | 411.24 |
| Next line/replies | 73.57 | 67.34 | 112.31 | 230.69 |
| Free play | 65.12 | 63.02 | 94.02 | 135.31 |
| Journal | 64.50 | 59.97 | 90.42 | 226.02 |
| Skills | 57.98 | 55.22 | 78.36 | 177.73 |
| Conversation after menus | 75.63 | 69.70 | 114.33 | 220.30 |

The means split as follows. Rows marked "within" are nested
measurements, not extra costs to add to the enclosing row.

| Mean wall ms/frame | Conversation | Free play | Journal | Skills |
| --- | ---: | ---: | ---: | ---: |
| Simulation | 1.274 | 0.823 | paused | paused |
| Scene synchronization | 4.400 | 3.710 | 3.928 | 3.794 |
| Posing, within synchronization | 2.643 | 2.327 | 2.321 | 2.128 |
| Conversation camera | 0.276 | 0.002 | 0.002 | 0.002 |
| World encoding, including constants and effects | 27.167 | 26.127 | 24.400 | 20.446 |
| Draw constants/preparation/upload, within encoding | 6.478 | 4.956 | 3.908 | 3.393 |
| Scaleform binding/update | 1.707 | 2.407 | 3.666 | 2.312 |
| Scaleform compositing/changed draw lists | 0.338 | 0.815 | 0.462 | 0.536 |
| Finish encoder and submit | 10.278 | 13.477 | 11.439 | 9.182 |
| Wait for GPU completion | 28.722 | 17.603 | 20.509 | 21.582 |

Portrait checks and loop overhead account for the remaining approximately
0.1 ms/frame. In the conversation, staging cache lookups total about 0.002 ms
for three calls/frame, line-cut lookups about 0.003 ms for two calls/frame.
Parsing occurs on the first use of a resource, not every frame. In the older,
interrupted run a first stage parse cost 89.9 ms; warmed lookups were then
microseconds. `set_extra_lights` costs about 0.001 ms/frame for two unchanged
sets; warmed light picking is also microseconds. No every-frame lighting cache
invalidation was observed.

For 20 warmed serial render captures, actual GPU timestamps have medians of
7.14 ms for the conversation's level-forward pass and 1.79 ms for its objects;
free play is 6.76 and 0.77 ms. The separately fenced resolve/post stage takes
13.74 ms in conversation versus 8.95 ms in free play. These last two numbers
include submission/wait overhead and are **not** GPU timestamps. They must not
be added to the ordinary frame's GPU wait a second time. The rendering work
and its driver/synchronization costs explain most of the steady budget.

### Allocation fixes

The single-fix runs retain the other original allocation path. Main-thread CPU
medians, ms/frame:

| Operation | Original | Its fix only | Both fixes, reverse-order repeat |
| --- | ---: | ---: | ---: |
| Conversation camera | 0.2270 | 0.0731 | 0.0699 |
| Constant fill | 0.3428 | 0.2920 | 0.2791 |

The camera saving comes from avoiding a lowercase-string allocation for every
world object visited by repeated stage/tag searches. The constant-fill saving
comes from borrowing `usage` instead of cloning it on each value lookup. No
index or new cache needs invalidation.

There is no defensible whole-frame FPS claim from these small changes on this
loaded machine. The first conversation pair has wall medians 68.06 → 73.31 ms;
the reverse-order repeat has original 64.14 versus optimized 57.81 ms. A further
pair with the followed conversation camera gives 56.20 → 56.05 ms. The component
CPU reduction repeats, but large variations elsewhere swamp it. Journal and
skills did not leave a persistent conversation slowdown: camera/UI costs after
closing them match a fresh conversation, and shown screens return to
`{Conversation}`.

### Spikes

- A 411.24 ms warmed conversation frame used 34.88 ms of main-thread CPU. Its
  world encode interval was 336.94 ms, including 261.82 ms in shadow preparation
  and encoding; simulation was not its cause. The large wall/CPU gap localizes
  the delay to off-CPU/driver/scheduling time, without claiming which external
  source caused it.
- A different 197.88 ms frame spent 145.95 ms in submission, while total
  main-thread CPU was 29.00 ms. It is not a camera parse spike.
- Reply reveal produces a repeatable UI burst: token tracing shows
  `Conversation.ResponsesReady` changing at benchmark frame 91, followed by
  the UI peak at frame 99. Across runs that peak costs 41–93 ms wall and
  35–45 ms CPU in UI compositing/changed draw-list preparation, plus UI tick
  work. It occurs when the reply interface changes, not on every frame. The
  direct UI renderer already retains unchanged prepared draw lists. With audio
  disabled the reading-time fallback determines these frame indices; voice
  playback can move the same UI transition to a different time.
- Cold frames are larger still: the first measured conversation frame in one
  pair was 340.04 ms, with 235.76 ms in GPU wait. These are excluded from the
  steady statistics, not hidden from the raw logs.
- The original report log records Journal construction at 100–110 ms, Skills
  at 190 ms, and HUD reconstruction at 210–240 ms. Its 27 FPS/186.7 ms and
  34 FPS/178.6 ms windows include screen changes. Those numbers are not proof
  of a permanently invalidated render cache after opening menus.

### Decisions left unchanged

Diagnostic quality changes, with the followed conversation camera and the same
200-frame procedure, have these wall medians:

| Diagnostic setting | Median ms | Consequence |
| --- | ---: | --- |
| Report quality, 3200×1800, 4× MSAA | 56.05 | Reference |
| All frame effects off | 43.59 | Changes depth of field, bloom and grading |
| Physical framebuffer 1600×900 | 35.36 | One quarter as many pixels; loses Retina detail |
| 1× sampling | 61.95 | Uses the existing FXAA path; no clear win in this run |

These are quality/performance decisions, not settings changed by this PR.
In particular, the frame-effects comparison does not isolate depth of field
from the other effects. Lowering resolution is a substantial saving but changes
the requested output.

Caching or prewarming Scaleform screens/reply geometry could move the cold UI
cost off the visible transition. It needs a lifecycle/memory decision and
validation of observer registration, menu state and animation; keeping a closed
movie alive is not equivalent to the current unload/recreate behavior. No such
change is made here.

## Rendering and scope

Tag matching keeps its previous ASCII-only case handling and world-object order;
it still scans current objects and observes tag edits immediately. Borrowing a
material usage string changes no shader inputs. No lighting, animation, camera,
effect, quality or resolution settings are changed.

All six report-camera before/after captures have **zero changed pixels**, at a
strict channel-difference threshold of 0, as does the additional followed-camera
conversation pair. Each comparison is at native 3200×1800, including alpha.
Three-panel review images include an amplified absolute delta (×8), which is
entirely black. The final branch also merges the upstream OpenGL-only MSAA
availability fix; the measurements use Metal.

A further comparison of the clean release, with the temporary benchmark and
timers removed, against the baseline release gives zero changed RGBA pixels for
both conversation and free play. There are nine zero-delta capture pairs in
total. Formatting and workspace/all-target Clippy with `-D warnings` pass;
workspace tests report 1,458 passed, 25 ignored and zero failures. A full
simulation-semantic sweep was not run because these allocation changes preserve
matching and shader-value semantics.
