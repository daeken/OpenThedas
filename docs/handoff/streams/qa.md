# QA stream: play-test, grade against the original, sign off candidates

## What QA owns

- Grading candidate builds (Mac `thedas-app`, Windows packages) and giving a PASS/blocker verdict per candidate. On PASS, triage pins `play.command` to that build and Sera is asked to publish.
- Reference captures from the original Dragon Age: Origins, run under CrossOver on the Mac. QA is the only stream that runs the original, one session at a time, and with its sound off unless Sera has said yes.
- Turning other streams' recipes ("load this save, run these steps, expect this log line or frame") into graded results, with paths to frames and logs.
- The Windows smoke test in Sera's Parallels VM.
- QA makes no engine fixes. Findings go to the owning stream through the coordinator.

Organisation: QA is one thread in the project. The coordinator relays requests from the triage, sim (combat and simulation), UI (input and interface), rendering, audio, mods and issue-report streams, and passes QA's verdicts back. Triage builds and places candidates and runs the gate. QA grades what triage places and reports to the coordinator.

## Where things are (all under `~/projects` = `/Volumes/External2/projects`, a case-sensitive APFS volume)

- `OpenThedas-testdata/qa/`
  - `SCORECARD.md`: the running log of every grade, finding and pause, newest at the end. Read its end first; it lists the open items.
  - `bin/`: candidate binaries placed by triage (`thedas-app-<commit>`, `thedas-app-testers1-<tree>`).
  - `runNN/`: one folder per grading session, holding frames, logs and sheets.
  - `saves/`: QA's sim saves (Dwarf Commoner chain `dc_*`, City Elf `ce_*`, Dalish `da_*`, Dwarf Noble `dn_*`, and `dc_02_beraht_start.json`).
  - `sim-recipes/`: saves and scripts from sim for specific checks.
  - `for-qa-from-triage/`, `for-qa-from-sim/`: notes handed to QA.
  - `cap.sh NAME [app args]`: headless capture wrapper (capped, `nice`). Use `APP=../bin/thedas-app-X ../cap.sh ...` from a run folder.
  - `wine_seh/`: CrossOver harness for the original (see below).
  - `vm/`: Windows packages (zips, verified by sha256) and the VM job scripts.
- `OpenThedas-testdata/reference/original/`: captures from the original. `notes.txt` lists sessions 1 to 12; newer folders under `triage/` each have a README (`hn_opening_clip`, `chargen_ui`, `dc_intro_beraht`, `wilds_wind_burst`, `wilds_wind_clip`, `first_conversations`, `line_cutscenes`, `vaughan_scene`).
- `OpenThedas-testdata/notes/vm-smoke-test.md`: Windows VM state and how to drive it.
- `OpenThedas-testdata/scripts/`
  - `capped.sh`: memory and time cap for every tool run.
  - `audio/syscapture`: ScreenCaptureKit audio recorder; use the binary by its path and never rebuild it.
  - `audio/fixwav.py`: repairs the header of a syscapture WAV.
  - `video/wincapture`: ScreenCaptureKit window frame grabber: `wincapture OUT SECONDS FPS --pid N`, with `WC_CURSOR=1` to draw the pointer.
- Game data: `DAODecomp/gamedata` (`THEDAS_INSTALL`). The original's install, used by CrossOver: `OpenThedas-wine/game/bin_ship/DAOrigins.exe`.

## How to grade a candidate (Mac)

1. Always use a scratch HOME (`HOME=$PWD/home`), because Sera's real `~/.openthedas` holds a live Discord webhook. Add `THEDAS_MODS=none` for reproducible runs, and never set `THEDAS_REPORT_UPLOAD=1` unless a send is the point of the test.
2. Headless: `APP=../bin/thedas-app-X ../cap.sh NAME --no-audio --save SAVE.json --size 1280x720 --steps "pad a; run 300; ...; run 5000"`. Dismiss tutorial popups with pad A first. QA's driver always picks reply 0.
3. Hidden window with control: `THEDAS_HIDDEN_WINDOW=1 thedas-app --save S --control 127.0.0.1:PORT --no-audio`, then JSON lines (`status`, `input`, `screenshot`, `state`, `report`). QA's ports are 7940 to 7949; check with `lsof` before binding and kill by pid only. Ready drivers: `triage/firsthour3/g.py`, `qa/run*/banner.py`, `live.py`, `ws.py`.
4. Audio is graded with `--audio-wav FILE.wav` (headless, never the speakers). Our engine must never play to the speakers, and nobody touches the Mac's volume.
5. Every candidate gets one new game through the chargen screens in all six origins (`qa/run81/origins.sh`, with the binary name swapped), plus a sweep of saves (`qa/run81/loads.sh`), plus the specific items in the candidate's change list.
6. Sim-side checks drive `sim-bin/thedas-mcp-<commit>` through `OpenThedas-testdata/drive.py` (`load`, `walk`, `fight`, `talk`, `save`, `clear_area`). `qa/sim-recipes/fight_measure.py` measures fights. Note: the MCP always skips cutscenes; it has no Play option.

## Running the original (CrossOver, bottle `DAO_QA`)

- `qa/wine_seh/cx.sh start | stop | k <keys.exe args> | k2 <keys2.exe args> | shot NAME`. `stop` kills by pid only.
- `keys2.exe` (`qa/wine_seh/keys2.c`) does `click:`, `rclick:`, `move:`, `wheel:`, `key:`, `type:`, `hold:` and `waitms:`. It cannot drag.
- Sound: `SoundDisabled=1` in the bottle's `DragonAge.ini` stays on unless Sera has said yes. With sound on, the game plays out of the Mac's speakers; syscapture only records. In Nov 2026 a sound-on run under heavy Mac load stuttered into "five offset copies", so record only when nothing heavy is building, and keep takes short.
- Esc no longer skips the intro movie; New Game needs about 250 s to reach chargen. The chargen popup appears about 25 s after New Game. Harness scripts: `hn_clip.sh` (Human Noble opening), `origin_conv.sh` (first conversations), `alistair_ref.sh`, `proving_*.sh` (reverb takes; still owed).
- Console: `-enabledeveloperconsole`, backtick, `runscript zz_transition AREA WAYPOINT`. The game's own plot script is `runscript zz_getsetplotflag set|get <plot> <flag> <value>`. The console was unreliable under Wine in Nov 2026: the backtick sometimes didn't open it, and the typed letters then acted as hotkeys.
- Clicks only reach the game while its window can be on screen. If Sera is using the screen (full-screen app or presenting), input and captures fail.
- Never touch Sera's saves without reason (they are test-only, and she allows using them). The bottle's characters are Faren, Faren1, Faren2 and Jaden (Jaden is set aside under `OpenThedas-wine/runs/session-cx/set_aside_chars`).

## Windows VM (Parallels "Windows 11", ARM64), paused at Sera's request (it hurts the Mac's performance)

- Full notes: `notes/vm-smoke-test.md`. Drive it with `prlctl exec "Windows 11" [--current-user] ...`. Calls hang or fail ("PrlJob ... Invalid argument") while a game run is busy in the VM, so start runs detached (`qa/vm/run_qs.bat BUILD NAME [dx12]`, which writes `NAME.time`, `.log` and `.png` under `C:\OpenThedas-QA\out`) and poll with short calls, one poller at a time. A trimmed local copy of the game data (`qa/vm/copy_data.bat`, about 14 GB) makes runs fast; over the share a run takes about 40 minutes and the UI script timeout distorts frames.
- The VM's GPU is DirectX 11 class, so wgpu picks OpenGL by default; `WGPU_BACKEND=dx12` uses WARP; there is no Vulkan.
- State: cb8205a x64 passed (GL and dx12, movies without ffmpeg), and Sera clicked through the launcher. Unfinished: ARM64 cb8205a, the chest take in the VM, and the bffd164 launcher hook plus the UI Automation/AccessKit check. `C:\OpenThedas-QA` is still in the VM.
- Parallels resume, restart and stop each needed Sera's explicit line in the QA thread.

## Current state at handoff (2026-11-14)

- Published testers' build: testers-1 8137c5d (QA PASS). The first update candidate cb8205a passed on the Mac and on Windows x64; the items above are open.
- Recent PASSes on main: #712 unusable items greyed (078b146), weapon sets (#666, #670), #620 ambient lines, cutscene lights, death screen, Ostagar Alistair and Flemeth, chests and stores, door occlusion (footstep flicker reduced but not gone, reported to audio).
- Open references in the original: the inventory with an unusable item; a City Elf or Dwarf Noble opening without the hero; Alistair at King's Camp and the Rica corridor (parked: console unreliable); reverb takes in the Proving (route rehearsed, two sound-on attempts failed).
- Open for sim: a cutscene-policy option in the MCP, so saves can keep cutscenes (needed for a Beraht walk-in save).

## What a newcomer must not break

- No speaker output from our engine; nobody changes the Mac's volume; the original's sound stays off unless Sera says yes.
- Scratch HOME for every run (the webhook). Ports 7940 to 7949 for QA. Kill by pid only, never by pattern (a pattern kill once hit other streams' shells).
- Keep work under `~/projects`; read `SCORECARD.md`'s end before acting; never fix engine code from QA.
- Verdicts name the exact binary and sha, the saves and steps, and the frames; say plainly what was not covered.
