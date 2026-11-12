//! Render-ready output of a computed tree (generation.md stage 7, the specified parts).
//!
//! [`TreeMesh::new`] computes a tree once (it is instanced at every placement, 7.1) and
//! flattens it into plain vertex and index lists per LOD level: branch tubes, fronds, leaf
//! cards and the far billboard. Everything is in the tree's own space (Z up, the file's
//! units); a placement applies its RML transform and scale.
//!
//! Texture coordinates are given as `uv` with `v = 1 − t` (format.md: the file's `t` runs
//! upward). Fronds, leaves and the billboard index the level's atlases; branches use the
//! tree's own bark texture (`TreeInfo::branch_texture`), with the same flip assumed
//! (provisional).
//!
//! Not yet specified, so not here: the game's vertex packing for the `spt_*` shaders, wind
//! (hold it still), mesh leaves, and how fades and transitions are drawn (all "(exe)" in
//! 7.2–7.5).

use super::frame::{Vec3, add, scale};
use super::{GeneratedTree, generate, leaves::card_size};
use crate::speedtree::SpeedTree;

/// One vertex of a branch or frond surface.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MeshVertex {
    pub position: Vec3,
    pub normal: Vec3,
    pub tangent: Vec3,
    pub binormal: Vec3,
    /// Diffuse coordinates (branches: the diffuse layer; fronds: the atlas).
    pub uv: [f32; 2],
    /// Branches: the detail layer; fronds: the same as `uv`.
    pub uv_detail: [f32; 2],
    /// LOD fade hint (3.7, 4.6): −1 on geometry kept to the last level, else 1 − (k + 1)/N
    /// for geometry last drawn at level k.
    pub fade: f32,
}

/// A surface with its triangle lists per LOD level (level 0 the fullest).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Part {
    pub vertices: Vec<MeshVertex>,
    /// Per LOD level, a triangle list (three indices per triangle, degenerate triangles
    /// removed, strip winding kept).
    pub lods: Vec<Vec<u32>>,
}

/// One leaf card (7.2). Its corners are offsets in card space, `[right, up]`, to be turned to
/// face the camera: corner = position + right · camera_right + up · camera_up (see
/// [`LeafCard::corner`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LeafCard {
    pub position: Vec3,
    /// Leaf texture index (into `LeafInfo::textures`).
    pub texture: usize,
    /// Texture choice: odd choices are the mirrored image.
    pub choice: usize,
    /// P1…P4 of 7.2 as `[y, z]` (right, up).
    pub corners: [[f32; 2]; 4],
    /// Atlas coordinates per corner, `v = 1 − t`; image coordinates when the texture has no
    /// atlas entry (5.5 F).
    pub uv: [[f32; 2]; 4],
    pub normals: [Vec3; 4],
    pub colors: [[f32; 3]; 4],
    /// Colour scale, 0–1 (the stored `short` / 255).
    pub color_scale: f32,
    /// Rocking group, 0–2.
    pub group: i32,
    /// The card's two angle offsets in degrees (7.2's table).
    pub angle_offsets: [f32; 2],
}

impl LeafCard {
    /// Corner `i` in tree space for a camera whose right and up vectors (in tree space) are
    /// given: a plain camera-facing card, without rocking (provisional, 7.2 "(exe)").
    pub fn corner(&self, i: usize, right: Vec3, up: Vec3) -> Vec3 {
        let [y, z] = self.corners[i];
        add(self.position, add(scale(right, y), scale(up, z)))
    }
}

/// The far billboard (7.4).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Billboard {
    /// Full width and height of the camera-facing quad, standing on the tree's origin.
    pub width: f32,
    pub height: f32,
    /// The billboard image's four atlas corners as stored, `v = 1 − t`, in
    /// `<level>_diffuse_billboards.dds`.
    pub uv: [[f32; 2]; 4],
}

/// A computed tree, flattened for drawing.
#[derive(Clone, Debug)]
pub struct TreeMesh {
    pub branches: Part,
    pub fronds: Part,
    /// Per leaf LOD level, its cards.
    pub leaves: Vec<Vec<LeafCard>>,
    pub billboard: Option<Billboard>,
    /// Bounding box of the level-0 geometry (branch and frond vertices and leaf positions).
    pub bounds: (Vec3, Vec3),
    /// `TreeInfo::near` and `far`, for [`TreeMesh::lod_value`].
    pub near: f32,
    pub far: f32,
    /// The three rocking groups' time offsets (7.1).
    pub rocking_offsets: [f32; 3],
    /// The full computation, for anything not flattened here.
    pub generated: GeneratedTree,
}

/// The 30 leaf-card angle offsets of 7.2, degrees.
const ANGLE_OFFSETS: [f32; 30] = [
    6.0, -10.0, 16.0, -8.0, 2.0, -18.0, -14.0, 0.0, -4.0, 10.0, -6.0, 8.0, 4.0, -2.0, 14.0, -12.0,
    14.0, -16.0, 4.0, -18.0, 13.0, 9.0, 17.0, -9.0, 3.0, -7.0, -15.0, 7.0, -5.0, 15.0,
];

/// A strip read as triangles (each index with the two before it, alternating winding),
/// without degenerate ones.
fn strip_to_list(strip: &[u32], out: &mut Vec<u32>) {
    for k in 2..strip.len() {
        let (a, b, c) = (strip[k - 2], strip[k - 1], strip[k]);
        if a == b || b == c || a == c {
            continue;
        }
        if k % 2 == 0 {
            out.extend([a, b, c]);
        } else {
            out.extend([b, a, c]);
        }
    }
}

fn flip([s, t]: [f32; 2]) -> [f32; 2] {
    [s, 1.0 - t]
}

impl TreeMesh {
    /// Computes the tree and flattens it.
    pub fn new(tree: &SpeedTree) -> Self {
        let generated = generate(tree);
        let g = &generated;

        let branches = Part {
            vertices: g
                .branches
                .vertices
                .iter()
                .map(|v| MeshVertex {
                    position: v.position,
                    normal: v.normal,
                    tangent: v.tangent,
                    binormal: v.binormal,
                    uv: flip(v.texcoords[0]),
                    uv_detail: flip(v.texcoords[1]),
                    fade: v.fade,
                })
                .collect(),
            lods: (0..g.branches.lods.len())
                .map(|k| {
                    let mut list = Vec::new();
                    for strip in g.branches.lod_strips(k) {
                        strip_to_list(strip, &mut list);
                    }
                    list
                })
                .collect(),
        };

        let fronds = Part {
            vertices: g
                .fronds
                .vertices
                .iter()
                .map(|v| MeshVertex {
                    position: v.position,
                    normal: v.normal,
                    tangent: v.tangent,
                    binormal: v.binormal,
                    uv: flip(v.texcoord),
                    uv_detail: flip(v.texcoord),
                    fade: v.fade,
                })
                .collect(),
            lods: g
                .fronds
                .strips
                .iter()
                .map(|strips| {
                    let mut list = Vec::new();
                    for strip in strips {
                        strip_to_list(strip, &mut list);
                    }
                    list
                })
                .collect(),
        };

        let growth = tree.lod.and_then(|l| l.leaf_size_increase).unwrap_or(0.0);
        let atlas = tree.texcoords.as_ref().map(|t| t.leaves.as_slice());
        let leaves = g
            .leaf_lods
            .iter()
            .enumerate()
            .map(|(k, level)| {
                level
                    .iter()
                    .map(|leaf| leaf_card(tree, leaf, k, growth, atlas))
                    .collect()
            })
            .collect();

        let mut lo = [f32::MAX; 3];
        let mut hi = [f32::MIN; 3];
        let points = branches
            .vertices
            .iter()
            .map(|v| v.position)
            .chain(fronds.vertices.iter().map(|v| v.position))
            .chain(g.skeleton.leaves.iter().map(|l| l.position));
        let mut any = false;
        for p in points {
            any = true;
            for i in 0..3 {
                lo[i] = lo[i].min(p[i]);
                hi[i] = hi[i].max(p[i]);
            }
        }
        if !any {
            (lo, hi) = ([0.0; 3], [0.0; 3]);
        }

        let billboard = tree
            .texcoords
            .as_ref()
            .and_then(|t| t.billboards.first())
            .map(|c| {
                let reach = [lo[0], hi[0]]
                    .iter()
                    .flat_map(|&x| [lo[1], hi[1]].map(|y| (x * x + y * y).sqrt()))
                    .fold(0.0f32, f32::max);
                Billboard {
                    width: 2.0 * reach,
                    height: hi[2] - lo[2],
                    uv: [0, 1, 2, 3].map(|i| flip([c[2 * i], c[2 * i + 1]])),
                }
            });

        TreeMesh {
            branches,
            fronds,
            leaves,
            billboard,
            bounds: (lo, hi),
            near: tree.tree.near,
            far: tree.tree.far,
            rocking_offsets: g.rocking_offsets,
            generated,
        }
    }

    /// The LOD value at a camera distance (7.3): 1 near, 0 far.
    pub fn lod_value(&self, distance: f32) -> f32 {
        let span = self.far - self.near;
        if span == 0.0 {
            return if distance <= self.near { 1.0 } else { 0.0 };
        }
        (1.0 - (distance - self.near) / span).clamp(0.0, 1.0)
    }
}

/// The discrete level of `n` levels for a LOD value (7.3).
pub fn discrete_level(lod: f32, n: usize) -> usize {
    let level = ((1.0 - lod) * n as f32) as usize;
    if level >= n {
        n.saturating_sub(1)
    } else {
        level
    }
}

/// A leaf's card at leaf LOD level `k` (7.2).
fn leaf_card(
    tree: &SpeedTree,
    leaf: &super::leaves::Leaf,
    k: usize,
    growth: f32,
    atlas: Option<&[[f32; 8]]>,
) -> LeafCard {
    let texture = leaf.texture();
    let [w, h] = card_size(tree, texture);
    let multiplier = if k > 0 { 1.0 + growth * k as f32 } else { 1.0 };
    let (w, h) = (w * multiplier, h * multiplier);
    let origin = tree.leaves.textures[texture].origin;
    let mirrored = leaf.choice % 2 == 1;
    let ox = if mirrored { 1.0 - origin[0] } else { origin[0] };
    let oy = origin[1];
    let corners = [
        [(1.0 - ox) * w, (1.0 - oy) * h],
        [-ox * w, (1.0 - oy) * h],
        [-ox * w, -oy * h],
        [(1.0 - ox) * w, -oy * h],
    ];
    let mut st = match atlas.and_then(|a| a.get(texture)) {
        Some(c) => [0, 1, 2, 3].map(|i| [c[2 * i], c[2 * i + 1]]),
        // Image coordinates in P1…P4 order: top right, top left, bottom left, bottom right.
        None => [[1.0, 1.0], [0.0, 1.0], [0.0, 0.0], [1.0, 0.0]],
    };
    if mirrored {
        let (s0, s1, s2, s3) = (st[0][0], st[1][0], st[2][0], st[3][0]);
        st[0][0] = s1;
        st[1][0] = s0;
        st[2][0] = s3;
        st[3][0] = s2;
    }
    let t = (leaf.choice * 3) % 30;
    LeafCard {
        position: leaf.position,
        texture,
        choice: leaf.choice,
        corners,
        uv: st.map(flip),
        normals: leaf.corners.map(|c| c.normal),
        colors: leaf.corners.map(|c| c.color),
        color_scale: f32::from(leaf.color_scale) / 255.0,
        group: leaf.group,
        angle_offsets: [ANGLE_OFFSETS[t], ANGLE_OFFSETS[29 - t]],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::speedtree::{LeafTexture, Lod, TexCoordTables};
    use crate::speedtree_gen::skeleton::tests::{level, tree};

    fn leafy_trunk() -> SpeedTree {
        let mut levels = vec![level(-90.0, 2.0), level(0.0, 3.0), level(0.0, 0.0)];
        levels[0].cross_section_segments = 4;
        levels[1].cross_section_segments = 4;
        let mut t = tree(levels);
        t.cluster_first_branch_level = Some(0);
        t.leaves.textures = vec![LeafTexture {
            origin: [0.25, 0.5, 0.0],
            size: [0.1, 0.2, 0.0],
            ..LeafTexture::default()
        }];
        t.leaves.collision = 0;
        t.texcoords = Some(TexCoordTables {
            leaves: vec![[0.5, 1.0, 0.0, 1.0, 0.0, 0.5, 0.5, 0.5]],
            billboards: vec![[1.0, 1.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0]],
            ..TexCoordTables::default()
        });
        t.lod = Some(Lod {
            branch_lod_count: Some(2),
            branch_max_volume_percent: Some(1.0),
            branch_min_volume_percent: Some(0.5),
            ..Lod::default()
        });
        t
    }

    #[test]
    fn branches_flatten_to_triangle_lists() {
        let mesh = TreeMesh::new(&leafy_trunk());
        assert_eq!(mesh.branches.lods.len(), 2);
        let n = mesh.branches.vertices.len() as u32;
        for list in &mesh.branches.lods {
            assert_eq!(list.len() % 3, 0);
            assert!(list.iter().all(|&i| i < n));
            assert!(
                list.chunks(3)
                    .all(|t| t[0] != t[1] && t[1] != t[2] && t[0] != t[2])
            );
        }
        // A trunk tube of 5 rings of 4 points: 4 pairs × 4 quads × 2 triangles, plus one
        // zero-area triangle at each of the 3 joins (it spans the ring's seam point, which
        // repeats the first).
        let trunk = mesh.generated.branches.tubes[0].as_ref().unwrap();
        let mut list = Vec::new();
        strip_to_list(&trunk.strip, &mut list);
        assert_eq!(list.len() / 3, 4 * 4 * 2 + 3);
        assert!(mesh.branches.lods[0].len() > mesh.branches.lods[1].len());
    }

    #[test]
    fn leaf_cards_use_the_pivot_and_atlas() {
        let mesh = TreeMesh::new(&leafy_trunk());
        let cards = &mesh.leaves[0];
        assert!(!cards.is_empty());
        for card in cards {
            // Card size: 0.1 · 10 by 0.2 · 10; pivot 0.25 across (mirrored: 0.75), 0.5 up.
            let ox = if card.choice % 2 == 1 { 0.75 } else { 0.25 };
            assert!((card.corners[0][0] - (1.0 - ox)).abs() < 1e-6);
            assert!((card.corners[1][0] + ox).abs() < 1e-6);
            assert!((card.corners[0][1] - 1.0).abs() < 1e-6);
            assert!((card.corners[2][1] + 1.0).abs() < 1e-6);
            let s0 = if card.choice % 2 == 1 { 0.0 } else { 0.5 };
            assert_eq!(card.uv[0], [s0, 0.0]);
            let t = (card.choice * 3) % 30;
            assert_eq!(
                card.angle_offsets,
                [ANGLE_OFFSETS[t], ANGLE_OFFSETS[29 - t]]
            );
            let c = card.corner(0, [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]);
            assert!((c[2] - (card.position[2] + 1.0)).abs() < 1e-5);
        }
    }

    #[test]
    fn the_billboard_spans_the_bounds() {
        let mesh = TreeMesh::new(&leafy_trunk());
        let b = mesh.billboard.unwrap();
        let (lo, hi) = mesh.bounds;
        assert!((b.height - (hi[2] - lo[2])).abs() < 1e-6);
        assert!(b.width >= hi[0] - lo[0] && b.width > 0.0);
        assert_eq!(b.uv[0], [1.0, 0.0]);
    }

    #[test]
    fn lod_levels_follow_distance() {
        let mesh = TreeMesh::new(&leafy_trunk());
        assert_eq!(mesh.lod_value(0.0), 1.0);
        assert_eq!(mesh.lod_value(1.0e9), 0.0);
        assert_eq!(discrete_level(1.0, 3), 0);
        assert_eq!(discrete_level(0.0, 3), 2);
        assert_eq!(discrete_level(0.5, 2), 1);
    }
}
