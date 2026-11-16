# Triage tools

Scripts for the triage stream's repeated work, so an agent decides and the script executes.
Python 3 standard library or bash only; no game data, no secrets. Game runs are headless or in a
hidden window, under a scratch HOME, with `THEDAS_MODS=none` and no audio: nothing is sent and
the player's own data is untouched.

| Script | What it does |
|---|---|
| `pin.py <sha> [--dry-run] [--force] [--no-facing]` | Checks a commit (workspace clippy `-D warnings`, debug `cargo test --workspace`, release build) and runs the spot-checks below on that exact binary, printing one pass/fail table and its sha256. Only if every check passed (or `--force`), and not with `--dry-run`, copies the binary to `$PLAY_CHECKOUT/target/release/thedas-app`, keeping the one it replaces as `thedas-app.<sha8>.bak`, and runs `play.command --help` under a scratch HOME. Never edits play.command. |
| `bisect_merges.py <good> <bad> <check> [--min N]` | First-parent bisect: builds each tested merge once (cached in `$THEDAS_TESTDATA/bin/<package>-<sha8>`) and runs `<check> <binary> <out-dir>`. A numeric last line is good at or above `--min` (default 30); otherwise the exit code decides. |
| `check_bunk_psnr.py <binary> <out-dir>` | A `bisect_merges.py` check: the City Elf opening's bunk-region PSNR against `$BUNK_REF`. |
| `replay_report.py <issue> [binary] [--steps S]` | Finds a player report's local folder from the issue's Discord message id, copies it to `$THEDAS_TESTDATA/triage-reports/<N>/`, replays its "at report" and "before" saves with the report's camera (captures and logs, gait trace on) and writes a `verdict.md` skeleton. No judgement. |
| `merge-pr.sh <pr> [--dry-run]` | Cloud-side merge of one PR (merge main in, fmt, clippy and tests for the touched crates, merge). |

`checks.py` holds the spot-checks `pin.py` runs: slums click (moves, does not pick the exit),
Oskias's body bag (opens the inventory), City Elf bunk (PSNR against `$BUNK_REF`), footlocker
(inventory screen), F8 note (apostrophes and length kept), facing probe (a logging build: at most
30° of turn a frame), detcheck (two saves, run twice and save and reload).

## Environment

| Variable | Default | Used for |
|---|---|---|
| `THEDAS_INSTALL` | (required) | the Dragon Age: Origins install the game runs on |
| `THEDAS_TESTDATA` | `~/projects/OpenThedas-testdata` | saves, references, binary cache, outputs |
| `TRIAGE_WORKTREE` | this repository | the worktree builds check out commits in (detached) |
| `PLAY_CHECKOUT` | `~/projects/OpenThedas` | where play.command runs `target/release/thedas-app` |
| `TRIAGE_SCRATCH` | `<tmp>/thedas-triage` | scratch HOMEs |
| `BUNK_REF` | `$THEDAS_TESTDATA/triage-celf/ref/bunk.png` | the bunk reference capture |
| `REPORTS_DIR` | `~/.openthedas/reports` | local report folders |
| `FFMPEG`, `CARGO`, `REPO` | from PATH / `daeken/OpenThedas` | tools |

Builds use the worktree's own `target/`; use a worktree no other stream builds in.
