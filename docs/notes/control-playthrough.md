# Fixed-clock playthrough control

`THEDAS_HIDDEN_WINDOW=1 thedas-app --control 127.0.0.1:PORT --control-step-ms 20 ...` opts the hidden windowed app into fixed-clock automation. Audio is off (`--audio` and `--audio-wav` are rejected), and native keyboard, mouse, focus and controller input is ignored in this mode. Normal interactive runs keep their existing real-time timing.

Only requested `run MS` steps advance the fixed clock. Each frame uses `min(remaining, control_step_ms)` milliseconds, including a shorter final frame; idle control polls do not advance it. A non-time input gets a zero-time frame for direct UI callbacks before acknowledgement; queued simulation events wait for the next eligible positive-duration frame. While an asynchronous game or area is loading, the app pumps at zero time without consuming a run's duration. Queries report `control_time_ms` separately from simulation `time_ms`, which can stop behind screens or during pauses. A fixed-mode screenshot captures the current frame offscreen without ticking the interface.

The existing line-oriented loopback JSON protocol gains:

- `{"cmd":"input","text":"literal; text\nwith newline"}`: exact focused-field text; never parsed as steps.
- `{"cmd":"input","key":"escape"}`: physical-key tap through the window's key/text-focus paths. Optional `"down":true/false` holds/releases it. Names are case insensitive; letters, digits, F1–F12, navigation/edit keys and modifiers are accepted. A physical key does not synthesize printable text. Send literal `text`, or use the existing `windowkey` step with explicit text to replay a particular keyboard event.
- `{"cmd":"input","steps":"pad stick-left 0,1"}`: hold a stick until changed; the older duration form still releases automatically.
- `{"cmd":"quit"}`: closes the game journal and exits normally.

An input request must contain exactly one of `steps`, `text`, or `key`. Existing step commands remain available. Coordinates use physical window/PNG pixels. Trigger buttons retain the same digital semantics as hardware input. Send one request at a time and wait for its reply before observing the result. Observations from another connection can see intermediate state while input is pending.

`status` preserves older fields and adds PID, fixed step/clock, loading, front-end state, hero and party health/position/combat/death/target records, tutorial pause, dialog resource/owner, and resolved active/completed journal text plus codex count. These are observations; no simulation state is changed by control inspection (resource caches may be populated).

The local stdlib Python CLI and operating instructions live in `OpenThedas-testdata/playtest-agent/`. They are not game assets and are not distributed in this repository. Session logs record exact requests and observations for reproduction. The fixed clock and synthetic input sequence are reproducible with the same build, inputs and assets. Worker completion, wall-time loading presentation (for example, the travel-map outro), GPU bit identity and cross-build save compatibility are outside that guarantee. Boot logo movies are skipped for control sessions as before; the main menu and new-game/chargen/origin paths still run normally.
