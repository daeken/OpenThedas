# OpenThedas handoff

Written 2026-11-14, when the project moved to a new agentic development harness. This page is the overview. Each workstream's own handoff is in [`streams/`](streams/), and the retrospective is in [`RETRO.md`](RETRO.md).

## The goal

OpenThedas is a Rust replacement engine for Dragon Age: Origins, and later Dragon Age II, running the original game data. **Parity with the original executable comes first.** Modernization is welcome where it doesn't break parity (an optional deferred renderer, MSAA, a launcher, mod mode). Targets: macOS first, then Windows and Linux, later iOS and Android. The web is a non-goal. The licence is MIT.

The working rules are in [`CLAUDE.md`](../../CLAUDE.md). The short version:
- No game data or exe bytes in the repo.
- xoreos and the SpeedTree SDK may be read, never copied.
- The simulation is deterministic and headless.
- Format code never panics.
- Anything not traced in `DAOrigins.exe` is marked provisional.

## Repo layout

| Crate | What |
|---|---|
| `thedas-vfs` | archives (ERF, RIM, .dazip, .7z, .rar), resource stack, mod layers, DLC keys |
| `thedas-formats` | file formats: GFF, GDA, TLK, MMH/MSH, MAO, ANI, FaceFX, SpeedTree parse and generation, DA2 variants |
| `thedas-script` | NWScript VM for compiled .ncs |
| `thedas-sim` | the deterministic simulation: rules, commands, combat, conversations, cutscenes, saves |
| `thedas-render` | wgpu renderer: the faithful D3D9 path (translated .fxo shaders) and the optional deferred path |
| `thedas-shader` | D3D9 shader bytecode translation |
| `thedas-gfx` | our Scaleform/Flash player, the only UI player since Ruffle was removed in #818 |
| `thedas-ui` | interface bridge between the movies and the sim |
| `thedas-audio` | FMOD-bank playback, our own mixer, reverb and occlusion |
| `thedas-video` | native Bink decoder (#663; no ffmpeg) |
| `thedas-launcher`, `thedas-store` | launcher, setup wizard, Steam/GOG download (sign-in never tested for real) |
| `thedas-app` | the game binary (`thedas-app`): window, input, pad, mod mode, issue reports, self-update |
| `thedas-cli` | `thedas` tools: `corpus`, `addins`, `mods`, `tree`, and more |
| `thedas-mcp` | headless MCP harness that drives the sim for gates and QA |

Docs worth knowing:
- `docs/KNOWN_GAPS.md` and `docs/gaps/<area>.md`: the list of record of everything wrong or missing, with stable IDs (SIM-, SCR-, RND-, UI-, AUD-, CAM-, FMT-, TOOL-). A fixing PR deletes its line and cites the ID.
- `docs/deferred.md`, `docs/scaleform-player.md`, `docs/speedtree/` (the clean-room spec), `docs/launcher.md`, `docs/mods.md`, `docs/mod-mode-testing.md`, `docs/resource-order-*.md`, and `docs/notes/da2-*.md`.

## Build, run, test

```sh
cargo build --release -p thedas-app -p thedas-cli
THEDAS_INSTALL=/path/to/Dragon\ Age thedas-app            # the game (no install set: the launcher)
thedas-app --save <save.json> --steps "run 100" --capture out.png   # headless capture
thedas corpus "$THEDAS_INSTALL"                              # every archive and format against a real install
cargo fmt --all && cargo clippy --all-targets -- -D warnings && cargo test   # debug tests, always
```

- **Headless runs** set `THEDAS_HIDDEN_WINDOW=1` and `THEDAS_ASYNC_LOADS=1`.
- **Tests and automation never use the real HOME.** Use a scratch `HOME` with `THEDAS_MODS=none`, because the real `~/.openthedas` holds a live report webhook.
- **Frame-rate figures** are in the window title, and logged every 10 s (#681, #692).
- **Windows** is cross-built from Linux with `cargo zigbuild` (`x86_64-pc-windows-gnu`, `aarch64-pc-windows-gnullvm`). A `llvm-windres` shim embeds the icon. See [`streams/triage.md`](streams/triage.md).

## Merge rule and gate

**Merge rule** (CLAUDE.md):
- Merge the latest `main` into the branch, then run fmt, clippy and debug tests on that exact result.
- If `main` moves into the same files while the checks run, do it again.
- Stage by path, and never reset onto `origin/main` by name.
- A change to another stream's code is announced to its owner and waits for their OK.

**The gate.** It lives outside the repo, in `OpenThedas-testdata/` on the Mac. It is required for sim behaviour changes and tester candidates:
- `gate/gate.sh <mcp main> <mcp branch> [steps]`: scenario steps from Human Noble through Ostagar and the tower (s01-s16 exist; the default runs five, and candidates run ten). Compares end areas, plot sets and log diffs.
- `gate/detcheck.sh <mcp>`: four saves run twice and through save and reload must give identical worlds.
- `sim-work/s13seed.sh`: tower seeds 1-3, judged by step and call counts, never wall time.
- A chargen click-through (`triage/convcam/bin/convcap.sh dcommon|hnoble`) and the six origin saves.
- `thedas corpus` for anything that touches parsing.

**Every gate difference is explained in the PR.**

## Machines, paths and ports (the maintainer's Mac)

- **Volume and repos.** Everything runs on `/Volumes/External2`, and `~/projects` links there.
  - Repo: `~/projects/OpenThedas`.
  - Worktrees: `~/projects/OpenThedas-wt/<stream>`, each with its own target dir.
  - Test data, outside git, never committed: `~/projects/OpenThedas-testdata` (saves, gate, notes, references, recipes).
- **Game data**, read-only: DA:O at `~/projects/DAODecomp/gamedata/`, DA2 at `/Volumes/External2/da2`, mods at `/Volumes/External2/daomods`.
- **`play.command`** is the maintainer's launcher. It always points at the newest QA-graded commit. Pins copy a binary; nobody builds in that checkout.
- **Control ports:**

| Stream | Ports |
|---|---|
| mods | 7870-79 |
| DA2 | 7890-99 |
| issue reports | 7900-09 |
| triage helpers | 7910-29 |
| triage | 7930-39 |
| QA | 7940-49 |
| sim | 7950-59 |
| UI | 7960-69 |
| rendering | 7970-79 |
| audio | 7980-89 |
| SpeedTree | 7990-99 |

  Run `lsof` before binding, and kill by pid only.
- **Shared-machine rules:** keep about 15 GB of disk free, hide engine windows, play no sound, and never close the maintainer's game windows.
- **The original game** runs under CrossOver for reference captures (QA only, one session at a time). References are in `OpenThedas-testdata/reference/original/`.
- **Windows 11 ARM VM** (Parallels): QA owns smoke tests. It runs our OpenGL path, since there's no D3D11 backend in wgpu.
- **Testers:**
  - Builds are served from the maintainer's server, through a private `latest.json` that the self-updater reads.
  - Reports open GitHub issues labelled `player-report`, and Discord posts go through the bot.
  - Only the issue-report stream posts or publishes. Nothing goes to testers without a Windows VM pass and the maintainer's go.

## Where each stream stands

| Stream | Handoff | State, and what's next |
|---|---|---|
| Coordinator | [coordinator](streams/coordinator.md) | hub for routing, approvals and project memory |
| Triage and releases | [triage](streams/triage.md) | Testers have 8137c5d. First update candidate cb8205a is gated GREEN and waits on the VM grade and the maintainer's go. Beraht's moonwalk is ours: the original shows none, and the cause isn't found yet. |
| Combat and sim parity | [sim](streams/sim.md) | All merged. Open: arrival placement (0x6c73c0), item attribute requirements, the jump formation case. |
| Controller and UI | [input-ui](streams/input-ui.md) | All merged. About 180 UI- gaps; anti-aliasing setting storage unverified. |
| Rendering parity | [rendering](streams/rendering.md) | All merged, including lip sync (FaceFX) and MSAA. Parked branch `m1-da2-runtime-light`. |
| Deferred renderer | [deferred](streams/deferred.md) | Optional renderer behind `--renderer` / F11. Next: 1440p cost (target about 1.3x the faithful path), and the 136 passes still lit faithfully. |
| Audio | [audio](streams/audio.md) | Done for the first pass. The reverb is ungraded; spell and projectile sounds are next. |
| QA | [qa](streams/qa.md) | grades against the original and runs the Windows VM smoke tests |
| SpeedTree clean-room | [speedtree](streams/speedtree.md) | Generator complete through spec §7.11. Sway re-measure after #470 is unconfirmed. |
| Issue reports and packaging | [issue-reports](streams/issue-reports.md) | Reports, self-update and the Discord bot are live. `latest.json` lists 8137c5d. |
| DA2 | [da2](streams/da2.md) | Prologue plays headless; Act 1 runs from a Gallows checkpoint. Blocker: the post-ogre fight wipes the party at t≈903 s. |
| Mods and mod mode | [mods](streams/mods.md) | Parked branch `m1-mod-report-fixes` (merge main, check, PR). No real .dazip validated yet. |
| Launcher | [launcher](streams/launcher.md) | Merged. No real Steam or GOG sign-in yet. Mac and Windows accessibility checks pending. |
| Scaleform player | [scaleform](streams/scaleform.md) | the only UI player; open PRs #860, #864, #866 |
| Codex orchestration | [codex](streams/codex.md) | runs Codex jobs for deferred, DA2, Scaleform, mods and gaps; RULES and briefs in `OpenThedas-wt/codex-briefs/` |
