# Rendering stream: handoff

The rendering stream owned how the world looks: the faithful renderer that runs the original
D3D9 shaders and material states, the scene built from the game's models and levels, animation
posing, cutscene and conversation presentation (cameras, actors, lights, effects), lip sync, and
Dragon Age II's materials, shadows and characters. It also wrote the clean-room SpeedTree
specification. The deferred renderer is a separate stream (Codex at the end); this stream
reviewed its changes for the faithful path.

## Organization and responsibilities

- **One thread, one worktree.** This stream worked in `OpenThedas-wt/render` with its own target
  folder (`OpenThedas-wt/render-target`), on `m1-*` branches cut from `origin/main`, one branch
  per change. It never created or switched worktrees.
- **The coordinator** routed work between streams (rendering, simulation, UI, audio, triage/QA,
  DA2, deferred, mods) and relayed Sera's requests. Messages to other streams went through it.
  This stream reviewed other streams' PRs when they touched `gpu.rs`, `d3d_gpu.rs` or `live.rs`'s
  render flow, and replied "OK" or with what to change.
- **SpeedTree clean-room role.** This stream wrote the specification in `docs/speedtree/`
  (`README.md`, `format.md`, `generation.md`) from reading the SDK sources. It must never write
  generator code or edit `thedas_formats::speedtree_gen`, and never copy SDK code into the repo.
  The generator is implemented by another stream from the spec alone.
- **Merging.** Sera allowed streams to merge green PRs themselves. Every merge: merge the latest
  `main` into the branch; run `cargo fmt --all`, `cargo clippy --all-targets` and debug
  `cargo test` on that exact result; check `git diff --stat origin/main...HEAD` lists only the
  intended files; confirm `main` has not moved; then `gh pr merge N --merge`. If the permission
  layer refuses a merge, stop and report; never try another form. A scratch script did this
  (`checkloop.sh` / `check_and_merge.sh`; it reran when `main` moved, which happened often).
- **Screenshots for #media** went to `OpenThedas-testdata/media-queue/rendering/` as
  `<name>.png` then `<name>.txt` (caption; say which side is which; no paths), posted
  automatically.

## Current state (2026-11-14)

No open PRs and no uncommitted work. Merged in the last two days, newest first:

- #736 alpha-to-coverage for opaque cut-outs when multisampling (a modern extra).
- #731 hide party members a cutscene does not cast (sim's `cutscene_hidden_party`; fixes the
  Dwarf Commoner hero in the brazier).
- #729 lip sync on the voice-over's playback clock.
- #725 lip sync for cutscenes' own voiced lines and lines said in passing.
- #720 MSAA (`--antialiasing N`), #706 FXAA stand-in, both for the original's `AntialiasingLevel`.
- #714 faster camera collision sweeps, #711 faster creature posing (cached track nodes, threads).
- #705 DA2 conversation camera (face height, DOF tables by name), #701 aiming reticle under
  creatures, #693 lip sync (FaceFX), #679 DA2 dynamic sun shadows.
- Earlier: DA2 materials/lightmaps/per-vertex light/sky/fog/characters/hair/beards, cutscene
  lights, head tracking, cutscene shake, depth of field, selection rings, and the faithful
  D3D9 path itself.

**Parked:** branch `m1-da2-runtime-light` (DA2 run-time lit props form; no piece in the levels
checked used it, so it was never proposed).

**Waiting on others:** UI is wiring the options screen's Anti-Aliasing slider
(`ClientOptions.AntialiasList`/`AntialiasIndex`) to `thedas_render::msaa::supported_samples` and
`Live::antialiasing`; until then MSAA is the `--antialiasing` flag (default 1 = off, FXAA on).

## How to build, run and verify

- Build: `CARGO_TARGET_DIR=<target> cargo build --release -p thedas-app`.
- **Always use a scratch HOME** for runs (Sera's real `~/.openthedas` holds a live webhook):
  `HOME=OpenThedas-testdata/render-review/wind/home`, and pass `--saves $HOME/saves`.
- Headless capture: `thedas-app --install <DAO install> --save <save.json> --no-audio
  --size 1280x720 --steps "run 2000; ..." --capture out.png`. Steps include `run MS`,
  `pad a`, `talk TAG`, `cutscene NAME`, `hover X,Y`, `click X,Y`, `key ...`.
- Area viewer: `--area <layout> [--camera "x,y,z,yaw,pitch"] [--objects]`. DA2:
  `--install /Volumes/External2/da2 --area ht_0_hightown` etc.
- Useful saves (OpenThedas-testdata): `01_cousland_start.json` (Human Noble hall conversation:
  `"run 2000; pad a; run N"`; Duncan's face at crop 780,230 180x180), `07_ostagar_arrival.json`
  (Ostagar camp, the heavy scene), `qa/saves/dc_21_wolves.json` (Korcari Wilds, party and
  foliage), `qa/saves/dc_19_duncan.json` + `cutscene pre100cs_joining_ritual_dm` (cutscene
  lights). Some qa saves have `tutorial_paused: true`, which holds `cutscene X` steps at t=0;
  clear it in a scratch copy.
- New game into the Dwarf Commoner opening: triage's `triage/convcam/bin/convcap.sh dcommon`
  steps (drop the final `pad a` to watch the intro and Beraht's walk-in).
- Original reference captures: `OpenThedas-testdata/reference/original/...` (QA runs the
  original; never run it yourself).
- Profiling: run the app headless with a long `run`, wait for "Hud up" plus 12 s, then
  `sample <pid> 8 -file out.txt` and compare main-thread shares (the machine is usually loaded,
  so wall times mislead). `THEDAS_BENCH_FRAMES=n` times the render loop.
- Comparing frames: `ffmpeg -i a.png -i b.png -lavfi psnr -f null -`; label A/B by measuring
  region averages, not by eye. Captures of busy areas differ run to run (about 52 dB in the
  camp) from their own timing.
- Control ports for live sessions are 7970-7979: check with `lsof` before binding, kill by pid.
  No speaker output: use `--no-audio` or `--audio-wav out.wav`.
- Debugging technique that worked repeatedly: temporary env overrides (shader constants, forced
  face curves, model-name logs) in a scratch build, removed before committing.

## Key files and design decisions

- `crates/thedas-render/src/d3d.rs`, `d3d_gpu.rs`: the faithful path. Materials are drawn with
  their original D3D9 shaders (translated by `thedas-shader`) and state files; constants are
  filled by usage (`ConstantContext::value`). Pipelines are cached by program, format and sample
  count. SpeedTree draws are instanced.
- `gpu.rs` (`AreaRenderer`): frame entry points `render`, `render_main`/`render_over`,
  `capture_with` (with an `under` hook for ground marks), `frame_view`/`depth`/`resolve` for
  MSAA, `finish_frame_with` for the frame buffer effects and FXAA.
- `msaa.rs`, `fxaa.rs`: anti-aliasing. Depth is resolved by its first sample for the effects.
- `scene.rs`: building the scene from levels, models, materials, lightmaps, probes, DA2 `bmat`.
- `animate.rs`, `skeleton.rs`: posing. Layers carry precomputed track nodes; `pose_props`
  looks things up serially and poses on up to four threads.
- `crates/thedas-app/src/live.rs`: the game's frame (level, rings, reticle, objects, effects),
  cutscene actors and lights, lip sync hook. Order in a frame: level → reticle → rings →
  objects → effects → resolve → effects passes → interface.
- `crates/thedas-app/src/faces.rs`, `crates/thedas-formats/src/facefx.rs`: lip sync. FaceFX
  `.fxe` per line (named like the voice sample), `.fxa` rig per race (named by field 6248 of
  `<race><gender>_uhm_basa_0.mmh`). Rig space to skeleton: centimetres / 100, y and z swapped,
  quaternion (w,x,y,z) → (−x,−z,−y,w). The face graph evaluation is traced (0xa19b60).
- `crates/thedas-app/src/talkcam.rs`, `cutcam.rs`, `cutactors.rs`: conversation and cutscene
  cameras and actors. A conversation line's cutscene comes either inline (`CUT ` struct) or as a
  `CUTR` reference played through the cutscene mode.
- Traces and notes live in `OpenThedas-testdata/notes/` (`cutscene-lights-trace.md`,
  `da2-shadows-notes.md`, `headtracking-trace.md`, and others).

## Known gaps (see docs/gaps/rendering.md and friends)

- Lip sync: bone pose blend inferred; rig choice from the head morph's race prefix; narration
  mixed into a cutscene's sound and facial tracks between lines not animated (AUD-052, CAM-013,
  RND-070, FMT-027).
- Multisampling not yet on the options screen; hair's blended cut-outs still step (RND-109).
- DA2: run-time lit props parked; DA2 tables are numbered differently from Origins (look things
  up by name when ids miss).
- Camp frame cost: what is left is the GPU, wgpu's per-draw encoding and validation, the
  interface (ruffle) and the simulation tick; draw batching would buy little (the trees are
  already instanced; about 1,500 real draws).

## What a newcomer must not break

- **Parity first.** Every change to how the original looks needs a trace, the data, or an
  original capture behind it, and anything guessed is marked provisional in its doc comment and
  its gap entry. Modern extras (FXAA, alpha-to-coverage) are opt-in or replace nothing the
  original did.
- **Origins frames stay identical** when touching DA2 or deferred code: compare a capture
  (Korcari Wilds, the Human Noble hall) to the pixel before merging.
- **The faithful path's draw order** (level → reticle/rings → objects → effects) and the
  multisampled flow (`frame_view`, `depth`, `resolve` before the effects) are relied on by the
  deferred path.
- **Rendering reads the simulation and never writes it.**
- **SpeedTree clean room:** the spec writer never writes generator code.
- **No game data in the repo**, no SDK or xoreos code, no secrets; use the scratch HOME.
