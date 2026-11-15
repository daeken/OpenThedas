# Rendering stream: retro

## What went well

- **Tracing the original before changing behaviour.** Reading the executable's code for the
  thing at hand (cutscene light tracks, the FaceFX face graph, conversation depth of field, the
  shadow constants) gave fixes that matched the original instead of looking plausible. Where a
  trace was not possible, the provisional notes in the code and the gap docs kept the
  uncertainty visible.
- **Headless captures as the verification loop.** `--steps ... --capture` with saves and
  crops, plus side-by-side comparisons and PSNR, made most visual changes checkable without a
  window and without running the original. QA's original captures closed the loop.
- **Temporary instrumentation.** Env-var overrides and logs in a scratch build (forced face
  curves, constant dumps, model names, draw-group counts) found causes quickly: the DA2 sun
  turned the wrong way, the speaker's rig lookup failing on named-character bodies, the
  conversation DOF tables numbered differently in DA2.
- **Profiling before optimizing.** A `sample` of the settled camp showed posing at about half
  of the main thread, not draw submission, so the work went where it paid (53% → 15%), and the
  "draw batching" ask was answered with data (the trees were already instanced).
- **Small PRs with a mechanical merge check.** One change per branch, merged after the checks
  ran on the exact merge with `main`, kept a fast-moving shared `main` green.
- **Cross-stream reviews through the coordinator** caught overlaps early (the deferred stream's
  ring and MSAA changes), and handing work to the stream that owned the code (the loading
  screen's multiply blend to UI) went faster than doing it in someone else's area.

## What could have been better

- **Usage.** A week of usage went in about a day. Long autonomous loops, big rebuilds under a
  loaded machine, and re-running checks every time `main` moved all cost a lot. Some captures
  were repeated because the first try used the wrong save, a paused tutorial, or the wrong part
  of a cutscene.
- **Repro setup took longer than fixes.** Several bugs (Beraht's walk-in, cutscene lip sync)
  needed the right game state; saves made by tools that skip cutscenes, or saved with a
  tutorial pause, did not reproduce them. Agreeing early on a canonical way to reach each scene
  would have saved hours.
- **Machine load made timing unreliable.** With many streams building at once (load average
  30 to 80), wall-clock numbers were noise; only profile shares were usable.
- **Merge races.** `main` moved during most check runs, forcing reruns. A merge queue would
  have avoided that.
- **Occasional claims ahead of evidence.** Once a debug view was misread, once a plan was
  promised to another stream (bypassing MSAA in deferred mode) and then implemented
  differently. Both were corrected, but stating only what has been checked is cheaper.
- **The permission layer** refused some actions (a one-command merge, a hard reset) without a
  clear rule. Those cost round trips.

## Lessons learned

- Look at the data first: a single dump (the cutscene's actors and tracks, the atlas's alpha,
  the conversation line's `CUTR` reference) usually explained more than reading code.
- Name and parse by the game's own names when numeric ids differ between games (DA2's 2DAs).
- Model names are not uniform: named characters' bodies (`nm_…`) carry no race prefix; prefer
  the head morph.
- A save is not a neutral starting point: check `tutorial_paused`, skipped cutscenes and the
  conversation state before trusting a repro.
- Keep presentation strictly reading the simulation; it made the cutscene and lip-sync work
  safe to iterate on.
- For "it looks wrong" reports, get the screenshot. The "ring over the NPC" was the aiming
  reticle, which no description would have shown.

## How to improve

- A **repro catalogue**: per scene, the save or step recipe that reaches it from a clean state
  (new-game flows included), kept with the test data and updated whenever a bug needs one.
- A **merge queue** (or a single merger) instead of every stream racing `main`.
- **Golden captures**: a small set of Origins and DA2 frames checked automatically for the
  faithful path on every PR touching the renderer, so "Origins frames identical" is a CI check,
  not a manual step.
- **Budget-aware scheduling**: batch captures, reuse builds across checks, and avoid large
  rebuilds while other streams build.
- Keep **one owner per area** and route cross-area changes through it, as the coordinator did;
  write the handoff notes as you go (this doc took one pass because the memory notes were
  current).
