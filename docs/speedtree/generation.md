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

Stage 2 and later: in progress. Do not start them until this note is replaced.
