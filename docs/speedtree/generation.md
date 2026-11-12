# SpeedTree 4 tree generation

How the trees, bushes and grass of Dragon Age: Origins' outdoor levels are grown from their
`.spt` descriptions at load time. The game links SpeedTree 4.x's runtime statically; this is a
behavioural description of what it computes, written in our own terms, for a clean-room
implementation. It names no types, functions or files of any SpeedTree source. Facts checked
in DAOrigins.exe say so; the rest is to be verified against the executable where practical.

See [format.md](format.md) for the file format and how levels place trees, and
[rendering.md](rendering.md) (later stage) for how the game draws them.

Stages, each implementable and reviewable on its own:

1. Random numbers and curves (below).
2. Tree set-up and the trunk/branch skeleton.
3. Branch geometry (cross sections, texture coordinates, normals, flares, roots, LOD).
4. Fronds.
5. Leaves (placement, cards, meshes, colours, LOD).
6. Bushes, grass and cattails.
7. Rendering: vertex layouts for the game's `spt_*` shaders, billboards, LOD, wind.

## Stage 1 — Random numbers and curves

### 1.1 The single random stream

Every random number in generation comes from **one process-wide generator** with one shared
state. There are several "random" objects in the engine (one per branch object, one inside
each curve's variance routine, one for leaves, …) but they are all thin views of the same
global state: drawing from any of them advances the same sequence. Consequently the exact
**order of every draw** matters, and later stages state the order of draws explicitly.

The generator is a Park–Miller "minimal standard" Lehmer generator (multiplier 16807, modulus
2³¹ − 1, Schrage's decomposition with q = 127773, r = 2836) feeding a 128-entry shuffle table
(Bays–Durham style), returning single-precision values.

State: a seed (held as an integer value, stored in a double in the original but always
integral) and a table `T[0..127]` of floats.

**Raw step** (produces a float in (0, 1)):

```
hi   = seed / 127773            (integer division, truncating)
lo   = seed − hi · 127773
seed = 16807 · lo − 2836 · hi
if seed ≤ 0: seed += 2147483647
return (float)seed · 4.656612875e-10      (computed in float: the seed is first
                                            converted to float, then multiplied)
```

**Seeding with an integer s** (the form generation uses): if s ≤ 1 use 1; set seed = s; then
fill `T[0]..T[127]` in order, each with one raw step.

**Next value** (what every "uniform" draw uses):

```
i = (int)(raw_step() · 128)        (0 ≤ i < 128; truncation)
f = T[i]
T[i] = raw_step()
return f
```

**Uniform in [a, b]**: `a + (b − a) · next()` (float arithmetic).

(There is also a time-based reseed used only when no seed is given; generation always seeds
explicitly, so it can be ignored.)

When the tree is computed, the generator is seeded with the tree's seed (the `.spt` tree
seed; `TreeInfo::seed` in `thedas_formats::speedtree`) before any other draw; stage 2 gives the exact point.

### 1.2 Curves ("BezierSpline" parameters)

Most per-level parameters in a `.spt` are stored as a text string of this form:

```
BezierSpline <min> <max> <variance>
{
  <n>
  <x> <y> <tx> <ty> <tlen>      (n times: control point, tangent direction, tangent length)
}
```

`thedas_formats::speedtree` already reads these into `BezierSpline { min, max, variance, points }`.
Numbers are parsed with a simple decimal reader: optional leading white space, optional sign,
integer digits, optional fraction, optional `e`/`E` exponent (applied as `· 10^exp`), all
in float precision. Matching that reader matters only in the last bit; using a standard float
parse is acceptable unless a mismatch shows.

**Building the curve.** Control points are in a unit square (x from 0 to 1 along the
parameter, y the normalized value). Tangent directions are normalized on input. Consecutive
control points P₍k₎ and P₍k+1₎ form one cubic Bézier segment with inner handles
`P₍k₎ + t̂₍k₎ · len₍k₎` and `P₍k+1₎ − t̂₍k+1₎ · len₍k+1₎`. A Bézier point at fraction u is
evaluated by repeated linear interpolation (de Casteljau), separately in x and y.

**Resampling to a table.** The curve is turned into a fixed table of N = 500 samples, and all
later evaluation reads only the table:

1. Raw samples: walk the segments with a running fraction u, starting at 0, step
   `(n − 1) / (N − 1)` where n is the number of control points. While on segment k and u < 1,
   evaluate the segment at u, store the (x, y) point, advance u by the step and count it; when
   u reaches or passes 1, move to the next segment and subtract 1 from u. Stop when segments
   run out or N raw points exist. If fewer than N were produced, append the last control
   point.
2. Table: entry 0 is the first control point; entry N−1 is the last control point. For each
   i from 1 to N−2 compute the target `p = i / N` (note: divided by N, not N − 1). Keep a
   cursor into the raw points that only moves forward across i: advance it until the raw
   point at the cursor has x ≤ p < x of the next raw point (stop advancing at the last pair if
   none matches). Then the table value is the y linearly interpolated between those two raw
   points at `(p − x₀)/(x₁ − x₀)`.

   Only the y values of the table are used later.

A curve with fewer than two control points builds no table (treat as all zeros unless a later
stage says otherwise).

**Evaluation at a parameter p ∈ [0, 1]** (table y values `Y[0..499]`):

```
k = (int)(p · 499)
if k == 499: y = Y[499]
else:        f = (p − k · (1/499)) / (1/499);  y = Y[k] + (Y[k+1] − Y[k]) · f
value = y · (max − min) + min
```

Three evaluation flavours exist; which one each use calls is given in later stages:

- **With variance (unscaled)**: value + uniform(−variance, +variance). One draw.
- **With variance, scaled**: value + y · uniform(−variance, +variance), where y is the
  normalized table value before the min/max mapping. One draw.
- **Without variance**: value only. No draw.

**Scaled variance alone** (used for "disturbance"-type parameters): read the table at
`k = (int)(p · 499 + 0.5)` (rounded, no interpolation), giving y; return
uniform(−variance · y, +variance · y). One draw. (Here the result is a pure random offset;
min/max are not involved.)

**min / max** of a curve are the stored min and max (not the curve's extremes).

### 1.3 Float details that matter

Generation is float (32-bit) throughout. Angles are in degrees at the parameter level and are
converted to radians where rotations are built (stage 2 gives each conversion). Integer
conversions are truncations toward zero unless stated.

---

## Stage 2 — Tree set-up and the branch skeleton

This stage grows the tree's skeleton: every branch's list of vertices, each with a position,
an orientation frame, a radius and a running length, and decides where children grow. Branch
geometry (stage 3), fronds (stage 4) and leaves (stage 5) hang off this skeleton. The order of
random draws below is exact and must be kept.

Field names in `code` refer to `thedas_formats::speedtree` (our parser).

### 2.1 Conventions

- **Axes.** Z is up. A branch frame's "outward" axis is X: a frame's growth direction is the
  vector (1, 0, 0) carried through the frame (see 2.2). "Down" is (0, 0, −1).
- **Angles** at the parameter level are in degrees. Sine and cosine of a degree angle a are
  `sin(a / 57.29578)` and `cos(a / 57.29578)` in float. Radians to degrees multiplies by
  57.29577951308232.
- **Interpolation** `lerp(a, b, t) = a + (b − a) · t`.
- **Angle between two unit vectors** is `acos(clamp(dot, −1, 1))` in radians.
- **Cross product** `u × v` is the usual right-handed one.

### 2.2 Rotation frames

A frame is a 3×3 matrix M (rows r₀, r₁, r₂) used two ways:

- **Direction of a frame**: the row vector (1, 0, 0) times M, i.e. M's first row r₀
  (`v · M`, with `(v·M)ⱼ = Σᵢ vᵢ · M[i][j]`).
- **Frame applied to a vector** (used once, for the azimuth axis, 2.4 step 7): the column
  form `M · v`, `(M·v)ᵢ = Σⱼ M[i][j] · vⱼ`.
- **Composition** `A ∘ B` is the ordinary matrix product A·B.

Elementary rotations by a degree angle a (with c = cos, s = sin), as matrices:

- About Z: `[[c, s, 0], [−s, c, 0], [0, 0, 1]]`.
- "Pitch and yaw" by angles y and z (one matrix): with cy, sy, cz, sz,
  `[[cy·cz, cy·sz, −sy], [−sz, cz, 0], [sy·cz, sy·sz, cy]]`.
- About a unit axis (x, y, z) by a, with t = 1 − c:
  `[[t·x·x + c, t·x·y + s·z, t·x·z − s·y], [t·x·y − s·z, t·y·y + c, t·y·z + s·x], [t·x·z + s·y, t·y·z − s·x, t·z·z + c]]`.

"**Rotate frame F by R**" always means **F ← R · F** (the new rotation on the left). "**Then
apply R**" means **F ← F · R** (on the right); the text says which.

### 2.3 Before the trunk (once per tree)

1. Seed the random stream with the tree seed (`TreeInfo::seed`; seeds ≤ 1 become 1).
2. **Random bank**: draw 1000 uniforms in [0, 1] into a bank B[0..999]. Two counters into the
   bank, the *roughness index* and the *frequency index*, start at 0; they are shared by the
   whole tree and only ever increase. The bank is read later as `B[counter mod 1000]`.
3. Reseed with the tree seed. **Tree size** = uniform(size − size_variance, size +
   size_variance) (`TreeInfo::size`, `size_variance`).
4. Seed the C runtime generator used for flares (2.6) with the flare seed (`flare_seed`).
5. Leaf card sizes are the leaf textures' relative sizes times `TreeInfo::size` (the stored
   size, not the random tree size); stage 5 uses them.
6. Reseed with the tree seed again.
7. Grow the trunk: level 0, size = tree size, base position (0, 0, 0), percent of parent 0,
   base frame = identity rotated about Z by `tree_angle` (0 when absent), azimuth axis
   (0, 0, 1), parent radius −1, not a root.

### 2.4 Growing one branch

Inputs: level L, size S (the tree size, passed unchanged to every branch), base position P₀,
percent of parent p (where along the parent it grows, 0 for the trunk), base frame F₀, azimuth
axis a, parent radius R_parent (−1 for the trunk), the seed value σ (the tree seed for the
trunk; children receive their parent's running value, see 2.5), and whether it is a root.

The level parameters `info` are branch level L (or the root parameters `RootSupport::roots`
when growing a root). Curves missing from a file default to a flat curve of value 1:
`BezierSpline 0 1 0 { 2  0 1 1 0 0.1  1 1 1 0 0.1 }` for the cross-section profile,
light-seam profile, roughness profile, frequency profile, light-seam bias and gnarl profile.

1. **Frond test.** If fronds are enabled and L ≥ the frond level, apply the frond rules (2.7)
   to this branch with p. A root never becomes a frond (an "enabled" result is treated as
   "disabled" for roots). If the result is *pruned*, the branch does not exist: stop, with no
   draws.
2. **Flares** (only for real branches, not fronds): compute this branch's flare entries
   (2.6, C runtime generator only).
3. **Length** = length.evaluate(p) · S (with variance, unscaled: one draw).
   Segment length = length / `segments` (provisional, replaced per vertex below).
4. **Azimuth** = uniform(−180, 180) (one draw).
5. **Start angle** = start_angle.evaluate(p) (one draw).
   **Gravity** = gravity.evaluate(p) (one draw).
   **Radius** = radius.evaluate(p) · S (one draw); use its absolute value.
   **Flexibility** = flexibility.evaluate(p) (one draw).
   Number of cross-section points c = `cross_section_segments`.
6. **Radius limits.** Maximum radius = radius.max · S. If L > the first branch level
   (`cluster_first_branch_level`, default 0), R_parent > 0 and 0.85 · R_parent is smaller
   than that maximum, the maximum becomes 0.85 · R_parent. Clamp the radius to the maximum.
   **First radius** = radius · radius_scale.evaluate(0) (one draw; used by fronds only).
7. **Vertex count** n = `segments` + 1, then (for real branches; fronds may override it, stage
   4):
   - if `min_length_percent` ≠ 1 and length.min < length.max: f = clamp((length −
     length.min·S) / (length.max·S − length.min·S), 0, 1); n = (int)lerp(n ·
     min_length_percent, n, f); at least 2.
   - if `min_cross_section_percent` ≠ 1 and radius.min < radius.max: f = clamp((radius −
     radius.min·S) / (radius.max·S − radius.min·S), 0, 1); c = (int)lerp(c ·
     min_cross_section_percent, c, f); at least 3.
   If n < 2 the branch has no vertices and no children; skip to step 12 (volume 0).
8. **First vertex** (index 0): position P₀.
   1. Azimuth disturbance = disturbance.scaled_variance(0) (one draw), then pitch disturbance
      = disturbance.scaled_variance(0) (one draw).
   2. Frame F = F₀. Azimuth axis in world = F₀ · a (column form, 2.2). Rotate F by the
      axis rotation (azimuth, that axis): F ← R · F. Then rotate F by the pitch-and-yaw
      matrix with y = start angle + pitch disturbance and z = azimuth disturbance: F ← R · F.
   3. Radius = radius · radius_scale.evaluate(0, scaled) (one draw).
      Vertex flexibility = flexibility · flexibility_scale.evaluate(0) (one draw).
   4. **Gravity**: d = F's direction. Gravity angle g = degrees(angle between d and down).
      Facing factor = 1 − |90 − g| / 90. Axis k = normalize(d × down). Gravity profile =
      −2 · (angle_profile.evaluate(0) − 0.5) (one draw). Then apply (on the right) the axis
      rotation by (profile · gravity · g · facing factor) degrees about k: F ← F · R.
      (Built from identity; if d is parallel to down the axis is degenerate — normalize of a
      zero vector; keep whatever the zero-length normalize gives, see 2.8.)
   5. **Gnarl** = `gnarl` · gnarl_profile.evaluate_without_variance(0). If `gnarl_unison` is
      false and (int)azimuth is even, negate it (and remember "negated" for the whole
      branch). Apply (on the right) a rotation about Z by gnarl degrees: F ← F · R.
   6. The vertex stores F, its direction (first row of F), radius, fork count 0, running
      length 0.
   7. Cross-section count for this vertex = (int)lerp(3, c, clamp(cross_section_profile.
      evaluate_without_variance(0), 0, 1)) (stage 3 uses it).
9. **Remaining vertices** i = 1 … n − 1, each from the previous one (index i − 1):
   1. Linear progress u = i / (n − 1); progress t = u ^ `segment_packing_exponent`
      (`Flares::segment_packing_exponent`).
      Segment length = t · length − running length so far.
   2. Cross-section count = (int)lerp(3, c, clamp(cross_section_profile.
      evaluate_without_variance(t), 0, 1)).
   3. Radius = radius · radius_scale.evaluate(t, scaled) (one draw).
      Vertex flexibility = flexibility · flexibility_scale.evaluate(t) (one draw).
   4. F = previous vertex's frame.
   5. Gravity exactly as 8.4 but with angle_profile.evaluate(t) (one draw), applied on the
      right.
   6. Azimuth disturbance = disturbance.scaled_variance(t) (one draw), pitch disturbance =
      disturbance.scaled_variance(t) (one draw); rotate F by the pitch-and-yaw matrix with
      y = pitch disturbance, z = azimuth disturbance (on the left: F ← R · F).
   7. Gnarl = `gnarl` · gnarl_profile.evaluate_without_variance(t), negated if the branch is
      negated; apply on the right as in 8.5.
   8. Direction = first row of F. Position = previous position + previous vertex's
      direction · segment length.
   9. **Floor** (when the floor is enabled, the branch has ever had a vertex at or above the
      floor height, L ≥ the floor level, and this vertex is not higher than the previous):
      if z < floor, z = lerp(floor, z, u ^ floor exponent · floor bias). After that, a vertex
      with z ≥ floor marks the branch as having been above the floor. (The first vertex marks
      it too when P₀.z ≥ floor.) Floor values: `Floor` (enabled, value, level, exponent,
      bias).
   10. Fork count 0; running length += segment length (stored on the vertex).
10. Wind weights for each vertex are computed alongside (stage 7); they draw nothing.
11. **Children** — see 2.5.
12. **Volume** of the branch (stage 3 LOD uses it): Σ over consecutive vertex pairs of
    distance · (radius₁ + radius₂).

Real branch geometry (stage 3) is built at each vertex as it is made, for L ≥ the first
branch level; levels below the first branch level grow skeletons only. Building it draws no
random numbers from the stream (it reads the bank, 2.3).

### 2.5 Children

After the vertices:

1. Number of children N = (int)((`frequency` / S) · length) of level L.
2. The children's level is L + 1. If L + 1 equals the root level (`RootSupport::level`), add
   (int)((`RootSupport::frequency` / S) · length) **root candidates**, after the normal
   children. A root itself has no children at all (N = 0, no roots).
3. Children of the last level are **buds** (leaf sites, stage 5): when L + 1 ≥ (number of
   branch levels − 1). If the children would be buds and leaves are not being computed (the
   file carries leaf LODs; see stage 5), skip the children loop entirely (no draws).
4. Keep the incoming seed value σ; call the value of σ on entry σ₀ (the "root seed").
5. For each child index k from 0 to (N + roots − 1); "root candidate" means k ≥ N:
   1. **Reseeding:** if k == N (the first root candidate), reseed the stream with σ₀.
      Otherwise, if the children are not buds and k < N: σ ← σ + 3; reseed with σ.
      (Buds reseed nothing.)
   2. **Where:** if the children are not buds, k == 0, L + 1 > the first branch level and it
      is not a root candidate: position along the parent q = uniform(lerp(first, last, 0.85),
      lerp(first, last, 0.95)) where first/last are this level's `first_branch`/
      `last_branch`. Otherwise q = uniform(root first, root last) for a root candidate
      (`RootSupport::first`, `last`), else uniform(first, last). One draw.
   3. **Frequency culling** (not for roots): f = clamp((q − first)/(last − first), 0, 1);
      value = frequency_profile.evaluate_without_variance(f). If value ≠ 1, take the next
      bank value `B[frequency index mod 1000]` (incrementing the index); if it is greater
      than value, skip this child.
   4. **Forking** (not roots, not buds, when `fork_enabled`): draw uniform(0, 1) (one draw)
      and if it is ≤ `fork_bias`, move q onto a parent vertex (2.5.1); if that fails or the
      new q falls outside [first, last], skip this child. (When forking is disabled no draw
      is made.)
   5. **Pruning:** find the "test percent": q itself if `pruning_depth` is 0, otherwise walk
      up from this branch: start at this branch, counter = 1, and while the current branch has
      a parent and counter < `pruning_depth`, move to the parent and add 1; the test percent is
      that branch's own percent of parent (the p it was grown with). If `pruning_percent` < 0,
      keep the child only if test percent ≤ −pruning_percent; otherwise keep it only if test
      percent ≥ pruning_percent. A dropped child makes no further draws.
   6. Locate q · length on the parent: the segment j is the first vertex index i ≥ 1 whose
      running length is greater than q · length, minus one (0 if none);
      s = (q·length − run[j]) / (run[j+1] − run[j]).
   7. **Another reseed** (not buds, not roots): reseed with σ and draw one uniform(0, 100),
      discarding it.
   8. Child base position = lerp(position[j], position[j+1], s). Profile percent =
      (q − first) / (last − first) for normal children (1 if first == last); for roots
      (q − root first)/(root last − root first) (1 if equal).
   9. **Buds** go to stage 5 with the parent vertex j's frame and direction.
      **Branches** grow recursively (2.4) with: level L + 1, size S, base position as above,
      percent of parent = the profile percent, the seed value σ (current), the parent radius
      lerp(radius[j], radius[j+1], s), root flag, and:
      - if L + 1 == the first branch level: base frame identity and azimuth axis (0, 0, 1);
      - otherwise: base frame = vertex j's frame, azimuth axis = vertex j's direction.
      A child that came back *pruned* (frond rule) is discarded.

**2.5.1 Fork placement.** With g = q · length: find j as in step 6 (first i ≥ 1 with
running length > g, minus one; 0 if none). If g − run[j] > run[j+1] − g, use j + 1. If
j < n − 1 and vertex j's fork count < `fork_limit`: increase that count; the fork angle is
180 if j == 0, else degrees(angle between direction[j] and direction[j−1]); if it is
≥ `fork_angle`, the new q = run[j] / length. Otherwise the child is skipped.

### 2.6 Flares (C runtime random numbers)

Flares widen a branch's base (stage 3 uses them). They use the C runtime generator seeded in
2.3 step 4, not the stream. DA:O's runtime is Microsoft's: state ← state · 214013 + 2531011
(mod 2³²), output (state >> 16) & 0x7FFF, maximum 32767; a float in [a, b] is
a + (output / 32767) · (b − a). If `Flares::count` is 0 nothing is drawn. Otherwise:

- initial angle = rand_float(0, 2π); balance = 2π / count;
- for each flare i: angle = initial + i · rand_float(`balance` · balance, balance), minus 2π
  if above 2π; radial exponent and length exponent from the level; radial influence =
  radians(rand_float(influence − variance, influence + variance)); length influence =
  rand_float(length distance − variance, + variance); distance = rand_float(radial distance −
  variance, + variance) (the `Flares` fields of the level, in that order).

### 2.7 Frond rules

Fronds have a distance percent d, a distance level m and two outcomes (enabled, disabled,
pruned) for "above" and "below" (`Fronds::distance_percent`, `distance_level`,
`above_condition`, `below_condition`). Test percent: the branch's p if m is 0; otherwise walk
up from the branch's parent, counting levels from 0, while the current branch has a parent and
the count < m, moving up one each time; use that branch's own percent of parent. If d < 0:
"below" when test ≥ −d, else "above"; if d ≥ 0: "above" when test ≥ d, else "below". With
fronds disabled the result is always "disabled".

### 2.8 Edge cases to match

- Normalizing a zero vector: return it unchanged (zero) and let the axis rotation degenerate
  to a scale by cos; this happens only for a direction exactly along down. Flag it in tests.
- `(int)` of a negative azimuth truncates toward zero before the parity test.
- All arithmetic in 32-bit float; accumulate running lengths in float.

Stages 3 and later: in progress.
