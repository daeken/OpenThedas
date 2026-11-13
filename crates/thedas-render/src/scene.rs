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
    FaceOverlay, Morph, MorphNode, MorphPreset, TargetMesh, blend_targets, slot as morph_slot,
};
use thedas_formats::probe::IrradianceProbe;
use thedas_formats::speedtree_gen::mesh::{
    LeafCard, LeafMeshTemplate, MeshLeaf, MeshVertex, Part, TreeMesh,
};
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
    /// The id of the room it stands in ([`thedas_formats::level::ArlRoom::id`]), for the
    /// lights' room test (see [`crate::d3d::pick_lights`]); `None` outside every room.
    pub room_id: Option<i64>,
    /// The irradiance probe lighting it ([`AreaScene::probes`]).
    pub probe: Option<usize>,
    /// For a skinned mesh: how its vertices follow its model's skeleton.
    pub skin: Option<DrawSkin>,
    /// Whether its model says the mesh casts run-time shadows (6176), when it says (see
    /// [`crate::shadow`]).
    pub cast_shadow: Option<bool>,
    /// For a SpeedTree part: the camera distances it is drawn at (see [`DrawLod`]).
    pub lod: Option<DrawLod>,
}

/// The distances a SpeedTree instance's part is drawn at, by DAOrigins.exe's per-instance LOD
/// (0x52e260, 0x536085): with `d²` the squared distance from the camera to `anchor` (the
/// placement point, not the box centre), the geometry draws while `d² < far²` and the
/// billboard from there out.
///
/// Provisional: the exe fades between them with alpha tests over `near²..far²` (the
/// library's alphas); here they switch at `far`, and the detail option and zoom factor `G`
/// are taken as 1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DrawLod {
    pub anchor: Vec3,
    pub far_sq: f32,
    /// Drawn beyond `far` (the billboard) rather than inside it (the geometry).
    pub beyond: bool,
}

impl DrawLod {
    /// The exe's far distance for an instance whose bounding radius (half the diagonal of its
    /// box, times its placement scale) is `radius`, of LOD distance class `class` (RML 3379):
    /// `t = (clamp(r, 2, 30) − 2) / 28`, `far = (35 + 85·t) · M[class]`, `M = {1, 1.5, 2}`.
    pub fn far(radius: f32, class: i32) -> f32 {
        let t = (radius.clamp(2.0, 30.0) - 2.0) / 28.0;
        let m = [1.0, 1.5, 2.0][class.clamp(0, 2) as usize];
        (35.0 + 85.0 * t) * m
    }

    /// Whether the part draws with the camera at `eye`.
    pub fn shown(&self, eye: Vec3) -> bool {
        (eye.distance_squared(self.anchor) < self.far_sq) != self.beyond
    }
}

/// One placement in the level's far billboard batch (`SPT_Billboards0`): DAOrigins.exe
/// rewrites its four vertices' fade every frame from the instance's LOD (0x532bd8 →
/// 0x52ce50; speedtree-exe-trace §11.3; docs/speedtree 7.5, 7.8).
///
/// Provisional: the detail option and zoom factor `G` are taken as 1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BillboardInstance {
    /// The placement point the LOD is measured from.
    pub anchor: Vec3,
    pub near: f32,
    pub far: f32,
    pub transition: thedas_formats::speedtree_gen::mesh::Transition,
    /// The billboard's width and height before the placement's scale.
    pub size: [f32; 2],
}

impl BillboardInstance {
    /// Its vertices' TEXCOORD0 with the camera at `eye`: (width, height, 0, fade) while the
    /// billboard shows (fade > 0; 1 beyond the far distance), else all zero. The game leaves
    /// a hidden instance out of the frame's index list (0x532a70); a quad of no size draws
    /// nothing either.
    pub fn texcoord0(&self, eye: Vec3) -> [f32; 4] {
        use thedas_formats::speedtree_gen::mesh::{lod_alphas, lod_value};
        let l = lod_value(eye.distance_squared(self.anchor), self.near, self.far, 1.0);
        let fade = lod_alphas(l, [1, 1, 1], self.transition).billboard_alpha;
        if fade > 0.0 {
            [self.size[0], self.size[1], 0.0, fade]
        } else {
            [0.0; 4]
        }
    }
}

/// Which part of a SpeedTree a draw is, for its level of detail ([`TreeLod`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TreePart {
    Branches,
    Fronds,
    /// One of the two leaf slots, which cross-fade between leaf LOD levels.
    Leaves(usize),
}

/// A SpeedTree draw's level of detail, as DAOrigins.exe works it out each frame
/// (0x536085–0x5361d4, speedtree-exe-trace §5, §11; docs/speedtree 7.4, 7.8): the LOD value
/// from the squared distance between the camera and the placement point, then the part's
/// level and alpha-test reference.
///
/// Provisional: the detail option and zoom factor `G` are taken as 1.
#[derive(Clone, Debug, PartialEq)]
pub struct TreeLod {
    pub anchor: Vec3,
    pub near: f32,
    pub far: f32,
    /// The tree's branch, frond and leaf level counts.
    pub counts: [usize; 3],
    pub transition: thedas_formats::speedtree_gen::mesh::Transition,
    pub part: TreePart,
    /// Per level of this part, its (first index, index count) in the draw's mesh.
    pub levels: Vec<(u32, u32)>,
}

impl TreeLod {
    /// The exe's near and far distances for an instance of bounding radius `radius` (half
    /// the diagonal of its box times its placement scale) and LOD class `class` (RML 3379).
    pub fn range(radius: f32, class: i32) -> (f32, f32) {
        let t = (radius.clamp(2.0, 30.0) - 2.0) / 28.0;
        let m = [1.0, 1.5, 2.0][class.clamp(0, 2) as usize];
        ((15.0 + 5.0 * t) * m, (35.0 + 85.0 * t) * m)
    }

    /// What the part draws with the camera at `eye`: its index range and alpha-test
    /// reference (0–255), or `None` when it is not drawn.
    pub fn frame(&self, eye: Vec3) -> Option<(u32, u32, f32)> {
        use thedas_formats::speedtree_gen::mesh::{lod_alphas, lod_value};
        let lod = lod_value(eye.distance_squared(self.anchor), self.near, self.far, 1.0);
        // The geometry list takes instances with |LOD| ≥ 0.001.
        if lod.abs() < 0.001 {
            return None;
        }
        let a = lod_alphas(lod, self.counts, self.transition);
        let (level, alpha) = match self.part {
            TreePart::Branches => (a.branch_level, 0.0),
            // The game draws the frond level before the one the library names, never below
            // the first (0x52cdbc).
            TreePart::Fronds => ((a.frond_level - 1).max(0), a.alpha),
            TreePart::Leaves(slot) => a.leaves.get(slot).copied().unwrap_or((-1, 255.0)),
        };
        if level < 0 || self.levels.is_empty() {
            return None;
        }
        let (first, count) = self.levels[(level as usize).min(self.levels.len() - 1)];
        (count > 0).then_some((first, count, alpha.floor()))
    }
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

/// Lights that replace a scene's for a picture of its own: the inventory's paperdoll is lit
/// by `paperdoll_lightrig` and `paperdoll_lightprobe` (DAOrigins.exe 0x7bbb30-0x7bbbb1).
#[derive(Clone, Debug, Default)]
pub struct LightRig {
    /// Point lights, placed in the world.
    pub lights: Vec<SceneLight>,
    pub probe: Option<IrradianceProbe>,
    pub ambient: Vec3,
}

impl LightRig {
    /// The point lights of light model `rig` placed by `placement` (model to world), and
    /// the irradiance probe light probe model `probe` names (its `nirr` node's `.mtx`).
    /// Missing or unreadable resources leave their part out.
    pub fn load(vfs: &Vfs, rig: &str, probe: &str, placement: Mat4) -> LightRig {
        let model = |name: &str| {
            let (d, _) = vfs.read(&format!("{name}.mmh")).ok()?;
            Mmh::parse(&d).ok()
        };
        let mut out = LightRig::default();
        if let Some(m) = model(rig) {
            for lp in m.lights() {
                let at = lp
                    .chain
                    .iter()
                    .fold(placement, |m, t| m * transform_matrix(t));
                if let thedas_formats::model::Light::Point {
                    color,
                    radius,
                    affect_domain,
                    is_static,
                    version,
                } = lp.light
                {
                    out.lights.push(SceneLight::Point {
                        position: at.transform_point3(Vec3::ZERO),
                        color: Vec3::new(color[0], color[1], color[2]),
                        radius: *radius,
                        name: lp.name.unwrap_or("").to_string(),
                        domain: *affect_domain,
                        is_static: is_static.unwrap_or(false),
                        version: version.unwrap_or(0),
                        room: None,
                    });
                }
            }
        }
        out.probe = model(probe)
            .and_then(|m| m.irradiance_file().map(str::to_string))
            .and_then(|f| vfs.read(&f).ok())
            .and_then(|(d, _)| IrradianceProbe::parse(&d).ok());
        out
    }
}

/// The VFX contact-sheet shaders' frame grid (`VFX0_CS_*.vsh` read it as columns, rows and
/// frame count). Provisional: who fills it in the original (the emitter's flipbook columns
/// and rows, as here, or the material) is not traced.
pub const CONTACT_SHEET_PARAMETER: &str = "mml_vContactSheetParams";

/// What a placement's user parameter (`UPAR`) sets on the emitters naming it (their
/// `UserParamName`): the VFX shaders' colour multiplier, which scales the particles' colour
/// and alpha. Provisional: DAOrigins.exe stores the placement's parameters on its object
/// (0xa29d43-0xa29dee) and the emitter blueprint keeps the name (+0x2f0, 0x593ad7); how the
/// value reaches the draw is not traced. char_stage's floor fog gives `fog` = (0.3, 0.25,
/// 0.25, 0.75).
pub const USER_COLOR_PARAMETER: &str = "mml_vUserColorMultiplier";

/// A particle emitter placed in the level, by a level model or a placed object's model.
#[derive(Clone, Debug)]
pub struct SceneEmitter {
    pub emitter: Arc<thedas_formats::emitter::Emitter>,
    /// Emitter node to world.
    pub transform: Mat4,
    /// The material its particles draw with ([`AreaScene::materials`]); a flipbook's
    /// carries `mml_vContactSheetParams` (columns, rows, frames, 0).
    pub material: usize,
    /// The placed model and node, for diagnostics.
    pub model: String,
    pub node: String,
    /// Whether its model has a `CutAway` mesh: its particles fade with the cutaway
    /// (DAOrigins.exe 0x466f40).
    pub cut_away: bool,
    /// For a prop's visual effect: the prop, the effect ([`PropEffect::key`]) and which of
    /// the effect's emitters it is, the same from one scene to the next, so its particles
    /// can carry over when the scene is rebuilt.
    pub owner: Option<String>,
}

/// A light placed in the level (from the room's `_ll` light model).
#[derive(Clone, Debug, PartialEq)]
pub enum SceneLight {
    Point {
        position: Vec3,
        color: Vec3,
        radius: f32,
        name: String,
        /// What it lights: [`DOMAIN_CHARACTERS`] (characters only) or anything else (the
        /// level and placeables); `None` when its node does not say (the level).
        domain: Option<i64>,
        /// `IsStatic`: a static light joins the three-slot lists (A, B, C), any other the
        /// two-slot ones (D, E). Absent reads as false (the blueprint's default, 0x55f3bc).
        is_static: bool,
        /// `Version`: 0 or 1, the influence formula (see [`crate::d3d::pick_lights`]).
        version: i64,
        /// The room (id) whose layout placed it: it reaches that room and the rooms the
        /// room's [`AreaScene::light_rooms`] lists. `None` for a light a placed object
        /// carries (provisional: it reaches every room).
        room: Option<i64>,
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
/// High, so High's materials draw as Medium's; its sun shadows are sharper and reach further
/// (see [`crate::shadow::partition_size`] and [`crate::shadow::partition_reach`]). Medium by default: the original's own default (chosen by
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
    /// Each room's light reach (RML 3164, by room id): the other rooms its lights and probes
    /// reach (see [`crate::d3d::reaches`]).
    pub light_rooms: HashMap<i64, Vec<i64>>,
    /// The layout's rooms' boxes (id, min, max), to find the room an object stands in.
    pub room_boxes: Vec<(i64, Vec3, Vec3)>,
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
    /// The particle emitters of every placed model (see [`crate::particles`]).
    pub emitters: Vec<SceneEmitter>,
    /// The draws of meshes flagged `CutAway` (ceilings and roofs), which fade out while the
    /// game camera is high (see [`thedas_formats::level::Arl::cutaway_height`]).
    pub cut_away: Vec<usize>,
    /// The layout's cutaway height ([`thedas_formats::level::Arl::cutaway_height`]), when
    /// the scene has a layout.
    pub cutaway_height: Option<f32>,
    /// SpeedTree placements' constants by draw, for draws with the game's `spt_*` shaders.
    pub tree_constants: HashMap<usize, crate::d3d::TreeConstants>,
    /// SpeedTree draws' levels of detail by draw.
    pub tree_lods: HashMap<usize, TreeLod>,
    /// The level's far billboard batch by draw: its placements, four vertices each, in order.
    pub tree_billboards: HashMap<usize, Vec<BillboardInstance>>,
    /// The level's wind for its trees (strength, direction), from its `.wnd` file.
    pub wind: Option<(f32, [f32; 3])>,
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
    /// Visual effects on it: their emitters follow the body (see `Loader::place_effect`).
    pub effects: Vec<PropEffect>,
    /// Model to world.
    pub transform: Mat4,
}

/// A visual effect on a prop.
#[derive(Clone, Debug, PartialEq)]
pub struct PropEffect {
    /// Its model, without `.mmh` (`fxa_fire_crust_c`).
    pub model: String,
    /// Tells this effect from the prop's others, the same from one scene to the next while
    /// the effect lasts (its emitters are named by it: [`SceneEmitter::owner`]).
    pub key: String,
    /// Seconds since it started, when the scene is built.
    pub age: f32,
    /// It stays until it is removed (its duration animation loops after its impact one);
    /// else it plays its impact animation once.
    pub lasting: bool,
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
    /// Where its light probe node stands, when a room's light model places it.
    pub position: Option<Vec3>,
    /// The room (id) whose light model places it (see [`SceneLight::Point::room`]).
    pub room: Option<i64>,
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
    /// The emitters of its visual effects ([`Prop::effects`]), which follow it.
    pub emitters: Vec<PropEmitter>,
}

/// An emitter that follows a prop: its index in [`AreaScene::emitters`], its placement in
/// the prop's model space at rest, and the body node it follows (a skeleton of
/// [`AreaScene::skeletons`] and a node of it), if any; else the prop's root.
#[derive(Clone, Debug, PartialEq)]
pub struct PropEmitter {
    pub emitter: usize,
    pub in_model: Mat4,
    pub follows: Option<(usize, usize)>,
    /// Its birthrate and birthrate range over the effect's life, as its effect's
    /// animations set them (see `Loader::place_effect`); `None` leaves the emitter's own.
    pub rate: Option<EffectTimeline>,
}

/// An effect emitter's birthrate over its effect's life: the impact animation
/// (`<effect>.impa.ani`) once from the start, then, for a lasting effect, the duration
/// animation (`<effect>.dura.ani`) looped. An animation without tracks for the emitter
/// gives it no particles while it plays, as does the time after an instant effect's
/// impact. Provisional: the exe's playing of these animations is not traced, and the
/// cessation animation (`.cess.ani`) is not played.
#[derive(Clone, Debug, PartialEq)]
pub struct EffectTimeline {
    /// The effect's age in seconds when the scene was built ([`PropEffect::age`]).
    pub age: f32,
    pub lasting: bool,
    pub impact: Option<EffectRate>,
    pub duration: Option<EffectRate>,
}

impl EffectTimeline {
    /// (birthrate, range) `since_built` seconds after the scene was built.
    pub fn at(&self, since_built: f32) -> (f32, f32) {
        let t = self.age + since_built.max(0.0);
        let impact = self.impact.as_ref().map_or(0.0, |i| i.length);
        match (&self.impact, &self.duration) {
            (Some(i), _) if t < impact => i.at(t),
            (_, Some(d)) if self.lasting => d.at(t - impact),
            _ => (0.0, 0.0),
        }
    }
}

/// An effect emitter's birthrate over one of its effect's animations: the
/// `<emitter>_birthrate` and `<emitter>_birthraterange` float tracks.
#[derive(Clone, Debug, PartialEq)]
pub struct EffectRate {
    pub length: f32,
    pub birthrate: Option<thedas_formats::anim::ScalarTrack>,
    pub range: Option<thedas_formats::anim::ScalarTrack>,
}

impl EffectRate {
    /// (birthrate, range) at `t` seconds, the animation looping.
    pub fn at(&self, t: f32) -> (f32, f32) {
        let t = if self.length > 0.0 {
            t.rem_euclid(self.length)
        } else {
            0.0
        };
        (
            self.birthrate.as_ref().map_or(0.0, |k| k.at(t)),
            self.range.as_ref().map_or(0.0, |k| k.at(t)),
        )
    }
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
            .chain(p.emitters.iter().filter_map(|e| e.follows.map(|(s, _)| s)))
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
    /// The id of the room the draws stand in.
    room_id: Option<i64>,
    /// The room whose layout places the model (its lights then belong to that room).
    layout_room: Option<i64>,
    probe: Option<usize>,
    /// A head morph shaping and colouring the model's face meshes: (name, morph).
    morph: Option<(&'a str, &'a Morph)>,
    /// An item tint for every material of the model: (name, tint).
    tint: Option<(&'a str, &'a Tint)>,
    /// Only the model's lights (a level placement in a scene of objects alone).
    lights_only: bool,
    /// The placement's user parameters, for its emitters (see [`USER_COLOR_PARAMETER`]).
    user_params: &'a [(String, [f32; 4])],
    /// The wearer's skin tone (its head morph's skin tint, slot 0): the tint mask's skin
    /// entries of every material of the model.
    skin: Option<&'a Tint>,
}

/// Face morph target families' neutral targets: the head's, the eyes' and the lashes'
/// (their targets start `UH_`/`UHM_`, `UEM_` and `ULM_`).
const NEUTRALS: [(&str, &str); 3] = [
    ("UEM_", "UH_UEM_BASa_0"),
    ("ULM_", "UH_ULM_BASa_0"),
    ("", "UH_UHM_BASa_0"),
];

/// Sets each family's neutral target weight to minus the sum of its other targets' (the
/// presets' rule, see [`blend_targets`]), adding the neutral where it is missing.
fn rebalance_neutrals(weights: &mut Vec<(String, f32)>) {
    let family = |t: &str| {
        let up = t.to_ascii_uppercase();
        NEUTRALS
            .iter()
            .position(|(p, _)| up.starts_with(p))
            .unwrap_or(2)
    };
    let is_neutral = |t: &str| NEUTRALS.iter().any(|(_, n)| t.eq_ignore_ascii_case(n));
    let mut sums = [0.0f32; 3];
    for (t, w) in weights.iter() {
        if !is_neutral(t) {
            sums[family(t)] += *w;
        }
    }
    for (i, (_, neutral)) in NEUTRALS.iter().enumerate() {
        match weights
            .iter_mut()
            .find(|(t, _)| t.eq_ignore_ascii_case(neutral))
        {
            Some(e) => e.1 = -sums[i],
            None if sums[i] != 0.0 => weights.push((neutral.to_string(), -sums[i])),
            None => {}
        }
    }
}

/// The material parameter an item tint fills.
pub const TINT_PARAMETER: &str = "mml_vTintMaskColours";

/// The tint mask entries a skin tone fills: its diffuse and specular colours (see
/// `thedas_formats::morph::tint_slot_targets`).
const SKIN_ENTRIES: [usize; 2] = [8, 9];

/// The tint mask entries whose `w` is the skin channel's opacity (diffuse, specular):
/// `Ch1ArmTnt.psh` lerps the diffuse texture towards the skin colour by the tint mask's alpha
/// times entry 0's `w`.
const SKIN_OPACITY_ENTRIES: [usize; 2] = [0, 1];

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

    /// The level's SpeedTrees as their billboards: each placement draws two crossed quads
    /// textured with its tree's billboard image from the level's billboard atlas
    /// (`<level>_diffuse_billboards.dds`; the tree's `.spt` gives the image's corners,
    /// `t` upward), as tall as the tree's size (`.spt` 2000s) times the placement's scale
    /// (RML 3356), as wide as the image's aspect makes it, drawn with `prop.mat`'s `Punch`
    /// semantic. Provisional and an interim: the original builds the trees' branches,
    /// fronds and leaves at run time (SpeedTreeRT) and shows billboards only far away;
    /// trees without a billboard image (grass, bushes) are left out.
    fn place_tree_billboards(
        &mut self,
        level: &str,
        trees: &[String],
        spots: &[(usize, Mat4, f32, [f32; 4])],
    ) {
        let level = level.to_ascii_lowercase();
        let probe = self.room_probe(&level, "");
        // Materials by texture, once: bark per tree, the level's leaf/frond atlas, and the
        // billboard atlas for trees without geometry.
        let mut materials: HashMap<String, Option<usize>> = HashMap::new();
        let mut material = |l: &mut Self, texture: &str, semantic: &str| -> Option<usize> {
            *materials
                .entry(format!("{texture}:{semantic}"))
                .or_insert_with(|| {
                    l.read(texture).ok()?;
                    let key = format!("{texture}:spt_{semantic}.mao");
                    // The game's own leaf-card material, else a `prop.mat` stand-in.
                    let xml = if semantic == "spt_leafcards" {
                        format!(
                            r#"<MaterialObject Name="spt_leafcards"><Material Name="SPT_LeafCards.mat"/><DefaultSemantic Name="default"/><Texture Name="smpDiffuseMap" ResName="{texture}"/></MaterialObject>"#
                        )
                    } else if semantic == "spt_leafmesh" {
                        format!(
                            r#"<MaterialObject Name="spt_leafmesh"><Material Name="SPT_LeafMesh.mat"/><DefaultSemantic Name="default"/><Texture Name="smpDiffuseMap" ResName="{texture}"/></MaterialObject>"#
                        )
                    } else if semantic == "spt_fronds" {
                        format!(
                            r#"<MaterialObject Name="spt_fronds"><Material Name="SPT_Fronds.mat"/><DefaultSemantic Name="default"/><Texture Name="smpDiffuseMap" ResName="{texture}"/></MaterialObject>"#
                        )
                    } else if semantic == "spt_billboards" {
                        // The level's billboard normal atlas beside its diffuse one.
                        let normal = texture.replace("_diffuse_billboards", "_normal_billboards");
                        let normal = if l.read(&normal).is_ok() {
                            format!(r#"<Texture Name="smpNormalMap" ResName="{normal}"/>"#)
                        } else {
                            String::new()
                        };
                        format!(
                            r#"<MaterialObject Name="spt_billboards"><Material Name="SPT_Billboards0.mat"/><DefaultSemantic Name="default"/><Texture Name="smpDiffuseMap" ResName="{texture}"/>{normal}</MaterialObject>"#
                        )
                    } else if let Some(normal) = semantic.strip_prefix("spt_branch:") {
                        // The bark's normal map, when it loads (the shader's default
                        // otherwise).
                        let normal = if l.read(normal).is_ok() {
                            format!(r#"<Texture Name="smpNormalMap" ResName="{normal}"/>"#)
                        } else {
                            String::new()
                        };
                        format!(
                            r#"<MaterialObject Name="spt_branch"><Material Name="SPT_Branch.mat"/><DefaultSemantic Name="default"/><Texture Name="smpDiffuseMap" ResName="{texture}"/>{normal}</MaterialObject>"#
                        )
                    } else {
                        format!(
                            r#"<MaterialObject Name="spt"><Material Name="prop.mat"/><DefaultSemantic Name="{semantic}"/><Texture Name="mml_tDiffuse" ResName="{texture}"/></MaterialObject>"#
                        )
                    };
                    let m = l.build_material(&key, xml.as_bytes())?;
                    l.scene.materials.push(m);
                    Some(l.scene.materials.len() - 1)
                })
        };
        let atlas = format!("{level}_diffuse.dds");
        let billboards = format!("{level}_diffuse_billboards.dds");
        // Per tree: its meshes with their materials (and whether each is the billboard,
        // drawn beyond the far distance), and its bounds, once.
        type Built = (Vec<(usize, usize, bool)>, Vec3, f32);
        let mut built_trees: HashMap<usize, Option<Built>> = HashMap::new();
        // By ground cell, tree, part mesh, material and whether it is the billboard: the
        // placements' transforms, points and far distances.
        type Batch = Vec<(Mat4, Vec3, f32)>;
        type BatchKey = ((i64, i64), usize, usize, usize, bool);
        let mut batches: BTreeMap<BatchKey, Batch> = BTreeMap::new();
        // Per tree: its leaf cards for the game's `SPT_LeafCards` shader (one draw per
        // placement, placed by constants), and its leaf rock and rustle scalars.
        type SptPart = (usize, usize, TreePart, Vec<(u32, u32)>);
        struct SptTree {
            /// Mesh, material, part and the part's level ranges.
            parts: Vec<SptPart>,
            leaf_angle_scalars: [f32; 2],
            counts: [usize; 3],
            transition: thedas_formats::speedtree_gen::mesh::Transition,
        }
        let mut spt_parts: HashMap<usize, SptTree> = HashMap::new();
        let leaf_material = material(self, &atlas, "spt_leafcards");
        let frond_material = material(self, &atlas, "spt_fronds");
        let leaf_mesh_material = material(self, &atlas, "spt_leafmesh");
        // The far billboards: one batch for the level with the game's `SPT_Billboards0`,
        // and per tree its image's entry in the batch's image table (at most 120 entries,
        // 0x530b70) and what its vertices carry.
        let billboard_material = material(self, &billboards, "spt_billboards");
        struct BillboardTree {
            size: [f32; 2],
            image: usize,
            transition: thedas_formats::speedtree_gen::mesh::Transition,
            /// TEXCOORD1.x: the file's billboard transition (0 when absent).
            threshold: f32,
            /// TEXCOORD2.xyz: the file's light scalars (7.5).
            light: [f32; 3],
        }
        let mut billboard_trees: HashMap<usize, BillboardTree> = HashMap::new();
        let mut billboard_images: Vec<[f32; 4]> = Vec::new();
        // Per placement: its tree, placement point, scale, yaw, colour and LOD radius.
        let mut billboard_spots: Vec<(usize, Vec3, f32, f32, [f32; 4], f32)> = Vec::new();
        let mut placed_trees = 0usize;
        for &(tree, placement, scale, color) in spots {
            let built = built_trees.entry(tree).or_insert_with(|| {
                let name = format!("{}.spt", trees.get(tree)?.to_ascii_lowercase());
                let tree_file = thedas_formats::speedtree::parse(&self.read(&name).ok()?).ok()?;
                let mesh = match TreeMesh::try_new(&tree_file) {
                    Ok(m) => m,
                    Err(e) => {
                        self.warn(format!("{name}: {e}"));
                        return None;
                    }
                };
                let mut parts = Vec::new();
                // The parts drawn with the game's own `spt_*` programs, one draw each per
                // placement; a part whose material does not load falls back to a batched
                // `prop.mat` stand-in.
                let mut spt = Vec::new();
                let bark = bark_texture(&tree_file.tree.branch_texture);
                if let Some(lod) = mesh.branches.lods.first()
                    && !lod.is_empty()
                {
                    let normal = tree_file
                        .maps
                        .as_ref()
                        .and_then(|m| m.branch.as_ref())
                        .map(|b| bark_texture(&b.normal))
                        .filter(|n| n != ".dds")
                        .unwrap_or_else(|| bark.replace(".dds", "_n.dds"));
                    if let Some(m) = material(self, &bark, &format!("spt_branch:{normal}")) {
                        let (sm, levels) = self.spt_surface(&name, "branches", &mesh.branches);
                        spt.push((sm, m, TreePart::Branches, levels));
                    } else if let Some(m) = material(self, &bark, "Default") {
                        let indices = facing_front(&mesh.branches.vertices, lod);
                        parts.push((
                            self.tree_mesh(&name, "branches", &mesh.branches.vertices, indices),
                            m,
                            false,
                        ));
                    }
                }
                if let Some(lod) = mesh.fronds.lods.first()
                    && !lod.is_empty()
                    && let Some(m) = frond_material
                {
                    let _ = lod;
                    let (sm, levels) = self.spt_surface(&name, "fronds", &mesh.fronds);
                    spt.push((sm, m, TreePart::Fronds, levels));
                }
                // Leaves draw in two slots, which cross-fade between leaf LOD levels.
                if mesh.leaves.first().is_some_and(|c| !c.is_empty())
                    && let Some(m) = leaf_material
                {
                    let (sm, levels) = self.spt_leaf_cards(&name, &mesh.leaves);
                    for slot in 0..2 {
                        spt.push((sm, m, TreePart::Leaves(slot), levels.clone()));
                    }
                }
                if mesh.mesh_leaves.first().is_some_and(|l| !l.is_empty())
                    && let Some(m) = leaf_mesh_material
                {
                    let (sm, levels) =
                        self.spt_leaf_meshes(&name, &mesh.mesh_leaves, &mesh.leaf_mesh_templates);
                    for slot in 0..2 {
                        spt.push((sm, m, TreePart::Leaves(slot), levels.clone()));
                    }
                }
                if !spt.is_empty() {
                    spt_parts.insert(
                        tree,
                        SptTree {
                            parts: spt,
                            leaf_angle_scalars: mesh.leaf_angle_scalars,
                            counts: [
                                mesh.branches.lods.len(),
                                mesh.fronds.lods.len(),
                                mesh.leaves.len().max(mesh.mesh_leaves.len()),
                            ],
                            transition: mesh.transition,
                        },
                    );
                }
                if let Some(m) = material(self, &atlas, "Punch") {
                    if frond_material.is_none()
                        && let Some(lod) = mesh.fronds.lods.first()
                        && !lod.is_empty()
                    {
                        let indices = both_sides(lod);
                        parts.push((
                            self.tree_mesh(&name, "fronds", &mesh.fronds.vertices, indices),
                            m,
                            false,
                        ));
                    }
                    if leaf_material.is_none()
                        && let Some(cards) = mesh.leaves.first()
                        && !cards.is_empty()
                    {
                        let (vertices, indices) = leaf_cards(cards);
                        parts.push((
                            self.tree_mesh(&name, "leaves", &vertices, indices),
                            m,
                            false,
                        ));
                    }
                    // Mesh leaves (7.3), already placed in tree space; drawn from both sides
                    // like the cards.
                    if leaf_mesh_material.is_none()
                        && let Some(lod) = mesh.leaf_meshes.lods.first()
                        && !lod.is_empty()
                    {
                        let indices = both_sides(lod);
                        let vertices = &mesh.leaf_meshes.vertices;
                        parts.push((
                            self.tree_mesh(&name, "leaf_meshes", vertices, indices),
                            m,
                            false,
                        ));
                    }
                }
                // Bushes and grass get no billboard: the exe leaves models named `_bsh_` or
                // `_gra_` out of every billboard batch (0x533f18).
                let bush = ["bsh_", "gra_"]
                    .iter()
                    .any(|k| name.starts_with(k) || name.contains(&format!("_{k}")));
                if !bush
                    && let Some(b) = mesh.billboard
                    && b.height > 0.0
                    && billboard_material.is_some()
                    && billboard_images.len() < 120
                {
                    // The image's corners 2 and 0 (`v` down, as the leaf cards').
                    billboard_images.push([b.uv[2][0], b.uv[2][1], b.uv[0][0], b.uv[0][1]]);
                    let s = tree_file.shader_scalars.unwrap_or_default();
                    let global = s.global_light.unwrap_or(1.0);
                    billboard_trees.insert(
                        tree,
                        BillboardTree {
                            size: [b.width, b.height],
                            image: billboard_images.len() - 1,
                            transition: mesh.transition,
                            threshold: tree_file
                                .lod
                                .and_then(|l| l.billboard_transition)
                                .unwrap_or(0.0),
                            light: [
                                global * s.billboard_bright_side.unwrap_or(1.0),
                                s.billboard_dark_side.unwrap_or(1.0) * global,
                                s.billboard_ambient.unwrap_or(0.5),
                            ],
                        },
                    );
                } else if !bush
                    && let Some(b) = mesh.billboard
                    && b.height > 0.0
                    && let Some(m) = material(self, &billboards, "Punch")
                {
                    let us = b.uv.map(|c| c[0]);
                    let vs = b.uv.map(|c| c[1]);
                    let fold = |v: [f32; 4], f: fn(f32, f32) -> f32, i| v.into_iter().fold(i, f);
                    let uv = [
                        fold(us, f32::min, f32::MAX),
                        fold(us, f32::max, f32::MIN),
                        fold(vs, f32::min, f32::MAX),
                        fold(vs, f32::max, f32::MIN),
                    ];
                    let billboard = self.billboard_mesh(&name, 0.5 * b.width, b.height, uv);
                    parts.push((billboard, m, true));
                }
                if parts.is_empty()
                    && !spt_parts.contains_key(&tree)
                    && !billboard_trees.contains_key(&tree)
                {
                    return None;
                }
                let (lo, hi) = (Vec3::from(mesh.bounds.0), Vec3::from(mesh.bounds.1));
                Some((parts, (lo + hi) * 0.5, (hi - lo).length() * 0.5))
            });
            let Some((parts, _, radius)) = built.clone() else {
                continue;
            };
            let transform = placement * Mat4::from_scale(Vec3::splat(scale));
            // Provisional: the LOD class (RML 3379) is taken as 0, as in every ost100d
            // placement.
            let far = DrawLod::far(radius * scale, 0);
            let anchor = placement.transform_point3(Vec3::ZERO);
            let cell = (
                (anchor.x / TREE_CELL).floor() as i64,
                (anchor.y / TREE_CELL).floor() as i64,
            );
            let yaw = {
                let x = placement.x_axis;
                x.y.atan2(x.x)
            };
            if billboard_trees.contains_key(&tree) {
                billboard_spots.push((tree, anchor, scale, yaw, color, radius * scale));
            }
            if let Some(spt) = spt_parts.get(&tree) {
                let constants = crate::d3d::TreeConstants {
                    position: [anchor.x, anchor.y, anchor.z, yaw],
                    scale,
                    color,
                    leaf_angle_scalars: spt.leaf_angle_scalars,
                    // The game draws `rand() % 100` per placement (0x5cbfb7) and hands out
                    // its five winds in creation order (0x531de6); ours follow the
                    // placement's order, so they repeat from load to load.
                    wind_offset: (placed_trees.wrapping_mul(37) % 100) as f32,
                    wind: placed_trees % 5,
                    billboard_images: std::sync::Arc::from([]),
                };
                placed_trees += 1;
                let (near, far) = TreeLod::range(radius * scale, 0);
                for (mesh, material, part, levels) in &spt.parts {
                    let draw = self.scene.draws.len();
                    self.scene.tree_constants.insert(draw, constants.clone());
                    self.scene.tree_lods.insert(
                        draw,
                        TreeLod {
                            anchor,
                            near,
                            far,
                            counts: spt.counts,
                            transition: spt.transition,
                            part: *part,
                            levels: levels.clone(),
                        },
                    );
                    self.scene.draws.push(DrawItem {
                        mesh: *mesh,
                        material: *material,
                        transform,
                        lightmap: None,
                        sphere: (transform.transform_point3(Vec3::ZERO), radius * scale),
                        model: name_of_tree(trees, tree),
                        room: String::new(),
                        room_id: None,
                        probe,
                        skin: None,
                        cast_shadow: Some(false),
                        lod: None,
                    });
                }
            }
            for (mesh, material, beyond) in parts {
                batches
                    .entry((cell, tree, mesh, material, beyond))
                    .or_default()
                    .push((transform, anchor, far));
            }
        }
        if let Some(material) = billboard_material
            && !billboard_spots.is_empty()
        {
            self.spt_billboards(
                material,
                probe,
                billboard_spots
                    .iter()
                    .filter_map(|&(tree, anchor, scale, yaw, color, r)| {
                        let t = billboard_trees.get(&tree)?;
                        let (near, far) = TreeLod::range(r, 0);
                        let instance = BillboardInstance {
                            anchor,
                            near,
                            far,
                            transition: t.transition,
                            size: t.size,
                        };
                        // TEXCOORD1 and TEXCOORD2 (speedtree-exe-trace §7): the yaw as the
                        // game keeps it, in [0, 2π).
                        let yaw = yaw.rem_euclid(std::f32::consts::TAU);
                        let tc1 = [t.threshold, t.image as f32, 1.0, yaw];
                        let tc2 = [t.light[0], t.light[1], t.light[2], scale];
                        Some((instance, tc1, tc2, color, scale))
                    }),
                billboard_images.into(),
            );
        }
        // Each tree type's part, per cell of the ground, is one draw: its placements' copies
        // merged in world space. Interim batching (the game instances each placement with
        // shader constants); the LOD switch is per cell, at the mean far distance from the
        // placements' mean point, so a tree switches up to about a cell's width early or
        // late (provisional).
        for ((cell, tree, mesh, material, beyond), placed) in batches {
            let Some(source) = self.scene.meshes.get(mesh) else {
                continue;
            };
            let mut vertices = Vec::with_capacity(source.vertices.len() * placed.len());
            let mut indices = Vec::with_capacity(source.indices.len() * placed.len());
            for (transform, _, _) in &placed {
                let base = vertices.len() as u32;
                let turn = glam::Mat3::from_mat4(*transform);
                vertices.extend(source.vertices.iter().map(|v| {
                    Vertex {
                        position: transform
                            .transform_point3(Vec3::from(v.position))
                            .to_array(),
                        normal: (turn * Vec3::from(v.normal)).normalize_or_zero().to_array(),
                        uv0: v.uv0,
                        uv1: v.uv1,
                    }
                }));
                indices.extend(source.indices.iter().map(|i| base + i));
            }
            let (lo, hi) = vertices.iter().fold(
                (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)),
                |(lo, hi), v| {
                    let p = Vec3::from(v.position);
                    (lo.min(p), hi.max(p))
                },
            );
            if vertices.is_empty() {
                continue;
            }
            let n = placed.len() as f32;
            let anchor = placed.iter().map(|p| p.1).sum::<Vec3>() / n;
            let far = placed.iter().map(|p| p.2).sum::<f32>() / n;
            let key = format!(
                "{}:cell{}_{}{}",
                self.scene.meshes[mesh].key,
                cell.0,
                cell.1,
                if beyond { ":far" } else { "" }
            );
            let merged = self.push_mesh(key, vertices, indices);
            self.scene.draws.push(DrawItem {
                mesh: merged,
                material,
                transform: Mat4::IDENTITY,
                lightmap: None,
                sphere: ((lo + hi) * 0.5, (hi - lo).length() * 0.5),
                model: name_of_tree(trees, tree),
                room: String::new(),
                room_id: None,
                probe,
                skin: None,
                cast_shadow: Some(false),
                lod: Some(DrawLod {
                    anchor,
                    far_sq: far * far,
                    beyond,
                }),
            });
        }
    }

    /// The level's far billboards as one draw with the game's `SPT_Billboards0` (two passes),
    /// as DAOrigins.exe batches them per forest (0x534230, 0x532c70; speedtree-exe-trace §7,
    /// §11.3; docs/speedtree 7.5): four vertices per placement, each FLOAT4 POSITION0
    /// (placement point, corner 0–3), TEXCOORD0 (width, height, 0, fade; rewritten every
    /// frame, see [`BillboardInstance::texcoord0`]), TEXCOORD1 (billboard transition, image
    /// entry, image count 1, yaw), TEXCOORD2 (bright and dark side light scalars, billboard
    /// ambient scalar, placement scale), TEXCOORD3 the placement colour (RML 3375). The
    /// shaders turn the quad to the camera and fade it by alpha test.
    fn spt_billboards(
        &mut self,
        material: usize,
        probe: Option<usize>,
        spots: impl Iterator<Item = (BillboardInstance, [f32; 4], [f32; 4], [f32; 4], f32)>,
        images: std::sync::Arc<[[f32; 4]]>,
    ) {
        use thedas_formats::mesh::{DeclType, VertexElement};
        const STRIDE: u32 = 80;
        let mut raw: Vec<u8> = Vec::new();
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        let mut instances = Vec::new();
        let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for (instance, tc1, tc2, color, scale) in spots {
            let base = vertices.len() as u32;
            let a = instance.anchor;
            let [w, h] = instance.size;
            for corner in 0..4 {
                let attrs: [[f32; 4]; 5] = [
                    [a.x, a.y, a.z, corner as f32],
                    [w, h, 0.0, 1.0],
                    tc1,
                    tc2,
                    color,
                ];
                raw.extend_from_slice(bytemuck::cast_slice(&attrs));
                vertices.push(Vertex {
                    position: a.to_array(),
                    normal: [0.0, 0.0, 1.0],
                    uv0: [0.0, 0.0],
                    uv1: [0.0, 0.0],
                });
            }
            indices.extend([0, 1, 2, 0, 2, 3].map(|i| base + i));
            let r = 0.5 * w * scale;
            lo = lo.min(a - Vec3::new(r, r, 0.0));
            hi = hi.max(a + Vec3::new(r, r, h * scale));
            instances.push(instance);
        }
        if instances.is_empty() {
            return;
        }
        let element = |offset: u32, usage: DeclUsage, usage_index: u32| VertexElement {
            stream: 0,
            offset,
            ty: DeclType::Float4,
            usage,
            usage_index,
        };
        let key = "spt_billboards".to_string();
        let count = vertices.len() as u32;
        self.scene.meshes.push(SceneMesh {
            key: key.clone(),
            chunk: MeshChunk {
                name: key,
                vertex_size: STRIDE,
                vertex_count: count,
                index_count: indices.len() as u32,
                primitive: 0,
                index_format: thedas_formats::mesh::IndexFormat::U32,
                base_vertex: 0,
                vertex_offset: 0,
                start_index: 0,
                elements: vec![
                    element(0, DeclUsage::Position, 0),
                    element(16, DeclUsage::TexCoord, 0),
                    element(32, DeclUsage::TexCoord, 1),
                    element(48, DeclUsage::TexCoord, 2),
                    element(64, DeclUsage::TexCoord, 3),
                ],
                bounds: None,
            },
            vertices,
            indices,
            raw_vertices: raw,
            positions: None,
        });
        let mesh = self.scene.meshes.len() - 1;
        let draw = self.scene.draws.len();
        self.scene.tree_constants.insert(
            draw,
            crate::d3d::TreeConstants {
                position: [0.0; 4],
                scale: 1.0,
                color: [1.0; 4],
                leaf_angle_scalars: [0.0; 2],
                wind_offset: 0.0,
                wind: 0,
                billboard_images: images,
            },
        );
        self.scene.tree_billboards.insert(draw, instances);
        self.scene.draws.push(DrawItem {
            mesh,
            material,
            transform: Mat4::IDENTITY,
            lightmap: None,
            sphere: ((lo + hi) * 0.5, (hi - lo).length() * 0.5),
            model: "spt_billboards".into(),
            room: String::new(),
            room_id: None,
            probe,
            skin: None,
            // Provisional (RND-080): no shadow, as before.
            cast_shadow: Some(false),
            lod: None,
        });
    }

    /// A tree's leaf cards as the game packs them for `SPT_LeafCards` (speedtree-exe-trace
    /// §3; docs/speedtree 7.2), six vertices (two triangles) per card in tree space:
    /// POSITION the leaf, NORMAL the corner's normal, TEXCOORD0 (u, v, corner, 1),
    /// TEXCOORD1 (w, h, pivot x − ½, pivot y − ½), TEXCOORD2 (angle offsets in radians,
    /// leaf-angle index, colour scale), TEXCOORD3 the wind (index, weight, index, weight).
    /// The game stores halves; floats here.
    /// Every leaf LOD level goes into the one mesh, each with its (first index, count).
    fn spt_leaf_cards(&mut self, name: &str, levels: &[Vec<LeafCard>]) -> (usize, Vec<(u32, u32)>) {
        use thedas_formats::mesh::{DeclType, VertexElement};
        const STRIDE: u32 = 96;
        let mut raw: Vec<u8> = Vec::new();
        let mut vertices = Vec::new();
        let mut ranges = Vec::with_capacity(levels.len());
        for cards in levels {
            let first = vertices.len() as u32;
            self.pack_leaf_cards(cards, &mut raw, &mut vertices);
            ranges.push((first, vertices.len() as u32 - first));
        }
        let element = |offset: u32, usage: DeclUsage, usage_index: u32| VertexElement {
            stream: 0,
            offset,
            ty: DeclType::Float4,
            usage,
            usage_index,
        };
        let key = format!("{name}:spt_leafcards");
        let count = vertices.len() as u32;
        self.scene.meshes.push(SceneMesh {
            key: key.clone(),
            chunk: MeshChunk {
                name: key,
                vertex_size: STRIDE,
                vertex_count: count,
                index_count: count,
                primitive: 0,
                index_format: thedas_formats::mesh::IndexFormat::U32,
                base_vertex: 0,
                vertex_offset: 0,
                start_index: 0,
                elements: vec![
                    element(0, DeclUsage::Position, 0),
                    element(16, DeclUsage::Normal, 0),
                    element(32, DeclUsage::TexCoord, 0),
                    element(48, DeclUsage::TexCoord, 1),
                    element(64, DeclUsage::TexCoord, 2),
                    element(80, DeclUsage::TexCoord, 3),
                ],
                bounds: None,
            },
            vertices,
            indices: (0..count).collect(),
            raw_vertices: raw,
            positions: None,
        });
        (self.scene.meshes.len() - 1, ranges)
    }

    /// One level's leaf cards, appended to a leaf-card mesh (see [`Self::spt_leaf_cards`]).
    fn pack_leaf_cards(&self, cards: &[LeafCard], raw: &mut Vec<u8>, vertices: &mut Vec<Vertex>) {
        for card in cards {
            // The card's size and pivot from its corners: corner 0 is (P8·w, P12·h),
            // corner 2 ((P8 − 1)·w, (P12 − 1)·h).
            let [c0, _, c2, _] = card.corners;
            let (w, h) = (c0[0] - c2[0], c0[1] - c2[1]);
            let (px, py) = (
                if w != 0.0 { c0[0] / w } else { 0.5 },
                if h != 0.0 { c0[1] / h } else { 0.5 },
            );
            let rad = std::f32::consts::PI / 180.0;
            for corner in [0usize, 1, 2, 0, 2, 3] {
                let p = card.position;
                let n = card.normals[corner];
                let uv = card.uv[corner];
                let wind = card.wind;
                let attrs: [[f32; 4]; 6] = [
                    [p[0], p[1], p[2], 1.0],
                    [n[0], n[1], n[2], 1.0],
                    [uv[0], uv[1], corner as f32, 1.0],
                    [w, h, px - 0.5, py - 0.5],
                    [
                        card.angle_offsets[0] * rad,
                        card.angle_offsets[1] * rad,
                        card.angle_index as f32,
                        card.color_scale,
                    ],
                    [
                        wind.matrices[0] as f32,
                        wind.weights[0],
                        wind.matrices[1] as f32,
                        wind.weights[1],
                    ],
                ];
                for a in attrs {
                    raw.extend_from_slice(bytemuck::cast_slice(&a));
                }
                vertices.push(Vertex {
                    position: p,
                    normal: n,
                    uv0: uv,
                    uv1: [0.0, 0.0],
                });
            }
        }
    }

    /// A tree's branches or fronds as the game packs them for `SPT_Branch` / `SPT_Fronds`
    /// (speedtree-exe-trace 8.1, 10.2), in tree space: POSITION, NORMAL, BINORMAL, TANGENT,
    /// TEXCOORD0 the wind (index, weight, index, weight), TEXCOORD1 the diffuse coordinate.
    /// `indices` is the level's triangle list with the strips' winding, which the branch
    /// state culls by. The game stores halves for the first four; floats here.
    fn spt_surface(&mut self, name: &str, part: &str, surface: &Part) -> (usize, Vec<(u32, u32)>) {
        // Every LOD level's triangles, one after another, each with its range.
        let mut indices: Vec<u32> = Vec::new();
        let mut ranges = Vec::with_capacity(surface.lods.len());
        for lod in &surface.lods {
            ranges.push((indices.len() as u32, lod.len() as u32));
            indices.extend_from_slice(lod);
        }
        use thedas_formats::mesh::{DeclType, VertexElement};
        const STRIDE: u32 = 96;
        let mut raw: Vec<u8> = Vec::with_capacity(surface.vertices.len() * STRIDE as usize);
        let mut vertices = Vec::with_capacity(surface.vertices.len());
        for (i, v) in surface.vertices.iter().enumerate() {
            let wind = surface.wind.get(i).copied().unwrap_or_default();
            let attrs: [[f32; 4]; 6] = [
                [v.position[0], v.position[1], v.position[2], 1.0],
                [v.normal[0], v.normal[1], v.normal[2], 1.0],
                [v.binormal[0], v.binormal[1], v.binormal[2], 1.0],
                [v.tangent[0], v.tangent[1], v.tangent[2], 1.0],
                [
                    wind.matrices[0] as f32,
                    wind.weights[0],
                    wind.matrices[1] as f32,
                    wind.weights[1],
                ],
                [v.uv[0], v.uv[1], 0.0, 1.0],
            ];
            for a in attrs {
                raw.extend_from_slice(bytemuck::cast_slice(&a));
            }
            vertices.push(Vertex {
                position: v.position,
                normal: v.normal,
                uv0: v.uv,
                uv1: [0.0, 0.0],
            });
        }
        let element = |offset: u32, usage: DeclUsage, usage_index: u32| VertexElement {
            stream: 0,
            offset,
            ty: DeclType::Float4,
            usage,
            usage_index,
        };
        let key = format!("{name}:spt_{part}");
        self.scene.meshes.push(SceneMesh {
            key: key.clone(),
            chunk: MeshChunk {
                name: key,
                vertex_size: STRIDE,
                vertex_count: vertices.len() as u32,
                index_count: indices.len() as u32,
                primitive: 0,
                index_format: thedas_formats::mesh::IndexFormat::U32,
                base_vertex: 0,
                vertex_offset: 0,
                start_index: 0,
                elements: vec![
                    element(0, DeclUsage::Position, 0),
                    element(16, DeclUsage::Normal, 0),
                    element(32, DeclUsage::Binormal, 0),
                    element(48, DeclUsage::Tangent, 0),
                    element(64, DeclUsage::TexCoord, 0),
                    element(80, DeclUsage::TexCoord, 1),
                ],
                bounds: None,
            },
            vertices,
            indices,
            raw_vertices: raw,
            positions: None,
        });
        (self.scene.meshes.len() - 1, ranges)
    }

    /// A tree's mesh leaves as the game packs them for `SPT_LeafMeshes`
    /// (speedtree-exe-trace §4; docs/speedtree 7.3): each leaf's copy of its choice's mesh,
    /// with POSITION and NORMAL the mesh vertex's, TEXCOORD0–2 the leaf's tangent, binormal
    /// and normal as three rows, TEXCOORD3 the leaf's position, TEXCOORD4 (u, v, leaf-angle
    /// index, colour scale) and TEXCOORD5 the wind. The shader rocks, turns and places it.
    fn spt_leaf_meshes(
        &mut self,
        name: &str,
        levels: &[Vec<MeshLeaf>],
        templates: &[Option<LeafMeshTemplate>],
    ) -> (usize, Vec<(u32, u32)>) {
        use thedas_formats::mesh::{DeclType, VertexElement};
        const STRIDE: u32 = 128;
        let mut raw: Vec<u8> = Vec::new();
        let mut vertices = Vec::new();
        let mut indices: Vec<u32> = Vec::new();
        let mut ranges = Vec::with_capacity(levels.len());
        for leaves in levels {
            let first = indices.len() as u32;
            for leaf in leaves {
                let Some(template) = templates.get(leaf.choice).and_then(|t| t.as_ref()) else {
                    continue;
                };
                let base = vertices.len() as u32;
                let (a, b, c) = (leaf.tangent, leaf.binormal, leaf.normal);
                let p = leaf.position;
                for v in &template.vertices {
                    let attrs: [[f32; 4]; 8] = [
                        [v.position[0], v.position[1], v.position[2], 1.0],
                        [v.normal[0], v.normal[1], v.normal[2], 1.0],
                        [a[0], b[0], c[0], 1.0],
                        [a[1], b[1], c[1], 1.0],
                        [a[2], b[2], c[2], 1.0],
                        [p[0], p[1], p[2], 1.0],
                        [v.uv[0], v.uv[1], leaf.angle_index as f32, leaf.color_scale],
                        [
                            leaf.wind.matrices[0] as f32,
                            leaf.wind.weights[0],
                            leaf.wind.matrices[1] as f32,
                            leaf.wind.weights[1],
                        ],
                    ];
                    for attr in attrs {
                        raw.extend_from_slice(bytemuck::cast_slice(&attr));
                    }
                    // For the bounds: the leaf's position stands in for the placed vertex.
                    vertices.push(Vertex {
                        position: p,
                        normal: c,
                        uv0: v.uv,
                        uv1: [0.0, 0.0],
                    });
                }
                indices.extend(template.indices.iter().map(|i| base + i));
            }
            ranges.push((first, indices.len() as u32 - first));
        }
        let element = |offset: u32, usage: DeclUsage, usage_index: u32| VertexElement {
            stream: 0,
            offset,
            ty: DeclType::Float4,
            usage,
            usage_index,
        };
        let key = format!("{name}:spt_leafmeshes");
        self.scene.meshes.push(SceneMesh {
            key: key.clone(),
            chunk: MeshChunk {
                name: key,
                vertex_size: STRIDE,
                vertex_count: vertices.len() as u32,
                index_count: indices.len() as u32,
                primitive: 0,
                index_format: thedas_formats::mesh::IndexFormat::U32,
                base_vertex: 0,
                vertex_offset: 0,
                start_index: 0,
                elements: vec![
                    element(0, DeclUsage::Position, 0),
                    element(16, DeclUsage::Normal, 0),
                    element(32, DeclUsage::TexCoord, 0),
                    element(48, DeclUsage::TexCoord, 1),
                    element(64, DeclUsage::TexCoord, 2),
                    element(80, DeclUsage::TexCoord, 3),
                    element(96, DeclUsage::TexCoord, 4),
                    element(112, DeclUsage::TexCoord, 5),
                ],
                bounds: None,
            },
            vertices,
            indices,
            raw_vertices: raw,
            positions: None,
        });
        (self.scene.meshes.len() - 1, ranges)
    }

    /// A SpeedTree surface as a scene mesh.
    fn tree_mesh(
        &mut self,
        name: &str,
        part: &str,
        vertices: &[MeshVertex],
        indices: Vec<u32>,
    ) -> usize {
        let vertices = vertices
            .iter()
            .map(|v| Vertex {
                position: v.position,
                normal: v.normal,
                uv0: v.uv,
                uv1: [0.0, 0.0],
            })
            .collect();
        self.push_mesh(format!("{name}:{part}"), vertices, indices)
    }

    /// Two crossed upright quads `2·half` wide and `height` tall, standing on the origin,
    /// textured with the atlas rectangle `[u0, u1, v_top, v_bottom]`.
    fn billboard_mesh(&mut self, name: &str, half: f32, height: f32, uv: [f32; 4]) -> usize {
        let [u0, u1, vt, vb] = uv;
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        for (axis, normal) in [(Vec3::X, Vec3::Y), (Vec3::Y, -Vec3::X)] {
            let base = vertices.len() as u32;
            for (x, z, u, v) in [
                (-half, 0.0, u0, vb),
                (half, 0.0, u1, vb),
                (half, height, u1, vt),
                (-half, height, u0, vt),
            ] {
                vertices.push(Vertex {
                    position: (axis * x + Vec3::Z * z).to_array(),
                    normal: normal.to_array(),
                    uv0: [u, v],
                    uv1: [0.0, 0.0],
                });
            }
            // Both faces: the quads are seen from either side.
            indices.extend([0, 1, 2, 0, 2, 3, 0, 2, 1, 0, 3, 2].map(|i| base + i));
        }
        self.push_mesh(format!("{name}:billboard"), vertices, indices)
    }

    /// A generated mesh (plain vertices, a triangle list) added to the scene.
    fn push_mesh(&mut self, key: String, vertices: Vec<Vertex>, indices: Vec<u32>) -> usize {
        self.scene.meshes.push(SceneMesh {
            key: key.clone(),
            chunk: MeshChunk {
                name: key,
                vertex_size: 0,
                vertex_count: vertices.len() as u32,
                index_count: indices.len() as u32,
                primitive: 0,
                index_format: thedas_formats::mesh::IndexFormat::U32,
                base_vertex: 0,
                vertex_offset: 0,
                start_index: 0,
                elements: Vec::new(),
                bounds: None,
            },
            vertices,
            indices,
            raw_vertices: Vec::new(),
            positions: None,
        });
        self.scene.meshes.len() - 1
    }

    /// A material object's resolution down to its definition (see [`Self::material`]).
    fn build_material(&mut self, key: &str, d: &[u8]) -> Option<SceneMaterial> {
        match MaterialObject::parse(d) {
            Ok(object) => {
                let mut semantic = object.semantic().to_string();
                let definition = self.material_def(&object.material_resource());
                // An object whose semantic its material lacks (no semantic named, of a
                // material without `Default`: the sky's cloud layer, `Clouds`; or a name
                // the material does not have: the lava's `Diffuse`, `Lava.mat` having
                // only `Opaque` and `OpaqueVolume`) takes the material's first.
                // Provisional: the exe's lookup on a missing semantic is not traced.
                if let Some(d) = definition.as_ref()
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
                    mao: key.to_string(),
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
        }
    }

    fn material(&mut self, mao_stem: &str) -> Option<usize> {
        // Terrain chunks name their material with the extension, other meshes without.
        let stem = mao_stem.to_ascii_lowercase();
        let key = format!("{}.mao", stem.strip_suffix(".mao").unwrap_or(&stem));
        if let Some(i) = self.material_index.get(&key) {
            return *i;
        }
        let built = match self.read(&key) {
            Ok(d) => self.build_material(&key, &d),
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
        // Choices made over a face (character creation's sliders) ride after its name.
        let (base, overlay) = FaceOverlay::split(name);
        if !overlay.is_empty() {
            // Face-shape sliders reshape a preset face: its targets with theirs in place.
            let base_morph = if !overlay.targets.is_empty()
                && self
                    .read(&format!("{}.mop", base.to_ascii_lowercase()))
                    .is_ok()
            {
                self.preset_morph(base, &overlay.targets)
            } else {
                self.morph(base)
            };
            let loaded = base_morph.map(|m| {
                let mut m = m.with_overlay(&overlay);
                // A tint chosen for a slot colours that slot's meshes.
                for (slot, name) in &overlay.tints {
                    if let Some(t) = name.as_deref().and_then(|n| self.tint(n)) {
                        m = m.with_tint(*slot, &t);
                    }
                }
                Arc::new(m)
            });
            self.morphs.insert(key, loaded.clone());
            return loaded;
        }
        // A face-screen preset (`hm_cps_p01.mop`) on its race and gender's base face
        // (`hm_pcc_b01.mor`, the face screen's last preset). Provisional: the base is named
        // after the preset's first two letters.
        let preset = format!("{}.mop", name.to_ascii_lowercase());
        let is_base = name.to_ascii_lowercase().ends_with("_pcc_b01");
        if !is_base && self.read(&key).is_err() && self.read(&preset).is_ok() {
            let loaded = self.preset_morph(name, &[]);
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

    /// A preset face: the race and gender's base face with the preset's choices on it, and
    /// `targets` (the face-shape sliders' weights) in place of the preset's same targets.
    fn preset_morph(&mut self, name: &str, targets: &[(String, f32)]) -> Option<Arc<Morph>> {
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
        let mut weights = preset.targets.clone();
        for (t, w) in targets {
            match weights.iter_mut().find(|(n, _)| n.eq_ignore_ascii_case(t)) {
                Some(e) => e.1 = *w,
                None => weights.push((t.clone(), *w)),
            }
        }
        // Each family's neutral target carries minus the sum of the others (see
        // `blend_targets`), so the sliders' new weights move it too.
        if !targets.is_empty() {
            rebalance_neutrals(&mut weights);
        }
        weights.retain(|(_, w)| *w != 0.0);
        let targets: Vec<(f32, Vec<FacePiece>)> = weights
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
                position: None,
                room: None,
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

    /// Gives each draw the probe the original's light manager would pick for it, when the
    /// rooms' light models place their probes: among the probes whose room reaches the
    /// draw's ([`crate::d3d::reaches`]), the nearest to its bounds' centre (DAOrigins.exe's
    /// probe influence is `1 / d`, 0x53d3e0, one slot). Draws no placed probe reaches keep
    /// the room's (provisional). Only the placed objects' character and placeable draws
    /// ([`crate::d3d::LightClass`]): the level's own draws and level-class draws keep their room's probe (see
    /// [`crate::d3d::pick_level_lights`] for why the traced rule is not applied to them yet).
    fn assign_probes(&mut self) {
        let scene = &mut self.scene;
        if scene.probes.iter().all(|p| p.position.is_none()) {
            return;
        }
        let first = scene.static_draws;
        for d in scene.draws.iter_mut().skip(first) {
            let class =
                crate::d3d::LightClass::of(scene.materials[d.material].render_class.as_deref());
            if class == crate::d3d::LightClass::Level {
                continue;
            }
            if let Some(p) = nearest_probe(&scene.probes, &scene.light_rooms, d.sphere.0, d.room_id)
            {
                d.probe = Some(p);
            }
        }
    }

    /// Adds the draws (and lights) of one placed model: `placement` is model to world.
    /// Returns its rigid (unskinned) draws with the names of their mesh nodes, which an
    /// animation of the model moves (a door's `DoorLow`).
    fn place(
        &mut self,
        model_name: &str,
        placement: Mat4,
        at: Placing<'_>,
    ) -> Vec<(usize, String)> {
        let Placing {
            scale,
            sphere,
            lightmaps,
            room,
            room_id,
            layout_room,
            probe,
            morph,
            tint,
            lights_only,
            user_params,
            skin,
        } = at;
        let mut rigid = Vec::new();
        let Some(model) = self.model(model_name) else {
            return rigid;
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
                    is_static,
                    version,
                } => SceneLight::Point {
                    position: m.transform_point3(Vec3::ZERO),
                    color: Vec3::new(color[0], color[1], color[2]),
                    radius: *radius,
                    name,
                    domain: *affect_domain,
                    is_static: is_static.unwrap_or(false),
                    version: version.unwrap_or(0),
                    room: layout_room,
                },
                thedas_formats::model::Light::Ambient { color } => SceneLight::Ambient {
                    color: Vec3::new(color[0], color[1], color[2]),
                    name,
                    room: room.to_string(),
                },
            });
        }
        // The light probes a room's light model places: where they stand, and their room.
        if let Some(room) = layout_room {
            for (file, chain) in model.irradiance_pieces() {
                let at = chain
                    .iter()
                    .fold(placement, |m, t| m * transform_matrix(t))
                    .transform_point3(Vec3::ZERO);
                let stem = file.to_ascii_lowercase();
                let stem = stem.trim_end_matches(".mtx");
                if let Some(p) = self
                    .scene
                    .probes
                    .iter_mut()
                    .find(|p| p.position.is_none() && p.name.trim_end_matches(".mtx") == stem)
                {
                    p.position = Some(at);
                    p.room = Some(room);
                }
            }
        }
        if lights_only {
            return rigid;
        }
        let cut_away = model.mesh_pieces().iter().any(|p| p.mesh.cut_away);
        self.place_emitters(model_name, &model, placement, user_params, cut_away);
        let pieces = model.mesh_pieces();
        if pieces.is_empty() {
            return rigid;
        }
        let Some(msh_name) = model.mesh_file.clone() else {
            self.warn(format!("{model_name}.mmh: mesh nodes but no .msh"));
            return rigid;
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
            if let Some(skin) = skin {
                material = self.skinned_material(material, skin);
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
            if skin.is_none()
                && let Some(n) = piece.node_name
            {
                rigid.push((self.scene.draws.len(), n.to_string()));
            }
            self.scene.draws.push(DrawItem {
                mesh,
                material,
                transform,
                lightmap,
                sphere,
                model: model_name.to_string(),
                room: room.to_string(),
                room_id,
                probe,
                skin,
                cast_shadow: piece.mesh.cast_runtime_shadow,
                lod: None,
            });
            if piece.mesh.cut_away {
                self.scene.cut_away.push(self.scene.draws.len() - 1);
            }
        }
        rigid
    }

    /// A visual effect model on `prop` (DAOrigins.exe's crust effects, `VFX_base.gda` models
    /// such as `fxa_fire_crust_c`): its emitters, placed on the body and following it. An
    /// emitter under a crust node (`crst`, named after a body node: `CrustHook1` …) sits
    /// where the first of the prop's models with that node has it, by the transforms below
    /// the crust node, and follows that node as the body moves; the others follow the
    /// prop's root by their place in the effect model. Their birthrates come from the
    /// effect's duration animation (`<effect>.dura.ani`, whose float tracks are named
    /// `<emitter>_birthrate` and `<emitter>_birthraterange`), looped: the models themselves
    /// carry 0. Provisional: the impact (`.impa`) and cessation (`.cess`) animations the
    /// game plays around it are not played, the crust node's own transform is not used,
    /// effect models' meshes and other animated parameters (`gob_remotematerialalpha`)
    /// are not drawn, and an effect on a body without the named node is left out.
    fn place_effect(&mut self, instance: &PropEffect, prop: &Prop) -> Vec<PropEmitter> {
        use thedas_formats::emitter::Emitter;
        use thedas_formats::model::{KIND_CRUST, Node};
        let effect = instance.model.as_str();
        let Some(model) = self.model(effect) else {
            self.warn(format!("{}: no effect model {effect}", prop.label));
            return Vec::new();
        };
        // Emitters with the crust node above them, and their transform below it (or from
        // the model root when there is none).
        type Found<'a> = Vec<(&'a Emitter, Option<&'a str>, Option<&'a str>, Mat4)>;
        fn walk<'a>(n: &'a Node, below: Mat4, hook: Option<&'a str>, out: &mut Found<'a>) {
            let (below, hook) = if n.kind == KIND_CRUST {
                (Mat4::IDENTITY, n.name.as_deref().or(hook))
            } else {
                (below * transform_matrix(&n.transform), hook)
            };
            if let Some(e) = &n.emitter {
                out.push((e, n.name.as_deref(), hook, below));
            }
            for c in &n.children {
                walk(c, below, hook, out);
            }
        }
        let mut found = Vec::new();
        for n in &model.nodes {
            walk(n, Mat4::IDENTITY, None, &mut found);
        }
        let mut animation = |suffix: &str| {
            self.read(&format!("{effect}.{suffix}.ani"))
                .ok()
                .and_then(|d| thedas_formats::anim::Animation::parse(&d).ok())
        };
        let (impact, looping) = (animation("impa"), animation("dura"));
        let mut out = Vec::new();
        for (k, (e, name, hook, below)) in found.into_iter().enumerate() {
            let (in_model, follows) = match hook {
                Some(hook) => {
                    let body = prop.models.iter().find_map(|m| {
                        let mmh = self.model(m)?;
                        let chain = mmh.node_chain(hook)?;
                        Some((m.clone(), mmh, chain))
                    });
                    let Some((body, mmh, chain)) = body else {
                        continue;
                    };
                    let at = chain
                        .iter()
                        .fold(Mat4::IDENTITY, |m, t| m * transform_matrix(t));
                    let skeleton = self.skeleton(&body, &mmh);
                    let node = self.scene.skeletons[skeleton].node(hook);
                    (at * below, node.map(|n| (skeleton, n)))
                }
                None => (below, None),
            };
            // The emitter's tracks in an animation; an animation that plays without them
            // gives it nothing.
            let rate_in = |a: &Option<thedas_formats::anim::Animation>| {
                let a = a.as_ref()?;
                let name = name.unwrap_or("").to_ascii_lowercase();
                let track = |suffix: &str| {
                    a.scalars
                        .iter()
                        .find(|s| s.target == format!("{name}_{suffix}"))
                        .cloned()
                };
                Some(EffectRate {
                    length: a.length,
                    birthrate: track("birthrate"),
                    range: track("birthraterange"),
                })
            };
            let has_tracks = |r: &Option<EffectRate>| {
                r.as_ref()
                    .is_some_and(|r| r.birthrate.is_some() || r.range.is_some())
            };
            let (impact, duration) = (rate_in(&impact), rate_in(&looping));
            // An emitter no animation drives keeps its own birthrate while a lasting
            // effect is on.
            let rate =
                (has_tracks(&impact) || has_tracks(&duration) || !instance.lasting).then(|| {
                    EffectTimeline {
                        age: instance.age,
                        lasting: instance.lasting,
                        impact,
                        duration,
                    }
                });
            if let Some(emitter) =
                self.push_emitter(effect, e, name, prop.transform * in_model, &[], false)
            {
                self.scene.emitters[emitter].owner =
                    Some(format!("{}/{}/{k}", prop.label, instance.key));
                out.push(PropEmitter {
                    emitter,
                    in_model,
                    follows,
                    rate,
                });
            }
        }
        out
    }

    /// The particle emitters of a placed model. Mesh-particle and physics emitters are left
    /// out (not mirrored yet), as are emitters whose material does not load.
    fn place_emitters(
        &mut self,
        model_name: &str,
        model: &Mmh,
        placement: Mat4,
        user_params: &[(String, [f32; 4])],
        cut_away: bool,
    ) {
        for piece in model.emitters() {
            let node = piece
                .chain
                .iter()
                .fold(Mat4::IDENTITY, |m, t| m * transform_matrix(t));
            self.push_emitter(
                model_name,
                piece.emitter,
                piece.name,
                placement * node,
                user_params,
                cut_away,
            );
        }
    }

    /// Adds one emitter at `transform` (node to world) to the scene and returns its index:
    /// `None` for those not drawn (mesh-particle and physics emitters, and emitters whose
    /// material does not load).
    fn push_emitter(
        &mut self,
        model_name: &str,
        e: &thedas_formats::emitter::Emitter,
        name: Option<&str>,
        transform: Mat4,
        user_params: &[(String, [f32; 4])],
        cut_away: bool,
    ) -> Option<usize> {
        {
            if e.mesh_model.is_some() || e.physics || e.material.is_empty() {
                return None;
            }
            let mut material = self.material(&e.material)?;
            if e.flipbook {
                let (c, r) = (e.flipbook_columns.max(1), e.flipbook_rows.max(1));
                material = self.overridden_material(
                    material,
                    &format!("sheet{c}x{r}"),
                    &[],
                    &[(
                        CONTACT_SHEET_PARAMETER.to_string(),
                        vec![[c as f32, r as f32, (c * r) as f32, 0.0]],
                    )],
                );
            }
            let user = e.user_param.as_deref().and_then(|name| {
                user_params
                    .iter()
                    .find(|(n, _)| n.eq_ignore_ascii_case(name))
            });
            if let Some((name, value)) = user {
                material = self.overridden_material(
                    material,
                    &format!("upar:{name}:{value:?}"),
                    &[],
                    &[(USER_COLOR_PARAMETER.to_string(), vec![*value])],
                );
            }
            log::debug!(
                "emitter {model_name}:{} at {:?} ({})",
                name.unwrap_or(""),
                transform.w_axis.truncate(),
                e.material
            );
            self.scene.emitters.push(SceneEmitter {
                emitter: Arc::new(e.clone()),
                transform,
                material,
                model: model_name.to_string(),
                node: name.unwrap_or("").to_string(),
                cut_away,
                owner: None,
            });
            Some(self.scene.emitters.len() - 1)
        }
    }

    /// `base` with a wearer's skin tone in its tint mask's skin channel: the skin colours
    /// (entries 8, 9 in the tint's shader order) and the skin opacities (the `w` of entries 0
    /// and 1). Bare hands and arms of a body, clothing or glove model take the face's skin
    /// tint (its morph's slot 0, `t1_skn_*`, whose opacities are (0, 0, 0, 1)); the item tint
    /// (or the material's own colours) keeps the red, green and blue channels. Item tints
    /// carry zero skin opacity, and a material without a tint array has the definition's
    /// default (opacities 0), so without this every body part's skin stays the white of
    /// its diffuse texture. The tint masks' alpha marks only skin (`hm_arm_nuda_0t` 95%,
    /// noble clothing `pm_cth_nblc_0t` 4%). Only the `*ArmourSkinTint` semantics take it:
    /// their shader (`Ch1ArmTnt.psh`) multiplies the diffuse texture by the skin colour,
    /// where the `LerpedTint` one (`Ch1.psh`) would replace the texture with it (Rica's arms
    /// in Dust Town turned flat and pale). Provisional: the exe's composition of the body
    /// tint is not traced.
    fn skinned_material(&mut self, base: usize, skin: &Tint) -> usize {
        let m = &self.scene.materials[base];
        if !m.semantic.to_ascii_lowercase().contains("skintint") {
            return base;
        }
        let mut colours: Vec<[f32; 4]> = m
            .object
            .vector_arrays
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(TINT_PARAMETER))
            .map(|(_, v)| {
                v.chunks_exact(4)
                    .map(|c| [c[0], c[1], c[2], c[3]])
                    .collect()
            })
            .or_else(|| crate::d3d::default_vectors(m, TINT_PARAMETER))
            .unwrap_or_default();
        if colours.len() < thedas_formats::tint::COLOURS {
            // The definitions' default: zero opacities, white colours.
            let start = colours.len();
            colours.extend((start..thedas_formats::tint::COLOURS).map(|i| {
                if SKIN_OPACITY_ENTRIES.contains(&i) {
                    [0.0; 4]
                } else {
                    [1.0; 4]
                }
            }));
        }
        for e in SKIN_OPACITY_ENTRIES {
            colours[e][3] = skin.colours[e][3];
        }
        for e in SKIN_ENTRIES {
            colours[e] = skin.colours[e];
        }
        let key: String = SKIN_ENTRIES
            .iter()
            .flat_map(|&e| skin.colours[e])
            .chain(SKIN_OPACITY_ENTRIES.iter().map(|&e| skin.colours[e][3]))
            .map(|x| format!("{x:.4}"))
            .collect::<Vec<_>>()
            .join(",");
        self.overridden_material(
            base,
            &format!("skin:{key}"),
            &[],
            &[(TINT_PARAMETER.to_string(), colours)],
        )
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

/// The probe (index into `probes`) DAOrigins.exe's light manager picks for an object at
/// `at` in room `room`: the nearest placed probe whose room reaches it (see
/// `Loader::assign_probes`).
pub fn nearest_probe(
    probes: &[SceneProbe],
    light_rooms: &HashMap<i64, Vec<i64>>,
    at: Vec3,
    room: Option<i64>,
) -> Option<usize> {
    probes
        .iter()
        .enumerate()
        .filter_map(|(i, p)| {
            let pos = p.position?;
            crate::d3d::reaches(p.room, light_rooms, room).then(|| (i, pos.distance(at)))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| i)
}

/// The id of the smallest room box (see [`AreaScene::room_boxes`]) holding `p`.
/// Provisional: the original's object room (its `vfunc+0x6c`) is not traced.
pub fn room_id_at(boxes: &[(i64, Vec3, Vec3)], p: Vec3) -> Option<i64> {
    boxes
        .iter()
        .filter(|(_, lo, hi)| p.cmpge(*lo).all() && p.cmple(*hi).all())
        .min_by(|a, b| {
            let vol = |(_, lo, hi): &&(i64, Vec3, Vec3)| (*hi - *lo).element_product();
            vol(a).total_cmp(&vol(b))
        })
        .map(|(id, _, _)| *id)
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
        Self::load_parts(vfs, level, props, true, false, Detail::default())
    }

    /// The objects alone in a level: `props` lit by the level's lights and probes, without
    /// the level's own meshes, for drawing over a scene of the level drawn once (see
    /// `AreaRenderer::render_over`): objects that come and go then rebuild only themselves.
    pub fn load_objects(vfs: &Vfs, level: &str, props: &[Prop]) -> Result<AreaScene, SceneError> {
        Self::load_parts(vfs, level, props, false, false, Detail::default())
    }

    /// [`AreaScene::load_with_props`] at a graphics detail level.
    pub fn load_with_props_at(
        vfs: &Vfs,
        level: &str,
        props: &[Prop],
        detail: Detail,
    ) -> Result<AreaScene, SceneError> {
        Self::load_parts(vfs, level, props, true, false, detail)
    }

    /// [`AreaScene::load_objects`] at a graphics detail level.
    pub fn load_objects_at(
        vfs: &Vfs,
        level: &str,
        props: &[Prop],
        detail: Detail,
    ) -> Result<AreaScene, SceneError> {
        Self::load_parts(vfs, level, props, false, false, detail)
    }

    /// Only the props' visual effects ([`Prop::effects`]): the props' models are read for
    /// the nodes the effects sit on and follow, and not drawn. A scene of its own for the
    /// effects lets them come and go without the objects' scene being rebuilt.
    pub fn load_effects_at(
        vfs: &Vfs,
        level: &str,
        props: &[Prop],
        detail: Detail,
    ) -> Result<AreaScene, SceneError> {
        Self::load_parts(vfs, level, props, false, true, detail)
    }

    fn load_parts(
        vfs: &Vfs,
        level: &str,
        props: &[Prop],
        level_meshes: bool,
        effects_only: bool,
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
        l.scene.cutaway_height = Some(arl.cutaway_height);
        // The level's wind file, `<level>_<n>.wnd` (one per level).
        let prefix = format!("{}_", level.to_ascii_lowercase());
        let wnd = vfs
            .names()
            .into_iter()
            .find(|n| n.starts_with(&prefix) && n.ends_with(".wnd"));
        if let Some(name) = wnd {
            match l.read(&name).map(|d| thedas_formats::level::Wnd::parse(&d)) {
                Ok(Ok(w)) => l.scene.wind = Some((w.strength, w.direction)),
                Ok(Err(e)) => l.warn(format!("{name}: {e}")),
                Err(e) => l.warn(e.to_string()),
            }
        }
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
        l.scene.room_boxes = arl
            .rooms
            .iter()
            .map(|r| {
                (
                    r.id,
                    Vec3::from_array(r.bounds_min),
                    Vec3::from_array(r.bounds_max),
                )
            })
            .collect();
        // SpeedTree placements (rooms' and the level's): tree, world placement, scale.
        let mut tree_spots: Vec<(usize, Mat4, f32, [f32; 4])> = Vec::new();
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
            l.scene.light_rooms.insert(room.id, rml.light_rooms.clone());
            let room_m = transform_matrix(&rml.transform);
            tree_spots.extend(rml.trees.iter().map(|t| {
                (
                    t.tree,
                    room_m * transform_matrix(&t.transform),
                    t.scale,
                    t.color,
                )
            }));
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
                        room_id: Some(room.id),
                        layout_room: Some(room.id),
                        probe: room_probe,
                        morph: None,
                        tint: None,
                        lights_only: !level_meshes,
                        user_params: &p.user_params,
                        skin: None,
                    },
                );
            }
        }
        // The level's own layout (`<level>.rml`, beside the rooms' in the level archive):
        // what stands outside the rooms, such as water (`oth000d_water_1442`), the horizon
        // (`vst_frostbacks01_1`, `hro_fie_bkcitye`), tree lines, Ostagar's tents and its
        // SpeedTrees. It names no room id; its models belong to no room.
        let level_rml = format!("{level}.rml");
        if l.vfs.lookup(&level_rml).is_some() {
            match l.read(&level_rml).map(|d| Rml::parse(&d)) {
                Ok(Ok(rml)) => {
                    let probe = l.room_probe(level, "");
                    let level_m = transform_matrix(&rml.transform);
                    tree_spots.extend(rml.trees.iter().map(|t| {
                        (
                            t.tree,
                            level_m * transform_matrix(&t.transform),
                            t.scale,
                            t.color,
                        )
                    }));
                    for p in &rml.placements {
                        let placement = level_m
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
                                room: "",
                                room_id: None,
                                layout_room: None,
                                probe,
                                morph: None,
                                tint: None,
                                lights_only: !level_meshes,
                                user_params: &p.user_params,
                                skin: None,
                            },
                        );
                    }
                }
                Ok(Err(e)) => l.warn(format!("{level_rml}: {e}")),
                Err(e) => l.warn(e.to_string()),
            }
        }
        if level_meshes && !tree_spots.is_empty() {
            l.place_tree_billboards(level, &arl.trees, &tree_spots);
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
                    room_id: None,
                    layout_room: None,
                    probe: None,
                    morph: None,
                    tint: None,
                    lights_only: false,
                    user_params: &[],
                    skin: None,
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
            if effects_only {
                let emitters = prop
                    .effects
                    .iter()
                    .flat_map(|effect| l.place_effect(effect, prop))
                    .collect();
                l.scene.props.push(PropDraws {
                    label: prop.label.clone(),
                    transform: prop.transform,
                    draws: first..first,
                    attachments: Vec::new(),
                    emitters,
                });
                continue;
            }
            let at = prop.transform.w_axis.truncate();
            let in_room = room_at(&arl.rooms, at);
            let probe = match in_room {
                Some(room) => l.room_probe(level, &room.name),
                None => l.room_probe(level, ""),
            };
            // Objects are lit as part of the room they stand in.
            let room_name = in_room.map_or(prop.label.as_str(), |r| r.name.as_str());
            let room_id = in_room.map(|r| r.id);
            let placing = |room| Placing {
                scale: 1.0,
                sphere: None,
                lightmaps: &[],
                room,
                room_id,
                layout_room: None,
                probe,
                morph: None,
                tint: None,
                lights_only: false,
                user_params: &[],
                skin: None,
            };
            // The face's skin tone (its skin tint, slot 0), for the body's bare skin.
            let skin_tint: Option<Arc<Tint>> = prop
                .morph
                .as_ref()
                .and_then(|n| l.morph(n))
                .and_then(|m| m.tints.first().cloned().flatten())
                .and_then(|t| l.tint(&t));
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
            let mut rigid_draws = Vec::new();
            for model in &prop.models {
                let at = Placing {
                    tint: tint_of(model),
                    skin: skin_tint.as_deref(),
                    ..placing(room_name)
                };
                let rigid = l.place(model, prop.transform, at);
                rigid_draws.push((model.clone(), rigid));
            }
            // A model's rigid meshes follow their nodes as its animation moves them (a
            // door's leaf swinging open), as attachments to its own skeleton.
            let mut attachments = Vec::new();
            for (model, rigid) in rigid_draws {
                if rigid.is_empty() {
                    continue;
                }
                let Some(mmh) = l.model(&model) else { continue };
                let skeleton = l.skeleton(&model, &mmh);
                for (d, node) in rigid {
                    if let Some(n) = l.scene.skeletons[skeleton].node(&node) {
                        attachments.push(PropAttachment {
                            draws: d..d + 1,
                            skeleton,
                            node: n,
                        });
                    }
                }
            }
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
            let draws = first..l.scene.draws.len();
            let emitters = prop
                .effects
                .iter()
                .flat_map(|effect| l.place_effect(effect, prop))
                .collect();
            l.scene.props.push(PropDraws {
                label: prop.label.clone(),
                transform: prop.transform,
                draws,
                attachments,
                emitters,
            });
        }
        l.assign_probes();
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

/// A tree's bark texture as the resource it loads: the file names a `.tga`, the game ships
/// the `.dds` (`newbarkL.tga` → `newbarkl.dds`).
fn bark_texture(name: &str) -> String {
    let name = name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(name)
        .to_ascii_lowercase();
    match name.rsplit_once('.') {
        Some((stem, _)) => format!("{stem}.dds"),
        None => format!("{name}.dds"),
    }
}

/// A closed surface's triangles turned to face out (the renderer's front face is clockwise
/// seen from the outside, where the vertex normals point). The generator keeps the strips'
/// winding, which this does not rely on.
fn facing_front(vertices: &[MeshVertex], indices: &[u32]) -> Vec<u32> {
    let at = |i: u32| vertices.get(i as usize);
    let mut out = Vec::with_capacity(indices.len());
    for t in indices.chunks_exact(3) {
        let (Some(a), Some(b), Some(c)) = (at(t[0]), at(t[1]), at(t[2])) else {
            continue;
        };
        let p = |v: &MeshVertex| Vec3::from(v.position);
        let n = Vec3::from(a.normal) + Vec3::from(b.normal) + Vec3::from(c.normal);
        let face = (p(b) - p(a)).cross(p(c) - p(a));
        if face.dot(n) <= 0.0 {
            out.extend([t[0], t[1], t[2]]);
        } else {
            out.extend([t[0], t[2], t[1]]);
        }
    }
    out
}

/// A triangle list drawn from both sides (fronds are thin sheets).
fn both_sides(indices: &[u32]) -> Vec<u32> {
    let mut out = indices.to_vec();
    out.extend(indices.chunks_exact(3).flat_map(|t| [t[0], t[2], t[1]]));
    out
}

/// Leaf cards as two fixed upright crossed quads each, both sides. Interim: the original
/// turns each card to the camera in its vertex shader (7.2 "(exe)"); the scene is static.
fn leaf_cards(cards: &[LeafCard]) -> (Vec<MeshVertex>, Vec<u32>) {
    let mut vertices = Vec::with_capacity(cards.len() * 8);
    let mut indices = Vec::with_capacity(cards.len() * 24);
    for card in cards {
        for right in [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]] {
            let base = vertices.len() as u32;
            for i in 0..4 {
                vertices.push(MeshVertex {
                    position: card.corner(i, right, [0.0, 0.0, 1.0]),
                    normal: card.normals[i],
                    tangent: right,
                    binormal: [0.0, 0.0, 1.0],
                    uv: card.uv[i],
                    uv_detail: card.uv[i],
                    fade: 1.0,
                });
            }
            indices.extend([0, 1, 2, 0, 2, 3, 0, 2, 1, 0, 3, 2].map(|i| base + i));
        }
    }
    (vertices, indices)
}

/// The side of the ground cells SpeedTree placements are batched by, in metres.
const TREE_CELL: f32 = 24.0;

/// A SpeedTree's name for diagnostics (`spt:<name>`).
fn name_of_tree(trees: &[String], tree: usize) -> String {
    format!(
        "spt:{}",
        trees
            .get(tree)
            .map_or("?", |t| t.as_str())
            .to_ascii_lowercase()
    )
}

#[cfg(test)]
mod tests {

    #[test]
    fn a_tree_part_draws_its_level_inside_the_far_distance_only() {
        let lod = |part| TreeLod {
            anchor: Vec3::ZERO,
            near: 15.0,
            far: 35.0,
            counts: [3, 3, 3],
            transition: Default::default(),
            part,
            levels: vec![(0, 30), (30, 18), (48, 6)],
        };
        // Inside near: the full level, at the target alpha reference.
        let eye = Vec3::new(5.0, 0.0, 0.0);
        assert_eq!(lod(TreePart::Branches).frame(eye), Some((0, 30, 0.0)));
        assert_eq!(lod(TreePart::Fronds).frame(eye), Some((0, 30, 84.0)));
        assert_eq!(lod(TreePart::Leaves(0)).frame(eye), Some((0, 30, 84.0)));
        assert_eq!(lod(TreePart::Leaves(1)).frame(eye), None);
        // Beyond far: nothing (the billboard's range).
        let eye = Vec3::new(40.0, 0.0, 0.0);
        assert_eq!(lod(TreePart::Branches).frame(eye), None);
        assert_eq!(lod(TreePart::Fronds).frame(eye), None);
        // Part way: a lower branch level.
        let eye = Vec3::new(30.0, 0.0, 0.0);
        let (first, _, _) = lod(TreePart::Branches).frame(eye).unwrap();
        assert!(first > 0);
    }

    #[test]
    fn a_billboard_fades_in_at_the_end_of_the_lod_range_and_hides_inside_it() {
        let b = BillboardInstance {
            anchor: Vec3::ZERO,
            near: 15.0,
            far: 35.0,
            transition: Default::default(),
            size: [8.0, 12.0],
        };
        // Inside the geometry's range: no size, so nothing draws.
        assert_eq!(b.texcoord0(Vec3::new(20.0, 0.0, 0.0)), [0.0; 4]);
        // Beyond far: fully on.
        assert_eq!(
            b.texcoord0(Vec3::new(40.0, 0.0, 0.0)),
            [8.0, 12.0, 0.0, 1.0]
        );
        // Just inside far (LOD below the transition width 0.14): partly faded in.
        let [w, h, _, fade] = b.texcoord0(Vec3::new(34.5, 0.0, 0.0));
        assert_eq!((w, h), (8.0, 12.0));
        assert!(fade > 0.0 && fade <= 1.0, "{fade}");
    }

    #[test]
    fn an_effect_birthrate_loops_with_its_animation() {
        use thedas_formats::anim::ScalarTrack;
        let r = EffectRate {
            length: 2.0,
            birthrate: Some(ScalarTrack {
                target: "flame_default_birthrate".into(),
                keys: vec![(0.0, 5.0), (2.0, 10.0)],
            }),
            range: None,
        };
        assert_eq!(r.at(1.0), (7.5, 0.0));
        // Looped: 3 s is 1 s into the second pass.
        assert_eq!(r.at(3.0), (7.5, 0.0));
    }

    #[test]
    fn the_lod_distances_follow_the_radius() {
        assert_eq!(TreeLod::range(1.0, 0), (15.0, 35.0));
        assert_eq!(TreeLod::range(30.0, 0), (20.0, 120.0));
        assert_eq!(TreeLod::range(30.0, 2), (40.0, 240.0));
    }
    use super::*;

    #[test]
    fn an_object_takes_the_nearest_probe_its_room_sees() {
        let probe = |x: f32, room| SceneProbe {
            name: String::new(),
            archive: String::new(),
            probe: IrradianceProbe::uniform([0.5; 3]),
            position: Some(Vec3::new(x, 0.0, 0.0)),
            room,
        };
        let probes = vec![probe(1.0, Some(5)), probe(10.0, Some(6)), probe(50.0, None)];
        let rooms: HashMap<i64, Vec<i64>> = [(6, vec![7])].into_iter().collect();
        // Room 7 sees room 6's probe and the roomless one, not room 5's (nearer).
        assert_eq!(nearest_probe(&probes, &rooms, Vec3::ZERO, Some(7)), Some(1));
        assert_eq!(nearest_probe(&probes, &rooms, Vec3::ZERO, Some(5)), Some(0));
        assert_eq!(nearest_probe(&probes, &rooms, Vec3::ZERO, Some(8)), Some(2));
        assert_eq!(nearest_probe(&probes, &rooms, Vec3::ZERO, None), Some(0));
        let boxes = vec![
            (1, Vec3::splat(-10.0), Vec3::splat(10.0)),
            (2, Vec3::splat(-1.0), Vec3::splat(1.0)),
        ];
        assert_eq!(room_id_at(&boxes, Vec3::ZERO), Some(2));
        assert_eq!(room_id_at(&boxes, Vec3::splat(5.0)), Some(1));
        assert_eq!(room_id_at(&boxes, Vec3::splat(50.0)), None);
    }

    #[test]
    fn a_face_target_family_stays_balanced() {
        let mut w = vec![
            ("UH_UHM_BASa_0".to_string(), -0.5),
            ("UH_NOS_07a_0".to_string(), 0.5),
            ("UH_NOS_08a_0".to_string(), 0.25),
            ("UEM_EYE_03a_0".to_string(), 0.3),
        ];
        rebalance_neutrals(&mut w);
        let get = |n: &str| w.iter().find(|(t, _)| t == n).map(|(_, v)| *v);
        assert_eq!(get("UH_UHM_BASa_0"), Some(-0.75));
        assert_eq!(get("UH_UEM_BASa_0"), Some(-0.3));
        assert_eq!(get("UH_ULM_BASa_0"), None);
    }

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
