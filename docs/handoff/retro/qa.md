# QA retro

## What went well

- One running scorecard (`qa/SCORECARD.md`) with timestamps, binaries, saves and frame paths made every verdict checkable and let a fresh session pick up after restarts, the drive move and usage stops.
- Recipe-driven grading: other streams sent "save + steps + expected log line or frame", and QA returned PASS or a finding with paths. Short loops, few arguments.
- The original as ground truth settled real questions quickly. Howe's guards do cross the camera; there is no Alistair "empty talk" and no Beraht moonwalk; the chargen dimming rules; the wind sway rhythm and amount. Frame bursts and `wincapture` clips beat single screenshots.
- Catching blockers before testers: chargen starting with a naked "undefined" hero, the City Elf hero stuck in a crowd, the OpenGL depth-of-field crash on Windows, the DLC mount turning the hero into Leliana, the Dalish mirror door. All were found by running the full new-game path on every candidate.
- The Windows VM (once the game data was local) gave fast, real answers: GL versus dx12, WARP being faster than Parallels GL, movies without ffmpeg, and the UI script timeout as the cause of the "half bars" frames.

## What could have been better

- I reported things I hadn't verified. The worst case was "using the mirror does nothing", which came from my driver matching a tag by substring and using the mirror DOOR. I corrected it, but it cost triage and sim time. Exact-match tags and a second check before reporting a blocker would have avoided it.
- Sound-on recordings of the original went wrong three times: the wrong save resumed, Esc opened the menu, and the console jump didn't fire. Each attempt spent Sera's speaker time. The rule "prove the whole route silently first" should have come before the first attempt, not after the second.
- Long `sleep` waits and stacked `prlctl` pollers wasted time and sometimes collided. One poller at a time, with detached jobs writing status files, worked; it should have been the default from the start.
- Permission prompts and classifier refusals (VM resume and stop, download-and-run, prlctl exec) stalled work until Sera wrote exact lines in the thread. Asking for the exact wording up front, once per kind of action, would have saved round trips.
- The coordinator relays were a firehose. Many recipes arrived for builds that weren't placed yet. A per-candidate checklist from triage ("this build carries X, Y, Z; recipes here") would have cut the back-and-forth.
- Grading on a loaded Mac produced noise (frame-time outliers, Wine audio stutter, flaky pad timing) that I sometimes reported before ruling load out.

## Lessons

- Always run the real player path, a new game through chargen, on every candidate; most blockers lived there, not in the feature under test.
- A capture of the original is only evidence if it is like-for-like: same spot, same party size, and HUD-free crops (party portraits once skewed a wind measure by 15%).
- Settle "is this a bug or the save?" before reporting. Old saves carried stale tutorial pauses, pending popups, wrong gear and states made by walking past doors.
- Headless and windowed paths differ (reports, intros, update children). Know which one a feature lives in before grading it.
- Small shared tools paid off: `cap.sh`, `capped.sh`, `wincapture`, `run_qs.bat`, `g.py`.

## How to improve in the next harness

- Give QA a candidate manifest per build: commit, sha, the PRs it carries, and a recipe per PR, in one file. Grade against it and write the verdict next to it.
- Make the original controllable without the screen: a dedicated display or VM for it, a reliable console path, and a scripted input tool that can drag and wait for UI states instead of fixed sleeps.
- Expose test hooks everywhere a player clicks: the launcher's `--launcher-steps` and `--control` came late and should be standard. Add accessibility trees so automation and screen readers both work.
- Let the sim play cutscenes on request (a cutscene policy in MCP `load`), so saves can sit just before a scene.
- Keep heavy builds off the machine during perf, audio and VM grading, or grade on a separate machine.
- Pre-approve the routine actions (VM start, stop and restart; scratch-folder installs; downloads from the project's own update server) so testing does not stop on permission prompts.
