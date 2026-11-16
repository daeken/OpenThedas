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

## Matched measurements

The supplied fresh3 real-arrival checkpoint and driver are replayed with idle
Hawke attack orders. Baseline main `c609d2b7` produces exactly the same complete
206,496-entry trace as `249769ac` and the requested `d87590e5` control. The
standalone repair clears the first real fight in 25.7 seconds, compared with
29.8 seconds on main. In the second encounter Bethany's Fireball produces three
damage events totalling 104.32791. Main produces zero Fireball damage events.
The second encounter still wipes, after 28.8 seconds with the repair and
48.6 seconds on main. The repaired control leaves five initial enemies alive.
The control's target selection and elapsed progression differ.

The frozen fake-wave probe clears the initial six in 147.200 seconds on main
and 146.784 seconds with the repair. Mean Hawke basic health loss is 5.589890
over 87 events and 7.352569 over 53 events. Each initial target loses 100 HP.
This historical checkpoint contains weapon power 5. Complete probe calls also
advance later waves and last 148.4 and 427.5 seconds respectively.

A separate same-save control issues one identical ordinary Hawke attack and
advances exactly ten seconds. Bethany deals 45.31219 damage in four events on
main and 150.71920 in seven events with the repair. Both complete controls
wipe. The paired states are captured at zero time with the same retained
renderer, fixed camera and crop. Delta multiplied by four changes 4.71673% of
source-crop pixels above an any-channel threshold of 3/255.

All controls have zero script errors, developer heals and forced kills. The
supplied driver's health-item helper attempts Origins tags and consumes zero
DA2 potions in these matched controls.

## Validation

Formatting and strict workspace/all-target clippy pass. Workspace tests report
1,377 passed, 22 ignored and zero failures. Baseline and candidate release MCP
builds pass. Five independent Origins fixture scenarios have identical logs,
plot sequences and six produced checkpoint JSONs. Joining and Lothering's board
complete. Tower's five seeds wipe on level one on both builds. Four 600-tick
Origins determinism controls pass repeat and save/reload comparisons.

Extra 100-tick DA2 runs repeat identically on both builds. Reload increments the
next-effect counter by one and changes one effect ID on main or two on the
candidate. All other world fields under the standard bookkeeping normalization
match. This effect-allocation difference is retained in the external evidence.

## Integration control

A local control combines this repair with the separately reviewed waiting-queue
repair. Its source is `3832b54a`. Both cluster regressions and the queue
regression pass on that combination. The release MCP build passes. From the
same real-arrival input, the first fight clears in 17.7 seconds. The second
fight wipes after 43.1 seconds. Carver deals 240.92151 damage in 29 events and
Bethany's Fireball deals 254.87286 in seven events. The frozen fake wave clears
its initial six in 49.519 seconds, with mean Hawke basic health loss 5.963279
over 86 events.

An additional harness control corrects the health-potion tag to the DA2 item
already carried by Hawke. He consumes one owned health potion through the
ordinary item-use API. That second encounter still wipes, after 51.5 seconds,
with zero developer heals and forced kills. These controls leave the remaining
wipe reproducible with ordinary basic-attack input.
