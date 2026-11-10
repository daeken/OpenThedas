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
- Mark anything taken from xoreos or guesswork, rather than verified against the original
  executable, as provisional in its doc comment.
