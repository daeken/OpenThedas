# Scripted rendering comparisons

`capture_runner.py` replays a JSON scenario against two `thedas-app` binaries and
produces PNGs, per-frame pixel metrics, heatmaps and labelled three-panel comparisons.
Use it for faithful, deferred, UI, DA2 or mod rendering reviews. It does not change
the engine or require a control port.

Requires Python 3.11+ and Pillow. From the repository root:

```sh
python3 -m venv .capture-venv
.capture-venv/bin/python -m pip install -r scripts/capture-runner/requirements.txt

.capture-venv/bin/python scripts/capture_runner.py scripts/capture-runner/cousland.json \
  --save "$TESTDATA/01_cousland_start.json" --install "$GAME" \
  --baseline "$BASELINE_APP" --baseline-label main-d87590e5 \
  --candidate "$CANDIDATE_APP" --candidate-label my-branch \
  --output "$TESTDATA/codex-captures/my-stream/cousland-001"
```

Set `TESTDATA`, `GAME`, `BASELINE_APP` and `CANDIDATE_APP` to your local paths.
The sample save is deliberately not included; `--save` supplies it. All relative
save paths in scenarios are relative to the JSON file. CLI paths are relative to
the current directory. Output must be a **new** directory: existing runs are
never overwritten. Keep output outside the repository; it includes a save copy.

On the shared development Mac, Marten agents run the comparison through
`openthedas_game_slot` and build the binaries separately through
`openthedas_cargo_slot`, reusing their assigned target directory. Do not nest
slots. The runner starts only one engine process at a time; `--timeout` bounds
each capture, not the entire comparison.

For a commit comparison, build that commit's `thedas-app` in an isolated checkout
and supply its binary and commit label with `--baseline` and `--baseline-label`.
The runner does not switch branches or build implicitly. Binary SHA-256 hashes
are the authoritative identities; labels are human annotations. The same binary
can also compare renderers or environment options:

```sh
.capture-venv/bin/python scripts/capture_runner.py scripts/capture-runner/cousland.json \
  --save "$TESTDATA/01_cousland_start.json" --install "$GAME" \
  --baseline "$APP" --candidate "$APP" \
  --baseline-renderer faithful --candidate-renderer deferred \
  --baseline-label faithful --candidate-label deferred \
  --threshold 8 --amplify 4 \
  --output "$TESTDATA/codex-captures/my-stream/faithful-deferred"
```

## Scenario format

```json
{
  "save": "saves/after-dialog.json",
  "module": "single player",
  "steps": "key space; run 500",
  "options": {"size": "1280x720", "time": 3, "renderer": "deferred", "lighting": "baked"},
  "env": {"WGPU_BACKEND": "metal"},
  "baseline": {"label": "main", "env": {}},
  "candidate": {"label": "branch", "env": {"THEDAS_DEFERRED_VIEW": "normal"}},
  "cameras": [
    {"name": "hall", "camera": "-42.68,-3.63,2.01,2.5,-5"},
    {"name": "hall-later", "camera": "-42.68,-3.63,2.01,2.5,-5", "steps": "run 1000"},
    {"name": "follow", "camera": null, "steps": "run 500"}
  ]
}
```

- Exactly one of `save` or `area` is required. `area` is a viewer resource such as
  `hrt201d` or `none` for UI-only work; viewer scenarios reject steps because the
  engine would ignore them. `--save` replaces the scenario's source.
- `steps` uses the engine's existing `--steps` syntax. Each named camera starts
  fresh from the same save and replays common steps followed by its own steps.
  Entries are independent checkpoints, **not** successive views of one live run.
  Use simulation durations (`run MS`), never wall-clock sleeps.
- `camera` is `x,y,z,yaw,pitch`, world units/degrees, or `null`/omitted for the
  game's follow/conversation camera. A fixed camera applies from the beginning,
  including world clicks in the steps; use suitable input for each view.
- `options` accepts only rendering switches: `size`, `time`, `renderer`,
  `lighting`, `detail`, `shaders`, `fov`, `antialiasing`, `frame-effects`, `ui`,
  `ui-frames`, `vfx`. Spell keys as in the engine CLI. Baseline/candidate can each
  override `options` and `env`; both must use the same size. Unknown keys fail.
  An explicit `ui` option loads standalone movies instead of attaching the game
  UI. Use it for a static capture; the runner rejects it with any replay steps.
- Defaults are 1280x720, shader clock 3 seconds, faithful, medium detail,
  original shaders, FOV 60, AA 1, frame effects on, UI frames 30. Engine defaults
  apply to omitted optional switches. With a save, `run` steps advance the shader
  clock from `time` along with the presentation, as `--capture` normally does.
- `--env NAME=VALUE` adds a common override; `--baseline-env` and `--candidate-env`
  add side-specific overrides. CLI overrides win over JSON. Environment values
  are literal strings, passed without a shell. Path-valued environment options
  should use absolute paths. Use `--env THEDAS_KEYS=/path/to/keys.txt` when needed;
  never put keys, passwords or tokens in scenarios or environment values.

## Determinism and isolation

This deliberately reuses `--capture --save --steps`, whose run steps use fixed
30 Hz updates, and its fixed `--time` shader clock. Every capture starts a fresh
process with a fresh private user directory, isolated save slots and no audio.
The input save is copied once. Inherited `THEDAS_*` and `WGPU_*` variables are
removed; requested overrides are recorded. Mods default to `THEDAS_MODS=none`;
enable them explicitly for mod reviews. `HOME`, XDG paths and `THEDAS_USER_DIR`
are isolated in the child process. The caller's environment is untouched.
`THEDAS_HIDDEN_WINDOW=1` and `THEDAS_NO_AUDIO=1` are forced for every engine launch.
`--capture` itself creates no window, so captures do not require a display.

This runner uses the headless capture path, not the live `--control` path.
Headless checkpoints are not evidence of live-window timing or presentation
parity: for example, a standing particle effect may not be visible in a short
capture before its emitters warm up. For such cases, cross-check a settled
hidden live frame using the fixed-clock control mode where available (see
[fixed-clock playthrough control](../../docs/notes/control-playthrough.md)).

By default every side/camera is captured twice. Repeats must match **exact RGB
pixels**, independently of the comparison threshold; a mismatch writes the report
and returns failure. Repeat comparisons also get labelled triptychs and heatmaps.
`--repeat 1` skips this check; larger counts are allowed. If a GPU path has a
measured rounding/noise floor, `--repeat-threshold N` explicitly permits repeats
with maximum RGB delta <= N (default 0). Reports retain the nonzero-pixel counts
and raw deltas and label this **not exact**; the A/B comparison threshold is
independent. Do not silently treat noisy captures as bit-identical.
Fixed inputs do not promise equality across GPU vendors/drivers or changing game
assets, mods or external lightsets. Keep those constant during a run. Asset trees
are not hashed/copied. Binary and save hashes, resolved settings, environment
overrides and argv are recorded. A binary changing during capture is an error.

## Output and review

`baseline/` and `candidate/` contain each frame, repeat PNGs and per-process logs.
`delta/NAME.png` is a heatmap; `delta/NAME.comparison.png` is the **baseline |
candidate | delta** triptych with labels and metrics outside the original pixels.
`report.md` is the review table; `report.json` includes the same per-frame metrics
and provenance. `resolved-spec.json` records replay inputs; `input-save.json`
holds the private save snapshot. Errors retain logs and write `failure.json`.

For each pixel, `d = max(abs(baseline.RGB - candidate.RGB))` on 8-bit output
channels. A changed pixel has **d > threshold**, strictly greater (default 8).
The report gives its count/percentage, maximum channel delta (0–255), and mean
absolute channel delta. Alpha and PNG metadata are ignored. The heatmap displays
`min(255, round(d * amplify))`: black at 0, red at 128, yellow at 255. Default x4 means
red at a raw delta of about 32 and yellow at 64 or above. It shows even changes
below the counting threshold. Zero differences are stated explicitly.

Exit codes: **0** comparison completed with stable repeats; **1** repeats differ
beyond the repeat threshold or a frame exceeds `--fail-above PERCENT`; **2** invalid inputs/capture errors.
Ordinary A/B differences are findings, not failures unless `--fail-above` is set.
`--timeout` bounds each process (default 900 seconds); it kills/reaps that process
on timeout. No shell interpolation or process-name killing is used.

When staging comparisons for review, copy the triptych PNG first, then write its
`.caption` (under 1500 characters) stating left/right panels, heat scale,
percentage, threshold and PR number. State plainly when the delta is zero. Use
the shared `codex-captures/media-staging/<stream>/` folder; never `media-queue`.

Synthetic tests need no game files, GPU or engine binary:

```sh
.capture-venv/bin/python -m unittest discover -s scripts/capture-runner -p 'test_*.py'
```

## Cousland demo

The initial demo used the supplied camera, the post-conversation Cousland save,
`run 500`, a shader clock of 3 seconds and 1280x720 frames. At threshold 8:

| Comparison | Changed pixels | Max RGB delta /255 |
| --- | ---: | ---: |
| Historical main `0e3c29a` vs frozen renderer snapshot | 47.086263% | 253 |
| Same renderer snapshot vs itself | 0.000000% | 3 |
| Faithful vs deferred, same binary | 29.123589% | 174 |

Strict repeats exposed existing small rendering drift: up to 212/921600 nonzero
pixels (0.023003%), with maximum channel delta 8. The demo comparisons therefore
used explicit `--repeat-threshold 8`; all repeats passed that tolerance and their
reports say **not exact**. Raw repeat metrics and diagnostic panels were retained.
The same-binary comparison's delta is essentially zero at threshold 8, with 26
nonzero pixels below that threshold. These measurements use frozen example
binaries; each report records their SHA-256 identities.
