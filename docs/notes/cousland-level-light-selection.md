# Cousland hall light-selection review

The new original D3D9 hall trace contradicts the proposed sunlight-restoration
fallback. Its identified floor and carpet draws exclude `sun_fake_static` and
agree with PR #1074's point-light choices. The unfinished fallback has been
discarded. This revision adds a synthetic regression test protecting that
exclusion; it does not restore the old bright floor or claim that the reported
free-camera image discrepancy is resolved.

## Inputs and scope

Original: `reference/original/triage/hn_hall_trace/` in OpenThedas-testdata,
39 Static1 lightmapped draws from the last complete frame of `hn_hall.trace`.
This is the letterboxed Howe conversation, not the save01 free camera. The
comparison reads the trace in place and keeps all generated data outside Git,
under `codex-captures/level-light-selection-cousland/`.

The Commons harness is reused with the trace directory made an argument. It
links by vertex count, lightmap upload signature, atlas region and world matrix.
There are **38 strict identities, one provisional identity, no unmatched draws**.
The provisional draw is original call 14263241, scene draw 592,
`fca_outwall01_0.msh:mshfade_fca_outwall01_1`; its unique dimensions/atlas/world
match has no verified lightmap payload signature. Identities are independent of
the light values being scored.

Main means the legacy picker used by main 6d66c10e (also retained by the merged
main adf58096). PR means the original PR #1074 picker at 2cd03631. The diagnostic
replays both choices through the unchanged ConstantContext packer and compares
the revision's live renderer dump separately. Historical picker replays are
not newly built historical applications.

## What the old picker loses

The following IDs are scene-light indices in the saved diagnostic, not original
runtime pointers. Light **0**, `sun_fake_static`, is domain 2, static, Version 0,
intensity 1, radius 2250, RGB (1.60, 1.26, 0.80), position approximately
(463.592896, 328.669250, 1099.824097), assigned to room **79**. All 29 hall
floor/carpet draws below belong to room **4**. Room 79's explicit RML 3164 list
contains 37 other room IDs and **omits 4**. It therefore fails `reaches`.

The class/domain filter accepts this light. Both the placement sphere and MSH
8019 light sphere intersect it, and its positive influence would rank first if
the room test admitted it. For the two traced Floor02 tiles, influence is about
1000.0803 and 1002.2146. Their 8019 and placement radii are both 5.6571746; centre
Z differs by only 0.0000102. The choice does not depend on which of those spheres
is used. All relevant lights are Version 0, so final Version ordering cannot
explain the rejection or restore a discarded member.

| Scene draw | Mesh/chunk | Legacy A/B/C | PR A/B/C | Dropped |
| ---: | --- | --- | --- | --- |
| 1109 | Floor02 | 0,28,27 | 27,28,20 | 0 |
| 1119 | Floor02 | 0,21,22 | 27,21,22 | 0 |
| 1120 | Floor02 | 0,29,20 | 29,28,20 | 0 |
| 1121 | Floor02 | 0,29,22 | 29,25,22 | 0 |
| 1122 | Floor02 | 0,29 | 29 | 0 |
| 1123 | Floor02 | 0,29,25 | 29,25 | 0 |
| 1124 | Floor02 | 0,27,19 | 27,26,24 | 0,19 |
| 1144 | Floor02 | 0,21,23 | 27,24,23 | 0,21 |
| 1199 | carpet2b/carpet08 | 0,25,22 | 25,29,22 | 0 |
| 1222 | carpet3b/carpet07 | 0,28,27 | 27,28,19 | 0 |
| 1223 | carpetg_l/carpet13 | 0,28,27 | 27,28,20 | 0 |
| 1224 | carpetg_l/object05 | 0,28,27 | 27,28,20 | 0 |
| 1225 | carpet3b/carpet07 | 0,29 | 29 | 0 |
| 1236 | carpet2b/carpet08 | 0,20,28 | 29,28,20 | 0 |
| 1237 | carpet1b/carpet04 | 29,0 | 29 | 0 |
| 1267 | carpet3b/carpet07 | 0,21,22 | 22,21,27 | 0 |
| 1297 | carpet1b/carpet04 | 0,29 | 29 | 0 |
| 1320 | Floor01 | 0,27,19 | 27,19,28 | 0 |
| 1321 | Floor01 | 0,27 | 27 | 0 |
| 1322 | Floor01 | 0,27,19 | 27,28,19 | 0 |
| 1323 | Floor01 | 0,27,28 | 27,28,20 | 0 |
| 1324 | Floor01 | 0,20,28 | 28,20,27 | 0 |
| 1325 | Floor01 | 0,29 | 29 | 0 |
| 1326 | Floor01 | 0 | — | 0 |
| 1327 | Floor01 | 0,21 | 21 | 0 |
| 1328 | Floor01 | 0,21 | 21 | 0 |
| 1329 | Floor01 | 0,22 | 22 | 0 |
| 1330 | Floor01 | 0,22,25 | 25,22 | 0 |
| 1331 | Floor01 | 0,29,25 | 25,29 | 0 |

The two extra dropped local lights pass class, reach and sphere tests. At draw
1124, light 19 (`lamp_001_RT`) has influence 3.5133, below selected 27/26/24 at
10.5469/4.5698/3.7661. At draw 1144, light 21 (`lamp_003_RT`) has 3.4504, below selected
27/24/23 at 5.7317/4.5743/4.5593. Those are changes from brightness × cos²
ranking to R−d membership, not additional room failures. No D/E points are
selected on these hall floor/carpet draws.

## Original observation and decision

The original directly verifies seven of these draws:

| Original call | Scene draw | Original A/B/C (scene indices) |
| ---: | ---: | --- |
| 14263166 | 1237 | 29 |
| 14263175 | 1297 | 29 |
| 14263341 | 1331 | 25,29 |
| 14263354 | 1120 | 29,28,20 |
| 14263363 | 1121 | 29,25,22 |
| 14263457 | 1223 | 27,28,20 |
| 14263526 | 1224 | 27,28,20 |

Light 29 is `fireplace_001_RT`; light 25 is `bounce_008_RT`. The distant
`sun_fake_static` appears on the separate exterior-wall draw 592 in room 65,
which room 79's list reaches, so its absence on the floors is not a
missing-light-catalog artifact. D/E position registers
are zero on all 39 original Static1 draws.

For these seven observed draws, **none of the questioned selection steps is
shown wrong**: the room exclusion and R−d selections match the original. A
second-room path or broad sphere exception that admits light 0 would contradict
the captured selections. The Commons residual-room question remains open; it
cannot justify this particular exception.

The discarded provisional rule admitted legacy-selected static points from rigs
with a nonempty cross-room list, then kept chunk spheres, influence ranking and
Version ordering. On the original hall fixture it destroys all 38 strict
point-slot matches. Its source diff and diagnostic results are retained locally
as `rejected-fallback.patch` and `hall/`, not in the shipped renderer.

**QA needs a free-camera original frame and D3D9 trace at the requested save01
front/back views, after the conversation has ended**, to settle whether the
original exploration state selects different links or whether the remaining
bright-window discrepancy is in shader/lightmap/shadow behavior. The conversation
trace establishes per-draw selections for its state; it cannot establish
exploration-frame pixel parity or reconstruct unobserved room/cache transitions.
No camera-specific room or sunlight override is introduced.

For the observed draws, restoring the legacy sun and retaining the original
point-light matches are conflicting requirements. No provisional admission rule
supported by this trace can do both. An exploration-only exception would guess
at the missing runtime state, so the narrow action is to retain the traced
selection and obtain that state before changing rendering behavior.

The hall probe has zero shader preparation failures and 38 existing scene
warnings about missing `hrt201d_*_ll.mmh` rigs in other layouts. Neither room 4's
nor room 79's rig is missing. These warnings are retained in the artifacts;
agreement on the observed draws does not establish completeness of every room's
lighting catalog.

## Metrics

The unchanged metric compares **every component of c0–c11**, tolerance 0.0001;
Static1 c12 is shadow data, not part of the metric. Every original hall draw has
directional c2=(0,0,−1,1), c3=(0,0,0,1). Our `sun=None` fills both with zero in
main and PR #1074. Thus all full-pack comparisons fail independently of point
selection. This revision leaves that separate directional-state discrepancy
visible rather than normalizing registers to improve the score.

| Hall mode | Full c0–c11 strict | Full provisional | Point slots strict | Point provisional |
| --- | ---: | ---: | ---: | ---: |
| Main picker replay | 0/38 | 0/1 | 0/38 | 0/1 |
| PR #1074 picker replay | 0/38 | 0/1 | 38/38 | 0/1 |
| Rejected sunlight fallback | 0/38 | 0/1 | 0/38 | 0/1 |
| Trace-validated revision | 0/38 | 0/1 | 38/38 | 0/1 |

Point-only is an **additional diagnostic, not a replacement metric**: it compares
c0,c1,c5–c11, with the same tolerance, and verifies selected point identities,
order, positions/radii and colours. The one provisional draw selects the correct
distant sun, but its Z position differs by 0.0001220703 (one float ULP there),
exceeding 0.0001. It remains a failure; the tolerance is not relaxed. All seven
identified floor/carpet draws match their original point slots under the PR and
revision, and fail under main and the rejected fallback.

The final probe supplies 3,110 CPU Static1 draw records. Eleven of the 39 linked
original draws are also visible to the probe's free camera and have live renderer
constant dumps, including five of the seven identified floor/carpet draws.
The other 28 linked draws use CPU ConstantContext fills; the live subset agrees
with its CPU point slots. These are static-scene measurements without the
original conversation's dynamic actors or link history.

## Commons rerun

The unchanged 124 strict + 17 provisional Commons fixture remains **104/124
strict, 0/17 provisional**, with **141 linked live dumps**. No previously matching
PR draw regresses, and there are no additional matches. The main baseline remains
0/124 + 0/17. The rerun explicitly asserts that all 104 previous call IDs survive.

## Fresh save01 frames and floor regions

The requested scenario was rerun at 1280×720, faithful/high/original shaders,
time 3, `run 500`, both front/back cameras, with two repeats per side. New frames
are `cousland-validated/candidate/cousland-hall.png` and
`cousland-validated/candidate/cousland-hall-back.png` under the job artifacts.
The comparison baseline is the frozen legacy-main application. The captured
candidate is the merged trace-validated revision; its temporary dump block is
inert because the capture runner unsets `THEDAS_COMMONS_DRAWS`. That block and
both helper examples were removed before final workspace validation and the
clean application rebuild.

| Static crop, pixels [left,top,right,bottom) | Changed vs legacy main (>8/255) | Max channel delta |
| --- | ---: | ---: |
| Front carpet [930,560,1230,650) | 26,941/27,000 = 99.781481% | 144/255 |
| Front stone [40,495,270,580) | 3,011/19,550 = 15.401535% | 65/255 |
| Back stone [840,540,1280,710) | 72,147/74,800 = 96.453209% | 150/255 |

All three crops are **exact RGB matches to QA's earlier PR #1074 candidate
frames** (max delta 0). Both repeats of each crop are also exact RGB matches
within each side: six repeat-crop checks, max delta 0. Thus the new revision
preserves the PR's floor image and reproduces the reported change from legacy;
it does not claim to have repaired the visible complaint.

Whole-frame repeatability fails on animated characters. The capture runner
returns 1 for that expected instability, after writing all frames and reports;
clean validation continues separately. Whole-image changed percentages are
retained in its report for provenance and are not the floor metric.

Six labelled floor comparisons (legacy/revision and PR/revision, each with a
Delta ×4 third panel) and captions are staged under
`codex-captures/media-staging/level-light-selection-cousland/` for orchestrator
review. Nothing was published. `floor-final/metrics.json` records every crop
and repeat result; `cousland-validated/report.json` records input/binary hashes,
commands, environment and all complete-frame repeat measurements.

## Validation and review status

On the merged result with main **adf58096**: formatting check PASS; clippy
workspace/all-targets with warnings denied PASS; workspace tests **1,462 passed,
25 ignored, zero failures** across 31 result groups, including the new synthetic
room-exclusion guard; clean debug application build PASS. Temporary probes are
removed. The final artifact audit checks all 29 table rows against the dump,
the retained Commons call set, hall scores, exact floor comparisons/repeats, and
six staged three-panel images/captions.

This revision changes only a test and prose; it adds no simulation or production
rendering behavior. The simulation sweep gate is not applicable. The rendering
checks are the Commons harness, original hall draw comparison and save01 floor
captures above. The visible free-camera complaint remains open pending QA's
matching original exploration capture/trace. PR #1074 remains unmerged.
