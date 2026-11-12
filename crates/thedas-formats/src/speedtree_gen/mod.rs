//! Growing SpeedTree 4 trees from their `.spt` descriptions, as the game does at load time.
//!
//! A clean-room implementation of `docs/speedtree/generation.md`: this code is written from
//! that document only. Everything here is provisional until checked against the executable,
//! except where a doc comment says otherwise.
//!
//! Stage 1 (this file): the single random stream and the curve parameters. Stage 2
//! ([`skeleton`]): tree set-up and the branch skeleton, on the rotation frames of [`frame`].
//! Stage 3 ([`geometry`]): branch tubes and branch LOD. Stage 4 ([`fronds`]): fronds. Stage 5
//! ([`leaves`]): leaves. [`generate`] runs them all in the order of 5.1; [`mesh::TreeMesh`]
//! flattens the result for drawing (stage 7).

pub mod frame;
pub mod fronds;
pub mod geometry;
pub mod leaves;
pub mod mesh;
pub mod skeleton;

pub use skeleton::{Branch, Bud, Flare, Guide, Skeleton, Vertex, grow};

use crate::speedtree::{BezierSpline, BranchLevel, SpeedTree};

/// Modulus of the Lehmer generator, 2³¹ − 1.
const MODULUS: i64 = 2_147_483_647;
/// Multiplier of the Lehmer generator.
const MULTIPLIER: i64 = 16_807;
/// Schrage's decomposition of the modulus: `MODULUS = MULTIPLIER · Q + R`.
const Q: i64 = 127_773;
const R: i64 = 2_836;
/// Scale from a seed to a float in (0, 1), as the spec writes it (it rounds to 2⁻³¹).
#[allow(clippy::excessive_precision)]
const SCALE: f32 = 4.656_612_875e-10;
/// Entries of the shuffle table.
const SHUFFLE: usize = 128;

/// The generation random stream: a Park–Miller Lehmer generator behind a 128-entry
/// Bays–Durham shuffle table (generation.md 1.1).
///
/// The original keeps one process-wide state that every "random" object draws from; here the
/// one stream is passed explicitly, so every draw of a tree goes through the same value and
/// the order of draws is the order of calls.
#[derive(Clone, Debug)]
pub struct Random {
    /// Always integral and in `1..MODULUS` after a step.
    seed: i64,
    table: [f32; SHUFFLE],
    /// Values drawn since the stream was made (for tests and diagnostics).
    draws: u64,
}

impl Random {
    /// A stream seeded with `seed` (values ≤ 1 seed with 1), its shuffle table filled.
    pub fn new(seed: i32) -> Self {
        let mut random = Random {
            seed: 1,
            table: [0.0; SHUFFLE],
            draws: 0,
        };
        random.reseed(seed);
        random
    }

    /// Reseeds the stream: `seed` (or 1 if it is ≤ 1), then refills the shuffle table in
    /// order with one raw step per entry.
    pub fn reseed(&mut self, seed: i32) {
        self.seed = i64::from(seed.max(1));
        for i in 0..SHUFFLE {
            self.table[i] = self.raw_step();
        }
    }

    /// One step of the Lehmer generator, as a float in (0, 1].
    fn raw_step(&mut self) -> f32 {
        let hi = self.seed / Q;
        let lo = self.seed - hi * Q;
        self.seed = MULTIPLIER * lo - R * hi;
        if self.seed <= 0 {
            self.seed += MODULUS;
        }
        self.seed as f32 * SCALE
    }

    /// The next value of the stream (what every uniform draw uses): picks a table entry with
    /// a raw step, returns it, and refills it with another raw step.
    pub fn draw(&mut self) -> f32 {
        // A seed within 64 of the modulus rounds to 2³¹ as a float, giving exactly 1.0 and an
        // index of 128. The spec states 0 ≤ i < 128; clamp until the executable's behaviour
        // in that case is known (provisional).
        let i = ((self.raw_step() * SHUFFLE as f32) as usize).min(SHUFFLE - 1);
        let value = self.table[i];
        self.table[i] = self.raw_step();
        self.draws += 1;
        value
    }

    /// A uniform draw in `[a, b]`: `a + (b − a) · draw()`.
    pub fn uniform(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * self.draw()
    }

    /// How many values have been drawn (reseeding does not reset it).
    pub fn draws(&self) -> u64 {
        self.draws
    }

    /// The generator's integer state, for tests and diagnostics.
    pub fn state(&self) -> i64 {
        self.seed
    }
}

/// Samples in a curve's table (generation.md 1.2).
pub const CURVE_SAMPLES: usize = 500;
/// Index of the last table sample.
const LAST: usize = CURVE_SAMPLES - 1;
/// Width of one table step in the parameter, `1/499`.
const STEP: f32 = 1.0 / LAST as f32;

/// A curve parameter resampled to its fixed table (generation.md 1.2).
#[derive(Clone, Debug, PartialEq)]
pub struct Curve {
    /// The stored minimum (not the curve's extreme).
    pub min: f32,
    /// The stored maximum.
    pub max: f32,
    pub variance: f32,
    /// Normalized curve values; all zeros for a curve of fewer than two control points.
    table: Box<[f32; CURVE_SAMPLES]>,
}

/// A 2-D point of the curve under construction.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Point {
    x: f32,
    y: f32,
}

fn lerp(a: f32, b: f32, u: f32) -> f32 {
    a + (b - a) * u
}

impl Curve {
    /// Builds a curve's table from its control points.
    pub fn new(spline: &BezierSpline) -> Self {
        let mut table = Box::new([0.0; CURVE_SAMPLES]);
        let points = &spline.points;
        if points.len() >= 2 {
            let raw = raw_samples(spline);
            let first = points[0].y;
            let last = points[points.len() - 1].y;
            table[0] = first;
            table[LAST] = last;
            let mut cursor = 0;
            for (i, entry) in table.iter_mut().enumerate().take(LAST).skip(1) {
                let p = i as f32 / CURVE_SAMPLES as f32;
                while cursor + 2 < raw.len() && !(raw[cursor].x <= p && p < raw[cursor + 1].x) {
                    cursor += 1;
                }
                let (a, b) = match (raw.get(cursor), raw.get(cursor + 1)) {
                    (Some(&a), Some(&b)) => (a, b),
                    (Some(&a), None) => (a, a),
                    _ => (Point { x: 0.0, y: first }, Point { x: 0.0, y: first }),
                };
                let span = b.x - a.x;
                // Equal x only arises at the fallback last pair of a malformed curve, where
                // the original would divide by zero; take the earlier point (provisional).
                *entry = if span != 0.0 {
                    lerp(a.y, b.y, (p - a.x) / span)
                } else {
                    a.y
                };
            }
        }
        Curve {
            min: spline.min,
            max: spline.max,
            variance: spline.variance,
            table,
        }
    }

    /// The normalized table values.
    pub fn table(&self) -> &[f32; CURVE_SAMPLES] {
        &self.table
    }

    /// The normalized value at `p`, linearly interpolated between table entries.
    pub fn normalized(&self, p: f32) -> f32 {
        // Parameters outside [0, 1] would index outside the table; clamp (provisional).
        let k = ((p * LAST as f32) as i64).clamp(0, LAST as i64) as usize;
        if k == LAST {
            self.table[LAST]
        } else {
            let f = (p - k as f32 * STEP) / STEP;
            self.table[k] + (self.table[k + 1] - self.table[k]) * f
        }
    }

    /// Maps a normalized value into `min..max`.
    fn map(&self, y: f32) -> f32 {
        y * (self.max - self.min) + self.min
    }

    /// The value at `p` without variance. No draw.
    pub fn value(&self, p: f32) -> f32 {
        self.map(self.normalized(p))
    }

    /// The value at `p` plus `uniform(−variance, +variance)`. One draw.
    pub fn value_with_variance(&self, p: f32, random: &mut Random) -> f32 {
        let value = self.value(p);
        value + random.uniform(-self.variance, self.variance)
    }

    /// The value at `p` plus `y · uniform(−variance, +variance)`, `y` the normalized value.
    /// One draw.
    pub fn value_with_scaled_variance(&self, p: f32, random: &mut Random) -> f32 {
        let y = self.normalized(p);
        self.map(y) + y * random.uniform(-self.variance, self.variance)
    }

    /// A pure random offset scaled by the curve ("disturbance"-type parameters): the table
    /// entry nearest `p` (no interpolation) as `y`, then `uniform(−variance·y, +variance·y)`.
    /// One draw.
    pub fn scaled_variance(&self, p: f32, random: &mut Random) -> f32 {
        let k = ((p * LAST as f32 + 0.5) as i64).clamp(0, LAST as i64) as usize;
        let y = self.table[k];
        random.uniform(-self.variance * y, self.variance * y)
    }
}

/// Step 1 of resampling: walks the Bézier segments with a running fraction, producing up to
/// [`CURVE_SAMPLES`] points, then the last control point if fewer were produced.
fn raw_samples(spline: &BezierSpline) -> Vec<Point> {
    let points = &spline.points;
    let n = points.len();
    // Control points with normalized tangents.
    let controls: Vec<(Point, Point)> = points
        .iter()
        .map(|p| {
            let [tx, ty] = p.tangent;
            let length = (tx * tx + ty * ty).sqrt();
            // A zero tangent has no direction; leave it zero (provisional).
            let t = if length > 0.0 {
                Point {
                    x: tx / length,
                    y: ty / length,
                }
            } else {
                Point { x: 0.0, y: 0.0 }
            };
            (
                Point { x: p.x, y: p.y },
                Point {
                    x: t.x * p.tangent_length,
                    y: t.y * p.tangent_length,
                },
            )
        })
        .collect();
    let step = (n - 1) as f32 / LAST as f32;
    let mut raw = Vec::with_capacity(CURVE_SAMPLES);
    let mut segment = 0;
    let mut u = 0.0f32;
    while segment + 1 < n && raw.len() < CURVE_SAMPLES {
        if u < 1.0 {
            let (p0, h0) = controls[segment];
            let (p3, h3) = controls[segment + 1];
            let p1 = Point {
                x: p0.x + h0.x,
                y: p0.y + h0.y,
            };
            let p2 = Point {
                x: p3.x - h3.x,
                y: p3.y - h3.y,
            };
            raw.push(Point {
                x: bezier(p0.x, p1.x, p2.x, p3.x, u),
                y: bezier(p0.y, p1.y, p2.y, p3.y, u),
            });
            u += step;
        } else {
            segment += 1;
            u -= 1.0;
        }
    }
    if raw.len() < CURVE_SAMPLES {
        let last = points[n - 1];
        raw.push(Point {
            x: last.x,
            y: last.y,
        });
    }
    raw
}

/// A cubic Bézier coordinate at `u` by de Casteljau's repeated linear interpolation.
fn bezier(a: f32, b: f32, c: f32, d: f32, u: f32) -> f32 {
    let ab = lerp(a, b, u);
    let bc = lerp(b, c, u);
    let cd = lerp(c, d, u);
    let abc = lerp(ab, bc, u);
    let bcd = lerp(bc, cd, u);
    lerp(abc, bcd, u)
}

/// A fully computed tree (stages 2–5 and the post-generation draws of 7.1).
#[derive(Clone, Debug)]
pub struct GeneratedTree {
    pub skeleton: Skeleton,
    pub branches: geometry::BranchGeometry,
    /// Leaf LOD levels (5.4); level 0 is `skeleton.leaves`.
    pub leaf_lods: Vec<Vec<leaves::Leaf>>,
    pub fronds: fronds::FrondGeometry,
    /// The three rocking groups' time offsets (7.1).
    pub rocking_offsets: [f32; 3],
}

/// Computes a tree from its description, keeping the order of random draws of 5.1: growth
/// (branches, guides, buds and leaves), branch LOD, leaf LOD, fronds, then the rocking
/// groups' offsets.
pub fn generate(tree: &SpeedTree) -> GeneratedTree {
    let (skeleton, mut random) = grow(tree);
    let branches = geometry::build(tree, &skeleton, &mut random);
    let leaf_lods = leaves::lods(tree, &skeleton.leaves, &mut random);
    let fronds = fronds::build(tree, &skeleton, &mut random);
    let rocking_offsets = [
        random.uniform(0.0, 10_000.0),
        random.uniform(0.0, 10_000.0),
        random.uniform(0.0, 10_000.0),
    ];
    GeneratedTree {
        skeleton,
        branches,
        leaf_lods,
        fronds,
        rocking_offsets,
    }
}

/// Every curve of a tree with where it sits, for checks over the game's files.
pub fn curves(tree: &SpeedTree) -> Vec<(String, &BezierSpline)> {
    fn level<'t>(out: &mut Vec<(String, &'t BezierSpline)>, name: &str, level: &'t BranchLevel) {
        let fields = [
            ("disturbance", &level.disturbance),
            ("gravity", &level.gravity),
            ("flexibility", &level.flexibility),
            ("flexibility_scale", &level.flexibility_scale),
            ("length", &level.length),
            ("radius", &level.radius),
            ("radius_scale", &level.radius_scale),
            ("start_angle", &level.start_angle),
            ("angle_profile", &level.angle_profile),
            ("cross_section_profile", &level.cross_section_profile),
            ("light_seam_profile", &level.light_seam_profile),
            ("roughness_profile", &level.roughness_profile),
            ("frequency_profile", &level.frequency_profile),
            ("light_seam_bias", &level.light_seam_bias),
            ("gnarl_profile", &level.gnarl_profile),
        ];
        for (field, spline) in fields {
            if let Some(spline) = spline {
                out.push((format!("{name}.{field}"), spline));
            }
        }
    }
    let mut out = Vec::new();
    for (i, branch) in tree.branch_levels.iter().enumerate() {
        level(&mut out, &format!("branch level {i}"), branch);
    }
    if let Some(roots) = &tree.roots {
        level(&mut out, "roots", &roots.roots);
    }
    if let Some(profile) = tree.fronds.as_ref().and_then(|f| f.profile.as_ref()) {
        out.push(("fronds.profile".into(), profile));
    }
    out
}

/// Builds every curve of a tree and computes the whole tree, checking every value is finite
/// and every strip indexes real vertices (a corpus check).
pub fn check(tree: &SpeedTree) -> Result<(), String> {
    for (name, spline) in curves(tree) {
        let curve = Curve::new(spline);
        if let Some(i) = curve.table().iter().position(|y| !y.is_finite()) {
            return Err(format!("{name}: table entry {i} is {}", curve.table()[i]));
        }
    }
    let mesh = mesh::TreeMesh::new(tree);
    for (name, part) in [("branch", &mesh.branches), ("frond", &mesh.fronds)] {
        let count = part.vertices.len() as u32;
        if part.lods.iter().flatten().any(|&i| i >= count) {
            return Err(format!("a {name} triangle list indexes past the vertices"));
        }
    }
    if let Some(b) = mesh.billboard
        && !(b.width.is_finite() && b.height.is_finite())
    {
        return Err("the billboard size is not finite".into());
    }
    let generated = &mesh.generated;
    let skeleton = &generated.skeleton;
    for (i, branch) in skeleton.branches.iter().enumerate() {
        let finite = branch.vertices.iter().all(|v| {
            v.position.iter().all(|x| x.is_finite()) && v.radius.is_finite() && v.length.is_finite()
        });
        if !finite {
            return Err(format!("branch {i} (level {}) is not finite", branch.level));
        }
    }
    let geometry = &generated.branches;
    let finite = |v: &[f32; 3]| v.iter().all(|x| x.is_finite());
    if let Some(i) = geometry.vertices.iter().position(|v| {
        !(finite(&v.position)
            && finite(&v.normal)
            && v.texcoords.iter().flatten().all(|x| x.is_finite()))
    }) {
        return Err(format!("branch vertex {i} is not finite"));
    }
    if let Some(i) = skeleton.leaves.iter().position(|l| {
        !(finite(&l.position)
            && l.corners
                .iter()
                .all(|c| finite(&c.normal) && finite(&c.color)))
    }) {
        return Err(format!("leaf {i} is not finite"));
    }
    let fronds = &generated.fronds;
    if let Some(i) = fronds.vertices.iter().position(|v| {
        !(finite(&v.position) && finite(&v.normal) && v.texcoord.iter().all(|x| x.is_finite()))
    }) {
        return Err(format!("frond vertex {i} is not finite"));
    }
    let frond_count = fronds.vertices.len() as u32;
    if fronds
        .strips
        .iter()
        .flatten()
        .flatten()
        .any(|&i| i >= frond_count)
    {
        return Err("a frond strip indexes past the vertices".into());
    }
    let count = geometry.vertices.len() as u32;
    for tube in geometry.tubes.iter().flatten() {
        if tube.strip.iter().any(|&i| i >= count) {
            return Err(format!(
                "branch strip at {} indexes past the vertices",
                tube.start
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::speedtree::SplinePoint;

    /// The reference Lehmer sequence from seed 1, computed directly with 64-bit arithmetic.
    fn lehmer(seed: i64) -> i64 {
        seed * MULTIPLIER % MODULUS
    }

    #[test]
    fn raw_steps_follow_the_minimal_standard() {
        let random = Random::new(1);
        // The table fill took 128 steps from seed 1.
        let mut reference = 1;
        for _ in 0..SHUFFLE {
            reference = lehmer(reference);
        }
        assert_eq!(random.state(), reference);
        // Park and Miller's published check: seed 1 reaches 1043618065 after 10000 steps.
        let mut random = Random {
            seed: 1,
            table: [0.0; SHUFFLE],
            draws: 0,
        };
        assert_eq!(random.raw_step(), 16807.0 * SCALE);
        for _ in 1..10_000 {
            random.raw_step();
        }
        assert_eq!(random.state(), 1_043_618_065);
    }

    #[test]
    fn schrage_matches_direct_arithmetic_near_the_extremes() {
        for seed in [1, 2, Q - 1, Q, Q + 1, MODULUS / 2, MODULUS - 2, MODULUS - 1] {
            let mut random = Random {
                seed,
                table: [0.0; SHUFFLE],
                draws: 0,
            };
            random.raw_step();
            assert_eq!(random.state(), lehmer(seed), "seed {seed}");
        }
    }

    #[test]
    fn seeds_at_or_below_one_seed_with_one() {
        let one = Random::new(1);
        for seed in [0, -5, i32::MIN] {
            let other = Random::new(seed);
            assert_eq!(other.state(), one.state());
            assert_eq!(other.table, one.table);
        }
    }

    #[test]
    fn next_returns_the_picked_entry_and_refills_it() {
        let mut random = Random::new(1200);
        let before = random.clone();
        let value = random.draw();
        // Replay by hand: one step picks the index, one step refills it.
        let mut replay = before.clone();
        let i = (replay.raw_step() * 128.0) as usize;
        let refill = replay.raw_step();
        assert_eq!(value, before.table[i]);
        assert_eq!(random.table[i], refill);
        assert_eq!(random.state(), replay.state());
        for (j, (&a, &b)) in random.table.iter().zip(&before.table).enumerate() {
            if j != i {
                assert_eq!(a, b);
            }
        }
    }

    #[test]
    fn uniform_spans_its_range() {
        let mut random = Random::new(42);
        for _ in 0..10_000 {
            let v = random.uniform(-3.0, 5.0);
            assert!((-3.0..=5.0).contains(&v), "{v}");
        }
        let mut a = Random::new(42);
        let mut b = Random::new(42);
        assert_eq!(a.uniform(2.0, 4.0), 2.0 + 2.0 * b.draw());
    }

    #[test]
    fn index_at_exactly_one_stays_in_the_table() {
        // A seed whose next step rounds to 2³¹ as a float.
        let mut random = Random::new(1);
        let mut probe = random.clone();
        probe.seed = 411_174_057; // 16807 · 411174057 ≡ 2147483600 (mod 2³¹ − 1)
        assert_eq!(probe.raw_step(), 1.0);
        random.seed = 411_174_057;
        random.draw();
    }

    fn point(x: f32, y: f32, tangent: [f32; 2], tangent_length: f32) -> SplinePoint {
        SplinePoint {
            x,
            y,
            tangent,
            tangent_length,
        }
    }

    fn spline(min: f32, max: f32, variance: f32, points: Vec<SplinePoint>) -> BezierSpline {
        BezierSpline {
            min,
            max,
            variance,
            points,
        }
    }

    /// A straight line from (0, 0) to (1, 1) with handles along it.
    fn diagonal() -> BezierSpline {
        spline(
            0.0,
            1.0,
            0.0,
            vec![
                point(0.0, 0.0, [1.0, 1.0], 0.3),
                point(1.0, 1.0, [2.0, 2.0], 0.3),
            ],
        )
    }

    #[test]
    fn a_constant_curve_maps_into_min_max() {
        let curve = Curve::new(&spline(
            10.0,
            20.0,
            0.0,
            vec![
                point(0.0, 0.5, [1.0, 0.0], 0.2),
                point(1.0, 0.5, [1.0, 0.0], 0.2),
            ],
        ));
        assert!(curve.table().iter().all(|&y| (y - 0.5).abs() < 1e-6));
        for p in [0.0, 0.25, 0.5, 0.999, 1.0] {
            assert!((curve.value(p) - 15.0).abs() < 1e-4, "{p}");
        }
    }

    #[test]
    fn the_table_samples_at_i_over_500() {
        let curve = Curve::new(&diagonal());
        let table = curve.table();
        assert_eq!(table[0], 0.0);
        assert_eq!(table[LAST], 1.0);
        // On y = x the table holds its targets i/500, not i/499.
        for (i, &y) in table.iter().enumerate().take(LAST).skip(1) {
            assert!((y - i as f32 / 500.0).abs() < 1e-5, "{i}: {y}");
        }
    }

    #[test]
    fn evaluation_interpolates_table_entries() {
        let curve = Curve::new(&diagonal());
        let table = curve.table();
        assert_eq!(curve.normalized(1.0), table[LAST]);
        assert_eq!(curve.normalized(0.0), table[0]);
        let p = 10.5 / 499.0;
        let mid = (table[10] + table[11]) / 2.0;
        assert!((curve.normalized(p) - mid).abs() < 1e-6);
        // Out of range parameters stay in the table.
        assert_eq!(curve.normalized(2.0), table[LAST]);
        assert!(curve.normalized(-1.0).is_finite());
    }

    #[test]
    fn a_short_curve_is_all_zeros() {
        let curve = Curve::new(&spline(
            3.0,
            7.0,
            0.0,
            vec![point(0.0, 0.9, [1.0, 0.0], 1.0)],
        ));
        assert!(curve.table().iter().all(|&y| y == 0.0));
        assert_eq!(curve.value(0.5), 3.0);
        let empty = Curve::new(&spline(3.0, 7.0, 0.0, vec![]));
        assert_eq!(empty.value(0.5), 3.0);
    }

    #[test]
    fn multi_segment_curves_hit_their_control_points() {
        // Up from 0 to 1 at x = 0.5 and back down, flat tangents at each point.
        let curve = Curve::new(&spline(
            0.0,
            2.0,
            0.0,
            vec![
                point(0.0, 0.0, [1.0, 0.0], 0.1),
                point(0.5, 1.0, [1.0, 0.0], 0.1),
                point(1.0, 0.0, [1.0, 0.0], 0.1),
            ],
        ));
        assert!((curve.value(0.5) - 2.0).abs() < 0.01);
        assert!(curve.value(0.25) > 0.5 && curve.value(0.75) > 0.5);
        assert!(curve.table().iter().all(|y| (0.0..=1.0).contains(y)));
    }

    #[test]
    fn degenerate_curves_stay_finite() {
        for points in [
            vec![
                point(0.0, 0.0, [0.0, 0.0], 1.0),
                point(1.0, 1.0, [0.0, 0.0], 1.0),
            ],
            vec![
                point(0.0, 0.0, [1.0, 0.0], 0.0),
                point(0.0, 1.0, [1.0, 0.0], 0.0),
            ],
            vec![
                point(0.5, 0.2, [1.0, 0.0], 0.0),
                point(0.5, 0.2, [1.0, 0.0], 0.0),
            ],
        ] {
            let curve = Curve::new(&spline(0.0, 1.0, 0.5, points));
            assert!(curve.table().iter().all(|y| y.is_finite()));
        }
    }

    #[test]
    fn variance_flavours_draw_once_each() {
        let curve = Curve::new(&spline(
            0.0,
            10.0,
            2.0,
            vec![
                point(0.0, 0.5, [1.0, 0.0], 0.2),
                point(1.0, 0.5, [1.0, 0.0], 0.2),
            ],
        ));
        let mut random = Random::new(7);
        let mut shadow = Random::new(7);

        let v = curve.value_with_variance(0.3, &mut random);
        let d = shadow.uniform(-2.0, 2.0);
        assert_eq!(v, curve.value(0.3) + d);

        let v = curve.value_with_scaled_variance(0.3, &mut random);
        let y = curve.normalized(0.3);
        let d = shadow.uniform(-2.0, 2.0);
        assert_eq!(v, curve.value(0.3) + y * d);

        let v = curve.scaled_variance(0.3, &mut random);
        let y = curve.table()[(0.3f32 * 499.0 + 0.5) as usize];
        assert_eq!(v, shadow.uniform(-2.0 * y, 2.0 * y));

        // No draw without variance.
        curve.value(0.3);
        assert_eq!(random.draw(), shadow.draw());
    }
}
