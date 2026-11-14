# Retro: Codex orchestration (2026-11-14)

## What went well

- **Throughput at low Claude cost.** About 110 PRs merged in roughly 14 hours, with Codex doing the heavy work. The Claude side
  mostly read diffs and wrote briefs.
- **Strict isolation held.** Separate clones, worktrees, target dirs and ports, plus the 2-slot build limiter, kept everyone off the
  shared Mac's toes. No other checkout was touched.
- **Gates caught real things.** The DA:O s01-s16 gate, detcheck, corpus runs, origin saves and the chargen click-through ran on
  every sim change, and they surfaced intended differences with explanations.
- **Owners stayed in the loop.** Announcing changes to the owning streams worked. Their conditions improved PRs, for example:
  - #838: proof that the immortal damage path is unaffected.
  - #851: keyed to a single movie, with a negative test.
  - #844 and #853: door and chest bash-to-0 tests.
  - #834: the Mouse.MIDDLE init test.
- **Traces before code.** Each SCR fix cites its exe handler and is gated to Origins, so DA2 is untouched.
- **Media and deltas.** Media became a steady stream, and the pixel-delta panels made subtle renderer changes legible.

## What could have been better

- **Discord slip.** An early brief pointed Codex at `media-queue/` for capture storage, and the watcher posted 2-3 test images. Fix: a
  hard rule never to write to `media-queue`, with staging plus orchestrator review instead.
- **Stacked-PR base mistake.** #772 was based on #759's branch, so merging it landed in that branch rather than main. Main briefly had
  miss/block without the stats it depends on, until #792 restored them. Later #856 was stacked on #855 the same way.
- **Conflict churn.** Many PRs touched `deferred.rs` and `engine.rs`, so every merge left the rest conflicting, and it took many
  re-merge rounds. Batching (#817) and capping open PRs per job (one for deferred, two for DA2) fixed it.
- **Back-to-back merges without a combined check.** Several PRs landed together, then needed a "verify latest main" pass afterwards.
- **Auto-mode friction.** The harness blocked merges of unread diffs, loops, and actions approved only through a relayed message.
  Each needed Sera to approve in-thread or the orchestrator to re-read the diff. These blocks were correct, but they cost round trips.
- **Briefs changed while Codex was mid-run.** A job ran on its old brief, so conditions slipped by a round (#827, #838, #834).
- **Unexplained closure.** #809 was closed without being merged and with no record of why. It was later reopened and merged.
- **Shell slips.** An unquoted heredoc ate backticks, and zsh didn't split a word list. Both were caught, but they cost reruns.

## Lessons

1. Base every PR on main. Check `baseRefName` before merging anything stacked.
2. Cap open PRs per job, and batch PRs that touch the same hot file.
3. After any back-to-back merges, run one combined check on main.
4. Put every owner condition into the brief as a checklist, and verify it in the PR body before merging.
5. Never point an agent at a folder another automation watches.
6. Write briefs with quoted heredocs or the editor, never with unquoted shell text.

## How to improve in the next harness

- Give the orchestrator a merge-queue primitive: rebase, test and merge serially, automatically.
- Let briefs reach a running agent, for example with a resumable session or `codex exec resume`, instead of waiting for the run to end.
- Turn the announce and approve loop into structured checks, such as a PR label plus the owner's approval, rather than relayed chat.
- Keep the RULES and brief pattern. It scaled well to six concurrent jobs.
