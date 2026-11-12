//! Stage 3: branch geometry and branch LOD (generation.md 3.1–3.7).
//!
//! Tubes of vertices around every real branch's skeleton, their texture coordinates, normals
//! and triangle strips, and the volume-based selection of branches for each LOD level.
//! Provisional until checked against the executable.

use super::frame::{Vec3, add, cross, lerp, lerp3, normalize, scale, sub};
use super::skeleton::{BANK, Branch, Flare};
use super::{Curve, Random, Skeleton};
use crate::speedtree::{BezierSpline, BranchLevel, SpeedTree, TEXTURE_LAYERS, TexLayer};

/// 2π and π/2 as the original's float constants.
#[allow(clippy::approx_constant)]
const TWO_PI: f32 = 6.283185;
#[allow(clippy::approx_constant)]
const HALF_PI: f32 = 1.570796;
#[allow(clippy::approx_constant)]
const PI: f32 = 3.141593;

/// One vertex of a branch tube.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BranchVertex {
    pub position: Vec3,
    pub normal: Vec3,
    pub tangent: Vec3,
    pub binormal: Vec3,
    /// (s, t) per texture layer (diffuse, detail, normal, height, specular, user 1, user 2).
    pub texcoords: [[f32; 2]; TEXTURE_LAYERS],
    /// LOD fade hint (3.7 step 6); 0 until a LOD level sets it.
    pub fade: f32,
}

/// One branch's tube in the shared vertex list.
#[derive(Clone, Debug, PartialEq)]
pub struct Tube {
    /// Index of the branch's first vertex (its start).
    pub start: usize,
    /// Points per skeleton vertex (each cross section's count plus the seam point).
    pub points: Vec<usize>,
    /// The triangle strip over the tube (3.6), as indices into the shared list.
    pub strip: Vec<u32>,
}

/// A tree's branch geometry.
#[derive(Clone, Debug, PartialEq)]
pub struct BranchGeometry {
    pub vertices: Vec<BranchVertex>,
    /// Per skeleton branch (same indices as [`Skeleton::branches`]): its tube, if it has one.
    pub tubes: Vec<Option<Tube>>,
    /// Per LOD level, the branches (skeleton indices) drawn, in list order.
    pub lods: Vec<Vec<usize>>,
    /// Bank values the roughness took (the roughness index, 2.3).
    pub roughness_index: usize,
}

impl BranchGeometry {
    /// The strips of LOD level `k`, one per drawn branch with geometry.
    pub fn lod_strips(&self, k: usize) -> impl Iterator<Item = &[u32]> {
        self.lods
            .get(k)
            .into_iter()
            .flatten()
            .filter_map(|&b| self.tubes[b].as_ref().map(|t| t.strip.as_slice()))
    }
}

/// The curves stage 3 reads, per level.
struct GeometryCurves {
    roughness_profile: Curve,
    light_seam_profile: Curve,
    light_seam_bias: Curve,
}

impl GeometryCurves {
    fn new(level: &BranchLevel) -> Self {
        let profile = |s: &Option<BezierSpline>| {
            Curve::new(&s.clone().unwrap_or_else(super::skeleton::flat_one))
        };
        GeometryCurves {
            roughness_profile: profile(&level.roughness_profile),
            light_seam_profile: profile(&level.light_seam_profile),
            light_seam_bias: profile(&level.light_seam_bias),
        }
    }
}

/// Builds the branch tubes (3.2–3.6) in growth order, then selects branches for each LOD
/// level (3.7), drawing from `random`.
pub fn build(tree: &SpeedTree, skeleton: &Skeleton, random: &mut Random) -> BranchGeometry {
    let levels: Vec<GeometryCurves> = tree.branch_levels.iter().map(GeometryCurves::new).collect();
    let roots = tree.roots.as_ref().map(|r| GeometryCurves::new(&r.roots));
    let first_level = tree.cluster_first_branch_level.unwrap_or(0);
    let mut out = BranchGeometry {
        vertices: Vec::new(),
        tubes: Vec::with_capacity(skeleton.branches.len()),
        lods: Vec::new(),
        roughness_index: 0,
    };
    for branch in &skeleton.branches {
        let (info, curves) = if branch.root {
            match (tree.roots.as_ref(), roots.as_ref()) {
                (Some(r), Some(c)) => (&r.roots, c),
                _ => {
                    out.tubes.push(None);
                    continue;
                }
            }
        } else {
            match usize::try_from(branch.level)
                .ok()
                .and_then(|l| Some((tree.branch_levels.get(l)?, levels.get(l)?)))
            {
                Some(x) => x,
                None => {
                    out.tubes.push(None);
                    continue;
                }
            }
        };
        let tube = if !branch.frond && branch.level >= first_level && branch.vertices.len() >= 2 {
            Some(tube(
                &mut out,
                branch,
                info,
                curves,
                skeleton.size,
                &skeleton.bank,
            ))
        } else {
            None
        };
        out.tubes.push(tube);
    }
    lod(tree, skeleton, &mut out, random);
    out
}

/// Per-branch values of 3.1.
struct BranchValues {
    tile_offset: f32,
    base_radius: f32,
    relative_length: f32,
    negated: bool,
}

/// One branch's tube: cross sections (3.2), normals (3.5) and strip (3.6).
fn tube(
    out: &mut BranchGeometry,
    branch: &Branch,
    info: &BranchLevel,
    curves: &GeometryCurves,
    size: f32,
    bank: &[f32],
) -> Tube {
    let start = out.vertices.len();
    let values = BranchValues {
        tile_offset: (branch.azimuth + 180.0) / 360.0 + branch.percent,
        base_radius: branch.vertices[0].radius,
        relative_length: branch.length / size,
        negated: branch.gnarl_negated,
    };
    let mut points = Vec::with_capacity(branch.vertices.len());
    for v in &branch.vertices {
        let m = v.cross_sections.max(0) as usize;
        let t = v.progress;
        let rows = v.frame.0;
        let roughness = curves.roughness_profile.value(t);
        let mut f = 0.0f32;
        let step = 1.0 / m as f32;
        let mut first = v.position;
        for i in 0..=m {
            let alpha = TWO_PI * f;
            let mut texcoords = [[0.0; 2]; TEXTURE_LAYERS];
            for (layer, tc) in texcoords.iter_mut().enumerate() {
                let controls = if layer != 0 && info.layers[layer].sync {
                    &info.layers[0]
                } else {
                    &info.layers[layer]
                };
                *tc = texcoord(controls, f, t, &values);
            }
            let (ca, sa) = (alpha.cos(), alpha.sin());
            let radial = add(scale(rows[1], ca), scale(rows[2], sa));
            let (ct, st) = ((alpha + HALF_PI).cos(), (alpha + HALF_PI).sin());
            let tangent = add(scale(rows[1], ct), scale(rows[2], st));
            let binormal = cross(radial, tangent);
            let position = if i < m {
                let mut r = v.radius;
                // A zero maximum (a zero radius curve) makes 0/0 in the original; take 0, so
                // the tube collapses onto its skeleton (provisional, asked of the spec).
                let k = if branch.max_radius != 0.0 {
                    v.radius / branch.max_radius
                } else {
                    0.0
                };
                r += k
                    * info.roughness
                    * size
                    * roughness
                    * (info.roughness_vertical_frequency * t).sin()
                    * (info.roughness_horizontal_frequency * t).sin()
                    * (info.roughness_horizontal_frequency * alpha).cos();
                if r > branch.max_radius {
                    r = branch.max_radius;
                }
                let b = bank[out.roughness_index % BANK];
                out.roughness_index += 1;
                r += (b - 0.5) * 2.0 * info.random_roughness * size * k * roughness;
                let mut flare = 1.0f32;
                for fl in &branch.flares {
                    flare += flare_distance(fl, alpha, t);
                }
                let p = add(v.position, scale(scale(radial, r), flare));
                if i == 0 {
                    first = p;
                }
                p
            } else {
                first
            };
            out.vertices.push(BranchVertex {
                position,
                normal: radial,
                tangent,
                binormal,
                texcoords,
                fade: 0.0,
            });
            f += step;
        }
        points.push(m + 1);
    }
    normals(out, branch, curves, start, &points);
    let strip = strip(start, &points);
    Tube {
        start,
        points,
        strip,
    }
}

/// Texture coordinates of one layer (3.3).
fn texcoord(layer: &TexLayer, f: f32, t: f32, v: &BranchValues) -> [f32; 2] {
    let s_tile = if layer.s_tile_absolute {
        layer.s_tile
    } else {
        layer.s_tile * v.base_radius * TWO_PI
    };
    let t_tile = if layer.t_tile_absolute {
        layer.t_tile
    } else {
        layer.t_tile * v.relative_length
    };
    let twist = if v.negated { -layer.twist } else { layer.twist };
    let s = if layer.clamp_s {
        let (left, right) = (layer.clamp[0], layer.clamp[1]);
        let mut start = layer.s_offset * 90.0;
        if start < 0.0 {
            start = 0.0;
        }
        while start > 360.0 {
            start -= 360.0;
        }
        while start < 0.0 {
            start += 360.0;
        }
        let sweep = if s_tile == 0.0 { 0.0 } else { 360.0 / s_tile };
        let mut end = start + sweep;
        if end > 360.0 {
            end = 360.0;
            start = 360.0 - sweep;
        }
        let this = (f * 360.0).max(start).min(end);
        // A zero sweep divides by zero in the original; take the left edge (provisional; no
        // game file has a zero tile).
        if sweep != 0.0 {
            lerp(left, right, (this - start) / sweep)
        } else {
            left
        }
    } else {
        f * s_tile + layer.s_offset + twist * t
    };
    let offset = if layer.random_t_offset {
        v.tile_offset
    } else {
        0.0
    };
    let mut tt = (t + offset) * t_tile - layer.t_offset;
    if layer.clamp_t {
        tt = lerp(layer.clamp[2], layer.clamp[3], tt.clamp(0.0, 1.0));
    }
    [s, tt]
}

/// One flare's widening at angle `alpha` and branch progress `t` (3.4).
pub fn flare_distance(flare: &Flare, alpha: f32, t: f32) -> f32 {
    let (mut a, mut b) = (alpha, flare.angle);
    let mut difference = (a - b).abs();
    if difference > PI {
        if a < b {
            a += TWO_PI;
        } else {
            b += TWO_PI;
        }
        difference = (a - b).abs();
    }
    let ri = flare.radial_influence;
    let li = flare.length_influence;
    if difference < ri && li - t > 0.0 {
        let radial = f64::from(1.0 - difference / ri).powf(f64::from(flare.radial_exponent)) as f32;
        let vertical = f64::from((li - t) / li).powf(f64::from(flare.length_exponent)) as f32;
        flare.distance * radial * vertical
    } else {
        0.0
    }
}

/// Normals of one branch's tube from its neighbouring points (3.5).
fn normals(
    out: &mut BranchGeometry,
    branch: &Branch,
    curves: &GeometryCurves,
    start: usize,
    points: &[usize],
) {
    let n = branch.vertices.len();
    let bias = curves.light_seam_bias.value(branch.percent);
    let mut offset = start;
    for i in 0..n {
        let m = points[i] - 1;
        let d = branch.vertices[i].direction;
        let progress = i as f32 / (n - 1) as f32;
        let seam = curves.light_seam_profile.value(progress);
        for j in 0..=m {
            let position = |k: usize| out.vertices[k].position;
            let prev = offset + if j == 0 { m.saturating_sub(1) } else { j - 1 };
            let next = offset + if j == m { 1 } else { j + 1 };
            let h = normalize(sub(position(next), position(prev)));
            let g = j as f32 / m as f32;
            let prev = if i == 0 {
                offset + j
            } else {
                let mp = points[i - 1] - 1;
                offset - ((mp + 1) - (mp as f32 * g) as usize)
            };
            let next = if i == n - 1 {
                offset + j
            } else {
                let mn = points[i + 1] - 1;
                offset + (m + 1) + (mn as f32 * g) as usize
            };
            let w = normalize(sub(position(next), position(prev)));
            let mut normal = cross(h, w);
            if normal == [0.0; 3] {
                normal = d;
            }
            let weight = seam * lerp(bias, 1.0, progress.powf(0.5));
            out.vertices[offset + j].normal = normalize(lerp3(d, normal, weight));
        }
        offset += m + 1;
    }
}

/// The triangle strip over a tube (3.6).
fn strip(start: usize, points: &[usize]) -> Vec<u32> {
    let mut out: Vec<u32> = Vec::new();
    let mut run = 0;
    for pair in points.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let big = a.max(b);
        for j in 0..big {
            // M = 1 would divide by zero; tubes always have at least four points (provisional).
            let g = if big > 1 {
                j as f32 / (big - 1) as f32
            } else {
                0.0
            };
            let x = ((a - 1) as f32 * g + 0.5) as usize;
            let y = ((b - 1) as f32 * g + 0.5) as usize;
            out.push((start + run + a + y) as u32);
            out.push((start + run + x) as u32);
        }
        if out.len() >= 2 {
            let again = out[out.len() - 2];
            out.push(again);
            out.push(again);
        }
        run += a;
    }
    out
}

/// Branch LOD (3.7): orders branches by fuzzy volume and fills each level up to its share of
/// the total volume, then sets fade hints.
fn lod(tree: &SpeedTree, skeleton: &Skeleton, out: &mut BranchGeometry, random: &mut Random) {
    let lod = tree.lod.unwrap_or_default();
    let count = lod.branch_lod_count.unwrap_or(1).max(0) as usize;
    let max_percent = lod.branch_max_volume_percent.unwrap_or(1.0);
    let min_percent = lod.branch_min_volume_percent.unwrap_or(1.0);
    let fuzziness = lod.branch_reduction_fuzziness.unwrap_or(0.0);
    let large = lod.large_branch_percent.unwrap_or(0.0);

    // Step 1: growth order without fronds.
    let list: Vec<usize> = (0..skeleton.branches.len())
        .filter(|&i| !skeleton.branches[i].frond)
        .collect();
    let volume = |i: usize| skeleton.branches[i].volume;
    // Step 2.
    let total = list.iter().fold(0.0f32, |sum, &i| sum + volume(i).max(0.0));
    let largest = list
        .iter()
        .map(|&i| volume(i))
        .fold(None, |m: Option<f32>, v| Some(m.map_or(v, |m| m.max(v))))
        .unwrap_or(0.0);
    // Step 3.
    let mut kept = Vec::new();
    let mut others: Vec<(usize, f32)> = Vec::new();
    for &i in &list {
        let v = volume(i);
        if v > largest * (1.0 - large) {
            kept.push(i);
        } else {
            let f = random.uniform(0.0, fuzziness);
            others.push((i, ((1.0 - f) * v + f * largest).max(0.0)));
        }
    }
    // Step 4: descending by fuzzy volume; ties keep list order (provisional).
    others.sort_by(|a, b| b.1.total_cmp(&a.1));
    let mut order: Vec<usize> = others.into_iter().map(|(i, _)| i).collect();
    for &k in &kept {
        order.insert(0, k);
    }
    // Step 5.
    out.lods = (0..count)
        .map(|k| {
            let fraction = if count < 2 {
                1.0
            } else {
                lerp(max_percent, min_percent, k as f32 / (count - 1) as f32)
            };
            let target = fraction * total;
            let mut sum = 0.0f32;
            let mut level = Vec::new();
            for &i in &order {
                if sum >= target {
                    break;
                }
                sum += volume(i);
                level.push(i);
            }
            level
        })
        .collect();
    // Step 6: fade hints.
    for k in 0..count {
        let hint = if k + 1 == count {
            -1.0
        } else {
            1.0 - (k + 1) as f32 / count as f32
        };
        for &b in &out.lods[k] {
            if let Some(tube) = &out.tubes[b] {
                for &index in &tube.strip {
                    out.vertices[index as usize].fade = hint;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::speedtree::Lod;
    use crate::speedtree_gen::skeleton::tests::{level, tree};
    use crate::speedtree_gen::{frame::dot, grow};

    /// A straight trunk of length 10 and radius 1 with level 0 as a branch level.
    fn trunk() -> SpeedTree {
        let mut levels = vec![level(-90.0, 0.0), level(0.0, 0.0), level(0.0, 0.0)];
        levels[0].cross_section_segments = 4;
        let mut t = tree(levels);
        t.cluster_first_branch_level = Some(0);
        t
    }

    fn build_tree(t: &SpeedTree) -> (Skeleton, BranchGeometry) {
        let (skeleton, mut random) = grow(t);
        let geometry = build(t, &skeleton, &mut random);
        (skeleton, geometry)
    }

    #[test]
    fn a_trunk_gets_a_ring_of_points_per_vertex() {
        let (skeleton, g) = build_tree(&trunk());
        let tube = g.tubes[0].as_ref().unwrap();
        // Five skeleton vertices, each 4 points plus the seam point.
        assert_eq!(tube.points, vec![5; 5]);
        assert_eq!(g.vertices.len(), 25);
        assert_eq!(g.roughness_index, 20);
        for (i, v) in skeleton.branches[0].vertices.iter().enumerate() {
            let ring = &g.vertices[i * 5..i * 5 + 5];
            for p in &ring[..4] {
                let d = sub(p.position, v.position);
                // Radius 1 around the skeleton, in the plane across it.
                assert!((dot(d, d).sqrt() - 1.0).abs() < 1e-4, "{d:?}");
                assert!(dot(d, v.direction).abs() < 1e-4);
                // Normals point outward on a straight tube.
                assert!(dot(p.normal, normalize(d)) > 0.99, "{:?}", p.normal);
            }
            assert_eq!(ring[4].position, ring[0].position);
        }
    }

    #[test]
    fn strips_index_the_tube() {
        let (_, g) = build_tree(&trunk());
        let tube = g.tubes[0].as_ref().unwrap();
        // Per pair: 5 · 2 indices and the repeat twice.
        assert_eq!(tube.strip.len(), 4 * 12);
        assert_eq!(&tube.strip[..4], &[5, 0, 6, 1]);
        assert_eq!(&tube.strip[10..12], &[9, 9]);
        assert!(tube.strip.iter().all(|&i| (i as usize) < g.vertices.len()));
    }

    #[test]
    fn a_skeleton_only_trunk_has_no_tube() {
        let (_, g) = build_tree(&tree(vec![
            level(-90.0, 0.0),
            level(0.0, 0.0),
            level(0.0, 0.0),
        ]));
        assert!(g.tubes[0].is_none());
        assert!(g.vertices.is_empty());
    }

    #[test]
    fn texture_coordinates_wrap_and_run_along() {
        let mut t = trunk();
        let layer = &mut t.branch_levels[0].layers[0];
        layer.s_tile = 2.0;
        layer.s_tile_absolute = true;
        layer.t_tile = 3.0;
        layer.t_tile_absolute = true;
        layer.s_offset = 0.0;
        layer.t_offset = 0.0;
        layer.twist = 0.0;
        layer.random_t_offset = false;
        layer.clamp_s = false;
        layer.clamp_t = false;
        let (_, g) = build_tree(&t);
        let ring = |i: usize| &g.vertices[i * 5..i * 5 + 5];
        let s: Vec<f32> = ring(0).iter().map(|v| v.texcoords[0][0]).collect();
        assert_eq!(s, vec![0.0, 0.5, 1.0, 1.5, 2.0]);
        assert_eq!(ring(4)[0].texcoords[0][1], 3.0);
        assert_eq!(ring(2)[0].texcoords[0][1], 1.5);
    }

    #[test]
    fn flares_widen_only_inside_their_reach() {
        let flare = Flare {
            angle: 1.0,
            radial_exponent: 1.0,
            length_exponent: 1.0,
            radial_influence: 0.5,
            length_influence: 0.4,
            distance: 2.0,
        };
        assert_eq!(flare_distance(&flare, 1.0, 0.0), 2.0);
        assert!((flare_distance(&flare, 1.25, 0.2) - 2.0 * 0.5 * 0.5).abs() < 1e-6);
        assert_eq!(flare_distance(&flare, 2.0, 0.0), 0.0);
        assert_eq!(flare_distance(&flare, 1.0, 0.5), 0.0);
        // Across 0/2π.
        let wrap = Flare {
            angle: 0.1,
            ..flare
        };
        assert!(flare_distance(&wrap, TWO_PI - 0.1, 0.0) > 0.0);
    }

    #[test]
    fn lod_keeps_the_largest_branches_longest() {
        // A trunk with two children, three LOD levels from 100% to 10% of the volume.
        let mut levels = vec![level(-90.0, 2.0), level(0.0, 0.0), level(0.0, 0.0)];
        levels[0].cross_section_segments = 4;
        levels[1].cross_section_segments = 4;
        levels[1].length = Some({
            let mut s = super::super::skeleton::flat_one();
            s.max = 0.3;
            s
        });
        let mut t = tree(levels);
        t.cluster_first_branch_level = Some(0);
        t.lod = Some(Lod {
            branch_lod_count: Some(3),
            branch_max_volume_percent: Some(1.0),
            branch_min_volume_percent: Some(0.1),
            branch_reduction_fuzziness: Some(0.0),
            large_branch_percent: Some(0.0),
            ..Lod::default()
        });
        let (skeleton, g) = build_tree(&t);
        assert_eq!(skeleton.branches.len(), 3);
        assert_eq!(g.lods.len(), 3);
        assert_eq!(g.lods[0], vec![0, 1, 2]);
        assert_eq!(g.lods[2], vec![0]);
        // The trunk is drawn at the last level, so its hint is −1; a child last drawn at level
        // k has 1 − (k + 1)/3.
        assert_eq!(g.vertices[0].fade, -1.0);
        let child = g.tubes[1].as_ref().unwrap().start;
        let k = if g.lods[1].contains(&1) { 1 } else { 0 };
        assert!((g.vertices[child].fade - (1.0 - (k + 1) as f32 / 3.0)).abs() < 1e-6);
    }
}
