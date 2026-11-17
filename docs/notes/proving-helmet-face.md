# Proving helmet face audit

The supplied helmet case does **not reproduce a missing or black face** on main
`d6b15225`. No renderer behavior is changed by this investigation. The tested
helmet's metal mask covers most of the face; the eyes and adjacent skin remain
visible through its openings. This is a bounded reproduction result, not a claim
of original-game framebuffer parity or an explanation of every possible helmet.
The requested rendering fix remains unresolved without a failing view.

## Inputs and related changes

The copied `render-review/sera-1117/` reports are the source of saves and camera/
animation information. No player journal or live save directory is used.
The 02:44 Beraht's Shop report has **no helmet equipped**, so its “gfx bug” image
is not evidence for this case. The later Proving saves equip object 879,
`bdc140im_everd_helm`, in slot 5: BaseItem 45, ModelVariation 11. The actual model
resolved from the save is `df_hlf_d01a_0`.

The copied character is Natia, a dwarf female in the `bdc` Commoner origin;
these are not Noble-origin snapshots. The main experiment uses
`2026-11-17-02-56-bdc140ar_proving/save_now.json`. The same helmet/morph combination
is present in the two 02:54 current saves and the last report's earlier save.

Reviewed PRs #929, #947, #994 and #1048 cover pose culling, re-posing after clip
changes, appearance-scaled root translations and floor/root ownership. All are
already incorporated. The diagnostic poses every skeleton of the hero together,
first at a fixed idle and optionally at the recorded placement and animation.
It does not alter pose caches, translations, floor placement or equipment state.
Subsequent #1074 changed level light picking; its character-class path still uses
the placement sphere and the existing character picker. No causal bisection to
one of these PRs is established.

## Draws

Indices below are from a hero-only objects scene. They are not indices into the
tester's complete scene. Hair/beard are omitted by `hide_hair`; head, eyes and
lashes remain. There is no helmet-specific head substitution.

| Draw | Model / mesh chunk | Material / shaders at Medium detail |
| --- | --- | --- |
| 3 | `df_hlf_d01a_0` / `df_hlf_d01_0.msh:df_hlf_d01_helmetm1` (3,408 vertices, 12,378 indices) | `pn_hlf_d01a.mao#t3_arm_lst`, Character / NoTint, `Char1.vsh` + `Ch1NoTnt.psh`, Character state |
| 6 | `df_uhm_basa_0` / `df_uhm_bas_0.msh:df_uhm_bas_facem1#df_cps_p01` (2,288 vertices, 10,788 indices) | `uh_hed_fema.mao#df_cps_p01:facem1`, Face / Default, `Face1.vsh` + `Face0.psh`, Face state |
| 7 | `df_uem_basa_0` / `df_uem_bas_0.msh:df_uem_bas_eyesm1#df_cps_p01` | `pn_eye_nrma.mao#df_cps_p01:eyesm1`, `Eye0.vsh` + `Eye0.psh` |
| 8 | `df_ulm_basa_0` / `df_ulm_bas_0.msh:df_ulm_bas_lashesm1#df_cps_p01` | `pm_ulm_bas.mao#df_cps_p01:lashesm1`, `Eye0.vsh` + `Eyelash0.psh` |

The helmet resolves diffuse/normal/specular textures
`pn_hlf_d01a_0d.dds`, `pn_hlf_d01a_0n.dds`, `pn_hlf_d01a_0s.dds`.
All are present. Its semantic is opaque; interpreting the diffuse alpha as a
visibility mask would be incorrect for this selected material.

The face's material texture bindings all resolve: diffuse/normal
`uh_hed_fema_0d` / `uh_hed_fema_0n`, brow/stubble `uh_hed_stb_0t` / `_0n`,
emotion masks `uh_hed_mlw` / `uh_hed_mup`, emotion normal `uh_hed_emo_0n`,
tint `uh_hed_maka_0t`, age diffuse/normal `uh_hed_olda_0d` / `_0n`, and tattoo
`uh_tat_p01_0t` (all DDS). The eyes' `LM.dds` entry is a runtime lightmap
placeholder, not a missing face texture. `draws.json` records authored bindings;
not every binding is sampled by every selected shader.

## Lighting and shadow receipt

Head, eyes, lashes and helmet are character-class draws in room 0,
`orz500d_0`, using probe index 1, `orz500d_probe_2to.mtx`. The selected static
character-light slots are:

| Slot | Position | Radius | RGB |
| --- | --- | --- | --- |
| A | (121.616425, -63.541588, 4.9130535) | 5000 | (0.2205, 0.203, 0.1295) |
| B | (74.183784, 55.817772, 4.9130535) | 5000 | (0.72, 0.58, 0.27) |
| C | (-14.095032, -27.701618, 4.9130535) | 5000 | (0.2795, 0.3705, 0.2795) |

There are no selected dynamic D/E lights. Characters do not take ambient light
nodes; their ambient irradiance is supplied by the probe.

The Proving layout `orz500d.arl` has 3122 = 0, so `scene.sun` is absent.
There are no active faithful sun-shadow partitions. Faithful point lights have
no point-shadow maps. Consequently the helmet cannot darken the face through a
runtime shadow in this fixture. The authored cast-runtime-shadow flag is true
on both meshes; this does not create a shadow map without a sun. The MMH
receive-runtime-shadow flag is not separately modeled by this renderer; this
audit does not claim that general flag handling is complete.

## Controlled captures and regression

The offscreen `helmet_face_probe` example freezes the copied hero with its
equipment and morph, poses a neutral humanoid idle at 0 seconds, keeps shader
time at zero, uses faithful shaders at Medium detail, and disables post effects
and antialiasing. Other dynamic actors and attached effects are omitted.
The camera is `(63.805,-19.745,1.41,151.806,0)`, at 1280×720. These are diagnostic
controls, not unmodified reproductions of the tester's running frame.

| Control against equipped baseline | Pixels changed above 2/255 | Full-frame percentage | Result |
| --- | --- | --- | --- |
| Remove only helmet; keep hair hidden | 38,871 | 4.217773% | A textured, lit face is revealed |
| Remove morph's head/eyes/lashes | 3,095 | 0.335829% | Visor openings lose visible eyes/skin |
| Disable every runtime caster | 0 | 0% | Essentially zero delta; max channel difference 1/255 |

The dark area in this tested view belongs to helmet coverage, rather than a head
variant swap, missing diffuse texture, helmet self-shadow, or missing probe.
No behavior correction is justified by these controls. No “after fix” frame is
claimed. Labelled comparisons have a third **Delta ×4** absolute RGB panel;
their captions include the threshold and changed percentage. Captures and
derived metadata stay outside the repository under the job's testdata directory.

An additional fixed-idle capture with frame effects enabled still shows the eyes
and adjacent skin. Replaying the last report's recorded `runf` clip
(`mh_m.cmov_rf.ani`, raw clock 1.8266685 s) and world placement, with frame effects
enabled and a further-back camera `(64.5,-20.11,1.3,151.806,-4)`, shows the
helmet pitched downward. That camera sees the brow/metal front rather than the
same open visor area as the neutral idle. This supports a view/coverage
explanation for that appearance; it does not establish original-game animation
parity or reproduce the history of a long-lived pose cache.

The example's ignored, install-dependent regression renders the supplied save
three times. It checks that helmet/head/eyes/lashes are retained and hair is
hidden; removing the head must visibly change the image, and disabling shadow
casters must not change pixels above 2/255. Two synthetic tests cover replaying
the complete nested pose inputs from `presentation.json`, including the raw
clock rather than its already wrapped sample time, and refusing incomplete or
unrecoverable inline animations. These protect the diagnostic and the observed
working case; they are **not a regression test for a demonstrated renderer fix**.

Reproduce with the job's own `CARGO_TARGET_DIR`, through `cargo-slot.sh`, `-j 2`:

```sh
cargo build -j 2 -p thedas-app --example helmet_face_probe
helmet_face_probe --install "$THEDAS_INSTALL" --save "$THEDAS_HELMET_SAVE" \
  --out "$THEDAS_HELMET_OUT" --camera 63.805,-19.745,1.41,151.806,0
```

Repeat into separate output folders with `--no-helmet`, `--no-head` or
`--unshadowed`. `--no-helmet` removes full-helmet (`_hlf_`) models only and keeps
the same hair-hiding input, to isolate the face. `--presentation PATH` restores
the hero's recorded world matrix and full animation-layer tree instead of the
idle; `--rest` selects the bind pose. Recorded-pose cameras must be reframed for
that pose. `--frame-effects` enables the level's post effects for an additional
presentation check. The example does not reproduce a long-lived renderer's pose-cache
history, lip sync, head tracking not represented by the saved clip, or the
complete frame's actors/effects. `--metadata-only` omits GPU work.

```sh
cargo test -j 2 -p thedas-app --example helmet_face_probe
# Set THEDAS_INSTALL, THEDAS_HELMET_SAVE and THEDAS_HELMET_OUT for the copied
# 02:56 Proving save, with THEDAS_HIDDEN_WINDOW=1 and THEDAS_MODS=none:
cargo test -j 2 -p thedas-app --example helmet_face_probe proving_visor \
  -- --ignored --nocapture
```

Review should identify a failing helmet/view or confirm the covered appearance
before any renderer behavior changes. The separately queued dropped-helmet
equipment/placement issue is outside this investigation.

Validation on merged main `d6b15225`: formatting and workspace/all-target clippy
with warnings denied pass; debug workspace tests **1,478 passed, 26 ignored,
zero failed**. All **3 probe tests pass**, including the explicitly enabled
offscreen GPU regression against the copied save. The example builds. No
simulation behavior changes, so the simulation-semantics sweep gate does not
apply. The requested black-face fix remains unproven and is not marked complete.
