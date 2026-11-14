# Origins placeable maximum-health setter (SCR-026)

Read-only static Origins 1.05 analysis. This slice covers only the
placeable `SetMaxHealth` virtual; it does not establish the full placeable
health-loading or death lifecycle.

The script handler `0x65a330` resolves CCObject through `0x5d5be0`,
sign-extends the low 16 argument bits at `0x65a378` and calls virtual
`+0x12c`. CCPlaceable's vtable `0xb1c084` selects `0x750f70`. That method
stores the supplied integer directly in member `+0x670` and returns.
The neighboring maximum getter `0x750f60` reads the same member. Current
health has its own getter `0x750f50` and member `+0x66c`. The maximum
setter neither assigns nor clamps current, invokes a property setter,
checks immortal/plot/dead flags, nor schedules a death notification.

The initializer `0x74ea90` zeros both members at `0x74eec4`/`0x74eeca`.
Copy construction at `0x752489`..`0x75249b` copies them independently.
These observations support their independence; full template and native
save serialization remain unverified and are not changed here.

OpenThedas already reads the `MaxHealth` integer field for a placeable's
maximum query. The setter now writes that field after signed-short
narrowing and preserves `Health`, any provisional property 7, modifiers,
flags and pending events. Our existing object cloning and JSON saves
retain the field. This corrects the old write to property 7's base and
conditional reset of its current value, which did not update the queried
maximum at all. For example, a maximum of 77 and current of 80 followed
by `SetMaxHealth(chest, 30)` now yields maximum 30 and current 80.

The remaining maximum getter fallback, full template/native-save mapping,
`SetPlaceableHealth`, death handling, stage/map/map-pin paths and DA2
stay under SCR-026. This change does not infer any of their behavior.

The s01–s16 gate has an intentional saved-state delta. Placeable startup
reads its appearance's health setting and invokes `SetMaxHealth`; that call
does not initialize native current. In 19 of the 35 gate saves, 4,765
placeable occurrences now retain `MaxHealth=100` instead of zero or one and no
longer have the incorrectly created property pair `(100, 100)`. Their
independent `Health` fields remain unchanged (1, 3, 5, 20 or 50 in this
sweep). Reader `0x750370` loads that field as a signed short through current
virtual `+0x124` before loading maximum through `+0x12c`. For these positive
currents, `0x74e8e0` stores directly; getter `0x750f50` reads that member.
`GetHealth` now uses the independently loaded field when no explicit
property current exists, so it reports the template's current rather than
the invented 100. Existing explicit currents from our older saves keep
precedence and remain untouched by a maximum write.

These 9,530 field changes are the only saved differences. Complete story
logs, engine logs, ordered plots, ending areas, creature health observations
and developer-heal results remain equal. This does not complete the missing
explicit placeable current setter or its nonpositive destruction lifecycle.

## Compatibility and destruction review

This change introduces no required JSON members or save revision. The old
explicit property-7 current keeps precedence, including zero; missing
`Health` reads as zero. A regression loads all four combinations of missing
and present `Health`/`MaxHealth`, with absent, zero and positive explicit
currents, then saves and reloads each without changing either field map.

The separate bashing requirement is not established by this maximum setter.
A private compiled-script lethal-damage probe against main and this change
leaves both an ordinary door and a chest unchanged, with or without an
explicit current. `SetPlaceableHealth` is still a presentation no-op on
both branches. Its native handler (`0x65a250`) calls the current virtual
`+0x124` after signed-short narrowing. The placeable method (`0x74e8e0`)
has additional nonpositive-current inventory and destruction side effects;
these need their own traced implementation and end-to-end tests. The
maximum setter must not stand in for that missing current setter.
