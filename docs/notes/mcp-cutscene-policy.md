# Keeping cutscenes in MCP sessions and QA saves

`load` accepts `cutscenes: "play"` or `"skip"` (default). `load_state` accepts
the same option for both in-memory slots and JSON saves. Omitting it on restore
keeps the current session policy. Unknown values are rejected, and the tool schema
lists both choices. `status.cutscenes` reports the selected policy;
`status.cutscene` reports the active resource, elapsed milliseconds, length and
whether a conversation started it (or `null` if none is playing).

The policy belongs to the session, rather than the save. The active cutscene,
including its elapsed time, does survive a save. Set `cutscenes: "play"` before
the scripts that start the scene run. A save made after skipping it cannot recover
the walk-in just by changing policy. An already playing scene continues until its
end; selecting `skip` controls subsequent scenes rather than ending the current one.

## Dwarf Commoner: Beraht's walk-in

This recipe uses the game's character-generation events. Skip the boot intro to
reach manual character creation, then enable playing before entering the origin:

1. `load({"chargen":"manual"})`
2. `save_state({"slot":"chargen"})`
3. `load_state({"slot":"chargen","cutscenes":"play"})`
4. Send `chargen_event` for `CHARGEN_SELECT_GENDER` with `ints:[1,1]`,
   `CHARGEN_SELECT_RACE` with `[1,1]` (dwarf), `CHARGEN_SELECT_CLASS` with
   `[1,1]` (warrior), and `CHARGEN_SELECT_BACKGROUND` with `[2,1]` (commoner).
5. `chargen_event({"event":"CHARGEN_END","ints":[0,2]})`
6. Advance `step({"ticks":1,"tick_ms":100})`, checking `status` until
   `cutscene.name` is `39366.cut`. The origin's `bdccs_intro.cut` plays first.
7. `save_state({"path":"beraht-walk-in.json"})`

On the tested install the line begins after about 57 seconds of cutscene ticks.
Polling the active resource avoids depending on that timing. The resulting save
keeps Beraht's opening line and walk, Rica seated, and the wide cutscene camera.
Load it with `load_state({"path":"beraht-walk-in.json","cutscenes":"play"})`
or render it using `thedas-app --save beraht-walk-in.json`.

To keep the boot intro as well, use `load({"chargen":"manual","cutscenes":"play"})`
and advance through it before sending character-generation events.
