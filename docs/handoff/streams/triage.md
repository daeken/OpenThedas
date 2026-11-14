# Triage ("Fix what breaks during play-testing")

Role: the release and integration lane. Triage takes what breaks in play-testing, fixes it or routes it to its owner, decides what is good enough to hand to testers, and builds what testers and QA run. It is the only stream that cuts tester builds and pins `play.command`.

## How the stream was set up

- **Two halves.** A cloud session (this thread) holds the plan, talks to the coordinator, merges PRs through the GitHub API, and cross-builds the Windows packages. A Remote Control session on Sera's Mac (`/Volumes/External2`, triage ports 7930-39, helpers 7910-29) does everything that needs the game data: native builds, the gate, captures, traces of `DAOrigins.exe`, and fixes. The Mac half never merges. The cloud half never runs the game.
- **The candidate gate.** A commit is "good" for testers only after this whole list passes on that exact commit, compared against the last good build:
  - fmt, clippy and debug `cargo test`
  - a chargen click-through (`triage/convcam/bin/convcap.sh dcommon|hnoble`, plus the other origins)
  - the six origin saves load
  - `OpenThedas-testdata/gate/gate.sh` (s01-s16 scenario steps: end areas, plot sets, log diffs)
  - s13 tower seeds 1-3 via `sim-work/s13seed.sh`, judged by step and call counts, never wall time
  - `gate/detcheck.sh` (four saves run twice and through save+reload must match)
  - `thedas corpus <install>`
  - targeted checks for whatever changed

  A delta check, for when a candidate moves a few merges, re-runs only what those merges can touch, and says what it skipped.
- **Pins and builds.**
  - `~/projects/OpenThedas/play.command` follows the newest commit QA has fully graded. Pinning copies a binary only; nobody builds in that checkout, which holds an outside agent's uncommitted edits.
  - QA and sim get `qa/bin/thedas-app-<sha>` and `sim-bin/thedas-mcp-<sha>`.
  - Windows x64 and ARM64 are cross-built in the cloud (`cargo zigbuild`, `x86_64-pc-windows-gnu` and `aarch64-pc-windows-gnullvm`, a `llvm-windres` shim for the icon). They are stripped, zipped, split into 8 MB parts, and put in `windows-build/<sha>/` with hashes in README.txt, because the Mac can't fetch shared files over ~16 MB.
- **Known gaps.** `docs/KNOWN_GAPS.md` plus `docs/gaps/<area>.md` are triage's. They hold one ID per wrong or missing thing (SIM-, SCR-, RND-, UI-, AUD-, CAM-, FMT-, TOOL-). A fixing PR deletes its line and cites the ID. IDs are never renumbered. The README status is refreshed with them.

## Responsibilities

1. Read play-test reports (Sera's notes, F8 reports, QA grades). Reproduce each one, decide whether it is a parity bug, an accepted difference or an improvement, and route it with a repro to the owning stream. Fix it in triage when it sits between streams or blocks testers (for example cutscene walk clips, #690).
2. Cut tester builds. The testers-1 branch was 8137c5d, published Nov 14 after the gate, the QA VM smoke test, and Sera's go. The first update candidate was main cb8205a: gate GREEN, packages built, awaiting QA's VM grade and Sera's go when this was written. Hold a cut against late merges unless a fix is small, high-reach and checked (the dog-naming fix #669 went in; the cutscene-music fix waited for the next update).
3. Merge green PRs for streams that can't merge from the Mac. The CLAUDE.md merge rule applies: merge the latest main, check that exact result, and check again if main moved into the same files.
4. Developer tooling found on the way: frame-rate figures in the window title and the log (#681, #692), the DLC password search (#623), and the gate scripts.
5. Keep the shared Mac usable: disk floor, kill only by pid, hidden windows, no speaker output, Sera's game windows never closed by us.

## In flight or parked when this was written

- Beraht "moonwalking" at the start of the Dwarf Commoner conversation: his line cutscene 39366.cut plays `mh.dg_wandereast` with baked root motion, and ours turns him further than its keys. Waiting on QA's footage of the original's first ~10 s.
- The first update (cb8205a or later): needs QA's VM grade on `windows-build/cb8205a(-arm64)`, then Sera's go. After that, issue reporting publishes it, play.command is pinned, and its notes link `docs/mod-mode-testing.md`.
- Performance: the interface used about 15 ms of a 22 ms frame (UI cut the idle cost in #698; picking with the mouse in the window still costs), and the Ostagar camp issues about 9,700 draws (rendering).
- DLC passwords by search (#623) were never checked on a real non-GOG install. Don't send that check to a Mac session: one ended with a safeguards refusal.
