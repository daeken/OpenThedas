# Fixed-clock playthrough control

`--control 127.0.0.1:PORT --control-step-ms 20` opts the full windowed app into deterministic automation. With `THEDAS_HIDDEN_WINDOW=1`, the app remains invisible and screenshots render offscreen. Normal interactive runs keep their existing real-time timing.

Only requested `run MS` steps advance game/UI/presentation/camera time. Each frame uses `min(remaining, control_step_ms)` milliseconds, including a shorter final frame; idle control polls do not advance any game time. A non-time input settles through a zero-time UI frame before acknowledgement. Asynchronous game/area opening pumps at zero time without consuming a run's duration. Queries report `control_time_ms` separately from simulation `time_ms`. Screenshots in fixed mode capture the settled frame immediately without ticking the interface. Physical controller devices and relative mouse motion do not affect fixed-clock runs.

The existing line-oriented loopback JSON protocol gains:

- `{"cmd":"input","text":"literal; text\nwith newline"}`: exact focused-field text; never parsed as steps.
- `{"cmd":"input","key":"escape"}`: physical-key tap through the window's key/text-focus paths. Optional `"down":true/false` holds/releases it. Names are case insensitive; letters, digits, F1–F12, navigation/edit keys and modifiers are accepted.
- `{"cmd":"input","steps":"pad stick-left 0,1"}`: hold a stick until changed; the older duration form still releases automatically.
- `{"cmd":"quit"}`: closes the game journal and exits normally.

An input request must contain exactly one of `steps`, `text`, or `key`. Existing step commands remain available. Coordinates use physical window/PNG pixels. Trigger buttons retain the same digital semantics as hardware input.

`status` preserves older fields and adds PID, fixed step/clock, loading, front-end state, hero and party health/position/combat/death/target records, tutorial pause, dialog resource/owner, and resolved active/completed journal text plus codex count. These are observations; no simulation state is changed by control inspection (resource caches may be populated).

The local stdlib Python CLI and operating instructions live in `OpenThedas-testdata/playtest-agent/`. They are not game assets and are not distributed in this repository. Session logs record exact requests and observations for reproduction. The clock and synthetic input are reproducible with the same build, inputs and assets; GPU bit identity and cross-build save compatibility are outside that guarantee. Boot logo movies are skipped for control sessions as before; the main menu and new-game/chargen/origin paths still run normally.
