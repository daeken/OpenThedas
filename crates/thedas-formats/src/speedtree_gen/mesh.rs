//! Render-ready output of a computed tree (generation.md stage 7, the specified parts).
//!
//! [`TreeMesh::new`] computes a tree once (it is instanced at every placement, 7.1) and
//! flattens it into plain vertex and index lists per LOD level: branch tubes, fronds, leaf
//! cards and the far billboard. Everything is in the tree's own space (Z up, the file's
//! units); a placement applies its RML transform and scale.
//!
//! Texture coordinates are given as `uv` with `v = 1 − t`, which samples what the game's
//! flipped `−t` does (7.1). Fronds, leaves and the billboard index the level's atlases;
//! branches use the tree's own bark texture (`TreeInfo::branch_texture`).
//!
//! Not yet specified, so not here: the wind weights and matrix indices of vertices and leaves
//! (hold the wind still), and how the LOD value becomes per-part alphas (7.4).

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
    /// Leaf-angle index (7.2): the leaf's position in its LOD level's list, mod 8.
    pub angle_index: u32,
}

/// A leaf drawn as a mesh (7.3): the mesh template of its choice, placed at `position` in the
/// leaf's frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MeshLeaf {
    pub position: Vec3,
    pub texture: usize,
    pub choice: usize,
    /// The leaf's corner-0 tangent, binormal and normal: mesh x, y and z.
    pub tangent: Vec3,
    pub binormal: Vec3,
    pub normal: Vec3,
    pub color_scale: f32,
    pub angle_index: u32,
}

impl MeshLeaf {
    /// A template vector (mesh x, y, z) turned into the leaf's frame.
    pub fn turn(&self, v: Vec3) -> Vec3 {
        add(
            add(scale(self.tangent, v[0]), scale(self.binormal, v[1])),
            scale(self.normal, v[2]),
        )
    }
}

/// One choice's copy of a leaf mesh (7.3): positions scaled by the card size, coordinates in
/// the atlas.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LeafMeshTemplate {
    pub vertices: Vec<MeshVertex>,
    pub indices: Vec<u32>,
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
    /// Per leaf LOD level, its card leaves.
    pub leaves: Vec<Vec<LeafCard>>,
    /// Per leaf LOD level, its mesh leaves (7.3).
    pub mesh_leaves: Vec<Vec<MeshLeaf>>,
    /// Per texture choice, its mesh copy when that choice draws meshes.
    pub leaf_mesh_templates: Vec<Option<LeafMeshTemplate>>,
    /// The mesh leaves placed in tree space, one triangle list per leaf LOD level.
    pub leaf_meshes: Part,
    /// The billboard size and image; whether a placement draws it depends on its model name
    /// ([`draws_billboard`]).
    pub billboard: Option<Billboard>,
    /// Bounding box of every branch and frond vertex and every leaf position (5.6 F).
    pub bounds: (Vec3, Vec3),
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
        let templates: Vec<Option<LeafMeshTemplate>> = (0..2 * tree.leaves.textures.len())
            .map(|c| mesh_template(tree, c, atlas))
            .collect();
        let mut leaves = Vec::with_capacity(g.leaf_lods.len());
        let mut mesh_leaves = Vec::with_capacity(g.leaf_lods.len());
        let mut leaf_meshes = Part::default();
        for (k, level) in g.leaf_lods.iter().enumerate() {
            let mut cards = Vec::new();
            let mut meshes = Vec::new();
            let mut list = Vec::new();
            for (i, leaf) in level.iter().enumerate() {
                let angle_index = (i % 8) as u32;
                match &templates[leaf.choice] {
                    Some(template) => {
                        let m = MeshLeaf {
                            position: leaf.position,
                            texture: leaf.texture(),
                            choice: leaf.choice,
                            tangent: leaf.corners[0].tangent,
                            binormal: leaf.corners[0].binormal,
                            normal: leaf.corners[0].normal,
                            color_scale: f32::from(leaf.color_scale) / 255.0,
                            angle_index,
                        };
                        let base = leaf_meshes.vertices.len() as u32;
                        for v in &template.vertices {
                            leaf_meshes.vertices.push(MeshVertex {
                                position: add(m.position, m.turn(v.position)),
                                normal: m.turn(v.normal),
                                tangent: m.turn(v.tangent),
                                binormal: m.turn(v.binormal),
                                ..*v
                            });
                        }
                        list.extend(template.indices.iter().map(|&i| base + i));
                        meshes.push(m);
                    }
                    None => {
                        cards.push(leaf_card(tree, leaf, k, growth, atlas, angle_index));
                    }
                }
            }
            leaves.push(cards);
            mesh_leaves.push(meshes);
            leaf_meshes.lods.push(list);
        }

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
            mesh_leaves,
            leaf_mesh_templates: templates,
            leaf_meshes,
            billboard,
            bounds: (lo, hi),
            rocking_offsets: g.rocking_offsets,
            generated,
        }
    }

    /// A 64-bit FNV-1a hash of every output value (vertices, triangle lists, leaf cards,
    /// billboard), bit for bit, to pin generator output in regression checks. Float functions
    /// (`sin`, `cos`, `acos`, `powf`) come from the platform's math library, so a hash is only
    /// comparable on the same platform.
    pub fn fingerprint(&self) -> u64 {
        let mut h = Fnv::new();
        for part in [&self.branches, &self.fronds, &self.leaf_meshes] {
            h.word(part.vertices.len() as u32);
            for v in &part.vertices {
                for x in v
                    .position
                    .iter()
                    .chain(&v.normal)
                    .chain(&v.tangent)
                    .chain(&v.binormal)
                {
                    h.float(*x);
                }
                for x in v.uv.iter().chain(&v.uv_detail) {
                    h.float(*x);
                }
                h.float(v.fade);
            }
            for list in &part.lods {
                h.word(list.len() as u32);
                list.iter().for_each(|&i| h.word(i));
            }
        }
        for level in &self.leaves {
            h.word(level.len() as u32);
            for c in level {
                c.position.iter().for_each(|&x| h.float(x));
                h.word(c.choice as u32);
                h.word(c.group as u32);
                c.corners.iter().flatten().for_each(|&x| h.float(x));
                c.uv.iter().flatten().for_each(|&x| h.float(x));
                c.normals.iter().flatten().for_each(|&x| h.float(x));
                c.colors.iter().flatten().for_each(|&x| h.float(x));
                h.float(c.color_scale);
                c.angle_offsets.iter().for_each(|&x| h.float(x));
                h.word(c.angle_index);
            }
        }
        for level in &self.mesh_leaves {
            h.word(level.len() as u32);
            for m in level {
                h.word(m.choice as u32);
                h.word(m.angle_index);
                h.float(m.color_scale);
            }
        }
        if let Some(b) = self.billboard {
            h.float(b.width);
            h.float(b.height);
            b.uv.iter().flatten().for_each(|&x| h.float(x));
        }
        self.rocking_offsets.iter().for_each(|&x| h.float(x));
        h.0
    }

    /// The game's near and far distances for one placement (7.4): from half the bounding
    /// box's diagonal times the placement `scale` (RML 3356), and the placement's LOD class
    /// (RML 3379: 0, 1 or 2 multiply by 1, 1.5 or 2; provisional mapping).
    pub fn lod_range(&self, scale: f32, lod_class: i32) -> (f32, f32) {
        let (lo, hi) = self.bounds;
        let d = super::frame::sub(hi, lo);
        let r = 0.5 * super::frame::dot(d, d).sqrt() * scale;
        let t = (r.clamp(2.0, 30.0) - 2.0) / 28.0;
        let m = match lod_class {
            1 => 1.5,
            2 => 2.0,
            _ => 1.0,
        };
        ((15.0 + 5.0 * t) * m, (35.0 + 85.0 * t) * m)
    }
}

/// 64-bit FNV-1a.
struct Fnv(u64);

impl Fnv {
    fn new() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }

    fn word(&mut self, w: u32) {
        for byte in w.to_le_bytes() {
            self.0 ^= u64::from(byte);
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
    }

    fn float(&mut self, x: f32) {
        self.word(x.to_bits());
    }
}

/// The LOD value of a placement (7.4): 1 inside `near`, 0 beyond `far`, from the squared
/// distance between the camera and the placement point and the detail factor
/// `g = (detail scale × zoom)²`.
pub fn lod_value(distance_sq: f32, near: f32, far: f32, g: f32) -> f32 {
    let (n2, f2) = (near * near * g, far * far * g);
    if f2 <= n2 {
        return if distance_sq <= n2 { 1.0 } else { 0.0 };
    }
    1.0 - ((distance_sq - n2) / (f2 - n2)).clamp(0.0, 1.0)
}

/// Whether the game draws a billboard for a model of this name (7.5): never for bushes and
/// grass (`_bsh_`, `_gra_`).
pub fn draws_billboard(model: &str) -> bool {
    let lower = model.to_ascii_lowercase();
    !(lower.contains("_bsh_") || lower.contains("_gra_"))
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
    angle_index: u32,
) -> LeafCard {
    let texture = leaf.texture();
    let [w, h] = card_size(tree, texture);
    let multiplier = if k > 0 { 1.0 + growth * k as f32 } else { 1.0 };
    let (w, h) = (w * multiplier, h * multiplier);
    let origin = tree.leaves.textures[texture].origin;
    // 7.2: even choices mirror the pivot, odd choices the image.
    let mirrored = leaf.choice % 2 == 1;
    let ox = if mirrored { origin[0] } else { 1.0 - origin[0] };
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
        angle_index,
    }
}

/// Image corners in P1…P4 order for a texture without an atlas entry: top right, top left,
/// bottom left, bottom right, as (s, t) with t upward.
const IMAGE_CORNERS: [f32; 8] = [1.0, 1.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0];

/// Choice `choice`'s copy of its texture's leaf mesh (7.3), or `None` when it draws a card.
fn mesh_template(
    tree: &SpeedTree,
    choice: usize,
    atlas: Option<&[[f32; 8]]>,
) -> Option<LeafMeshTemplate> {
    let texture = choice / 2;
    let use_ = tree.leaves.textures[texture]
        .mesh
        .filter(|m| m.use_meshes)?;
    let mesh = usize::try_from(use_.mesh)
        .ok()
        .and_then(|i| tree.meshes.get(i))
        .filter(|m| m.vertices.len() > 2)?;
    let [w, h] = card_size(tree, texture);
    let size = w.max(h);
    let c = atlas.and_then(|a| a.get(texture)).unwrap_or(&IMAGE_CORNERS);
    let even = choice % 2 == 0;
    let vertices = mesh
        .vertices
        .iter()
        .map(|v| {
            let [mu, mv] = v.uv;
            let u = if even {
                super::frame::lerp(c[4], c[0], mu)
            } else {
                super::frame::lerp(c[2], c[6], mu)
            };
            let uv = [u, 1.0 - super::frame::lerp(c[5], c[1], mv)];
            MeshVertex {
                position: scale(v.position, size),
                normal: v.normal,
                tangent: v.tangent,
                binormal: v.binormal,
                uv,
                uv_detail: uv,
                fade: 0.0,
            }
        })
        .collect();
    // Indices outside the mesh are bad input; drop their triangles.
    let count = mesh.vertices.len();
    let indices = mesh
        .indices
        .chunks_exact(3)
        .filter(|t| t.iter().all(|&i| i >= 0 && (i as usize) < count))
        .flatten()
        .map(|&i| i as u32)
        .collect();
    Some(LeafMeshTemplate { vertices, indices })
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
            // Card size: 0.1 · 10 by 0.2 · 10; pivot 0.25 across, mirrored to 0.75 for even
            // choices (7.2), 0.5 up.
            let ox = if card.choice % 2 == 0 { 0.75 } else { 0.25 };
            assert!((card.corners[0][0] - (1.0 - ox)).abs() < 1e-6);
            assert!((card.corners[1][0] + ox).abs() < 1e-6);
            assert!((card.corners[0][1] - 1.0).abs() < 1e-6);
            assert!((card.corners[2][1] + 1.0).abs() < 1e-6);
            let s0 = if card.choice % 2 == 1 { 0.0 } else { 0.5 };
            assert_eq!(card.uv[0], [s0, 0.0]);
            assert!(card.angle_index < 8);
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
    fn the_fingerprint_is_stable_and_sensitive() {
        let t = leafy_trunk();
        let a = TreeMesh::new(&t).fingerprint();
        assert_eq!(a, TreeMesh::new(&t).fingerprint());
        let mut other = t.clone();
        other.tree.seed += 1;
        assert_ne!(a, TreeMesh::new(&other).fingerprint());
    }

    /// Pinned output of representative game files (tree, pine, fern, bushes, grass,
    /// cattail), so changes to the generator are deliberate. Needs the game's `.spt` files
    /// extracted into `$THEDAS_SPT_DIR`; hashes are for macOS on Apple silicon (the platform
    /// math library feeds them). Update a hash only for an intended change, and say why in
    /// the commit.
    #[test]
    #[ignore = "needs game files: set THEDAS_SPT_DIR and run with --ignored"]
    fn game_trees_match_their_pinned_output() {
        let dir = std::env::var("THEDAS_SPT_DIR").expect("THEDAS_SPT_DIR");
        let mut failures = Vec::new();
        for (name, pinned) in PINNED {
            let path = std::path::Path::new(&dir).join(name);
            let data = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            let tree = crate::speedtree::parse(&data).expect(name);
            let hash = TreeMesh::new(&tree).fingerprint();
            if hash != *pinned {
                failures.push(format!("(\"{name}\", 0x{hash:016x}),"));
            }
        }
        assert!(failures.is_empty(), "changed:\n{}", failures.join("\n"));
    }

    const PINNED: &[(&str, u64)] = &[
        ("brc999d_tre_c_oak.spt", 0xd66e2cc22fb3a446),
        ("ost000c_tre_m_pinesm.spt", 0xb6ed20f6f8130431),
        ("brc000d_bsh_f_fern.spt", 0x1f8f493b7a9e2717),
        ("lgt600d_bsh_c_boxwd.spt", 0xd5ec4621e2656640),
        ("lgt602d_gra_c_gtall1.spt", 0xf5293f359a263450),
        ("brc504d_bsh_f_catgras.spt", 0xbc630cd8660a525f),
        ("brc100d_bsh_f_catflwr.spt", 0x5b9f51243c93b528),
    ];

    #[test]
    fn lod_follows_the_games_distances() {
        let mesh = TreeMesh::new(&leafy_trunk());
        // A small tree (half diagonal · scale under 2): near 15, far 35; class 2 doubles them.
        assert_eq!(mesh.lod_range(0.01, 0), (15.0, 35.0));
        assert_eq!(mesh.lod_range(0.01, 2), (30.0, 70.0));
        // A huge one: near 20, far 120.
        assert_eq!(mesh.lod_range(1000.0, 0), (20.0, 120.0));
        assert_eq!(lod_value(0.0, 15.0, 35.0, 1.0), 1.0);
        assert_eq!(lod_value(40.0 * 40.0, 15.0, 35.0, 1.0), 0.0);
        // Not linear in distance: halfway in squared distance is 0.5.
        let mid = (15.0f32 * 15.0 + 35.0 * 35.0) / 2.0;
        assert!((lod_value(mid, 15.0, 35.0, 1.0) - 0.5).abs() < 1e-6);
        assert_eq!(discrete_level(1.0, 3), 0);
        assert_eq!(discrete_level(0.0, 3), 2);
        assert_eq!(discrete_level(0.5, 2), 1);
        assert!(draws_billboard("brc999d_tre_c_oak"));
        assert!(!draws_billboard("ost100d_BSH_m_gorse"));
        assert!(!draws_billboard("ost100d_gra_m_gmed"));
    }

    #[test]
    fn mesh_leaves_replace_cards() {
        use crate::speedtree::{LeafMesh, LeafMeshUse, MeshVertex as FileVertex};
        let mut t = leafy_trunk();
        t.leaves.textures[0].mesh = Some(LeafMeshUse {
            use_meshes: true,
            mesh: 0,
            hang: 0.0,
            rotate: 0.0,
        });
        let v = |position: [f32; 3], uv: [f32; 2]| FileVertex {
            position,
            normal: [0.0, 0.0, 1.0],
            binormal: [0.0, 1.0, 0.0],
            tangent: [1.0, 0.0, 0.0],
            uv,
        };
        t.meshes = vec![LeafMesh {
            name: "tri".into(),
            vertices: vec![
                v([0.0, 0.0, 0.0], [0.0, 0.0]),
                v([1.0, 0.0, 0.0], [1.0, 0.0]),
                v([0.0, 1.0, 0.0], [0.0, 1.0]),
            ],
            indices: vec![0, 1, 2, 0, 2, 7],
        }];
        let mesh = TreeMesh::new(&t);
        assert!(mesh.leaves.iter().all(|l| l.is_empty()));
        let leaves = &mesh.mesh_leaves[0];
        assert_eq!(leaves.len(), mesh.generated.skeleton.leaves.len());
        // Size: max(0.1, 0.2) · 10 = 2. The bad triangle (index 7) is dropped.
        let template = mesh.leaf_mesh_templates[0].as_ref().unwrap();
        assert_eq!(template.vertices[1].position, [2.0, 0.0, 0.0]);
        assert_eq!(template.indices, vec![0, 1, 2]);
        // Even choice: u = lerp(s2, s0, mu); odd: lerp(s1, s3, mu); v = 1 − lerp(t2, t0, mv).
        assert_eq!(template.vertices[1].uv, [0.5, 0.5]);
        assert_eq!(template.vertices[2].uv, [0.0, 0.0]);
        let odd = mesh.leaf_mesh_templates[1].as_ref().unwrap();
        assert_eq!(odd.vertices[1].uv, [0.5, 0.5]);
        assert_eq!(odd.vertices[0].uv, [0.0, 0.5]);
        // Placed: one triangle per leaf, in the leaf's frame.
        assert_eq!(mesh.leaf_meshes.lods[0].len(), 3 * leaves.len());
        let m = leaves[0];
        let placed = mesh.leaf_meshes.vertices[1].position;
        let expect = add(m.position, scale(m.tangent, 2.0));
        assert!(
            placed
                .iter()
                .zip(&expect)
                .all(|(a, b)| (a - b).abs() < 1e-5)
        );
    }
}
