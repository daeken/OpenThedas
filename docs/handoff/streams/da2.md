# Stream handoff: Dragon Age II

## What this stream owns

DA2 support in OpenThedas, from reading the archives to playing the story headless. The constraint on every change: Dragon Age: Origins must not change.

- Formats and VFS as they differ for DA2:
  - ERF V3.0 name-hash index (FNV-1 64 of the lowercase name)
  - `.crf` and `.rimp` archives
  - `#hash.ext` display names
  - GFF4 strings with a trailing NUL
  - binary `.mao` and compiled material definitions
  - `PLOT` V2.0
  - CONV 3.0
  - `.cut` and `.cl`
  - `.rml` numeric lightmap chunks
  - `.vlm` vertex lighting
  - `*_p.tlk` patch tables
  - DLC keys files
- Game detection: `thedas_vfs::Game` and `DaoInstall::game()`. DA2 is the install that has `modules/campaign_base`. `campaign_module()` gives the default module.
- The DA2 sides of shared systems: script engine functions and constants, locals, conversations and speakers, plot flags, chargen, combat rules, encounters, creature models and equipment.
- Rendering owns the DA2 shaders and drawing. Sim owns the script VM and the shared simulation, and is told about any change there.

## Current state (2026-11-14)

- All 4,868 DA2 archives open, and every GFF3/GFF4 parses and round-trips. The typed corpus is clean apart from a few DDS and pixel-shader leftovers that rendering owns.
- The prologue plays headless from a new game:
  1. character creation (quick start is a male warrior)
  2. the fake waves and the ogre
  3. the real blightlands
  4. talk triggers, then Aveline and Wesley
  5. Flemeth's dragon and the `dae000_flemeth` conversation
  6. back to Cassandra and Varric
- Media of each step went to #media.
- Known blocker: on a fresh run, the fight against the hurlocks left after the ogre still wipes the party (t=903150). Codex found no proven combat defect and made no speculative change, so the cause is still undiagnosed.
- Codex DA2 work landed on 2026-11-14 (#759, #772, #784, #801, #804, #810, #820, #821, #823, #824, #828, #829, #830, #841, #842, #849, #855, #856, #861, #862). It covers:
  - stats, encounters, level-up, stamina and recovery
  - ability ranges, context and impacts
  - projectiles
  - item power
  - animation readiness
  - cutscene completion
  - conversation history and ownership
  - plot-flag links
- Each of those has a note in `docs/notes/da2-*.md`.
- From a saved Gallows checkpoint, Act 1 now runs as follows:
  1. Varric's Grey Warden lead
  2. Lirene's directions
  3. Darktown travel and a won coterie fight
  4. a proper visit to Anders's clinic, where Hawke agrees to help Karl
- This is a checkpoint continuation, not a fresh run.
- Next Act 1 blocker: DA2 night-map selection. The world map's night button sends `WorldMap.WorldMapSetPrimaryMapByTag` with `wm_city_night`, but the sim has no handler, so the map stays on day. The next step is to trace that handler natively and wire it in, then continue Karl's Chantry route.

## In-flight and parked work

Nothing is open from this stream, and no DA2 PRs are open. `da2-movehero` was redone and landed as #741, which keeps hero approaches outside creature capsules. Codex's evidence for the Act 1 route, frozen binaries and resume saves are under `OpenThedas-testdata/codex-da2/2026-11-14-cont5/`, outside the repo. Resume points:

- after accepting Karl: `replay/clinic-final-end.json`
- at the world-map exit: `replay/chantry-final-end.json`, t=1303942

## Build, run, verify

Use a scratch `HOME` for every run and test, with `CARGO_HOME` and `RUSTUP_HOME` pointed at the real ones. The real `~/.openthedas` holds a live Discord webhook. Use one cargo target dir per worktree.

```bash
# quick looks
thedas sim /Volumes/External2/da2 --area dae110ar_lowtown --ticks 5
thedas sim /Volumes/External2/da2 --area dae110ar_lowtown --talk mer110cr_jansen --replies 0,0,0
thedas dump 'archive:#<hash>.<ext>'
# render a save headless (camera is x,y,z,yaw,pitch in degrees; omit it to follow the hero)
thedas-app --install /Volumes/External2/da2 --save <save.json> --camera "x,y,z,yaw,pitch" --capture out.png
```

- **Story driving.** Drive the story through `thedas-mcp` with the testdata `drive.py` and `drive.INSTALL` set to the DA2 path. A fresh prologue starts from `load(install=..., area='pro000ar_blightlands_fake')` with autopilot on. For the conversation-level flow, see the old driver scripts, which this session kept in scratch space (not in the repo).
- **Keys.** DA2 DLC needs a player-supplied keys file in `erfpasswords.txt` format. The lookup order is `--keys <path>`, then `THEDAS_KEYS`, then `keys.txt` in the user data dir. Never log, commit or report key material.
- **Gating, for every PR that touches shared sim, script, MCP or reader code:**
  - fmt and clippy with `-D warnings` on all targets
  - the workspace tests
  - `gate.sh <mcp-main> <mcp-branch> s01..s13` (the full DA:O chain, not one step, when walking or MCP behaviour changes)
  - `detcheck.sh`
  - the DA:O corpus: `thedas corpus /Volumes/External2/projects/DAODecomp/gamedata` must PASS, with `script/undeclared-action` unchanged (5219 when last run)
- Gate the binaries you actually built. Once a package-name typo left a stale binary in place, and the gate "passed" without testing anything.

## Key files and decisions

- **`crates/thedas-vfs`:** the hash index, `Game`, `keys_file`, and the ERF type 3 (Blowfish) reader.
- **`crates/thedas-sim/src/cnv.rs`:** CONV 3.0 into `Dlg`. Speakers are resolved in this order:
  1. the `.cl` performance actor with an ASLN track
  2. otherwise the follower that link field 30303 names, a row in `conversation_followers` (M2DA 296, column `Tag`)
  3. otherwise the owner
  The PARTNER fallback applies only to conversations the player owns.
- **`dialog.rs`:** `Sim::line_speaker_tag`.
- **`engine.rs`:**
  - `local_name`: DA2 passes local names as ints. They are row ids of `var_<kind>.gda` (column `Label`), or template VarTable order for foreign tables such as `var_trigger_talk`.
  - DA2-only functions.
- **`session.rs`:** `add_dragon_age_2_constants`. DA2's script.ldf has no PROPERTY_* constants: health is 7, mana/stamina 10, xp 35 and level 36.
- **`plot.rs`:** PLOT V2.0, composite flags, and the plot links Codex added (13306).
- **`combat.rs`:** DA2 blows. rules_core sets an attack result only for deathblows, so result 0 with an impact effect is a hit.
- **`thedas-ui` `models.rs`:**
  - DA2 creature models from `item_variations.gda`, using full lowercase art paths
  - head morphs
  - deaths through the damage-reaction tree (column hash 0x367842f0)
- **Animation and morph name hash:** h = h*1000003 ^ c over lowercased UTF-16 units. It was found in the exe and is implemented in the readers.
- **How decisions were made:** trace the native exe (statically, from Sera's decrypted analysis copy, read-only) or read the data. A heuristic ships only when that can't be done, and it is labelled as an inference. Addresses go in notes; exe bytes never do.

## Known gaps

- The cleanup fight after the ogre wipes the party on fresh runs (undiagnosed).
- No night-map handler (`WorldMapSetPrimaryMapByTag`).
- Unperformed lines by NPCs who aren't followers go to the conversation owner. The speaker's rig is named in `<strref>_m.fxe`, but those archives aren't mounted.
- DA2 face-bone morph offsets are parked.
- Plot 13320 area-state actions, plus full cinematic and journal presentation, are unfinished.
- Old saves don't replay plot links that were missed earlier.

## What a newcomer must not break

- DA:O behaviour. Every DA2 path is gated on `Sim::game` or the install, and DA:O's gate, detcheck and corpus must stay identical.
- Never commit game data, derived bytes, exe bytes, disassembly or key material. The DA2 install and the decrypted exe are read-only.
- Saves stay loadable. New fields get `serde(default)`, as `Locals.order` and `Event.vectors` do.
