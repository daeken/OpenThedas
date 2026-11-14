# OpenThedas retrospective

Written 2026-11-14 from every workstream's own retro in [`retro/`](retro/). About fifteen agent streams (Claude sessions plus a Codex lane) worked in parallel on one shared Mac for several days and merged around 850 PRs. Each section below draws out the themes several streams named, and points to the stream files for detail.

## What went well

- **Trace the original first.** Almost every stream named this. Reading `DAOrigins.exe`, the movies' bytecode or the shipped data before changing behaviour gave fixes that matched and stayed fixed, and guessed fixes came back. Addresses in doc comments and notes, plus a "provisional" mark on guesses, made claims reviewable and saved re-tracing. ([sim](retro/sim.md), [input-ui](retro/input-ui.md), [mods](retro/mods.md), [rendering](retro/rendering.md), [audio](retro/audio.md), [da2](retro/da2.md))
- **Checking against the data, not just the code.** Some examples:
  - Listing what the shipped data actually uses avoided building dead paths (audio's `combat` cue option).
  - The Scaleform inventory of all 68 movies cut "a Flash player" down to "this game's movies".
  - DA2 opened every archive and round-tripped every GFF before writing game logic.
- **A gate with a verdict.** The scenario gate, detcheck, corpus runs, origin saves and the chargen click-through turned "is it worse?" into numbers: end areas, plot sets, log diffs and identical replays. Determinism (`DetMath`, journaled inputs) made "behaviour-free" provable for refactors and speedups. ([triage](retro/triage.md), [sim](retro/sim.md), [codex](retro/codex.md))
- **Ownership plus announce-before-merge.** Changes to another stream's code went to its owner first, and the owners' conditions improved PRs. Nine to fifteen streams merged into one `main` for days without breaking parity. ([coordinator](retro/coordinator.md), [codex](retro/codex.md))
- **A list of record in the repo.** `docs/KNOWN_GAPS.md` and `docs/gaps/*`, with stable IDs and "delete your line in the PR that fixes it", kept scope honest.
- **Seams instead of rewrites.** These made big swaps into deletions rather than migrations:
  - The deferred renderer and the Scaleform player each started behind a runtime switch, with the faithful path or Ruffle as the default.
  - The SpeedTree clean room ran through a numbered spec, with questions answered as doc amendments.
- **Pictures as evidence.** Headless captures, side-by-sides with the original (QA's CrossOver captures), pixel-delta panels and the #media feed. They caught what tests couldn't, and kept the maintainer in the loop.
- **Recipes and scorecards.** Every fix came with "save, steps, what to look for". QA graded from those without arguing, and the same recipes became regression checks. ([qa](retro/qa.md), [input-ui](retro/input-ui.md))
- **Codex as a reviewed worker.** Codex merged about 110 PRs in roughly 14 hours under strict rules (isolation, gates, owner sign-off, no merging by itself), at low Claude cost. ([codex](retro/codex.md))

## What could have been better

- **Usage.** About a week of usage went in a day: many parallel sessions on the expensive model, re-reading context, polling, long logs, and re-running checks every time `main` moved. ([coordinator](retro/coordinator.md), [rendering](retro/rendering.md), [da2](retro/da2.md))
- **Merge races.** `main` moved during most 10-30 minute check runs, so PRs needed three or four rounds. Hot files (`engine.rs`, `deferred.rs`, `thedas-ui/src/lib.rs`) churned through conflicts. Mistakes followed:
  - stacked PRs merged into the wrong base (#772);
  - merges committed conflict markers, or ran checks on a conflicted tree;
  - a stale binary was gated after a failed build.

  ([codex](retro/codex.md), [scaleform](retro/scaleform.md), [issue-reports](retro/issue-reports.md), [da2](retro/da2.md))
- **One laptop as the build farm.**
  - Load averages of 20-110 made timings meaningless and builds slow (10-20 min release builds).
  - Disk fell below the floor.
  - Every remote session stopped when the Mac slept or lost Wi-Fi.
  - A debug-build capture opened a slow window on the maintainer's screen.
- **Permission friction.** The Mac's auto-mode check refused merges, comments, VM control, `kill -STOP` and actions approved only through a relayed message, until the maintainer wrote exact approvals in the thread. Each refusal was right by its own rules, and each cost round trips. ([audio](retro/audio.md), [qa](retro/qa.md), [codex](retro/codex.md))
- **Hub-and-spoke messaging.** Every cross-stream message went through the coordinator, which added latency to one-line answers and turned into a firehose for QA (recipes for builds not yet placed). ([issue-reports](retro/issue-reports.md), [qa](retro/qa.md), [speedtree](retro/speedtree.md))
- **Claims ahead of evidence:**
  - a misread driver reported a broken mirror;
  - a build was described with a fix it didn't carry;
  - timing numbers were quoted before load was ruled out;
  - a plan was promised and then built differently.
- **Reproduction setup cost more than fixes.** Saves made by tools that skip cutscenes or carry tutorial pauses couldn't reach the bug, and saves went stale as the state gained fields. ([rendering](retro/rendering.md), [qa](retro/qa.md), [da2](retro/da2.md))
- **Real-world checks deferred.** These never happened:
  - a real Steam or GOG sign-in;
  - a real .dazip;
  - the DLC key search on a non-GOG install;
  - macOS and Windows accessibility checks;
  - the reverb grading.

## Lessons

1. **Put rules in scripts and the repo, not in memory or prose.** The merge loop, kill-by-pid, hidden windows, scratch HOME and holds all worked best once they were a script (`land.sh`, the gate, `capped.sh`), or once they were impossible to break (reports unable to send from automated runs).
2. **Fail fast in check chains.** Use `set -e`, print the commit you built, compare it with the one you meant to test, and grep for conflict markers before committing a merge.
3. **Base every PR on `main`, cap open PRs per hot file, and run one combined check after back-to-back merges.**
4. **Look at the original before tracing.** A like-for-like capture (same spot, same party, HUD-free) often settles a question in minutes that a trace takes hours on.
5. **Know which path you're testing.** Headless and windowed differ, a harness's own helpers count as engine behaviour, and an old save may be the bug.
6. **Measure before optimizing, and on a quiet machine.** Profile shares beat wall time under load. Ruffle's picking and creature posing were found this way.
7. **Say what's unverified, per platform, in the PR's first lines.**
8. **Give cheap, well-specified work to cheaper agents, and keep the expensive model for diagnosis and review.** Briefs need owner conditions as a checklist.
9. **Write traces down in the turn they're found.** A context reset then costs minutes, not hours.

## How to improve in the next harness

- **An always-on build and test host with a GPU and the game data mounted.** Run the gate on every PR there, post the verdict to the PR, and keep heavy jobs off it during performance, audio and VM grading. Make frame-rate and per-pass timings part of the gate.
- **A merge queue** that merges `main`, builds, runs fmt, clippy, tests and the gate, then merges serially, with an explicit hold flag. One `land` script for the whole project.
- **Permissions set once, per action class.** Per stream: merge own green PRs, comment, VM start and stop, scratch installs, downloads from the project's own server. Also a defined way for the maintainer to approve a class of actions ahead of time.
- **Direct stream-to-stream messaging** on a shared announce channel, with structured approvals (a PR label plus the owner's OK) instead of relayed chat.
- **A candidate manifest per build:** commit, hashes, the PRs it carries, and a recipe per PR. QA grades against it and writes the verdict beside it. Windows packages and the update manifest are built in CI from a tag.
- **A save library and cutscene policy.** Versioned checkpoints just before key moments, for both games, each with a one-line driver. An MCP option to keep cutscenes, so walk-ins and intros can be reproduced.
- **Golden-image checks** for UI and renderer captures, with tolerances, instead of eyeballing.
- **An index of "how the original does X"** (addresses by topic) at the top of the notes, so tracing isn't repeated.
- **Real-world validation slots:** a real Steam and GOG account run, a real .dazip, Mac and Windows accessibility checks, and recordings of the original for reverb, occlusion and combat music.
- **Budget-aware planning.** Fewer, longer-lived sessions with tight briefs, no polling, and a usage estimate per task.
