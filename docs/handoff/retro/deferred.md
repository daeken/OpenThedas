# Retro: the deferred renderer stream

## What went well

- **Hybrid first, coverage later.** The first PR put only Static1 level geometry in a
  G-buffer and drew everything else with the original shaders on top. It merged in a few
  hours, never touched the faithful path's behaviour, and gave a working F11 toggle from
  day one. Every later family (terrain, objects, characters, trees, cutouts) slotted into
  the same seam.
- **Writing the design first.** `docs/deferred.md` was written before code, kept in the
  repo, and updated with each PR. Codex picked the stream up from it and carried it from
  two PRs to about twenty in a day.
- **Debug views and off switches.** The G-buffer views (normal, lightmap, visibility,
  diffuse, flat, wrap, shadows) turned two "it looks wrong" moments into concrete
  findings within a capture or two: the falloff wrap behind the darker floor, and room
  placement swallowing the emitter lights. Codex kept the habit, adding a
  `THEDAS_DEFERRED_*` switch per addition and same-process ABBA benchmarks.
- **Portability tests.** `backend_check` caught storage buffers and depth `textureLoad`
  (not supported on GL) before anything ran.
- **Posting side-by-sides early, broken ones included.** Sera loved them, and they doubled
  as documentation. The media drop folder let us post without a coordinator round-trip.

## What could have been better

- **Build and capture turnaround on the shared Mac.** With load averages of 20 to 50 from
  other streams, a release build took 10 to 20 minutes and each capture several more.
  Many tuning iterations were one constant change followed by a full rebuild. Changing
  emitter and shadow constants through env vars from the start would have saved hours.
- **Tuning blind.** Emitter-light strength was first guessed (it flooded the tower
  orange), then overcorrected (invisible), before I added the light log. Add logging or a
  debug view before tuning, not after.
- **Wrong assumptions in test setup.** I copied "pad a ×4" from QA's recipe. It talked to
  an NPC, the resulting popup obscured every save capture, and other streams chased a
  "bug" that was our own input. Use viewer mode (`--area` + `--camera`) for rendering
  shots.
- **Edits during builds.** I edited sources while a build was running and couldn't tell
  whether the binary had them. Commit or snapshot before building, or touch the files
  afterwards.
- **One feature, many reasons.** Room-box placement for roomless lights looked right but
  silently removed light from Ishal's floors. Check each new visibility rule with its
  own debug view before stacking more on it.

## Lessons learned

- Know the target API's hard limits before designing data layouts: 32 bytes per pixel of
  attachments, a 16 KiB portable uniform array, no storage buffers on GL. The G-buffer
  had to be redesigned twice around them.
- When "ours looks different", first prove what the original does (here, the Static1
  disassembly and the falloff wrap). That turns a vague aesthetic dispute into one
  documented, switchable decision.
- A large flame light needs a near-plane exclusion, or its own torch bowl shadows
  everything (0.35 m worked).
- Keep the reference renderer untouchable. Read-only accessors and a planning step that
  faithful loads skip let two streams work on the same renderer files without stepping on
  each other.

## How to improve

- Give the new harness a fast loop for rendering work: a long-lived viewer process
  driven by a control port, hot-reloadable constants (env or a small TOML), and a
  "capture both renderers and stack them" command. Rebuild only when code changes.
- Keep a fixed set of reference cameras (Cousland hall, Tower torch, Tower fight,
  Ostagar at night) and an automated before/after diff across both renderers. Run it
  for every PR that touches `thedas-render`.
- Give heavy builds an explicit budget or queue across streams on shared hardware, and
  give each stream its own target dir (we did) and its own time slot (we didn't).
- Coverage is now broad. The next wins are quality, not more families: real mesh
  tangents, more shadow slots and an atlas, and turning lightmaps-off into a
  player-facing option once the probe and SSAO leaks are fixed.
