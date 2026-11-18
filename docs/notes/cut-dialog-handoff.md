# Orzammar Commons cutscene and dialog handoff (#1111)

The F8 journal records a player skip at world time 3918332 ms. The tick
inputs at that frozen world time total 10231 ms before the skip. The first tick
can contribute only its time remaining after the cut starts. This places the
reported skip during the second speech track. The log records the opening
voice, the Conversation screen, an automatic empty reply 3 ms later, and the
second voice. Those are separate from the player's skip command.

## Installed data and scripts

Node 17 of `bdc140_duncan.dlg` uses TLK 41525 and a CUTR (field 12211,
child field 12210) pointing to `bdc100ar_outofberahtdungeon.cut`. Its only
reply, 97, resolves to empty text and leads to node 18. Node 18 uses TLK
41526, has four substantive replies in the report, and has a 6.300024-second
inline CUT with its own ASLN for 41526 beginning at zero.

The 16-second CUTR contains explicit ASLNs for 41525 at 2.550–5.033 s and
41526 at 6.260–12.560 s. The first actor is the gate guard and the second is
the proving master. Both tracks name `bdc140_duncan` in fields 5562 and 5563,
and both specify 0 in fields 5564 and 5565. The English voice samples are
1968 ms and 5760 ms respectively.

The Commons area script has a plot-gated conversation start
(`bdc100ar_commons.ncs`, call at script offset 0x3d0). The CUTR names
`gen00cs_cutscene_end` in field 5001. Its `CS_CutsceneEnd` reads and clears the
module's `CUTSCENE_SET_PLOT`, `CUTSCENE_SET_PLOT_FLAG`, and
`CUTSCENE_TALK_SPEAKER` locals, and can set a plot flag and start a talk if those
locals were primed. It has no track-to-node walk. The two NPC nodes and their
empty reply have no action scripts.

## DAOrigins.exe trace

Addresses are virtual addresses in the GOG 1.05a executable (SHA-256
`2346caee611002d4dff6ecef68eb315b63a37004dd98b39c11a9c26c0b45bfb3`), from
static analysis. The cut update and the dialog graph have separate state.

- GameModeDialog installs the current node and replaces its old cinematic at
  0x68e3d9–0x68e43d. It obtains the new cut at 0x68e53c (`0x7f0960`) and
  starts it at 0x68e5a0. The authored cut length seeds the node timer at
  0x68e5ee–0x68e5ff for this node (dialog field 12216 is zero). When the timer
  expires, 0x68e210 calls 0x7f1a40 to prepare responses.
- ActionSpeakLine loads the explicit field 22 plus the voice fields at
  0x7dc172–0x7dc2dd. The timed update at 0x7cf840 uses a per-track entered
  latch at +0x60. It tries to start the authored voice and sets the active
  flag at +0x61 on success. That increments cut +0xe8 at 0x7cfa05. On play-out
  the track can remember a running sound at +0x62 and later clears +0x61 and
  decrements +0xe8 at 0x7cfc07–0x7cfc11. The callback at 0x7cfc69 calls
  `Conversation.OnNPCLineFinished` in the movie (0x68cc60). The native ASLN
  path writes subtitle text and signals the movie. In `conversation.gfx`,
  `OnNPCLineFinished` can schedule a delayed subtitle update when responses
  exist. It has no reply-selection call. `UpdateMainText` takes the text and
  consults subtitles and VO settings. Neither callback records a consumed
  dialog node.
- Cut completion (0x7cd290) tests terminal state 6 or timeline-end +0x18.
  If CUT field 5018 is enabled (loaded into +0x11c at 0x7e210a), timeline-end
  also requires +0xe8 to be zero. That field is enabled in this CUTR. +0xe8
  counts active voices. OpenThedas's `cut_lines_left` counts extra graph
  entries inferred from textless references, a different quantity.
- The dialog skip callback at 0x68dc10 checks its guard and current cut state
  before calling 0x7cf2e0 at 0x68dca8. The track factory maps kind 47 to
  `ActionSkipMarker` (constructor 0x7dcc60, vtable 0xb2355c). Its constructor
  inserts the track's time into cut +0x190. If a marker after the current time
  exists, 0x7cf2e0 stores it at +0x1b0 and the cut update seeks there, resetting
  actor tracks and +0xe8 at 0x7e430c–0x7e4380. This CUTR has zero kind-47 tracks.
  The no-marker branch sets cut +0x1b4 at 0x68dcce, prepares pending replies
  if needed, and takes the empty reply at 0x68dd89. The normal update also
  takes a lone empty response through 0x68ded1–0x68df57 once the cut completes.
  Both transitions call 0x68d8f0, which advances reply 97 to node 18 via
  0x7f1910 at 0x68dbca.
- The voiced/entered latches are local to the ASLN instance. The graph has
  separate show-once state written by 0x7f1640 (node history) and 0x7f1700
  (global TLK history). Both read field 12206, which is zero on nodes 17 and
  18. The traced paths allow node 18 to install its own ASLN instance after
  the external cut. Whether the original audio system suppresses a new
  voice there remains unverified. The skip and completion policy below is
  the OpenThedas behavior chosen for #1111.

## OpenThedas behavior

The original OpenThedas path used the foreground node's voice even while the
second track's subtitle was up. At handoff the UI took the opening empty
reply. Node 18 then started voice 41526 and presented the same line again.

For Origins, a playing CUTR can drive voice from its timeline if its first
explicit ASLN for the dialog matches the owning node's TLK ID. On close the
simulation follows completed matching lines across lone empty replies, using
ordinary reply actions and conditions. A real reply stays available for the
player. A line the cut never reached is presented normally. A line interrupted
by a skip keeps its active voice and presents the replies when the voice
finishes. A subtitle already shown in the cut is cleared at handoff. The
matching inline cinematic supplies its final shot and actor poses for the
reply wait. After natural completion both tracks have voiced in the cut and
the conversation opens on the replies.

The match is local to the cut and current graph walk. Textless ASLNs follow
the existing reference path. The tests build synthetic graphs and cut
timelines.

## Control-channel replays

Each replay loads the F8 report's `save_before.json` in a hidden window.
The audio log records a real event handle for each of TLK 41525 and 41526
exactly once per run. The movie has four live replies at node 18 in
each case, with no automatic `Conversation.SelectResponse` at handoff.

| Path | Track 41526 | Conversation at handoff |
| --- | --- | --- |
| Skip at 10.363 s | Starts in the cut and retains its event across the skip | Subtitle cleared. Replies show after the voice finishes. |
| Full 16 s playback | Starts and finishes in the cut | Subtitle cleared. Replies are ready. |
| Skip at 5.485 s | Starts once in the conversation | The unheard line is subtitled while it plays, followed by the replies. |
