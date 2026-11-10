//! CPU-side loading of an area's static geometry: walks the chain
//! area layout -> room layouts -> placed models -> mesh chunks and material objects, and
//! produces a flat list of draws ready for upload. Nothing here touches the GPU, so it can be
//! tested and inspected headless.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use glam::{Mat4, Quat, Vec3};
use thedas_formats::level::Transform;
use thedas_formats::material::{MaterialDef, MaterialObject, Technique};
use thedas_formats::mesh::{DeclUsage, MeshChunk, Msh};
use thedas_formats::{Arl, Mmh, Rml};
use thedas_vfs::{Provenance, Vfs};

#[derive(Debug, thiserror::Error)]
pub enum SceneError {
    #[error("{name}: {source}")]
    Vfs {
        name: String,
        #[source]
        source: thedas_vfs::Error,
    },
    #[error("{name}: {message}")]
    Parse { name: String, message: String },
}

/// The vertex format the stand-in shaders consume. Translated original shaders will read the
/// raw D3D9 streams instead (kept in [`SceneMesh::chunk`] and [`SceneMesh::raw_vertices`]).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv0: [f32; 2],
    pub uv1: [f32; 2],
}

/// One mesh chunk, shared by every placement of its model.
#[derive(Clone, Debug)]
pub struct SceneMesh {
    /// `<msh>:<chunk>`.
    pub key: String,
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    /// The original chunk header and declaration.
    pub chunk: MeshChunk,
    /// The chunk's original vertex bytes, laid out as `chunk.elements` describes.
    pub raw_vertices: Vec<u8>,
}

/// A material object resolved down to its material definition.
#[derive(Clone, Debug)]
pub struct SceneMaterial {
    /// The `.mao` resource.
    pub mao: String,
    pub object: MaterialObject,
    /// The material definition (`static.mat` and so on), if it resolved.
    pub definition: Option<Arc<MaterialDef>>,
    pub semantic: String,
    /// `RenderClass` of the semantic, e.g. `Opaque,Static1`.
    pub render_class: Option<String>,
    /// The technique the original would pick at shader model 3, HIGH detail (provisional).
    pub technique: Option<Technique>,
}

impl SceneMaterial {
    /// The material file in lowercase, e.g. `static.mat`.
    pub fn material_name(&self) -> String {
        self.object.material.to_ascii_lowercase()
    }

    pub fn texture(&self, parameter: &str) -> Option<String> {
        self.object
            .texture(parameter)
            .map(|t| t.resource().to_ascii_lowercase())
    }
}

/// A lightmap atlas region for one draw.
#[derive(Clone, Debug, PartialEq)]
pub struct LightmapRef {
    pub texture: String,
    pub offset: [f32; 2],
    pub scale: [f32; 2],
}

#[derive(Clone, Debug)]
pub struct DrawItem {
    pub mesh: usize,
    pub material: usize,
    /// Model to world.
    pub transform: Mat4,
    pub lightmap: Option<LightmapRef>,
    /// The placed model and room, for diagnostics.
    pub model: String,
    pub room: String,
}

#[derive(Clone, Debug, Default)]
pub struct AreaScene {
    pub level: String,
    pub name: Option<String>,
    pub bounds: Option<(Vec3, Vec3)>,
    pub rooms: Vec<String>,
    pub meshes: Vec<SceneMesh>,
    pub materials: Vec<SceneMaterial>,
    pub draws: Vec<DrawItem>,
    /// Where each resource the scene used came from (resource -> provenance label).
    pub provenance: BTreeMap<String, String>,
    /// Resources that were missing or failed to parse; the scene loads without them.
    pub warnings: Vec<String>,
}

impl AreaScene {
    /// Every texture the scene references (material textures and lightmap atlases).
    pub fn textures(&self) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        for m in &self.materials {
            for t in &m.object.textures {
                if !t.requires_id {
                    out.insert(t.resource().to_ascii_lowercase());
                }
            }
        }
        for d in &self.draws {
            if let Some(l) = &d.lightmap {
                out.insert(l.texture.clone());
            }
        }
        out
    }
}

pub fn transform_matrix(t: &Transform) -> Mat4 {
    let q = Quat::from_array(t.rotation);
    let q = if q.length_squared() > 0.0 {
        q.normalize()
    } else {
        Quat::IDENTITY
    };
    Mat4::from_rotation_translation(q, Vec3::from_array(t.position))
}

struct Loader<'a> {
    vfs: &'a Vfs,
    scene: AreaScene,
    mmh: HashMap<String, Option<Arc<Mmh>>>,
    msh: HashMap<String, Option<Arc<Msh>>>,
    mesh_index: HashMap<String, Option<usize>>,
    mat_defs: HashMap<String, Option<Arc<MaterialDef>>>,
    material_index: HashMap<String, Option<usize>>,
}

impl<'a> Loader<'a> {
    fn read(&mut self, name: &str) -> Result<Vec<u8>, SceneError> {
        let (data, prov): (Vec<u8>, Provenance) =
            self.vfs.read(name).map_err(|e| SceneError::Vfs {
                name: name.to_string(),
                source: e,
            })?;
        self.scene
            .provenance
            .insert(name.to_ascii_lowercase(), prov.layer_label);
        Ok(data)
    }

    fn warn(&mut self, message: String) {
        log::warn!("{message}");
        self.scene.warnings.push(message);
    }

    fn model(&mut self, name: &str) -> Option<Arc<Mmh>> {
        let key = format!("{}.mmh", name.to_ascii_lowercase());
        if let Some(m) = self.mmh.get(&key) {
            return m.clone();
        }
        let loaded = match self.read(&key) {
            Ok(d) => match Mmh::parse(&d) {
                Ok(m) => Some(Arc::new(m)),
                Err(e) => {
                    self.warn(format!("{key}: {e}"));
                    None
                }
            },
            Err(e) => {
                self.warn(e.to_string());
                None
            }
        };
        self.mmh.insert(key, loaded.clone());
        loaded
    }

    fn mesh_file(&mut self, name: &str) -> Option<Arc<Msh>> {
        let key = name.to_ascii_lowercase();
        if let Some(m) = self.msh.get(&key) {
            return m.clone();
        }
        let loaded = match self.read(&key) {
            Ok(d) => match Msh::parse(&d) {
                Ok(m) => Some(Arc::new(m)),
                Err(e) => {
                    self.warn(format!("{key}: {e}"));
                    None
                }
            },
            Err(e) => {
                self.warn(e.to_string());
                None
            }
        };
        self.msh.insert(key, loaded.clone());
        loaded
    }

    fn mesh(&mut self, msh_name: &str, chunk: &str) -> Option<usize> {
        let key = format!(
            "{}:{}",
            msh_name.to_ascii_lowercase(),
            chunk.to_ascii_lowercase()
        );
        if let Some(i) = self.mesh_index.get(&key) {
            return *i;
        }
        let built = self.build_mesh(msh_name, chunk, &key);
        let index = built.map(|m| {
            self.scene.meshes.push(m);
            self.scene.meshes.len() - 1
        });
        self.mesh_index.insert(key, index);
        index
    }

    fn build_mesh(&mut self, msh_name: &str, chunk_name: &str, key: &str) -> Option<SceneMesh> {
        let msh = self.mesh_file(msh_name)?;
        let Some(chunk) = msh.chunk(chunk_name) else {
            self.warn(format!("{msh_name}: no chunk {chunk_name}"));
            return None;
        };
        if chunk.primitive != 0 {
            self.warn(format!(
                "{key}: unsupported primitive type {}",
                chunk.primitive
            ));
            return None;
        }
        let attrs = (|| {
            let pos = msh.attribute(chunk, DeclUsage::Position, 0)?;
            let normal = msh.attribute(chunk, DeclUsage::Normal, 0)?;
            let uv0 = msh.attribute(chunk, DeclUsage::TexCoord, 0)?;
            let uv1 = msh.attribute(chunk, DeclUsage::TexCoord, 1)?;
            let indices = msh.indices(chunk)?;
            let raw = msh.vertex_bytes(chunk)?.to_vec();
            Ok::<_, thedas_formats::TypedError>((pos, normal, uv0, uv1, indices, raw))
        })();
        let (pos, normal, uv0, uv1, indices, raw) = match attrs {
            Ok(a) => a,
            Err(e) => {
                self.warn(format!("{key}: {e}"));
                return None;
            }
        };
        let Some(pos) = pos else {
            self.warn(format!("{key}: no position stream"));
            return None;
        };
        let n = pos.len();
        let get2 = |v: &Option<Vec<[f32; 4]>>, i: usize| {
            v.as_ref().map_or([0.0; 2], |v| [v[i][0], v[i][1]])
        };
        let vertices = (0..n)
            .map(|i| Vertex {
                position: [pos[i][0], pos[i][1], pos[i][2]],
                normal: normal
                    .as_ref()
                    .map_or([0.0, 0.0, 1.0], |v| [v[i][0], v[i][1], v[i][2]]),
                uv0: get2(&uv0, i),
                uv1: get2(&uv1, i),
            })
            .collect();
        // Chunks index relative to their own vertices; Msh::parse already checked the range.
        let base = chunk.base_vertex;
        Some(SceneMesh {
            key: key.to_string(),
            vertices,
            indices: indices.into_iter().map(|i| i + base).collect(),
            chunk: chunk.clone(),
            raw_vertices: raw,
        })
    }

    fn material_def(&mut self, name: &str) -> Option<Arc<MaterialDef>> {
        let key = name.to_ascii_lowercase();
        if let Some(m) = self.mat_defs.get(&key) {
            return m.clone();
        }
        let loaded = match self.read(&key) {
            Ok(d) => match MaterialDef::parse(&d) {
                Ok(m) => Some(Arc::new(m)),
                Err(e) => {
                    self.warn(format!("{key}: {e}"));
                    None
                }
            },
            Err(e) => {
                self.warn(e.to_string());
                None
            }
        };
        self.mat_defs.insert(key, loaded.clone());
        loaded
    }

    fn material(&mut self, mao_stem: &str) -> Option<usize> {
        let key = format!("{}.mao", mao_stem.to_ascii_lowercase());
        if let Some(i) = self.material_index.get(&key) {
            return *i;
        }
        let built = match self.read(&key) {
            Ok(d) => match MaterialObject::parse(&d) {
                Ok(object) => {
                    let semantic = object.semantic().to_string();
                    let definition = self.material_def(&object.material_resource());
                    let sem = definition.as_ref().and_then(|d| d.semantic(&semantic));
                    if definition.is_some() && sem.is_none() {
                        self.warn(format!(
                            "{key}: {} has no semantic {semantic}",
                            object.material
                        ));
                    }
                    let render_class = sem.and_then(|s| s.render_class.clone());
                    let technique = definition
                        .as_ref()
                        .and_then(|d| d.select(&semantic, 3, "HIGH"))
                        .cloned();
                    Some(SceneMaterial {
                        mao: key.clone(),
                        object,
                        definition,
                        semantic,
                        render_class,
                        technique,
                    })
                }
                Err(e) => {
                    self.warn(format!("{key}: {e}"));
                    None
                }
            },
            Err(e) => {
                self.warn(e.to_string());
                None
            }
        };
        let index = built.map(|m| {
            self.scene.materials.push(m);
            self.scene.materials.len() - 1
        });
        self.material_index.insert(key, index);
        index
    }
}

impl AreaScene {
    /// Loads a level from a stack that already has the level mounted
    /// (see [`thedas_vfs::DaoInstall::mount_level`]).
    ///
    /// Transform rule (provisional): `world = room * T(position) * R(rotation) * S(scale) *
    /// node chain`, where the node chain runs from the model root down to the mesh node.
    pub fn load(vfs: &Vfs, level: &str) -> Result<AreaScene, SceneError> {
        let mut l = Loader {
            vfs,
            scene: AreaScene {
                level: level.to_string(),
                ..Default::default()
            },
            mmh: HashMap::new(),
            msh: HashMap::new(),
            mesh_index: HashMap::new(),
            mat_defs: HashMap::new(),
            material_index: HashMap::new(),
        };
        let arl_name = format!("{level}.arl");
        let arl = Arl::parse(&l.read(&arl_name)?).map_err(|e| SceneError::Parse {
            name: arl_name.clone(),
            message: e.to_string(),
        })?;
        l.scene.name = arl.name.clone();
        l.scene.bounds = arl
            .bounds()
            .map(|(lo, hi)| (Vec3::from_array(lo), Vec3::from_array(hi)));
        for room in &arl.rooms {
            l.scene.rooms.push(room.name.clone());
            let rml_name = format!("{}.rml", room.name);
            let rml = match l.read(&rml_name).map(|d| Rml::parse(&d)) {
                Ok(Ok(r)) => r,
                Ok(Err(e)) => {
                    l.warn(format!("{rml_name}: {e}"));
                    continue;
                }
                Err(e) => {
                    l.warn(e.to_string());
                    continue;
                }
            };
            let room_m = transform_matrix(&rml.transform);
            for p in &rml.placements {
                let Some(model) = l.model(&p.model) else {
                    continue;
                };
                let placement = room_m
                    * transform_matrix(&p.transform)
                    * Mat4::from_scale(Vec3::splat(p.scale));
                let pieces = model.mesh_pieces();
                if pieces.is_empty() {
                    continue;
                }
                let Some(msh_name) = model.mesh_file.clone() else {
                    l.warn(format!("{}.mmh: mesh nodes but no .msh", p.model));
                    continue;
                };
                for piece in pieces {
                    let Some(mesh) = l.mesh(&msh_name, &piece.mesh.chunk) else {
                        continue;
                    };
                    let Some(material) = l.material(&piece.mesh.material) else {
                        continue;
                    };
                    let node = piece
                        .chain
                        .iter()
                        .fold(Mat4::IDENTITY, |m, t| m * transform_matrix(t));
                    let lightmap = piece.mesh.lightmap_id.as_ref().and_then(|id| {
                        p.lightmaps
                            .iter()
                            .find(|b| b.chunk.eq_ignore_ascii_case(id))
                            .map(|b| LightmapRef {
                                texture: b.texture.to_ascii_lowercase(),
                                offset: b.offset,
                                scale: b.scale,
                            })
                    });
                    l.scene.draws.push(DrawItem {
                        mesh,
                        material,
                        transform: placement * node,
                        lightmap,
                        model: p.model.clone(),
                        room: room.name.clone(),
                    });
                }
            }
        }
        Ok(l.scene)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transforms_compose_room_then_placement() {
        // Room rotated 90 degrees about Z and moved; a placement at local (1, 0, 0).
        let s = std::f32::consts::FRAC_1_SQRT_2;
        let room = Transform {
            position: [10.0, 0.0, 1.0],
            rotation: [0.0, 0.0, s, s],
        };
        let local = Transform {
            position: [1.0, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
        };
        let world = transform_matrix(&room) * transform_matrix(&local);
        let p = world.transform_point3(Vec3::ZERO);
        assert!((p - Vec3::new(10.0, 1.0, 1.0)).length() < 1e-5, "{p:?}");
        // A zero quaternion is treated as identity rather than producing NaNs.
        let z = transform_matrix(&Transform {
            position: [0.0; 3],
            rotation: [0.0; 4],
        });
        assert_eq!(z, Mat4::IDENTITY);
    }
}
