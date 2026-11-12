//! Stage 2: tree set-up and the trunk/branch skeleton (generation.md 2.3–2.9).
//!
//! Grows every branch's vertices (position, frame, radius, running length) and decides where
//! children and buds grow, keeping the exact order of random draws. Provisional until checked
//! against the executable.

use super::frame::{
    Frame, Vec3, add, angle_between, cross, degrees, dot, fast_sqrt, lerp, lerp3, normalize, scale,
    sub,
};
use super::leaves::{Leaf, LeafContext, about_y, make_leaf};
use super::limits::{
    GenerateError, MAX_BRANCHES, MAX_CHILD_CANDIDATES, MAX_LEAVES, MAX_SKELETON_VERTICES,
};
use super::{Curve, Random};
use crate::speedtree::{BezierSpline, BranchLevel, SpeedTree, SplinePoint};
use std::rc::Rc;

/// Size of the tree's bank of random values (2.3 step 2).
pub const BANK: usize = 1000;
/// 2π as the original's float constant (one float below `TAU`).
#[allow(clippy::approx_constant)]
const TWO_PI: f32 = 6.283185;
/// Straight down.
const DOWN: Vec3 = [0.0, 0.0, -1.0];

/// A grown skeleton: the trunk, branches and roots in growth order (each branch before its
/// children), and the leaf sites.
#[derive(Clone, Debug, Default)]
pub struct Skeleton {
    /// The random tree size, the `S` every branch grows with.
    pub size: f32,
    /// The bank of 1000 uniform values (2.3 step 2).
    pub bank: Vec<f32>,
    /// Bank values the children's frequency culling took (the frequency index, 2.5 step 3).
    pub frequency_index: usize,
    pub branches: Vec<Branch>,
    pub buds: Vec<Bud>,
    /// Frond guides, in the order frond branches grew (4.1).
    pub guides: Vec<Guide>,
    /// Leaves in creation order (stage 5).
    pub leaves: Vec<Leaf>,
}

/// A frond branch's record for stage 4 (4.1), made when its vertices are done.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Guide {
    /// The frond branch in [`Skeleton::branches`].
    pub branch: usize,
    /// Its length along the vertices (by the approximate square root).
    pub length: f32,
    /// Index into `Fronds::textures`.
    pub texture: usize,
    pub radius: f32,
    /// Degrees about the guide's X axis.
    pub angle_offset: f32,
    /// Length times the branch's first radius.
    pub area: f32,
}

/// One flare of a branch's base (2.6).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Flare {
    /// Radians around the branch.
    pub angle: f32,
    pub radial_exponent: f32,
    pub length_exponent: f32,
    /// Radians.
    pub radial_influence: f32,
    pub length_influence: f32,
    pub distance: f32,
}

/// Wind matrices the game animates (7.1); groups pick among them.
pub const WIND_MATRICES: u8 = 6;

/// The two wind pairs of a skeleton vertex or leaf as the computation carries them (7.7):
/// weights where 1 is rigid, and the round-robin groups.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Wind {
    pub weights: [f32; 2],
    pub groups: [i32; 2],
}

impl Wind {
    /// The output form (7.7): weights `clamp(1 − w, 0, 1)` (0 rigid, 1 free) and matrix
    /// indices `(g as an unsigned byte) mod 6`.
    pub fn output(&self) -> ([f32; 2], [u8; 2]) {
        (
            self.weights.map(|w| (1.0 - w).clamp(0.0, 1.0)),
            self.groups.map(|g| (g as u8) % WIND_MATRICES),
        )
    }

    /// Weights interpolated between two vertices, groups from `groups`.
    fn between(a: Wind, b: Wind, s: f32, groups: [i32; 2]) -> Wind {
        Wind {
            weights: [
                lerp(a.weights[0], b.weights[0], s),
                lerp(a.weights[1], b.weights[1], s),
            ],
            groups,
        }
    }
}

/// One vertex along a branch.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vertex {
    pub position: Vec3,
    pub frame: Frame,
    /// The frame's first row.
    pub direction: Vec3,
    pub radius: f32,
    pub flexibility: f32,
    /// Points around the cross section here (stage 3).
    pub cross_sections: i32,
    /// Children forked onto this vertex (2.5.1).
    pub forks: i32,
    /// Length along the branch to here.
    pub length: f32,
    /// Branch progress: 0 at the first vertex, the packed progress after (2.4 step 9.1).
    pub progress: f32,
    /// Wind weights and groups (7.7).
    pub wind: Wind,
}

/// One branch (the trunk is the first).
#[derive(Clone, Debug, PartialEq)]
pub struct Branch {
    pub level: i32,
    /// Index of the parent in [`Skeleton::branches`].
    pub parent: Option<usize>,
    /// Where along the parent it grows (the profile percent it was grown with).
    pub percent: f32,
    pub root: bool,
    /// A frond rather than a real branch (2.7).
    pub frond: bool,
    pub length: f32,
    pub azimuth: f32,
    pub start_angle: f32,
    pub gravity: f32,
    /// The branch radius before the per-vertex profile.
    pub radius: f32,
    /// The radius limit of 2.4 step 6.
    pub max_radius: f32,
    /// The radius at the base by the unscaled profile (fronds use it).
    pub first_radius: f32,
    pub flexibility: f32,
    /// Points around the cross section, after the minimum-size reduction.
    pub cross_sections: i32,
    /// The gnarl was negated for this branch.
    pub gnarl_negated: bool,
    /// The wind values it was grown with, its groups as assigned at its start (7.7).
    pub wind: Wind,
    pub flares: Vec<Flare>,
    pub vertices: Vec<Vertex>,
    pub volume: f32,
}

/// A leaf site: where a child of the last level would grow (stage 5).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bud {
    /// The branch it grows on and the vertex it follows.
    pub branch: usize,
    pub vertex: usize,
    /// Fraction along that segment (may exceed 1, 2.9 answer 6).
    pub segment_fraction: f32,
    pub position: Vec3,
    /// The parent vertex's frame and direction.
    pub frame: Frame,
    pub direction: Vec3,
    /// Where along the parent, as drawn, and as a fraction of first..last.
    pub along: f32,
    pub percent: f32,
}

/// The C runtime generator the flares draw from (2.6).
struct CRandom(u32);

impl CRandom {
    fn rand(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(214_013).wrapping_add(2_531_011);
        (self.0 >> 16) & 0x7FFF
    }

    fn float(&mut self, a: f32, b: f32) -> f32 {
        a + (self.rand() as f32 / 32767.0) * (b - a)
    }
}

/// A level's curves, built once.
struct LevelCurves {
    disturbance: Curve,
    gravity: Curve,
    flexibility: Curve,
    flexibility_scale: Curve,
    length: Curve,
    radius: Curve,
    radius_scale: Curve,
    start_angle: Curve,
    angle_profile: Curve,
    cross_section_profile: Curve,
    frequency_profile: Curve,
    gnarl_profile: Curve,
}

/// `BezierSpline 0 1 0 { 2  0 1 1 0 0.1  1 1 1 0 0.1 }`: the flat default of value 1.
pub(super) fn flat_one() -> BezierSpline {
    let point = |x| SplinePoint {
        x,
        y: 1.0,
        tangent: [1.0, 0.0],
        tangent_length: 0.1,
    };
    BezierSpline {
        min: 0.0,
        max: 1.0,
        variance: 0.0,
        points: vec![point(0.0), point(1.0)],
    }
}

impl LevelCurves {
    fn new(level: &BranchLevel) -> Self {
        // Every curve is present in every game file. The spec gives the flat default for the
        // profiles; other missing curves build empty (all zeros, provisional).
        let required = |s: &Option<BezierSpline>| Curve::new(&s.clone().unwrap_or_default());
        let profile = |s: &Option<BezierSpline>| Curve::new(&s.clone().unwrap_or_else(flat_one));
        LevelCurves {
            disturbance: required(&level.disturbance),
            gravity: required(&level.gravity),
            flexibility: required(&level.flexibility),
            flexibility_scale: required(&level.flexibility_scale),
            length: required(&level.length),
            radius: required(&level.radius),
            radius_scale: required(&level.radius_scale),
            start_angle: required(&level.start_angle),
            angle_profile: required(&level.angle_profile),
            cross_section_profile: profile(&level.cross_section_profile),
            frequency_profile: profile(&level.frequency_profile),
            gnarl_profile: profile(&level.gnarl_profile),
        }
    }
}

/// What the frond rules make of a branch (2.7).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FrondRule {
    Enabled,
    Disabled,
    Pruned,
}

impl FrondRule {
    fn from_condition(condition: i32) -> Self {
        match condition {
            0 => FrondRule::Enabled,
            2 => FrondRule::Pruned,
            _ => FrondRule::Disabled,
        }
    }
}

/// Inputs to growing one branch (2.4).
struct Sprout {
    level: i32,
    percent: f32,
    position: Vec3,
    frame: Frame,
    azimuth_axis: Vec3,
    parent_radius: f32,
    seed: i32,
    root: bool,
    parent: Option<usize>,
    wind: Wind,
}

struct Grower<'t> {
    tree: &'t SpeedTree,
    levels: Rc<Vec<LevelCurves>>,
    roots: Option<Rc<LevelCurves>>,
    random: Random,
    crt: CRandom,
    size: f32,
    first_level: i32,
    bank: Vec<f32>,
    frequency_index: usize,
    branches: Vec<Branch>,
    buds: Vec<Bud>,
    guides: Vec<Guide>,
    leaves: Vec<Leaf>,
    /// Per branch, its leaf-mesh rotation counter (5.3 step 8).
    mesh_counters: Vec<usize>,
    /// Running totals against the limits, and the first limit passed.
    vertex_count: usize,
    candidates: usize,
    exceeded: Option<GenerateError>,
    /// The wind group counter and settings (7.7).
    wind_counter: i32,
    wind_level: i32,
    propagate_flexibility: bool,
}

/// The flare seed, 444 when the file has none (7.10 items 5 and 10).
fn flare_seed(tree: &SpeedTree) -> i32 {
    tree.flare_seed.unwrap_or(444)
}

/// Grows a tree's skeleton (stage 2). The stream is returned in its final state so later
/// stages continue the same sequence.
pub fn grow(tree: &SpeedTree) -> Result<(Skeleton, Random), GenerateError> {
    super::limits::validate(tree)?;
    let seed = tree.tree.seed as i32;
    // 2.3 steps 1–2: the bank.
    let mut random = Random::new(seed);
    let bank: Vec<f32> = (0..BANK).map(|_| random.uniform(0.0, 1.0)).collect();
    // Step 3: the tree size.
    random.reseed(seed);
    let info = &tree.tree;
    let size = random.uniform(
        info.size - info.size_variance,
        info.size + info.size_variance,
    );
    // Step 4: the flare generator. Step 5 (leaf card sizes) belongs to stage 5.
    let crt = CRandom(flare_seed(tree) as u32);
    // Step 6.
    random.reseed(seed);
    let mut grower = Grower {
        tree,
        levels: Rc::new(tree.branch_levels.iter().map(LevelCurves::new).collect()),
        roots: tree
            .roots
            .as_ref()
            .map(|r| Rc::new(LevelCurves::new(&r.roots))),
        random,
        crt,
        size,
        first_level: tree.cluster_first_branch_level.unwrap_or(0),
        bank,
        frequency_index: 0,
        branches: Vec::new(),
        buds: Vec::new(),
        guides: Vec::new(),
        leaves: Vec::new(),
        mesh_counters: Vec::new(),
        vertex_count: 0,
        candidates: 0,
        exceeded: None,
        // 7.7 / 7.10 item 5: the stored seed, without the ≤ 1 rule.
        wind_counter: seed,
        wind_level: tree.wind_level.unwrap_or(1),
        propagate_flexibility: tree.propagate_flexibility.unwrap_or(false),
    };
    // Step 7: the trunk.
    if !tree.branch_levels.is_empty() {
        grower.grow(Sprout {
            level: 0,
            percent: 0.0,
            position: [0.0; 3],
            frame: Frame::about_z(tree.tree_angle.unwrap_or(0.0)).then(&Frame::IDENTITY),
            azimuth_axis: [0.0, 0.0, 1.0],
            parent_radius: -1.0,
            seed,
            root: false,
            parent: None,
            wind: Wind {
                weights: [1.0, 1.0],
                groups: [seed, flare_seed(tree)],
            },
        });
    }
    if let Some(e) = grower.exceeded {
        return Err(e);
    }
    let skeleton = Skeleton {
        size,
        bank: grower.bank,
        frequency_index: grower.frequency_index,
        branches: grower.branches,
        buds: grower.buds,
        guides: grower.guides,
        leaves: grower.leaves,
    };
    Ok((skeleton, grower.random))
}

impl<'t> Grower<'t> {
    fn info(&self, level: i32, root: bool) -> Option<&'t BranchLevel> {
        let tree = self.tree;
        if root {
            tree.roots.as_ref().map(|r| &r.roots)
        } else {
            usize::try_from(level)
                .ok()
                .and_then(|l| tree.branch_levels.get(l))
        }
    }

    /// A level's curves, shared so they can be read while the grower changes.
    fn curves(&self, level: i32, root: bool) -> Option<CurvesRef> {
        if root {
            self.roots.clone().map(CurvesRef::Roots)
        } else {
            let l = usize::try_from(level).ok()?;
            (l < self.levels.len()).then(|| CurvesRef::Level(self.levels.clone(), l))
        }
    }

    /// The frond rules for a branch about to grow (2.7).
    fn frond_rule(&self, sprout: &Sprout) -> FrondRule {
        let Some(fronds) = self.tree.fronds.as_ref().filter(|f| f.enabled) else {
            return FrondRule::Disabled;
        };
        if sprout.level < fronds.level {
            return FrondRule::Disabled;
        }
        let m = fronds.distance_level;
        let test = if m == 0 {
            sprout.percent
        } else {
            // Walk up from the parent; the trunk has none, so it tests its own percent
            // (provisional).
            match sprout.parent {
                None => sprout.percent,
                Some(mut current) => {
                    let mut count = 0;
                    while let Some(parent) = self.branches[current].parent {
                        if count >= m {
                            break;
                        }
                        current = parent;
                        count += 1;
                    }
                    self.branches[current].percent
                }
            }
        };
        let d = fronds.distance_percent;
        let above = if d < 0.0 { test < -d } else { test >= d };
        FrondRule::from_condition(if above {
            fronds.above_condition
        } else {
            fronds.below_condition
        })
    }

    /// The flares of one branch from the C runtime generator (2.6).
    fn flares(&mut self, info: &BranchLevel) -> Vec<Flare> {
        let f = &info.flares;
        if f.count <= 0 {
            return Vec::new();
        }
        let initial = self.crt.float(0.0, TWO_PI);
        let balance = TWO_PI / f.count as f32;
        let mut out = Vec::with_capacity(f.count as usize);
        for i in 0..f.count {
            let mut angle = initial + i as f32 * self.crt.float(f.balance * balance, balance);
            if angle > TWO_PI {
                angle -= TWO_PI;
            }
            let radial_influence = self.crt.float(
                f.radial_influence - f.radial_influence_variance,
                f.radial_influence + f.radial_influence_variance,
            ) / 57.29578;
            let length_influence = self.crt.float(
                f.length_distance - f.length_distance_variance,
                f.length_distance + f.length_distance_variance,
            );
            let distance = self.crt.float(
                f.radial_distance - f.radial_distance_variance,
                f.radial_distance + f.radial_distance_variance,
            );
            out.push(Flare {
                angle,
                radial_exponent: f.radial_exponent,
                length_exponent: f.length_exponent,
                radial_influence,
                length_influence,
                distance,
            });
        }
        out
    }
}

/// Shared access to one level's curves.
enum CurvesRef {
    Level(Rc<Vec<LevelCurves>>, usize),
    Roots(Rc<LevelCurves>),
}

impl std::ops::Deref for CurvesRef {
    type Target = LevelCurves;
    fn deref(&self) -> &LevelCurves {
        match self {
            CurvesRef::Level(levels, l) => &levels[*l],
            CurvesRef::Roots(roots) => roots,
        }
    }
}

/// Gravity at one vertex (2.4 step 8.4), given the drawn angle-profile value; applied on the
/// right.
fn apply_gravity(frame: Frame, curves_profile: f32, gravity: f32) -> Frame {
    let d = frame.direction();
    let g = degrees(angle_between(d, DOWN));
    let facing = 1.0 - (90.0 - g).abs() / 90.0;
    let axis = normalize(cross(d, DOWN));
    let profile = -2.0 * (curves_profile - 0.5);
    frame.then(&Frame::about_axis(axis, profile * gravity * g * facing))
}

impl<'t> Grower<'t> {
    /// Grows one branch and its children (2.4, 2.5). Returns its index, or `None` when the
    /// frond rules prune it.
    /// One skeleton vertex's wind weights (7.7) from those the branch was grown with, its
    /// progress `t` and its vertex flexibility `f`.
    fn vertex_wind(&self, level: i32, received: Wind, t: f32, f: f32) -> Wind {
        let w = self.wind_level;
        let mut out = received;
        if level < w {
            out.weights = [1.0, 1.0];
        } else if level == w {
            out.weights = [1.0 - t * f, 1.0];
        } else if level == w.wrapping_add(1) {
            out.weights[1] = 1.0 - t * f;
        } else if self.propagate_flexibility {
            out.weights[1] = lerp(received.weights[1], 0.0, f);
        }
        out
    }

    /// Records the first limit passed; growth stops from then on.
    fn exceed(&mut self, what: &'static str, max: usize) {
        if self.exceeded.is_none() {
            self.exceeded = Some(GenerateError::TooLarge { what, max });
        }
    }

    fn grow(&mut self, sprout: Sprout) -> Option<usize> {
        if self.exceeded.is_some() {
            return None;
        }
        if self.branches.len() >= MAX_BRANCHES {
            self.exceed("branches", MAX_BRANCHES);
            return None;
        }
        let tree = self.tree;
        let (Some(info), Some(c)) = (
            self.info(sprout.level, sprout.root),
            self.curves(sprout.level, sprout.root),
        ) else {
            return None;
        };
        let level = sprout.level;
        let s = self.size;
        let p = sprout.percent;

        // Step 1: fronds.
        let frond = match self.frond_rule(&sprout) {
            FrondRule::Pruned => return None,
            FrondRule::Enabled => !sprout.root,
            FrondRule::Disabled => false,
        };
        // 7.7: the round-robin wind groups, right after the frond test.
        let mut wind = sprout.wind;
        if level == self.wind_level {
            wind.groups[0] = self.wind_counter;
            self.wind_counter = self.wind_counter.wrapping_add(1);
        } else if level == self.wind_level.wrapping_add(1) {
            wind.groups[1] = self.wind_counter;
            self.wind_counter = self.wind_counter.wrapping_add(1);
        }
        // Step 2: flares.
        let flares = if frond { Vec::new() } else { self.flares(info) };

        let c = &*c;
        let random = &mut self.random;
        // Steps 3–5.
        let length = c.length.value_with_variance(p, random) * s;
        let azimuth = random.uniform(-180.0, 180.0);
        let start_angle = c.start_angle.value_with_variance(p, random);
        let gravity = c.gravity.value_with_variance(p, random);
        let mut radius = (c.radius.value_with_variance(p, random) * s).abs();
        let flexibility = c.flexibility.value_with_variance(p, random);
        let mut cross_sections = info.cross_section_segments;

        // Step 6: radius limits.
        let mut max_radius = c.radius.max * s;
        if level > self.first_level && sprout.parent_radius > 0.0 {
            let limit = 0.85 * sprout.parent_radius;
            if limit < max_radius {
                max_radius = limit;
            }
        }
        if radius > max_radius {
            radius = max_radius;
        }
        let first_radius = radius * c.radius_scale.value_with_variance(0.0, random);

        // Step 7: vertex count.
        let mut n = info.segments + 1;
        let fronds = tree.fronds.as_ref();
        let segment_override = fronds.filter(|f| frond && f.use_segment_override);
        if let Some(f) = segment_override {
            n = f.segment_override + 1;
        } else {
            let (lmin, lmax) = (c.length.min, c.length.max);
            if info.min_length_percent != 1.0 && lmin < lmax {
                let f = ((length - lmin * s) / (lmax * s - lmin * s)).clamp(0.0, 1.0);
                n = (lerp(n as f32 * info.min_length_percent, n as f32, f) as i32).max(2);
            }
            let (rmin, rmax) = (c.radius.min, c.radius.max);
            if info.min_cross_section_percent != 1.0 && rmin < rmax {
                let f = ((radius - rmin * s) / (rmax * s - rmin * s)).clamp(0.0, 1.0);
                cross_sections = (lerp(
                    cross_sections as f32 * info.min_cross_section_percent,
                    cross_sections as f32,
                    f,
                ) as i32)
                    .max(3);
            }
        }

        let mut branch = Branch {
            level,
            parent: sprout.parent,
            percent: p,
            root: sprout.root,
            frond,
            length,
            azimuth,
            start_angle,
            gravity,
            radius,
            max_radius,
            first_radius,
            flexibility,
            cross_sections,
            gnarl_negated: false,
            wind,
            flares,
            vertices: Vec::new(),
            volume: 0.0,
        };
        self.vertex_count += n.max(0) as usize;
        if self.vertex_count > MAX_SKELETON_VERTICES {
            self.exceed("skeleton vertices", MAX_SKELETON_VERTICES);
            return None;
        }
        if n < 2 {
            self.branches.push(branch);
            return Some(self.branches.len() - 1);
        }

        // Absent floor values mean the floor is disabled (5.6 E).
        let floor = tree.floor.as_ref().filter(|f| {
            f.enabled == Some(true)
                && f.value.is_some()
                && f.level.is_some()
                && f.exponent.is_some()
                && f.bias.is_some()
        });
        let floor_value = floor.and_then(|f| f.value).unwrap_or(0.0);
        let floor_applies = floor.is_some_and(|f| level >= f.level.unwrap_or(0));
        let floor_exponent = floor.and_then(|f| f.exponent).unwrap_or(1.0);
        let floor_bias = floor.and_then(|f| f.bias).unwrap_or(0.0);
        let cross_count = |c: &LevelCurves, t: f32| {
            let profile = c.cross_section_profile.value(t).clamp(0.0, 1.0);
            lerp(3.0, cross_sections as f32, profile) as i32
        };

        // Step 8: the first vertex.
        let azimuth_disturbance = c.disturbance.scaled_variance(0.0, random);
        let pitch_disturbance = c.disturbance.scaled_variance(0.0, random);
        let axis = sprout.frame.apply(sprout.azimuth_axis);
        let mut frame = Frame::about_axis(axis, azimuth).then(&sprout.frame);
        frame = Frame::pitch_yaw(start_angle + pitch_disturbance, azimuth_disturbance).then(&frame);
        let vertex_radius = radius * c.radius_scale.value_with_scaled_variance(0.0, random);
        let vertex_flexibility = flexibility * c.flexibility_scale.value_with_variance(0.0, random);
        let profile = c.angle_profile.value_with_variance(0.0, random);
        frame = apply_gravity(frame, profile, gravity);
        let mut gnarl = info.gnarl * c.gnarl_profile.value(0.0);
        if !info.gnarl_unison && (azimuth as i32) % 2 == 0 {
            gnarl = -gnarl;
            branch.gnarl_negated = true;
        }
        frame = frame.then(&Frame::about_z(gnarl));
        let mut above_floor = sprout.position[2] >= floor_value;
        branch.vertices.push(Vertex {
            position: sprout.position,
            frame,
            direction: frame.direction(),
            radius: vertex_radius,
            flexibility: vertex_flexibility,
            cross_sections: cross_count(c, 0.0),
            forks: 0,
            length: 0.0,
            progress: 0.0,
            wind: self.vertex_wind(level, wind, 0.0, vertex_flexibility),
        });

        // Step 9: the remaining vertices.
        for i in 1..n {
            let previous = branch.vertices[i as usize - 1];
            let u = i as f32 / (n - 1) as f32;
            let t = u.powf(info.flares.segment_packing_exponent);
            let segment = t * length - previous.length;
            let cross = cross_count(c, t);
            let random = &mut self.random;
            let vertex_radius = radius * c.radius_scale.value_with_scaled_variance(t, random);
            let vertex_flexibility =
                flexibility * c.flexibility_scale.value_with_variance(t, random);
            let profile = c.angle_profile.value_with_variance(t, random);
            let mut frame = apply_gravity(previous.frame, profile, gravity);
            let azimuth_disturbance = c.disturbance.scaled_variance(t, random);
            let pitch_disturbance = c.disturbance.scaled_variance(t, random);
            frame = Frame::pitch_yaw(pitch_disturbance, azimuth_disturbance).then(&frame);
            let mut gnarl = info.gnarl * c.gnarl_profile.value(t);
            if branch.gnarl_negated {
                gnarl = -gnarl;
            }
            frame = frame.then(&Frame::about_z(gnarl));
            let vertex_wind = self.vertex_wind(level, wind, t, vertex_flexibility);
            let mut position = add(previous.position, scale(previous.direction, segment));
            // Step 9.9: the floor.
            if floor_applies
                && above_floor
                && position[2] <= previous.position[2]
                && position[2] < floor_value
            {
                let blend = u.powf(floor_exponent) * floor_bias;
                position[2] = lerp(floor_value, position[2], blend);
            }
            if position[2] >= floor_value {
                above_floor = true;
            }
            branch.vertices.push(Vertex {
                position,
                frame,
                direction: frame.direction(),
                radius: vertex_radius,
                flexibility: vertex_flexibility,
                cross_sections: cross,
                forks: 0,
                length: previous.length + segment,
                progress: t,
                wind: vertex_wind,
            });
        }

        // Step 12: volume, with the original's approximate square root for distances.
        branch.volume = branch
            .vertices
            .windows(2)
            .map(|w| {
                let d = sub(w[1].position, w[0].position);
                fast_sqrt(dot(d, d)) * (w[0].radius + w[1].radius)
            })
            .fold(0.0, |sum, v| sum + v);

        self.branches.push(branch);
        let index = self.branches.len() - 1;
        // Step 10a: a frond branch becomes a guide.
        if frond {
            self.guide(index);
        }
        // Step 11: children.
        self.children(index, info, sprout.seed);
        Some(index)
    }

    /// A bud on branch `index` (5.2): its own two draws, then a leaf (5.3).
    #[allow(clippy::too_many_arguments)]
    fn bud(
        &mut self,
        index: usize,
        level: i32,
        base: Vec3,
        frame0: Frame,
        direction: Vec3,
        percent: f32,
        wind: Wind,
    ) {
        let tree = self.tree;
        let (Some(info), Some(c)) = (self.info(level, false), self.curves(level, false)) else {
            return;
        };
        let length = c.length.value_with_variance(percent, &mut self.random) * self.size;
        let step = length / info.segments as f32;
        let azimuth = self.random.uniform(-180.0, 180.0);
        let axis = frame0.apply(direction);
        let mut frame = Frame::about_axis(axis, azimuth).then(&frame0);
        frame = about_y(60.0).then(&frame);
        let position = add(base, scale(frame.direction(), step));
        let nu = normalize(sub(position, base));
        // Step 5: the parent direction.
        let branch = &self.branches[index];
        let depth = tree.leaves.dimming_depth;
        let delta = if branch.vertices.is_empty() {
            nu
        } else if depth == 0 {
            let n = branch.vertices.len();
            let k = ((f64::from(percent) * (n - 1) as f64) as usize).min(n - 1);
            normalize(branch.vertices[k].direction)
        } else {
            let mut current = Some(index);
            let mut count = 0;
            while let Some(c) = current {
                if count >= depth - 1 {
                    break;
                }
                current = self.branches[c].parent;
                count += 1;
            }
            let base = current
                .and_then(|c| self.branches[c].vertices.first())
                .or(branch.vertices.first())
                .map(|v| v.position)
                .unwrap_or(base);
            normalize(sub(position, base))
        };
        if self.leaves.len() >= MAX_LEAVES {
            self.exceed("leaves", MAX_LEAVES);
            return;
        }
        if self.mesh_counters.len() <= index {
            self.mesh_counters.resize(index + 1, 0);
        }
        make_leaf(
            LeafContext {
                tree,
                branches: &self.branches,
                bank: &self.bank,
                random: &mut self.random,
                leaves: &mut self.leaves,
                mesh_counter: &mut self.mesh_counters[index],
            },
            position,
            percent,
            index,
            nu,
            delta,
            wind,
        );
    }

    /// Records frond branch `index` as a guide (4.1), drawing the texture choice and angle
    /// offset when the fronds have textures.
    fn guide(&mut self, index: usize) {
        let branch = &self.branches[index];
        let length = branch
            .vertices
            .windows(2)
            .map(|w| {
                let d = sub(w[1].position, w[0].position);
                fast_sqrt(dot(d, d))
            })
            .fold(0.0, |sum, d| sum + d);
        let area = length * branch.first_radius;
        let textures = self
            .tree
            .fronds
            .as_ref()
            .map(|f| f.textures.as_slice())
            .unwrap_or_default();
        let (texture, radius, angle_offset) = if textures.is_empty() {
            (0, length * 0.5, 0.0)
        } else {
            let texture = self.random.uniform(0.0, 100_000.0) as u32 as usize % textures.len();
            let x = &textures[texture];
            let radius = length * x.aspect_ratio * 0.5 * x.size_scale;
            let mut offset = self.random.uniform(x.min_angle_offset, x.max_angle_offset);
            if (self.guides.len() + 1) % 2 == 1 {
                offset = -offset;
            }
            (texture, radius, offset)
        };
        self.guides.push(Guide {
            branch: index,
            length,
            texture,
            radius,
            angle_offset,
            area,
        });
    }

    /// Finds the segment holding running length `g`: the first vertex `i ≥ 1` whose running
    /// length exceeds `g`, minus one, or 0 when none does (2.9 answer 6).
    fn locate(vertices: &[Vertex], g: f32) -> usize {
        vertices
            .iter()
            .skip(1)
            .position(|v| v.length > g)
            .unwrap_or(0)
    }

    /// The children of branch `index` (2.5).
    fn children(&mut self, index: usize, info: &BranchLevel, seed: i32) {
        let tree = self.tree;
        let (level, length, root) = {
            let b = &self.branches[index];
            (b.level, b.length, b.root)
        };
        if root {
            return;
        }
        let s = self.size;
        let count = ((info.frequency / s) * length) as i32;
        let child_level = level + 1;
        let roots = match &tree.roots {
            Some(r) if r.level == child_level => ((r.frequency / s) * length) as i32,
            _ => 0,
        };
        let buds = child_level >= tree.branch_levels.len() as i32 - 1;
        if buds && tree.leaf_lods.is_some() {
            return;
        }
        let (first, last) = (info.first_branch, info.last_branch);
        let (root_first, root_last) = tree
            .roots
            .as_ref()
            .map(|r| (r.first, r.last))
            .unwrap_or((0.0, 0.0));
        let Some(curves) = self.curves(level, false) else {
            return;
        };
        let seed0 = seed;
        let mut seed = seed;
        for k in 0..count.saturating_add(roots) {
            self.candidates += 1;
            if self.candidates > MAX_CHILD_CANDIDATES {
                self.exceed("child candidates", MAX_CHILD_CANDIDATES);
            }
            if self.exceeded.is_some() {
                return;
            }
            let is_root = k >= count;
            // Step 5.1: reseeding.
            if k == count {
                self.random.reseed(seed0);
            } else if !buds && k < count {
                seed = seed.wrapping_add(3);
                self.random.reseed(seed);
            }
            // Step 5.2: where.
            let mut q = if !buds && k == 0 && child_level > self.first_level && !is_root {
                self.random
                    .uniform(lerp(first, last, 0.85), lerp(first, last, 0.95))
            } else if is_root {
                self.random.uniform(root_first, root_last)
            } else {
                self.random.uniform(first, last)
            };
            // Step 5.3: frequency culling.
            if !is_root {
                let span = last - first;
                // first == last divides by zero in the original; take 0 (provisional).
                let f = if span != 0.0 {
                    ((q - first) / span).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let value = curves.frequency_profile.value(f);
                if value != 1.0 {
                    let b = self.bank[self.frequency_index % BANK];
                    self.frequency_index += 1;
                    if b > value {
                        continue;
                    }
                }
            }
            // Step 5.4: forking.
            if !is_root && !buds && info.fork_enabled {
                let roll = self.random.uniform(0.0, 1.0);
                if roll <= info.fork_bias {
                    match self.fork(index, q, info) {
                        Some(forked) if forked >= first && forked <= last => q = forked,
                        _ => continue,
                    }
                }
            }
            // Step 5.5: pruning.
            let test = if info.pruning_depth == 0 {
                q
            } else {
                let mut current = index;
                let mut counter = 1u32;
                while let Some(parent) = self.branches[current].parent {
                    if counter >= info.pruning_depth {
                        break;
                    }
                    current = parent;
                    counter += 1;
                }
                self.branches[current].percent
            };
            let keep = if info.pruning_percent < 0.0 {
                test <= -info.pruning_percent
            } else {
                test >= info.pruning_percent
            };
            if !keep {
                continue;
            }
            // Step 5.6: locate on the parent.
            let parent = &self.branches[index];
            let g = q * length;
            let j = Self::locate(&parent.vertices, g);
            let (a, b) = (parent.vertices[j], parent.vertices[j + 1]);
            let span = b.length - a.length;
            // Zero only for a zero-length parent; take the segment start (provisional).
            let fraction = if span != 0.0 {
                (g - a.length) / span
            } else {
                0.0
            };
            // Step 5.7.
            if !buds && !is_root {
                self.random.reseed(seed);
                self.random.uniform(0.0, 100.0);
            }
            // Step 5.8.
            let position = lerp3(a.position, b.position, fraction);
            let percent = if is_root {
                if root_first == root_last {
                    1.0
                } else {
                    (q - root_first) / (root_last - root_first)
                }
            } else if first == last {
                1.0
            } else {
                (q - first) / (last - first)
            };
            // Step 5.9.
            if buds {
                self.buds.push(Bud {
                    branch: index,
                    vertex: j,
                    segment_fraction: fraction,
                    position,
                    frame: a.frame,
                    direction: a.direction,
                    along: q,
                    percent,
                });
                let groups = self.branches[index].wind.groups;
                let mut wind = Wind::between(a.wind, b.wind, fraction, groups);
                wind.weights = wind.weights.map(|w| w.clamp(0.0, 1.0));
                self.bud(
                    index,
                    child_level,
                    position,
                    a.frame,
                    a.direction,
                    percent,
                    wind,
                );
                continue;
            }
            let (frame, azimuth_axis) = if child_level == self.first_level {
                (Frame::IDENTITY, [0.0, 0.0, 1.0])
            } else {
                (a.frame, a.direction)
            };
            self.grow(Sprout {
                level: child_level,
                percent,
                position,
                frame,
                azimuth_axis,
                parent_radius: lerp(a.radius, b.radius, fraction),
                seed,
                root: is_root,
                parent: Some(index),
                wind: Wind::between(a.wind, b.wind, fraction, self.branches[index].wind.groups),
            });
        }
    }

    /// Fork placement (2.5.1): moves `q` onto a vertex of the parent, or `None` to skip.
    fn fork(&mut self, index: usize, q: f32, info: &BranchLevel) -> Option<f32> {
        let branch = &mut self.branches[index];
        let length = branch.length;
        let g = q * length;
        let vertices = &mut branch.vertices;
        let n = vertices.len();
        let mut j = Self::locate(vertices, g);
        if g - vertices[j].length > vertices[j + 1].length - g {
            j += 1;
        }
        if j < n - 1 && vertices[j].forks < info.fork_limit {
            vertices[j].forks += 1;
            let angle = if j == 0 {
                180.0
            } else {
                degrees(angle_between(
                    vertices[j].direction,
                    vertices[j - 1].direction,
                ))
            };
            if angle >= info.fork_angle {
                return Some(vertices[j].length / length);
            }
        }
        None
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::speedtree::{Floor, Fronds, TreeInfo};

    /// A constant curve of `value` with the given variance.
    fn constant(value: f32, variance: f32) -> Option<BezierSpline> {
        let mut s = flat_one();
        s.min = 0.0;
        s.max = value;
        s.variance = variance;
        Some(s)
    }

    /// A level with no randomness: length 1 (times the size), straight growth.
    pub(crate) fn level(start_angle: f32, frequency: f32) -> BranchLevel {
        BranchLevel {
            disturbance: constant(0.0, 0.0),
            gravity: constant(0.0, 0.0),
            flexibility: constant(0.5, 0.0),
            flexibility_scale: constant(1.0, 0.0),
            length: constant(1.0, 0.0),
            radius: constant(0.1, 0.0),
            radius_scale: constant(1.0, 0.0),
            start_angle: constant(start_angle, 0.0),
            angle_profile: constant(0.5, 0.0),
            segments: 4,
            frequency,
            first_branch: 0.2,
            last_branch: 0.8,
            ..BranchLevel::default()
        }
    }

    pub(crate) fn tree(levels: Vec<BranchLevel>) -> SpeedTree {
        SpeedTree {
            tree: TreeInfo {
                seed: 1234,
                size: 10.0,
                size_variance: 0.0,
                ..TreeInfo::default()
            },
            branch_levels: levels,
            flare_seed: Some(1),
            cluster_first_branch_level: Some(1),
            ..SpeedTree::default()
        }
    }

    #[test]
    fn the_c_runtime_generator_matches_msvc() {
        let mut crt = CRandom(1);
        let first: Vec<u32> = (0..4).map(|_| crt.rand()).collect();
        assert_eq!(first, [41, 18467, 6334, 26500]);
    }

    #[test]
    fn a_trunk_pitched_minus_90_grows_straight_up() {
        let t = tree(vec![level(-90.0, 0.0), level(0.0, 0.0), level(0.0, 0.0)]);
        let (skeleton, _) = grow(&t).unwrap();
        assert_eq!(skeleton.size, 10.0);
        assert_eq!(skeleton.branches.len(), 1);
        let trunk = &skeleton.branches[0];
        assert_eq!(trunk.vertices.len(), 5);
        for (i, v) in trunk.vertices.iter().enumerate() {
            let z = i as f32 * 2.5;
            assert!(v.position[0].abs() < 1e-5 && v.position[1].abs() < 1e-5);
            assert!((v.position[2] - z).abs() < 1e-4, "{i}: {:?}", v.position);
            assert!((v.length - z).abs() < 1e-4);
            assert!((v.direction[2] - 1.0).abs() < 1e-5);
        }
        // Volume: four segments of 2.5 with radii 1 + 1 (radius 0.1 · size 10), the length
        // by the approximate square root.
        assert_eq!(trunk.volume, 4.0 * fast_sqrt(6.25) * 2.0);
    }

    #[test]
    fn draws_follow_the_spec_order() {
        let t = tree(vec![level(-90.0, 0.0), level(0.0, 0.0), level(0.0, 0.0)]);
        let (_, random) = grow(&t).unwrap();
        // Bank 1000, size 1; the trunk: length, azimuth, start angle, gravity, radius,
        // flexibility, first radius (7); first vertex: two disturbances, radius,
        // flexibility, angle profile (5); four more vertices of 5 each.
        assert_eq!(random.draws(), 1000 + 1 + 7 + 5 + 4 * 5);
    }

    #[test]
    fn children_grow_between_first_and_last() {
        // Frequency 2 per unit length over a trunk of 10 → 2 children, then buds.
        let t = tree(vec![level(-90.0, 2.0), level(0.0, 3.0), level(0.0, 0.0)]);
        let (skeleton, _) = grow(&t).unwrap();
        let children: Vec<_> = skeleton.branches.iter().filter(|b| b.level == 1).collect();
        assert_eq!(children.len(), 2);
        for child in &children {
            assert_eq!(child.parent, Some(0));
            assert!((0.0..=1.0).contains(&child.percent));
            let base = child.vertices[0].position;
            assert!(base[2] >= 2.0 - 1e-4 && base[2] <= 8.0 + 1e-4, "{base:?}");
            // Level 1 is the first branch level: identity base frame, so the start angle of 0
            // grows it level.
            assert!(child.vertices[0].direction[2].abs() < 1e-5);
        }
        // With three levels, the children of level 1 are buds (1 + 1 ≥ 3 − 1).
        assert!(skeleton.buds.iter().all(|b| b.branch != 0));
        assert_eq!(skeleton.buds.len(), 2 * 3);
        // Growing twice gives the same tree.
        let (again, _) = grow(&t).unwrap();
        assert_eq!(again.branches, skeleton.branches);
    }

    #[test]
    fn pruning_drops_children_below_the_percent() {
        let mut levels = vec![
            level(-90.0, 2.0),
            level(0.0, 0.0),
            level(0.0, 0.0),
            level(0.0, 0.0),
        ];
        levels[0].pruning_percent = 2.0;
        let (skeleton, _) = grow(&tree(levels)).unwrap();
        assert_eq!(skeleton.branches.len(), 1);
    }

    #[test]
    fn frond_rules_prune_or_mark_branches() {
        let levels = vec![
            level(-90.0, 2.0),
            level(0.0, 0.0),
            level(0.0, 0.0),
            level(0.0, 0.0),
        ];
        let mut t = tree(levels);
        t.fronds = Some(Fronds {
            enabled: true,
            level: 1,
            above_condition: 0,
            below_condition: 0,
            ..Fronds::default()
        });
        let (skeleton, _) = grow(&t).unwrap();
        assert_eq!(skeleton.branches.len(), 3);
        assert!(!skeleton.branches[0].frond);
        assert!(
            skeleton.branches[1..]
                .iter()
                .all(|b| b.frond && b.flares.is_empty())
        );
        t.fronds.as_mut().unwrap().above_condition = 2;
        let (skeleton, _) = grow(&t).unwrap();
        assert_eq!(skeleton.branches.len(), 1);
    }

    #[test]
    fn the_floor_lifts_falling_vertices() {
        // A trunk pitched down from 1 unit up: the floor at 0 catches it.
        let mut levels = vec![level(90.0, 0.0), level(0.0, 0.0), level(0.0, 0.0)];
        levels[0].segments = 8;
        let mut t = tree(levels);
        t.floor = Some(Floor {
            enabled: Some(true),
            value: Some(-1.0),
            level: Some(0),
            exponent: Some(1.0),
            bias: Some(0.0),
        });
        let (skeleton, _) = grow(&t).unwrap();
        // The base is at 0 ≥ −1, so the branch counts as above the floor; with bias 0 every
        // vertex that would go below is put on it.
        let zs: Vec<f32> = skeleton.branches[0]
            .vertices
            .iter()
            .map(|v| v.position[2])
            .collect();
        assert!(zs.iter().all(|&z| z >= -1.0 - 1e-5), "{zs:?}");
        assert!(zs.iter().filter(|&&z| z == -1.0).count() >= 2, "{zs:?}");
    }

    #[test]
    fn flares_draw_from_the_c_runtime() {
        let mut levels = vec![level(-90.0, 0.0), level(0.0, 0.0), level(0.0, 0.0)];
        levels[0].flares.count = 3;
        levels[0].flares.balance = 0.5;
        let (skeleton, random) = grow(&tree(levels)).unwrap();
        let flares = &skeleton.branches[0].flares;
        assert_eq!(flares.len(), 3);
        assert!(flares.iter().all(|f| (0.0..=TWO_PI).contains(&f.angle)));
        // Flares draw nothing from the stream.
        assert_eq!(random.draws(), 1000 + 1 + 7 + 5 + 4 * 5);
    }

    #[test]
    fn wind_groups_and_weights_follow_the_levels() {
        // Wind level 1: the trunk (level 0) is rigid; level-1 branches take groups from the
        // counter (starting at the seed) and bend with their progress and flexibility.
        let mut t = tree(vec![level(-90.0, 2.0), level(0.0, 0.0), level(0.0, 0.0)]);
        t.wind_level = Some(1);
        let (skeleton, _) = grow(&t).unwrap();
        let trunk = &skeleton.branches[0];
        assert_eq!(trunk.wind.groups, [1234, 1]);
        assert!(trunk.vertices.iter().all(|v| v.wind.weights == [1.0, 1.0]));
        assert_eq!(
            trunk.vertices[0].wind.output(),
            ([0.0, 0.0], [(1234u32 % 256 % 6) as u8, 1])
        );
        let children: Vec<_> = skeleton.branches.iter().filter(|b| b.level == 1).collect();
        assert_eq!(children.len(), 2);
        assert_eq!(children[0].wind.groups, [1234, 1]);
        assert_eq!(children[1].wind.groups, [1235, 1]);
        for v in &children[0].vertices {
            // flexibility 0.5 · flexibility_scale 1.
            assert!((v.wind.weights[0] - (1.0 - v.progress * 0.5)).abs() < 1e-6);
            assert_eq!(v.wind.weights[1], 1.0);
        }
    }

    #[test]
    fn wind_groups_use_the_stored_seed_and_the_default_flare_seed() {
        // 7.10 item 5: seed 0 stays 0 for the groups (the stream still seeds with 1), and an
        // absent flare seed is 444.
        let mut t = tree(vec![level(-90.0, 0.0), level(0.0, 0.0), level(0.0, 0.0)]);
        t.tree.seed = 0;
        t.flare_seed = None;
        let (skeleton, _) = grow(&t).unwrap();
        assert_eq!(skeleton.branches[0].wind.groups, [0, 444]);
    }
}
