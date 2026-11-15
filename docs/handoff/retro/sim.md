# Simulation stream retro

## What went well

- **Trace first.** Reading the original's code before changing ours kept fixes small and right:
  the jump spot, the move-to-object range test, ClearCurrentCommand, item restrictions. It also
  stopped wrong fixes, such as the Dwarf Commoner brazier, where the original places the hero
  in the same spot and the real difference was elsewhere.
- **The gate.** Running the Human Noble chain against main for every behaviour change caught
  regressions (an ambient-talk change once stalled Ostagar) and made "identical" a cheap,
  strong claim for refactors and speedups.
- **Determinism as a rule.** `DetMath`, journaled inputs and detcheck made save/replay and
  bug-report journals work, and made a 20% speedup provably behaviour-free.
- **Writing findings down** with addresses, in doc comments and testdata notes, so later
  questions ("what does the exe do on arrival?") were answered by reading, not re-tracing.
- **Routing.** Saying plainly "this is presentation, not sim" (Beraht's walk-in, sit poses)
  saved other streams time.

## What could have been better

- **Shared worktree races.** The gate and landing scripts check out branches in the one
  worktree. Switching branches while one ran invalidated a premerge once and had to be rerun.
  Several streams also shared a detcheck output folder, which looked like a determinism bug.
- **Long traces under a usage budget.** Some traces (SIM-148 start position, the cutscene
  hiding code) cost a lot and ended inconclusive. Time-boxing them, or asking for a capture of
  the original earlier, would have been cheaper.
- **Speculative first drafts.** Twice a first idea was wrong (snap-on-arrival timing, the
  first in-range window for move-to-object); a unit test caught it, but writing the test first
  would have been faster.
- **Message volume.** Many small status messages to Sera; batching on the checklist is better
  when nothing needs her.

## Lessons

- Never touch a worktree a background script is using; wait for it to finish first.
- Give every harness run its own output directory.
- `gh` needs the real HOME; tests need the scratch HOME.
- When a capture of the original exists, look at it before tracing; it often settles the
  question (two "players" on the benches were ambient commoners).
- A behaviour change with any gate difference needs the difference explained in the PR, with a
  control run if the cause is not obvious (the Codex PRs did this well).

## How to improve

- Give each stream its own worktree and its own gate output, so gates can run in parallel.
- Keep a short "how the original does X" index (addresses per topic) at the top of the notes
  folder; much tracing was re-finding known addresses.
- Make the MCP harness configurable for cutscenes and other policies from the start, so QA can
  make captures and saves the way the game plays.
- Budget traces: an hour, then either a capture request or a provisional implementation with
  the open question written down.
