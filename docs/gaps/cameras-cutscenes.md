# Known gaps: cameras, cutscene presentation and startup

[Back to the index](../KNOWN_GAPS.md)

Exploration, conversation and cutscene cameras, cutscene actors on screen, movies, command-line flags and configuration.

## Code inventory

Found by reading the code at main on 2026-11-12: provisional markers, stubs, ignored inputs and features with no code. "Original: not traced" means nobody has yet checked what the original does. Line numbers drift; search for the quoted names if a line has moved.

### Cutscene camera

- **CAM-001** **Camera roll is dropped.** Original: the rotation track's third curve turns about the view axis. Ours: dropped (renderer camera has no roll). Evidence: `crates/thedas-app/src/cutcam.rs:13`. Size: S. Depends: none. Kind: provisional.
- **CAM-002** **Blends between cuts are not followed.** Original: 0x6c45f0's blend time. Ours: every cut is a hard cut. Evidence: `crates/thedas-app/src/cutcam.rs:16`. Size: M. Depends: none. Kind: provisional.
- **CAM-003** **Camera axis and FOV convention guessed.** Original: not traced. Ours: camera looks along local +Y, Z up; FOV taken as vertical; a camera with no FOV track sees 45 degrees. Evidence: `crates/thedas-app/src/cutcam.rs:13`, `crates/thedas-sim/src/cuttracks.rs:47`. Size: S. Depends: none. Kind: provisional.
- **CAM-004** **LinkToActor is not followed.** Original: `ActionLinkToActor` (kind 27; 14 camera tracks in the game). Ours: while a camera is linked no camera is given (the gameplay camera stays). Evidence: `crates/thedas-sim/src/cuttracks.rs:51`, `crates/thedas-sim/src/cuttracks.rs:704`. Size: M. Depends: none. Kind: missing.
- **CAM-005** **Several cutscene track kinds are not played.** Original: the factory 0x7ddaa0 builds every kind (class names from the executable). Ours: `cuttracks` reads kinds 4, 11, 13, 17, 18, 19, 20, 24, 25, 27, 28, 32, 33, 34, 35, 38 and 41, and `cutscene` the spoken lines (23), sounds (31) and movies (42); any other kind falls through `_ => {}`. Not played, with their count across the single player campaign's 2,574 cutscenes: 45 `ActionSetGore` (704; field 5750), 44 `ActionAnimateFace` (201), 47 `ActionSkipMarker` (117), 43 `ActionAutoAdjustHeight` (62), 39 `ActionOrientationFollowsCurve` (58), 46 `ActionSetFog` (7) and 36 `ActionTogglePhysics`. Played with limits: a hidden actor (kind 32) is drawn at no opacity and still casts its shadow, and weapons drawn by kind 41 change hands at once, with no draw animation. A light actor (`base_light`, 92 in 56 cutscenes; kinds 33, 34 and 35 set its radius, intensity and colour) lights the level and the objects from where its tracks put it, picked together with the level's lights as each draw's lights are picked; its intensity scales its colour and weighs its pick (0x46b47e, 0x53d35f), but two more factors of the register colour (light +0x1bc and 0x46a470's result, both taken as 1) are not traced, and nothing is compared with the original. Actors that are props rather than creatures (a model in field 5202; 1,843 across 416 of the campaign's cutscenes) are drawn when the model is a mesh (placeables, weapons: cards, arrows, daggers, coins), only in cutscenes played by name, not in a conversation line's own; those that are effects (`fx…` models: blood, fire, smoke, about a third of them) are not. Evidence: `crates/thedas-sim/src/cuttracks.rs:1`, `crates/thedas-app/src/cutactors.rs:80`. Size: L. Depends: none. Kind: missing.
- **CAM-006** **Track span and end-blend rules guessed.** Original: blending out at a track's end (0x7d9450, 0x7d92e0). Ours: a track applies from its start and holds its last value; missing transition is linear; end blend not followed. Evidence: `crates/thedas-sim/src/cuttracks.rs:47`, `crates/thedas-sim/src/cuttracks.rs:409`. Size: S. Depends: none. Kind: provisional.
- **CAM-007** **Jump-to-camera field 5571 not followed.** Original: a camera named after another actor's jump-to-place (0x7df369). Ours: ignored (no game cutscene sets it, per comment). Evidence: `crates/thedas-sim/src/cuttracks.rs:330`. Size: S. Depends: none. Kind: provisional.
- **CAM-009** **Camera shake outside cutscenes, and a cutscene shake's second kind.** Original: a cutscene's `ActionShake` (kind 20; on every conversation camera) turns its actor by seeded noise (0x7da990, 0x7d0ed0, 0xa3c530), and the game camera shakes on scripted events. Ours: the cutscene action's turning kind (field 5600 = 1) is followed, noise and all; its other kind (2, not read in the executable; taken to shift rather than turn) and any shake of the game camera outside cutscenes are not. Provisional: that the shake's turn comes after the camera's jumps and links. Evidence: `crates/thedas-sim/src/cuttracks.rs:1`, `crates/thedas-app/src/follow.rs:1`. Size: S. Depends: none. Kind: missing.
- **CAM-010** **README lists JumpToCamera as open, code follows it.** Original: n/a. Ours: kind 24 is read and applied; README status line still says open. Evidence: `README.md:32`, `crates/thedas-sim/src/cuttracks.rs:316`. Size: S. Depends: none. Kind: bug.

### Cutscene actors

- **CAM-011** **Animations are layered, not weight-blended.** Original: not traced. Ours: animations laid over each other by bones moved; snap-to-walkmesh sets actors on the level surface; loop rule partly inferred (an animation loops when the creature's own trees loop it, else when the cutscene's tree loops its node, as every `.cub` node does: a passer-by's `mov.walkf` cycle keeps walking while its position track moves it; the executable's own wrap of a track's time is not traced, only its stop 0.001 s short of the track's end, 0x7db400); the cutscene's own creatures take their template's look. Evidence: `crates/thedas-app/src/cutactors.rs:16`. Size: M. Depends: none. Kind: provisional.
- **CAM-012** **Animations held until the conversation closes.** Original: leaves the model's animation as the cutscene set it (per comment). Ours: kept until the conversation closes, then dropped. Evidence: `crates/thedas-app/src/cutactors.rs:110`. Size: S. Depends: none. Kind: provisional.
- **CAM-013** **Facial animation in cutscenes: only voiced lines.** Original: lines a cutscene voices move their speakers' faces, and its facial tracks animate faces between lines. Ours: a cutscene's own voiced lines (field 5564 0) play their FaceFX animation on the object drawn for the line's actor (AUD-052); lines mixed into the cutscene's sound (5564 2, the intro's narration) and any facial tracks are not animated. Evidence: `crates/thedas-app/src/faces.rs:1`, `crates/thedas-app/src/cutactors.rs:1`. Size: S. Depends: AUD-052. Kind: provisional.

### Conversation camera

- **CAM-014** **Camera moves between lines, and within lines without camera cuts, are not followed.** Original: a line's own cutscene drives the camera for the whole line (traced: `GameModeDialog` 0x68e53c -> 0x7f0960 builds it, its tick 0x7e3d30 hands the camera the active camera actor's pose). Ours: a line whose cutscene cuts between camera actors follows those cuts (`line_cut_view`); other lines hold one shot, nothing blends from one line's camera to the next, and the view holds the last pose once the cutscene has run its length (not traced to the end). The `decision` camera shown while the player chooses is our own invention. Evidence: `crates/thedas-app/src/talkcam.rs:13`, `crates/thedas-app/src/talkcam.rs:445`. Size: M. Depends: CAM-002. Kind: provisional.
- **CAM-015** **`StageAtCurrentLocation` is not followed.** Original: stage placed at the speaker's current location. Ours: not followed. Evidence: `crates/thedas-app/src/talkcam.rs:16`. Size: M. Depends: none. Kind: missing.
- **CAM-016** **Decision camera timing uses the cut length.** Original: the line's voice-over length. Ours: the cut's length (5000); lines without one keep their shot. Evidence: `crates/thedas-app/src/talkcam.rs:98`. Size: S. Depends: none. Kind: provisional.
- **CAM-017** **Default framing details guessed.** Original: 0x68d4b0 (listener from 0x68d270, left eye bone, FOV 40 / renderer value). Ours: listener is the party leader (or owner), eyes are the eye meshes' centre, divisor 1, FOV vertical. Evidence: `crates/thedas-app/src/talkcam.rs:152`, `crates/thedas-app/src/talkcam.rs:287`, `crates/thedas-app/src/talkcam.rs:300`. Size: S. Depends: none. Kind: provisional.
- **CAM-018** **Over-the-shoulder conversation shots.** Original: over-the-shoulder. Ours: no code found beyond stage cameras and the default framing. Evidence: `crates/thedas-app/src/talkcam.rs:1`. Size: M. Depends: none. Kind: missing.
- **CAM-019** **Cutscene subtitle doc may be stale.** Original: n/a. Ours: `ui.rs` says nothing is staged behind cutscene subtitles (no camera, animation or voice), while cutcam/cutactors/audio stage them. Evidence: `crates/thedas-app/src/ui.rs:1204`. Size: S. Depends: none. Kind: bug.

### Exploration camera

- **CAM-020** **Wheel zoom step guessed.** Original: an option times 0.05 (0x7714c4, 0x7edbe7). Ours: 0.1 per notch. Evidence: `crates/thedas-app/src/follow.rs:23`. Size: S. Depends: none. Kind: provisional.
- **CAM-021** **Cut threshold guessed.** Original: not traced. Ours: a look-point jump of 10 m or more is a cut. Evidence: `crates/thedas-app/src/follow.rs:70`. Size: S. Depends: none. Kind: provisional.
- **CAM-022** **Far-end (overhead) collision not traced.** Original: checks only that the camera is not inside something (0x6c3180). Ours: no check at the far end. Evidence: `crates/thedas-app/src/follow.rs:85`. Size: S. Depends: none. Kind: provisional.
- **CAM-023** **Far-half pitch blends from the current pitch.** Original: blends from the pitch on entering the far half (+0x98, 0x76fc41). Ours: from the player's current pitch. Evidence: `crates/thedas-app/src/follow.rs:176`. Size: S. Depends: none. Kind: provisional.
- **CAM-024** **Pull-in facing test uses this frame's facing.** Original: the game camera's own facing (+0x90). Ours: this frame's. Evidence: `crates/thedas-app/src/follow.rs:268`. Size: S. Depends: none. Kind: provisional.
- **CAM-027** **Screen-edge pan.** Original: screen-edge pan. Ours: no code found. Evidence: `crates/thedas-app/src/follow.rs:1`. Size: S. Depends: none. Kind: missing.
- **CAM-028** **Free look.** Original: free look. Ours: no in-game free-look mode found; F toggles a developer free-flying camera. Evidence: `crates/thedas-app/src/main.rs:17`. Size: S. Depends: none. Kind: missing.

### Movies

- **CAM-031** **Movie skip input not traced.** Original: not traced. Ours: any key, click or pad A/B/Start skips. Evidence: `crates/thedas-app/src/intro.rs:7`. Size: S. Depends: none. Kind: provisional.
- **CAM-032** **`DisableIntroMovies` and the FirstRun gate are not read.** Original: options `DisableIntroMovies` and the exe's `FirstRun` gate (per comment). Ours: only `--no-intro`. Evidence: `crates/thedas-app/src/intro.rs:8`. Size: S. Depends: CAM-044. Kind: missing.
- **CAM-033** **Movie path resolution guessed.** Original: not traced. Ours: the resource stack's loose file, else `packages/core/data/movies`. Evidence: `crates/thedas-app/src/intro.rs:88`. Size: S. Depends: none. Kind: provisional.
- **CAM-034** **Movie timing edge cases guessed.** Original: `ActionPlayMovie` update 0x7ce740 (not traced for short movies). Ours: black after a movie ends before its cutscene; frames dropped when behind; the world returns when the last movie ends early. Evidence: `crates/thedas-app/src/intro.rs:473`, `crates/thedas-app/src/intro.rs:593`. Size: S. Depends: none. Kind: provisional.
- **CAM-035** **Subtitles in movies.** Original: subtitles in movies. Ours: no code found. Evidence: `crates/thedas-app/src/intro.rs:1`. Size: M. Depends: none. Kind: missing.
- **CAM-036** **Movie docs contradict each other.** Original: n/a. Ours: `intro.rs` and `main.rs` say movies play silent; `audio.rs` plays boot-movie sound; README "Playing" says boot movies are not shown while README status says they play. Evidence: `crates/thedas-app/src/intro.rs:7`, `crates/thedas-app/src/intro.rs:96`, `crates/thedas-app/src/main.rs:4`, `README.md:158`. Size: S. Depends: none. Kind: bug.

### Startup, configuration, user files

- **CAM-038** **The original's command-line flags.** Original: command-line flags (window, resolution, console). Ours: no code found; only our own clap flags. Evidence: `crates/thedas-app/src/main.rs:93`. Size: S. Depends: none. Kind: missing.
- **CAM-039** **Fullscreen and resolution.** Original: fullscreen and resolution options. Ours: window size only from `--size`; no fullscreen code; the options screen's resolution changes nothing. Evidence: `crates/thedas-app/src/main.rs:108`, `crates/thedas-app/src/main.rs:788`, `crates/thedas-ui/src/options.rs:12`. Size: M. Depends: CAM-044. Kind: missing.
- **CAM-040** **Developer console and debug scripts.** Original: developer console. Ours: no code found in the app (the CLI autoplay calls developer scripts through the sim). Evidence: `crates/thedas-cli/src/main.rs:152`. Size: M. Depends: none. Kind: missing.
- **CAM-041** **Screenshots go to the working directory and overwrite.** Original: screenshots (location not traced). Ours: F12 writes `<level>-capture-N.png` in the current directory; N restarts each run. Evidence: `crates/thedas-app/src/main.rs:1205`. Size: S. Depends: none. Kind: missing.
- **CAM-042** **Crash reporting.** Original: crash reporting. Ours: no code found (no panic hook). Evidence: `crates/thedas-app/src/main.rs:2939`. Size: S. Depends: none. Kind: missing.
- **CAM-043** **Logging to stderr only.** Original: logging. Ours: `env_logger` to stderr, no log file. Evidence: `crates/thedas-app/src/main.rs:2939`. Size: S. Depends: none. Kind: missing.
- **CAM-044** **DragonAge.ini / KeyBindings.ini and options are not read or written.** Original: configuration files (DragonAge.ini, KeyBindings.ini). Ours: no code found; keys are hard-coded to the default bindings; options live only in memory. Evidence: `crates/thedas-app/src/main.rs:1322`, `crates/thedas-ui/src/options.rs:11`. Size: M. Depends: none. Kind: missing.
- **CAM-046** **Saves: one hero, no screenshots, manual kind only.** Original: not traced. Ours: one hero's list, no thumbnails, every save "manual" (no auto/quick kinds on disk). Evidence: `crates/thedas-app/src/saves.rs:7`. Size: M. Depends: FMT-019. Kind: provisional.
- **CAM-047** **Texture quality is fixed to High.** Original: texture detail option. Ours: `global_vfs(TextureQuality::High)` in both entry points; Medium is never chosen. Evidence: `crates/thedas-app/src/main.rs:319`, `crates/thedas-app/src/live.rs:915`. Size: S. Depends: CAM-044. Kind: missing.

### Checked and looks complete

- Follow camera: zoom range (-1..1, 20 m to 3 m), start shot, pitch limits (30..150 degrees from overhead), look height ease, sweep collision, pull-in and ease-out speeds, as traced.
- Q/E camera turn, wheel zoom, right-mouse orbit.
- Cutscene camera from `ActionMakeActiveCamera` cuts, move-spline, rotate and FOV tracks, `ActionJumpToCamera`.
- Cutscene actor placement (`AJSP`, base pose), `AANI` timing, cutscene-built creatures.
- Staged conversation shots from `.stg` and the line's cut, decision camera.
- Default speaker framing (0.9 m, 15 degrees, 6 cm, 40 degrees).
- Boot movie list and order; cutscene `APBM` movies kept on the cutscene clock.
- Headless capture, `--steps` scripted input, WAV recording, `--control` loopback driver.
