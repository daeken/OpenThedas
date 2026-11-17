# Mouse's orphaned line cinematics (#1080)

Reproduced from the Fade-start checkpoint through hidden control port 8047, with
the input sequence attached to the issue. Captures and full control/state logs live
in the testdata `codex-captures/mouse-dialog-camera` directory. No game assets are
fixtures in this repository.

The initial replay uses the issue's capture-harness binary and Fade-start save.
The instrumented native-control follow-up starts from its saved line-102 state;
the controlled comparison restores the hero's recorded bound position and clears
the old movement order, leaving the cinematic, conversation, and stage context
alone. Native control uses `windowkey` to exercise the window's skip handler.

## Source and placement

All the origin shots are **conversation-line CUTR resources** played through the
world-cutscene path. They are neither the default talk camera nor a missing area
stage. Each resource names `mouse_new.stg`; the originating conversation is
`bhm600_mouse_new`.

| Line / resource | Active camera and resource-space position | Observed origin view |
| --- | --- | --- |
| 102 / `387621.cut` | Secondary Camera, stage camera `2_1_3_mouse`, `(2.5405, 0.0982, -0.2805)` | The issue session shows `(2.540, 0.098, -0.280)` after dialog closure. In the local replay this cut started sooner and still had its placement. |
| 103 / `387623.cut` | Primary Camera, authored base `(-2.1889, 0.5668, 1.9945)`, with camera motion | Local replay at 1.07 s: `(-3.015, 0.780, 2.175)`. |
| 118 / `387632.cut` | Primary Camera, authored base `(-0.6384, 0.3620, 1.8171)`, with camera motion | Local replay at 1 s: `(-2.116, 0.557, 1.986)`, matching the reported near-origin shot. |

The area has a `mouse_new` stage at `(163.0695, 98.4452, 27.2913)`, facing
285 degrees. The initial `386342.cut` is placed there correctly. Inline staged
shots also find that stage. Correct placement is the stage translation plus its
rotation about Z, applied once to the sampled resource-space camera and actors.
There is no extra area offset or object-relative attachment in this case.
The resources' authoring-position field 5014 is approximately `(1.1243, 0.0874,
-0.1525)`; it is not the area-stage anchor.

The failed shots have `from_dialog=true`, `placement=null`, and no remaining
conversation in their saved state. `Sim::bind_cutscene_file` needs `self.talk` to
obtain the originating line's stage context. Once that is gone it returns no
placement. `CutsceneCamera::camera` substitutes `Placement::NONE`, so local
coordinates become world coordinates. The pending CUTR queue outlives the line
that owned it; retargeting those orphaned resources to the hero would mask the
ownership error.

## Executable evidence and fix

DAOrigins.exe virtual addresses, using the existing read-only disassembly:

- `0x68e53c -> 0x7f0960` constructs the line cinematic from either a CUTR resource
  or inline CUT data. `0x7f0b04–0x7f0b39` installs conversation ownership.
- `0x68e584 -> 0x7d2aa0` places it. The staged branch finds the nearest area stage
  at `0x7d2b85 -> 0x67cdd0`, obtains its transform, and installs the placement at
  `0x7d2c02 -> 0x7cdc40`. This confirms the existing area-stage anchor rule.
- New-line setup `0x68e3f8–0x68e43d` releases the previous cinematic through
  `0x7e10a0` before installing the next one. Dialog close/reset also releases it
  at `0x68f4a7–0x68f4c3` and `0x691115–0x691131`. These cleanup paths were checked
  directly and agree with the earlier `dialog-reply-camera-trace.md` analysis.

Origins' shared line cleanup now cancels that line's pending referenced cinematic
and releases its playing referenced cinematic, as it already did for inline
staging. It does so only when the line is actually replaced or closed. Reply
availability alone does not release it. Script-owned playing cuts are independent;
DA2's playback path is unchanged. No edits to `talkcam.rs` or `live.rs` overlap
the remove-decision-camera branch.

Internal dialog-node walking can pre-walk several nodes spoken by one resource;
it retains the source's ownership until an actual player reply or dialog cleanup.
Synthetic tests cover that distinction, queued/playing cleanup on reply, close
and replacement, actor release, and isolation from script-owned cuts and DA2.

The corrected line-118 capture shows Mouse and the hero while `387632.cut` is
playing. Its source log has the stage placement above and a world camera near
`(163.051, 100.699, 29.285)`, rather than the near-origin camera. After the dialog
closes, the pending line-resource queue is empty and gameplay stays near
`(161.979, 102.311, 29.449)` through the former bad-shot/skip milestones.

Validation: formatting and full-workspace Clippy with warnings denied passed;
workspace tests passed (1,460 passed, 25 ignored, no failures). The five standard
isolated baseline/branch gate scenarios have zero normalized log differences,
identical plot traces and destination areas, and no refused saves. The native
capture baseline predates the main merge's deathblow work, which changes no
app/render camera source; the comparison-gate baseline is the merged main
`45e01895` built before this fix.

This fixes the stale-resource lifetime, not the larger provisional architecture
that queues CUTR resources as world cinematics. Full fidelity of retained playback
past nominal duration, multi-line cinematic advancement, and unrelated skip/UI
policies remains outside this change. The anchor and release rules above are
traced; those broader behaviors are not claimed as implemented.
