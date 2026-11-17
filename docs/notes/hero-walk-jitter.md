# Hero walking frame jitter

## Reproduction and measurement

The primary comparison uses the copied Sera report
`sera-1117/2026-11-17-02-48-bdc140ar_proving/save_before.json`. Both optimized
builds start from main `d6b15225`, with movement diagnostics enabled in the
baseline and the frame-order fix added in the candidate. Each run warms three
captures, orbits 90 degrees, then holds the real window `KeyW` for approximately
three seconds through control port 8039. The baseline travels 12.840 m along a
straight westward line. The candidate travels 14.889 m, with 14.971 m of path
length as its tail approaches geometry. The initial shared straight segment is
used in the position tables below.

Runs use hidden windows, the faithful renderer, medium detail and a requested
960×540 window, with 1920×1080 physical pixels on this display. Screenshot
requests force offscreen rendering while the hidden surface is unavailable.
Readbacks and host load determine these probe timings. The baseline has 53
synchronized updates and 19 rendered frames. The candidate has 59 updates and
23 rendered frames. Their simulation clocks advance 2.144 and 2.499 seconds
respectively because long frames are capped at 100 ms.

The control command `motion_trace` with `enabled` true starts a bounded
4096-update trace. Sending it with `enabled` false returns the frames and stops
recording. Each row contains simulation position/time, placed feet, all posed
model roots, animation resource/phase/translation scale, floor query result,
camera position, follow target, frame dt, simulation Tick command count and a
rendered flag. Poses come from the same cached skeleton matrices used for draws.
The drawn root is the world-space root bone after placement and skeletal
translation. This dwarf body's neutral model-space root Z is 0.789479 m.

Full JSON and CSV vectors are staged under
`OpenThedas-testdata/codex-captures/hero-walk-jitter/{before-west,after-west}/`.
`motion.json` retains every body root and clip. `frames.csv` tabulates XYZ
positions, drawn−sim and target−sim offsets, camera−target offsets and adjacent
update deltas. Its rendered flag distinguishes the submitted frames.

## Oscillating term

The baseline follow target XY equals the **previous update's** simulation XY
on every sampled update, with maximum error **0.000000 m**. The rendered
creature already uses the current simulation position. At the measured
6.000 m/s, the target error therefore equals the movement consumed by that
update. Its rendered-frame magnitude ranges from **0.024000 to 0.078000 m**,
with standard deviation **0.013888 m**. Across all synchronized updates, the
maximum reaches **0.600000 m**, the 100 ms cap times 6 m/s.

The camera−target vector stays at approximately
**(2.988584, 0.000001, 0.261467) m** in both runs. The camera orbit/collision
distance stays steady while its target trails the hero by a varying amount.
The integer-millisecond simulation accepts one variable-duration Tick command
per update here. Successive world times record the consumed milliseconds.

The supplied 02-54 platform report's first twelve recorded playtest frames
alternate around 20 and 30 ms. At the measured movement speed, this frame-order
error predicts approximately **0.12 and 0.18 m** of lag, changing by **0.06 m**
at approximately **20 Hz**. This is an inference combining the report's frame
cadence with the replacement engine's measured speed and update ordering.

The root animation's Z offset spans −0.090876 to +0.002493 m before and
−0.091546 to +0.001786 m after, including the locomotion start and running
cycle. Floor heights follow the same authored geometry across the common path,
including its +0.099976 m strip. These terms continue to animate and ground
while the horizontal camera-target error becomes **0.000000 m** on every
candidate update, including all **23 rendered frames**.

## Before

All positions are metres. These are successive rendered samples, with their
original synchronized-update indices. Each rendered update consumes one Tick.

| Frame | dt ms | Ticks | Sim X | Drawn root X | Camera X | Target X | Target−sim X mm |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 2 | 3.825 | 1 | 1.517435 | 1.513342 | 4.530020 | 1.541435 | 24.000 |
| 5 | 8.166 | 1 | 0.833435 | 0.851094 | 3.876019 | 0.887435 | 54.000 |
| 7 | 8.652 | 1 | 0.185435 | 0.180883 | 3.222019 | 0.233435 | 48.000 |
| 9 | 5.674 | 1 | -0.450565 | -0.453411 | 2.574019 | -0.414565 | 36.000 |
| 13 | 8.750 | 1 | -1.194565 | -1.189958 | 1.848019 | -1.140565 | 54.000 |
| 15 | 5.559 | 1 | -1.830565 | -1.838110 | 1.194019 | -1.794565 | 36.000 |
| 18 | 4.487 | 1 | -2.508564 | -2.515337 | 0.504019 | -2.484565 | 24.000 |
| 20 | 6.998 | 1 | -3.150564 | -3.147717 | -0.119980 | -3.108564 | 42.000 |

## After

| Frame | dt ms | Ticks | Sim X | Drawn root X | Camera X | Target X | Target−sim X mm |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 2 | 11.250 | 1 | 1.469435 | 1.466364 | 4.458019 | 1.469435 | 0.000 |
| 4 | 7.929 | 1 | 0.821435 | 0.839566 | 3.810019 | 0.821435 | 0.000 |
| 6 | 7.014 | 1 | 0.179435 | 0.174881 | 3.168020 | 0.179435 | 0.000 |
| 8 | 7.750 | 1 | -0.468565 | -0.471372 | 2.520020 | -0.468565 | 0.000 |
| 10 | 17.012 | 1 | -1.170565 | -1.165829 | 1.818019 | -1.170565 | 0.000 |
| 12 | 8.827 | 1 | -1.824565 | -1.831879 | 1.164019 | -1.824565 | 0.000 |
| 15 | 6.763 | 1 | -2.496565 | -2.503583 | 0.492019 | -2.496565 | 0.000 |
| 18 | 3.909 | 1 | -3.150565 | -3.147794 | -0.161981 | -3.150565 | 0.000 |

## Offsets

All tuples are XYZ metres from the same rendered samples. The drawn-root Z
includes the skeleton's neutral root height. The complete per-frame series is
in each run's CSV.

| Run/frame | Drawn−sim | Camera−target | Target−sim |
| --- | --- | --- | --- |
| before/2 | (-0.004094, -0.005337, 0.791972) | (2.988584, 0.000001, 0.261467) | (0.024000, 0.000000, 1.426000) |
| before/5 | (0.017659, -0.033842, 0.752678) | (2.988584, 0.000001, 0.261467) | (0.054000, 0.000000, 1.426000) |
| before/7 | (-0.004553, -0.022714, 0.741241) | (2.988584, 0.000001, 0.261467) | (0.048000, 0.000000, 1.426000) |
| before/9 | (-0.002846, -0.004532, 0.740201) | (2.988584, 0.000001, 0.261467) | (0.036000, 0.000000, 1.426000) |
| after/2 | (-0.003071, -0.005920, 0.791265) | (2.988584, 0.000001, 0.261467) | (0.000000, 0.000000, 1.426000) |
| after/4 | (0.018130, -0.033690, 0.751956) | (2.988584, 0.000001, 0.261467) | (0.000000, 0.000000, 1.426000) |
| after/6 | (-0.004554, -0.022608, 0.741032) | (2.988584, 0.000001, 0.261467) | (0.000000, 0.000000, 1.426000) |
| after/8 | (-0.002807, -0.004338, 0.739769) | (2.988584, 0.000001, 0.261467) | (0.000000, 0.000000, 1.426000) |

## Source fix and executable evidence

The window loop now issues input, advances the simulation, resolves the camera
from that stepped state, then poses and renders it. Input, simulation and
camera share the frame dt captured at redraw entry. A common preparation path
serves surface and offscreen frames. Camera fades are resolved before sync
applies object opacity. This removes the previous-position dependency at its
source.

The frame-order repair follows the measured replacement-engine data.
**Native frame-scheduling parity is provisional.** Addresses below are
DAOrigins.exe virtual addresses. Existing static traces
provide the appearance-scaled translation sampler at **0x811a60**, destination
base-plus-animation composition at **0x55e9d0**, and appearance multipliers
loaded at **0x6a13c0**. The existing floor approximation uses the controller's
0.5 m step offset installed at **0x6b1ed0–0x6b2206** and the movement path
**0x6b6880**. The point-ray/render-triangle floor approximation remains
provisional. The detailed ground/controller evidence is in
[creature-floor-presentation.md](creature-floor-presentation.md).

PRs #929 and #947 retain placement while reusing culled poses and refresh poses
when clip shape changes. The warmed hero's posed roots are sampled after those
paths. PR #994's appearance-scaled translations and #1048's placement are both
included in the recorded drawn-root matrices.

## Platform sinking in the 02-54 report

The copied `2026-11-17-02-54-bdc140ar_proving-2/presentation.json` places the
hero at sim/drawn placement **Z −1.689837 m** and retains a full-weight
`bdc140_everd.dlb` layer playing `mh.dg_tn_90_l.ani`. Its skeletal root has
another **−0.008521 m** of animation offset. Loading that same report's
`save_now.json` in the instrumented engine grounds ordinary placement at
**Z −1.500004 m**, a **+0.189833 m** correction. The reported held-cutscene
placement bypasses ordinary grounding in `Live::sync`.

This points to a separate retained-pose/root-ownership problem. In
`CutsceneActors::switch`, an equal source key returns before closed-conversation
cleanup. A transition from an already empty source to an empty source can keep
the retained layers alive. The saved presentation supports this explanation,
while a fresh load rebuilds presentation state. **The historical retained-pose
lifetime diagnosis is provisional**, pending replay of that conversation's
presentation history. Camera frame ordering explains the rapid horizontal
walking jitter.

## Regression coverage

`tools/repro/check_walk_camera.py PORT OUT.json` drives a three-second W walk
through a running control server and forces rendered samples. It checks at
least ten rendered frames, movement over one metre, active follow targets and
target XY within 0.1 mm of current simulation XY. The baseline fails with
maximum error **0.600000 m**. The candidate passes on both the northward
17-rendered-frame run and the westward 23-rendered-frame run, with exact zero
target XY error throughout. The standalone regression entry point also passes
with 19 rendered frames and a 12.468 m walk.

The shared window-loop scheduling serves both games. Live coverage uses the
supplied Origins reports. Validation results are recorded in the PR.
Formatting and workspace/all-target clippy with warnings denied pass. Debug
workspace tests finish with **1,478 passed, 0 failed and 26 ignored**. The
optimized application build passes.
Simulation-semantics sweep gates are inapplicable to this application
presentation change.
