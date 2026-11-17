# Conversation timing, terminal voice, and reply refresh (#1110, #1111, #1095)

The report pin is `81af4d7247aa73e75fc8a461f0610e96bc9c3acc`. The implementation
starts from `d6b15225` (including the already landed CUTR cleanup), merged with
`origin/main` before validation. Replays use the loopback control channel, hidden
windows, a scratch home, and `--audio-wav` for silent audio timing. No original-game
frames or game assets are included in this change.

Verified fixes: **#1110 and #1095**. **#1111 remains** a cut/dialog handoff issue.

## Recorded evidence and classification

| Report | Recorded timing | Classification |
| --- | --- | --- |
| #1110 | Talk order 3044.172; Conversation up 3045.043; voice **attempt** 3045.047; SelectResponse 0 at 3045.108 | Empty, unvoiced opening node automatically continues. The voice-start log alone does not establish that a sample played. |
| #1111 | Voice attempt 3075.989; Conversation up 3087.487; SelectResponse 0 at 3087.490 | The completed/skipped owned cut releases the first line's automatic continue. A response 3 ms after movie construction does not establish carried pointer input. |
| #1095 | Final line starts 449.654; the report has node 12 / reply 39 waiting | The presentation snapshot has ResponsesReady=1. The sole reply's text changed while the observed list count stayed one. |

### #1110

The opening node is 274105, followed by empty reply 7 and terminal NPC node
274106. The installed English voice bank has no 274105 sample. It does have a
274106 sample lasting **5,808 ms**. The original `voice starts` message is logged
before sample lookup, so the 274105 log is an attempt, not a voice being cut off.

`UiLayer::play` -> `Line::played` -> `continue_conversation` produces the first
SelectResponse for the empty opening. The audible defect is later:
`voice_line` used `Sim::conversation_line`, which excludes ended graphs. The graph
has already ended on 274106, so audio never received that voiced final line;
the UI instead held its text for an unvoiced reading time.

The recorded second opening has another explicit talk order at 3051.723, after
the first one was dismissed. Both report saves have an empty pending-talk list.
Do not label that order as an autonomous stale-trigger reopen. Existing main's
`end_presented_line` also removes a line's queued/playing CUTR on cleanup.

### #1111

The 16-second owned cut has explicit tracks for 41525 at 2.55–5.033331 seconds
and 41526 at 6.26–12.560024 seconds. Their individual voice samples last 1,968 ms
and 5,760 ms. The conversation remains on node 17 with `cut_lines_left=0` while
these tracks play. `dialog_refs` deliberately excludes tracks with explicit TLK
text. The post-cut automatic reply advances to node 18, corresponding to the
second track.

This cut/dialog handoff needs separate verification of how the spoken explicit
tracks advance the dialog and suppress replay. Pointer protection and voice timing
must not be presented as proof that this duplicate is fixed.

### #1095

The final Unna node has a single real reply after lines with a single empty
continue. `Conversation.Responses` carries a count. The movie observes that root
and loads reply text from its callback; changing only `.0.Text` does not change
the count. A ready flag of one cannot display a real reply if the movie still
holds the previous empty continue.

#1084 retains cinematic cameras during reply waits; it did not implement the
response-ready gate. The camera is retained here.

## Changes

- Expose the still-presented terminal NPC line to audio without changing which
  conversations are active in the simulation.
- An expected voice with unknown duration cannot fall through to the empty-text
  reading timer. Measure known voice duration from when audio reports its start,
  excluding time spent held behind loading. Explicit skips and completed cuts
  retain their behavior.
- Bind readiness and voice flags before constructing a conversation movie.
- Refresh the movie's response-list observer on a new line even when the count
  remains unchanged.
- Reject releases/repeated presses inherited from an earlier screen or line.
  A subsequent fresh click works normally, including after a talk order.
- Scope deferred automatic continues to the line that requested them and clear
  them when the presentation is replaced. Drop premature movie responses and
  accept at most one response per frame, so duplicate callbacks cannot answer
  the next line.

## Validation

- `cargo fmt --all --check`: pass.
- `cargo clippy --workspace --all-targets -j 2 -- -D warnings`: pass.
- `cargo test --workspace -j 2 -- --test-threads=2`: 1487 passed, 26 ignored,
  zero failures across 31 test suites (including doc-test suites).
- `OpenThedas-testdata/ui/regress/run.py APP 8047`: **15/15 passed**.
- Control-channel pointer checks: **2/2 passed**; a press begun before the
  conversation cannot release into it, and the next fresh click dismisses once.

The candidate starts terminal voice 274106 with a real audio event and duration
5808 ms, then continues at **5808.07 ms**. It returns to the HUD and stays there
through the subsequent wait. Unna's movie reports `HasResponses=true` and caches
the goodbye text without any forced refresh calls; the screenshot shows the
choice, and its selection returns to the HUD. #1111 retains its post-cut replay
path and is not claimed fixed.

The job capture folder contains request/response JSONL, logs, a reusable replay
script, and labelled before/after/delta comparisons. The simulation change only
adds a read-only presentation query; active-dialog and command semantics are
preserved. Unit tests cover unknown voice
duration, delayed voice start, reading-time boundaries, owned cuts, explicit skip,
terminal voice visibility, duplicate/premature responses, and pointer carry across
opening/replacing a screen.

### Optimized diagnostic observations

The instrumented pre-fix build reports #1110's opening at `t=100 ms`,
`voice=None`, `voiced=false`, with no click callback. Its terminal text is
continued again at `t=5681.94 ms`, still unvoiced. The original voice bank's
terminal sample is 5808 ms; it was never requested by this pre-fix audio path.
The diagnostic shop checkpoint was derived by the original pinned replay from
#1110's `save_before.json` through the hideout exit and the talk order.

For #1111, the controlled cut skip produces automatic continuation at
`t=17780.35 ms`, `voice=1968 ms`, `cut_done=true`, `owns_cut=true`. No
`UiLayer::clicked` callback runs at handoff. The cutscene key-skip handler returns
before recording that key in `input.txt`; the journal's player `SkipCutscene`
command is the stronger evidence of a skip in the original report.

At Unna node 12, control probes report ready=1, count=1,
`HasResponses=false`, and the movie's cached first response text is empty.
Calling `UpdateResponses` and `ShowResponses(true)` shows the goodbye; selecting
it returns to the HUD. This isolates stale response data from cinematic-camera
retention and from line completion.
