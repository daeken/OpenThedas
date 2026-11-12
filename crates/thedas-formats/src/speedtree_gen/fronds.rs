//! Stage 4: fronds (generation.md 4.2–4.6).
//!
//! Blade ribbons or extruded profiles along the skeletons of frond branches (their guides,
//! recorded in stage 2), with their own LOD selection. Built after branch and leaf LOD.
//! Provisional until checked against the executable.

use super::frame::{
    Frame, Vec3, add, cos, cos_deg, cross, dot, fast_sqrt, lerp, normalize, scale, sin, sin_deg,
    sub,
};
use super::limits::{GenerateError, MAX_SURFACE_VERTICES};
use super::{Curve, Random, Skeleton};
use crate::speedtree::{Fronds, SpeedTree};

#[allow(clippy::approx_constant)]
const HALF_PI: f32 = 1.570796;

/// One frond vertex.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrondVertex {
    pub position: Vec3,
    pub normal: Vec3,
    pub tangent: Vec3,
    pub binormal: Vec3,
    /// Texture coordinates in the frond image, before atlas mapping.
    pub local: [f32; 2],
    /// Atlas coordinates (4.6), in the file's convention (t upward).
    pub texcoord: [f32; 2],
    /// Index into `Fronds::textures`.
    pub texture: usize,
    /// LOD fade hint (4.6); 0 until a level sets it.
    pub fade: f32,
    /// The guide vertex's wind values (7.7).
    pub wind: super::Wind,
}

/// A tree's fronds.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FrondGeometry {
    pub vertices: Vec<FrondVertex>,
    /// Per LOD level, the guides (indices into [`Skeleton::guides`]) drawn.
    pub lods: Vec<Vec<usize>>,
    /// Per LOD level, its triangle strips.
    pub strips: Vec<Vec<Vec<u32>>>,
}

/// Rotation about X by `a` degrees (4.4).
fn about_x(a: f32) -> Frame {
    let (c, s) = (cos_deg(a), sin_deg(a));
    Frame([[1.0, 0.0, 0.0], [0.0, c, s], [0.0, -s, c]])
}

/// `v · M` (row vector times matrix).
fn row_times(v: Vec3, m: &Frame) -> Vec3 {
    let m = &m.0;
    [
        v[0] * m[0][0] + v[1] * m[1][0] + v[2] * m[2][0],
        v[0] * m[0][1] + v[1] * m[1][1] + v[2] * m[2][1],
        v[0] * m[0][2] + v[1] * m[1][2] + v[2] * m[2][2],
    ]
}

fn fast_length(v: Vec3) -> f32 {
    fast_sqrt(dot(v, v))
}

/// Builds the fronds (4.2): guide LOD selection, then each guide's vertices in level-0 order
/// (extrusion profiles draw as they are built), then strips and fade hints.
pub fn build(
    tree: &SpeedTree,
    skeleton: &Skeleton,
    random: &mut Random,
) -> Result<FrondGeometry, GenerateError> {
    let Some(fronds) = tree.fronds.as_ref().filter(|f| f.enabled) else {
        return Ok(FrondGeometry::default());
    };
    let guides = &skeleton.guides;
    let lods = select(fronds, skeleton, random);
    let count = lods.len();
    let mut out = FrondGeometry {
        vertices: Vec::new(),
        strips: vec![Vec::new(); count],
        lods,
    };
    let Some(level0) = out.lods.first().cloned() else {
        return Ok(out);
    };
    // The vertices about to be built, against the limit.
    let per_vertex = if fronds.kind == 0 {
        2 * fronds.blades.max(0) as usize
    } else {
        2 * fronds.profile_segments.max(2) as usize - 1
    };
    let total: usize = level0
        .iter()
        .map(|&g| skeleton.branches[guides[g].branch].vertices.len() * per_vertex)
        .sum();
    if total > MAX_SURFACE_VERTICES {
        return Err(GenerateError::TooLarge {
            what: "frond vertices",
            max: MAX_SURFACE_VERTICES,
        });
    }
    let profile = Curve::new(&fronds.profile.clone().unwrap_or_else(default_profile));
    let fade_extrusions = tree.lod.and_then(|l| l.fade_extrusions).unwrap_or(false);
    let atlas = tree.texcoords.as_ref().map(|t| t.fronds.as_slice());
    for &g in &level0 {
        let guide = &guides[g];
        let branch = &skeleton.branches[guide.branch];
        if branch.vertices.len() < 2 {
            continue;
        }
        let start = out.vertices.len();
        // The levels this guide belongs to (each level's list is a prefix of level 0's).
        let levels: Vec<usize> = (0..count).filter(|&l| out.lods[l].contains(&g)).collect();
        let strips = if fronds.kind == 0 {
            blades(
                &mut out.vertices,
                fronds,
                guide.radius,
                guide.angle_offset,
                branch,
            )
        } else {
            extrusion(
                &mut out.vertices,
                fronds,
                &profile,
                guide.radius,
                guide.angle_offset,
                branch,
                random,
            )
        };
        for v in &mut out.vertices[start..] {
            v.texture = guide.texture;
            v.texcoord = atlas_map(atlas.and_then(|a| a.get(guide.texture)), v.local);
        }
        for &l in &levels {
            let level_strips: Vec<Vec<u32>> = match &strips {
                Strips::Blades(s) => s
                    .iter()
                    .map(|s| s.iter().map(|&i| (start + i) as u32).collect())
                    .collect(),
                Strips::Extrusion { n, v } => {
                    vec![extrusion_strip(
                        fronds,
                        l,
                        count,
                        *n,
                        *v,
                        start,
                        fade_extrusions,
                    )]
                }
            };
            let hint = if l + 1 == count {
                -1.0
            } else {
                1.0 - (l + 1) as f32 / count as f32
            };
            for strip in &level_strips {
                for &i in strip {
                    out.vertices[i as usize].fade = hint;
                }
            }
            out.strips[l].extend(level_strips);
        }
    }
    Ok(out)
}

/// The built-in profile of a file without one (5.7 C): a rounded arch, highest in the middle.
#[allow(clippy::excessive_precision)]
fn default_profile() -> crate::speedtree::BezierSpline {
    use crate::speedtree::{BezierSpline, SplinePoint};
    let point = |x, y, tx, ty, tangent_length| SplinePoint {
        x,
        y,
        tangent: [tx, ty],
        tangent_length,
    };
    BezierSpline {
        min: 0.0,
        max: 1.0,
        variance: 0.0,
        points: vec![
            point(0.0, 0.00138887, 0.337009, 0.941501, 0.132767),
            point(0.493215, 0.998903, 1.0, 0.00102074, 0.23702),
            point(1.0, -6.24607e-8, 0.307222, -0.951638, 0.126974),
        ],
    }
}

/// Maps a frond texture coordinate into its atlas rectangle (4.6).
fn atlas_map(corners: Option<&[f32; 8]>, st: [f32; 2]) -> [f32; 2] {
    match corners {
        Some(c) => [lerp(c[2], c[0], st[0]), lerp(c[5], c[1], st[1])],
        // No texture-coordinate entry: leave the image coordinates (provisional).
        None => st,
    }
}

/// Frond LOD selection (4.3): the guides of each level, by fuzzy surface area.
fn select(fronds: &Fronds, skeleton: &Skeleton, random: &mut Random) -> Vec<Vec<usize>> {
    let guides = &skeleton.guides;
    let count = fronds.lod_count.max(0) as usize;
    let area = |i: usize| guides[i].area;
    let total = (0..guides.len()).fold(0.0f32, |sum, i| sum + area(i));
    let largest = (0..guides.len())
        .map(area)
        .fold(None, |m: Option<f32>, v| Some(m.map_or(v, |m| m.max(v))))
        .unwrap_or(0.0);
    let squared = largest * largest;
    let mut kept = Vec::new();
    let mut others: Vec<(usize, f32)> = Vec::new();
    for i in 0..guides.len() {
        let a = area(i);
        if a > largest * (1.0 - fronds.large_frond_percent) {
            kept.push(i);
        } else {
            let f = random.uniform(0.0, fronds.reduction_fuzziness);
            others.push((i, (1.0 - f) * a + f * squared));
        }
    }
    others.sort_by(|a, b| b.1.total_cmp(&a.1));
    let mut order: Vec<usize> = others.into_iter().map(|(i, _)| i).collect();
    for &k in &kept {
        order.insert(0, k);
    }
    (0..count)
        .map(|l| {
            let fraction = if count == 1 {
                fronds.max_surface_percent
            } else {
                lerp(
                    fronds.max_surface_percent,
                    fronds.min_surface_percent,
                    l as f32 / (count - 1) as f32,
                )
            };
            let target = fraction * total;
            let mut taken = 0;
            if target > 0.0 {
                let mut sum = 0.0f32;
                while taken < order.len() && sum <= target {
                    sum += area(order[taken]);
                    taken += 1;
                }
            }
            order[..taken].to_vec()
        })
        .collect()
}

/// A guide's strips before offsetting: per blade, or the extrusion's grid size.
enum Strips {
    Blades(Vec<Vec<usize>>),
    Extrusion { n: usize, v: usize },
}

/// Blade fronds (4.4).
fn blades(
    out: &mut Vec<FrondVertex>,
    fronds: &Fronds,
    radius: f32,
    offset: f32,
    branch: &super::Branch,
) -> Strips {
    let verts = &branch.vertices;
    let n = verts.len();
    let count = fronds.blades.max(0) as usize;
    let spacing = 180.0 / count as f32;
    let start = out.len();
    let mut strips = Vec::with_capacity(count);
    for b in 0..count {
        let blade_start = out.len();
        let right: Vec<Vec3> = (0..n)
            .map(|i| {
                let frame = verts[if i + 1 == n { i - 1 } else { i }].frame;
                about_x(spacing * b as f32 + offset).then(&frame).0[1]
            })
            .collect();
        for i in 0..n {
            let p = verts[i].position;
            let normal = if i == 0 {
                normalize(sub(verts[1].position, verts[0].position))
            } else {
                normalize(sub(p, verts[0].position))
            };
            let (r, l) = if i + 1 == n {
                (
                    add(p, scale(right[i - 1], radius)),
                    sub(p, scale(right[i - 1], radius)),
                )
            } else if i > 0 {
                (
                    scale(
                        add(
                            add(p, scale(right[i], radius)),
                            add(p, scale(right[i - 1], radius)),
                        ),
                        0.5,
                    ),
                    scale(
                        add(
                            sub(p, scale(right[i], radius)),
                            sub(p, scale(right[i - 1], radius)),
                        ),
                        0.5,
                    ),
                )
            } else {
                (
                    add(p, scale(right[i], radius)),
                    sub(p, scale(right[i], radius)),
                )
            };
            let tangent = right[i];
            let c = cross(normal, tangent);
            let binormal = if fast_length(c) < 0.1 {
                scale(normal, -1.0)
            } else {
                scale(normalize(c), -1.0)
            };
            let t = i as f32 / (n - 1) as f32;
            for (position, s) in [(r, 1.0), (l, 0.0)] {
                out.push(FrondVertex {
                    position,
                    normal,
                    tangent,
                    binormal,
                    local: [s, t],
                    texcoord: [s, t],
                    texture: 0,
                    fade: 0.0,
                    wind: verts[i].wind,
                });
            }
        }
        // Unpinching, measured on the guide's first blade.
        for side in 0..2 {
            let at = |i: usize| out[start + 2 * i + side].position;
            let mut lengths = vec![0.0f32; n];
            for i in 1..n {
                lengths[i] = lengths[i - 1] + fast_length(sub(at(i), at(i - 1)));
            }
            let total = lengths[n - 1];
            for i in 1..n.saturating_sub(1) {
                // A zero-length blade keeps its parametric t (provisional).
                if total != 0.0 {
                    out[blade_start + 2 * i + side].local[1] = 0.999 * lengths[i] / total;
                }
            }
        }
        strips.push((blade_start - start..blade_start - start + 2 * n).collect());
    }
    Strips::Blades(strips)
}

/// Extrusion fronds (4.5): the profile (with its draws), then the vertex grid.
fn extrusion(
    out: &mut Vec<FrondVertex>,
    fronds: &Fronds,
    profile: &Curve,
    radius: f32,
    offset: f32,
    branch: &super::Branch,
    random: &mut Random,
) -> Strips {
    let p = fronds.profile_segments.max(2) as usize;
    let a = profile.value_with_variance(0.0, random) * radius;
    let mut points: Vec<Vec3> = (0..p)
        .map(|i| {
            let f = 1.0 - i as f32 / (p - 1) as f32;
            let z = profile.value_with_variance(f, random) * radius - a;
            [0.0, -f * radius, z]
        })
        .collect();
    for j in (0..p - 1).rev() {
        let [x, y, z] = points[j];
        points.push([x, -y, z]);
    }
    let v = points.len();
    let pair =
        |i: usize, j: usize| (points[j][2] - points[i][2]).atan2(points[j][1] - points[i][1]);
    let mut normals = Vec::with_capacity(v);
    let mut tangents = Vec::with_capacity(v);
    for i in 0..v {
        let angle = if i == p - 1 {
            HALF_PI
        } else if i == 0 {
            pair(0, 1) + HALF_PI
        } else if i == v - 1 {
            pair(v - 2, v - 1) + HALF_PI
        } else {
            0.5 * (pair(i - 1, i) + pair(i, i + 1)) + HALF_PI
        };
        normals.push(normalize([0.0, cos(angle), sin(angle)]));
        tangents.push(normalize([
            0.0,
            -cos(angle + HALF_PI),
            -sin(angle + HALF_PI),
        ]));
    }
    let verts = &branch.vertices;
    let n = verts.len();
    let start = out.len();
    let rotate = about_x(offset);
    for i in 0..n {
        let t = rotate.then(&verts[i].frame);
        let t2 = rotate.then(&verts[if i == 0 { 0 } else { i - 1 }].frame);
        let both = |v: Vec3| scale(add(row_times(v, &t), row_times(v, &t2)), 0.5);
        for j in 0..v {
            let position = add(verts[i].position, both(points[j]));
            let normal = normalize(both(normals[j]));
            let tangent = normalize(both(tangents[j]));
            let c = cross(tangent, normal);
            let binormal = if fast_length(c) < 0.1 {
                normal
            } else {
                normalize(c)
            };
            let st = [j as f32 / (v - 1) as f32, i as f32 / (n - 1) as f32];
            out.push(FrondVertex {
                position,
                normal,
                tangent,
                binormal,
                local: st,
                texcoord: st,
                texture: 0,
                fade: 0.0,
                wind: verts[i].wind,
            });
        }
    }
    // Unpinching per column.
    for j in 0..v {
        let at = |i: usize| out[start + i * v + j].position;
        let mut lengths = vec![0.0f32; n];
        for i in 1..n {
            lengths[i] = lengths[i - 1] + fast_length(sub(at(i), at(i - 1)));
        }
        let total = lengths[n - 1];
        if total != 0.0 {
            for i in 1..n {
                out[start + i * v + j].local[1] = 0.999 * lengths[i] / total;
            }
        }
    }
    Strips::Extrusion { n, v }
}

/// One extrusion strip for LOD level `l` of `count` (4.5).
fn extrusion_strip(
    fronds: &Fronds,
    l: usize,
    count: usize,
    n: usize,
    v: usize,
    start: usize,
    fade_extrusions: bool,
) -> Vec<u32> {
    let fraction = if count == 1 {
        1.0
    } else {
        1.0 - l as f32 / (count - 1) as f32
    };
    let (mut nl, mut vl) = (
        lerp((fronds.min_length_segments + 1) as f32, n as f32, fraction) as usize,
        lerp(
            (2 * fronds.min_cross_segments + 1) as f32,
            v as f32,
            fraction,
        ) as usize,
    );
    if fade_extrusions {
        (nl, vl) = (n, v);
    }
    // Fewer than two of either makes no surface (and divides by zero in the steps).
    if nl < 2 || vl < 2 {
        return Vec::new();
    }
    let length_step = n as f32 / (nl - 1) as f32;
    let cross_step = v as f32 / (vl - 1) as f32;
    let mut out: Vec<u32> = Vec::new();
    for i in 0..=nl - 2 {
        let this = (i as f32 * length_step) as usize;
        let mut next = ((i + 1) as f32 * length_step) as usize;
        if i == nl - 2 || next > n - 1 {
            next = n - 1;
        }
        let skip = next.saturating_sub(this);
        let base = this * v;
        let mut column = |j: usize| {
            let c = if j == vl - 1 {
                v - 1
            } else {
                ((j as f32 * cross_step) as usize).min(v - 1)
            };
            out.push((start + base + c) as u32);
            out.push((start + base + c + skip * v) as u32);
        };
        if i % 2 == 0 {
            (0..vl).for_each(&mut column);
        } else {
            (0..vl).rev().for_each(&mut column);
        }
        if i < nl - 2 {
            let last = out[out.len() - 1];
            out.push(last);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::speedtree::{FrondTexture, Fronds};
    use crate::speedtree_gen::generate;
    use crate::speedtree_gen::skeleton::tests::{level, tree};

    /// A trunk with two level-1 frond branches of 5 vertices.
    fn frondy(kind: i32) -> SpeedTree {
        let mut t = tree(vec![level(-90.0, 2.0), level(0.0, 0.0), level(0.0, 0.0)]);
        t.fronds = Some(Fronds {
            enabled: true,
            level: 1,
            kind,
            blades: 2,
            profile: Some(super::super::skeleton::flat_one()),
            profile_segments: 3,
            textures: vec![FrondTexture::default()],
            lod_count: 2,
            max_surface_percent: 1.0,
            min_surface_percent: 0.0,
            min_length_segments: 2,
            min_cross_segments: 1,
            ..Fronds::default()
        });
        t
    }

    #[test]
    fn blade_fronds_make_ribbons() {
        let t = frondy(0);
        let g = generate(&t).unwrap();
        assert_eq!(g.skeleton.guides.len(), 2);
        assert_eq!(g.fronds.lods[0].len(), 2);
        // Two guides × two blades × 2 · 5 vertices.
        assert_eq!(g.fronds.vertices.len(), 2 * 2 * 10);
        assert_eq!(g.fronds.strips[0].len(), 4);
        assert!(g.fronds.strips[1].is_empty());
        let strip = &g.fronds.strips[0][0];
        assert_eq!(strip, &(0..10).collect::<Vec<u32>>());
        // A blade is radius·2 wide: the guide radius is length · 0.5 aspect · 0.5 · scale.
        let guide = g.skeleton.guides[g.fronds.lods[0][0]];
        let v = &g.fronds.vertices;
        let width = sub(v[0].position, v[1].position);
        assert!((dot(width, width).sqrt() - 2.0 * guide.radius).abs() < 1e-3);
        // Unpinched t runs 0 → 0.999 · … → 1 along the blade.
        assert_eq!(v[0].local[1], 0.0);
        assert_eq!(v[8].local[1], 1.0);
        assert!(v[2].local[1] > 0.0 && v[2].local[1] < v[4].local[1]);
        assert!(v.iter().all(|x| x.fade == -1.0 || x.fade == 0.5));
    }

    #[test]
    fn extrusion_fronds_make_a_grid() {
        let t = frondy(1);
        let g = generate(&t).unwrap();
        // Profile of 3 points mirrored to 5; 5 guide vertices per guide.
        assert_eq!(g.fronds.vertices.len(), 2 * 5 * 5);
        let strip = &g.fronds.strips[0][0];
        assert!(!strip.is_empty());
        assert!(strip.iter().all(|&i| (i as usize) < 25));
        // The centre point of the flat profile lies on the guide.
        let guide = g.skeleton.guides[g.fronds.lods[0][0]];
        let base = g.skeleton.branches[guide.branch].vertices[0].position;
        let centre = g.fronds.vertices[2].position;
        assert!(
            sub(centre, base).iter().all(|x| x.abs() < 1e-4),
            "{centre:?}"
        );
    }

    #[test]
    fn disabled_fronds_build_nothing() {
        let mut t = frondy(0);
        t.fronds.as_mut().unwrap().enabled = false;
        let g = generate(&t).unwrap();
        assert!(g.fronds.vertices.is_empty());
    }

    #[test]
    fn a_missing_profile_uses_the_built_in_arch() {
        let mut t = frondy(1);
        t.fronds.as_mut().unwrap().profile = None;
        let g = generate(&t).unwrap();
        assert!(!g.fronds.vertices.is_empty());
        let arch = Curve::new(&default_profile());
        assert!(arch.value(0.5) > 0.99 && arch.value(0.0) < 0.01 && arch.value(1.0).abs() < 0.01);
    }
}
