# Enemy animation cadence

## Reproduction

The baseline is `d6b15225`, including the paired death-blow presentation from #1090.
The inputs come from Sera's Proving reports copied into
`OpenThedas-testdata/triage-reports/{1106,1098,1099}`. Each run restores
`save_before.json` with its saved runtime and mounted resource tables, applies the
recorded journal inputs, and draws every recorded simulation tick during the last
three seconds of the available journal prefix. These are offscreen 960 × 540 captures at each
report's saved camera. A separate three-second run starts directly at #1098's
before save. The journal supplies the simulation commands corresponding to the
report's recorded input.

The #1106 replay reaches its report's exact saved time and creature command states.
The available #1098 journal ends 29 ms before its report snapshot, and #1099 ends
19 ms before its snapshot. Before and after runs use identical prefix boundaries.

The capture harness and plans live under
`OpenThedas-testdata/codex-captures/enemy-anim-rate/harness/`. They contain local
asset paths and remain outside the repository. The ordinary death probe issues
the existing debug `Kill` command to sliding NPC 851 after the #1098 replay and
samples the next three seconds at 60 Hz. The paired probe advances #1090's saved
Oskias reaction from 391 ms after the kill, at 17 ms per frame.

`RUST_LOG=thedas_app::animation=trace` enables one JSON row per creature per
presentation sync. Rows include the frame, simulation time, elapsed clock input,
resolved resource, clip/sample times, all blend layers, position, measured velocity,
command and deadline, death state, visibility decision, actual re-pose decision,
and a hash of the retained model-space bone matrices. The first row has an unknown
elapsed time and velocity. A paused sync has zero elapsed time and unknown velocity.
`presentation.json` also records the last re-pose decision.

## Pose culling

#929 added visibility-driven pose reuse. #947 added invalidation when a clip's
shape changes, so a culled creature can receive occasional new poses when its
action or layers change. Clip time alone keeps the same shape.

`D3dPass::in_view` indexes shader passes. A scene draw can have several passes,
and a draw rendered with the stand-in can have none. `D3dPass::seen_draws` returned
that pass-indexed vector to callers indexing `AreaScene::draws`, including
`AreaRenderer::prop_seen`. Its shadow-caster union already used scene draw indices,
which mixed the two index spaces in the returned vector.

This can classify a visible creature using another draw's hidden flag. Its world
placement and clip clock continue advancing, while `Animator::pose_props` reuses
the old bone matrices. Action changes can make it appear to animate intermittently.

The correction reduces all pass visibility into scene draw indices, unions visible
shadow casters in those same indices, and keeps draws with no translated passes
eligible for posing. Cold visibility remains unknown until the first update.
Three portable regressions cover multipass indexing and union, shadow-only
visibility, and absent translated passes.

#994's root-translation scaling and #1048's floor placement were also reviewed.
#994 applies appearance translation multipliers. #1048 adjusts floor placement.

## Sliding replay before and after

For #1098, NPC 851, `bdc140cr_amb_f_3`, receives 3 fresh poses in 110 baseline
frames and 110 fresh poses in 110 corrected frames. The hero receives 110 fresh
poses in both runs. The NPC's simulation positions, velocities and clip times agree
between runs. Its walk resource is `mh_m.mov_wf.ani`.

| Frame | World ms | Clock dt ms | Clip clock s | Simulation position | Fresh pose before | Fresh pose after |
| --- | --- | --- | --- | --- | --- | --- |
| 2 | 2422676 | 31 | 0.052 | (-13.865572, 4.988148, 0.000000) | false | true |
| 30 | 2423411 | 30 | 0.787 | (-13.643581, 6.098103, 0.000000) | false | true |
| 70 | 2424561 | 21 | 1.937 | (-13.099924, 7.766124, 0.043761) | false | true |
| 100 | 2425362 | 22 | 2.738 | (-12.450012, 8.671741, 0.441121) | false | true |

The full rows for every creature, including the hero and opponent in #1106, are
under `enemy-anim-rate/{before,after}/<case>/per-frame.csv` beside the raw JSON trace
in `run.log`. Captures and per-frame presentation snapshots share those folders.

## Death evidence on current main

In the ordinary-death probe, NPC 851 is sampled once at 0.016 seconds into
`mh_c.sw_die_v1.ani`. Its retained bone hash then stays identical for the next
179 frames. The clip clock reaches 3.000 seconds and the sampled time reaches
the animation's 2.100-second end while those bones remain at the first pose.

The #1090 paired path reproduces Sera's freeze followed by a sudden fall.
Oskias, NPC 706, resolves `db_h_dw_die_v1` to `mh_c.dw_dblow_die_v1.ani`.
Frame 0 samples 0.408 seconds into the reaction. The same bone hash remains through
frame 153, whose requested sample is 3.009 seconds. At frame 154, 3.026 seconds
into the clip, the hero and Leske holster their weapons. Their changed attachment
nodes rebuild the objects' scene, so the new renderer forces another pose and
Oskias snaps into the late reaction. His action and animation resource stay the
same throughout. There are two pose evaluations in 180 frames.

Both death probes therefore exercise the same incorrect visibility indexing as
the sliding creature, including with the paired playback changes from #1090.
With the correction, NPC 851 receives a fresh pose on all 180 death frames. It
shows 126 distinct poses followed by its held final pose. Oskias receives a fresh
pose on all 180 paired-reaction frames, with 180 distinct poses through the sampled
interval. The hero's paired action also continues to be sampled every frame.

Labelled before/after strips with amplified RGB delta panels are staged under
`OpenThedas-testdata/codex-captures/media-staging/enemy-anim-rate/`. Their captions
give the changed-pixel percentage and threshold for each displayed crop.

All seven before/after cases contain identical complete saved simulation states.
Across 1,003 frames per build, every trace row agrees on clip selection, clip
times, clock input, position, velocity, death state and active command. The
comparison excludes the visibility decision, re-pose decision and bone hash.
Canonical saved-state hashes and frame counts are recorded in
`enemy-anim-rate/verification.json`.

An additional #1106 comparison carries presentation controllers through every
recorded Tick before the captured window. Its camera follows the drawn hero using
the saved report offset (2.652462, -1.421850, 1.728330), yaw and pitch. A temporary
evidence-build switch selects the exact old visibility reduction for the baseline.
Both runs perform 2,185 presentation updates and capture their last 113 frames.
All hero/opponent clip, clock, position, velocity and command rows agree, and both
complete saved states also match the primary #1106 replay. The source files were
restored byte-for-byte to their validated versions after building this driver.
The history CSVs identify the captured window starting at sync 2072.

## `HANDOFF: sim`

### Issue 1099

The marked creature is NPC 798, `bdc140cr_amb_m_3`. Its scripted animation command
is completed and replaced every 500 ms. For example, command 3020 has deadline
2448431, followed by command 843 with deadline 2448931. These actions resolve to
multi-second animations. `Sim::command_ms` supplies the generic 500 ms duration
for this command, and `models::played_animation` derives its start from that same
duration. Successive gestures therefore play their beginnings and crossfade into
the next gesture before reaching their authored ends.

The per-frame trace shows this creature being re-posed on all 117 frames. The command
executor's duration and loop-count handling need a sim investigation. Issue #1099
remains open for that work.

### Issue 1106

Mainar is NPC 855, `bdc140cr_mainar`. In the sampled three-second replay, its pose
is re-evaluated on all 113 frames, including all 24 frames with measured movement.
Its movement clip resolves to `mh_m.cmov_rf.ani`. The trace includes the outgoing
attack under the quarter-second transition into locomotion.
The controller-history and hero-following-camera comparison also gives Mainar
113 fresh, distinct poses in 113 captured frames in both builds, with continuous
movement clip clocks and no retained cutscene animation over the locomotion clip.

The original report's `save_now.json` has an executing
`CommandUseAbility(1)`, deadline 2759500, no movement origin, and neither
`combat.attack` nor `combat.attack_ms`. Its recorded presentation selects
`pause` / `mh_c.sw_rdy.ani`. The combat command contract and repeated short
approaches need further diagnosis before attributing this report to culling.
Issue #1106 remains open for that work.

### Proving victory death clock

A separate debug kill of Mainar starts `bdc140cs_everd_victory.cut`. Over the next
180 presentation frames, the cutscene advances to 3000 ms while world time stays
at 2758435. Mainar's ordinary death clip consequently remains at its start. The
cutscene's authored duration is 5.833333 seconds. This supplies a second death
stall mechanism for sim/presentation-clock follow-up. The culling correction
addresses the advancing-clock death probe on NPC 851.
