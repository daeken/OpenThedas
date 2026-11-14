# SpeedTree clean-room implementer stream: retro

## What went well

- **The clean room worked in practice.** A prose spec in the repo, numbered by section, was
  enough to write the whole generator (random stream to wind animation) without the
  implementer ever seeing the SDK. Stages 1–5 and a renderer-ready API went from first spec
  to merged in about a day.
- **The spec writer stayed a stage ahead.** While one stage was implemented the next was being
  written, so there was little waiting until the "(exe)" items, which needed tracing.
- **Questions by section number, answers as doc amendments.** Every unclear point went back as
  "§x.y: the code does A, confirm A or B", and came back as a numbered clarification in the
  doc (§2.9, §3.8, §5.5–5.7, §7.10). The doc became the record of every decision, which is
  what a successor needs, and nothing reached the implementer except through it.
- **Whole-corpus checks from day one.** Hooking `speedtree_gen::check` into `thedas corpus`
  meant every change was run on all 552 game files, not just the few under test. Off-by-one
  draws and edge cases showed up immediately.
- **Pinned output hashes** for seven representative files made every output change deliberate,
  and caught a real problem early: optimized builds fused `sin`/`cos` and changed trees in the
  last bit between debug and release.
- **Tools that let others judge the result.** `thedas tree` (PNG side/top views, OBJ) and the
  corpus table let QA and rendering compare shapes and density with the original's screenshots
  without reading the generator.
- **Robustness as a feature.** Limits, `Result` instead of panics, and a mutation fuzzer meant
  bad mod files produce an empty tree, not a crash.

## What could have been better

- **Amendments that changed earlier stages.** Stage 4's spec added random draws to stage 2,
  and later rounds corrected stage 1 wording. Each one meant re-checking merged code. Writing
  the global draw order (§5.1) first would have avoided most of it.
- **No way to check numbers inside the clean room.** The wind oscillator values were in the
  wrong order in the spec; the code matched the spec exactly, and the error only showed when
  QA measured sway in the running game at half the original's. The implementer can confirm
  only that code matches the spec, not that the spec matches the game.
- **Relay latency.** Every question went implementer → coordinator → spec writer → doc PR →
  coordinator → implementer. That was right for the clean room but slow for one-line answers.
- **Continuity across sessions.** When the Mac's drive layout changed, the session could not
  resume and a fresh one started with no memory of the work. The memory file and the thread
  history were enough to recover, but only because they had been kept current.
- **Platform-bound pins.** The pinned hashes hold only on macOS/Apple silicon, which will bite
  the first run on another machine.
- **Shared-machine pressure.** Disk dropped under the floor several times while many streams
  built at once; work paused for space more than once.

## Lessons learned

- In a clean room, the spec must carry test vectors, not only prose: expected numbers for a
  few inputs (first draws of the random stream for a seed, one branch's vertices for a given
  file, the wind values at a given time). They are data, not code, so they don't break the
  separation, and they would have caught the wind order before merge.
- Literal implementation pays off. Several things that looked like bugs (sparse grass, the
  τ/τ′ asymmetry, two different atlas mappings) were the original's behaviour; asking instead
  of "fixing" them kept parity.
- Pin constants with tests that name their spec section, so a spec correction shows up as a
  deliberate test change.
- Keep the stream's memory file updated in the same turn as each merge. It was the only thing
  that made a cold restart cheap.

## How to improve

- Ask the spec side for test vectors with every numeric section, and add them as unit tests.
- Give the implementer and spec writer a direct, logged channel for clarifications (still
  prose only, still landing as doc amendments), rather than routing each one through a
  coordinator.
- Make the pinned-output and corpus checks part of a single pre-merge script, and add a
  tolerance-based comparison (or per-platform hashes) so other machines can run it.
- In a harness where one agent can see everything, keep the separation by giving the
  implementer role a sandbox without the SDK, the trace notes or the exe disassembly, and treat
  `docs/speedtree/` as the only interface between the roles.
