# Origins team queries (SCR-027)

Static trace of the Origins executable. The descriptions below are our own prose;
the original game was not executed. DA2's query remains provisional and unchanged.

GetTeam (0x66e570) reads the team id and defaults the member-type bucket to 6,
the creature bucket. When a type argument is supplied, it computes the integer
part of log(type) / log(2), then adds one. The argument therefore selects a
single object-kind bucket. It is not a bitmask union: 512 and 544 both select the
placeable bucket, while an omitted argument selects creatures. Zero and negative
type values cannot select a valid object-kind bucket.

The invalid team id is -1; the handler returns an empty object array for it.
For other ids, it obtains the current module through 0x5d4fc0 and its current
area through 0x5fe470. No module or area also gives an empty array. The caller's
area does not choose the registry.

With an area, 0x66e3d0 obtains that area's team entry through 0x6863c0, then
looks up the one member-type bucket through 0x5d0ab0. This is an area-owned
registry lookup, not a scan of all areas. The handler applies no active, living,
hostility, party-membership or perception filter. The resulting registry array
is copied through 0xa2e490. Inactive and dead members therefore remain visible
to the query; the same team number in a different area does not contribute.

OpenThedas already filtered by the loaded area. The changes keep that behavior,
make the missing-area case empty instead of selecting area-less objects, reject
the invalid team id, and match the single-bucket type selection. Existing id
ordering is retained. Tests exercise the default, inactive/dead members, caller
isolation, switching the loaded area, absent teams/areas, explicit invalid types,
multi-bit types and the preserved DA2 behavior.
