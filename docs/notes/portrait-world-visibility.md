# HUD portrait inherits cutscene visibility (#1060)

## Reproduction and cause

The reported Human Noble checkpoint does **not** preserve the black portrait.
Loading `hall-checkpoint.json` in the reported playtest binary produces a visible
hero portrait. Its cached portrait and the renderer's visibility state are not
part of the simulation save.

Replaying the report's `session.jsonl` through the playtest harness, starting at
`start human-noble`, reproduces the black HUD portrait after the introduction and
after movement. Inventory still draws the hero.

Engine diagnostics at the first portrait capture after character creation showed
ten hero model draws ready, with renderer draw opacity **0**. The introduction's
hidden-actor list applies that opacity in `Live::sync`. `Live::portraits` reused
the same renderer, so its otherwise successful capture returned a black image.
`PortraitClock` marked that member as drawn. Ending the cutscene and moving do
not change its simulation appearance key, so there was no later capture to
replace the black one.

## Correction

Portrait capture temporarily sets its subject's prop opacity to one, submits the
capture, then restores world opacity even if capture returns an error. The world
renderer's hidden-actor and fade behavior is unchanged. The portrait camera uses
the actual posed placement, including cutscene displacement and floor snapping;
face offsets measured at scene construction are transformed into that placement.

The cache key includes each subject's resolved model/morph/attachment/tint list
and whether bounds, face and placement are ready, alongside the existing
simulation appearance, morph, equipment and portrait-slider inputs. A missing
or empty prop is omitted so the existing missing-portrait retry remains active.
Movement and opacity are intentionally absent from the key: a world cutscene or
camera fade should neither blank a portrait nor force a capture every frame.

## Regression coverage

The ordinary tests cover moved/rotated framing, animated head offsets, creatures
without a face mesh, and appearance/morph/equipment/framing cache invalidation.
The opt-in `portrait_draws_a_hidden_displaced_actor_and_preserves_world_visibility`
test loads a local save, verifies model readiness changes the key, displaces and
hides the actor in the renderer, checks the portrait contains visible pixels,
and checks world opacity, simulation position and the stable cache key afterward.

Run the GPU test with a local game install and save (neither belongs in git):

```sh
THEDAS_INSTALL=/path/to/game THEDAS_PORTRAIT_SAVE=/path/to/hall-checkpoint.json \
  cargo test -p thedas-app \
  live::portrait_tests::portrait_draws_a_hidden_displaced_actor_and_preserves_world_visibility \
  -- --ignored --exact
```

On the shared Mac, also use the job's `CARGO_TARGET_DIR`, `cargo-slot.sh` and
`-j 2`, as required by the job rules. Before/after checkpoint captures use the
capture runner; a separate harness replay tests the presentation state the save
cannot retain.

## Executable evidence and provisional behavior

The existing UI code records `UpdatePortraitMessage` construction at 0x607b70
(vtable 0xb02298), handled by `GUIPaperdoll`. This investigation traces our engine's
state leak; it adds no new DAOrigins.exe trace. The original's exact message
sources and camera framing remain **provisional**, as do the existing portrait
gain, light-column handling and light-rig placement. This change does not claim
new executable parity for those details and changes no simulation semantics.
