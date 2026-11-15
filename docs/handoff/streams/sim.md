# Simulation stream ("sim")

The simulation stream owned the headless game: scripts' effect on the world, combat, movement,
commands, conversations, cutscene logic, saves, and the parity traces behind them. Work was done
in one worktree (`OpenThedas-wt/sim`), from one Claude thread, in coordination with a
coordinator Claude and the other streams (triage, QA, UI, rendering, audio, mods, DA2,
issue-report, and later a Codex stream).

## What it owned

- `crates/thedas-sim`: everything except formats and presentation. The modules worked on most:
  `sim.rs` (ticks, commands, movement, arrival, save hooks), `engine.rs` (engine functions by
  name), `combat.rs` (perception, game mode, attacks, abilities), `dialog.rs` (conversations,
  ambient talk), `cutactors.rs` / `cuttracks.rs` / `cutscene.rs` (cutscene binding and
  placement), `nav.rs` (pathing grid, safe-location search, doors), `inventory.rs`,
  `training.rs`, `replay.rs` (journal saves), `fmath.rs` (deterministic maths),
  `commands.rs` (the `Sim::apply` command API).
- The sim's side of `crates/thedas-mcp` (the headless MCP the gates and QA drive).
- Gap docs for the area: `docs/gaps/simulation.md` (`SIM-`) and `docs/gaps/scripting.md`
  (`SCR-`), and their rows in `docs/KNOWN_GAPS.md`.
- Answering other streams' "is this sim or presentation?" questions, and reviewing other
  streams' PRs that touch sim files (the Codex SCR- PRs at the end).

## How work was decided

Parity with DAOrigins.exe comes first. The loop was:

1. A report (Sera, QA, triage, another stream) or a gap ID.
2. Trace the original in the disassembly (`OpenThedas-testdata/disasm/daorigins.asm`, the
   handler table `notes/exe-script-handlers.tsv`), and read the shipped scripts with
   `thedas script disasm <install> <script>` and tables with `thedas dump`.
3. Write down what was found, with addresses, in the code's doc comments and, when long, in
   `OpenThedas-testdata/notes/<topic>-trace.md`. Anything not traced is marked **Provisional**.
4. Implement, unit test with synthetic fixtures, gate, merge.

When the trace showed the original does the same thing we do (e.g. the hero placed in the Dwarf
Commoner's brazier), the answer was "no sim change" plus routing to the right stream.

## Build, test, verify

All commands from the worktree. Run tests with a scratch HOME: Sera's real `~/.openthedas`
holds a live webhook.

```
REAL=$HOME; HOME=/tmp/claude-501/scratch-home CARGO_HOME=$REAL/.cargo RUSTUP_HOME=$REAL/.rustup cargo test -p thedas-sim
cargo fmt --all && cargo clippy --all-targets -- -D warnings
```

(CARGO_HOME and RUSTUP_HOME must point at the real home or rustup breaks; `gh` needs the
real HOME.)

- **Gate** (behaviour changes): `OpenThedas-testdata/sim-work/gate_round2.sh <branch-suffix>`
  with `MAIN=<merge-base sha>`. It builds release thedas-mcp for main and for
  `m1-<suffix>`, runs the Human Noble chain s01-s12 through `gate/gate.sh` (log diff, plot sets,
  areas, refused saves) and the long s13 trace. Identical means no behaviour change; any
  difference must be explained in the PR. It does `git checkout` in the worktree (commit first,
  and do not touch the worktree while it runs).
- **Determinism**: `OpenThedas-testdata/gate/detcheck.sh <mcp binary>`: four saves, run twice and
  through save/reload, must be identical. Each run now writes to its own temp folder.
- **Landing**: `/tmp/claude-501/land_existing.sh <branch> <pr> <bodyfile>` merges main, runs fmt,
  clippy and debug tests, pushes, and merges the PR only if main did not move (retries). Also
  uses the worktree.
- **Timing** a change: release thedas-mcp on `07_ostagar_arrival.json`, 600 ticks (see the
  #719 PR for the script shape).
- **Ports**: the sim stream used 7950-7959 for the live app's `--control` port; `lsof` first,
  kill by pid only.
- Test saves: `OpenThedas-testdata/*.json` (the chain), `qa/sim-recipes/` (QA scenarios),
  `qa/saves/`. Driver for scripted MCP runs: `OpenThedas-testdata/drive.py`.

## Key design decisions (do not undo without a trace)

- The simulation is deterministic and headless; presentation reads it and never writes it. UI
  queries that need scripts (e.g. `CreatureCanUseItem`) read answers the sim worked out on a
  journaled input (`GuiOpened`), never run scripts themselves.
- All maths that feeds the world goes through `fmath::DetMath`. No wall clock, no hash-map
  iteration order in anything that changes state.
- Inputs go through `Sim::apply` / `Sim::input` (`CommandSource`), so journal saves can replay
  them (`replay.rs`, app `journal.rs`).
- Scripts own the game mode; the engine only applies `SetGameMode` (combat.rs module doc).
- Movement: timed paths on the pathing grid; creature capsules block (slide, no push). Walks to
  a creature end in range or against its body (#727); jumps land beside, through the
  safe-location search (#713, `nav.rs` `safe_search`).
- Cutscenes: tracks move actors as drawn, not the objects; objects are set at end poses when
  the cut ends (`cutactors.rs` module doc). `Sim::cutscene_hidden_party` lists uncast party
  members for rendering to hide (provisional).

## State at handoff (2026-11-14)

All sim branches merged; nothing in flight. Last merges: #694 GetSafeLocation, #697 item
restrictions, #713 jumps, #719 perception speedup, #727 move-to-object, #728 cutscene hidden
party, #733 ClearCurrentCommand / SetEffectAnimation / SetEffectFlags, #737 MCP `cutscenes`
argument. Codex SCR- PRs reviewed and OK'd (#760-#774, #811, #819, #827, #838, #844, #863).

Open / parked:
- Brazier hiding rule is read from one capture; the original's hiding code is not traced.
- `CommandJumpToLocation` still lands exactly (exe 0x7a4100 uses the search with the check on).
- `CommandMoveToObject` keeps only fMinRange (fMaxRange not stored on `Command`).
- Party formation special case in jumps (0x6aa440) not modelled.
- Item restrictions: attribute requirements and allowed-user lists not checked (UI-029).
- Arrival placement (0x6c73c0) not applied to our arrivals (they go exactly on the waypoint).
- See `docs/gaps/simulation.md` and `docs/gaps/scripting.md` for the full list.

## What a newcomer must not break

- Determinism (gate + detcheck identical for refactors).
- The gate chain reaching its end areas (s13 must reach Flemeth's hut).
- Save compatibility: new saved fields take `#[serde(default)]`.
- The "presentation never writes the sim" rule.
- Never commit game data or bytes from the exe; testdata lives in OpenThedas-testdata.
