# SpeedTree clean-room implementer stream

The stream that wrote `thedas_formats::speedtree_gen`: growing the game's SpeedTree 4 trees,
bushes and grass from their `.spt` files the way Dragon Age: Origins does at load time, from a
written spec only. Written 2026-11-14 by the Claude session that ran it (worktree
`~/projects/OpenThedas-wt/speedtree`).

## The clean-room arrangement (read this first)

Sera set this up on 2026-11-12 so that no SpeedTree SDK code or structure reaches the repo
except through prose. There are two roles, and they must be different sessions (or people):

- **Spec writer** (was the rendering stream). May read the SpeedTree reference sources and the
  original executable's tree code. Writes only prose into `docs/speedtree/` (README, format,
  generation), with no code, type, function or file names from those sources. Never writes
  generator code.
- **Implementer** (this stream). Writes `speedtree_gen` from `docs/speedtree/` alone.

Rules the implementer kept, and that any successor in this role must keep:

- Never open, fetch, clone or search the SpeedTree SDK (anywhere: GitHub or local copies),
  `OpenThedas-testdata/notes/speedtree.md`, `OpenThedas-testdata/notes/speedtree-exe-trace.md`,
  `OpenThedas-testdata/spt-scratch/`, or the original exe's tree code or any disassembly of it
  (DAODecomp, `OpenThedas-testdata/disasm`). Do not hand any of that to a subagent either.
- Do not trace the game's SpeedTree code yourself. Items the spec marked "(exe)" were traced
  by the spec side and arrived as spec amendments.
- On a drive with many other projects, don't run broad searches that could surface SDK or
  trace material. Stay in `OpenThedas`, `OpenThedas-wt/speedtree*` and the
  `OpenThedas-testdata/notes/speedtree-*` files written for this side (corpus, shape-check,
  spec-questions).
- When the spec is unclear or looks wrong, ask the spec side by section number (through the
  coordinator) and wait for a doc amendment. Don't guess from outside sources. Defaults the
  code had to pick where the doc is silent are marked as choices in doc comments.
- Never copy SpeedTree SDK code into the repo.

A new harness that cannot guarantee this separation (one agent with access to everything)
should treat `speedtree_gen` as finished and change it only from spec amendments written by
someone else.

## What it owns

- `crates/thedas-formats/src/speedtree_gen/` (about 5,800 lines with tests):
  - `mod.rs`: the single random stream (`Random`: Lehmer generator behind a 128-entry shuffle
    table, §1.1), curve parameters (`Curve`: Bézier build, 500-point table, three evaluation
    flavours), `generate` (all stages in the §5.1 order), `check` (used by `thedas corpus`).
  - `frame.rs`: rotation frames and the `sin`/`cos` wrappers (see gotchas).
  - `skeleton.rs`: stage 2, tree set-up and the branch skeleton (`grow`, `Skeleton`, `Branch`,
    `Bud`, `Flare`, `Guide`).
  - `geometry.rs`: stage 3, branch tubes, texture layers, normals, strips, branch LOD.
  - `fronds.rs`: stage 4, blade and extrusion fronds, frond LOD.
  - `leaves.rs`: stage 5, buds, leaves, leaf LOD.
  - `limits.rs`: `validate` and growth budgets; `GenerateError`. Files beyond the limits are
    rejected (none of the 552 game files comes close).
  - `mesh.rs`: stage 7, the renderer-facing API. `TreeMesh::new` / `try_new` (new gives an
    empty tree on error): `Part`s with `MeshVertex` lists and triangle lists per LOD for
    branches and fronds, `wind` parallel to vertices (§7.7), leaf cards (§7.2), mesh leaves
    (`leaf_meshes`, `mesh_leaves`, `leaf_mesh_templates`, §7.3), billboard (§7.5), bounds
    (§5.7 F), self-shadow coordinates (§7.9), `lod_range`, `lod_value`, `lod_alphas` (§7.8),
    `discrete_level`, `draws_billboard`, `fingerprint`.
  - `wind.rs`: wind animation (§7.11): `Wind::dao(strength, direction)`, `advance`,
    `matrices`, `leaf_angles`; `Settings::dao` holds the game's oscillator values.
- `crates/thedas-cli/src/tree.rs`: `thedas tree <file.spt | archive.rim:entry.spt>
  [--lod N] [--obj out.obj] [--png out.png --size 300] [--scale 0.31]`: statistics, OBJ
  export, flat-shaded side and top views.
- The `speedtree_gen::check` hook in `thedas corpus` (`crates/thedas-cli/src/corpus.rs`).

Not owned here: the `.spt` parser (`crates/thedas-formats/src/speedtree.rs`, shared), the spec
(`docs/speedtree/`, spec side), and all drawing (`thedas-render`: `scene.rs`, `d3d_gpu.rs`,
`deferred_trees.rs`, `deferred_tree_shadow.rs`, rendering/deferred streams).

## State at handoff

- Every part of `docs/speedtree/generation.md` that concerns generation is implemented, through
  spec round 6 (§7.10) and the wind animation (§7.11). All merged; no open PRs, no parked
  branches. History: #335 (stage 1), #349 (stages 2–3), #351 (4–5), #356 (TreeMesh), #360
  (`thedas tree`, pinned hashes), #377 (mesh leaves, §7.4 LOD), #380, #386 (limits, fuzzing),
  #401 (wind weights, LOD alphas, bounds, billboard corners), #410 (self-shadow), #415
  (round 6), #461 (wind animation), #470 (corrected oscillator values per #468).
- Spec changes since then (#475 lighting frame and light scalars for billboards, the §7.8
  note on `billboard_transition`, 15f1228d) are renderer-side; the parser already reads those
  fields and the generator needed no change.
- All 11 questions of the implementer's last audit
  (`OpenThedas-testdata/notes/speedtree-spec-questions.md`) were answered in §7.10 and match
  the code.
- Open: QA was to re-measure tree sway against the original on a d893fa0 build after #470
  (the first measurement found half the original's sway, traced to the spec listing the
  oscillator values in the wrong order). I never received that result; ask QA or check its
  notes. If sway is still off, the fix starts as a spec amendment to §7.11.
- The `origin/speedtree-*` branches are all merged and can be deleted.

## How to build, test and verify

- Unit tests: `cargo test -p thedas-formats speedtree_gen` (fast, no game files).
- Game files: all 552 `.spt` are extracted (not in the repo) to
  `~/projects/OpenThedas-wt/speedtree-spt`. With them:
  - Pinned output of 7 representative files (oak, small pine, fern, boxwood, tall grass,
    cattail grass, cattail flower):
    `THEDAS_SPT_DIR=~/projects/OpenThedas-wt/speedtree-spt cargo test -p thedas-formats pinned -- --ignored`.
    Hashes are for macOS on Apple silicon (the platform math library feeds them). Change a
    hash only for an intended output change, and say why in the commit.
  - Mutation fuzz (must never panic):
    `THEDAS_SPT_DIR=... cargo test -p thedas-formats mutated -- --ignored`.
  - `thedas corpus` runs a full generate on every `.spt` it finds; all 552 pass.
- Visual check: `thedas tree <file> --png out.png`, compared with the original's screenshots
  in `reference/original/` (e.g. `trees_wilds_load.png`). Notes from the Korcari Wilds check
  are in `OpenThedas-testdata/notes/speedtree-shape-check.md` with PNGs next to it.
- Corpus table for rendering and QA: `OpenThedas-testdata/notes/speedtree-corpus.{tsv,md}`
  (per file: kind, wind level, triangles per LOD, leaves, bounds, billboard, generation time,
  fingerprint, plus suggested test trees). Generation of all 552 takes about 0.9 s in a
  release build; the largest (`brc100d_tre_f_oakhero`) about 12 ms.
- Scratch tools outside the repo: `~/projects/OpenThedas-wt/speedtree-survey` (a cargo
  project; `cargo run -- <spt dir>` surveys field values; bins `fuzz` (env PER, ONLY, SHOW,
  TIMEOUT), `cost`, `dump`, `grow`, `meshes`, `obj`, `svg`, `report` and others for one-off
  checks). Not needed for normal work.
- No saves, ports or game launch are needed for this stream. (It was allotted control ports
  7990–7999 and never used them.)
- Merge per CLAUDE.md: merge the exact latest `main` SHA in, fmt, clippy `-D warnings`, debug
  `cargo test`, `git diff --stat origin/main HEAD` shows only intended files, then merge.

## Design decisions and gotchas

- **One explicit random stream.** The original draws from one process-wide generator; here a
  `Random` is passed by `&mut` through every stage, so the order of calls is the order of
  draws. Almost every bug during development was a draw out of order or a missing draw; any
  new draw must sit exactly where the spec says (§5.1 has the global order).
- **`sin`/`cos` never fused.** Optimized builds fuse adjacent `sin` and `cos` into `sincos`,
  which differs in the last bit on macOS and changes trees between debug and release. Always
  call `frame::sin` / `frame::cos` (they go through `black_box`). The pinned hashes catch this.
- **Literal spec, even when odd.** Sparse grass (§5.5 A), the τ/τ′ asymmetry in §7.8, the two
  different atlas mappings for fronds and mesh leaves (§7.10 item 11) and empty branch LOD 0
  for 35 files (§7.10 item 8) all look like bugs and are what the original does. Don't
  "fix" them without a spec amendment.
- **Vertices carry wind as a parallel array** (`Part::wind`) rather than a field in
  `MeshVertex`, so the renderer's vertex literals kept compiling when wind arrived.
- **Errors, not panics.** `generate` and `grow` return `Result`; `TreeMesh::new` falls back to
  an empty tree, so a malformed mod file can't take the game down.
- Everything is marked provisional in its module doc until checked against the executable,
  as CLAUDE.md asks; the spec is the source of truth for this side.

## Known gaps

- Sway amplitude not yet confirmed against the original after #470 (see State).
- Pinned hashes only hold on macOS/Apple silicon; other platforms need their own or a
  tolerance-based comparison.
- Only 90 game files grow mesh leaves and 164 grow any leaves; rendering coverage of the rarer
  paths (extrusion fronds, mesh leaves with odd texture choices) is thinner than of the common
  ones.

## What a newcomer must not break

- The clean-room separation above. It is the reason this code can be published.
- The draw order and count of random draws (pinned hashes and the corpus check catch most
  breaks; run both for any generator change).
- Debug/release agreement (`frame::sin`/`cos`).
- `TreeMesh`'s public shape: rendering and deferred depend on `Part`, `MeshVertex`, `wind`,
  leaf cards, mesh leaves, billboard, `lod_alphas` and `Wind`. Change them with those streams.
