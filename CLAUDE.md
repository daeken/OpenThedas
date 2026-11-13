# OpenThedas working rules

- Goal: a Rust replacement engine for Dragon Age: Origins that runs the original data. Parity
  with the original is the top priority; modernization must not break it.
- Never commit game data, extracted resources, corpus reports or anything derived byte-for-byte
  from the game. Unit tests use synthetic fixtures.
- References: xoreos (GPLv3) and the SpeedTree 4.1 SDK sources (unlicensed) may be read to learn
  formats. Never copy or link their code into this MIT repository.
- The simulation (scripts, rules, saves) stays deterministic and headless. Rendering, audio and UI
  read simulation state and never write it.
- Format code: bounds-check every read and return errors; never panic on bad input.
- Before pushing: `cargo fmt --all`, `cargo clippy --all-targets`, `cargo test`. When a change
  touches parsing, also run `thedas corpus <install>` against a real install and report the result
  and the host it ran on.
- Before merging: merge the latest `main` into the branch and run `cargo fmt --all`,
  `cargo clippy --all-targets` and `cargo test` on that exact result. Several streams merge in
  parallel, and two PRs that each pass alone can break `main` together. If `main` moved while
  the checks ran, do it again.
  Run the tests in a debug build (plain `cargo test`), not only `--release`: debug stack frames
  and overflow checks catch things a release build hides.
  All worktrees share one `.git`, so another stream's fetch moves `origin/main` under you.
  Never squash or reset onto `origin/main` by name; use the exact commit you merged and tested.
  Stage by path. Before every push and merge, after a fresh fetch and merge,
  `git diff --stat origin/main HEAD` must list only files you meant to change.
- Mark anything taken from xoreos or guesswork, rather than verified against the original
  executable, as provisional in its doc comment.
