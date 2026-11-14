//! The deferred renderer: ours, not the original's, chosen at run time with
//! [`RenderPath`] (see `docs/deferred.md`).
//!
//! Eligible level, terrain, object and skinned character draws share a G-buffer; one pass lights them
//! per pixel with every light in reach: the sum Static1.psh works out per object for three
//! lights (`notes/frame-gamma-lightmap-trace.md`), over all of them. Everything else (water,
//! sky, tree billboards, unsupported character, blended and multi-pass materials, particles) is drawn
//! afterwards by the faithful pass, with its original shaders, over that colour and depth.
//!
//! Nothing here changes how the faithful path draws: with [`RenderPath::Faithful`] this
//! module is not reached, and its GPU objects are not even made.

use std::collections::HashMap;

use glam::{Mat4, Vec3, Vec4};
use thedas_formats::state::Cull;
use thedas_shader::bytecode::TextureType;
use thedas_vfs::Vfs;

use crate::camera::Camera;
use crate::d3d_gpu::D3dPass;
use crate::deferred_mesh::{DeferredVertex, ROWS, SKIN_STRIDE};
use crate::gpu::{DEPTH_FORMAT, GpuContext, GpuMesh, TextureCache, checked};
use crate::scene::{AreaScene, DOMAIN_CHARACTERS, DOMAIN_LEVEL, SceneLight};

#[path = "deferred_light_masks.rs"]
mod light_masks;

#[path = "deferred_trees.rs"]
mod trees;

#[path = "deferred_tree_shadow.rs"]
pub(crate) mod tree_shadow;

pub const DEFERRED_WGSL: &str = concat!(
    include_str!("deferred_skin.wgsl"),
    include_str!("deferred.wgsl")
);

#[cfg(test)]
#[path = "deferred_object_tests.rs"]
mod object_gpu_tests;

#[cfg(test)]
#[path = "deferred_gpu_tests.rs"]
mod gpu_tests;

#[cfg(test)]
#[path = "deferred_alpha_tests.rs"]
mod alpha_gpu_tests;

#[cfg(test)]
#[path = "deferred_reflection_tests.rs"]
mod reflection_gpu_tests;

#[cfg(test)]
#[path = "deferred_terrain_gpu_tests.rs"]
mod terrain_gpu_tests;

/// How the level is drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RenderPath {
    /// Every material through its original shaders (the default, and the reference).
    #[default]
    Faithful,
    /// Ours: eligible world geometry lit per pixel from a shared G-buffer.
    Deferred,
}

impl RenderPath {
    pub fn name(self) -> &'static str {
        match self {
            RenderPath::Faithful => "faithful",
            RenderPath::Deferred => "deferred",
        }
    }

    pub fn parse(s: &str) -> Option<RenderPath> {
        match s.trim().to_ascii_lowercase().as_str() {
            "faithful" => Some(RenderPath::Faithful),
            "deferred" => Some(RenderPath::Deferred),
            _ => None,
        }
    }

    /// The other one.
    pub fn toggled(self) -> RenderPath {
        match self {
            RenderPath::Faithful => RenderPath::Deferred,
            RenderPath::Deferred => RenderPath::Faithful,
        }
    }
}

/// The most lights one frame lights with (the nearest, past it): the shader's uniform
/// array, at most 16 KiB, which every backend can bind.
const MAX_LIGHTS: usize = 192;
/// Dynamic uniform offsets must be multiples of this on every backend.
const DRAW_STRIDE: usize = 1280;
/// Room indices the lights' room sets can name; rooms past it are lit by every light.
const MAX_ROOMS: u32 = 255;

/// The G-buffer's targets: albedo; specular mask and power; normal, room and specular
/// falloff; HDR base colour (or lightmap texel in separate-pass modes). That is 32
/// bytes a pixel as wgpu counts them, its limit. Baked frames fold reflection into
/// the base; probe/AO and diagnostic modes add it through [`Gpu::env`].
const GBUFFER: [wgpu::TextureFormat; 4] = [
    wgpu::TextureFormat::Rgba8Unorm,
    wgpu::TextureFormat::Rgba8Unorm,
    wgpu::TextureFormat::Rgba16Float,
    wgpu::TextureFormat::Rgba16Float,
];

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct FrameUniforms {
    eye: [f32; 4],
    /// xy: half-pixel correction, z: fused baked base, w: unused.
    fixup: [f32; 4],
    fog: [f32; 4],
}

/// What a draw's slot holds: its matrices this frame, then what it has throughout.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct DrawUniforms {
    mvp: [[f32; 4]; 4],
    world: [[f32; 4]; 4],
    data: DrawData,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
struct DrawData {
    lightmap: [f32; 4],
    falloff: [f32; 4],
    room: [f32; 4],
    terrain: crate::deferred_terrain::TerrainData,
    /// The assigned irradiance probe's twelve rows (RGB).
    probe: [[f32; 4]; 12],
    tint: [[f32; 4]; 10],
    /// x: 0 level, 1 prop, 2 character; y: emissive; z: tint; w: 1 face, 2 eye.
    material: [f32; 4],
    tint_color: [f32; 4],
    character: crate::deferred_character::CharacterData,
    alpha: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct LightingUniforms {
    inv_view_proj: [[f32; 4]; 4],
    view_proj: [[f32; 4]; 4],
    eye: [f32; 4],
    sun_dir: [f32; 4],
    sun_colour: [f32; 4],
    fog0: [f32; 4],
    fog1: [f32; 4],
    counts: [u32; 4],
    depth_plane: [f32; 4],
    size: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
struct TileUniforms {
    screen: crate::deferred_tiles::Masks,
    depth: crate::deferred_tiles::DepthMasks,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct SunUniforms {
    data: [[f32; 4]; 9],
    direction: [f32; 4],
    enabled: [u32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
struct GpuLight {
    position: [f32; 4],
    colour: [f32; 4],
    rooms: [u32; 8],
    /// x: its shadow slot ([`crate::deferred_shadow`]), −1 for none.
    shadow: [f32; 4],
}

/// One draw the deferred pass takes from the faithful one.
#[derive(Clone, Debug)]
struct PlanDraw {
    alpha_test: bool,
    /// 0: general material, 1: Static1, 2: terrain.
    kind: usize,
    /// Its index in [`D3dPass::draws`].
    k: usize,
    /// Its scene draw.
    draw: usize,
    mesh: usize,
    /// Its textures ([`DeferredPass::groups`]).
    group: usize,
    cull: Option<wgpu::Face>,
    /// Its world bounding sphere as placed, for picking shadow casters.
    sphere: (Vec3, f32),
    room: Option<i64>,
    skin_slot: usize,
    casts: bool,
}

/// The deferred geometry of one renderer: what it draws, worked out when the
/// renderer is made, and its GPU objects, made the first time a frame is drawn deferred.
pub(crate) struct DeferredPass {
    draws: Vec<PlanDraw>,
    trees: trees::Plan,
    tree_shadows: tree_shadow::Plan,
    tree_shadows_enabled: Option<bool>,
    cutouts: Vec<crate::deferred_cutout::Draw>,
    cutout_materials: Vec<crate::deferred_cutout::Material>,
    data: Vec<DrawData>,
    /// Diffuse, normal, specular, lightmap, cube, terrain masks, tint and emissive.
    groups: Vec<[wgpu::TextureView; 12]>,
    group_samplers: Vec<Option<wgpu::Sampler>>,
    vertices: HashMap<usize, Vec<DeferredVertex>>,
    skin_count: usize,
    /// Per faithful draw (index into [`D3dPass::draws`]): whether this pass draws it.
    takes: Vec<bool>,
    logged_materials: bool,
    /// The index each room id has in the lights' room sets (1 on; 0 is "no room").
    rooms: HashMap<i64, u32>,
    light_rooms: HashMap<i64, Vec<i64>>,
    gpu: Option<Gpu>,
    /// Lights only this pass draws, set each frame ([`DeferredPass::set_effect_lights`]):
    /// the glowing emitters of the renderers drawn over the level's.
    effect_lights: Vec<SceneLight>,
    /// The lights its own renderer's glowing emitters give, set each frame.
    own_lights: Vec<SceneLight>,
    /// The GPU refused the pass: the level stays faithful.
    refused: bool,
    /// Experimental probe + runtime visibility mode. Fixed when this renderer is built.
    baked_lighting: bool,
    ssao: bool,
    /// Diagnostic override: tiled lighting and cube-face culling, respectively.
    culling: Option<[bool; 2]>,
    shadow_reuse: Option<bool>,
    fused_reflections: Option<bool>,
    tight_tiles: Option<bool>,
    light_masks: Option<bool>,
    opaque_gbuffer: Option<bool>,
    specialized_gbuffer: Option<bool>,
    early_light_reject: Option<bool>,
    depth_lights: Option<bool>,
    direct_terrain_layers: Option<bool>,
}

pub(crate) struct Contributor<'a> {
    pub pass: &'a mut DeferredPass,
    pub d3d: &'a D3dPass,
    pub meshes: &'a [GpuMesh],
}

struct Gpu {
    trees: trees::Gpu,
    tree_shadows: tree_shadow::Gpu,
    /// By cull mode: none, back, front.
    gbuffer: [wgpu::RenderPipeline; 3],
    gbuffer_opaque: [wgpu::RenderPipeline; 3],
    gbuffer_static: Option<[[wgpu::RenderPipeline; 3]; 2]>,
    gbuffer_terrain: Option<[wgpu::RenderPipeline; 3]>,
    /// Separate reflection for probe/AO and diagnostic modes (by cull mode).
    env: [wgpu::RenderPipeline; 3],
    lighting: wgpu::RenderPipeline,
    frame_buffer: wgpu::Buffer,
    /// Every draw's [`DrawUniforms`], [`DRAW_STRIDE`] apart, rewritten each frame.
    draw_buffer: wgpu::Buffer,
    scene_group: wgpu::BindGroup,
    vertices: HashMap<usize, wgpu::Buffer>,
    skin_buffer: wgpu::Buffer,
    skin_slots: Vec<u8>,
    /// The draw slots as last written.
    slots: Vec<u8>,
    material_groups: Vec<wgpu::BindGroup>,
    lighting_buffer: wgpu::Buffer,
    light_buffer: wgpu::Buffer,
    tile_buffer: wgpu::Buffer,
    tiles: Option<TileUniforms>,
    mask_buffer: wgpu::Buffer,
    light_masks: Option<light_masks::Masks>,
    /// The shadowed lights' maps, and their faces' view-projections for the lighting.
    shadows: Option<crate::deferred_shadow::ShadowPool>,
    sun_fallback: wgpu::TextureView,
    sun_sampler: wgpu::Sampler,
    sun_buffer: wgpu::Buffer,
    sun_layout: wgpu::BindGroupLayout,
    sun_group: Option<wgpu::BindGroup>,
    faces_buffer: wgpu::Buffer,
    lighting_group: wgpu::BindGroup,
    targets_layout: wgpu::BindGroupLayout,
    targets: Option<Targets>,
    /// The draws in drawing order (grouped by pipeline and textures).
    order: Vec<usize>,
    /// The lights as last written.
    lights: Vec<GpuLight>,
}

struct Targets {
    size: (u32, u32),
    views: [wgpu::TextureView; 4],
    /// The G-buffer and the depth it was drawn with, for the lighting pass.
    group: wgpu::BindGroup,
    /// The depth buffer `group` reads.
    depth: wgpu::TextureView,
}

fn cull_of(c: Cull) -> Option<wgpu::Face> {
    // As the faithful pipelines map it (see `d3d_gpu`'s `D3D_CW_IS_WGPU_CW`).
    let cw = crate::d3d_gpu::D3D_CW_IS_WGPU_CW;
    match c {
        Cull::None => None,
        Cull::Cw => Some(if cw {
            wgpu::Face::Back
        } else {
            wgpu::Face::Front
        }),
        Cull::Ccw => Some(if cw {
            wgpu::Face::Front
        } else {
            wgpu::Face::Back
        }),
    }
}

fn cull_index(c: Option<wgpu::Face>) -> usize {
    match c {
        None => 0,
        Some(wgpu::Face::Back) => 1,
        Some(wgpu::Face::Front) => 2,
    }
}

/// Whether a faithful draw with `program` (its name `vs+ps+state`) can be drawn deferred:
/// Static1's opaque passes (Static1.psh, and Static1_Fast.psh at Low detail), which write
/// depth and test no alpha.
fn static1(program: &crate::d3d::D3dProgram) -> bool {
    let p = &program.pass;
    matches!(
        (
            program.name.split('+').next(),
            program.name.split('+').nth(1)
        ),
        (
            Some("static1.vsh"),
            Some("static1.psh" | "static1_fast.psh")
        )
    ) && !program.instanced
        && p.blend.is_none()
        && p.alpha_test.is_none()
        && p.z_enable
        && p.z_write
}

fn terrain(program: &crate::d3d::D3dProgram) -> bool {
    let p = &program.pass;
    (program.name.starts_with("terrain2.vsh+terrain1.psh+")
        || program.name.starts_with("terrain3.vsh+terrain3.psh+"))
        && !program.instanced
        && p.blend.is_none()
        && p.alpha_test.is_none()
        && p.z_enable
        && p.z_write
}

/// Only these single-pass Origins material equations are represented in the G-buffer.
/// Hair, alpha-tested and remote-material variants stay forward.
fn object_material(program: &crate::d3d::D3dProgram) -> bool {
    let mut names = program.name.split('+');
    matches!(
        (names.next(), names.next()),
        (Some("prop1.vsh"), Some("prop1.psh"))
            | (Some("weapon1.vsh"), Some("weapon1.psh"))
            | (Some("cloth1.vsh"), Some("cloth1.psh"))
            | (Some("face1.vsh"), Some("face0.psh"))
            | (Some("eye0.vsh"), Some("eye0.psh"))
            | (
                Some("char1.vsh"),
                Some(
                    "ch1.psh"
                        | "ch1notnt.psh"
                        | "ch1multnt.psh"
                        | "ch1armtnt.psh"
                        | "ch1em.psh"
                        | "ch1emnotnt.psh"
                        | "ch1emmultnt.psh"
                        | "ch1emarmtnt.psh"
                )
            )
    ) && !program.instanced
        && program.pass.blend.is_none()
        && program.pass.alpha_test.is_none()
        && program.pass.z_enable
        && program.pass.z_write
}

impl DeferredPass {
    /// What a renderer of `scene` draws deferred, with the
    /// textures it reads; `None` when nothing qualifies.
    pub(crate) fn plan(
        ctx: &GpuContext,
        vfs: &Vfs,
        scene: &AreaScene,
        d3d: &D3dPass,
        cache: &mut TextureCache,
    ) -> Option<DeferredPass> {
        let started = std::time::Instant::now();
        let punch_enabled = crate::config::CONFIG.punch;
        let punch: HashMap<usize, _> = if punch_enabled {
            d3d.draws
                .iter()
                .enumerate()
                .filter_map(|(k, d)| {
                    let m = scene.materials.get(scene.draws.get(d.draw)?.material)?;
                    Some((
                        k,
                        crate::deferred_alpha::material(ctx, vfs, cache, m, d3d.program(d))?,
                    ))
                })
                .collect()
        } else {
            HashMap::new()
        };
        let mut texture = |names: &[Option<String>], ty: TextureType, fallback: [u8; 4]| {
            names
                .iter()
                .flatten()
                .find_map(|n| cache.view(ctx, vfs, n, ty))
                .unwrap_or_else(|| cache.solid(ctx, ty, fallback))
        };
        let tex2d = TextureType::Tex2D;
        let mut rooms: HashMap<i64, u32> = HashMap::new();
        let mut group_of: HashMap<(usize, String, Option<usize>, String), usize> = HashMap::new();
        let mut groups = Vec::new();
        let mut group_samplers = Vec::new();
        let mut draws = Vec::new();
        let mut data = Vec::new();
        let mut takes = vec![false; d3d.draws.len()];
        let mut vertices = HashMap::new();
        let mut skin_count = 1; // Slot zero is identity for rigid meshes.
        let objects_enabled = crate::config::CONFIG.objects;
        let terrain_enabled = crate::config::CONFIG.terrain;
        let faces_enabled = crate::config::CONFIG.faces;
        for (k, d) in d3d.draws.iter().enumerate() {
            let program = d3d.program(d);
            let Some(item) = scene.draws.get(d.draw) else {
                continue;
            };
            let alpha = punch
                .get(&k)
                .filter(|_| objects_enabled || !crate::deferred_alpha::character(&program.name));
            let taken = !d3d.is_blended(d)
                && !d.multipass
                && (faces_enabled
                    || (!program.name.starts_with("face1.") && !program.name.starts_with("eye0.")))
                && (objects_enabled || (d.draw < scene.static_draws && item.skin.is_none()))
                && (static1(program)
                    || (terrain_enabled && terrain(program))
                    || (objects_enabled && object_material(program))
                    || alpha.is_some())
                && item
                    .skin
                    .as_ref()
                    .is_none_or(|s| s.palette.len() <= crate::deferred_mesh::BONES)
                && item.lod.is_none()
                && !scene.cut_away.contains(&d.draw)
                && item.lightmap.as_ref().is_none_or(|l| l.planes().is_none());
            let Some(m) = taken.then(|| scene.materials.get(item.material)).flatten() else {
                continue;
            };
            if let std::collections::hash_map::Entry::Vacant(entry) = vertices.entry(d.mesh) {
                let Some(v) = scene
                    .meshes
                    .get(d.mesh)
                    .and_then(|m| crate::deferred_mesh::vertices(m, item.skin.is_some()))
                else {
                    continue;
                };
                entry.insert(v);
            }
            let lightmap = item.lightmap.as_ref().map(|l| l.texture.clone());
            let is_terrain = terrain(program);
            let is_face = program.name.starts_with("face1.");
            let is_eye = program.name.starts_with("eye0.");
            let is_object =
                object_material(program) || crate::deferred_alpha::character(&program.name);
            // The probe's cubemap as the faithful pass binds it (see `d3d::sampler_texture`).
            let cube = item.probe.and_then(|p| scene.probes.get(p)).map(|p| {
                let name = p.name.to_ascii_lowercase();
                format!("{}.dds", name.trim_end_matches(".mtx"))
            });
            let group = *group_of
                .entry((
                    item.material,
                    lightmap.clone().unwrap_or_default(),
                    item.probe,
                    program.name.clone(),
                ))
                .or_insert_with(|| {
                    let param = |p: &str| [m.texture(p), crate::d3d::texture_default(m, p)];
                    group_samplers.push(alpha.map(|a| a.sampler.clone()));
                    groups.push([
                        alpha.map(|a| a.diffuse.clone()).unwrap_or_else(|| {
                            texture(
                                &param(if is_terrain {
                                    "palette"
                                } else {
                                    "mml_tDiffuse"
                                }),
                                tex2d,
                                [128, 128, 128, 255],
                            )
                        }),
                        // A flat normal in DXT5nm's channels (x in alpha, y in green).
                        texture(
                            &param(if is_terrain {
                                "normal"
                            } else {
                                "mml_tNormalMap"
                            }),
                            tex2d,
                            [128, 128, 128, 128],
                        ),
                        texture(
                            &param(if is_face {
                                "mml_tAgeNormalMap"
                            } else if is_eye {
                                "mml_tCorneaNormalMap"
                            } else {
                                "mml_tSpecularMask"
                            }),
                            tex2d,
                            if is_face || is_eye { [128; 4] } else { [0; 4] },
                        ),
                        if is_face {
                            texture(&param("mml_tEmotionsNormal"), tex2d, [128; 4])
                        } else {
                            texture(
                                &[lightmap.clone(), Some("default_black.dds".into())],
                                tex2d,
                                [0, 0, 0, 0],
                            )
                        },
                        texture(
                            &[cube, crate::d3d::texture_default(m, "aur_cmLightProbe0")],
                            TextureType::Cube,
                            [0, 0, 0, 255],
                        ),
                        texture(
                            &param(if is_face {
                                "mml_tAgeDiffuseMap"
                            } else {
                                "MaskV"
                            }),
                            tex2d,
                            [0, 0, 0, 255],
                        ),
                        texture(
                            &param(if is_face {
                                "mml_tEmotionsMask0"
                            } else {
                                "MaskA"
                            }),
                            tex2d,
                            if is_face { [0; 4] } else { [255, 0, 0, 0] },
                        ),
                        texture(
                            &param(if is_face {
                                "mml_tEmotionsMask1"
                            } else {
                                "MaskA2"
                            }),
                            tex2d,
                            [0; 4],
                        ),
                        texture(&param("mml_tTintMask"), tex2d, [0, 0, 0, 0]),
                        texture(
                            &param(if is_face {
                                "mml_tBrowStubble"
                            } else {
                                "mml_tEmissiveMask"
                            }),
                            tex2d,
                            [0; 4],
                        ),
                        texture(
                            &param(if is_face {
                                "mml_tBrowStubbleNormal"
                            } else {
                                "palette"
                            }),
                            tex2d,
                            if is_face {
                                [128; 4]
                            } else {
                                [128, 128, 128, 255]
                            },
                        ),
                        texture(
                            &param(if is_face { "mml_tTattooMask" } else { "normal" }),
                            tex2d,
                            if is_face { [0; 4] } else { [128; 4] },
                        ),
                    ]);
                    groups.len() - 1
                });
            let room = match item.room_id {
                Some(id) => {
                    let next = rooms.len() as u32 + 1;
                    match rooms.get(&id) {
                        Some(&i) => i,
                        None if next <= MAX_ROOMS => {
                            rooms.insert(id, next);
                            next
                        }
                        None => 0,
                    }
                }
                None => 0,
            };
            let mut falloff = crate::d3d::material_vector(m, "mml_vFalloffParams")
                .unwrap_or([0.0, 0.0, 1.5, 1.5]);
            if falloff[2].abs() < 1e-3 {
                falloff[2] = 1e-3;
            }
            let reflection =
                crate::d3d::material_vector(m, "mml_fSpecularReflectionMult").map_or(1.0, |v| v[0]);
            takes[k] = true;
            draws.push(PlanDraw {
                alpha_test: alpha.is_some(),
                kind: if is_terrain {
                    2
                } else if is_object {
                    0
                } else {
                    1
                },
                k,
                draw: d.draw,
                mesh: d.mesh,
                group,
                cull: cull_of(program.pass.cull),
                sphere: item.sphere,
                room: item.room_id,
                skin_slot: if item.skin.is_some() {
                    let i = skin_count;
                    skin_count += 1;
                    i
                } else {
                    0
                },
                // Textured cutouts use the independent forward-cutout caster plan;
                // never submit their full card through the opaque cube pipeline.
                casts: item.cast_shadow != Some(false) && alpha.is_none(),
            });
            data.push(DrawData {
                lightmap: item.lightmap.as_ref().map_or([0.0, 0.0, 1.0, 1.0], |l| {
                    [l.offset[0], l.offset[1], l.scale[0], l.scale[1]]
                }),
                falloff,
                room: [
                    room as f32,
                    reflection,
                    if is_terrain { 1.0 } else { 0.0 },
                    if program.name.starts_with("terrain3.") {
                        1.0
                    } else {
                        0.0
                    },
                ],
                terrain: crate::deferred_terrain::TerrainData::new(m),
                probe: item
                    .probe
                    .and_then(|i| scene.probes.get(i))
                    .map(|p| p.probe.rows())
                    .unwrap_or_else(|| {
                        thedas_formats::probe::IrradianceProbe::uniform(
                            crate::d3d::NO_PROBE_IRRADIANCE,
                        )
                        .rows()
                    }),
                tint: {
                    let values = m
                        .object
                        .vector_arrays
                        .iter()
                        .find(|(n, _)| n.eq_ignore_ascii_case(crate::scene::TINT_PARAMETER))
                        .map(|(_, v)| {
                            v.chunks(4)
                                .map(|v| {
                                    let mut row = [0.0; 4];
                                    for (d, s) in row.iter_mut().zip(v) {
                                        *d = *s;
                                    }
                                    row
                                })
                                .collect::<Vec<_>>()
                        })
                        .or_else(|| crate::d3d::default_vectors(m, crate::scene::TINT_PARAMETER))
                        .unwrap_or_default();
                    std::array::from_fn(|i| {
                        values
                            .get(i)
                            .copied()
                            .unwrap_or(if i < 2 { [0.0; 4] } else { [1.0; 4] })
                    })
                },
                material: [
                    if is_object {
                        if crate::d3d::LightClass::of(m.render_class.as_deref())
                            == crate::d3d::LightClass::Character
                        {
                            2.0
                        } else {
                            1.0
                        }
                    } else {
                        0.0
                    },
                    if program.name.contains("ch1em")
                        || program.name.contains("ch1alphem")
                        || program.name.starts_with("prop1.")
                    {
                        1.0
                    } else {
                        0.0
                    },
                    if program.name.contains("notnt") {
                        0.0
                    } else if program.name.contains("multnt") {
                        2.0
                    } else if program.name.contains("armtnt") {
                        3.0
                    } else {
                        1.0
                    },
                    if is_face {
                        1.0
                    } else if is_eye {
                        2.0
                    } else {
                        0.0
                    },
                ],
                tint_color: crate::d3d::material_vector(m, "mml_vTintColor").unwrap_or([0.0; 4]),
                character: crate::deferred_character::CharacterData::new(m),
                alpha: alpha.map_or([0.0; 4], |a| a.test),
            });
        }
        let (cutouts, cutout_materials) = crate::deferred_cutout::plan(
            ctx,
            vfs,
            scene,
            d3d,
            cache,
            &mut vertices,
            &mut skin_count,
        );
        let trees = trees::Plan::new(scene, d3d, &mut takes);
        let tree_shadows = tree_shadow::Plan::new(scene, d3d);
        if crate::config::CONFIG.log_materials {
            // Count planned faithful passes, including offscreen and inactive LODs.
            // Keep this at load time so even large foliage scenes pay once.
            let mut counts: HashMap<&str, [usize; 2]> = HashMap::new();
            for (k, d) in d3d.draws.iter().enumerate() {
                counts.entry(&d3d.program(d).name).or_default()[usize::from(takes[k])] += 1;
            }
            let mut counts: Vec<_> = counts.into_iter().collect();
            counts.sort_by(|a, b| b.1[0].cmp(&a.1[0]).then_with(|| a.0.cmp(b.0)));
            for (program, [forward, gbuffer]) in counts {
                log::info!(
                    "deferred-material forward={forward} gbuffer={gbuffer} program={program}"
                );
            }
            let mut particles: HashMap<&str, usize> = HashMap::new();
            for d in &d3d.particles {
                *particles.entry(&d3d.particle_program(d).name).or_default() += 1;
            }
            let mut particles: Vec<_> = particles.into_iter().collect();
            particles.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
            for (program, emitters) in particles {
                log::info!("deferred-particle emitters={emitters} program={program}");
            }
            let mut reasons: HashMap<(&str, &str), usize> = HashMap::new();
            for (_, d) in d3d.draws.iter().enumerate().filter(|(k, _)| !takes[*k]) {
                let program = d3d.program(d);
                let item = scene.draws.get(d.draw);
                let reason = if scene.cut_away.contains(&d.draw) {
                    "cutaway"
                } else if d.multipass {
                    "multipass"
                } else if d3d.is_blended(d) {
                    "blended"
                } else if program.instanced {
                    "instance-material"
                } else if item.is_some_and(|i| i.lod.is_some()) {
                    "lod-material"
                } else if static1(program) || terrain(program) || object_material(program) {
                    "geometry-or-control"
                } else {
                    "material"
                };
                *reasons.entry((&program.name, reason)).or_default() += 1;
            }
            let mut reasons: Vec<_> = reasons.into_iter().collect();
            reasons.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
            for ((program, reason), draws) in reasons {
                log::info!("deferred-reason draws={draws} reason={reason} program={program}");
            }
        }
        log::info!(
            "deferred: {} of {} draws ({} terrain, {} props, {} character materials, {} skinned, {} tree materials), planned in {:.1} ms ({} static draws)",
            draws.len() + trees.len(),
            d3d.draws.len(),
            data.iter().filter(|d| d.room[2] > 0.5).count(),
            data.iter().filter(|d| d.material[0] == 1.0).count(),
            data.iter().filter(|d| d.material[0] == 2.0).count(),
            draws.iter().filter(|d| d.skin_slot > 0).count(),
            trees.len(),
            started.elapsed().as_secs_f64() * 1000.0,
            scene.static_draws,
        );
        log::info!(
            "deferred: {} punch G-buffer draws",
            data.iter().filter(|d| d.alpha[0] > 0.5).count()
        );
        log::info!(
            "deferred: {} character cutout G-buffer draws",
            data.iter()
                .filter(|d| d.material[0] > 0.5 && d.alpha[0] > 0.5)
                .count()
        );
        (!draws.is_empty() || !cutouts.is_empty() || !trees.is_empty()).then(|| DeferredPass {
            draws,
            trees,
            tree_shadows,
            tree_shadows_enabled: None,
            cutouts,
            cutout_materials,
            data,
            groups,
            group_samplers,
            vertices,
            skin_count,
            takes,
            logged_materials: false,
            rooms,
            light_rooms: scene.light_rooms.clone(),
            gpu: None,
            effect_lights: Vec::new(),
            own_lights: Vec::new(),
            refused: false,
            baked_lighting: crate::config::CONFIG.baked_lighting,
            ssao: crate::config::CONFIG.ssao,
            culling: None,
            shadow_reuse: None,
            fused_reflections: None,
            tight_tiles: None,
            light_masks: None,
            opaque_gbuffer: None,
            specialized_gbuffer: None,
            early_light_reject: None,
            depth_lights: None,
            direct_terrain_layers: None,
        })
    }

    /// Whether faithful draw `k` (an index into [`D3dPass::draws`]) is this pass's to draw.
    pub(crate) fn takes(&self, k: usize) -> bool {
        self.takes.get(k).copied().unwrap_or(false)
    }

    pub(crate) fn set_shadow_reuse(&mut self, enabled: Option<bool>) {
        self.shadow_reuse = enabled;
    }

    pub(crate) fn set_fused_reflections(&mut self, enabled: Option<bool>) {
        self.fused_reflections = enabled;
    }

    pub(crate) fn set_tight_tiles(&mut self, enabled: Option<bool>) {
        self.tight_tiles = enabled;
    }

    pub(crate) fn set_light_masks(&mut self, enabled: Option<bool>) {
        self.light_masks = enabled;
    }

    pub(crate) fn set_opaque_gbuffer(&mut self, enabled: Option<bool>) {
        self.opaque_gbuffer = enabled;
    }

    pub(crate) fn set_specialized_gbuffer(&mut self, enabled: Option<bool>) {
        self.specialized_gbuffer = enabled;
    }

    pub(crate) fn set_early_light_reject(&mut self, enabled: Option<bool>) {
        self.early_light_reject = enabled;
    }

    pub(crate) fn set_depth_lights(&mut self, enabled: Option<bool>) {
        self.depth_lights = enabled;
    }

    pub(crate) fn set_direct_terrain_layers(&mut self, enabled: Option<bool>) {
        self.direct_terrain_layers = enabled;
    }

    pub(crate) fn set_tree_materials(&mut self, enabled: Option<bool>) {
        for family in trees::Family::NEW {
            self.trees.set_enabled(family, enabled);
        }
    }

    pub(crate) fn set_fronds(&mut self, enabled: Option<bool>) {
        self.trees.set_enabled(trees::Family::Fronds, enabled);
    }

    pub(crate) fn set_tree_shadows(&mut self, enabled: Option<bool>) {
        self.tree_shadows_enabled = enabled;
    }

    /// Sets the lights only this pass draws (see [`DeferredPass::effect_lights`]).
    pub(crate) fn set_effect_lights(&mut self, lights: Vec<SceneLight>) {
        // `THEDAS_DEFERRED_LOG_LIGHTS=1`: the first frame's effect lights, once (to frame
        // shots of them).
        static LOGGED: std::sync::Once = std::sync::Once::new();
        if !lights.is_empty() && crate::config::CONFIG.log_lights {
            LOGGED.call_once(|| {
                for l in &lights {
                    if let SceneLight::Point {
                        position,
                        color,
                        radius,
                        name,
                        ..
                    } = l
                    {
                        log::info!(
                            "effect light {name} at {position} colour {color} reach {radius}"
                        );
                    }
                }
            });
        }
        self.effect_lights = lights;
    }

    /// Sets the lights its own renderer's glowing emitters give this frame.
    pub(crate) fn set_own_lights(&mut self, lights: Vec<SceneLight>) {
        self.own_lights = lights;
    }

    pub(crate) fn set_culling(&mut self, culling: Option<[bool; 2]>) {
        self.culling = culling;
    }

    /// Whether the GPU refused the pass.
    pub(crate) fn refused(&self) -> bool {
        self.refused
    }

    pub(crate) fn ready(&self) -> bool {
        self.gpu.is_some() && !self.refused
    }

    pub(crate) fn material_mask(&self) -> Option<&wgpu::TextureView> {
        Some(&self.gpu.as_ref()?.targets.as_ref()?.views[0])
    }

    fn build(&self, ctx: &GpuContext, color_format: wgpu::TextureFormat) -> Gpu {
        let device = &ctx.device;
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("deferred"),
            source: wgpu::ShaderSource::Wgsl(DEFERRED_WGSL.into()),
        });
        let buffer_entry = |binding, visibility, ty| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Buffer {
                ty,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let uniform = wgpu::BufferBindingType::Uniform;
        let vf = wgpu::ShaderStages::VERTEX_FRAGMENT;
        let f = wgpu::ShaderStages::FRAGMENT;
        let scene_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("deferred scene"),
            entries: &[
                buffer_entry(0, vf, uniform),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: vf,
                    ty: wgpu::BindingType::Buffer {
                        ty: uniform,
                        has_dynamic_offset: true,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: uniform,
                        has_dynamic_offset: true,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let tex = |binding, filterable| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: f,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let smp = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: f,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        };
        let material_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("deferred material"),
            entries: &[
                tex(0, true),
                tex(1, true),
                tex(2, true),
                smp(3),
                tex(4, true),
                smp(5),
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: f,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::Cube,
                        multisampled: false,
                    },
                    count: None,
                },
                tex(7, true),
                tex(8, true),
                tex(9, true),
                smp(10),
                tex(11, true),
                tex(12, true),
                tex(13, true),
                tex(14, true),
                tex(15, true),
            ],
        });
        let lighting_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("deferred lighting"),
            entries: &[
                buffer_entry(0, f, uniform),
                buffer_entry(1, f, uniform),
                buffer_entry(2, f, uniform),
                buffer_entry(3, f, uniform),
                buffer_entry(4, f, uniform),
            ],
        });
        let targets_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("deferred g-buffer"),
            entries: &[
                tex(0, false),
                tex(1, false),
                tex(2, false),
                tex(3, false),
                tex(4, false),
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: f,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let sun_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("deferred sun visibility"),
            entries: &[
                buffer_entry(0, f, uniform),
                tex(1, true),
                tex(2, true),
                smp(3),
            ],
        });
        let gbuffer_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("deferred g-buffer"),
            bind_group_layouts: &[
                Some(&scene_layout),
                Some(&material_layout),
                Some(&sun_layout),
            ],
            immediate_size: 0,
        });
        let env_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("deferred reflection"),
            bind_group_layouts: &[
                Some(&scene_layout),
                Some(&material_layout),
                Some(&sun_layout),
            ],
            immediate_size: 0,
        });
        let gbuffer_targets: Vec<Option<wgpu::ColorTargetState>> = GBUFFER
            .iter()
            .map(|&format| {
                Some(wgpu::ColorTargetState {
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })
            })
            .collect();
        let make_gbuffer = |entry_point, rigid| {
            [None, Some(wgpu::Face::Back), Some(wgpu::Face::Front)].map(|cull| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("deferred g-buffer"),
                layout: Some(&gbuffer_layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some(if rigid { "vs_gbuffer_rigid" } else { "vs_gbuffer" }),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<DeferredVertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![
                            0 => Float32x3, 1 => Float32x3, 2 => Float32x2, 3 => Float32x2, 4 => Float32x4, 5 => Float32x4
                        ],
                    })],
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: cull,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::LessEqual),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(entry_point),
                    compilation_options: Default::default(),
                    targets: &gbuffer_targets,
                }),
                multiview_mask: None,
                cache: None,
            })
        })
        };
        let gbuffer = make_gbuffer("fs_gbuffer", false);
        let gbuffer_opaque = make_gbuffer("fs_gbuffer_opaque", false);
        let gbuffer_static = self.draws.iter().any(|d| d.kind == 1).then(|| {
            [
                make_gbuffer("fs_gbuffer_static", true),
                make_gbuffer("fs_gbuffer_static_opaque", true),
            ]
        });
        let gbuffer_terrain = self
            .draws
            .iter()
            .any(|d| d.kind == 2)
            .then(|| make_gbuffer("fs_gbuffer_terrain", true));
        let env = [None, Some(wgpu::Face::Back), Some(wgpu::Face::Front)].map(|cull| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("deferred reflection"),
                layout: Some(&env_layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs_gbuffer"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<DeferredVertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![
                            0 => Float32x3, 1 => Float32x3, 2 => Float32x2, 3 => Float32x2, 4 => Float32x4, 5 => Float32x4
                        ],
                    })],
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: cull,
                    ..Default::default()
                },
                // The same vertex shader writes the same depth: only the nearest surface
                // passes.
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(false),
                    depth_compare: Some(wgpu::CompareFunction::LessEqual),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some("fs_env"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: color_format,
                        blend: Some(wgpu::BlendState {
                            color: wgpu::BlendComponent {
                                src_factor: wgpu::BlendFactor::One,
                                dst_factor: wgpu::BlendFactor::One,
                                operation: wgpu::BlendOperation::Add,
                            },
                            alpha: wgpu::BlendComponent::REPLACE,
                        }),
                        write_mask: wgpu::ColorWrites::COLOR,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        });
        let lighting_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("deferred lighting"),
                bind_group_layouts: &[
                    Some(&lighting_layout),
                    Some(&targets_layout),
                    Some(&sun_layout),
                ],
                immediate_size: 0,
            });
        let lighting = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("deferred lighting"),
            layout: Some(&lighting_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_fullscreen"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_lighting"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: color_format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        let frame_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("deferred frame"),
            size: std::mem::size_of::<FrameUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let draw_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("deferred draws"),
            size: (DRAW_STRIDE * self.draws.len().max(1)) as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let skin_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("deferred skin palettes"),
            size: (self.skin_count * SKIN_STRIDE) as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let scene_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("deferred scene"),
            layout: &scene_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: frame_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &draw_buffer,
                        offset: 0,
                        size: wgpu::BufferSize::new(std::mem::size_of::<DrawUniforms>() as u64),
                    }),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &skin_buffer,
                        offset: 0,
                        size: wgpu::BufferSize::new((ROWS * 16) as u64),
                    }),
                },
            ],
        });
        let sampler = |label, address_mode, anisotropy_clamp| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some(label),
                address_mode_u: address_mode,
                address_mode_v: address_mode,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::MipmapFilterMode::Linear,
                anisotropy_clamp,
                ..Default::default()
            })
        };
        let wrap = sampler("deferred wrap", wgpu::AddressMode::Repeat, 8);
        let clamp = sampler("deferred lightmap", wgpu::AddressMode::ClampToEdge, 1);
        let point = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("deferred terrain layer indices"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });
        let material_groups = self
            .groups
            .iter()
            .zip(&self.group_samplers)
            .map(
                |([d, n, s, l, e, mv, ma, ma2, tint, emissive, td, tn], sampler)| {
                    device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("deferred material"),
                        layout: &material_layout,
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: wgpu::BindingResource::TextureView(d),
                            },
                            wgpu::BindGroupEntry {
                                binding: 1,
                                resource: wgpu::BindingResource::TextureView(n),
                            },
                            wgpu::BindGroupEntry {
                                binding: 2,
                                resource: wgpu::BindingResource::TextureView(s),
                            },
                            wgpu::BindGroupEntry {
                                binding: 3,
                                resource: wgpu::BindingResource::Sampler(
                                    sampler.as_ref().unwrap_or(&wrap),
                                ),
                            },
                            wgpu::BindGroupEntry {
                                binding: 4,
                                resource: wgpu::BindingResource::TextureView(l),
                            },
                            wgpu::BindGroupEntry {
                                binding: 5,
                                resource: wgpu::BindingResource::Sampler(&clamp),
                            },
                            wgpu::BindGroupEntry {
                                binding: 6,
                                resource: wgpu::BindingResource::TextureView(e),
                            },
                            wgpu::BindGroupEntry {
                                binding: 7,
                                resource: wgpu::BindingResource::TextureView(mv),
                            },
                            wgpu::BindGroupEntry {
                                binding: 8,
                                resource: wgpu::BindingResource::TextureView(ma),
                            },
                            wgpu::BindGroupEntry {
                                binding: 9,
                                resource: wgpu::BindingResource::TextureView(ma2),
                            },
                            wgpu::BindGroupEntry {
                                binding: 10,
                                resource: wgpu::BindingResource::Sampler(&point),
                            },
                            // Separate bindings keep GL's one-sampler-per-image restriction:
                            // static textures wrap; terrain palette textures clamp.
                            wgpu::BindGroupEntry {
                                binding: 11,
                                resource: wgpu::BindingResource::TextureView(td),
                            },
                            wgpu::BindGroupEntry {
                                binding: 12,
                                resource: wgpu::BindingResource::TextureView(tn),
                            },
                            wgpu::BindGroupEntry {
                                binding: 13,
                                resource: wgpu::BindingResource::TextureView(tint),
                            },
                            wgpu::BindGroupEntry {
                                binding: 14,
                                resource: wgpu::BindingResource::TextureView(emissive),
                            },
                            // Face age diffuse needs linear sampling; terrain's MaskV uses
                            // a separate image binding with nearest sampling for portable GL.
                            wgpu::BindGroupEntry {
                                binding: 15,
                                resource: wgpu::BindingResource::TextureView(mv),
                            },
                        ],
                    })
                },
            )
            .collect();
        let lighting_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("deferred lighting"),
            size: std::mem::size_of::<LightingUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let light_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("deferred lights"),
            size: (std::mem::size_of::<GpuLight>() * MAX_LIGHTS) as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let faces_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("deferred shadow faces"),
            size: (64 * 6 * crate::deferred_shadow::SLOTS) as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let tile_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("deferred light tiles"),
            size: std::mem::size_of::<TileUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mask_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("deferred light membership"),
            size: std::mem::size_of::<light_masks::Masks>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let lighting_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("deferred lighting"),
            layout: &lighting_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: lighting_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: light_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: faces_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: tile_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: mask_buffer.as_entire_binding(),
                },
            ],
        });
        let mut order: Vec<usize> = (0..self.draws.len()).collect();
        order.sort_by_key(|&i| {
            let d = &self.draws[i];
            (cull_index(d.cull), d.group, d.mesh)
        });
        Gpu {
            trees: self.trees.build(device, &frame_buffer),
            tree_shadows: self.tree_shadows.build(device),
            gbuffer,
            gbuffer_opaque,
            gbuffer_static,
            gbuffer_terrain,
            env,
            lighting,
            mask_buffer,
            light_masks: None,
            frame_buffer,
            draw_buffer,
            scene_group,
            vertices: self
                .vertices
                .iter()
                .map(|(&i, vertices)| {
                    use wgpu::util::DeviceExt;
                    (
                        i,
                        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some("deferred vertices"),
                            contents: bytemuck::cast_slice(vertices),
                            usage: wgpu::BufferUsages::VERTEX,
                        }),
                    )
                })
                .collect(),
            skin_buffer,
            skin_slots: Vec::new(),
            slots: Vec::new(),
            material_groups,
            lighting_buffer,
            light_buffer,
            tile_buffer,
            tiles: None,
            shadows: None,
            sun_fallback: device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some("deferred unused sun partition"),
                    size: wgpu::Extent3d {
                        width: 1,
                        height: 1,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: crate::d3d_gpu::SHADOW_FORMAT,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&Default::default()),
            sun_sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("deferred sun partitions"),
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }),
            sun_buffer: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("deferred sun visibility"),
                size: std::mem::size_of::<SunUniforms>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            sun_layout,
            sun_group: None,
            faces_buffer,
            lighting_group,
            targets_layout,
            targets: None,
            order,
            lights: Vec::new(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn prepare(
        &mut self,
        ctx: &GpuContext,
        color_format: wgpu::TextureFormat,
        (width, height): (u32, u32),
        camera: &Camera,
        d3d: &D3dPass,
        rooms: &HashMap<i64, u32>,
        fused_reflections: bool,
        direct_terrain_layers: bool,
    ) -> bool {
        if self.refused {
            return false;
        }
        self.trees.prepare(d3d, &mut self.takes);
        if crate::config::CONFIG.log_materials && !self.logged_materials {
            self.logged_materials = true;
            let mut counts: HashMap<&str, [usize; 2]> = HashMap::new();
            for (k, d) in d3d
                .draws
                .iter()
                .enumerate()
                .filter(|(k, _)| d3d.in_view(*k))
            {
                let taken = self.takes[k] && !d3d.is_faded(d);
                counts.entry(&d3d.program(d).name).or_default()[usize::from(taken)] += 1;
            }
            let mut counts: Vec<_> = counts.into_iter().collect();
            counts.sort_by(|a, b| b.1[0].cmp(&a.1[0]).then_with(|| a.0.cmp(b.0)));
            for (program, [forward, gbuffer]) in counts {
                log::info!(
                    "deferred-visible forward={forward} gbuffer={gbuffer} program={program}"
                );
            }
        }
        if self.gpu.is_none() {
            self.gpu = checked(&ctx.device, "the deferred renderer", || {
                self.build(ctx, color_format)
            });
            if self.gpu.is_none() {
                self.refused = true;
                return false;
            }
        }
        let gpu = self.gpu.as_mut().expect("prepared GPU");
        let aspect = width as f32 / height.max(1) as f32;
        let (view, projection) = (camera.view(), camera.projection(aspect));
        // One write of every slot (many small writes are slow), when anything changed.
        let mut slots = vec![0u8; DRAW_STRIDE * self.draws.len()];
        for ((d, data), slot) in self
            .draws
            .iter()
            .zip(&self.data)
            .zip(slots.chunks_exact_mut(DRAW_STRIDE))
        {
            let world = d3d.world(d.draw).unwrap_or(Mat4::IDENTITY);
            let world_view = view * world;
            let mut data = *data;
            data.room[0] = d.room.and_then(|r| rooms.get(&r)).copied().unwrap_or(0) as f32;
            if data.material[0] == 1.0 {
                let ambient = crate::d3d::ambient_of(d3d.lights(), crate::d3d::LightClass::Prop);
                for c in 0..3 {
                    data.probe[c * 4 + 3][3] += ambient[c];
                }
            }
            let u = DrawUniforms {
                mvp: (projection * world_view).to_cols_array_2d(),
                world: world.to_cols_array_2d(),
                data,
            };
            slot[..std::mem::size_of::<DrawUniforms>()].copy_from_slice(bytemuck::bytes_of(&u));
        }
        if slots != gpu.slots {
            ctx.queue.write_buffer(&gpu.draw_buffer, 0, &slots);
            gpu.slots = slots;
        }
        let fog = d3d.fog();
        let frame = FrameUniforms {
            eye: camera.position.extend(1.0).to_array(),
            fixup: [
                1.0 / width.max(1) as f32,
                1.0 / height.max(1) as f32,
                if fused_reflections { 1.0 } else { 0.0 },
                if direct_terrain_layers { 1.0 } else { 0.0 },
            ],
            fog: [
                fog[0][3],
                fog[1][0],
                if debug_view() == 8 { 1.0 } else { 0.0 },
                if self.baked_lighting { 0.0 } else { 1.0 },
            ],
        };
        ctx.queue
            .write_buffer(&gpu.frame_buffer, 0, bytemuck::bytes_of(&frame));
        let mut skins = vec![0u8; self.skin_count * SKIN_STRIDE];
        crate::deferred_mesh::write_palette(&mut skins[..SKIN_STRIDE], &[]);
        for d in &self.draws {
            if d.skin_slot > 0 {
                let start = d.skin_slot * SKIN_STRIDE;
                crate::deferred_mesh::write_palette(
                    &mut skins[start..start + SKIN_STRIDE],
                    d3d.bones(d.draw),
                );
            }
        }
        for d in &self.cutouts {
            if d.skin_slot > 0 {
                let start = d.skin_slot * SKIN_STRIDE;
                crate::deferred_mesh::write_palette(
                    &mut skins[start..start + SKIN_STRIDE],
                    d3d.bones(d.draw),
                );
            }
        }
        if skins != gpu.skin_slots {
            ctx.queue.write_buffer(&gpu.skin_buffer, 0, &skins);
            gpu.skin_slots = skins;
        }
        true
    }

    /// Records the G-buffer and lighting passes for this frame into `encoder`: the lit
    /// deferred draws into `target` (cleared to `clear` first) and their depth into
    /// `depth` (cleared first), for the faithful pass to draw the rest over. Returns false,
    /// recording nothing, when the GPU refuses the pass: the frame is then drawn faithfully.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn encode(
        &mut self,
        ctx: &GpuContext,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        color_format: wgpu::TextureFormat,
        depth: &wgpu::TextureView,
        (width, height): (u32, u32),
        camera: &Camera,
        d3d: &D3dPass,
        meshes: &[GpuMesh],
        clear: wgpu::Color,
        contributors: &mut [Contributor<'_>],
        mut profile: Option<&mut crate::profile::CaptureProfile>,
    ) -> bool {
        // AO needs the completed depth buffer and independently scales probe bounce.
        // Keep separate reflection terms in that mode and in diagnostic views.
        let fused_reflections = self
            .fused_reflections
            .unwrap_or(crate::config::CONFIG.fused_reflections)
            && self.baked_lighting
            && contributors.iter().all(|c| c.pass.baked_lighting)
            && debug_view() == 0;
        let direct_terrain_layers = self
            .direct_terrain_layers
            .unwrap_or(crate::config::CONFIG.direct_terrain_layers);
        for c in contributors.iter() {
            for &room in c.pass.rooms.keys() {
                let next = self.rooms.len() as u32 + 1;
                if next <= MAX_ROOMS {
                    self.rooms.entry(room).or_insert(next);
                }
            }
        }
        if !self.prepare(
            ctx,
            color_format,
            (width, height),
            camera,
            d3d,
            &self.rooms.clone(),
            fused_reflections,
            direct_terrain_layers,
        ) {
            return false;
        }
        for c in contributors.iter_mut() {
            c.pass.prepare(
                ctx,
                color_format,
                (width, height),
                camera,
                c.d3d,
                &self.rooms,
                fused_reflections,
                direct_terrain_layers,
            );
        }
        let characters = self.data.iter().any(|d| d.material[0] > 1.5)
            || contributors
                .iter()
                .any(|c| c.pass.data.iter().any(|d| d.material[0] > 1.5));
        if self.gpu.as_ref().is_some_and(|g| g.shadows.is_none()) {
            let pool = checked(&ctx.device, "deferred point shadows", || {
                crate::deferred_shadow::ShadowPool::new(&ctx.device)
            });
            if pool.is_none() {
                self.refused = true;
                self.gpu = None;
                return false;
            }
            self.gpu.as_mut().expect("prepared GPU").shadows = pool;
        }
        let mut lights = self.lights(camera, width, height, d3d, characters);
        if !self.baked_lighting {
            // Without baked alpha every point light uses its own runtime visibility.
            // The sun alone keeps the directional visibility term in the lighting pass.
            for light in &mut lights {
                light.colour[3] = 0.0;
            }
        }
        // Keep a capture/benchmark switch for comparing only the shadow contribution.
        let shadowed = if !crate::config::CONFIG.point_shadows {
            Vec::new()
        } else {
            shadow_slots(&mut lights, camera.position)
        };
        if crate::config::CONFIG.log_lights {
            static ONCE: std::sync::Once = std::sync::Once::new();
            ONCE.call_once(|| {
                log::info!(
                    "deferred frame: {} lights in view ({} from emitters here, {} given), shadowed {:?}",
                    lights.len(),
                    self.own_lights.len(),
                    self.effect_lights.len(),
                    shadowed
                );
            });
        }
        let Some(gpu) = self.gpu.as_mut() else {
            return false;
        };
        let device = &ctx.device;
        let stale = !gpu
            .targets
            .as_ref()
            .is_some_and(|t| t.size == (width, height) && t.depth == *depth);
        if stale {
            let views = GBUFFER.map(|format| {
                device
                    .create_texture(&wgpu::TextureDescriptor {
                        label: Some("deferred g-buffer"),
                        size: wgpu::Extent3d {
                            width,
                            height,
                            depth_or_array_layers: 1,
                        },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format,
                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                            | wgpu::TextureUsages::TEXTURE_BINDING,
                        view_formats: &[],
                    })
                    .create_view(&wgpu::TextureViewDescriptor::default())
            });
            // Targets 0-3 at bindings 0-3, the depth at 4, the shadow maps at 5.
            let sun_views = d3d.sun_shadows().map(|(_, views)| views);
            let entries: Vec<wgpu::BindGroupEntry> = [
                &views[0],
                &views[1],
                &views[2],
                &views[3],
                depth,
                &gpu.shadows.as_ref().expect("point shadows").view,
            ]
            .into_iter()
            .enumerate()
            .map(|(i, v)| wgpu::BindGroupEntry {
                binding: i as u32,
                resource: wgpu::BindingResource::TextureView(v),
            })
            .collect();
            gpu.sun_group = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("deferred sun visibility"),
                layout: &gpu.sun_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: gpu.sun_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(
                            sun_views.map_or(&gpu.sun_fallback, |v| &v[0]),
                        ),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(
                            sun_views.map_or(&gpu.sun_fallback, |v| &v[1]),
                        ),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::Sampler(&gpu.sun_sampler),
                    },
                ],
            }));
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("deferred g-buffer"),
                layout: &gpu.targets_layout,
                entries: &entries,
            });
            gpu.targets = Some(Targets {
                size: (width, height),
                views,
                group,
                depth: depth.clone(),
            });
        }

        let aspect = width as f32 / height.max(1) as f32;
        let (view, projection) = (camera.view(), camera.projection(aspect));
        let fog = d3d.fog();
        if lights != gpu.lights {
            let masks = light_masks::build(&lights);
            if gpu.light_masks.as_ref() != Some(&masks) {
                ctx.queue
                    .write_buffer(&gpu.mask_buffer, 0, bytemuck::bytes_of(&masks));
                gpu.light_masks = Some(masks);
            }
            if !lights.is_empty() {
                ctx.queue
                    .write_buffer(&gpu.light_buffer, 0, bytemuck::cast_slice(&lights));
            }
            gpu.lights = lights;
        }
        let tight = self
            .tight_tiles
            .unwrap_or(crate::config::CONFIG.tight_tiles);
        let tiles = crate::deferred_tiles::build(
            projection * view,
            (width, height),
            gpu.lights.iter().map(|l| l.position),
            if debug_view() == 9 { 2.0 } else { 1.0 },
            self.culling
                .map(|c| c[0])
                .unwrap_or(crate::config::CONFIG.tiled_lights),
            tight,
        );
        let depth_plane = -view.row(2);
        let tile_uniforms = TileUniforms {
            screen: tiles,
            depth: crate::deferred_tiles::depth_masks(
                depth_plane,
                gpu.lights.iter().map(|l| l.position),
                if debug_view() == 9 { 2.0 } else { 1.0 },
                true,
            ),
        };
        if gpu.tiles.as_ref() != Some(&tile_uniforms) {
            ctx.queue
                .write_buffer(&gpu.tile_buffer, 0, bytemuck::bytes_of(&tile_uniforms));
            gpu.tiles = Some(tile_uniforms);
        }
        if crate::config::CONFIG.log_lights {
            static ONCE: [std::sync::Once; 2] = [std::sync::Once::new(), std::sync::Once::new()];
            ONCE[usize::from(tight)].call_once(|| {
                let counts: Vec<_> = tiles
                    .iter()
                    .map(|t| t.iter().map(|v| v.count_ones()).sum::<u32>())
                    .collect();
                log::info!(
                    "deferred tiles (frusta={tight}): {} lights, {:.1} average / {} max candidates per tile",
                    gpu.lights.len(),
                    counts.iter().sum::<u32>() as f32 / counts.len() as f32,
                    counts.iter().max().copied().unwrap_or(0)
                );
            });
        }
        let (sun_dir, sun_colour) = match d3d.sun() {
            Some((dir, colour)) => (dir.normalize_or_zero().extend(1.0), colour.extend(0.0)),
            None => (Vec4::ZERO, Vec4::ZERO),
        };
        let lighting = LightingUniforms {
            inv_view_proj: (projection * view).inverse().to_cols_array_2d(),
            view_proj: (projection * view).to_cols_array_2d(),
            eye: camera.position.extend(1.0).to_array(),
            sun_dir: sun_dir.to_array(),
            sun_colour: sun_colour.to_array(),
            fog0: fog[0],
            fog1: fog[1],
            counts: [
                gpu.lights.len() as u32,
                debug_view(),
                u32::from(
                    self.light_masks
                        .unwrap_or(crate::config::CONFIG.light_masks),
                ),
                if self.baked_lighting {
                    0
                } else if self.ssao {
                    2
                } else {
                    1
                },
            ],
            depth_plane: depth_plane.to_array(),
            size: [
                width as f32,
                height as f32,
                if fused_reflections { 1.0 } else { 0.0 },
                (u32::from(
                    self.early_light_reject
                        .unwrap_or(crate::config::CONFIG.early_light_reject),
                ) | (u32::from(
                    self.depth_lights
                        .unwrap_or(crate::config::CONFIG.depth_lights),
                ) << 1)) as f32,
            ],
        };
        let sun = SunUniforms {
            data: d3d
                .sun_shadows()
                .map_or(crate::d3d::NO_SHADOW_DATA, |(data, _)| data),
            direction: sun_dir.to_array(),
            enabled: [
                u32::from(d3d.sun_shadows().is_some() && crate::config::CONFIG.sun_shadows),
                0,
                0,
                0,
            ],
        };
        ctx.queue
            .write_buffer(&gpu.sun_buffer, 0, bytemuck::bytes_of(&sun));
        ctx.queue
            .write_buffer(&gpu.lighting_buffer, 0, bytemuck::bytes_of(&lighting));
        if let Some(p) = profile.as_deref_mut() {
            p.mark("deferred_prepare");
        }

        // The shadowed lights' maps, before anything reads them.
        let mut faces = Vec::with_capacity(64 * 6 * shadowed.len());
        for &(p, r) in &shadowed {
            for m in crate::deferred_shadow::faces(p, r) {
                faces.extend_from_slice(bytemuck::bytes_of(&m.to_cols_array_2d()));
            }
        }
        if !faces.is_empty() {
            ctx.queue.write_buffer(&gpu.faces_buffer, 0, &faces);
            let mut trees = Vec::new();
            if self
                .tree_shadows_enabled
                .unwrap_or(crate::config::CONFIG.tree_shadows)
            {
                trees = self.tree_shadows.casters(
                    &mut gpu.tree_shadows,
                    ctx,
                    d3d,
                    meshes,
                    camera,
                    &shadowed,
                );
                for c in contributors.iter_mut() {
                    if let Some(g) = c.pass.gpu.as_mut() {
                        trees.extend(c.pass.tree_shadows.casters(
                            &mut g.tree_shadows,
                            ctx,
                            c.d3d,
                            c.meshes,
                            camera,
                            &shadowed,
                        ));
                    }
                }
            }
            let mut casters = casters(
                &self.draws,
                &self.cutouts,
                &self.cutout_materials,
                d3d,
                meshes,
                (&gpu.vertices, &gpu.skin_buffer),
            );
            for c in contributors.iter() {
                if let Some(g) = c.pass.gpu.as_ref() {
                    casters.extend(casters_for_contributor(c, g));
                }
            }
            let pool = gpu.shadows.as_mut().expect("point shadows");
            pool.set_culling(self.culling.map(|c| c[1]));
            pool.reuse_override = self.shadow_reuse;
            pool.encode(device, &ctx.queue, encoder, &shadowed, &casters, &trees);
        }
        if let Some(p) = profile.as_deref_mut() {
            p.stage(ctx, encoder, "point_shadows");
        }

        let Some(targets) = gpu.targets.as_ref() else {
            return false;
        };
        {
            let attachments: Vec<Option<wgpu::RenderPassColorAttachment>> = targets
                .views
                .iter()
                .map(|v| {
                    Some(wgpu::RenderPassColorAttachment {
                        view: v,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    })
                })
                .collect();
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("deferred g-buffer"),
                color_attachments: &attachments,
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: profile.as_deref_mut().and_then(|p| p.pass("gbuffer")),
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(2, gpu.sun_group.as_ref().expect("sun targets"), &[]);
            let opaque = self
                .opaque_gbuffer
                .unwrap_or(crate::config::CONFIG.opaque_gbuffer);
            let specialized = self
                .specialized_gbuffer
                .unwrap_or(crate::config::CONFIG.specialized_gbuffer)
                // Separate reflection redraws use the general vertex entry point.
                // Retain identical clip/depth evaluation across those two passes.
                && fused_reflections;
            draw_all(
                &self.draws,
                &mut pass,
                gpu,
                &gpu.gbuffer,
                opaque.then_some(&gpu.gbuffer_opaque),
                specialized,
                d3d,
                meshes,
            );
            for c in contributors.iter() {
                if let Some(g) = c.pass.gpu.as_ref() {
                    draw_all(
                        &c.pass.draws,
                        &mut pass,
                        g,
                        &g.gbuffer,
                        opaque.then_some(&g.gbuffer_opaque),
                        specialized,
                        c.d3d,
                        c.meshes,
                    );
                }
            }
            self.trees.draw(&gpu.trees, &mut pass, d3d, meshes);
            for c in contributors.iter() {
                if let Some(g) = c.pass.gpu.as_ref() {
                    c.pass.trees.draw(&g.trees, &mut pass, c.d3d, c.meshes);
                }
            }
        }
        if let Some(p) = profile.as_deref_mut() {
            p.stage(ctx, encoder, "gbuffer");
        }
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("deferred lighting"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(clear),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: profile.as_deref_mut().and_then(|p| p.pass("lighting")),
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&gpu.lighting);
            pass.set_bind_group(0, &gpu.lighting_group, &[]);
            pass.set_bind_group(1, &targets.group, &[]);
            pass.set_bind_group(2, gpu.sun_group.as_ref().expect("sun targets"), &[]);
            pass.draw(0..3, 0..1);
        }
        if let Some(p) = profile.as_deref_mut() {
            p.stage(ctx, encoder, "lighting");
        }
        if fused_reflections {
            if let Some(p) = profile {
                p.mark("reflection");
            }
            return true;
        }
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("deferred reflection"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: profile.as_deref_mut().and_then(|p| p.pass("reflection")),
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_bind_group(2, gpu.sun_group.as_ref().expect("sun targets"), &[]);
        draw_all(
            &self.draws,
            &mut pass,
            gpu,
            &gpu.env,
            None,
            false,
            d3d,
            meshes,
        );
        for c in contributors.iter() {
            if let Some(g) = c.pass.gpu.as_ref() {
                draw_all(
                    &c.pass.draws,
                    &mut pass,
                    g,
                    &g.env,
                    None,
                    false,
                    c.d3d,
                    c.meshes,
                );
            }
        }
        drop(pass);
        if let Some(p) = profile {
            p.stage(ctx, encoder, "reflection");
        }
        true
    }

    /// The lights for a frame seen by `camera`: every point light of the scene's (and those
    /// added since) that lights the level and reaches into the view, nearest first past
    /// [`MAX_LIGHTS`].
    fn lights(
        &self,
        camera: &Camera,
        width: u32,
        height: u32,
        d3d: &D3dPass,
        characters: bool,
    ) -> Vec<GpuLight> {
        let planes = frustum(camera.view_proj(width as f32 / height.max(1) as f32));
        let mut out: Vec<(f32, GpuLight)> = d3d
            .lights()
            .iter()
            .chain(&self.effect_lights)
            .chain(&self.own_lights)
            .filter_map(|l| self.light(l, &planes, camera.position, characters))
            .collect();
        if out.len() > MAX_LIGHTS {
            out.sort_by(|a, b| a.0.total_cmp(&b.0));
            out.truncate(MAX_LIGHTS);
        }
        out.into_iter().map(|(_, l)| l).collect()
    }

    fn light(
        &self,
        l: &SceneLight,
        planes: &[Vec4; 6],
        eye: Vec3,
        characters: bool,
    ) -> Option<(f32, GpuLight)> {
        let SceneLight::Point {
            position,
            color,
            radius,
            domain,
            is_static,
            intensity,
            room,
            name,
            ..
        } = l
        else {
            return None;
        };
        let colour = *color * *intensity;
        let visible = planes
            .iter()
            .all(|p| p.truncate().dot(*position) + p.w >= -*radius);
        // As the faithful pass picks the level's lights (`d3d::pick_level_lights`): an affect
        // domain, read as a mask, that leaves the level out excludes the light.
        let character = *domain == Some(DOMAIN_CHARACTERS);
        if (domain.is_some_and(|m| m & DOMAIN_LEVEL == 0) && !(characters && character))
            || radius.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater)
            || colour.max_element() <= 0.0
            || !visible
        {
            return None;
        }
        Some((
            position.distance(eye) - radius,
            GpuLight {
                position: position.extend(*radius).to_array(),
                colour: colour.extend(if *is_static { 1.0 } else { 0.0 }).to_array(),
                shadow: [
                    -1.0,
                    if name.starts_with("emitter ") {
                        3.0
                    } else if character {
                        2.0
                    } else {
                        1.0
                    },
                    0.0,
                    0.0,
                ],
                // A light of no room (an object's, an emitter's) reaches every room: placed by
                // the rooms' boxes it missed the floors around Ishal's torches, whose boxes
                // overlap; the nearest of them get shadow maps instead.
                rooms: self.room_bits(*room),
            },
        ))
    }

    /// The room set of a light placed in `room`: that room and those its light reaches
    /// ([`AreaScene::light_rooms`]); every room for a light of no room.
    fn room_bits(&self, room: Option<i64>) -> [u32; 8] {
        let Some(room) = room else {
            return [u32::MAX; 8];
        };
        let mut bits = [0u32; 8];
        let reach = self.light_rooms.get(&room).into_iter().flatten();
        for r in std::iter::once(&room).chain(reach) {
            if let Some(&i) = self.rooms.get(r) {
                bits[(i / 32) as usize] |= 1 << (i % 32);
            }
        }
        // Rooms none of the deferred draws are in: no test, as for a light of no room.
        if bits == [0; 8] { [u32::MAX; 8] } else { bits }
    }
}

fn casters_for_contributor<'a>(
    c: &'a Contributor<'_>,
    gpu: &'a Gpu,
) -> Vec<crate::deferred_shadow::Caster<'a>> {
    casters(
        &c.pass.draws,
        &c.pass.cutouts,
        &c.pass.cutout_materials,
        c.d3d,
        c.meshes,
        (&gpu.vertices, &gpu.skin_buffer),
    )
}

fn casters<'a>(
    draws: &'a [PlanDraw],
    cutouts: &'a [crate::deferred_cutout::Draw],
    materials: &'a [crate::deferred_cutout::Material],
    d3d: &'a D3dPass,
    meshes: &'a [GpuMesh],
    streams: (&'a HashMap<usize, wgpu::Buffer>, &'a wgpu::Buffer),
) -> Vec<crate::deferred_shadow::Caster<'a>> {
    let (vertices, bones) = streams;
    let mut result: Vec<_> = draws
        .iter()
        .filter(|d| d.casts && !d3d.is_faded(&d3d.draws[d.k]))
        .filter_map(|d| {
            Some(crate::deferred_shadow::Caster {
                mesh: meshes.get(d.mesh)?,
                vertices: vertices.get(&d.mesh)?,
                bones,
                bone_offset: (d.skin_slot * SKIN_STRIDE) as u32,
                pose: if d.skin_slot > 0 {
                    d3d.bones(d.draw)
                } else {
                    &[]
                },
                world: d3d.world(d.draw).unwrap_or(Mat4::IDENTITY),
                sphere: d3d.draw_sphere(&d3d.draws[d.k]).unwrap_or(d.sphere),
                alpha: [0.0; 4],
                clip: 0.0,
                material: None,
            })
        })
        .collect();
    result.extend(
        cutouts
            .iter()
            .filter(|d| !d3d.is_faded(&d3d.draws[d.k]))
            .filter_map(|d| {
                Some(crate::deferred_shadow::Caster {
                    mesh: meshes.get(d.mesh)?,
                    vertices: vertices.get(&d.mesh)?,
                    bones,
                    bone_offset: (d.skin_slot * SKIN_STRIDE) as u32,
                    pose: if d.skin_slot > 0 {
                        d3d.bones(d.draw)
                    } else {
                        &[]
                    },
                    world: d3d.world(d.draw).unwrap_or(Mat4::IDENTITY),
                    sphere: d3d.draw_sphere(&d3d.draws[d.k]).unwrap_or(d.sphere),
                    alpha: d.alpha,
                    clip: d.clip,
                    material: materials.get(d.material),
                })
            }),
    );
    result
}

/// Draws the deferred draws in view with `pipelines` (by cull mode).
#[allow(clippy::too_many_arguments)]
fn draw_all(
    draws: &[PlanDraw],
    pass: &mut wgpu::RenderPass<'_>,
    gpu: &Gpu,
    pipelines: &[wgpu::RenderPipeline; 3],
    opaque_pipelines: Option<&[wgpu::RenderPipeline; 3]>,
    specialized: bool,
    d3d: &D3dPass,
    meshes: &[GpuMesh],
) {
    let (mut pipeline, mut group) = (usize::MAX, usize::MAX);
    for &i in &gpu.order {
        let d = &draws[i];
        if !d3d.in_view(d.k) || d3d.is_faded(&d3d.draws[d.k]) {
            continue;
        }
        let Some(mesh) = meshes.get(d.mesh) else {
            continue;
        };
        let p = cull_index(d.cull);
        let opaque = opaque_pipelines.filter(|_| !d.alpha_test);
        let family = if specialized && d.skin_slot == 0 {
            d.kind
        } else {
            0
        };
        let selected = match family {
            1 => &gpu.gbuffer_static.as_ref().expect("static pipelines")
                [usize::from(opaque.is_some())],
            2 => gpu.gbuffer_terrain.as_ref().expect("terrain pipelines"),
            _ => opaque.unwrap_or(pipelines),
        };
        let key = family * 6 + p + if opaque.is_some() { 3 } else { 0 };
        if key != pipeline {
            pass.set_pipeline(&selected[p]);
            pipeline = key;
        }
        if d.group != group {
            pass.set_bind_group(1, &gpu.material_groups[d.group], &[]);
            group = d.group;
        }
        pass.set_bind_group(
            0,
            &gpu.scene_group,
            &[(i * DRAW_STRIDE) as u32, (d.skin_slot * SKIN_STRIDE) as u32],
        );
        pass.set_vertex_buffer(0, gpu.vertices[&d.mesh].slice(..));
        pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..mesh.count, 0, 0..1);
    }
}

/// Gives the shadow slots to lights without baked visibility (emitters, moving lights,
/// and all point lights in probe mode) that matter most from `eye`: their
/// brightness times reach over their distance. Returns each slot's light (position,
/// radius).
fn shadow_slots(lights: &mut [GpuLight], eye: Vec3) -> Vec<(Vec3, f32)> {
    let score = |l: &GpuLight| {
        let p = Vec3::from_slice(&l.position[..3]);
        let bright = l.colour[0].max(l.colour[1]).max(l.colour[2]);
        bright * l.position[3] / (1.0 + p.distance(eye))
    };
    let mut order: Vec<usize> = (0..lights.len())
        .filter(|&i| lights[i].colour[3] < 0.5 && lights[i].position[3] > 0.0)
        .collect();
    order.sort_by(|&a, &b| score(&lights[b]).total_cmp(&score(&lights[a])));
    order.truncate(crate::deferred_shadow::SLOTS);
    order
        .into_iter()
        .enumerate()
        .map(|(slot, i)| {
            lights[i].shadow[0] = slot as f32;
            let l = &lights[i];
            (Vec3::from_slice(&l.position[..3]), l.position[3])
        })
        .collect()
}

/// A debug view in place of the lit frame, from `THEDAS_DEFERRED_VIEW`: `albedo`,
/// `normal`, `lightmap` (its rgb), `visibility` (its alpha), `diffuse` (the light arriving,
/// halved), `specular`, `no-rooms` (lit, every light reaching every room), `flat` (lit,
/// the normal maps left out) or `wrap` (lit with Static1's falloff as it is, which past a
/// light's radius rises again to full at twice it: most of the faithful Cousland floor's
/// brightness, per pixel a hard-edged ring) or `shadows` (red: how much of the first
/// shadowed light in reach gets through, green: its falloff). Read once.
fn debug_view() -> u32 {
    static VIEW: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *VIEW.get_or_init(|| {
        let v = std::env::var("THEDAS_DEFERRED_VIEW").unwrap_or_default();
        match v.as_str() {
            "albedo" => 1,
            "normal" => 2,
            "lightmap" => 3,
            "visibility" => 4,
            "diffuse" => 5,
            "specular" => 6,
            "no-rooms" => 7,
            "flat" => 8,
            "wrap" => 9,
            "shadows" => 10,
            "sun-shadows" => 11,
            "ao" => 12,
            _ => 0,
        }
    })
}

/// The brightest a glowing emitter's light gets (its strongest channel), approached as its
/// particles show more light: `1 − e^(−shown / EMITTER_SHOWN)`, with `shown` their summed
/// colour times alpha times area (m²). Ours, tuned by eye: a torch or brazier already has
/// a light of the level's beside its flames, so this adds a flicker and spell glow on top
/// rather than a second torch (provisional).
const EMITTER_PEAK: f32 = 2.0;
const EMITTER_SHOWN: f32 = 1.0;
/// An emitter light's reach: this much past its particles' spread, plus this much at its
/// peak.
const EMITTER_REACH: f32 = 2.0;
const EMITTER_REACH_PER: f32 = 4.0;

/// The light an emitter's living particles give, as a point light at their glow-weighted
/// centre (ours: the original's particles light nothing). `None` when they show none.
pub fn emitter_light(e: &crate::particles::EmitterState) -> Option<SceneLight> {
    if e.opacity <= 0.0 || e.particles.is_empty() {
        return None;
    }
    let mut total = Vec3::ZERO;
    let mut centre = Vec3::ZERO;
    let mut weight = 0.0;
    let mut glow = Vec::with_capacity(e.particles.len());
    for p in &e.particles {
        let look = e.look(p);
        let area = (look.size[0] * look.size[1]).abs().min(4.0);
        let c = look.color.truncate().max(Vec3::ZERO) * look.color.w.clamp(0.0, 1.0) * area;
        let w = c.x + c.y + c.z;
        if !w.is_finite() || w <= 0.0 {
            continue;
        }
        total += c;
        centre += p.position * w;
        weight += w;
        glow.push((p.position, w));
    }
    if weight <= 0.0 {
        return None;
    }
    let centre = centre / weight;
    let spread = glow
        .iter()
        .map(|(p, w)| p.distance(centre) * w)
        .sum::<f32>()
        / weight;
    let shown = total.x + total.y + total.z;
    let strength = EMITTER_PEAK * (1.0 - (-shown / EMITTER_SHOWN).exp()) * e.opacity.min(1.0);
    let color = total / total.max_element() * strength;
    (centre.is_finite() && strength > 0.01).then(|| SceneLight::Point {
        position: centre,
        color,
        radius: EMITTER_REACH + spread.min(5.0) + EMITTER_REACH_PER * strength / EMITTER_PEAK,
        name: format!("emitter {}", e.blueprint.material),
        domain: None,
        is_static: false,
        version: 1,
        intensity: 1.0,
        room: None,
    })
}

/// The six planes (`n·p + w ≥ 0` inside) of a view-projection with depth 0..1.
pub(super) fn frustum(m: Mat4) -> [Vec4; 6] {
    let (r0, r1, r2, r3) = (m.row(0), m.row(1), m.row(2), m.row(3));
    [r3 + r0, r3 - r0, r3 + r1, r3 - r1, r2, r3 - r2].map(|p| {
        let len = p.truncate().length();
        if len > 0.0 { p / len } else { p }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn material_and_skin_uniforms_fit_portable_limits() {
        assert_eq!(MAX_LIGHTS, 192);
        assert_eq!(std::mem::size_of::<TileUniforms>(), 5120);
        assert!(std::mem::size_of::<TileUniforms>() <= 16 * 1024);
        assert!(std::mem::size_of::<DrawUniforms>() <= DRAW_STRIDE);
        assert_eq!(DRAW_STRIDE % 256, 0);
        assert_eq!(SKIN_STRIDE % 256, 0);
        const {
            assert!(ROWS * 16 <= 16 * 1024);
        }
        assert!(
            GBUFFER
                .iter()
                .map(|f| f.target_pixel_byte_cost().unwrap())
                .sum::<u32>()
                <= 32
        );
        let module = naga::front::wgsl::parse_str(DEFERRED_WGSL).unwrap();
        let info = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap();
        for (i, entry) in module.entry_points.iter().enumerate() {
            if entry.name == "vs_gbuffer_rigid" {
                let used = info.get_entry_point(i);
                let (skin, _) = module
                    .global_variables
                    .iter()
                    .find(|(_, v)| v.name.as_deref() == Some("skin"))
                    .unwrap();
                assert!(
                    used[skin].is_empty(),
                    "rigid vertices must not read a bone palette"
                );
            }
            if entry.stage != naga::ShaderStage::Fragment {
                continue;
            }
            let used = info.get_entry_point(i);
            if entry.name.starts_with("fs_gbuffer_static") {
                for (handle, variable) in module.global_variables.iter() {
                    if variable.binding.as_ref().is_some_and(|b| {
                        b.group == 1 && matches!(b.binding, 7 | 8 | 9 | 11 | 12 | 15)
                    }) {
                        assert!(
                            used[handle].is_empty(),
                            "Static1 must not reach terrain/head samplers"
                        );
                    }
                }
            }
            if matches!(
                entry.name.as_str(),
                "fs_gbuffer_opaque" | "fs_gbuffer_static_opaque" | "fs_gbuffer_terrain"
            ) {
                assert!(
                    !used.may_kill,
                    "opaque G-buffer must have no reachable discard"
                );
            } else if matches!(entry.name.as_str(), "fs_gbuffer" | "fs_gbuffer_static") {
                assert!(used.may_kill, "cutout G-buffer must retain alpha rejection");
            }
            let textures = module
                .global_variables
                .iter()
                .filter(|(handle, v)| {
                    !used[*handle].is_empty()
                        && matches!(module.types[v.ty].inner, naga::TypeInner::Image { .. })
                })
                .count();
            assert!(textures <= 16, "{} uses {textures} textures", entry.name);
        }
    }

    #[test]
    fn deferred_shaders_translate_everywhere() {
        crate::backend_check::translates("deferred", DEFERRED_WGSL);
    }

    #[test]
    fn paths_parse_and_toggle() {
        assert_eq!(RenderPath::parse("Deferred"), Some(RenderPath::Deferred));
        assert_eq!(RenderPath::parse("faithful"), Some(RenderPath::Faithful));
        assert_eq!(RenderPath::parse("forward"), None);
        assert_eq!(RenderPath::default(), RenderPath::Faithful);
        assert_eq!(RenderPath::Faithful.toggled(), RenderPath::Deferred);
        assert_eq!(RenderPath::Deferred.toggled().name(), "faithful");
    }

    #[test]
    fn a_glowing_emitter_lights_around_its_particles() {
        use thedas_formats::emitter::{AgeKey, Emitter};
        let fire = Emitter {
            material: "fx".into(),
            birthrate: 20.0,
            life: 1.0,
            initial_speed: 1.0,
            age_color_multiplier: [1.0; 4],
            age_scale_multiplier: [1.0, 1.0],
            age_map: vec![AgeKey {
                time: 0.0,
                scale: [0.5, 0.5],
                color: [1.0, 0.5, 0.1, 1.0],
            }],
            ..Default::default()
        };
        let at = Vec3::new(10.0, 20.0, 1.0);
        let mut e = crate::particles::EmitterState::new(
            std::sync::Arc::new(fire),
            Mat4::from_translation(at),
            1,
        );
        assert_eq!(emitter_light(&e), None, "no particles, no light");
        e.advance(0.5);
        let Some(SceneLight::Point {
            position,
            color,
            radius,
            is_static,
            room,
            ..
        }) = emitter_light(&e)
        else {
            panic!("a burning emitter gives a point light");
        };
        // Near the emitter, rising with its particles; its colour the particles' hue.
        assert!(position.distance(at) < 1.0, "{position}");
        assert!(color.x > color.y && color.y > color.z, "{color}");
        assert!(color.max_element() <= EMITTER_PEAK);
        assert!(radius > EMITTER_REACH);
        assert!(!is_static && room.is_none());
        // Hidden (a cut-away model's), it gives none.
        e.opacity = 0.0;
        assert_eq!(emitter_light(&e), None);
    }

    #[test]
    fn shadow_pool_prioritizes_dynamic_lights_and_caps_the_slots() {
        let light = |distance: f32, brightness: f32, is_static: bool| GpuLight {
            position: [distance, 0.0, 0.0, 5.0],
            colour: [
                brightness,
                brightness,
                brightness,
                if is_static { 1.0 } else { 0.0 },
            ],
            rooms: [u32::MAX; 8],
            shadow: [-1.0, 0.0, 0.0, 0.0],
        };
        let mut lights = vec![
            light(1.0, 100.0, true),
            light(1.0, 1.0, false),
            light(2.0, 1.0, false),
            light(3.0, 1.0, false),
            light(4.0, 1.0, false),
            light(100.0, 1.0, false),
        ];
        let slots = shadow_slots(&mut lights, Vec3::ZERO);
        assert_eq!(slots.len(), crate::deferred_shadow::SLOTS);
        assert_eq!(
            lights[0].shadow[0], -1.0,
            "baked lights keep baked visibility"
        );
        assert_eq!(lights[1].shadow[0], 0.0, "nearest dynamic light wins");
        assert_eq!(lights[5].shadow[0], -1.0, "the pool has a fixed budget");
        for (slot, (position, radius)) in slots.into_iter().enumerate() {
            assert_eq!(position.x, (slot + 1) as f32);
            assert_eq!(radius, 5.0);
        }
    }

    #[test]
    fn the_frustum_keeps_what_is_ahead() {
        let camera = Camera::new(Vec3::ZERO, 0.0, 0.0);
        let planes = frustum(camera.view_proj(16.0 / 9.0));
        let inside = |p: Vec3, r: f32| planes.iter().all(|q| q.truncate().dot(p) + q.w >= -r);
        // The camera looks along +X.
        assert!(inside(Vec3::new(10.0, 0.0, 0.0), 0.0));
        assert!(!inside(Vec3::new(-10.0, 0.0, 0.0), 1.0));
        // Behind, but its reach comes into view.
        assert!(inside(Vec3::new(-1.0, 0.0, 0.0), 5.0));
    }
}
