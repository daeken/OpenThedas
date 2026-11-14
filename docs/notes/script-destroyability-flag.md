# Origins destroyability flag (SCR-001)

Read-only static Origins 1.05 analysis. `IsDestroyable` and `SetDestroyable`
were unanswered engine functions; their script defaults hid a separate
object flag already retained by our integer-field loader.

The query handler `0x659a50` resolves CCObject through `0x5d5be0` and returns
the byte at `+0x2bc`, or zero when lookup fails. The setter `0x659ab0`
uses the same lookup and stores one for any nonzero integer, zero otherwise.
It calls no virtual setter or health, death, inventory or event routine.
Neither operation checks plot, immortal, dead or active state.

Construction initializes that byte to one at `0x699afd`. The base-object
reader reads label `IsDestroyable` with the existing byte as its default
at `0x69ae33`..`0x69ae67`, then normalizes the result to zero or one.
Our existing template and instance readers retain that integer label;
the query now reads it, defaulting to one when absent, and the setter
writes the normalized value. Object clones and JSON saves retain the
existing field map, so no new save member or migration is required.

The CCObject cast uses virtual `+0x18`. The established
[base-object trace](script-base-health.md) identifies the eligible
creature, placeable, item, trigger, sound, waypoint, projectile, store and
area-effect paths and the rejected module, area and party paths. Queries
on those rejected objects return zero and setters leave them alone, even
when a misleading field is present. Stage, map and map-pin casts remain
untraced and unanswered. DA2 retains its unanswered dispatch path.

This flag is distinct from the appearance-table predicate at `0x74d1b0`
used by a placeable's nonpositive-current handling. Implementing these
two flag accessors does not establish the still-missing placeable current
setter, destruction animation, collision changes or contents lifecycle.
