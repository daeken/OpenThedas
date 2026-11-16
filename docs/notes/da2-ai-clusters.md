# DA2 ability spans for AI cluster targeting

Bethany's authored Fireball tactic calls `GetClusterCenter(438, 301030, 2, 0, 0)`.
In the supplied real-prologue encounter it repeatedly receives an invalid
location despite knowing the ability and perceiving nearby enemies. The
simulation reads Origins' `ABI_base` columns `aoe_type`, `aoe_param1` and
`0x5d3e0787`. These columns are absent from the DA2 ability rows, giving a zero
cluster span.

## Native span lookup

Read-only tracing of the DA2 analysis image finds command 445 registered at
`0x526dc2` to `0x51b160`. Its wrapper calls `0x51a4b0`. Span initialization at
`0x51a588..0x51a5ff` follows the ability's upgradable metadata.

- Ability 302030 uses twice its upgradable float column `0x15bdf19c`.
- Other abilities read the persistent-area row through upgradable integer
  column `0x53a8a258`. A zero row leaves the span zero.
- A nonzero row reads radius `0xad5fb9f8` and shape `0x4de70e62` from table 96.
  Shape 1 doubles the radius. Other shapes use the radius directly.

Installed Fireball 301030 points to persistent row 301030, radius 2.5 and shape
1. Its native cluster span is therefore 5.0 metres. The lookup uses the existing
upgradable-ability helpers, including learned direct upgrades and the column's
row-zero marker.

The function also reads an ability range class and a table-301 value into a
local which its clustering loop never uses. The candidate-order distance
cache, ally scoring, random tie handling, first-candidate exclusion and mean
hostile position follow the existing clustering implementation. The simulation
uses its perception list and DA2 dead flag for candidate eligibility. The
native perception query and its virtual eligibility predicate need further
tracing.

## Regression coverage

The regression calls the script-facing getter with conflicting Origins
metadata. It covers sphere doubling, another shape, zero and missing rows,
upgradable persistent-row selection, ability 302030's upgradable float,
save/reload and missing tables. The existing cluster-selection regression also
supplies native DA2 shape data for its shared candidate-eligibility checks.
