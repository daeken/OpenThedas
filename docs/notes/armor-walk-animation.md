# Everd armor and retained conversation poses (#1101)

The 2026-11-17 `02-54-bdc140ar_proving` report is from the Dwarf Commoner
origin (the save's `bdc` area and Everd quest), despite the initial job description
calling it Dwarf Noble. Evidence is in the copied report, not the live player folder.

## Captured evidence

Both report saves have hero 118, appearance row 3 of `apr_base`, out of combat.
The before save already has Everd's chest armor (item 878); between the two saves,
the hero adds the helm (879), boots (884), gloves (885), and sword (881).
The later presentation binds `df_arm_dwpa_0`, `df_boo_dwpa_0`, `df_glv_dwpa_0`,
and `df_hlf_d01a_0`, with dwarf root translations scaled by 0.73.

The presentation dump says the conversation is **closed**, but the hero still has
the full-weight absolute layer `bdc140_everd.dlb / mh.dg_tn_90_l__0`, resolving to
`mh.dg_tn_90_l.ani`, frozen at 1.1500349 seconds. The next report (`02-54…-2`)
has the same layer at exactly the same clock despite five seconds of world time
passing. It overrides the bones of any movement pose underneath. The captured
base is `pause` / `mh_m.p.ani`, because the report was pressed while standing.

The asset regression logs the following directly from both saves and a second
fresh load of `save_now.json`:

| Subject | Appearance row | Weapon style | Torso skeleton | Walk/run track binding |
| --- | --- | --- | --- | --- |
| Hero, before | 3 | `dw` | `df_arm_dwpa_0`, 412 bones | 118/118 for each clip |
| Hero, after | 3 | `tw` | `df_arm_dwpa_0`, 412 bones | 118/118 for each clip |
| Hero, worn reload | 3 | `tw` | `df_arm_dwpa_0`, 412 bones | 118/118 for each clip |
| Duncan, all three loads | 46 | `dw` | `nm_bdy_duna_0`, 422 bones | 118/118 for each clip |

All use `movement.anb`, then `combat_movement.anb`, resolving `walkf` to
`mh_m.mov_wf.ani` and `runf` to `mh_m.mov_rf.ani`. Each sampled skeleton changes
between 0.1 and 0.5 seconds of each clip. The armor and weapon combination therefore
has working locomotion resources; equipment does not select an obsolete row.

## Cause and change

`CutsceneActors::switch` retains a line's ending animations while its conversation
stays open. A reply without staging changes the source to `None` while keeping
those poses. Closing the conversation also supplies source `None`: the old early
return for an unchanged source skipped cleanup. The presentation then applied the
retained pose indefinitely, across equipment rebuilds. Reloading starts a fresh
`CutsceneActors`, which explains why loading either report save alone loses the
bad pose. This state is not serialized in the simulation.

Clear retained and pending poses when the conversation is closed **before**
checking the source key. Leave retention during open replies intact (seated actors
must remain seated). Regression tests exercise the staged-line → unstaged-reply →
closed sequence, retention while replies remain open, and a direct close followed
by another conversation.

Trace diagnostics: `thedas_app::live=trace` logs appearance rows, model/skeleton
bindings and motions when the scene is built; `thedas_app::animation=trace` logs
the actual resolved base and layer resources each frame. Existing
`thedas_ui::locomotion=trace` logs gait phases and clocks.

## Replay verification

A controlled variant of the earlier copied `02-48…-2` Proving save uses the
`02-54` report's final equipment and chest position, with Leske nearby. Replay
Everd's conversation, choose response 0 through the Everd/Leske sequence, close
it, force a model rebuild, then move right. The before binary is this same
checkout with only `switch` restored to its original implementation; the after
binary has the cleanup fix. All engine runs use hidden windows or headless GPU
captures.

The before trace reproduces the exact reported turn resource and frozen clock
(`mh.dg_tn_90_l.ani` at 1.1500349 s) over an advancing `runf` clock. The after
trace releases that layer on closure and shows the turn-to-run transition and
running blend. This also reproduces through the forced model rebuild, without
any equipment-specific animation lookup failure.

Validation: formatting and workspace clippy with warnings denied pass; 1,480
workspace tests pass (27 ignored), plus the opt-in report asset test. The new
closure regression fails on the original implementation at the retained-pose
assertion, and passes with the fix. No simulation semantics changed; the sim
sweep gate is not required for this presentation-only correction.

## Related changes reviewed

- #929 culls poses according to visibility; the report explicitly says the hero is
  visible. It does not explain the frozen conversation clock.
- #947 preserves a cutscene-held creature's standing pose and re-poses changed
  clip shapes. That is correct while the conversation owns the pose; the stale
  retention outlives that ownership.
- #994 scales root translations; the report's 0.73 scale agrees with the dwarf
  appearance, and does not select the locomotion animation.
- #1048 adjusts floor placement and preserves scripted roots. It does not alter
  the retained clip or its clock.

## Duncan and #1112

Duncan is present in these saves: appearance row 46, body `nm_bdy_duna_0`, human
translation scale 1. His unrecorded gliding can be affected by the same cleanup
bug if a conversation leaves him a full-body pose, but there is no captured
presentation of that gliding to prove it is the same occurrence.

The #1112 dump is different: the conversation is **open**. Duncan has a current
`41541_canandam.cub / mh.po_tw_a6__1` gesture and an additive
`mh.po_2arm_01__1` layer (plus head/eye tracking). Clearing closed-conversation
state does not change this open-conversation blend. Do not mark #1112 fixed by
this cleanup; its transient shoulder penetration requires a separate gesture /
retained-layer investigation.
