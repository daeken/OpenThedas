# Conversation exit replays the answered line (#1033, #1036)

Static investigation, 2026-11-15. Runtime reproduction and validation are pending:
the required build launcher refused to run with less than 15 GB free disk.

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
`save_before.json`; that requested runtime investigation remains pending.

The script requires HUD restoration within its first 250 ms sample and a
nonzero leader move order through the app's controller path by 500 ms. It logs
48 subsequent 250 ms samples so a failing binary's delayed HUD return is visible.
It uses a hidden capture, scratch HOME, no mods/audio, and the shared resource
launcher. Outputs stay under testdata `codex-captures/convhang`.
