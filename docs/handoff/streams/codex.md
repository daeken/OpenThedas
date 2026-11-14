# Codex orchestration stream (2026-11-14)

On 2026-11-14 Sera paused the Claude streams to save Claude usage. One Claude thread ("Orchestrate Codex across paused streams") then drove
OpenAI's Codex CLI to carry on the work: deferred renderer, DA2, the Scaleform player, mods and the KNOWN_GAPS burn-down. Codex wrote and
tested the code. The orchestrating Claude reviewed every diff, merged it, and relayed announcements to the coordinator.

## Organisational structure

```
Sera ── coordinator (project chat) ── triage / sim / UI / rendering streams (owners of shared files)
                     │
                     └── Codex orchestrator (Claude thread)
                            ├── job: deferred   (worktree codex-deferred)
                            ├── job: da2        (worktree codex-da2)
                            ├── job: sf         (worktree codex-sf)       Scaleform player
                            ├── job: mods/gaps  (worktree codex-mods)     mods sweep, then KNOWN_GAPS
                            ├── job: ruffle     (worktree codex-ruffle)   one-off Ruffle removal
                            └── job: media      (no worktree)             curates Discord #media images
```

The orchestrator's responsibilities:
- Write a precise brief for each job, launch Codex, and relaunch it with an updated "State:" line after each round.
- Read every PR diff before merging. Auto-mode refuses a merge of a diff the orchestrator hasn't read, and refuses loops or parallel
  merges. Merge one PR per call.
- Announce behaviour changes in sim, UI or render files to the coordinator, and wait for the owning stream's OK. Turn each condition
  the streams set into a brief line and check that it was met before merging.
- Review the media Codex stages, then queue it for Discord #media.
- Keep project memory current (memory file `openthedas-codex-orchestration.md`).

## How it works

- **Binary:** `/Applications/ChatGPT.app/Contents/Resources/codex-cli/bin/codex` (not on PATH). Jobs run
  `codex exec -m gpt-6.1-sol -c model_reasoning_effort="xhigh" -s danger-full-access`. Media curation uses `high`.
  Sera approved `danger-full-access` in the thread. The auto-mode check refused it before her in-thread approval.
- **Isolation:** a fresh clone at `OpenThedas-wt/codex-base`. `~/projects/OpenThedas` was never touched. Each job has its own worktree
  `OpenThedas-wt/codex-<job>`, its own target dir `codex-<job>-target` and its own port range (8030-8049).
- **Briefs folder** `OpenThedas-wt/codex-briefs/`:
  - `RULES.md`: 15 hard rules covering isolation, merge procedure, no secrets or exe bytes, no SDK code, no Discord, ports, stop on
    blocks, final report format, announcements, media staging and delta panels.
  - `<job>.brief.md`: the job's goal plus a "State:" paragraph that the orchestrator rewrites each round.
  - `run.sh <job>`: runs codex exec in the job's worktree. Its output goes to `<job>.log`, the last message to `<job>.last.md`, and the
    report to `<job>.report.md`.
  - `cargo-slot.sh`: allows at most 2 concurrent cargo or gate runs on the shared Mac, requires 15 GB free, and runs at nice 19.
- **Codex's merge procedure:** merge origin/main into the branch, then run fmt, clippy `-D warnings` and the full tests. Sim changes also
  need the DA:O s01-s13/s16 gate vs main, detcheck, the corpus run and the six origin saves. Then push and `gh pr create`. Codex never
  merges.
- **Push:** use the SSH remote. The first clone used HTTPS and pushes failed until the remote was switched.
- **Media:** Codex stages `<name>.png` plus `<name>.caption` under `OpenThedas-testdata/codex-captures/media-staging/<stream>/`. The
  orchestrator looks at them, then copies the png and then the txt into `media-queue/<stream>/`, which the issue-report loop posts to
  #media. Sera approved this in the thread. Since her request, every comparison carries a third pixel-delta panel and states the % of
  pixels changed.

## What landed, about 110 PRs (#738-#865)

- **Deferred renderer:**
  - Shadows: point shadows, sun shadows, cutout casters, tree point shadows, shadow reuse.
  - Coverage: terrain, objects and GPU-skinned characters, faces and eyes, punch materials, cutaways and black materials, SpeedTree
    fronds, branches and leaves, eyelash and hair forward lighting. 33,159 of 34,360 passes are now in the G-buffer.
  - Lighting and transparency: opt-in probe lighting with SSAO, late transparency, ground-mark masking.
  - Performance: tiled culling, fused reflections, light masks, specialized pipelines and baked lighting.
  - Result: frame cost went from about 2x the faithful path to 1.06-1.41x. Faithful captures stay pixel-identical.
- **DA2:**
  - move-hero, GetAbilityList, CombatGetPercentageFromScaled.
  - Combat: miss/block resolution, native stats, level-up, stamina, recovery, ability upgrades, conditional passives, item power,
    authored basic attacks, animation cadence, ability ranges, projectile flight.
  - Story and world: cutscene completion, encounters, VFX reload, coin events, linked plot flags, CNV history, conversation owner.
  - Result: a fresh game reaches Lowtown and Act 1 up to Anders's clinic, and the Ogre is beaten.
- **Scaleform player:** filters, blends, setMask, text selection and caret, mouse and picker, timers, credits, journal, Shift,
  fuzz/benchmark tools. Ruffle was removed in #818, so thedas-gfx is the only UI player. `serde_json/preserve_order` is now explicit.
- **Mods:** the launcher report fixes, plus data/* priority, .rim in scanned folders, TLK and DA2 order traces.
- **Gaps:** FMT-012 and FMT-013, plus about 30 SCR items. Each was traced in DAOrigins.exe and gated to Origins. Notable ones:
  heartbeat scheduling, health floor, placeable health and bashing, GetItemsInInventory, HandleEvent, StringToInt and StringToFloat.

## In flight and parked, by area

Each area below says who owns it. "Codex only" means no human or Claude stream works on it other than this Codex orchestration.
Each job's full state is in `OpenThedas-wt/codex-briefs/<job>.report.md`.

- **Deferred renderer (Codex only).** The paused Claude deferred stream never resumed.
  - Open PR: none. A job was running on worktree `codex-deferred`, branch to be named `m1-deferred-*`.
  - Next: 1440p lighting and G-buffer cost (Tower, Cousland and Lothering are the worst; target about 1.3x the faithful path), the 136
    passes still lit faithfully, and wider material coverage.
  - Parked: the experimental lightmaps-off probe mode has no tuning yet.
- **DA2 (Codex only).** The DA2 Claude stream is paused.
  - Open PR: none. A job was running on worktree `codex-da2`.
  - Next: the DA2 night-map selection message, which blocks Darktown, then the prologue cleanup fight that still wipes the party at
    about t=903 s, then continuing through Act 1 from Anders's clinic.
  - Parked: face-bone offsets, the full DA2 package-order trace (`docs/resource-order-da2.md`), and DA2 IsHumanoid tree names.
- **Scaleform player (Codex only, but the UI stream owns `thedas-ui` and the app input files).**
  - #860 `sf-window-leave`: UI approved it once the scripts that only read hover state get a `hover` step first.
  - #864 `sf-glyph-winding` and #866 `sf-bitmap-sampling`: open, not yet reviewed.
  - Parked: the remaining tooltip and glyph-outline differences. The comparison index is at
    `OpenThedas-testdata/codex-captures/sf-host-polish/candidate/index.html`.
- **Mods (Codex only, but triage owns the shared resource stack, so announce any stack change).**
  - Open PR: none.
  - Parked: a real .dazip validation (only synthetic packages and registry metadata have been tested), the named RIM/TLK load
    lifecycle (`docs/resource-order-rim.md`, `docs/resource-order-tlk.md`), and the end-to-end trace of the data/ tie-break (marked
    PROVISIONAL).
- **Script gaps from KNOWN_GAPS (Codex only, but sim owns `thedas-sim`, so announce behaviour changes).**
  - #863 `m1-gaps-scr068-heartbeat-scheduling`: sim approved it on two conditions, detcheck 4/4 and a table showing the final plot state
    at the end of each step matches main. Codex was adding both.
  - Next: appearance-dependent walkmesh, animation-completion destruction, the remaining SCR items, then the simulation and UI gaps.
  - Partial: SCR-024, SCR-026 and SCR-031 (x87 numeric parity).

## Running it in a new harness

Keep the same pieces: a fresh clone with one worktree and target dir per job; a RULES file plus a per-job brief with a rewritable State
section; a 2-slot build limiter; Codex opens PRs and never merges; a reviewer reads every diff and merges one PR at a time;
announcements go to the owners of shared files before merging; and media goes through a staging folder with delta panels. The
briefs and reports in `OpenThedas-wt/codex-briefs/` are the best record of each job's state.
