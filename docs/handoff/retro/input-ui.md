# Input / UI stream: retro

## What went well

- **Tracing the exe first** (and the movie bytecode) turned UI "bugs" into precise answers:
  argument order of a query, which message takes a chest item, how a slider's labels are
  written, how the right button reaches a clip. Every time a fix was guessed it came back;
  every time it was traced it stayed fixed. Notes in `OpenThedas-testdata/notes/` carried
  the knowledge between sessions.
- **Recipes next to every fix** (`ui/for-qa/recipes.md`): save, steps, what to look for, the
  broken behaviour before. QA could grade without asking, and the same recipes became
  regression checks.
- **A gap list of record in the repo** (`docs/gaps/ui.md`, ids, "delete your line in the PR
  that fixes it") kept scope honest and made "what is left" answerable.
- **A strict merge loop** (merge main by SHA, fmt, clippy `-D warnings`, tests, merge only if
  main did not move) kept main green across nine streams for days.
- **Measuring before optimising**: a per-movie tick profile showed Ruffle's mouse picking was
  most of the interface's time (not the scripts or rendering), which pointed straight at the
  fix and later at the argument for our own player.
- **Looking at the pixels**: nearly every UI change was checked in a real capture, including
  comparisons with QA's captures of the original. That caught several things tests could not
  (labels missing, glow timing, popups behind other screens).
- **Cross-stream review requests** (Codex stream asking before merging into thedas-ui) worked
  well: a concrete diff, a concrete question, a concrete test on a real build.

## What could have been better

- **Collisions on main.** With ten streams merging, a 20 minute check cycle often lost the
  race, and some PRs needed three or four rounds. Earlier requests for a merge hold, or a
  merge queue that tests the combination, would have saved hours of CPU.
- **Machine load.** Load averages of 30-60 made timings meaningless and builds slow; I
  repeatedly had to throw away a measurement. A budget for heavy jobs (and one place to see
  who is running what) was missing.
- **Parent-pid kill incident** and a few port/pattern-kill near misses: the rules existed
  only as habit until they bit. Rules like "kill by pid only after checking cwd" should have
  been a script, not prose.
- **Some work came from stale context**: reports about things already fixed on main, or
  repros that differed from the reporter's setup (the dog popup). Asking for the exact command,
  commit and log first would have saved cycles.
- **Release builds for UI checks** took 3-8 minutes each; I often rebuilt for a one-line change.
  A fast headless "UI preview" binary or a hot-reloadable movie harness would pay off.
- **No automated UI screenshot diffs.** The comparisons I did by eye (and QA did by hand)
  should be golden images with tolerances; Codex's changed-pixel reports were the right idea.
- **The exe disassembly tooling** was ad hoc (a Python xref script I rewrote more than once).
  A small shared tool (find string -> code xrefs -> disassembly window) would have been used
  daily.

## Lessons

1. If a movie shows nothing, check what it asked the host (`RUST_LOG=thedas_ui=debug`,
   `THEDAS_TOKEN_LOG`) before touching rendering. Most "UI bugs" were host answers.
2. List-valued tokens need a value on the parent token; zero-valued members must count;
   argument order in `fscommand`s is whatever the movie's function builds, not what the name
   suggests.
3. Left-button-only native events plus a movie fallback listener implement right clicks; do
   not define `Mouse.MIDDLE` before init (it disables that listener).
4. Keep presentation state out of the simulation and out of save files unless the original
   saves it.
5. Defaults for automated runs and for a player's window can differ (anti-aliasing is off in
   control runs so captures and timings stay comparable). Make that explicit in code.
6. Write the repro the way a player does it (real clicks, Next/Back, reopen) before saying a
   report is not reproducible.
7. Verify a merge by what is on main afterwards (`cargo check --all-targets`) when main moved
   during your checks.

## How to improve

- A merge queue (or "merge train") with tests on the combined head; a standing rule that
  streams touching `crates/thedas-ui` or `crates/thedas-app/src/main.rs` announce it.
- A load-aware job runner (a lockfile or semaphore for release builds and corpus runs) and a
  dashboard of who is using the machine.
- Golden screenshots for the interface (per movie, per state) with a changed-pixel budget,
  run in CI-like fashion after UI changes.
- A shared exe-trace helper and a notes index (`OpenThedas-testdata/notes/README`) so a trace
  is findable by address and by topic.
- Keep the per-stream recipes and gap lists; add one "first hour" smoke test script that
  walks the real UI from the start menu through chargen, a store, a chest, the journal and a
  fight, and runs on every merge.
- Document permission and sandbox expectations up front (what may be killed, which ports, what
  may touch the original game) in a file the harness enforces, not in prose.
