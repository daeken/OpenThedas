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
honoring earlier bindings, repeated aliases and explicit `FOLLOWER:tag` forms. The stored actor
index remains the resource actor's index. Origins keeps its existing resolver.
The change adds no persistent state. Standalone cutscenes can use the bindings;
spoken-line performances also use them when integrated with the independent
named-stage placement repair.

This does not implement relative-stage context, processing suspension, camera
playback or full native walkmesh projection. Synthetic coverage exercises the
actual binder, prior named claims, arbitrary numbered labels, exhaustion,
explicit tags, non-party/reserve exclusion, actor indices, empty parties and
Origins isolation. On merged main 90f64bab, fmt, workspace all-target clippy
with warnings denied, workspace tests (1,207 passed, 17 ignored, zero failures)
and release MCP/CLI/app builds pass. Corpus, A/B sweep, repeat/reload and
installed-resource integration evidence are recorded after their runs finish.
