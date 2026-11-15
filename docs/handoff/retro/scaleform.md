# Retro: the Scaleform player

From the thread that started `thedas-gfx` (inventory, design, the first player, #726). The
later `sf-*` work was done by other sessions; their lessons belong in their own notes and
are only touched on here.

## What went well

- **Inventory before design.** A small script over all 68 movies showed in half an hour that
  they were AS2-only SWF 8, with no buttons, sound, video or AS3, and only three filters.
  That cut the scope from "a Flash player" to "this game's movies". It is why a working
  player existed within a day.
- **A seam rather than a rewrite.** The first version kept `UiMovie`'s whole API and ran the
  existing injected glue unchanged, behind a runtime switch with Ruffle as the default.
  Nothing broke for play-testing while the new player caught up, and dropping Ruffle later
  (#818) was a deletion, not a migration.
- **Side by side from the first picture.** Every movie was rendered through Ruffle and
  through ours with the same engine glue. Later the original game's screenshot was added as
  a third panel. That turned "looks right" into a check, and it showed one place where ours
  beat Ruffle: the start menu's multiply.
- **Reading the movies' own code.** The worst bugs (portrait name panels showing, white
  quickbar slots, an unscaled background) were ordering bugs. The quick way to find them
  was a call tracer plus a disassembler, and reading the framework's `GetInitFunction` /
  `Initialize` paths. Guessing would have taken far longer.
- **Ruffle's source as a semantic reference.** We did not use its code, but reading its
  AVM1 action priorities settled ordering questions quickly.

## What could have been better

- **A merge committed conflict markers.** `git merge` reported one conflicted file, but
  another had markers too, and `git add -A` committed them. The full check caught it, but
  only after a long build. Grep for markers before every merge commit.
- **Shared-file churn with the UI stream.** Three UI PRs (#698, #703, #717) landed in
  `UiMovie` while the seam was in review. That meant two extra merges and re-runs of the full
  workspace check (each 10–30 minutes on a loaded machine). Asking the coordinator for a short
  merge hold on `thedas-ui/src/lib.rs` would have been cheaper.
- **Timing claims made too early.** One status message quoted per-movie idle times that had
  not been measured that way, and had to be corrected. Measure first, and say which build
  (debug or release) and what machine load.
- **Debug-build timing noise.** Most comparisons ran in debug builds on a machine at load
  40+. The ratio against Ruffle held, but the absolute numbers were misleading. A small
  release-only bench tool (`gfxrun GFX_RENDER=1`, later `movieaudit bench`) should have come
  first.
- **No engine state in the early side-by-sides.** Menus and the HUD were easy to compare;
  inventory, journal and conversation render empty without a game. The headless app capture
  path (`--capture --save --steps`) was found late, and the pause came before it was used.
- **Usage.** This stream burned a lot of tokens on long builds and full-workspace checks run
  repeatedly while main moved. The pause made that visible.

## Lessons

1. Inventory the data first; let it set scope and design.
2. Replace behind a switch that keeps the old path the default, with the same API, until the
   new path matches screen by screen. Then delete the old path.
3. Compare against the original game whenever a shot exists. The previous implementation
   is not the reference.
4. In an AS2 UI, almost everything is ordering: init actions, construction, `onLoad`,
   parent before child. Get the order right before adding built-ins.
5. Build debugging tools early: a disassembler, a call tracer with member names, a value
   printer and a tree dump. Each paid for itself within the hour.
6. Measure performance in release, say how, and keep the measuring tool in the repo.
7. Before merging a shared file, ask the owner to hold their merges for the window, or
   coordinate the order.
8. After a merge, grep for conflict markers before committing.

## How to improve (for the next harness)

- **Make the comparison harness the first deliverable** for any rewrite: original vs old vs
  new, scripted, with game state. The `--capture --save --steps` path plus a fixed set of
  MCP saves would make it routine.
- **Cut check cost.** Run crate-level checks (`-p thedas-gfx -p thedas-ui`) while iterating,
  and the full workspace check once, on the final merged head. Keep a warm per-stream
  target directory.
- **Small single-purpose branches**, as the later `sf-*` work did: one behaviour, one test,
  one capture each. They were easy to review and merge in parallel.
- **Write the "what must not break" list beside the code** (the design doc), and keep it
  current in every PR that changes an invariant.
- **Shared seams need an owner and a merge order.** Name, for each shared file, who merges
  first when two streams touch it.
