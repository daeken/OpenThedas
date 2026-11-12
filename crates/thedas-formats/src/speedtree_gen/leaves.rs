//! Stage 5: leaves (generation.md 5.2–5.4).
//!
//! Leaves are made at buds while the tree grows (their draws interleave with the branches'),
//! so [`super::skeleton`] calls [`make_leaf`]; leaf LOD runs after branch LOD. Provisional
//! until checked against the executable.

use super::Random;
use super::frame::{Frame, Vec3, cross, dot, fast_sqrt, lerp, lerp3, normalize, scale, sub};
use super::skeleton::{BANK, Branch, Wind};
use crate::speedtree::SpeedTree;

const UP: Vec3 = [0.0, 0.0, 1.0];
const DOWN: Vec3 = [0.0, 0.0, -1.0];

/// One corner's frame of a leaf card.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Corner {
    pub normal: Vec3,
    pub tangent: Vec3,
    pub binormal: Vec3,
    pub color: [f32; 3],
}

/// One leaf.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Leaf {
    pub position: Vec3,
    /// Texture choice: `2i` for texture `i`, `2i + 1` for its mirrored variant.
    pub choice: usize,
    /// The spawning branch in the skeleton.
    pub branch: usize,
    /// `(short)(255 · scale)`.
    pub color_scale: i16,
    /// Rocking group, 0–2.
    pub group: i32,
    pub corners: [Corner; 4],
    /// Wind values from the bud (7.7), weights clamped to [0, 1].
    pub wind: Wind,
}

impl Leaf {
    /// The leaf texture index.
    pub fn texture(&self) -> usize {
        self.choice / 2
    }
}

/// A leaf texture's card size (2.3 step 5): its relative size times the stored tree size.
pub fn card_size(tree: &SpeedTree, texture: usize) -> [f32; 2] {
    let x = &tree.leaves.textures[texture];
    [x.size[0] * tree.tree.size, x.size[1] * tree.tree.size]
}

/// Rotation about Y by `a` degrees (5.2 step 3).
pub fn about_y(a: f32) -> Frame {
    let (c, s) = (super::frame::cos_deg(a), super::frame::sin_deg(a));
    Frame([[c, 0.0, -s], [0.0, 1.0, 0.0], [s, 0.0, c]])
}

/// What making a leaf needs from the growing tree.
pub struct LeafContext<'a> {
    pub tree: &'a SpeedTree,
    pub branches: &'a [Branch],
    pub bank: &'a [f32],
    pub random: &'a mut Random,
    pub leaves: &'a mut Vec<Leaf>,
    /// The spawning branch's mesh counter (5.3 step 8).
    pub mesh_counter: &'a mut usize,
}

/// Makes a leaf at `position` (5.3), or nothing when it has no texture or is crowded out.
#[allow(clippy::too_many_arguments)]
pub fn make_leaf(
    cx: LeafContext<'_>,
    position: Vec3,
    percent: f32,
    branch: usize,
    nu: Vec3,
    delta: Vec3,
    wind: Wind,
) {
    let tree = cx.tree;
    let info = &tree.leaves;
    let textures = &info.textures;
    // Step 1.
    if textures.is_empty() {
        return;
    }
    let mut blossoms = Vec::new();
    let mut plain = Vec::new();
    for (i, x) in textures.iter().enumerate() {
        let list = if x.blossom { &mut blossoms } else { &mut plain };
        list.push(2 * i);
        list.push(2 * i + 1);
    }
    // Step 2: blossom?
    let mut blossom = false;
    if !blossoms.is_empty() {
        let test = if info.blossom_level == 0 {
            percent
        } else {
            let mut current = branch;
            let mut count = 1;
            while let Some(parent) = cx.branches[current].parent {
                if count >= info.blossom_level {
                    break;
                }
                current = parent;
                count += 1;
            }
            cx.branches[current].percent
        };
        let d = info.blossom_distance;
        let near = if d < 0.0 { test < -d } else { test > d };
        if near {
            blossom = cx.random.uniform(0.0, 1.0) <= info.blossom_weighting;
        }
    }
    // Step 3: the choice.
    let choice = if blossom {
        if blossoms.len() > 1 {
            blossoms[cx.random.uniform(0.0, 100_000.0) as u32 as usize % blossoms.len()]
        } else {
            blossoms[0]
        }
    } else {
        match plain.len() {
            0 => return,
            1 => plain[0],
            n => plain[cx.random.uniform(0.0, 1_000_000.0) as u32 as usize % n],
        }
    };
    let texture = &textures[choice / 2];
    // Step 4: crowding.
    if info.collision != 0 {
        let [w, h] = card_size(tree, choice / 2);
        let side = w.max(h) * info.spacing_tolerance;
        let crowded = cx.leaves.iter().rev().any(|other| {
            (info.collision != 1 || other.branch == branch)
                && (0..3).all(|k| (other.position[k] - position[k]).abs() < side)
        });
        if crowded {
            return;
        }
    }
    // Step 5: colour scale.
    let scale_value = if info.dimming {
        let mut d = percent;
        if info.dimming_depth != 0 {
            let mut current = Some(branch);
            let mut count = 0;
            while let Some(c) = current {
                if count >= info.dimming_depth {
                    break;
                }
                d *= cx.branches[c].percent;
                current = cx.branches[c].parent;
                count += 1;
            }
        }
        lerp(d, 1.0, 1.0 - info.dimming_scalar)
    } else {
        1.0
    };
    let color_scale = (255.0 * scale_value) as i16;
    // Step 6.
    let group = cx.random.uniform(0.0, 10_000.0) as i32 % 3;
    // Step 7: orientation.
    let mesh = texture.mesh.filter(|m| m.use_meshes);
    let mut n = if fast_sqrt(dot(nu, nu)) < 0.0001 {
        delta
    } else {
        lerp3(delta, nu, texture.color_variance)
    };
    n = normalize(n);
    let n0 = n;
    if let Some(m) = mesh {
        n = normalize(lerp3(n, UP, m.hang));
    }
    let mut tangent = normalize(cross(n, UP));
    let mut binormal = normalize(cross(tangent, n));
    if fast_sqrt(dot(tangent, tangent)) < 0.5 {
        binormal = normalize([-n0[0], -n0[1], 0.0]);
        tangent = normalize(cross(n, binormal));
    }
    // Step 8: mesh rotation.
    if let Some(m) = mesh.filter(|m| m.rotate != 0.0) {
        let k = *cx.mesh_counter;
        *cx.mesh_counter += 1;
        let angle = cx.bank[k % BANK] * 2.0 * m.rotate - m.rotate;
        let rotated = Frame::about_z(angle).then(&Frame([tangent, binormal, n]));
        [tangent, binormal, n] = rotated.0;
    }
    // Steps 9–10.
    let offset =
        |cx_random: &mut Random| cx_random.uniform(-texture.color_variance, texture.color_variance);
    let mut corners = [Corner {
        normal: n,
        tangent,
        binormal,
        color: [0.0; 3],
    }; 4];
    if info.smoothing_enabled && mesh.is_none() {
        let m = Frame([tangent, binormal, n]);
        let a = info.smoothing_angle;
        let t = a / 100.0;
        let plan = [(-a, UP), (a, UP), (a, DOWN), (-a, DOWN)];
        for (corner, (angle, toward)) in corners.iter_mut().zip(plan) {
            let row = about_y(angle).then(&m).0[2];
            let normal = normalize(lerp3(row, toward, t));
            let tangent = normalize(cross(normal, UP));
            corner.normal = normal;
            corner.tangent = tangent;
            corner.binormal = normalize(cross(tangent, normal));
        }
    }
    let shift = offset(cx.random);
    for corner in &mut corners {
        corner.color = [
            texture.color[0] + shift,
            texture.color[1] + shift,
            texture.color[2] + shift,
        ];
    }
    cx.leaves.push(Leaf {
        position,
        choice,
        branch,
        color_scale,
        group,
        corners,
        wind,
    });
}

/// Leaf LOD levels (5.4): level 0 is the full list; each further level pairs and merges the
/// one before, drawing from `random`.
pub fn lods(tree: &SpeedTree, leaves: &[Leaf], random: &mut Random) -> Vec<Vec<Leaf>> {
    let lod = tree.lod.unwrap_or_default();
    let count = lod.leaf_lod_count.unwrap_or(1).max(1) as usize;
    let reduction = lod.leaf_reduction.unwrap_or(0.0);
    let growth = lod.leaf_size_increase.unwrap_or(0.0);
    let info = &tree.leaves;
    let largest = (0..info.textures.len())
        .map(|i| {
            let [w, h] = card_size(tree, i);
            w.max(h) * info.spacing_tolerance
        })
        .fold(None, |m: Option<f32>, v| Some(m.map_or(v, |m| m.max(v))));
    let pairing = 10.0 * largest.unwrap_or(10.0);
    let mut levels = vec![leaves.to_vec()];
    for k in 1..count {
        let previous = &levels[k - 1];
        // Step 1: pairing.
        let mut paired = vec![false; previous.len()];
        let mut pairs = Vec::new();
        for i in 0..previous.len() {
            let mut best: Option<(usize, f32)> = None;
            for j in i + 1..previous.len() {
                if paired[j] {
                    break;
                }
                let d = sub(previous[j].position, previous[i].position);
                let distance = fast_sqrt(dot(d, d));
                if distance < pairing && best.is_none_or(|(_, b)| distance < b) {
                    best = Some((j, distance));
                }
            }
            if let Some((j, _)) = best {
                paired[j] = true;
                pairs.push((i, j));
            }
        }
        // Step 2: merging.
        let mut level = Vec::new();
        for (i, j) in pairs {
            let u = random.uniform(0.0, 1.0);
            if u > reduction {
                let (a, b) = (&previous[i], &previous[j]);
                let mut leaf = *a;
                leaf.position = scale(super::frame::add(a.position, b.position), 0.5);
                let texture = &info.textures[a.texture()];
                let [_, h] = card_size(tree, a.texture());
                leaf.position[2] +=
                    (texture.origin[1] - 0.5) * ((1.0 + growth * k as f32) * h * 0.5);
                for (corner, other) in leaf.corners.iter_mut().zip(&b.corners) {
                    for c in 0..3 {
                        corner.color[c] = (corner.color[c] + other.color[c]) * 0.5;
                    }
                }
                level.push(leaf);
            }
        }
        levels.push(level);
    }
    levels
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::speedtree::{LeafTexture, Lod};
    use crate::speedtree_gen::skeleton::tests::{level, tree};
    use crate::speedtree_gen::{generate, grow};

    /// A trunk with two children, each bearing three buds, and one leaf texture.
    fn leafy() -> SpeedTree {
        let mut t = tree(vec![level(-90.0, 2.0), level(0.0, 3.0), level(0.0, 0.0)]);
        t.leaves.textures = vec![LeafTexture {
            color: [0.5, 0.6, 0.7],
            color_variance: 0.0,
            origin: [0.5, 0.5, 0.0],
            size: [0.1, 0.1, 0.0],
            ..LeafTexture::default()
        }];
        t.leaves.collision = 0;
        t.leaves.dimming = false;
        t.leaves.smoothing_enabled = false;
        t
    }

    #[test]
    fn every_bud_makes_a_leaf_with_its_draws() {
        let t = leafy();
        let (skeleton, random) = grow(&t).unwrap();
        assert_eq!(skeleton.buds.len(), 6);
        assert_eq!(skeleton.leaves.len(), 6);
        // Without leaf textures the same tree draws 3 fewer per bud: bud length and azimuth
        // stay, the choice (two choices), group and colour go.
        let mut bare = t.clone();
        bare.leaves.textures.clear();
        let (_, bare_random) = grow(&bare).unwrap();
        assert_eq!(random.draws() - bare_random.draws(), 6 * 3);
        for (leaf, bud) in skeleton.leaves.iter().zip(&skeleton.buds) {
            assert!(leaf.choice < 2);
            assert!((0..3).contains(&leaf.group));
            assert_eq!(leaf.branch, bud.branch);
            assert_eq!(leaf.color_scale, 255);
            assert_eq!(leaf.corners[0].color, [0.5, 0.6, 0.7]);
            // A bud's leaf sits one segment (length 10 / 4 segments) from the bud.
            let d = sub(leaf.position, bud.position);
            assert!((dot(d, d).sqrt() - 2.5).abs() < 1e-3, "{d:?}");
            let n = leaf.corners[0].normal;
            assert!((dot(n, n) - 1.0).abs() < 1e-4);
        }
    }

    #[test]
    fn crowding_rejects_close_leaves() {
        let mut t = leafy();
        t.leaves.collision = 2;
        t.leaves.spacing_tolerance = 1000.0;
        let (skeleton, _) = grow(&t).unwrap();
        assert_eq!(skeleton.leaves.len(), 1);
    }

    #[test]
    fn leaf_lod_merges_pairs() {
        let mut t = leafy();
        t.lod = Some(Lod {
            leaf_lod_count: Some(3),
            leaf_reduction: Some(0.0),
            leaf_size_increase: Some(0.0),
            ..Lod::default()
        });
        t.leaves.spacing_tolerance = 1000.0;
        let generated = generate(&t).unwrap();
        let lods = &generated.leaf_lods;
        assert_eq!(lods.len(), 3);
        assert_eq!(lods[0].len(), 6);
        // Every level at most halves the one before.
        assert!(lods[1].len() <= 3 && !lods[1].is_empty());
        assert!(lods[2].len() <= lods[1].len().div_ceil(2));
        // A merged leaf sits at its pair's midpoint (origin y 0.5 adds nothing).
        let first = &lods[0];
        let m = lods[1][0];
        assert!(first.iter().any(|a| first.iter().any(|b| {
            let mid = scale(super::super::frame::add(a.position, b.position), 0.5);
            sub(mid, m.position).iter().all(|x| x.abs() < 1e-5)
        })));
    }
}
