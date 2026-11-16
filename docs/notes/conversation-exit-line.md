# Conversation exit replays the answered line (#1033, #1036)

Static investigation, 2026-11-15; runtime validation resumed 2026-11-16 after
the disk-space blocker was removed. Engine comparison: main `6d66c10e` and
candidate `67a379dd` (the fix merged with that main).

The reports distinguish an exhausted reply from a new final NPC line. In #1033,
`bdc100_kasch` remains on NPC node 6 (strref 41009) after selecting reply node 33
(strref 41038), whose child list is empty. In #1036, `bdc130_bartender` remains on
NPC node 11 (strref 41220) after selecting reply node 35 (strref 41237), also with
no children. Both saved conversations have `ended = true`, no replies, and the
old NPC node still in `line`. Both reports show `{Conversation}` with no cutscene.

`Sim::advance_reply` ended the graph but retained that answered line.
`thedas_ui::binding::conversation` deliberately presents ended conversations
with nonempty text, because a newly spoken terminal NPC line still has to play.
The app's `LineKey` includes the reply list, so removing the replies creates a
new presentation line and resets its timer. `Sim::conversation_line` filters
ended graphs out, so audio and the conversation camera already see no open line.
The old text consequently waits for a fresh unvoiced reading timer while input
remains behind the conversation screen. The listener fade does not own that
screen or timer. For the retained English text, the fallback is 3810 ms for
Kasch and 4745 ms for the bartender; these are source-derived times, not a
measurement of the reported roughly ten-second delay.

The fix clears `line` only when a selected reply has no usable successor.
A new terminal NPC line returned by `speak` remains available to presentation.
The existing UI then wants the HUD on its next update, without a new app-side
simulation write, timer override, or forced mode change. Unit coverage exercises
both paths for Origins and DA2.

## Original executable comparison

Read-only static source: DAODecomp `local/disassembly-llvm.txt`, matching GOG
1.05a DAOrigins.exe, SHA-256
`2346caee611002d4dff6ecef68eb315b63a37004dd98b39c11a9c26c0b45bfb3`.
Addresses are virtual addresses (image base `0x400000`). The original was not run.

- `GameModeDialog` consumes a queued reply at `0x68de07–0x68de18`, calling
  `0x68d8f0`. That helper calls conversation advance `0x7f1910` at `0x68dbca`.
- If advance returns false, `0x68dbcf–0x68dbd5` calls `0x68d810` immediately.
  That helper restores the resolved listener's opacity to 1 through `0x695060`
  at `0x68d826`, then sets the dialog's termination flag `+0x2bc` at `0x68d82b`.
- The dialog update checks that flag at `0x68de22` and jumps directly to
  `0x68e225`, bypassing line/cinematic timing and default framing. The ordinary
  line timer update is `0x68e1c7–0x68e221` and is also guarded by this flag.
- Separate teardown at `0x68f4a7–0x68f4c3` releases the owned line cinematic;
  reset at `0x68f420` clears the line timer and line/reply state. This is distinct
  from keeping a completed cinematic attached while waiting for a real reply.

This establishes that an exhausted reply terminates rather than starting another
hold of the answered line. It does not establish a wall-clock duration for the
original HUD restoration or resolve other terminal-NPC-line timing gaps.

## Reproduction

The external testdata script `sim-work/repro/convhang.sh <thedas-app>` derives
pre-exit fixtures from each report's `save_now.json` and final journal reply.
It restores the answered node's reply and dialog mode, then takes that reply
through the app. This isolates the bug without replaying unrelated travel,
inventory input, or build-specific journal hashes. It is not a full replay from
`save_before.json`. The strict CLI replay refuses the reports' `8bf06c21` build
identity on this build. A separate attempt from the unmodified earlier saves,
retaining only conversation orders, reply choices and reply intervals, takes a
different branch or fails to open the conversation when other journal inputs
are omitted. It is inconclusive; no full-journal equivalence is claimed and the
strict replay guard was not bypassed.

The script requires HUD restoration within its first 250 ms sample and a
nonzero leader move input through the app's controller path and more than 0.1
units of actual leader displacement by 500 ms. Play inputs are read from a fresh
app journal (`--journal` in the scratch HOME), rather than the save's edit-only
command journal. It logs
48 subsequent 250 ms samples so a failing binary's delayed HUD return is visible.
It uses a hidden capture, scratch HOME, no mods/audio, and the shared resource
launcher. Outputs stay under testdata `codex-captures/convhang`.

The focused repro fails both cases on main and passes both on the candidate.
Main's first HUD samples are at 4000 ms for #1033 and 5000 ms for #1036; neither
has a nonzero movement input or leader displacement by the 500 ms save. The
candidate shows the HUD at the first 250 ms sample and records fresh movement
inputs and leader displacement by 500 ms in both cases. Both saved simulations
are in mode 0, with no pending talk or cutscene at that point. These times are
elapsed headless input time, not wall-clock timings of asset loading.

## Regression gate

The serial, isolated `gate_round2.sh` recipe passes all 12 s01–s12 steps on
both binaries: zero gameplay-log differences, 503 ordered plot entries identical,
matching endpoint areas and no refused saves. S13 reaches Flemeth's hut on both
at time 1918259 ms, with the same last diagnostic counters (550 step requests,
11919 MCP calls). Its logs match after excluding wall-time `SLOW` diagnostics
and normalizing the output-directory path. The source tower fixture's MD5 is
`4668c45a9f092333fab9f124a3d45c55`.

The complete decoded save comparison finds exactly these eight differences:

| Snapshot | Main `talk.line` | Candidate `talk.line` |
| --- | ---: | --- |
| `01_cousland_start.json` | 109 | `null` |
| `03_after_fergus.json` | 10 | `null` |
| `04_siege_start.json` | 44 | `null` |
| `08_wilds_start.json` | 3 | `null` |
| `08a_ostagar_duncan.json` | 106 | `null` |
| `10_wilds_cache.json` | 1210 | `null` |
| `11_after_joining.json` | 45 | `null` |
| S13 `13_flemeth_hut.json` | 1177 | `null` |

Each is an ended conversation with an empty reply list in both versions: the
candidate drops the answered line as intended. Every other saved field matches.

Detcheck runs 600 ticks from `04_siege_start`, `07_ostagar_arrival`,
`09_wilds_vials` and `sc_tower_midfight_named` on both binaries. All eight runs
match on repetition and on save/reload halfway; the four main-versus-candidate
normalized worlds also match. Normalization is the standard detcheck harness's
bookkeeping-counter and event-sequence exclusion; the complete difference lists
are empty.
