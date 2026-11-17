# Origins map ally flag and placeable popup text

`ShowAsAllyOnMap` (364, handler 0x6665c0) resolves only creatures and writes
the boolean protected attribute at creature +0xcb4. Any nonzero argument enables
it. Creature construction initializes this attribute false (0x6ba6e6). This does
not change team, group, party membership, or hostility. Sim stores the flag in
the object's integer fields and exposes `show_as_ally_on_map`. The existing map
bindings only publish map notes; creature marker presentation remains separate.

`GetPlaceablePopupText` (356, handler 0x65f2e0) resolves only placeables and
returns the localized string at +0x5b8 through 0x4d1eb0. Invalid objects return
an empty string. The localized-string resolver 0x4d1d50 first looks up its talk
table reference and keeps its existing text if that lookup fails. Sim reads
`LocPopupText` from the template and then explicit instance overrides, preserving
an explicit empty value. It resolves the talk table first, then embedded English
male text. Sim's current English-only presentation remains a limitation; other
embedded languages, token expansion, and gender-specific tables are not added.

Both script handlers are Origins-only. Reading the extra placeable metadata does
not change DA2 dispatch. Stored values round-trip through existing object fields.
