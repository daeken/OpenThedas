# SpeedTree 4 tree generation

How the trees, bushes and grass of Dragon Age: Origins' outdoor levels are grown from their
`.spt` descriptions at load time. The game ships SpeedTree 4.x's runtime as a separate
library it calls once per tree file; this is a behavioural description of what it computes, written in our own terms, for a clean-room
implementation. It names no types, functions or files of any SpeedTree source. Facts checked
in DAOrigins.exe say so; the rest is to be verified against the executable where practical.

See [format.md](format.md) for the file format and how levels place trees; stage 7 covers how
the game draws them.

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
parameter, y the normalized value). Tangent directions are normalized on input: each component is divided by `sqrtf(tx·tx + ty·ty)` (a true square root), and a zero tangent is left as it is. Consecutive
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
- **Frame applied to a vector** (for the azimuth axis, 2.4 step 8.2, and in 5.2 step 3):
  the column form `M · v`, `(M·v)ᵢ = Σⱼ M[i][j] · vⱼ`.
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
7. **Vertex count** n = `segments` + 1. For a frond branch with `Fronds::use_segment_override`,
   n = `segment_override` + 1 and the two adjustments below are skipped. Otherwise:
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
10a. **Frond guide end** (frond branches only, after the vertices and before the children):
    the frond records this branch as a *guide* (4.1), which draws two numbers when the frond
    has at least one frond texture: a texture choice uniform(0, 100000) and an angle offset
    uniform(min, max). With no frond textures it draws nothing.
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

- 3D normalize (every "normalize" in stages 2 and later): sum of squares s = x·x + y·y + z·z
  (left to right); if s > 100 · FLT_EPSILON (FLT_EPSILON = 2⁻²³) the inverse length is
  1 / sqrtf(s), otherwise 1 / 0.01 = 100; multiply each component by that inverse (one
  division, three multiplications). A zero vector stays zero; a tiny one is scaled by 100.
- `(int)` of a negative azimuth truncates toward zero before the parity test.
- All arithmetic in 32-bit float; accumulate running lengths in float.

### 2.9 Clarifications (round 1)

Answers to the implementer's questions on stages 1 and 2.

1. **Raw step reaching 1.0.** For seeds from 2147483584 to 2147483646, `(float)seed ·
   4.656612875e-10` rounds to exactly 1.0, the table index becomes 128, and the original reads
   one element past its 128-entry table (whatever value lies next in memory) and then writes a
   new raw value there. It is undefined, not traced in the exe, and reachable only from 63 of
   2³¹ − 1 states. Clamping to 127 is acceptable; note it as provisional.
2. **Interpolation form.** Every interpolation (de Casteljau steps, table interpolation, all
   later `lerp`s) is `a + (b − a) · u`, never `a·(1 − u) + b·u`.
3. **σ is passed by value.** A child receives the parent's current σ; the child's own
   increments do not come back. The parent continues from its own count.
4. **Which level's parameters drive the children.** The parent's: everything in 2.5
   (`frequency` in step 1, `first_branch`, `last_branch`, `frequency_profile`, `fork_*`,
   `pruning_*`) comes from the level being grown, L, not L + 1. Root candidates use
   `RootSupport::first`/`last`/`frequency` for placement as stated.
5. **Plain "evaluate (one draw)"** means *with variance, unscaled* everywhere it is written
   without "scaled": length, start angle, gravity, radius, flexibility, `radius_scale(0)` for the
   first radius, `flexibility_scale`, `angle_profile`. Only the per-vertex radius uses the
   scaled form.
6. **Locating q·length beyond the last vertex.** Literally 0: when no running length exceeds
   q·length, j = 0 and s = (q·length − run[0]) / (run[1] − run[0]), which may exceed 1 (the
   child is extrapolated along the first segment). Same in 2.5.1.
7. **Normalize** — see 2.8 (three multiplications by one inverse; the small-length rule). The
   squared sum is accumulated x, then y, then z. One exception: the **distance** used for a
   branch's volume (2.4 step 12) uses a fast approximate square root, reinterpreting the float's
   bits as an int i, computing `(i >> 1) + 0x1FC00000` (arithmetic shift) and reinterpreting back;
   nothing else in stages 1–2 uses it.
8. **Flares.** Yes: each flare's angle is initial + i · rand_float(`balance` · (2π/count),
   2π/count), with "minus 2π" applied once if above 2π (2π here is the float constant
   6.283185). A C runtime float is `a + (rand() / 32767.0f) · (b − a)` in float. Flares are drawn
   for every branch that is not a frond, **including** levels below the first branch level
   (skeleton-only levels draw flares too).
9. **Frond branches in stage 2.** Besides "no flares": they build no cross sections and read
   no bank values (2.4 end), and stage 4 may change their vertex count. Their vertices, draws
   and children are otherwise identical.
10. Noted: literal handling of a first branch level of −1 and a root level of 0 is correct.
11. **Gravity units.** Yes: the rotation angle profile · gravity · g · facing is in degrees with
    g in degrees, exactly as written.

## Stage 3 — Branch geometry

Every branch that is not a frond and whose level is at or above the first branch level gets a
tube of vertices around its skeleton. A tree's branch vertices go into one shared vertex list,
branch after branch in the order branches are grown (2.4 is depth-first: a branch's own
vertices are emitted while its skeleton is grown, before any of its children). Each branch
remembers the index of its first vertex (its *start*).

### 3.1 Per-branch values

- **Tile offset** = (azimuth + 180) / 360 + p (azimuth from 2.4 step 4, p the percent of parent).
- **Base radius** = the first vertex's radius (2.4 step 8.3).
- **Maximum radius** as clamped in 2.4 step 6.
- **Relative length** = branch length / S.
- Whether the gnarl was negated (2.4 step 8.5) also negates twist (3.3).

### 3.2 One cross section (built at each skeleton vertex as it is made)

For the vertex with frame F (rows r₀, r₁, r₂), position C, radius r, branch progress t (0 for
the first vertex, the packed progress for the others), and its cross-section count m (2.4 steps
8.7 and 9.2):

- Roughness factor ρ = roughness_profile.evaluate_without_variance(t).
- A running angle fraction f starts at 0 and **is increased by the float 1/m after each point**
  (accumulated, not recomputed as i/m).
- For i = 0 … m (m + 1 points):
  1. angle α = 2π · f (2π = 6.283185 as a float).
  2. **Texture coordinates** for each of the seven layers except the shadow layer (`layers[0..6]`
     in our parser's order: diffuse, detail, normal, height, specular, user 1, user 2), see 3.3,
     using f as the radial progress.
  3. **Radial direction** ν = cos α · r₁ + sin α · r₂.
     **Tangent** τ = cos(α + π/2) · r₁ + sin(α + π/2) · r₂ (π/2 = 1.570796).
     **Binormal** = ν × τ.
  4. If i < m (a real point):
     - radius ρr = r; k = r / maximum radius;
     - ρr += k · `roughness` · S · ρ · sin(`roughness_vertical_frequency` · t) ·
       sin(`roughness_horizontal_frequency` · t) · cos(`roughness_horizontal_frequency` · α);
     - clamp ρr to the maximum radius;
     - ρr += (B[roughness index mod 1000] − 0.5) · 2 · `random_roughness` · S · k · ρ, then
       increase the shared roughness index (2.3) by one;
     - flare factor φ = 1 + Σ over the branch's flares of flare_distance(α, t) (3.4);
     - position = C + ν · ρr · φ.
     If i == 0 remember this position.
  5. If i == m: position = the remembered first position (the seam repeats the first point;
     its texture coordinates, tangent and binormal are those computed at f ≈ 1).
  6. Wind attributes: stage 7.

Normals are not taken from ν; they are computed once the whole branch exists (3.5).

### 3.3 Texture coordinates of one layer

Inputs: the layer's controls (`TexLayer`), radial progress f, branch progress t, the tile
offset, base radius, relative length, negated flag. If a layer other than the diffuse one has
`sync` set, it uses the diffuse layer's controls instead of its own.

- S tile = `s_tile` if `s_tile_absolute`, else `s_tile` · base radius · 2π.
  T tile = `t_tile` if `t_tile_absolute`, else `t_tile` · relative length.
  Twist = `twist`, negated if the branch's gnarl was negated.
- **s:**
  - with `clamp_s`: left/right = `clamp[0]`/`clamp[1]`. Start angle = `s_offset` · 90, then 0
    if negative, then wrapped into [0, 360] by subtracting/adding 360 while above 360/below 0.
    Sweep = 0 if S tile is 0, else 360 / S tile. End = start + sweep; if end > 360 then
    end = 360 and start = 360 − sweep. This angle = f · 360, clamped into [start, end].
    s = lerp(left, right, (this − start) / sweep).
  - otherwise s = f · S tile + `s_offset` + twist · t.
- **t** = (t + (tile offset if `random_t_offset` else 0)) · T tile − `t_offset`.
  With `clamp_t`: clamp to [0, 1], then t = lerp(`clamp[2]`, `clamp[3]`, t) (bottom, top).

### 3.4 Flare distance

For one flare (2.6: angle a_f, radial influence ri in radians, radial exponent re, length
influence li, length exponent le, distance d) at angle α and branch progress t:

- difference = |α − a_f|; if it exceeds π (3.141593), add 2π to whichever of α and a_f is
  smaller, and recompute the difference.
- If difference < ri and li − t > 0: radial = (1 − difference/ri) ^ re, vertical =
  ((li − t)/li) ^ le (both powers computed in double precision, then converted to float);
  result = d · radial · vertical. Otherwise 0.

### 3.5 Normals (per branch, after its vertices exist)

Let the branch have n skeleton vertices, vertex i having mᵢ + 1 tube points starting at offset
oᵢ (o₀ = the branch start, oᵢ₊₁ = oᵢ + mᵢ + 1). For each i and each j = 0 … mᵢ:

- **Around:** prev = oᵢ + (mᵢ − 1 if j == 0 else j − 1); next = oᵢ + (1 if j == mᵢ else
  j + 1). h = normalize(position[next] − position[prev]).
- **Along:** fraction g = j / mᵢ. prev = oᵢ + j if i == 0, else oᵢ − ((mᵢ₋₁ + 1) −
  (int)(mᵢ₋₁ · g)); next = oᵢ + j if i == n − 1, else oᵢ + (mᵢ + 1) + (int)(mᵢ₊₁ · g).
  w = normalize(position[next] − position[prev]).
- Normal = h × w; if exactly zero, use the skeleton vertex's direction D.
- **Light-seam reduction:** e = light_seam_profile.evaluate_without_variance(i / (n − 1));
  b = light_seam_bias.evaluate_without_variance(p) (p the branch's percent of parent);
  normal = normalize(lerp(D, normal, e · lerp(b, 1, (i/(n − 1)) ^ 0.5))).

### 3.6 Triangles

A branch's surface is one triangle strip over its tube points, with degenerate joins (the
same index lists serve every LOD, 3.7). For each consecutive pair of skeleton vertices
(i, i + 1), with a = mᵢ + 1 and b = mᵢ₊₁ + 1 points and M = max(a, b):

- for j = 0 … M − 1: g = j / (M − 1); x = (int)((a − 1) · g + 0.5); y = (int)((b − 1) · g +
  0.5); emit (start + run + a + y) then (start + run + x), where run is the number of points
  in skeleton vertices before i;
- then emit the second-to-last emitted index twice more. For example, if the pair ends
  with …, a+y, x, the list continues …, a+y, x, a+y, a+y. The join also produces one
  zero-area triangle through the seam point whose three indices differ; it is drawn and
  covers nothing;
- run += a.

Read the list as a triangle strip (each index forms a triangle with the two before it,
alternating winding; triangles with a repeated index are degenerate and drawn as nothing).

### 3.7 Branch LOD

Parameters: `Lod::branch_lod_count` (L_n), `branch_max_volume_percent`,
`branch_min_volume_percent`, `branch_reduction_fuzziness`, `large_branch_percent`.

After the whole tree has been grown (after stage 2's recursion and everything drawn during
it), and before leaves are reduced:

1. List the branches in growth order: the trunk, then recursively each branch followed by its
   kept children in the order they were made. Fronds and pruned branches are not in the list
   (but skeleton-only branches below the first branch level are).
2. Total = Σ max(volume, 0); largest = the largest volume.
3. In list order, a branch whose volume > largest · (1 − `large_branch_percent`) is *kept*
   (moved, in order, to a separate kept list). For every other branch draw f =
   uniform(0, `branch_reduction_fuzziness`) from the stream (one draw each, in order) and set
   its fuzzy volume to max((1 − f) · volume + f · largest, 0).
4. Sort the others by fuzzy volume, descending. Then insert the kept branches one by one at
   the front of that list, in kept-list order (so the last kept branch ends first).
   (The kept branches' fuzzy volume is 0; the original also "sorts" the kept list by it, which
   leaves an unspecified order for ties: keep kept-list order.)
5. For each LOD level k = 0 … L_n − 1: fraction = 1 if L_n < 2, else lerp(max percent, min
   percent, k / (L_n − 1)); target = fraction · total. Walk the list adding each branch's
   (real) volume while the running sum < target; every branch walked contributes its strip
   (3.6) to LOD k (branches with no geometry contribute nothing). If no branch is walked at
   LOD 0, LOD 0 is empty.
6. **Fade hints**: every index in a branch's strip for LOD k sets that vertex's fade hint to
   −1 if k is the last level, else 1 − (k + 1)/L_n; later levels overwrite earlier ones.

## Stage 4 — Fronds

Fronds are flat or profiled strips grown along the skeletons of frond branches instead of
tubes (`Fronds::enabled`, at levels ≥ `Fronds::level`, subject to the frond rules in 2.7).
Their vertex lists are separate from the branches'.

### 4.1 Guides (recorded while the tree grows)

A frond branch's skeleton vertices (position, frame, wind weights) become a **guide**, in the
order frond branches are grown. When the branch's vertices are done (2.4 step 10a):

- guide length = Σ distances between consecutive vertex positions, using the **fast square
  root** of §2.9 item 7;
- if there are frond textures (`Fronds::textures`): texture index =
  (unsigned)uniform(0, 100000) mod (number of textures) (one draw); radius = length ·
  `aspect_ratio` · 0.5 · `size_scale` of that texture; angle offset = uniform(`min_angle_offset`,
  `max_angle_offset`) (one draw), negated if the number of guides recorded so far (counting
  this one) is odd;
- with no textures: index 0, radius = length · 0.5, offset 0;
- surface area = length · the branch's first radius (2.4 step 6).

### 4.2 When fronds are built

After the whole tree, its branch LOD (3.7) and its leaf LOD (5.4) are computed, fronds are
built in one pass: first the guide LOD selection (4.3, draws), then, for extrusion fronds, each
guide's profile (4.5, draws), in level-0 list order (4.3 step 5).

### 4.3 Frond LOD selection

Parameters: `lod_count` (F_n), `max_surface_percent`, `min_surface_percent`,
`reduction_fuzziness`, `large_frond_percent`.

1. Total = Σ surface areas; largest = the largest area; L² = largest · largest.
2. In guide order: a guide whose area > largest · (1 − `large_frond_percent`) is kept (moved,
   in order, to a kept list); every other guide draws f = uniform(0, `reduction_fuzziness`)
   (one draw each) and gets fuzzy area (1 − f) · area + f · L² (note: the *square* of the
   largest area).
3. Sort the others by fuzzy area, descending; insert the kept guides one by one at the front
   (the last kept ends first; kept guides compare equal, keep kept-list order).
4. For level i: fraction = `max_surface_percent` if F_n == 1, else lerp(max, min,
   i/(F_n − 1)); target = fraction · total. If target > 0, the level uses the first guides of
   the sorted list, taking guides while the count is below the number of guides **and** the
   running area ≤ target (note ≤, unlike branches), adding each guide's real area. If target
   ≤ 0 the level has no fronds.
5. Only guides in level 0's list get vertices, built in **level-0 list order**. Lower levels
   reuse those vertices (each level's list is a prefix of level 0's).

### 4.4 Blade fronds (`kind` = 0)

With B = `blades` and N guide vertices, each guide gets B blades, each a ribbon of 2N
vertices (right, left per guide vertex), blades one after another:

- spacing angle = 180 / B degrees.
- For blade b and guide vertex i: frame G = the frame of vertex i, or of vertex i − 1 when i
  is the last; rotate it about X by (spacing · b + angle offset) degrees (on the left:
  G ← Rx · G, with Rx = `[[1, 0, 0], [0, c, s], [0, −s, c]]`).
  Right direction ρᵢ = (0, 1, 0) · G (G's second row).
- Normal = normalize(pos[1] − pos[0]) for i = 0, else normalize(pos[i] − pos[0]).
- Positions, with radius R and Pᵢ the guide vertex position:
  - last vertex (i = N − 1): right = Pᵢ + ρᵢ₋₁ · R, left = Pᵢ − ρᵢ₋₁ · R;
  - middle vertices: right = ((Pᵢ + ρᵢ · R) + (Pᵢ + ρᵢ₋₁ · R)) · 0.5, left likewise with −;
  - first vertex: right = Pᵢ + ρᵢ · R, left = Pᵢ − ρᵢ · R.
- Texture coordinates before atlas mapping: right (1, i/(N − 1)), left (0, i/(N − 1)).
- Tangent = ρᵢ; binormal = −normalize(normal × ρᵢ), or −normal when |normal × ρᵢ| < 0.1
  (that length uses the fast square root).
- **Unpinching** of t, per blade, after its 2N vertices exist: compute cumulative side
  lengths along the right vertices and along the left vertices, but **always measured on the
  guide's first blade** (the first 2N vertices of the guide), with the fast square root. For
  1 ≤ i ≤ N − 2, set t = 0.999 · (cumulative length to vertex i) / (total length) for that
  side. Vertex 0 keeps 0 and vertex N − 1 keeps 1.
- Strip per blade: its 2N vertex indices in order (right₀, left₀, right₁, left₁, …), one strip
  per blade, for every LOD level the guide belongs to.

### 4.5 Extrusion fronds (`kind` = 1)

Profile, with P = `profile_segments` and the guide radius R (all evaluations with variance,
unscaled, one draw each, in this order): first a = profile.evaluate(0) · R; then for i = 0 …
P − 1, f = 1 − i/(P − 1) and point i = (0, −f · R, profile.evaluate(f) · R − a). Then mirror:
for j = P − 2 down to 0 append point j with its y negated. That gives V = 2P − 1 points, the
centre point at index P − 1.

Profile normals and tangents, for each point i, with the angle of a neighbour pair being
`atan2(z₂ − z₁, y₂ − y₁)` (float):

- i = P − 1 (centre): angle = π/2;
- i = 0: angle = (angle from point 0 to 1) + π/2;
- i = V − 1: angle = (angle from point V − 2 to V − 1) + π/2;
- otherwise: angle = 0.5 · (previous pair's angle + next pair's angle) + π/2.

Normal = normalize(0, cos angle, sin angle); tangent = normalize(0, −cos(angle + π/2),
−sin(angle + π/2)).

Vertices, for each guide vertex i (N of them) and profile point j (V of them):

- T = vertex i's frame, T′ = vertex i − 1's frame (T′ = T for i = 0); rotate both about X by
  the angle offset (on the left, as in 4.4).
- position = Pᵢ + 0.5 · (pointⱼ · T + pointⱼ · T′) (row vector times matrix);
- normal = normalize(0.5 · (nⱼ · T + nⱼ · T′)); tangent = normalize(0.5 · (tⱼ · T + tⱼ ·
  T′)); binormal = normalize(tangent × normal), or the normal when |tangent × normal| < 0.1
  (fast square root);
- texture coordinates before atlas mapping (j/(V − 1), i/(N − 1)).

Unpinching: for each column j, cumulative length along that column's vertices (fast square
root); for every i ≥ 1 (including the last), t = 0.999 · cumulative / total.

Strips per LOD level l (levels as in 4.3, F_n levels): fraction = 1 if F_n == 1, else
1 − l/(F_n − 1). Length vertices Nl = (int)lerp(`min_length_segments` + 1, N, fraction);
cross vertices Vl = (int)lerp(2 · `min_cross_segments` + 1, V, fraction). If
`Lod::fade_extrusions` is set, Nl = N and Vl = V. Steps: length step = N / (Nl − 1), cross
step = V / (Vl − 1). For each i = 0 … Nl − 2:

- this = (int)(i · length step); next = (int)((i + 1) · length step); if i == Nl − 2 or
  next > N − 1, next = N − 1. skip = next − this; base = this · V.
- For even i walk j = 0 … Vl − 1, for odd i walk j = Vl − 1 down to 0; for each j the
  column c = (int)(j · cross step), but V − 1 when j == Vl − 1, and at most V − 1; emit
  (start + base + c) then (start + base + c + skip · V).
- If i < Nl − 2, repeat the last emitted index once.

### 4.6 Fade hints and atlas coordinates

- Each index of a frond's strip at LOD level l sets that vertex's fade hint to −1 if l is
  the last level, else 1 − (l + 1)/F_n; later levels overwrite.
- A vertex's texture coordinate (s, t) is mapped into the frond atlas rectangle of its
  texture (the texture-coordinate section's frond entry: corners c[0..7] = (s₀, t₀, s₁, t₁,
  s₂, t₂, s₃, t₃)): u = lerp(c[2], c[0], s), v = lerp(c[5], c[1], t). (The original's optional
  flip negates t twice on this path, so it has no effect.) These are in the file's
  convention, t upward; see format.md for sampling.

## Stage 5 — Leaves

Leaves are points with orientation, colour and a texture choice; their card shapes are drawn
at render time (stage 7). They are made at **buds**, the children of the last branch level
(2.5 step 3), while the tree grows, so their random draws are interleaved with the branches'
in growth order.

### 5.1 Order of the whole computation (for the random stream)

1. Set-up (2.3) and the trunk's recursive growth (2.4, 2.5): branch draws, frond guide draws
   (2.4 step 10a) and bud/leaf draws (5.2, 5.3) in growth order.
2. Branch LOD fuzziness (3.7).
3. Leaf LOD pairing dice (5.4).
4. Frond LOD fuzziness (4.3), then extrusion profiles (4.5).

### 5.2 A bud

A bud is made where 2.5 would have grown a child of the last level, at child level L + 1 =
(number of levels − 1), with that level's parameters (`branch_levels[L + 1]`), from parent
vertex j's frame F₀ and direction a, at base position P, with profile percent p (2.5 step 8):

1. Bud length = length.evaluate(p) · S (with variance, unscaled: one draw); step = bud length
   / `segments`.
2. Azimuth = uniform(−180, 180) (one draw).
3. Frame F = F₀; rotate F by the axis rotation (azimuth, F₀ · a) (on the left), then by a
   rotation about Y of 60 degrees (on the left), with Y-rotation matrix
   `[[c, 0, −s], [0, 1, 0], [s, 0, c]]`.
4. Leaf position Q = P + (first row of F) · step. Normal ν = normalize(Q − P).
5. **Parent direction** δ: if the spawning branch has no vertices, δ = ν. Otherwise, if
   `dimming_depth` is 0: the direction of the spawning branch's vertex at index
   (int)(p · (n − 1)) (computed in double precision; n its vertex count). Otherwise walk up:
   start at the spawning branch with a count of 0, and while there is a branch and count <
   `dimming_depth` − 1 move to its parent and count one; take that branch's first vertex
   position as the base (the spawning branch's first vertex if the walk ran off the trunk);
   δ = Q − base. Then δ = normalize(δ).
6. Make a leaf (5.3) at Q with p, the spawning branch, ν and δ.

### 5.3 Making a leaf

Each leaf texture i offers two choices, 2i and 2i + 1 (the odd one is the texture's mirrored
variant; stage 7). The choices of textures flagged `blossom` form the *blossom list*, the
others the *plain list*, in texture order.

1. No leaf textures: no leaf, no draws.
2. **Blossom?** Only if the blossom list is not empty. Test percent: p if `blossom_level` is 0;
   otherwise walk up from the spawning branch, counting from 1, while the branch has a parent
   and count < `blossom_level`, moving up and counting; use that branch's percent of parent.
   If `blossom_distance` < 0: blossom when test < −distance **and** a draw uniform(0, 1) ≤
   `blossom_weighting`; otherwise when test > distance and the draw ≤ weighting. The draw is
   made only when the percent condition holds.
3. **Choice:** a blossom picks, if the blossom list has more than one entry,
   (unsigned)uniform(0, 100000) mod its size (one draw), else its only entry. A plain leaf: if
   the plain list has more than one entry, (unsigned)uniform(0, 1000000) mod its size (one
   draw); if it is empty, no leaf; if it has one entry, that one.
4. **Crowding** (`collision`): 0 never crowded; 1 tested against the leaves made at this same
   spawning branch so far; 2 against every leaf made so far. With side = max(w, h) of the
   chosen texture · `spacing_tolerance`, where w and h are the texture's `size` times
   `TreeInfo::size` (2.3 step 5; this equals the file's stored `size_used` only to rounding,
   so compute it rather than reading `size_used`), the leaf
   is rejected if any earlier leaf (scanned from the newest back) lies strictly within ±side on
   all three axes. A rejected leaf makes no further draws.
5. **Colour scale:** 1, or with `dimming`: d = p; if `dimming_depth` ≠ 0, walk from the
   spawning branch with a count from 0 while there is a branch and count < depth, multiplying
   d by that branch's percent of parent and moving up. Scale = lerp(d, 1, 1 −
   `dimming_scalar`); stored as the integer (short)(255 · scale).
6. **Rocking group** = (int)uniform(0, 10000) mod 3 (one draw). (The group count is always 3.)
7. **Orientation.** n = δ if |ν| < 0.0001 (fast square root length), else lerp(δ, ν,
   `color_variance` of the texture); n = normalize(n). Keep n₀ = n.
   If the texture uses meshes (`mesh.use_meshes`): n = normalize(lerp(n, (0, 0, 1), `hang`)).
   Tangent τ = normalize(n × (0, 0, 1)); binormal β = normalize(τ × n). If |τ| < 0.5 (fast
   square root): β = −n₀ with its z set to 0, normalized; τ = normalize(n × β).
8. **Mesh rotation** (meshes with `rotate` ≠ 0): matrix M with rows τ, β, n; M ← Rz(angle) ·
   M with angle = B[k] · 2 · rotate − rotate, k being the spawning branch's own mesh counter
   (starts at 0 when that branch begins growing, +1 per use); read τ, β, n back from the rows.
9. **Corner normals.** With `smoothing_enabled` and no meshes: let M have rows τ, β, n and a =
   `smoothing_angle`, t = a / 100. For the four corners, with Ry(x) the Y rotation of 5.2
   applied on the left of M and "third row" its Z row:
   - corner 0: third row of Ry(−a)·M, lerped toward up (0, 0, 1) by t;
   - corner 1: third row of Ry(+a)·M, toward up by t;
   - corner 2: third row of Ry(+a)·M, toward down (0, 0, −1) by t;
   - corner 3: third row of Ry(−a)·M, toward down by t;
   each normalized; each corner's tangent = normalize(normal × (0, 0, 1)) and binormal =
   normalize(tangent × normal). Otherwise all four corners take n, τ, β.
10. **Colour:** offset = uniform(−color_variance, +color_variance) (one draw); every corner's
    colour = the texture's `color` + offset on each channel.
11. Store the leaf: position Q, choice index, colour scale, rocking group, corner frames and
    colours, wind weights and groups (stage 7).

The leaf list of the tree is in creation order.

### 5.4 Leaf LOD

Parameters: `Lod::leaf_lod_count` (levels), `leaf_reduction` (r), `leaf_size_increase` (g).
Level 0 is the full list. Pairing distance = 10 · (largest over textures of max(w, h) (the
card size of 7.2 at level 0) · `spacing_tolerance`), or 10 · 10 if there are no textures.
Level k (k ≥ 1) is made from level k − 1:

1. **Pairing**, a "paired" flag per leaf of level k − 1, all false. For each leaf i in order
   (whether or not it has been paired already): scan leaves j = i + 1, i + 2, …, **stopping the
   scan at the first leaf already flagged** (without considering it). Among scanned leaves
   take the one with distance (fast square root) < pairing distance and < the best so far
   (strict). If one was found, flag it and record the pair (i, match).
2. **Merging:** for each pair in order draw u = uniform(0, 1) (one draw); if u > r the pair
   becomes one leaf: a copy of leaf i, at the midpoint of the two positions, with z increased by
   (origin y − 0.5) · ((1 + g · k) · h · 0.5) of leaf i's texture, and each corner
   colour the average of the two leaves'. Leaves that were not paired do not carry over.

### 3.8 Clarifications (round 2)

1. **Zero maximum radius.** When a level's maximum radius is exactly 0, k = r / maximum is 0/0,
   which the original computes as NaN and carries into the position (NaN never compares
   greater than the maximum, so the clamp does not catch it). With a zero maximum the radius r
   is 0 too, so the intended surface is a tube of zero radius on the skeleton: use k = 0 and
   place every point at the centre C (the tube collapses), and note it as a deliberate
   deviation from the NaN.
2. **Flare factor order.** The flare distances are summed first, from 0, in flare order:
   D = ((0 + d₁) + d₂) + …; then φ = 1 + D. When φ is exactly 1 the position is C + ν · ρr
   (no multiplication by φ); otherwise C + ν · ρr · φ, computed as ((ν · ρr) · φ) per
   component.

### 5.5 Clarifications (round 3)

- **A. Pruning, confirmed literal.** Pruning uses the level being grown (L), its
  `pruning_percent` and `pruning_depth`; with depth 0 the test percent is q, the child's
  position along this branch (2.5 step 2). With level L = 2 of a grass file having
  `first_branch` 0.01, `last_branch` 0.02 and `pruning_percent` 0.019, q ∈ [0.01, 0.02] and only
  children with q ≥ 0.019 survive, about one in ten. That is the computation as specified; the
  sparse result (a few blades per clump) is expected from it, not a misreading. If the game's
  clumps look much denser in the reference shots, raise it as an (exe) item rather than
  changing the rule.
- **B.** Yes: in 4.4 "pos" are the guide's skeleton vertex positions (the frond branch's
  vertices from 2.4).
- **C.** Yes: the angle offset range is the chosen frond texture's `min_angle_offset` and
  `max_angle_offset`.
- **D.** Yes: Rz is the about-Z matrix of 2.2, its angle in degrees, applied on the left
  (M ← Rz · M).
- **E.** The midpoint is (a + b) · 0.5, per component.
- **F.** Atlas mapping is applied only to frond textures that have an entry in the
  texture-coordinate section; a frond texture without one keeps its unmapped (s, t) (0–1
  across the frond, as built in 4.4/4.5). The same holds for leaf textures without an entry.

### 5.6 Clarifications (round 4)

- **A. Child counts truncate toward zero.** N in 2.5 step 1 (and the root count in step 2)
  is the float product converted to an integer by truncation, so 2.9 gives 2, −0.4 gives 0.
  A negative branch length (a grass level whose length curve dips below 0) gives N ≤ 0: no
  children, no draws.
- **B. The pruning test uses the final q.** With `pruning_depth` 0 the test percent is q as
  it stands at step 5, after any fork move in step 4. Grass density is sensitive to this
  rule and to step 1's truncation, and both are as the original computes them: a file whose
  N truncates to 3 grows 3 candidate blades however close it came to 4.
- **C. The floor moves skeleton vertices only.** Fronds are built around their guides
  afterwards, so frond geometry of a low bush can reach below the floor height. The original
  does the same.
- **D. Undefined cases.** Where a computation divides by zero or indexes a curve table with a
  value outside [0, 1] (first == last in frequency culling, a zero-length parent, a curve
  parameter outside [0, 1], a malformed curve), the original's result is undefined or reads
  outside its tables. No game file reaches these cases. Any well-defined choice is acceptable
  (clamping the table index, f = 0, s = 0, the earlier point of a zero-width span); mark
  them provisional.
- **E. Missing data.** A missing curve other than the six profiles of 2.4 evaluates to its
  minimum, a branch level index past the file's levels grows nothing, absent floor values
  mean the floor is disabled, a missing Lod section means one level at 100%, and a frond LOD
  count of 0 means no fronds. Fuzzy sorts (3.7, 4.3, 5.4) are stable: ties keep list order.
  None of these occur in game files.
- **F. The tree's bounding box** (used for the billboard, 7.5; the game's LOD radius, 7.4;
  and self-shadow coordinates, 7.9) is corrected in 5.7 F.

### 5.7 Clarifications (round 5)

- **A. Leaf normals use `color_variance`.** 5.3 step 7 is right: the stored per-texture
  `color_variance` is also the weight of the normal toward ν (the perturbed direction), as
  the original computes it. `hang` then applies for mesh textures. (The original applies
  `hang` only under its dynamic leaf lighting, which is what DA:O uses.)
- **B. Leaf rock and rustle scalars** (7.2) are the file's `speedwind_rock_scalar` and
  `speedwind_rustle_scalar` as stored, the same for every leaf LOD; 1.0 when absent.
- **C. A missing frond profile curve** (4.5) takes the original's built-in default, not the
  flat curve of 2.4 and not the empty curve: `min` 0, `max` 1, variance 0, three points
  (x, y, tangent x, tangent y, length): (0, 0.00138887, 0.337009, 0.941501, 0.132767),
  (0.493215, 0.998903, 1, 0.00102074, 0.23702), (1, −6.24607e−8, 0.307222, −0.951638,
  0.126974). A rounded arch, highest in the middle.
- **D. More undefined cases** (add to 5.6 D): a bud step of length / `segments` with
  `segments` 0 at level L + 1 (5.2 step 1). Infinity is acceptable; no game file reaches it.
- **E. Angle offsets are degrees in the table**; the game's vertex data carries them in
  radians (× π/180). Renderers convert.
- **F. The tree's bounding box** is the union of:
  - the branch vertices referenced by **LOD level 0's** strips (3.6), and the same for
    fronds (4.x level 0);
  - for every leaf of **leaf LOD level 0** only (merged leaves of later levels do not count):
    the six points Q ± d along x, y and z, where Q is the leaf's position and d its texture's
    *longest reach*: for a mesh texture, max over the mesh's vertices of |p| · max(w, h); for a
    card texture, the largest distance from the scaled pivot (ox·w, oy·h) to the card's four
    corners (0, 0), (w, 0), (w, h), (0, h). Here w, h are the texture's card size of 7.2
    without the LOD multiplier and (ox, oy) its `origin`, unmirrored.
  This replaces 5.6 F. With a placement's scale, half its diagonal is the game's LOD radius
  r (7.4).

## Stage 6 — Bushes, grass and cattails

There is no separate engine: `bsh_*`, `gra_*` and cattail files (`*_catgras`, `*_catflwr`) are
ordinary `.spt` trees grown by stages 1–5 with their own parameters. Facts from all 552 game
files:

- None stores precomputed leaf LODs, so leaves are always grown (5.2–5.4).
- Fronds of both kinds occur (`kind` 0 blades in about 380 files, 1 extrusions in about 150),
  usually with 1–4 frond images; 47 files have fronds disabled.
- Bushes and grass have one leaf image and one frond image in the texture-coordinate section
  and **no billboard image**, so at any distance they are drawn from their geometry (or not at
  all, stage 7).
- Placement scales are the same 0.28–0.34 as trees (format.md).

Expect most of a grass clump's visible surface to come from fronds and leaf cards rather than
branch tubes; a quick check that the generator is right is that a grass clump comes out at a
plausible size (well under a metre or two after the placement scale) without special cases.

## Stage 7 — Rendering

What the game does with the computed tree, traced in DAOrigins.exe. The tree computation is a
separate library the game calls; everything below is the game's side of that boundary, except
where it says the value comes from the computation.

### 7.1 How the game computes a tree

- **One computation per `.spt` file**, shared by every placement of that tree. Placements are
  drawn by shader constants (the RML transform), never by transforming geometry.
- **Inputs:** keep the file's seed (no seed override), no transform, the file's size and size
  variance (no override), texture flip **on**, leaf rocking on. Lighting and wind methods are
  the defaults (no vertex colours, shader wind). Six wind matrices, eight leaf angles.
- **Texture flip on** means every texture coordinate the computation hands out has its t
  negated: leaf cards (both choices), billboards and branch layers. Frond coordinates are
  negated twice on their path (4.6), so they are unchanged. With wrap addressing −t samples
  the same texel row as 1 − t, which is the v this spec gives throughout (format.md).
- After fronds, three more stream draws give the three rocking groups' time offsets:
  uniform(0, 10000) each, in group order. (The game does not use them; see 7.2.)

### 7.2 Leaf cards

Each leaf choice c (2 per texture, 5.3) has a card:

- Width w and height h = the `size` x and y of texture c / 2 times `TreeInfo::size` (2.3
  step 5, not the stored `size_used`), times the LOD size multiplier 1 +
  `leaf_size_increase` · k for leaf LOD k > 0.
- Pivot (ox, oy) = the texture's `origin`; for **even** choices ox becomes 1 − ox. (The game
  builds its cards from this table; the per-frame corner path that mirrors odd choices is
  never used.)
- Corners in card space (x is the facing axis, y right, z up), before billboarding:
  P1 = (0, (1 − ox)·w, (1 − oy)·h), P2 = (0, −ox·w, (1 − oy)·h), P3 = (0, −ox·w,
  −oy·h), P4 = (0, (1 − ox)·w, −oy·h).
- Texture corners: choice 2i uses the leaf image's four atlas corners as stored (format.md),
  choice 2i + 1 the same with the s values of corners 0↔1 and 2↔3 swapped (mirrored).
  Corner order matches P1…P4. (So an even choice has a mirrored pivot and an unmirrored image,
  and an odd choice the reverse. That is what the original does.)
- Angle offsets per card: a 30-entry table of degree values, in order
  6, −10, 16, −8, 2, −18, −14, 0, −4, 10, −6, 8, 4, −2, 14, −12, 14, −16, 4, −18, 13, 9,
  17, −9, 3, −7, −15, 7, −5, 15; with t = (c · 3) mod 30, a card's offsets are entry t and
  entry 29 − t.
- **Drawing (game side).** Each card leaf is six vertices (two triangles, corners 0, 1, 2 and
  0, 2, 3), all leaf LOD levels in one buffer. Per vertex: the leaf position; that corner's
  normal (5.3 step 9); the corner's atlas coordinate and corner index; the card's w, h and
  pivot; its two angle offsets in radians, a leaf-angle index and the leaf's colour scale;
  two wind matrix indices and weights. The vertex shader `spt_leafcards` turns the card to the
  camera from `g_vCameraAngles` (azimuth, pitch) plus the angle offsets, adds rocking
  (`g_vLeafAngles[index]` scaled by `g_vLeafAngleScalars`), then places the corner at
  position + ((pivot-adjusted corner) · (w, h)) in the turned frame.
- **Leaf-angle index.** The game ignores the computed rocking group. The index is the leaf's
  position in its LOD level's list, mod 8.
- `g_vLeafAngleScalars` = the computation's leaf rock and rustle scalars of leaf LOD 0.

### 7.3 Mesh leaves

DA:O uses them, though less than the flag suggests: 446 of 552 tree files set `use_meshes` on
at least one leaf texture, but only 164 files grow any leaves and 90 grow mesh leaves (the
grass files, for example, set the flag and grow no leaves).

- A leaf is a **mesh leaf** when its texture has `use_meshes`, its mesh index names a mesh in
  the file's mesh section (`SpeedTree::meshes`), and that mesh has more than 2 vertices.
  Otherwise it is a card (7.2).
- **Mesh per choice.** Each choice c of a mesh texture has its own copy of the texture's mesh:
  - positions = the mesh's vertex positions × max(w, h), with w and h the texture's card
    size of 7.2 **without** the LOD size multiplier (the same at every leaf LOD);
  - normals, binormals and tangents as stored;
  - texture coordinates: with (mu, mv) the mesh vertex's own coordinate and the texture's
    atlas corners (s₀, t₀) … (s₃, t₃) as stored: an even choice takes u = lerp(s₂, s₀, mu),
    an odd choice u = lerp(s₁, s₃, mu); both take v = 1 − lerp(t₂, t₀, mv) (flip, 7.1).
    A texture without an atlas entry uses its 0–1 image corners;
  - indices = the mesh's indices.
- **Placement.** A mesh vertex p of a leaf at Q lands at Q + p.x·T + p.y·B + p.z·N, where T,
  B and N are the leaf's corner-0 tangent, binormal and normal (5.3 steps 7–9: for mesh
  textures these come from n lerped toward up by `hang` and turned by `rotate`). Normals turn
  the same way. In the game this happens in the vertex shader `spt_leafmeshes`, after rocking
  the mesh about its own x axis by the leaf angle and before the tree's yaw, scale and
  position.
- **Drawing (game side).** Indexed, one list per leaf LOD level (each leaf's mesh indices plus
  a running vertex base). Per vertex: mesh position and normal; the leaf's T, B, N as three
  rows; the leaf position; the mesh texture coordinate, leaf-angle index and colour scale;
  wind indices and weights.

### 7.4 LOD and fades

The game ignores the file's `near` and `far` and computes its own per placement:

- r = half the diagonal of the computed tree's bounding box × the placement scale;
  t = (clamp(r, 2, 30) − 2) / 28; near = (15 + 5·t) · M, far = (35 + 85·t) · M, with
  M from a three-entry table indexed by the placement's LOD class (RML field 3379, a signed
  byte): class 0 → 1, class 1 → 1.5, class 2 → 2. Every Korcari Wilds placement has class 0.
  Other values were not seen and how the game treats them is not traced; clamping to 0–2 is
  an acceptable choice.
- Each frame, with d² the squared distance from the camera to the placement point (not the
  box centre) and G = (detail scale × zoom)² (detail scale 0.5, 1.0, 1.2 or 1.5 by the
  graphics detail option; zoom = min(60° / field of view, 3) in the game's zoomable camera
  modes, else 1):
  **LOD = 1 − clamp((d² − near²·G) / (far²·G − near²·G), 0, 1).** It is 1 inside near and 0
  beyond far, and not linear in distance.
- LOD > 0.001 draws geometry; LOD ≤ 0 draws only the billboard, fully opaque.
- The computation maps the LOD value to a discrete level and an alpha (0–255) for branches,
  fronds, two leaf slots and the billboard: discrete level = (int)((1 − LOD) · N), minus one
  if it equals N. When a billboard image exists, leaves have N + 1 levels and the extra one
  means "billboard". The fade hints (3.7, 4.6) and the transition settings
  (`Lod::leaf_transition`, `leaf_transition_radius`, `billboard_transition`) feed those
  alphas; how the computation turns them into alphas is not yet specified here.
- **Fades are alpha tests, never blending.** Branches switch levels without a fade. Fronds
  test against their alpha. Leaves draw two LOD levels at once (the two slots), each with its
  own alpha reference, which cross-fades them. The game uses the frond level minus one,
  clamped at 0.
- **Whole-tree fade** for camera occlusion: a multiplier on the output alpha, at least 0.12,
  for trees that have collision.

### 7.5 Billboards

- Bushes and grass get **no** billboard: the game leaves out every model whose name contains
  `_bsh_` or `_gra_`, so they simply stop drawing beyond their far distance.
- Other trees draw, beyond their last level, a camera-facing quad textured with their image
  from `<level>_diffuse_billboards.dds` (one image per tree in DA:O even when the file says
  360° billboards). Its size comes from the computed tree's bounding box (5.7 F): the
  width is twice the largest horizontal distance from the origin to a corner of the box (in
  x and y), the height is the box's **top** (its largest z, measured from the tree's origin,
  not the box's own height).
- Its corners are, in card space as in 7.2 (y right, z up, the quad standing on the
  origin): corner 0 = (+½·width, height), 1 = (−½·width, height), 2 = (−½·width, 0),
  3 = (+½·width, 0). The billboard image's four stored corners (format.md) pair with these in
  order, the same convention as the leaf images and P1…P4.
- The game batches all billboards of a level into one buffer, four vertices per placement,
  each with the placement position and corner index, the billboard's width and height and a
  per-frame fade alpha, the image's atlas entry and the placement's yaw, scale and colour.
  The fade is an alpha test (greater than 84) on texture alpha scaled by the fade.

### 7.6 Branch and frond vertices, shader constants, wind

- **Branch and frond vertices** share one layout: position, normal, binormal, tangent (half
  floats), two wind matrix indices and weights, then the texture layers present (diffuse,
  normal map, detail, and the self-shadow layer). `spt_branch` reads position, normal,
  tangent, wind and the diffuse coordinate (it rebuilds the binormal); `spt_fronds` reads
  position, normal, wind and the coordinate.
- **Per-placement constants:** `g_vTreePos` (the RML position through the room's
  transform), `g_fTreeScale` (RML 3356), `g_vRotationTrigValues` (sin θ, cos θ, −sin θ, 0)
  for the yaw θ of the placement's rotation (trees are never tilted), `g_fWindMatrixOffset`
  (a random integer 0–99 per placement, rotating which of the six wind matrices it uses),
  `g_vColorMultiplier` (RML 3375, used as the ambient light added to N·L × sun colour), and
  the sun's `g_vLightDir` and `g_vLightDiffuse`.
- **Wind** is global per level: strength and direction come from the level's
  `<level>_<n>.wnd`, with no gusts in any shipped level. Five copies of the wind run 0.2 s
  apart and placements take them in turn. Held still (identity matrices, zero leaf angles),
  the shapes are unchanged. The wind weights and matrix indices the computation assigns to
  vertices and leaves are not yet specified here.

### 7.10 Clarifications (round 6)

1. **N_l in 7.8 is the leaf LOD count** of 5.4 (the file's `leaf_lod_count`), without an extra
   billboard level. (7.4's "N + 1 levels" describes the billboard as a further step beyond
   the last level; the computation's constants use N_l itself.)
2. **ℓ′ = ℓ − τ uses the full width τ**, not τ′. The asymmetry is the original's.
3. **Billboard alpha** is 0 wherever 7.8 does not set it (ℓ ≥ 1, and the middle range
   outside the shortened width).
4. **`leaf_transition` absent** means smooth (1).
5. **Defaults when absent:** `TreeInfo::seed` 1200, `flare_seed` 444. The trunk's groups and
   the round-robin counter use the stored values (7.7), not the ≤ 1 rule.
6. **Extrusion fronds:** a vertex built at guide vertex i takes guide vertex i's wind values,
   even though its frame also uses i − 1.
7. **Self-shadow coordinates (7.9):** c = (min + max)/2 per component, R = half the
   Euclidean diagonal. A zero-size box divides 0 by 0 in the original (undefined); 0 is an
   acceptable choice. A texture-coordinate section without a shadow entry maps through the
   default k of 7.9; a projection without a texture-coordinate section gives unmapped
   coordinates; no projection, no coordinates.
8. **An empty branch LOD 0** (`branch_max_volume_percent` 0) contributes nothing to the
   bounding box; the original reads only level 0's strips and does not fall back to other
   branch geometry. The box then comes from fronds and leaves.
9. **Mesh-leaf reach (5.7 F)** uses the mesh named by the texture's own mesh entry
   (`LeafMeshUse::mesh` of texture choice / 2), the same mesh 7.3 draws.
10. **`flare_seed` absent** seeds the flare generator with 444 (item 5).
11. **The two atlas mappings differ in the original too.** Fronds read corners 0 and 2 for s
    (lerp(c[2], c[0], s)) and t from corners 2 and 0; mesh leaves read the choice's card
    corners as 7.3 gives them. Keep both as written; they agree for axis-aligned atlas
    entries, which is every game file's.

### 7.7 Wind weights and matrix indices (the computation)

Every branch and frond vertex and every leaf carries two wind pairs, (weight 1, matrix index
1) and (weight 2, matrix index 2). 7.6 and 7.2–7.3 say how the game uses them.

- **Inputs.** The weighting level W = `wind_level` (1 when absent) and the propagate flag
  = `propagate_flexibility` (false when absent).
- **Groups (round robin).** A counter starts at the tree seed **as stored** (`TreeInfo::seed`;
  the "≤ 1 becomes 1" rule of 2.3 step 1 applies only inside the random stream, not here).
  The trunk is grown with groups (g₁, g₂) = (that seed, `flare_seed` as stored) and weights
  (w₁, w₂) = (1, 1). When a branch starts growing (2.4), right after its frond test (a pruned
  branch takes nothing): if its level L equals W, g₁ ← counter and the counter goes up by
  one; if L equals W + 1, g₂ ← counter and it goes up by one. The counter is shared by the
  whole tree and only increases, in growth order. Roots go through the same rule at their
  level.
- **Weights per skeleton vertex.** For skeleton vertex i of the branch, with t its progress
  (0 for the first vertex, else the segment-packed progress of 2.4) and f its vertex
  flexibility (2.4: `flexibility` · `flexibility_scale` at t), start from the weights the
  branch was grown with and set:
  - L < W: w₁ = 1, w₂ = 1;
  - L = W: w₁ = 1 − t·f, w₂ = 1;
  - L = W + 1: w₂ = 1 − t·f (w₁ as received);
  - L > W + 1 with the propagate flag: w₂ = lerp(w₂, 0, f) (from the received w₂, not the
    previous vertex's);
  - otherwise both as received.
- **Inheritance.** A child branch or bud (2.5) receives the weights interpolated between the
  parent's skeleton vertices j and j + 1 at s (2.5 step 6), and the parent's groups as
  assigned above.
- **Where they land.** Every tube vertex of a cross section takes its skeleton vertex's
  values. Every frond vertex takes the values of the guide vertex it is built from (4.1
  records them on the guide). A leaf takes its bud's values, its weights clamped to [0, 1];
  a merged leaf of a later LOD level (5.4) copies leaf i's.
- **Output.** weight = clamp(1 − w, 0, 1) (0 rigid, 1 free); matrix index = (g as an
  unsigned byte, that is g mod 256) mod N, with N the number of wind matrices, 6 in DA:O
  (7.1). Leaves from the file's own leaf LODs (stage 5) carry weight 1 as a byte /255 and
  group 1 as stored; weight 2 and group 2 are 0.

### 7.8 LOD value to levels and alphas (the computation)

Given the LOD value ℓ of 7.4, the computation returns, for branches, fronds, two leaf slots
and the billboard, a level (−1 = draw nothing) and an alpha-test value on a 0–255 scale.
The game uses them as 7.4 describes.

- **Constants.** Target T = 84; clear C = 255. Leaf transition method = `Lod::leaf_transition`
  (0 pop, 1 smooth, 3 none; 2 is read as smooth with transition factor 0.5); radius ρ =
  `leaf_transition_radius` (0.07 when absent); exponent e = `leaf_curve_exponent` (0.7);
  transition factor φ = the file's `leaf_transition_factor` (0). With N_b, N_f, N_l the
  branch, frond and leaf level counts: width τ = 2ρ; shortened width τ′ = τ·(1 − φ);
  transitions k = N_l − 1; crest c = 1 if k = 0 else (1 − (k + 1)·τ)/k; cycle λ = c + τ.
  (These are the billboard forms; DA:O always drops to billboards, 7.1.)
- **Curve.** rise(a, b, p) = lerp(b, a, (1 − p)^e); fall(a, b, p) = lerp(a, b, p^e).
- **Branches and fronds.**
  - ℓ ≤ 0: levels −1, billboard alpha 1.
  - ℓ ≥ 1: levels 0, alpha T.
  - Otherwise: level = (int)((1 − ℓ)·N) for each; if ℓ < τ′: alpha = rise(C, T, ℓ/τ′) and
    billboard alpha = min(1.5·(1 − ℓ/τ′), 1); else alpha = T (billboard alpha left at 0).
- **Leaves** (slot 1 defaults to level −1, alpha C).
  - ℓ ≤ 0: slot 0 level −1, alpha C.
  - ℓ ≥ 1: slot 0 level 0, alpha T.
  - Smooth, ℓ < τ′: slot 0 alpha = rise(C, T, ℓ/τ′), level −1 if that equals C exactly,
    else N_l − 1.
  - Smooth, otherwise: ℓ′ = ℓ − τ; u = ℓ′ mod λ (C-style floating remainder: it keeps the
    sign of ℓ′, which can be negative when φ > 0); region =
    max(N_l − (int)(ℓ′/λ) − 1, 0) (truncating toward zero); slot 0 level = region. If u < c: slot 0 alpha = T.
    Otherwise p = (u − c)/τ; slot 1 alpha = fall(T, C, (p − φ)/(1 − φ)) if p > φ, else T;
    slot 0 alpha = rise(C, T, p/(1 − φ)) if p < 1 − φ, else T; slot 0 level = region − 1,
    slot 1 level = region.
  - None: slot 0 level 0, alpha T. Pop: slot 0 level (int)((1 − ℓ)·N_l), alpha T.
- **Fade hints** (3.7, 4.6) are not used here; they are for renderers that fade per vertex.
  `billboard_transition` affects only 360° billboard images, which DA:O does not use.

### 7.9 Self-shadow coordinates

Only when the file has a shadow projection section (right, up and out vectors, and the
self-shadow map). DA:O's shaders never sample this layer (7.6), so it can be left out.

- From the bounding box (5.7 F): centre c, radius R = half its diagonal. With r the stored
  right vector and u the stored up vector (not normalised for the end points): for each
  branch and frond vertex v (all of them, in order), s = |(v − (c − r·R)) · r̂| / 2R and
  t = |(v − (c − u·R)) · û| / 2R, with r̂, û the normalised vectors.
- If the file has a texture-coordinate section, map through its `shadow` entry
  (corners k[0..7]): s ← lerp(k[2], k[0], s), t ← lerp(k[5], k[1], t). A section without a
  shadow entry uses k = (1, 1, 0, 1, 0, 0, 1, 0).
- With texture flip on (7.1), t is negated, as for the other layers.
