# Failed paths and endpoint adjustment (SIM-012)

Static DAOrigins.exe trace, 2026-11-15, extending the external
`OpenThedas-testdata/notes/path-follower-trace.md`. Image SHA-256:
`2346caee611002d4dff6ecef68eb315b63a37004dd98b39c11a9c26c0b45bfb3`.
Addresses are image VAs. This note contains original prose only. It establishes a
replacement contract; it does not change simulator behavior or establish DA2 behavior.

## Order of operations

The common move executor calls **0x73ea80**. Let P be the creature's path context
(getter **0x6a2570**). The initial request uses the creature position, or an explicitly
supplied start, and the requested goal (**0x73ebf9–0x73ec44**). A continuing request
can instead retain P's saved start, goal and direction (**0x73eb8a–0x73ebf4**).

For initial requests, endpoint adjustment precedes both direct reachability and search:

1. Test the goal with **0xa62a90**. If invalid, call **0x73e7f0** with the direction
   from goal toward start, a **2 m** limit, check argument zero, and a separate output
   point (**0x73ec4e–0x73ecb8**). The caller does not branch on its return value.
2. Test the start with **0xa62a90**. If invalid, first try **0x73df70** toward the
   original goal, limited to **5 m** (**0x73ecbe–0x73ecfa**). Success sets P+8 to one.
   Otherwise call **0x73e7f0**, also limited to 5 m. A nonzero return from this second
   helper moves the creature to its output through **0x695540**
   (**0x73ed04–0x73ed32**).
3. Install the resulting endpoints with **0xa62990 / 0xa629c0**. Try direct pathing
   reachability through **0xa626f0**. Success builds a two-point route and returns
   status 1 (**0x73ed37–0x73ede5**).
4. Otherwise call the incremental planner **0xa62640**. Statuses **1, 2, 3, 4, 6 and
   7** return without the fallback (**0x73ee50–0x73ee82**). Do not treat every status
   other than 1 as a failed search.
5. Only the remaining outcomes reach the height and ground-segment fallback below.

Consequently the fallback's start/goal height comparison is on the locally retained or
adjusted endpoints, not necessarily the coordinates originally submitted by a script.
The simulator's nearest-walkable-cell search is not an equivalent endpoint adjustment.

## Directional goal adjustment: 0x73e7f0

This helper takes a point, direction, maximum distance, path context, an argument passed
on to the final safe search, and an output point. A missing context or output returns
zero. Let u be the direction normalized in XY and h be half the pathing-cell spacing
(**0x73e825–0x73e851**, constant **0xae70f8 = 0.5**).

It first tries `point + h*u`, obtaining height through **0x73d520** and requiring
**0x73de70** to return exactly **1** (**0x73e857–0x73e8b1**). With positive h, failed
attempts continue in rows perpendicular to u:

- Advance the row distance r by h, starting at h, through the supplied maximum.
- At each row, convert `(r + 0.01)/h` to an integer n, then try k from **−n to +n**.
- The horizontal candidate is `point + r*u + k*h*(u.y, -u.x, 0)`; its height comes
  from **0x73de20**. Again require **0x73de70 == 1**
  (**0x73e8e1–0x73ea10**).
- A successful row candidate gets its final height through **0x685430** and is copied
  to the output (**0x73ea4c–0x73ea77**). The initial single-point success copies its
  already resolved height directly.

If these rows fail, call the ordinary **0x73e370** safe-location search at the
original point, with the same maximum distance, the supplied context, snap on,
minimum radius zero, and the caller's argument (zero for these path requests)
(**0x73ea22–0x73ea49**). This final search is not the first operation, and its square
rings do not replace the directional rows. No extra direct-segment test appears in the
row-candidate loop itself.

## Start adjustment: 0x73df70

The first start-repair helper is different. It normalizes the horizontal direction,
uses half-cell spacing, and searches increasing radii from the smaller of that spacing
and the supplied maximum (**0x73df99–0x73e05f**). Its angular increment is an integer
conversion of `360*h/(8*r)`, clamped to **5–120 degrees**
(**0x73e05f–0x73e0ad**). It consumes the native random generator to choose an initial
angle offset **rand%60 − 45 degrees** (**0x73e0ad–0x73e0fd**).

At each radius it tries positive angular offsets below 180 degrees; after offsets
greater than 5 degrees it also tries the corresponding negative offset. A candidate
must pass **0x73de70** with a nonzero result, get its height from **0x73de20**, and
pass **0x67cfa0** from the floor-resolved original start with flag 1. The winning
candidate's final height is resolved through **0x685430**
(**0x73e127–0x73e34f**). The random starting angle, mirrored candidate order, and
different success comparison matter; deterministic nearest-cell repair would be a
different algorithm and would also omit an RNG draw.

## Flat failed-search fallback: 0x685920

**0x73ee88–0x73eead** rejects the fallback when the absolute endpoint-height difference
exceeds **0.5 m** (float **0xb1b7b4**). Otherwise **0x73eeb3–0x73eeff** calls
**0x685920**, passing the endpoints, the width float from `[P+0x10]+4`, a height
argument of **0.5 m**, and a query-result record.

For horizontal unit direction u and that width w, **0x685920** forms the transverse
offset `s = (w/2)*(-u.y, u.x, 0)` (**0x685933–0x6859c2**; the divisor at
**0xae1998 is 2.0**). It calls **0x685790** in this order, stopping at the first
result other than 1:

1. start → goal;
2. start+s → goal+s;
3. start−s → goal−s.

These are three ground-aware segment tests, not a swept capsule and not the pathing
visibility query **0xa4bb80**. The half-width distinction is explicit in the arithmetic;
the semantic name and producer of w still deserve a separate audit.

**0x685790** resolves ground height through **0x685430**, uses the supplied height
argument to lift the query and accommodate an upward height difference with
**0x67c950**, then calls **0x6851d0** (**0x6857ec–0x685895**). That helper queries
the area's physics object with mask **0x24**, retaining first/last intersection
information and an optional result record (**0x685214–0x6853cb**). Hits return false;
no intersections, including an absent query object, return true
(**0x6853d1–0x68541e**). The caller returns −1 on a rejected query and otherwise
checks endpoint progress in XY with approximately **1e−6** tolerance before succeeding
or continuing (**0x68589a–0x685919**). Its height and continuation state must be
preserved by any port; three ordinary sight rays are not the recovered contract.

Only **0x685920 == 1** rebuilds a valid two-point route and returns status 1.
Anything else becomes status **5** (**0x73ef04–0x73ef5e**). Status 5 stops the path
context (**0x73efae–0x73efb9**); the ordinary executor maps path failures 5/8 to
`COMMAND_FAILED_INVALID_PATH` (**−4**) through **0x7a9880 / 0x7a3190**.

## Implementation boundary

OpenThedas still has a provisional grid A* and an unrestricted failed-search fallback.
The separate height-guard PR [#915](https://github.com/daeken/OpenThedas/pull/915)
addresses only tall failed routes. It does not implement these endpoint repairs or the
flat ground-segment query.

The flat segment mask 0x24 matches the simulator's sight mask. However, its ground-height
helper **0x685430** also has a **0x2000000** query branch
(**0x685554–0x6855af**), and the current loader retains only sight-mask shapes.
Replacing this fallback requires the ground-height queries, the height
and progress behavior above, endpoint repair in native order, and explicit planner
statuses. Keep missing navigation data separate from a known failed search. Neither
`wall_between`, `walkable_reach`, nor three calls to `sight_blocked` supplies that
contract. No movement change is made by this trace.
