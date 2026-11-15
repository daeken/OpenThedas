# Audio stream retro

## What went well

- **Trace first, then build.** Nearly every behaviour cites the exe address it came from, and the code marks what is provisional. That made reviews quick, and it let QA's reports be answered from the trace. Some QA reports were the game's own data, such as Orzammar ambience in the Dalish ruins and the `bdncs_intro_switch` typo. Others were the exe's own rule, such as footsteps flipping at a doorway.
- **Offline capture.** `--audio-wav` let us check real play without speakers. Recordings of the original under Wine gave one hard number: the main-menu music matches to within 0.1 dB.
- **A landing script.** `land.sh` ran the merge-main-then-check rule every time, retried when main moved, and honoured holds through a `HOLD` file. No audio PR broke main.
- **Notes outside the repo.** `notes/handoff-audio.md` meant a context reset or a storage move cost minutes, not hours.
- **Checking the data before building.** Before building the music cue's `combat` option, I listed every cue in the shipped banks. None uses it, so it was documented instead of built.

## What could have been better

- **Too many small PRs early.** The coordinator asked for fewer, larger audio PRs. Batching a trace note with the next real change worked better.
- **Captures that disturbed the owner.** One debug-build capture opened a slow, visible window on Sera's screen. Runs should have used a hidden window and a release build from the start.
- **Runs that never ended.** Without `--capture`, a headless run doesn't exit. Several runs hung until I killed them, which cost a few tool round-trips.
- **Waiting on permissions.** The auto-mode classifier blocked `gh pr merge` and `gh pr comment` until Sera wrote standing permission in the thread. Reviews of other streams' PRs had to go through the coordinator as text.
- **No end-to-end test saves for some features.** No test save reaches a cutscene that cues music, so that feature has only unit tests and a format check against the real bank.
- **The reverb is still ungraded.** It waited a long time on a usable QA recording.

## Lessons learned

- Write down the exact headless recipe (hidden window, release build, `--capture`, install path, scratch `HOME`) once and reuse it.
- Look at what the shipped data uses before building a traced feature. Many traced paths are dead in DA:O's data.
- A context reset is cheap when every trace goes into a notes file in the turn it is found.
- Pin the merge rule in a script, not in memory. Several streams merging at once is where breakage comes from.
- Mark guesses as provisional where they live (doc comments, gap doc). The next reader can then tell traced from guessed without re-reading the exe.

## How to improve

- **Give each stream a save library:** saves placed just before key moments (a music-cue cutscene, a reverb room, a doorway with a door). Features could then be checked end to end, not just by unit tests.
- **Grant merge and comment permission per stream up front** in the new harness, with an explicit hold mechanism (a file or flag that the landing script reads).
- **Keep one `land` script for the whole project,** not one per stream, with the same retry and hold behaviour.
- **Make capture a single command** (`thedas-app --audio-check-run <save>` or a script). It should always run hidden and in release, and always exit.
- **Record more of the original:** reverb rooms, occlusion at a door, combat music changes. Comparing against a recording beats arguing from the trace.
- **Start the next audio work** from `docs/gaps/audio.md` and the private `notes/handoff-audio.md`. Reverb grading and spell and projectile sounds are the biggest gaps players will notice.
