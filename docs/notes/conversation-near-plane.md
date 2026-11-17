# Conversation gradient (#1100)

Report 1100, build 81af4d72, reproduces from `save_now.json` in the Orzammar
Proving Grounds (`bdc140ar_proving`, layout `orz500d`).

## Diagnosis

`bdc140_everd`, line 1, selects camera actor **3 / Primary Camera**, whose
jump names **cam_wide_2** in `bdc140st_everd.stg`. The stage is placed at
(-34.526620, 39.986076, -0.999954), rotated -10 degrees about Z. The camera's
local position (-4.546618, -4.511138, 0.547649) becomes world
**(-39.787518, 36.332993, -0.452305)**, FOV 36 degrees. Its position is stable
in the first 0, 33, 66, 100 and 200 ms samples; the authored shake changes its
orientation slightly.

The camera source is correct. The established executable path is dialog line
setup **0x68e53c -> 0x7f0960**, stage placement **0x68e584 -> 0x7d2aa0**, and
active camera handoff **0x7e41b1–0x7e41ca**. Camera jumps resolve the named stage
camera at **0x7df2f0 -> 0x7deaf0**. Conversation collision correction is disabled
(**0x68fed5–0x68fedb**, **0x6c4af0–0x6c4b12**); inventing a blocker sweep would
change the authored shot.

A diagnostic center ray meets `prp_fireplace1_0` / `dci_wall04.mao` in room
`orz500d_3046` only **0.081972 m** ahead. Disabling all frame effects leaves the
same magnified masonry (brown instead of the graded lavender). There are no
color fades. The gradient is close geometry exposed by the wrong near plane,
not an extreme DOF radius, first-frame focus failure, or letterbox overlay.

The DOF values are stable from the first frame:

| Parameter | Conversation | Line camera |
| --- | --- | --- |
| FocusDistance | 5 | 2 |
| FocalRadius | 8 | 1 |
| NearFade / FarFade | 1 / 20 | 5 / 5 |
| WeightBlur | 0.5 | 1 |
| ArtifactBlur / MaxBlur | 1.3 / 1 | 1.3 / 1 |

Enable `RUST_LOG=info,thedas_app::talkcam=trace` for the source actor/jump,
stage, elapsed time, final camera including clip planes, and frame-effect
parameters. The logger does not change camera or simulation state.

## Source fix

Area creation calls scene-camera setup at **0x6892a7 -> 0x40ae60**. That setup
supplies **0.15 m** near distance at **0x40af65–0x40af82**, through
**0x537780 -> 0x465670**. The scalar at 0xadeea8 is 0.15. Our default was an
untraced 0.05 m. Restoring 0.15 clips the close fireplace surface and reveals
Everd lying beside it, without moving the camera or changing DOF.

A synthetic projection regression requires the 8.2 cm surface to fall before
the near plane and the speaker at 2 m to remain in the view volume. The default
is shared by Faithful and Deferred rendering. Its use for DA2 remains explicitly
provisional: the DA2 executable's setup has not been traced. The existing far
plane and other camera approximations are unchanged.

## Evidence

Own captures and trace logs are under
`codex-captures/conv-gradient-hearth-fire/` in testdata. `1100-before.png` and
`1100-no-post.png` isolate the source. `1100-near15-effective.png` verifies the
traced clipping distance, including the line's normal DOF. Final before/after
captures and an amplified delta panel are staged for rendering review.

This conclusion combines static executable analysis with replacement-engine
captures. No original-game run or dynamic original-game camera measurement was
performed.
