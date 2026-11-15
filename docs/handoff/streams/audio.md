# Audio stream handoff

The audio stream owned everything you hear: FMOD Designer 4 banks, music, voice, ambience, effects, reverb, occlusion and mixing. The goal was parity with DAOrigins.exe. Each behaviour was traced in the disassembly and then built, and every PR said what was traced and what is provisional. About 56 audio PRs were merged.

## How the stream was organised

- **Where it worked:** a git worktree at `OpenThedas-wt/audio` (branches `m1-audio-*`) with its own `CARGO_TARGET_DIR=../audio-target`, so its builds never fought the other streams'.
- **Private notes and tools** lived beside the repo in `OpenThedas-testdata` (never committed, because they hold addresses and data derived from the game):
  - `notes/handoff-audio.md`: every trace, with exe addresses. Read it before re-tracing anything.
  - `notes/pr-*.md`: PR bodies.
  - `qa/occlusion-recipe.md`: QA's recipe for occlusion takes.
  - `scripts/audio/`: `fev.py`/`fevscan.py`/`fsbscan.py` (bank inspection), `imports.py`/`funcs.py`/`pe.py` (exe lookups), `reverbsurvey.py`, `syscapture` (records the original under Wine), and `land.sh`.
- **Landing (`scripts/audio/land.sh "<title>" <body.md>`):** it merges the exact latest `origin/main`, then runs `cargo fmt --check`, `clippy --all-targets` and debug `cargo test`. If main hasn't moved, it pushes, opens or updates the PR and merges it. It retries up to 3 times when main moves. While a `HOLD` file sits beside it, it opens the PR but doesn't merge. That is how the coordinator's merge holds were honoured.
- **Responsibilities:**
  - Trace the exe's sound code and build it in `thedas-audio` and `thedas-app`.
  - Keep `docs/gaps/audio.md` true after each PR.
  - Answer QA's audio findings: occlusion runs 92/93, the Dalish ruins ambience, music levels.
  - Review the audio half of other streams' PRs (for example sim's #620).
  - Land our own green PRs. Sera gave standing permission in the thread; coordinator holds still applied.
- **Coordination:** messages to other streams went through the coordinator. QA asked for recipes, and audio wrote them (occlusion recipe, reverb waypoint). Triage decided what shipped in which update.

## What was built

- **Formats** (`thedas-formats`): FSB4 sample banks and FEV1 event projects (events, categories, parameters, sound defs, reverb presets, music-cue parameters).
- **`thedas-audio` crate:**
  - `engine.rs`: event engine (complex/simple events, pitch/volume randomisation, categories, `#duck:`/`#pause:` parameters, `#music:` cues, cones, fades, occlusion low-pass).
  - `mixer.rs`: buses, FMOD's Lowpass Simple step rule, limiter.
  - `reverb.rs`: I3DL2-style reverb, our own algorithm.
  - `area.rs`: `.are` SoundLists, placed-sound fields, audio volumes.
  - `library.rs`: bank discovery, case-insensitive install folders.
  - `decode.rs`: decoders.
  - `device.rs`: cpal output.
  - `wav.rs`: offline capture.
- **`thedas-app`:**
  - `audio.rs` (`GameAudio`):
    - Music director, traced at 0x5bb630: a track per game mode, the deferred combat cue, marker-synced changes and stingers.
    - Ducking per game mode.
    - Conversation, cutscene and ambient VO.
    - Sound sets and their probability modifier.
    - Animation sounds: footsteps, scuffs, rustles, foley, swings, blows, bodyfalls. They play from the creature's body or head emitter.
    - GUI sounds, boot-movie audio, script `PlaySound`/`PlaySoundSet`.
  - `occlusion.rs`: the exe's ray rule. It reads the simulation's collision shapes, read-only.
  - `main.rs` `audio_state()`: the per-frame snapshot of the simulation that audio reads.
- **CLI:** `thedas fsb`, `thedas fev`, `thedas play <install> event[@t][:programmer] --out x.wav`, `thedas audio-check`. `thedas corpus` covers FEV and FSB.

## How to verify

- `cargo test -p thedas-audio -p thedas-app` covers synthetic fixtures only: the event engine, cones, occlusion, the music director and the cue parser.
- **Real play, offline:** never play our audio through the speakers.

  ```bash
  THEDAS_INSTALL=<install> THEDAS_HIDDEN_WINDOW=1 HOME=<scratch> RUST_LOG=thedas_app::audio=debug \
    target/release/thedas-app --save <save.json> --capture x.png --steps "run 15000" --audio-wav x.wav
  ```

  - `--capture` makes the run exit.
  - `THEDAS_HIDDEN_WINDOW` keeps the window off the owner's screen.
  - Use a release build, because a debug build is too slow.
  - Use a scratch `HOME`, because the real `~/.openthedas` holds a live Discord webhook.
- **Bank questions:** `thedas fev <bank.fev>` lists events, parameters and categories.
- **Reference recordings of the original** are in `OpenThedas-testdata/reference/original/audio/` (main menu, Hideout reverb, Commons/Wilds swaps). The main menu matches to within 0.1 dB.

## Known gaps

`docs/gaps/audio.md` is the full list. The main open items:

- **Reverb:** the algorithm is ours, not FMOD's. Secondary reverb instances are untraced. Recordings of the Hideout exist, but the comparison hasn't been done.
- **Occlusion:** the box corner score, shape group 0x1000, and a comparison against a recording of the original.
- **Music:** theme-change timeout and delayed theme changes; other menu and credits music. Awakening and DLC themes don't resolve (`audio-check` reports 107 unresolved entries).
- **Mixing:** panning law, FMOD's 3D rolloff curves, global voice limits and 5.1 output aren't traced.
- **Sounds not yet played:**
  - Spells and abilities.
  - Projectiles.
  - Placeable sounds the exe plays from code (`Locked`, `Hit`).
  - Blows on placeable targets.
- **Provisional:**
  - The head emitter's height (the exe uses the model's node slot 3, 0xa66b40).
  - Our combat and interface game modes are inferred, not kept by the simulation.
  - Armour material comes from the chest item's base item.
