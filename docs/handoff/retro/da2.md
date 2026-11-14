# Retro: Dragon Age II stream

## What went well

- We went from zero to a headless DA2 prologue in about a day, because the order was right:
  1. corpus-wide readers first (every archive opened, every GFF round-tripped)
  2. then the game switch
  3. then scripts
  4. then the story
  By the time scripts ran, the data layer was rarely the problem.
- A corpus probe over all 383k entries turned "does DA2 work?" into a count of failures by type. It also caught real format differences (NUL-terminated strings, hashed track names) before they turned into confusing runtime bugs.
- Static tracing of the decrypted exe settled things guessing never could: the animation name hash, `IsVectorEmpty`, conversation ownership, plot links. The rule "trace it or read the data, and label anything else an inference" kept wrong guesses out of main.
- Gating every shared change on the DA:O gate, detcheck and corpus meant DA2 work never broke Origins.
- Headless story driving through MCP, with saves at each milestone, made progress repeatable and gave #media real frames.
- Codex, briefed with the same rules, carried the stream a long way (combat, stats, Act 1 to Anders), and it reported evidence rather than claims.

## What could have been better

- I once gated a stale binary: a package-name typo meant the branch never rebuilt, and the gate "passed". The checks should fail when a build fails, not carry on.
- I found the Flemeth overlap late. The script command had been fixed, but the harness's own `move_hero` walked onto the target. I should have looked at which code path the driver used before handing it to sim.
- Old saves kept going stale as state gained new fields (local order, event vectors), so whole replays had to be run again. Cheaper checkpoints, or replaying only the affected state, would have saved time.
- Thread attachment limits and long gate runs (6 to 10 minutes each) slowed the loop. Several merges raced main moving underneath them.
- Usage ran out quickly. Gates and replays are cheap in tokens, but polling, reading long logs and re-deriving context are not.

## Lessons

- Put DA2 behind the data first (tables, template order, link fields) and the exe second. Heuristics went wrong every time they were tested.
- Fail fast in check chains: `set -e`, print the commit hash that was built, and compare it with the one you mean to test.
- When a test harness drives the game, its own helpers count as engine behaviour for the purpose of a bug report.
- Never stack a cleanup fix on an undiagnosed failure (the post-ogre wipe). Codex was right to refuse a speculative balance change.

## How to improve

- Script the merge loop end to end, so a step can't be silently skipped: fetch, merge main, build both binaries, assert their hashes, fmt, clippy, tests, the full gate, detcheck, corpus, then push and open the PR.
- Keep a small set of versioned DA2 checkpoints (chargen done, real prologue, Flemeth, the Gallows, Anders), each with a one-line driver, so new work starts close to the code it touches.
- Keep a living `docs/notes/da2-*` index: one note per traced behaviour, with addresses but no bytes. Codex's notes are the model to follow.
- Hand long, mechanical work (gates, replays, wide traces) to cheaper agents with tight briefs, and keep the expensive model for diagnosis and review.
