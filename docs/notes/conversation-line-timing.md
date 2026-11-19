# Origins conversation line timing

Static trace of GOG 1.05a DAOrigins.exe (SHA-256
`2346caee611002d4dff6ecef68eb315b63a37004dd98b39c11a9c26c0b45bfb3`).
Addresses are virtual addresses. The original executable was read without
running it.

## Setup and completion

- GameModeDialog resolves the current entry's TLK 12201 at
  0x68e326–0x68e343 → 0x7f00f0 and publishes the text at 0x68eb31 in
  the no-cut path. The VO event name is formatted at 0x68e89c → 0x7f2b70.
  The TLK's string records contain only an ID and text.
- With no cinematic, the timer at GameModeDialog +0x2c4 comes from the
  speaker's playback duration (0x68e94e–0x68e961). Its getter at 0xa5d4d0
  tries the face animation and the audio event, converts the latter's
  milliseconds to seconds, and returns zero when neither supplies a
  duration (0xa5d598). The authored No VO In Game flag (DLG 12216, read by
  0x68d8a0) bypasses VO start and seeds a 1 s timer instead (0xc01814).
- With a cinematic attached (0x68e53c), the timer comes from the authored
  cut length (CUT 5000, read at 0x68e5ee–0x68e5ff). The combination of
  No VO In Game and Play Until VO Completes (CUT 5018) uses the same 1 s
  timer for responses. Setup subtracts 0.3 s (0xc01810) and clamps at zero in
  0x68ebfc–0x68ec3d. Expiry prepares the reply list (0x68e210 → 0x7f1a40).
- When the last entry has text and no eligible replies, reply preparation
  adds a sentinel response with index -1 (0x7f1c58–0x7f1da2). An unstaged
  line with a voice handle automatically queues the lone empty or sentinel
  reply after playback stops (0x68df83–0x68e063). Without a handle it omits
  that automatic path. The text is presented and the continue action
  takes the reply on input. A textless entry with no cinematic is bypassed
  by the graph (0x7f1f6e–0x7f1fa3 and 0x7f1d1c–0x7f1e78).
- With a cinematic, GameModeDialog tests for the cut's completion
  (0x68de74 → 0x7cd290) before automatically taking the empty or sentinel
  reply. CUT 5018 makes that completion test also wait for active voice
  tracks to finish. The authored No VO flag on a terminal cinematic with
  CUT 5018 sets the manual-continue guard (0x7f1da7–0x7f1df3,
  0x690b97–0x690bb4, 0x68dee3). Real replies wait for selection.
- The movie's `UpdateMainText` uses subtitle settings and `Conversation.HasVO`.
  `ShowResponses(true)` enables the continue context when there are no
  choices, and the input callback `ContinueButtonAction` sends response 0.
  Its 5 s interval in `OnNPCLineFinished` concerns a deferred subtitle
  update when the movie has responses.
- Selecting a reply with no usable successor sets the termination flag
  immediately (0x68dbca–0x68dbd5 → 0x68d810). It bypasses the ordinary
  timer (0x68de22, 0x68e1c7). See [the exit-line trace](conversation-exit-line.md).

The No VO In Game and Play Until VO Completes labels match the
[BioWare toolset documentation](https://www.datoolset.net/wiki/Cutscene#Titles_and_text).
The old OpenThedas formula (`1500 + 55 ms × characters`, capped at 12 s)
was introduced as provisional in `1a0b1476`. The traced foreground
dialog paths use audio, authored cinematic time, or input.

## Report routes

The F8 journals and installed English data distinguish speaking a goodbye
from replaying a previously answered node. Both logged the voice attempt
before the final reply. The voice banks contain the two samples.

| Report | NPC entry | English sample | Inline CUT 5000 | Empty exit reply |
| --- | --- | ---: | ---: | --- |
| #1033 | Kasch node 6, 41009 | 2.688 s | 2.933331 s | node 33, 41038 |
| #1036 | Bartender node 11, 41220 | 4.128 s | 4.383331 s | node 35, 41237 |

Field 12216 is clear and CUT 5018 is set on both lines. The original
presents each authored, voiced line before it takes the empty exit reply.
The exit reply returns control immediately.

OpenThedas uses the sample's timing for ordinary voiced lines and uses the
inline duration to prepare cinematic replies. The automatic path preserves
both the inline cut and the playing voice. This also lets the voice finish
for inline cuts whose CUT 5018 flag is clear, where the executable can
advance at the end of the cut. The existing referenced-cut handoff and
explicit skip still apply. Unknown voice lengths and the relation between
the original voice handle and a missing sample remain provisional.
With the audio backend disabled, an inline cut still supplies authored
timing and an unstaged line follows the silent path. The DA2 text fallback
is provisional.
