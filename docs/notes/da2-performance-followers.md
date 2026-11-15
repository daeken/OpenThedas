# DA2 performance follower bindings

DA2 performances name party actors with aliases such as `FOLLOWER 1` and
`FOLLOWER 2`. These previously resolved as ordinary object tags, so the active
companions played no world actors. The resolver is separate from named-stage
end placement: binding chooses who plays an actor; placement decides its pose.

## Native evidence

Native resolver 0x59bd80 recognizes the uppercase FOLLOWER prefix when the
binding is at least ten UTF-16 code units. It copies the module's active party
list from +0x2e8 at 0x59be96..0x59beab. For a colon at code-unit index eight,
0x59bed3..0x59bf6b compares the remaining tag with party members starting at
index one. A matching tag returns that member; it does not search reserves or
other world objects.

For the plain form, 0x59bfb4..0x59bff7 walks the party in order, skips the module
hero and skips ids in the performance's already-claimed set (0x4069a0). It
returns the first remaining member. It never parses the numeric suffix. Named
actors claimed earlier therefore affect which companion `FOLLOWER 1` takes.
The caller checks duplicate objects before taking another record.
Its name-to-object map at 0x5a0ec3..0x5a1016 reuses a binding already resolved:
two actors naming `FOLLOWER 1` do not consume two companions.

## Repair and limits

DA2 actor binding now resolves these aliases from the active party in order,
honoring earlier bindings, repeated aliases and explicit `FOLLOWER:tag` forms.
The stored actor index remains the resource actor's index. Origins keeps its resolver.
The change adds no persistent state. Standalone cutscenes can use the bindings;
spoken-line performances also use them when integrated with the independent
named-stage placement repair.

This does not implement relative-stage context, processing suspension, camera
playback or full native walkmesh projection. Synthetic coverage exercises the
actual binder, prior named claims, arbitrary numbered labels, exhaustion,
explicit tags, non-party/reserve exclusion, actor indices, empty parties and
Origins isolation. On merged main 7b377752, fmt, workspace all-target clippy
with warnings denied, workspace tests (1,220 passed, 17 ignored, zero failures)
and forced simulation/MCP/CLI release rebuilds pass. The DAO corpus has no archive,
read or typed failures; its existing 5,219 undeclared-action findings are unchanged.
The parsers are unchanged by the later main merges. The refreshed full s01–s13
A/B sweep has 26 successful runs, no story-log or plot-sequence differences,
15 identical complete checkpoints and no refused saves. Four Origins 600-tick
repeat/reload checks match. DA2 repeats match; reload differs only in
`next_effect_id` (2 versus 3), also on main. Matched main/candidate repeat and
reload checkpoints are identical. Frozen main d0bcc32c has the same core
sim/script/VFS/MCP source as 7b377752; the intervening changes are GFX and app work.
Later upstream changes are outside this frozen comparison.

The installed Karl performance is unchanged on this branch alone, since main
does not yet place spoken-line actors. Integrated with the separate named-stage
repair, identical replies finish at the same t=1323927 and place Bethany and
Varric on their authored end marks. A repeat produces identical complete JSON.
Matched captures use one renderer for both endpoints.

This changes combat starting positions. On the earlier d87f9ef2/90f64bab baseline,
the established conditional driver wins Karl's fight on main and placement alone,
but wipes with both repairs integrated at t=1414027. On refreshed frozen main
d0bcc32c, main and this branch alone both wipe at t=1488427 with byte-identical
complete end states; placement alone wipes at t=1483127. The refreshed integration
still wipes at t=1414027 and reproduces its earlier complete end state exactly.
Available level-ups/rest and a separate manual-input run did not clear the older
integration either. All runs have zero script errors and zero developer healing/kill aids.
The cause of the changed main-baseline outcome is unproven.

The earlier pre-Ogre replay still wipes on refreshed main and placement with
identical complete JSON at t=915150. The larger patrol ambush also remained
uncleared in the retained earlier-source runs. These checkpoint outcomes do not
establish fresh-run completion or prove a wipe's cause. Processing suspension,
command admission and locomotion state still need investigation; no stats,
damage, cooldowns or difficulty are tuned here.
