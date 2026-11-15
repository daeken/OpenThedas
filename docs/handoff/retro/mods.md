# Retro: the mods and mod mode stream

Written 2026-11-14 by the stream's agent.

## What went well

- **Tracing the original before changing anything.** Each ordering rule (resource priorities,
  add-in registration, offers, folder scanning, the `GetPRCEnabled` handler, table merging) was
  read from the executable, written up with addresses, and marked provisional where it was
  inference. Reviewers could check the claim instead of trusting it, and later changes built on
  earlier traces without re-deriving them.
- **Proving "nothing moved" with a measurement.** For the folder-order change I dumped the
  winning layer of all 79,282 resource names before and after; only 70 byte-identical cursors
  and a `.DS_Store` moved. That turned a risky change to every file lookup into a small,
  reviewable PR. The side-by-side gate with every plot/inventory difference explained did the
  same for the offers change.
- **Small, ordered slices with a plan first.** The Blender plan note
  (`OpenThedas-testdata/notes/mod-mode-blender-plan.md`) split a vague "seamless Blender round
  trip" into export, live textures, mesh import, placement, writers, encoder, animation; each
  slice merged and was verified on its own (including a real round trip over all 8,117 models).
- **Round-trip corpus tests over the real install** (ignored tests with `THEDAS_INSTALL`) found
  dozens of mismatches that no unit test would have (normals, tangent w, binormals, skin bytes,
  palette fallbacks). They were worth their runtime every time.
- **Agreeing APIs with the owning stream before building on them** (sim commands, UI hooks,
  launcher conventions) meant almost no rework and clean reviews.
- **Merging only on green, on exact main** (fmt, clippy `-D warnings`, debug tests, re-run when
  `main` moved) kept `main` healthy through hundreds of parallel merges.
- Writing the player page, tester guide and gap entries alongside the code meant testers and
  other streams could use the work without asking.

## What could have been better

- **A table change broke character creation (#544, reverted).** The sim-only gates compared row
  winners by id; chargen picks classes by row position, which my change shifted. I had no check
  for positions and did not run the chargen click-through. Cost: a revert in the testers' cut.
- **I merged a stack change without announcing it (#553)** and was called on it. The rule
  (announce every `stack` change) is cheap; I should have asked what the rule was before the
  first stack change.
- **A first misreading of the exe** (treating packages named in a campaign file as add-ins, and
  reading "one gate for the whole add-in" from one of two registration functions) cost several
  PRs of rework. I had read one function and generalised. Rule: find every caller and the sibling
  function before writing the rule down.
- **Debug-only failures slipped through release runs.** The egui `TexturesDelta` assertion only
  fires in debug builds (#658). UI changes need one debug run.
- **Blocked by permission prompts at the wrong moments**: crate downloads and some merges were
  refused by the harness until Sera typed the approval in-thread; I lost hours waiting on two
  occasions. Asking for the exact sentence early, and batching approvals ("you may add any crates
  you need"), would have saved time.
- **Throwaway tooling lived in scratch folders** (disassembly helpers, the winner dump, the gate
  recipe). Some of it is only described in notes. Anything used twice should be committed (a
  `tools/` folder or an ignored test).
- **Usage.** Big builds and corpus runs (4-10 minutes each) were repeated on every re-merge of
  `main`. A lighter "does it still apply and pass" loop would have spent less.
- Some designs waited a long time for Sera's one-line decisions (egui, BC encoder, `unrar`).
  Putting the decision, my recommendation and the default in the first message would have shortened it.

## Lessons learned

1. A rule read from one function is a hypothesis until you have seen its callers and its twin.
2. Winners are not positions: when something downstream iterates, measure order, not just who wins.
3. Prove no-change changes by dumping the whole observable surface before and after.
4. Ignored, install-backed round-trip tests are the cheapest way to find format bugs.
5. Every shared-stack or shared-API change is announced to its owner first; "inert" is not an excuse.
6. Debug build once for UI; release build for speed everywhere else.
7. Never touch the install, never commit game or mod files, run headless runs with a scratch
   `HOME` and a hidden window.
8. Mark provisional claims in the doc comment where the code lives; it saves the next reader a trace.
9. Branch hygiene in a shared `.git`: WIP commit before switching; stage by path; never reset
   onto a moving `origin/main`.

## How to improve

- Add a **row-position check** (and a chargen click-through) to the standard gate, and a
  **resource-winner dump tool** to the repo so stack changes ship with their before/after diff.
- Commit the **disassembly helpers and trace scripts** (strings, import slots, vtable dump,
  file-offset to address) so the next tracer does not rebuild them.
- Keep one **index of exe traces** (address, what it proves, status) in `docs/`, not only in
  local notes, so streams stop re-tracing the same functions.
- Give each stream a **pre-approved list of routine actions** (merge on green, add a vetted crate
  type, run a headless game with a scratch `HOME`) so permission prompts do not stall work.
- Run **heavy checks once per merge window**: batch the corpus and origin saves for several
  waiting PRs instead of per re-merge.
- Finish what is parked: `m1-mod-report-fixes`, the open sweep items, and DA2's package order
  (needs a DA2 install to trace against).
