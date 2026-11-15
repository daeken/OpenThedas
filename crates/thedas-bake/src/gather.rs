//! The bake scene: a level's static geometry as world-space triangles, and the regions of the
//! room atlases each lightmapped mesh piece fills.

use std::collections::{BTreeMap, HashMap};

use glam::{Vec2, Vec3};
use thedas_formats::mesh::DeclUsage;
use thedas_formats::model::Mmh;
use thedas_render::AreaScene;
use thedas_vfs::Vfs;

use crate::bvh::{Bvh, Tri};

/// One lightmapped mesh piece placed in the level: where its lightmap sits in an atlas, and its
/// triangles in lightmap space.
#[derive(Clone, Debug)]
pub struct Region {
    pub draw: usize,
    /// The atlas texture, lower case (`hrt201d_room1400.dds`).
    pub atlas: String,
    /// The RML's offset and scale (3324): atlas uv = uv · scale + offset.
    pub offset: Vec2,
    pub scale: Vec2,
    /// The piece's triangles: world positions and lightmap uvs.
    pub tris: Vec<([Vec3; 3], [Vec2; 3])>,
}

/// The level's static geometry and its lightmap regions.
pub struct BakeScene {
    pub bvh: Bvh,
    pub regions: Vec<Region>,
    /// Region indices by atlas.
    pub atlases: BTreeMap<String, Vec<usize>>,
    /// Draws that are no occluders because their mesh says it casts no baked shadow.
    pub non_casters: usize,
}

/// A placed mesh's baked-shadow flags (MMH 6177 cast, 6301 receive), from its model's node for
/// the mesh's chunk.
fn baked_flags(
    vfs: &Vfs,
    cache: &mut HashMap<String, Option<Mmh>>,
    model: &str,
    mesh_key: &str,
) -> (Option<bool>, Option<bool>) {
    let mmh = cache.entry(model.to_ascii_lowercase()).or_insert_with(|| {
        let (d, _) = vfs.read(&format!("{model}.mmh")).ok()?;
        Mmh::parse(&d).ok()
    });
    let Some(mmh) = mmh else {
        return (None, None);
    };
    let chunk = mesh_key.rsplit(':').next().unwrap_or(mesh_key);
    mmh.mesh_pieces()
        .into_iter()
        .find(|p| p.mesh.chunk.eq_ignore_ascii_case(chunk))
        .map(|p| (p.mesh.cast_baked_shadow, p.mesh.receive_baked_shadow))
        .unwrap_or((None, None))
}

impl BakeScene {
    /// Gathers the level's own draws (not props, trees or skinned meshes). Meshes whose model
    /// says they cast no baked shadow (6177 = 0) are seen by camera rays but block no shadow or
    /// occlusion ray (provisional: the editor's job export is not traced yet).
    pub fn new(scene: &AreaScene, vfs: &Vfs) -> BakeScene {
        let mut tris = Vec::new();
        let mut regions = Vec::new();
        let mut cache = HashMap::new();
        let mut non_casters = 0;
        for (i, d) in scene.draws.iter().enumerate().take(scene.static_draws) {
            if d.skin.is_some() || d.lod.is_some() || scene.tree_constants.contains_key(&i) {
                continue;
            }
            let Some(mesh) = scene.meshes.get(d.mesh) else {
                continue;
            };
            // The toolset's script takes texture coordinates 1 when the chunk has them, else 0
            // (terrain) (LightMapper.py, MSHChunk).
            let use_uv1 = mesh.chunk.element(DeclUsage::TexCoord, 1).is_some();
            let world: Vec<Vec3> = mesh
                .vertices
                .iter()
                .map(|v| d.transform.transform_point3(Vec3::from(v.position)))
                .collect();
            let uv: Vec<Vec2> = mesh
                .vertices
                .iter()
                .map(|v| Vec2::from(if use_uv1 { v.uv1 } else { v.uv0 }))
                .collect();
            let (cast, _receive) = baked_flags(vfs, &mut cache, &d.model, &mesh.key);
            let casts = cast != Some(false);
            non_casters += usize::from(!casts);
            let mut piece = Vec::new();
            for f in mesh.indices.chunks_exact(3) {
                let [a, b, c] = [f[0] as usize, f[1] as usize, f[2] as usize];
                if a >= world.len() || b >= world.len() || c >= world.len() {
                    continue;
                }
                tris.push(Tri {
                    a: world[a],
                    b: world[b],
                    c: world[c],
                    draw: i as u32,
                    casts,
                });
                piece.push(([world[a], world[b], world[c]], [uv[a], uv[b], uv[c]]));
            }
            if let Some(lm) = &d.lightmap
                && lm.planes().is_none()
            {
                regions.push(Region {
                    draw: i,
                    atlas: lm.texture.to_ascii_lowercase(),
                    offset: Vec2::from(lm.offset),
                    scale: Vec2::from(lm.scale),
                    tris: piece,
                });
            }
        }
        let mut atlases: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for (i, r) in regions.iter().enumerate() {
            atlases.entry(r.atlas.clone()).or_default().push(i);
        }
        BakeScene {
            bvh: Bvh::new(tris),
            regions,
            atlases,
            non_casters,
        }
    }
}
