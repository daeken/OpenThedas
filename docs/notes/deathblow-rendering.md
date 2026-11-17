# Paired death-blow presentation

`thedas_ui::models::creature_motion` reads the victim's persistent
`combat.death_blow_reaction` before generic death, commands or idle. It uses the
stored `action`, rather than resolving the animation-table row again. The
attacker's `combat.death_blow` supplies its own action and start/deadline in the
same way. Simulation already resolved the rows in `Sim::death_blow_clip`.

The renderer searches `Sim::ability_trees`, the same read-only tree order used
to select the pair: main-hand weapon override, weapon style, then appearance
combat tree. This also preserves a non-humanoid's own combat tree. The existing
animator resolves the action to an `.ani` resource. No pair selection, command,
impact index, notification, placement or deadline is changed by presentation.

Before the recorded deadline, non-looping playback is elapsed world milliseconds
from that actor's `started_ms`. At and after the deadline, time is `f32::MAX`,
which the animator clamps to the clip end. This matters when departure shortens
`ends_ms`: clamping elapsed time to the shortened deadline would freeze an
unfinished fall. The reaction remains the corpse pose for as long as the sim
retains it. Resurrection already clears it, returning presentation to normal
motion selection. `standing_motion` continues to provide the idle pose for the
locomotion controller; paired actions take the whole body like other one-shots.

Five UI regression tests cover both actors joining playback mid-way without
mutating saved state, the victim's exact deadline and later corpse pose,
save/load mid-reaction, the real resurrection effect, and a real `enter_area`
departure shortening a queued death's reaction deadline. The departure test
also saves and reloads the completed reaction.

## Oskias captures

The source is #1056's staged
`codex-captures/deathblow/departure/repro/frame.json`, originally prepared from
`oskias-0420/save_before.json` by `tools/repro/deathblow.sh`. The pair starts at
1382304 ms, with attacker action `db_h_dw_attk_v1` (304, 3133 ms) and reaction
`db_h_dw_die_v1` (314, 3467 ms).

The capture driver advances that snapshot through the unchanged simulator and
saves at 391, 1200, 2400, 3467 and 5000 ms after the pair starts. Both executables
render the same snapshots without advancing time, with the same fixed camera,
960 × 540 resolution and no UI. Baseline is merged main before the rendering
change. All runs use a hidden window, no audio, the dedicated cargo target and
the shared heavy-command wrapper.

Local snapshots, raw frames, capture logs and validation logs live under
`OpenThedas-testdata/codex-captures/death-blow-reaction-anim/`. Labelled
before/after/Delta ×4 triptychs and captions are staged under
`codex-captures/media-staging/death-blow-reaction-anim/`. Each comparison reports
the percentage of pixels whose largest absolute RGB channel difference exceeds
2/255. No game assets or save snapshots are included in the repository.

Validation on merged main `3a429faf`: formatting and workspace/all-target clippy
with warnings denied pass; debug workspace tests report 1464 passed, 25 ignored,
and no failures. All five new UI regressions pass. All ten hidden frame captures
succeed. Changed-pixel percentages for the Delta ×4 comparisons are:

| Time after start (ms) | Pixels changed (>2/255 in any RGB channel) |
| --- | --- |
| 391 | 2.035880% |
| 1200 | 2.038580% |
| 2400 | 1.623650% |
| 3467 | 0.422647% |
| 5000 | 0.419946% |
