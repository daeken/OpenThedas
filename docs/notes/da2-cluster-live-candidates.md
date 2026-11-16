# DA2 live cluster candidate indexes

The native `GetClusterCenter` wrapper (`0x51b160 -> 0x51a4b0`) constructs
its candidate vector from the ordered perception map at creature `+0xb4c`.
At `0x51a670..0x51a69d`, each entry is resolved as a creature; an absent
creature or nonzero dead predicate is skipped before the vector append.
The dead virtual at `+0x1d8` is `0x52e810`, reading the DA2 death byte.

The simulation already uses this death predicate, but kept a dead entry in
the candidate vector with an exclusion flag. Its normal perception refresh
removes dead actors; this construction still matters for retained entries
between refreshes and for restored perception lists. That changes indexes used by
the clustering algorithm. Its final best-index test intentionally excludes
index zero: a corpse followed by a sole living hostile made that hostile
selectable, although native construction would give it index zero and
return an invalid location.

DA2 now discards dead candidates during construction. The algorithm's live
order, distance cache, first-index exclusion, ally scoring and random draws
are unchanged. Origins keeps its independently traced health-based predicate
and existing construction path.

The script-facing regression exercises the failure with a dead first entry
and a sole living hostile, then makes the first entry live as a control and
obtains the second creature's location. Existing coverage retains dead/health
cross-products for both games, native DA2 span metadata and upgrades.

Matched story measurements and Origins regression results follow after the
merged candidate completes its checks.

No HP, damage or difficulty values are retuned.
