# Clicking a speaker during an ambient bark

DAOrigins.exe GOG 1.05a, SHA-256
`2346caee611002d4dff6ecef68eb315b63a37004dd98b39c11a9c26c0b45bfb3`.
Static trace only; the original game was not run for this investigation.

The conversation command executor at 0x7a3700 sends DIALOGUE (18) to the target.
The shipped `rules_core` UT_Talk path clears ambient dialogs before calling
BeginConversation. Thus a guard only inside BeginConversation does not protect
against repeated player clicks: each click has already cleared the active record.

The native ambient manager arbitrates the speaker against active speech before
starting a voice (0x6ffaa0 calls 0x6ff190; only an accepted line goes through
0x6fe2f0/0x6fe4a0). It tracks the speaker and the current line lifetime separately
from the incoming conversation request. OpenThedas previously discarded that
lifetime in the script's clear, while its already-started audio continued playing.

The Origins command path now leaves a target's active ambient conversation alone
before dispatching DIALOGUE. A subsequent click after its line/conversation ends
is accepted normally. This reproduces the reported native no-restart behavior;
ClearAmbientDialogs' explicit script behavior and DA2's command path are unchanged.

Reproduction must exercise the click path. Six direct BeginConversation calls
produce one line already; six creature_core DIALOGUE events produced six lines
within 600 ms on the baseline. The regression test uses player talk commands and
checks DIALOGUE dispatch before, during, and after ambient speech, and preserves
the existing DA2 behavior. A direct script-injected DIALOGUE event bypasses the
player command guard by design.
