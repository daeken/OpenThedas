//! CPU-side loading of an area's static geometry: walks the chain
//! area layout -> room layouts -> placed models -> mesh chunks and material objects, and
//! produces a flat list of draws ready for upload. Nothing here touches the GPU, so it can be
//! tested and inspected headless.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use glam::{Mat4, Quat, Vec3};
use thedas_formats::level::{ArlRoom, LightmapBinding, Transform};
use thedas_formats::material::{MaterialDef, MaterialObject, Technique, TextureBinding};
use thedas_formats::mesh::{DeclUsage, MeshChunk, Msh};
pub use thedas_formats::model::{DOMAIN_CHARACTERS, DOMAIN_LEVEL};
use thedas_formats::morph::{
    Morph, MorphNode, MorphPreset, TargetMesh, blend_targets, slot as morph_slot,
};
use thedas_formats::probe::IrradianceProbe;
use thedas_formats::tint::Tint;
use thedas_formats::{Arl, Mmh, Rml};

use crate::skeleton::Skeleton;
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
    /// Positions that replace the raw stream's (a head morph's face), one per vertex.
    pub positions: Option<Vec<[f32; 4]>>,
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
    /// World-space bounding sphere of the placed model (from the room layout, or the mesh
    /// chunk's bounds), used to pick lights.
    pub sphere: (Vec3, f32),
    /// The placed model and room, for diagnostics.
    pub model: String,
    pub room: String,
    /// The irradiance probe lighting it ([`AreaScene::probes`]).
    pub probe: Option<usize>,
    /// For a skinned mesh: how its vertices follow its model's skeleton.
    pub skin: Option<DrawSkin>,
}

/// A skinned draw's bones.
#[derive(Clone, Debug, PartialEq)]
pub struct DrawSkin {
    /// The model's skeleton ([`AreaScene::skeletons`]).
    pub skeleton: usize,
    /// Per palette slot (the values of the mesh's blend indices), the skeleton node.
    pub palette: Vec<Option<usize>>,
    /// The mesh node's rest placement in the model.
    pub mesh_node: Mat4,
}

/// A light placed in the level (from the room's `_ll` light model).
#[derive(Clone, Debug, PartialEq)]
pub enum SceneLight {
    Point {
        position: Vec3,
        color: Vec3,
        radius: f32,
        name: String,
        /// What it lights, as a mask of [`DOMAIN_LEVEL`] and [`DOMAIN_CHARACTERS`]; `None`
        /// when its node does not say (it then lights everything).
        domain: Option<i64>,
    },
    Ambient {
        color: Vec3,
        name: String,
        /// The room whose light model holds it: it lights that room.
        room: String,
    },
}

/// The original's "Graphics Detail" video option, which picks among the techniques a
/// material's logic tests allow (`LogicTest Detail="LOW"` / `"MEDIUM"`): at Low, `Static`
/// draws with `Static1_Fast.psh`, characters vertex-lit, terrain with `Terrain3`, trees with
/// their `_Low` shaders and bloom with the five-tap blur. No material in the data tests for
/// High, so High draws as Medium. Medium by default: the original's own default (chosen by
/// its hardware detection) is not traced.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Detail {
    Low,
    #[default]
    Medium,
    High,
}

impl Detail {
    /// The name the material logic tests use.
    pub fn name(self) -> &'static str {
        match self {
            Detail::Low => "LOW",
            Detail::Medium => "MEDIUM",
            Detail::High => "HIGH",
        }
    }

    /// `low`, `medium` or `high`, any case.
    pub fn parse(s: &str) -> Option<Detail> {
        [Detail::Low, Detail::Medium, Detail::High]
            .into_iter()
            .find(|d| d.name().eq_ignore_ascii_case(s))
    }
}

#[derive(Clone, Debug, Default)]
pub struct AreaScene {
    pub level: String,
    /// The level's sun (direction its light travels, colour), when it has one.
    pub sun: Option<(Vec3, Vec3)>,
    pub name: Option<String>,
    pub bounds: Option<(Vec3, Vec3)>,
    pub rooms: Vec<String>,
    pub meshes: Vec<SceneMesh>,
    pub materials: Vec<SceneMaterial>,
    pub draws: Vec<DrawItem>,
    pub lights: Vec<SceneLight>,
    /// Where each resource the scene used came from (resource -> provenance label).
    pub provenance: BTreeMap<String, String>,
    /// Resources that were missing or failed to parse; the scene loads without them.
    pub warnings: Vec<String>,
    /// The level's irradiance probes.
    pub probes: Vec<SceneProbe>,
    /// The skeletons of the skinned models, one per model.
    pub skeletons: Vec<Arc<Skeleton>>,
    /// Each skeleton's model (without `.mmh`).
    pub skeleton_models: Vec<String>,
    /// How many of `draws` are the level's own; the props' follow.
    pub static_draws: usize,
    /// The level's frame buffer effects (see [`crate::post`]).
    pub effects: Vec<thedas_formats::level::FrameEffect>,
    /// The graphics detail level the materials' techniques were chosen for.
    pub detail: Detail,
    /// The level's atmosphere: its sky's scattering and its fog.
    pub atmosphere: Option<thedas_formats::level::Atmosphere>,
    /// The level's clouds.
    pub clouds: Option<thedas_formats::level::Clouds>,
    /// The layout's sunlight direction as stored (see
    /// [`thedas_formats::level::Arl::sunlight_direction`]): the sky's celestial body sits
    /// against it.
    pub sunlight_direction: Option<Vec3>,
    /// The models the simulation placed, in the order given, with their draws.
    pub props: Vec<PropDraws>,
}

/// A model (or several, drawn together) the simulation places in the level: a creature or a
/// placeable.
#[derive(Clone, Debug, PartialEq)]
pub struct Prop {
    /// For diagnostics and lookups, e.g. the object's id and tag.
    pub label: String,
    /// Model resources without the `.mmh` extension.
    pub models: Vec<String>,
    /// Models hung from a node of the first of `models` that has it: (model, node), e.g. a
    /// sword from the right hand's `Prop01`.
    pub attached: Vec<(String, String)>,
    /// A head morph (without `.mor`): its head, eyes, hair, beard and lashes are drawn with
    /// the prop, shaped and coloured by it.
    pub morph: Option<String>,
    /// Leave out the morph's hair and beard (under a helmet).
    pub hide_hair: bool,
    /// Equipment tints by model (of `models` or `attached`): (model, `.tnt` without
    /// extension). They fill the model's materials' `mml_vTintMaskColours`.
    pub tints: Vec<(String, String)>,
    /// Model to world.
    pub transform: Mat4,
}

impl Prop {
    /// The transform of a model standing at `position` with the game's `facing`: a
    /// rotation of `facing` degrees about Z (models face +Y, so facing 0 looks along +Y).
    pub fn placement(position: [f32; 3], facing: f32) -> Mat4 {
        Mat4::from_translation(Vec3::from_array(position))
            * Mat4::from_rotation_z(facing.to_radians())
    }
}

/// An irradiance probe and the archive it came from.
#[derive(Clone, Debug, PartialEq)]
pub struct SceneProbe {
    /// The `.mtx` resource.
    pub name: String,
    /// The archive's stem: a room (`hrt001d_0`) or the level (`ost000t`).
    pub archive: String,
    pub probe: IrradianceProbe,
}

/// One prop's draws in [`AreaScene::draws`].
#[derive(Clone, Debug, PartialEq)]
pub struct PropDraws {
    pub label: String,
    /// Where the prop was placed when the scene loaded.
    pub transform: Mat4,
    pub draws: std::ops::Range<usize>,
    /// Models hung from a node of the body: their draws follow that node.
    pub attachments: Vec<PropAttachment>,
}

/// An attached model's draws and the body node they hang from.
#[derive(Clone, Debug, PartialEq)]
pub struct PropAttachment {
    pub draws: std::ops::Range<usize>,
    pub skeleton: usize,
    pub node: usize,
}

impl AreaScene {
    /// The skeletons a prop's draws and attachments use.
    pub fn prop_skeletons(&self, prop: usize) -> Vec<usize> {
        let Some(p) = self.props.get(prop) else {
            return Vec::new();
        };
        let mut out: Vec<usize> = p
            .draws
            .clone()
            .filter_map(|d| self.draws.get(d)?.skin.as_ref().map(|s| s.skeleton))
            .chain(p.attachments.iter().map(|a| a.skeleton))
            .collect();
        out.sort_unstable();
        out.dedup();
        out
    }

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

/// Where and how a model is placed, besides its transform.
struct Placing<'a> {
    /// Its uniform scale (for the fallback bounding sphere).
    scale: f32,
    /// World-space bounds the room layout gives, if any.
    sphere: Option<(Vec3, f32)>,
    lightmaps: &'a [LightmapBinding],
    /// The room, or the prop's label.
    room: &'a str,
    probe: Option<usize>,
    /// A head morph shaping and colouring the model's face meshes: (name, morph).
    morph: Option<(&'a str, &'a Morph)>,
    /// An item tint for every material of the model: (name, tint).
    tint: Option<(&'a str, &'a Tint)>,
    /// Only the model's lights (a level placement in a scene of objects alone).
    lights_only: bool,
}

/// The material parameter an item tint fills.
pub const TINT_PARAMETER: &str = "mml_vTintMaskColours";

struct Loader<'a> {
    vfs: &'a Vfs,
    scene: AreaScene,
    mmh: HashMap<String, Option<Arc<Mmh>>>,
    msh: HashMap<String, Option<Arc<Msh>>>,
    mesh_index: HashMap<String, Option<usize>>,
    mat_defs: HashMap<String, Option<Arc<MaterialDef>>>,
    material_index: HashMap<String, Option<usize>>,
    morphs: HashMap<String, Option<Arc<Morph>>>,
    tints: HashMap<String, Option<Arc<Tint>>>,
    skeleton_index: HashMap<String, usize>,
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
            positions: None,
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
        // Terrain chunks name their material with the extension, other meshes without.
        let stem = mao_stem.to_ascii_lowercase();
        let key = format!("{}.mao", stem.strip_suffix(".mao").unwrap_or(&stem));
        if let Some(i) = self.material_index.get(&key) {
            return *i;
        }
        let built = match self.read(&key) {
            Ok(d) => match MaterialObject::parse(&d) {
                Ok(object) => {
                    let mut semantic = object.semantic().to_string();
                    let definition = self.material_def(&object.material_resource());
                    // An object that names no semantic, of a material without `Default`, takes
                    // the material's first (the sky's cloud layer: `Clouds`). Provisional.
                    if object.default_semantic.is_none()
                        && let Some(d) = definition.as_ref()
                        && d.semantic(&semantic).is_none()
                        && let Some(first) = d.semantics.first()
                    {
                        semantic = first.name.clone();
                    }
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
                        .and_then(|d| d.select(&semantic, 3, self.scene.detail.name()))
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

    fn tint(&mut self, name: &str) -> Option<Arc<Tint>> {
        let key = format!("{}.tnt", name.to_ascii_lowercase());
        if let Some(t) = self.tints.get(&key) {
            return t.clone();
        }
        let loaded = match self.read(&key) {
            Ok(d) => match Tint::parse(&d) {
                Ok(t) => Some(Arc::new(t)),
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
        self.tints.insert(key, loaded.clone());
        loaded
    }

    fn morph(&mut self, name: &str) -> Option<Arc<Morph>> {
        let key = format!("{}.mor", name.to_ascii_lowercase());
        if let Some(m) = self.morphs.get(&key) {
            return m.clone();
        }
        // A face-screen preset (`hm_cps_p01.mop`) on its race and gender's base face
        // (`hm_pcc_b01.mor`, the face screen's last preset). Provisional: the base is named
        // after the preset's first two letters.
        let preset = format!("{}.mop", name.to_ascii_lowercase());
        let is_base = name.to_ascii_lowercase().ends_with("_pcc_b01");
        if !is_base && self.read(&key).is_err() && self.read(&preset).is_ok() {
            let loaded = self.preset_morph(name);
            self.morphs.insert(key, loaded.clone());
            return loaded;
        }
        let loaded = match self.read(&key) {
            Ok(d) => match Morph::parse(&d) {
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
        self.morphs.insert(key, loaded.clone());
        loaded
    }

    /// A preset face: the race and gender's base face with the preset's choices on it.
    fn preset_morph(&mut self, name: &str) -> Option<Arc<Morph>> {
        let file = format!("{}.mop", name.to_ascii_lowercase());
        let preset = match self.read(&file).map(|d| MorphPreset::parse(&d)) {
            Ok(Ok(p)) => p,
            Ok(Err(e)) => {
                self.warn(format!("{file}: {e}"));
                return None;
            }
            Err(e) => {
                self.warn(e.to_string());
                return None;
            }
        };
        let prefix: String = name.chars().take(2).collect();
        let base = self.morph(&format!("{prefix}_pcc_b01"))?;
        let mut morph = base.with_preset(&preset);
        // The preset's morph targets reshape the head's meshes (see
        // `thedas_formats::morph::blend_targets`).
        let models: Vec<String> = morph.models.iter().flatten().cloned().collect();
        let targets: Vec<(f32, Vec<FacePiece>)> = preset
            .targets
            .iter()
            .map(|(t, w)| (*w, self.face_pieces(t)))
            .collect();
        for model in models {
            for piece in self.face_pieces(&model) {
                let meshes: Vec<TargetMesh> = targets
                    .iter()
                    .flat_map(|(w, pieces)| {
                        pieces
                            .iter()
                            .filter(|p| p.node.eq_ignore_ascii_case(&piece.node))
                            .map(move |p| TargetMesh {
                                weight: *w,
                                positions: &p.positions,
                                indices: &p.indices,
                                uvs: &p.uvs,
                            })
                    })
                    .collect();
                let Some(vertices) =
                    blend_targets(&piece.positions, &piece.indices, &piece.uvs, &meshes)
                else {
                    continue;
                };
                match morph
                    .nodes
                    .iter_mut()
                    .find(|n| n.name.eq_ignore_ascii_case(&piece.node))
                {
                    Some(n) => n.vertices = vertices,
                    None => morph.nodes.push(MorphNode {
                        name: piece.node.clone(),
                        vertices,
                        ..Default::default()
                    }),
                }
            }
        }
        Some(Arc::new(morph))
    }

    /// A head model's mesh pieces by node (`FaceM1`): positions, triangles and texture
    /// coordinates, as a morph target blend reads them.
    fn face_pieces(&mut self, model: &str) -> Vec<FacePiece> {
        let Some(mmh) = self.model(model) else {
            return Vec::new();
        };
        let Some(msh) = mmh.mesh_file.as_deref().and_then(|f| self.mesh_file(f)) else {
            return Vec::new();
        };
        mmh.mesh_pieces()
            .into_iter()
            .filter_map(|p| {
                let c = msh.chunk(&p.mesh.chunk)?;
                Some(FacePiece {
                    node: p.node_name?.to_string(),
                    positions: msh.attribute(c, DeclUsage::Position, 0).ok()??,
                    indices: msh.indices(c).ok()?,
                    uvs: msh
                        .attribute(c, DeclUsage::TexCoord, 0)
                        .ok()
                        .flatten()
                        .unwrap_or_default(),
                })
            })
            .collect()
    }

    /// A copy of mesh `base` with a head morph's vertex positions, shared by every draw of
    /// that morph. The base mesh when the vertex counts disagree.
    fn morphed_mesh(&mut self, base: usize, morph: &str, node: &MorphNode) -> usize {
        let key = format!("{}#{morph}", self.scene.meshes[base].key);
        if let Some(Some(i)) = self.mesh_index.get(&key) {
            return *i;
        }
        let mut m = self.scene.meshes[base].clone();
        if node.vertices.len() != m.vertices.len() {
            self.warn(format!(
                "{morph}.mor: {} has {} vertices, {} has {}",
                node.name,
                node.vertices.len(),
                m.key,
                m.vertices.len()
            ));
            return base;
        }
        for (v, p) in m.vertices.iter_mut().zip(&node.vertices) {
            v.position = [p[0], p[1], p[2]];
        }
        m.positions = Some(node.vertices.clone());
        m.key = key.clone();
        self.scene.meshes.push(m);
        let i = self.scene.meshes.len() - 1;
        self.mesh_index.insert(key, Some(i));
        i
    }

    /// A copy of material `base` with textures and vector parameters replaced (a head
    /// morph's, an item tint's), shared by every draw with the same `source`.
    fn overridden_material(
        &mut self,
        base: usize,
        source: &str,
        textures: &[(String, String)],
        vectors: &[(String, Vec<[f32; 4]>)],
    ) -> usize {
        let key = format!("{}#{source}", self.scene.materials[base].mao).to_ascii_lowercase();
        if let Some(Some(i)) = self.material_index.get(&key) {
            return *i;
        }
        let mut m = self.scene.materials[base].clone();
        let o = &mut m.object;
        for (param, tex) in textures {
            match o
                .textures
                .iter_mut()
                .find(|t| t.parameter.eq_ignore_ascii_case(param))
            {
                Some(t) => t.res_name = tex.clone(),
                None => o.textures.push(TextureBinding {
                    parameter: param.clone(),
                    res_name: tex.clone(),
                    requires_id: false,
                }),
            }
        }
        for (name, values) in vectors {
            if let [v] = values.as_slice() {
                o.vectors.retain(|(n, _)| !n.eq_ignore_ascii_case(name));
                o.vectors.push((name.clone(), *v));
            } else {
                o.vector_arrays
                    .retain(|(n, _)| !n.eq_ignore_ascii_case(name));
                o.vector_arrays
                    .push((name.clone(), values.iter().flatten().copied().collect()));
            }
        }
        m.mao = key.clone();
        self.scene.materials.push(m);
        let i = self.scene.materials.len() - 1;
        self.material_index.insert(key, Some(i));
        i
    }

    /// Reads every irradiance probe the mounted level carries (`<level>_probe_*.mtx`).
    fn load_probes(&mut self, level: &str) {
        let prefix = format!("{}_probe_", level.to_ascii_lowercase());
        let names: Vec<String> = self
            .vfs
            .names()
            .into_iter()
            .filter(|n| {
                let n = n.to_ascii_lowercase();
                n.starts_with(&prefix) && n.ends_with(".mtx")
            })
            .collect();
        for name in names {
            let archive = self
                .vfs
                .lookup(&name)
                .and_then(|p| {
                    p.file
                        .file_name()
                        .map(|f| f.to_string_lossy().to_ascii_lowercase())
                })
                .map(|f| {
                    f.trim_end_matches(".rim")
                        .trim_end_matches(".erf")
                        .trim_end_matches(".gpu")
                        .to_string()
                })
                .unwrap_or_default();
            let probe = match self.read(&name) {
                Ok(d) => match IrradianceProbe::parse(&d) {
                    Ok(p) => p,
                    Err(e) => {
                        self.warn(format!("{name}: {e}"));
                        continue;
                    }
                },
                Err(e) => {
                    self.warn(e.to_string());
                    continue;
                }
            };
            self.scene.probes.push(SceneProbe {
                name: name.to_ascii_lowercase(),
                archive,
                probe,
            });
        }
    }

    /// The probe of a room: the one in the room's own archive, else the level's, else the
    /// first (provisional: the original's choice of probe is not known).
    fn room_probe(&self, level: &str, room: &str) -> Option<usize> {
        let room = room.to_ascii_lowercase();
        let level = level.to_ascii_lowercase();
        let find = |stem: &str| self.scene.probes.iter().position(|p| p.archive == stem);
        (!room.is_empty())
            .then(|| find(&room).or_else(|| find(&format!("{room}_0"))))
            .flatten()
            .or_else(|| find(&level))
            .or_else(|| (!self.scene.probes.is_empty()).then_some(0))
    }

    /// Adds the draws (and lights) of one placed model: `placement` is model to world.
    fn place(&mut self, model_name: &str, placement: Mat4, at: Placing<'_>) {
        let Placing {
            scale,
            sphere,
            lightmaps,
            room,
            probe,
            morph,
            tint,
            lights_only,
        } = at;
        let Some(model) = self.model(model_name) else {
            return;
        };
        for lp in model.lights() {
            let m = lp
                .chain
                .iter()
                .fold(placement, |m, t| m * transform_matrix(t));
            let name = lp.name.unwrap_or("").to_string();
            self.scene.lights.push(match lp.light {
                thedas_formats::model::Light::Point {
                    color,
                    radius,
                    affect_domain,
                } => SceneLight::Point {
                    position: m.transform_point3(Vec3::ZERO),
                    color: Vec3::new(color[0], color[1], color[2]),
                    radius: *radius,
                    name,
                    domain: *affect_domain,
                },
                thedas_formats::model::Light::Ambient { color } => SceneLight::Ambient {
                    color: Vec3::new(color[0], color[1], color[2]),
                    name,
                    room: room.to_string(),
                },
            });
        }
        if lights_only {
            return;
        }
        let pieces = model.mesh_pieces();
        if pieces.is_empty() {
            return;
        }
        let Some(msh_name) = model.mesh_file.clone() else {
            self.warn(format!("{model_name}.mmh: mesh nodes but no .msh"));
            return;
        };
        for piece in pieces {
            let Some(mut mesh) = self.mesh(&msh_name, &piece.mesh.chunk) else {
                continue;
            };
            let Some(mut material) = self.material(&piece.mesh.material) else {
                continue;
            };
            let shaped = morph.and_then(|(name, m)| {
                piece
                    .node_name
                    .and_then(|n| m.node(n))
                    .map(|node| (name, node))
            });
            if let Some((name, node)) = shaped {
                if !node.vertices.is_empty() {
                    mesh = self.morphed_mesh(mesh, name, node);
                }
                if !node.textures.is_empty() || !node.vectors.is_empty() {
                    material = self.overridden_material(
                        material,
                        &format!("{name}:{}", node.name),
                        &node.textures,
                        &node.vectors,
                    );
                }
            }
            if let Some((name, t)) = tint {
                material = self.overridden_material(
                    material,
                    name,
                    &[],
                    &[(TINT_PARAMETER.to_string(), t.colours.to_vec())],
                );
            }

            let node = piece
                .chain
                .iter()
                .fold(Mat4::IDENTITY, |m, t| m * transform_matrix(t));
            let lightmap = piece.mesh.lightmap_id.as_ref().and_then(|id| {
                lightmaps
                    .iter()
                    .find(|b| b.chunk.eq_ignore_ascii_case(id))
                    .map(|b| LightmapRef {
                        texture: b.texture.to_ascii_lowercase(),
                        offset: b.offset,
                        scale: b.scale,
                    })
            });
            let transform = placement * node;
            let skin = (!piece.mesh.bones.is_empty()).then(|| {
                let skeleton = self.skeleton(model_name, &model);
                let s = &self.scene.skeletons[skeleton];
                DrawSkin {
                    skeleton,
                    palette: piece
                        .mesh
                        .bones
                        .iter()
                        .map(|&b| s.node_of_bone(b))
                        .collect(),
                    mesh_node: node,
                }
            });
            let sphere = sphere.unwrap_or_else(|| {
                let m = &self.scene.meshes[mesh];
                let (lo, hi) = m.vertices.iter().fold(
                    (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)),
                    |(lo, hi), v| {
                        let p = Vec3::from_array(v.position);
                        (lo.min(p), hi.max(p))
                    },
                );
                let c = transform.transform_point3((lo + hi) * 0.5);
                (c, (hi - lo).length() * 0.5 * scale)
            });
            self.scene.draws.push(DrawItem {
                mesh,
                material,
                transform,
                lightmap,
                sphere,
                model: model_name.to_string(),
                room: room.to_string(),
                probe,
                skin,
            });
        }
    }

    /// The skeleton of a model, built once.
    fn skeleton(&mut self, model_name: &str, model: &Mmh) -> usize {
        let key = model_name.to_ascii_lowercase();
        if let Some(i) = self.skeleton_index.get(&key) {
            return *i;
        }
        self.scene
            .skeletons
            .push(Arc::new(Skeleton::from_mmh(model)));
        self.scene.skeleton_models.push(key.clone());
        let i = self.scene.skeletons.len() - 1;
        self.skeleton_index.insert(key, i);
        i
    }
}

/// The smallest room whose bounds hold `p`.
fn room_at(rooms: &[ArlRoom], p: Vec3) -> Option<&ArlRoom> {
    rooms
        .iter()
        .filter(|r| (0..3).all(|i| r.bounds_min[i] <= p[i] && p[i] <= r.bounds_max[i]))
        .min_by(|a, b| {
            let vol = |r: &ArlRoom| {
                (0..3)
                    .map(|i| r.bounds_max[i] - r.bounds_min[i])
                    .product::<f32>()
            };
            vol(a).total_cmp(&vol(b))
        })
}

impl AreaScene {
    /// Loads a level from a stack that already has the level mounted
    /// (see [`thedas_vfs::DaoInstall::mount_level`]).
    ///
    /// Transform rule (provisional): `world = room * T(position) * R(rotation) * S(scale) *
    /// node chain`, where the node chain runs from the model root down to the mesh node.
    pub fn load(vfs: &Vfs, level: &str) -> Result<AreaScene, SceneError> {
        Self::load_with_props(vfs, level, &[])
    }

    /// [`AreaScene::load`], then the models the simulation places (creatures and
    /// placeables), each prop's draws after the level's (see [`AreaScene::props`]).
    pub fn load_with_props(
        vfs: &Vfs,
        level: &str,
        props: &[Prop],
    ) -> Result<AreaScene, SceneError> {
        Self::load_parts(vfs, level, props, true, Detail::default())
    }

    /// The objects alone in a level: `props` lit by the level's lights and probes, without
    /// the level's own meshes, for drawing over a scene of the level drawn once (see
    /// `AreaRenderer::render_over`): objects that come and go then rebuild only themselves.
    pub fn load_objects(vfs: &Vfs, level: &str, props: &[Prop]) -> Result<AreaScene, SceneError> {
        Self::load_parts(vfs, level, props, false, Detail::default())
    }

    /// [`AreaScene::load_with_props`] at a graphics detail level.
    pub fn load_with_props_at(
        vfs: &Vfs,
        level: &str,
        props: &[Prop],
        detail: Detail,
    ) -> Result<AreaScene, SceneError> {
        Self::load_parts(vfs, level, props, true, detail)
    }

    /// [`AreaScene::load_objects`] at a graphics detail level.
    pub fn load_objects_at(
        vfs: &Vfs,
        level: &str,
        props: &[Prop],
        detail: Detail,
    ) -> Result<AreaScene, SceneError> {
        Self::load_parts(vfs, level, props, false, detail)
    }

    fn load_parts(
        vfs: &Vfs,
        level: &str,
        props: &[Prop],
        level_meshes: bool,
        detail: Detail,
    ) -> Result<AreaScene, SceneError> {
        let mut l = Loader {
            vfs,
            scene: AreaScene {
                level: level.to_string(),
                detail,
                ..Default::default()
            },
            mmh: HashMap::new(),
            msh: HashMap::new(),
            mesh_index: HashMap::new(),
            mat_defs: HashMap::new(),
            material_index: HashMap::new(),
            morphs: HashMap::new(),
            tints: HashMap::new(),
            skeleton_index: HashMap::new(),
        };
        let arl_name = format!("{level}.arl");
        let arl = Arl::parse(&l.read(&arl_name)?).map_err(|e| SceneError::Parse {
            name: arl_name.clone(),
            message: e.to_string(),
        })?;
        l.scene.name = arl.name.clone();
        l.scene.effects = arl.effects.clone();
        l.scene.atmosphere = arl.atmosphere;
        l.scene.clouds = arl.clouds;
        l.scene.sunlight_direction = arl.sunlight_direction.map(Vec3::from_array);
        // Characters (the objects' scene) take the sun's character colour.
        l.scene.sun = arl.sun.map(|s| {
            let color = if level_meshes {
                s.color
            } else {
                s.character_color
            };
            (Vec3::from_array(s.direction), Vec3::from_array(color))
        });
        l.scene.bounds = arl
            .bounds()
            .map(|(lo, hi)| (Vec3::from_array(lo), Vec3::from_array(hi)));
        l.load_probes(level);
        for room in &arl.rooms {
            l.scene.rooms.push(room.name.clone());
            let room_probe = l.room_probe(level, &room.name);
            // Exterior (terrain) levels keep each sector's layout per level of detail:
            // `<room>_0.rml` is the most detailed.
            let mut rml_name = format!("{}.rml", room.name);
            if l.vfs.lookup(&rml_name).is_none() {
                let lod0 = format!("{}_0.rml", room.name);
                if l.vfs.lookup(&lod0).is_some() {
                    rml_name = lod0;
                }
            }
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
                let placement = room_m
                    * transform_matrix(&p.transform)
                    * Mat4::from_scale(Vec3::splat(p.scale));
                let sphere = p.sphere.map(|(c, r)| (Vec3::from_array(c), r));
                l.place(
                    &p.model,
                    placement,
                    Placing {
                        scale: p.scale,
                        sphere,
                        lightmaps: &p.lightmaps,
                        room: &room.name,
                        probe: room_probe,
                        morph: None,
                        tint: None,
                        lights_only: !level_meshes,
                    },
                );
            }
        }
        // The sky, around the camera (its vertex shader centres it there): its dome and its
        // cloud layer.
        if level_meshes && let Some(sky) = arl.sky_model.as_deref() {
            let first = l.scene.draws.len();
            l.place(
                &sky.to_ascii_lowercase(),
                Mat4::IDENTITY,
                Placing {
                    scale: 1.0,
                    sphere: None,
                    lightmaps: &[],
                    room: "",
                    probe: None,
                    morph: None,
                    tint: None,
                    lights_only: false,
                },
            );
            let materials = &l.scene.materials;
            let mut i = 0;
            l.scene.draws.retain(|d| {
                let keep = i < first || {
                    let sem = &materials[d.material].semantic;
                    sem.eq_ignore_ascii_case("Sky") || sem.eq_ignore_ascii_case("Clouds")
                };
                i += 1;
                keep
            });
        }
        l.scene.static_draws = l.scene.draws.len();
        for prop in props {
            let first = l.scene.draws.len();
            let at = prop.transform.w_axis.truncate();
            let in_room = room_at(&arl.rooms, at);
            let probe = match in_room {
                Some(room) => l.room_probe(level, &room.name),
                None => l.room_probe(level, ""),
            };
            // Objects are lit as part of the room they stand in.
            let room_name = in_room.map_or(prop.label.as_str(), |r| r.name.as_str());
            let placing = |room| Placing {
                scale: 1.0,
                sphere: None,
                lightmaps: &[],
                room,
                probe,
                morph: None,
                tint: None,
                lights_only: false,
            };
            // Each tinted model's tint, read once per prop.
            let tints: Vec<(String, String, Option<Arc<Tint>>)> = prop
                .tints
                .iter()
                .map(|(m, t)| (m.clone(), t.clone(), l.tint(t)))
                .collect();
            let tint_of = |model: &str| {
                tints.iter().find_map(|(m, name, t)| {
                    m.eq_ignore_ascii_case(model)
                        .then(|| t.as_deref().map(|t| (name.as_str(), t)))
                        .flatten()
                })
            };
            for model in &prop.models {
                let at = Placing {
                    tint: tint_of(model),
                    ..placing(room_name)
                };
                l.place(model, prop.transform, at);
            }
            let mut attachments = Vec::new();
            for (model, node) in &prop.attached {
                let body = prop.models.iter().find_map(|m| {
                    let mmh = l.model(m)?;
                    let chain = mmh.node_chain(node)?;
                    Some((m.clone(), mmh, chain))
                });
                let Some((body, mmh, chain)) = body else {
                    l.warn(format!("{}: no node {node} for {model}", prop.label));
                    continue;
                };
                let at = chain
                    .iter()
                    .fold(prop.transform, |m, t| m * transform_matrix(t));
                let first = l.scene.draws.len();
                let placing_at = Placing {
                    tint: tint_of(model),
                    ..placing(room_name)
                };
                l.place(model, at, placing_at);
                let skeleton = l.skeleton(&body, &mmh);
                if let Some(n) = l.scene.skeletons[skeleton].node(node) {
                    attachments.push(PropAttachment {
                        draws: first..l.scene.draws.len(),
                        skeleton,
                        node: n,
                    });
                }
            }
            if let Some(name) = &prop.morph {
                if let Some(m) = l.morph(name) {
                    for (slot, model) in m.models.iter().enumerate() {
                        let Some(model) = model else { continue };
                        if prop.hide_hair && matches!(slot, morph_slot::HAIR | morph_slot::BEARD) {
                            continue;
                        }
                        let model = model.to_ascii_lowercase();
                        let at = Placing {
                            morph: Some((name.as_str(), &m)),
                            ..placing(room_name)
                        };
                        l.place(&model, prop.transform, at);
                    }
                }
            }
            l.scene.props.push(PropDraws {
                label: prop.label.clone(),
                transform: prop.transform,
                draws: first..l.scene.draws.len(),
                attachments,
            });
        }
        Ok(l.scene)
    }
}

/// One mesh piece of a head model (see `Loader::face_pieces`).
struct FacePiece {
    node: String,
    positions: Vec<[f32; 4]>,
    indices: Vec<u32>,
    uvs: Vec<[f32; 4]>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn head_morphs_reshape_meshes_and_override_materials() {
        let vfs = Vfs::new();
        let mut l = Loader {
            vfs: &vfs,
            scene: AreaScene::default(),
            mmh: HashMap::new(),
            msh: HashMap::new(),
            mesh_index: HashMap::new(),
            mat_defs: HashMap::new(),
            material_index: HashMap::new(),
            morphs: HashMap::new(),
            tints: HashMap::new(),
            skeleton_index: HashMap::new(),
        };
        let vertex = |z: f32| Vertex {
            position: [0.0, 0.0, z],
            normal: [0.0, 0.0, 1.0],
            uv0: [0.0; 2],
            uv1: [0.0; 2],
        };
        l.scene.meshes.push(SceneMesh {
            key: "head.msh:facem1".into(),
            vertices: vec![vertex(1.0), vertex(2.0)],
            indices: vec![0, 1, 0],
            chunk: thedas_formats::mesh::MeshChunk {
                name: "FaceM1".into(),
                vertex_size: 0,
                vertex_count: 2,
                index_count: 3,
                primitive: 0,
                index_format: thedas_formats::mesh::IndexFormat::U16,
                base_vertex: 0,
                vertex_offset: 0,
                start_index: 0,
                elements: vec![],
                bounds: None,
            },
            raw_vertices: vec![],
            positions: None,
        });
        let xml = br#"<MaterialObject Name="face"><Material Name="face.mat"/>
            <Texture Name="mml_tDiffuse" ResName="base_d.dds"/></MaterialObject>"#;
        l.scene.materials.push(SceneMaterial {
            mao: "face.mao".into(),
            object: MaterialObject::parse(xml).unwrap(),
            definition: None,
            semantic: "Default".into(),
            render_class: None,
            technique: None,
        });
        let node = MorphNode {
            name: "FaceM1".into(),
            vertices: vec![[0.0, 0.1, 1.5, 1.0], [0.0, 0.2, 1.7, 1.0]],
            textures: vec![("mml_tDiffuse".into(), "uh_hed_masa_0d".into())],
            vectors: vec![
                ("g_vFacialHairColour".into(), vec![[0.5, 0.25, 0.0, 1.0]]),
                ("mml_vTintMaskColours".into(), vec![[1.0; 4], [0.5; 4]]),
            ],
            ..Default::default()
        };
        let m = l.morphed_mesh(0, "hm_test", &node);
        assert_eq!(m, 1);
        assert_eq!(l.scene.meshes[1].vertices[1].position, [0.0, 0.2, 1.7]);
        assert_eq!(
            l.scene.meshes[1].positions.as_deref(),
            Some(&node.vertices[..])
        );
        assert_eq!(l.morphed_mesh(0, "hm_test", &node), 1, "shared");
        let short = MorphNode {
            vertices: vec![[0.0; 4]],
            ..node.clone()
        };
        assert_eq!(l.morphed_mesh(0, "hm_other", &short), 0, "counts differ");

        let mat = l.overridden_material(0, "hm_test:FaceM1", &node.textures, &node.vectors);
        let o = &l.scene.materials[mat].object;
        assert_eq!(
            o.texture("mml_tDiffuse").unwrap().res_name,
            "uh_hed_masa_0d"
        );
        assert_eq!(
            o.vectors,
            vec![("g_vFacialHairColour".into(), [0.5, 0.25, 0.0, 1.0])]
        );
        assert_eq!(o.vector_arrays[0].1.len(), 8);
        assert_eq!(
            l.scene.materials[0]
                .object
                .texture("mml_tDiffuse")
                .unwrap()
                .res_name,
            "base_d.dds",
            "the original is untouched"
        );
    }

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
