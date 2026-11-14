//! GPU side of drawing materials with their translated D3D9 shaders: pipelines from the
//! render state, per-draw constant buffers filled every frame by [`crate::d3d`], vertex
//! streams expanded from the original D3D9 declarations, and sampler bindings.

use std::collections::HashMap;
use std::sync::Arc;

use glam::Vec3;
use thedas_formats::mesh::DeclUsage;
use thedas_formats::state::{
    AddressMode, BlendFactor, BlendOp, CompareFunc, Cull, Filter, SamplerSettings,
};
use thedas_shader::Stage;
use thedas_shader::bytecode::{TextureType, Usage};
use thedas_shader::translate::{self, IoSlot};
use thedas_vfs::Vfs;

use crate::camera::Camera;
use crate::d3d::{
    ConstantContext, D3dProgram, DrawLights, LightClass, StageConstants, TextureSource, ambient_of,
    pick_level_lights, pick_lights, pick_rig_lights, sampler_texture,
};
use crate::gpu::{DEPTH_FORMAT, GpuContext, TextureCache};

/// The partitions' size as a float, for the half-pixel fix-up constant.
use crate::scene::{
    AreaScene, DOMAIN_CHARACTERS, DOMAIN_LEVEL, SceneLight, SceneMaterial, SceneMesh,
};

/// How `CULL_CW` / `CULL_CCW` map onto wgpu. D3D9 culls by screen-space winding; whether our
/// right-handed camera mirrors the original's is checked by comparing against captures
/// (provisional).
pub const D3D_CW_IS_WGPU_CW: bool = true;

const FLOATS: u64 = translate::FLOAT_CONSTANTS as u64 * 16;
const INTS: u64 = translate::INT_CONSTANTS as u64 * 16;
const BOOLS: u64 = translate::BOOL_CONSTANTS as u64 * 16;
const ALIGN: u64 = 256;

/// Byte offsets of each binding inside one draw's constant slot.
const fn a(x: u64) -> u64 {
    x.div_ceil(ALIGN) * ALIGN
}

const fn slot_layout() -> [u64; 8] {
    let vf = 0;
    let vi = a(vf + FLOATS);
    let vb = a(vi + INTS);
    let pf = a(vb + BOOLS);
    let pi = a(pf + FLOATS);
    let pb = a(pi + INTS);
    let fx = a(pb + BOOLS);
    let end = a(fx + 16);
    [vf, vi, vb, pf, pi, pb, fx, end]
}
const SLOT: [u64; 8] = slot_layout();

struct D3dPipeline {
    pipeline: wgpu::RenderPipeline,
    program: Arc<D3dProgram>,
    blended: bool,
}

pub(crate) struct D3dDraw {
    pub pipeline: usize,
    vertices: wgpu::Buffer,
    pub mesh: usize,
    consts: wgpu::BindGroup,
    textures: wgpu::BindGroup,
    slot: u64,
    pub draw: usize,
    lights: DrawLights,
    /// One pass of a multi-pass technique: drawn in pass order with the material's other
    /// passes, after the opaque draws.
    pub multipass: bool,
    /// The draw's bounding sphere in its own space (centre, radius), for the view culling
    /// (see [`in_view`]); `None` for draws that are never culled.
    sphere: Option<(Vec3, f32)>,
    /// A SpeedTree part's draw distances ([`crate::scene::DrawLod`]).
    lod: Option<crate::scene::DrawLod>,
    /// Its position in [`D3dPass::draws`].
    index: usize,
    /// Drawn with a program that reads its constants per instance ([`D3dProgram::instanced`]):
    /// with the other draws in view that share its pipeline, mesh, textures and index range,
    /// as one instanced draw (see [`Instancing`]).
    instanced: bool,
    /// Which slot chunk ([`D3dPass::slot_groups`]) holds its constants, and its slot there.
    chunk: usize,
    local: u32,
    /// What its textures come from: its material and probe; a draw with a lightmap never
    /// shares its textures.
    textures_key: (usize, Option<usize>, Option<usize>),
}

/// This frame's instanced draws ([`D3dDraw::instanced`]): per draw, the group it leads (one
/// draw per group leads it; the others draw with it), and per group its first instance in the
/// instance list, its instance count and its index range.
#[derive(Default)]
struct Instancing {
    lead: Vec<Option<usize>>,
    groups: Vec<(u32, u32, (u32, u32))>,
}

/// Pipelines and vertex buffers kept between passes (see [`crate::gpu::RenderCache`]).
#[derive(Default)]
pub(crate) struct D3dCache {
    /// The constants layout every cached pipeline was made with.
    consts_layout: Option<wgpu::BindGroupLayout>,
    /// The same for programs with per-instance constants ([`D3dProgram::instanced`]).
    instanced_layout: Option<wgpu::BindGroupLayout>,
    /// By program name and target format: the pipeline and its texture layout.
    pipelines:
        HashMap<(String, wgpu::TextureFormat), (wgpu::RenderPipeline, wgpu::BindGroupLayout)>,
    /// By mesh key and vertex input signature.
    vertices: HashMap<(String, String), wgpu::Buffer>,
}

impl D3dCache {
    /// Drops the vertex buffers of the meshes `of` names (by mesh key).
    pub(crate) fn forget_meshes(&mut self, of: &dyn Fn(&str) -> bool) {
        self.vertices.retain(|(k, _), _| !of(k));
    }
}

/// The level's far billboard batch's vertex buffer, whose TEXCOORD0 each frame rewrites with
/// the placements' fades ([`crate::scene::BillboardInstance::texcoord0`]).
struct BillboardStream {
    buffer: wgpu::Buffer,
    /// The buffer's contents as last written.
    data: std::sync::Mutex<Vec<f32>>,
    /// Floats per vertex, and where TEXCOORD0 sits in one.
    stride: usize,
    at: usize,
    instances: Vec<crate::scene::BillboardInstance>,
}

impl BillboardStream {
    /// Rewrites the TEXCOORD0 of the placements whose fade changed for a camera at `eye`, in
    /// one write spanning them.
    fn update(&self, queue: &wgpu::Queue, eye: Vec3) {
        let mut data = self.data.lock().unwrap_or_else(|e| e.into_inner());
        let mut span: Option<(usize, usize)> = None;
        for (k, instance) in self.instances.iter().enumerate() {
            let value = instance.texcoord0(eye);
            for v in 4 * k..4 * k + 4 {
                let at = v * self.stride + self.at;
                let Some(slot) = data.get_mut(at..at + 4) else {
                    continue;
                };
                if slot != value {
                    slot.copy_from_slice(&value);
                    span = Some(span.map_or((at, at + 4), |(a, b)| (a.min(at), b.max(at + 4))));
                }
            }
        }
        if let Some((a, b)) = span {
            queue.write_buffer(
                &self.buffer,
                (a * 4) as u64,
                bytemuck::cast_slice(&data[a..b]),
            );
        }
    }
}

pub(crate) struct D3dPass {
    pipelines: Vec<D3dPipeline>,
    /// Per pipeline, its faded variant (see [`Self::set_opacity`]), made when first needed.
    faded: Vec<Option<wgpu::RenderPipeline>>,
    /// Per pipeline, its texture bindings' layout (for the faded variants).
    layouts: Vec<wgpu::BindGroupLayout>,
    fade_consts: wgpu::BindGroupLayout,
    color_format: wgpu::TextureFormat,
    /// Per scene draw, the opacity its object is drawn at (1: as its material says).
    opacity: Vec<f32>,
    pub draws: Vec<D3dDraw>,
    buffer: Option<wgpu::Buffer>,
    materials: Vec<SceneMaterial>,
    /// Each material switched to its `PSSM` semantic, for the shadow casters' constants.
    caster_materials: Vec<Option<SceneMaterial>>,
    worlds: Vec<(glam::Mat4, Option<crate::scene::LightmapRef>, usize)>,
    /// Each scene draw's irradiance probe.
    probes: Vec<Option<usize>>,
    /// Each scene draw's skinning palette rows (empty: every bone at rest).
    bones: Vec<Vec<[f32; 4]>>,
    scene_probes: Vec<thedas_formats::probe::IrradianceProbe>,
    /// Each scene draw's ambient light node term (see [`ambient_of`]).
    ambients: Vec<Vec3>,
    /// Each scene draw's SpeedTree placement constants, for the `spt_*` shaders.
    trees: Vec<Option<crate::d3d::TreeConstants>>,
    /// Each scene draw's SpeedTree level of detail, and what it came to this frame: the
    /// index range to draw and the alpha reference, `None` when the part is not drawn.
    tree_lods: Vec<Option<crate::scene::TreeLod>>,
    tree_frame: std::sync::Mutex<Vec<Option<(u32, u32, f32)>>>,
    /// The level's far billboard batches.
    billboards: Vec<BillboardStream>,
    /// For the instanced programs: per chunk of slots, its constants and the instance list
    /// ([`instanced_const_layout`]); the list itself; and this frame's groups.
    slot_groups: Vec<wgpu::BindGroup>,
    instance_buffer: Option<wgpu::Buffer>,
    instancing: std::sync::Mutex<Instancing>,
    /// The level's five winds for its trees, 0.2 s apart (DAOrigins.exe 0x5365c0); empty
    /// when the level has no wind file.
    winds: std::sync::Mutex<Vec<thedas_formats::speedtree_gen::wind::Wind>>,
    /// The time the winds were last stepped to.
    wind_time: std::sync::Mutex<f32>,
    /// The level's sun (direction, colour), if it has one.
    sun: Option<(Vec3, Vec3)>,
    fog: [[f32; 4]; 2],
    sky: Option<[[f32; 4]; 9]>,
    clouds: [[f32; 4]; 3],
    pub programs: Vec<String>,
    /// The shadow partitions this pass draws its casters into and its receivers read.
    pub(crate) shadows: Option<Arc<ShadowTargets>>,
    /// Whether this pass places the partitions (the level's own pass; an objects' pass
    /// reads what the level's placed this frame).
    places_partitions: bool,
    reach: [f32; 2],
    /// The first partition gets its variance blur (Medium and High detail; see
    /// [`ShadowBlur`]).
    blur_shadows: bool,
    caster_pipelines: Vec<wgpu::RenderPipeline>,
    casters: Vec<CasterDraw>,
    caster_buffer: Option<wgpu::Buffer>,
    /// This frame's casters in reach of each partition (see [`D3dPass::update`]).
    caster_visible: std::sync::Mutex<Vec<[bool; 2]>>,
    /// What the casters' constants were last filled for (see [`shadow_key`]).
    casters_filled: std::sync::Mutex<Option<u64>>,
    /// This frame's draws (by index into `draws`) in the camera's view (see
    /// [`D3dPass::update`]): only they get constants and are drawn.
    in_view: std::sync::Mutex<Vec<bool>>,
    /// Each draw's constants that stay as it was made, once it was first in view, and which
    /// of its stages' constants change each frame (see [`ConstantContext::per_frame`]).
    /// Sound while a draw's material, lightmap, lights, ambient, probe and the pass's sun,
    /// fog, sky and clouds are fixed when the pass is made: only [`D3dPass::set_world`] and
    /// [`D3dPass::set_bones`] change a draw afterwards, and both feed per-frame constants
    /// (anything else that changes an object's look rebuilds its pass); the one exception
    /// is a moved object's lights, re-picked by [`D3dPass::repick_lights`], which clears
    /// the entry. A setter for any of the fixed inputs must clear the draw's entry here.
    statics: std::sync::Mutex<Vec<Option<Box<StaticConstants>>>>,
    /// Per draw, what its whole constant slot was last written with (target size and
    /// opacity), or `None` when it must be written whole: a draw whose slot is current
    /// gets only its per-frame registers rewritten (the slot keeps the rest between frames).
    written: std::sync::Mutex<Vec<Option<(u32, u32, f32)>>>,
    /// The scene's lights, then the lights added since ([`D3dPass::set_extra_lights`]), for
    /// re-picking the lights of objects that move.
    scene_lights: Vec<SceneLight>,
    /// How many of `scene_lights` are the scene's own.
    own_lights: usize,
    /// Per scene draw, what its lights were picked by when the pass was made: its bounding
    /// sphere and room as placed, and whether it is one of the level's own draws.
    placed: Vec<((Vec3, f32), Option<i64>, bool)>,
    /// Per draw, its lights picked again with the added lights, where they differ from
    /// those the pass was made with; a moved object's re-picked lights (`relit`) come first.
    extra_lit: Vec<Option<DrawLights>>,
    /// The rooms' light reach ([`AreaScene::light_rooms`]) and boxes
    /// ([`AreaScene::room_boxes`]), for re-picking a moved object's room and lights.
    light_rooms: HashMap<i64, Vec<i64>>,
    room_boxes: Vec<(i64, Vec3, Vec3)>,
    /// Each draw's light class (by its material's render class).
    classes: Vec<LightClass>,
    /// For each placed object ([`AreaScene::props`]), its draws (indices into `draws`),
    /// attachments included: they share one light selection, as the exe's render object's
    /// meshes share its light set.
    prop_draws: Vec<Vec<usize>>,
    /// Where each object stood when its lights were last picked.
    prop_at: std::sync::Mutex<Vec<Option<Vec3>>>,
    /// Each draw's lights as last re-picked for its object (`None`: as the pass was made).
    relit: std::sync::Mutex<Vec<Option<DrawLights>>>,
    /// A light rig lighting every draw in place of the scene's lights (see
    /// [`crate::scene::LightRig`]).
    rig: Option<crate::scene::LightRig>,
    caster_programs: Vec<Arc<D3dProgram>>,
    /// The (empty) texture bindings of the casters' programs.
    empty_group: wgpu::BindGroup,
    /// One draw per scene emitter whose material has a program (see [`ParticleDraw`]).
    pub particles: Vec<ParticleDraw>,
    particle_buffer: Option<wgpu::Buffer>,
    /// Two triangles per quad, for [`crate::particles::MAX_PARTICLES`] quads.
    quad_indices: Option<wgpu::Buffer>,
}

/// A particle emitter's draw: its quads, rebuilt each frame into `vertices` in the layout its
/// program's vertex shader reads, with its material's constants and textures.
pub(crate) struct ParticleDraw {
    pipeline: usize,
    vertices: wgpu::Buffer,
    consts: wgpu::BindGroup,
    textures: wgpu::BindGroup,
    slot: u64,
    material: usize,
    /// The scene emitter it draws ([`AreaScene::emitters`]).
    pub emitter: usize,
    /// How many indices to draw this frame (six per quad).
    count: std::sync::atomic::AtomicU32,
    /// Drawn into the distortion mask, not the frame (a `DistortionMask.sta` program; see
    /// [`crate::distortion`]).
    distortion: bool,
}

/// The format of the shadow partitions: `PSSMStatic.psh` writes depth and its square into
/// red and green (the state masks the rest).
pub const SHADOW_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// A draw's vertex and pixel constants that stay as it was made, each with which of its
/// table's constants are refilled every frame.
type StaticConstants = [(crate::d3d::StageConstants, Vec<bool>); 2];

/// A frame's partitions and the `g_vShadowData` they give.
type PlacedPartitions = ([crate::shadow::Partition; 2], [[f32; 4]; 9]);

/// The two shadow partitions' targets, shared by the renderers of one level (see
/// [`crate::shadow`]), and the partitions placed this frame.
pub struct ShadowTargets {
    views: [wgpu::TextureView; 2],
    depth: [wgpu::TextureView; 2],
    frame: std::sync::Mutex<Option<PlacedPartitions>>,
    /// The casters of the passes that do not place the partitions (the objects'), as they
    /// stood when last drawn: world bounding spheres. Provisional: the level's pass fits
    /// the partitions to them a frame late.
    others: std::sync::Mutex<Vec<(Vec3, f32)>>,
    /// What each pass (0 the level's, 1 the objects') has to draw this frame and drew last
    /// (see [`shadow_key`]): when nothing changed the partitions are left as they are.
    keys: std::sync::Mutex<ShadowKeys>,
    /// The first partition's variance blur (see [`ShadowBlur`]).
    blur: ShadowBlur,
    /// The partitions' boxes as last placed, which the next frame's smooth towards their
    /// new fit (see [`crate::shadow::partitions_smoothed`]).
    boxes: std::sync::Mutex<[Option<crate::shadow::LightBox>; 2]>,
    /// Texels on a side of each partition (see [`crate::shadow::partition_size`]).
    size: u32,
}

/// The passes' shadow keys (see [`ShadowTargets::keys`]).
#[derive(Default)]
struct ShadowKeys {
    current: [Option<u64>; 2],
    drawn: [Option<u64>; 2],
    /// This frame's partitions are kept from the last (decided by the level's pass, which
    /// draws first; the objects' pass follows it).
    keep: bool,
}

impl ShadowTargets {
    pub fn new(device: &wgpu::Device, size: u32) -> Self {
        let make = |label, format| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: size,
                    height: size,
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
        };
        let t0 = make("shadow partition 0", SHADOW_FORMAT);
        let t0_view_for_blur = t0.create_view(&Default::default());
        let t1 = make("shadow partition 1", SHADOW_FORMAT);
        let make = |label, format| {
            make(label, format).create_view(&wgpu::TextureViewDescriptor::default())
        };
        ShadowTargets {
            views: [
                t0.create_view(&Default::default()),
                t1.create_view(&Default::default()),
            ],
            depth: [
                make("shadow depth 0", DEPTH_FORMAT),
                make("shadow depth 1", DEPTH_FORMAT),
            ],
            frame: std::sync::Mutex::new(None),
            others: std::sync::Mutex::new(Vec::new()),
            keys: std::sync::Mutex::new(ShadowKeys::default()),
            blur: ShadowBlur::new(device, size, &t0_view_for_blur),
            boxes: std::sync::Mutex::new([None, None]),
            size,
        }
    }

    /// Texels on a side of each partition.
    pub fn size(&self) -> u32 {
        self.size
    }

    fn frame(&self) -> Option<PlacedPartitions> {
        *self.frame.lock().unwrap_or_else(|p| p.into_inner())
    }
}

impl ShadowTargets {
    /// Blurs the first partition, as `BlurShadowTargets` does at Medium and High detail
    /// (see [`ShadowBlur`]), unless this frame kept the partitions as they were (they are
    /// blurred already).
    pub fn blur_first(&self, encoder: &mut wgpu::CommandEncoder) {
        if self.frame().is_none() {
            return;
        }
        if self.keys.lock().unwrap_or_else(|e| e.into_inner()).keep {
            return;
        }
        self.blur.encode(encoder);
    }
}

/// The variance blur of the first shadow partition, as DAOrigins.exe runs it at Medium and
/// High detail: `BlurShadowTargets` → 0x4518e0 runs the slot's filter chain (0x45fd40 →
/// 0x45fcd0) for partition 0 only and only when the shadow quality `[0xbf22c8]` is at least
/// 2. The chain, built at 0x4545e4-0x454b8a: `gaussianblur.mat`'s `HBlurShadow` from the
/// partition into a temporary target, then `VBlurShadow` from it back into the partition,
/// `fBlurWidth` 0.35 (`[0xbf20ac]`, never written) and `fWindowWidth`/`fWindowHeight` the
/// partition's size (`[0xc1e800]`), point sampled and clamped (`PPQuadStateNoBlend.sta`).
///
/// `HorizontalBlurShadows.vsh` / `VerticalBlurShadows.vsh` give the step
/// `fBlurWidth / size` along one axis; `TwoPassBlurShadows.psh` keeps the texel's red (the
/// depth), sums its green and the greens 2, 3 and 4 steps either side, and writes that sum
/// squared over seven, saturated, as green (blue and alpha 0). Each pass maps target pixel
/// `(i, j)` to texture coordinate `(i, j) / size`, as D3D9 did a full-screen quad with no
/// half-texel correction. Transcribed from the shaders' code.
///
/// Provisional: the temporary target's format (taken as the partition's).
struct ShadowBlur {
    pipeline: wgpu::RenderPipeline,
    temp: wgpu::TextureView,
    /// Partition 0 into the temporary target (across), then back (down).
    passes: [wgpu::BindGroup; 2],
    target: wgpu::TextureView,
}

/// `fBlurWidth` for the shadow blur (`[0xbf20ac]`).
pub const SHADOW_BLUR_WIDTH: f32 = 0.35;

const SHADOW_BLUR_WGSL: &str = r#"
struct Params {
    // The step between taps in texture coordinates, and the target's size in texels.
    step: vec2<f32>,
    size: vec2<f32>,
}
@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var smp: sampler;
@group(0) @binding(2) var<uniform> p: Params;

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let x = f32((i << 1u) & 2u) * 2.0 - 1.0;
    let y = f32(i & 2u) * 2.0 - 1.0;
    return vec4<f32>(x, y, 0.0, 1.0);
}

@fragment
fn fs(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = (pos.xy - vec2<f32>(0.5)) / p.size;
    let c = textureSampleLevel(src, smp, uv, 0.0);
    var g = c.y;
    for (var k = 2; k <= 4; k = k + 1) {
        let o = p.step * f32(k);
        g = g + textureSampleLevel(src, smp, uv + o, 0.0).y;
        g = g + textureSampleLevel(src, smp, uv - o, 0.0).y;
    }
    return vec4<f32>(c.x, clamp(g * g * (1.0 / 7.0), 0.0, 1.0), 0.0, 0.0);
}
"#;

impl ShadowBlur {
    fn new(device: &wgpu::Device, size: u32, partition: &wgpu::TextureView) -> Self {
        use wgpu::util::DeviceExt;
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("shadow blur"),
            source: wgpu::ShaderSource::Wgsl(SHADOW_BLUR_WGSL.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shadow blur"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("shadow blur"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("shadow blur"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: SHADOW_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("shadow blur"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let temp = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("shadow blur target"),
                size: wgpu::Extent3d {
                    width: size,
                    height: size,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: SHADOW_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&Default::default());
        let n = size.max(1) as f32;
        let step = SHADOW_BLUR_WIDTH / n;
        let group = |src: &wgpu::TextureView, step: [f32; 2]| {
            let params: [f32; 4] = [step[0], step[1], n, n];
            let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("shadow blur"),
                contents: bytemuck::cast_slice(&params),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("shadow blur"),
                layout: &layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(src),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: buffer.as_entire_binding(),
                    },
                ],
            })
        };
        let passes = [group(partition, [step, 0.0]), group(&temp, [0.0, step])];
        ShadowBlur {
            pipeline,
            temp,
            passes,
            target: partition.clone(),
        }
    }

    fn encode(&self, encoder: &mut wgpu::CommandEncoder) {
        for (k, out) in [&self.temp, &self.target].into_iter().enumerate() {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("shadow blur"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: out,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::WHITE),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.passes[k], &[]);
            pass.draw(0..3, 0..1);
        }
    }
}

/// One shadow caster: a scene draw drawn with its material's `PSSM` program into both
/// partitions.
struct CasterDraw {
    pipeline: usize,
    vertices: wgpu::Buffer,
    mesh: usize,
    draw: usize,
    /// One constant slot per partition.
    consts: [wgpu::BindGroup; 2],
    slots: [u64; 2],
    /// The draw's bounding sphere in its own space (centre, radius), which the partitions
    /// fit (see [`crate::shadow::partitions`]).
    sphere: (Vec3, f32),
}

/// The largest scale a transform applies along any axis (a sphere's radius grows by it).
fn max_scale(m: &glam::Mat4) -> f32 {
    m.x_axis
        .truncate()
        .length()
        .max(m.y_axis.truncate().length())
        .max(m.z_axis.truncate().length())
}

/// Whether a caster's world bounding sphere reaches into a partition's box: anything
/// outside it draws nothing there (the box clips it). Conservative: the box is
/// orthographic, so the sphere's extent along each clip axis is its radius times that
/// row's length.
fn in_partition(p: &crate::shadow::Partition, centre: Vec3, radius: f32) -> bool {
    let m = &p.view_proj;
    let c = m.transform_point3(centre);
    let reach = |row: usize| radius * m.row(row).truncate().length();
    let (rx, ry, rz) = (reach(0), reach(1), reach(2));
    c.x.abs() <= 1.0 + rx && c.y.abs() <= 1.0 + ry && c.z >= -rz && c.z <= 1.0 + rz
}

/// Whether a draw's world bounding sphere reaches into the camera's view: what the
/// original asks Umbra each frame (0x467050, from 0x537e61: 0x466a20 sets the Umbra
/// camera's frustum, `Camera::setFrustum` at 0x466e61, then `Camera::resolveVisibility` at
/// 0x467095 gathers what it sees). Here only its view-frustum test: Umbra's occlusion
/// culling is not mirrored (provisional; it only drops what would be hidden anyway). Conservative: the near plane is taken
/// as OpenGL's (behind D3D's and wgpu's), and the sphere is widened by [`VIEW_MARGIN`].
fn in_view(view_proj: &glam::Mat4, centre: Vec3, radius: f32) -> bool {
    let (r0, r1, r2, r3) = (
        view_proj.row(0),
        view_proj.row(1),
        view_proj.row(2),
        view_proj.row(3),
    );
    let p = centre.extend(1.0);
    let r = radius + VIEW_MARGIN;
    [r3 + r0, r3 - r0, r3 + r1, r3 - r1, r3 + r2, r3 - r2]
        .into_iter()
        .all(|plane| {
            let n = plane.truncate().length();
            n <= f32::EPSILON || plane.dot(p) >= -r * n
        })
}

/// A mesh's bounding sphere in its own space (centre, radius): around its vertices' box.
/// What [`repick`] reads: each object's draws, which draws are in view, the scene's lights,
/// and by draw its position, bounds and the lights it was made with.
struct RepickInput<'a> {
    objects: &'a [Vec<usize>],
    in_view: &'a [bool],
    lights: &'a [SceneLight],
    light_rooms: &'a HashMap<i64, Vec<i64>>,
    /// The room (id) at a position.
    room_at: &'a dyn Fn(Vec3) -> Option<i64>,
    /// A draw's light class.
    class: &'a dyn Fn(usize) -> LightClass,
    position: &'a dyn Fn(usize) -> Vec3,
    sphere: &'a dyn Fn(usize) -> Option<(Vec3, f32)>,
    initial: &'a dyn Fn(usize) -> &'a DrawLights,
}

/// The light re-selection of [`D3dPass::repick_lights`]: for each object in view whose first
/// draw moved since `at` recorded it, picks lights for the union of its draws' bounds and
/// stores them in `relit` for each draw whose lights change. Returns those draws. An object
/// seen for the first time only records where it stands.
fn repick(
    input: &RepickInput<'_>,
    at: &mut Vec<Option<Vec3>>,
    relit: &mut Vec<Option<DrawLights>>,
) -> Vec<usize> {
    let mut changed = Vec::new();
    let draws = input.in_view.len();
    relit.resize(draws, None);
    at.resize(input.objects.len(), None);
    for (p, ks) in input.objects.iter().enumerate() {
        if !ks.iter().any(|&k| input.in_view.get(k) == Some(&true)) {
            continue;
        }
        let Some(&first) = ks.first() else { continue };
        let here = (input.position)(first);
        match at[p] {
            None => {
                at[p] = Some(here);
                continue;
            }
            Some(was) if was.distance_squared(here) < 1e-6 => continue,
            Some(_) => at[p] = Some(here),
        }
        let Some(bound) = ks
            .iter()
            .filter_map(|&k| (input.sphere)(k))
            .reduce(merge_spheres)
        else {
            continue;
        };
        let room = (input.room_at)(here);
        let mut picked: Vec<(LightClass, DrawLights)> = Vec::new();
        for &k in ks {
            if k >= draws {
                continue;
            }
            let class = (input.class)(k);
            let lights = match picked.iter().find(|(c, _)| *c == class) {
                Some((_, l)) => l.clone(),
                None => {
                    // Level-class draws of an object keep the old rule (see
                    // `pick_level_lights`), as before: the characters' domain.
                    let l = match class {
                        LightClass::Level => {
                            pick_level_lights(input.lights, bound, DOMAIN_CHARACTERS)
                        }
                        c => pick_lights(input.lights, input.light_rooms, bound, room, c),
                    };
                    picked.push((class, l.clone()));
                    l
                }
            };
            if *relit[k].as_ref().unwrap_or_else(|| (input.initial)(k)) != lights {
                relit[k] = Some(lights);
                changed.push(k);
            }
        }
    }
    changed
}

/// A draw's lights as a pass picks them when it is made, from `lights`: by its material's
/// light class, its bounding sphere and room as placed, and whether it is one of the
/// level's own draws (see [`pick_level_lights`]).
fn pick_as_built(
    lights: &[SceneLight],
    light_rooms: &HashMap<i64, Vec<i64>>,
    class: LightClass,
    sphere: (Vec3, f32),
    room: Option<i64>,
    level_own: bool,
) -> DrawLights {
    match class {
        LightClass::Level => pick_level_lights(
            lights,
            sphere,
            if level_own {
                DOMAIN_LEVEL
            } else {
                DOMAIN_CHARACTERS
            },
        ),
        c => pick_lights(lights, light_rooms, sphere, room, c),
    }
}

/// The smallest sphere holding two spheres.
fn merge_spheres(a: (Vec3, f32), b: (Vec3, f32)) -> (Vec3, f32) {
    let (d, len) = {
        let v = b.0 - a.0;
        (v, v.length())
    };
    if len + b.1 <= a.1 {
        return a;
    }
    if len + a.1 <= b.1 {
        return b;
    }
    let radius = (len + a.1 + b.1) * 0.5;
    (a.0 + d * ((radius - a.1) / len), radius)
}

fn mesh_sphere(mesh: &SceneMesh) -> (Vec3, f32) {
    let points: Box<dyn Iterator<Item = Vec3>> = match &mesh.positions {
        Some(p) => Box::new(p.iter().map(|p| Vec3::new(p[0], p[1], p[2]))),
        None => Box::new(mesh.vertices.iter().map(|v| Vec3::from_array(v.position))),
    };
    let (lo, hi) = points.fold(
        (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)),
        |(lo, hi), p| (lo.min(p), hi.max(p)),
    );
    if lo.x > hi.x {
        return (Vec3::ZERO, 0.0);
    }
    ((lo + hi) * 0.5, (hi - lo).length() * 0.5)
}

/// A skinned draw's bounding sphere in its own space, posed: every vertex is a weighted
/// blend of its rest position carried by each of its bones, so it lies within a sphere
/// around the rest sphere carried by every palette matrix (`rows`: three rows per bone).
/// The original culls its creatures as it does the rest: every object gets an Umbra object
/// (`Object::create`, 0x467a4b and the other cases of 0x4678f0) with a box model
/// (`OBBModel::create`, 0x466efb, 0x467bcd); this sphere is ours (provisional).
fn skinned_sphere(rest: (Vec3, f32), rows: &[[f32; 4]]) -> (Vec3, f32) {
    let carried: Vec<(Vec3, f32)> = rows
        .chunks_exact(3)
        .map(|r| {
            let m = glam::Mat4::from_cols_array_2d(&[r[0], r[1], r[2], [0.0, 0.0, 0.0, 1.0]])
                .transpose();
            (m.transform_point3(rest.0), rest.1 * max_scale(&m))
        })
        .collect();
    if carried.is_empty() {
        return rest;
    }
    let centre = carried.iter().map(|c| c.0).sum::<Vec3>() / carried.len() as f32;
    let radius = carried
        .iter()
        .map(|(c, r)| c.distance(centre) + r)
        .fold(0.0, f32::max);
    (centre, radius)
}

/// How far (in metres) a draw's bounding sphere is widened for the view culling: room for
/// vertex shaders that move vertices a little (wind, waves).
const VIEW_MARGIN: f32 = 1.0;

/// What a pass's shadow draws depend on: the partitions, which casters each draws, and
/// those casters' transforms and bones. The same key, the same partitions drawn.
fn shadow_key(
    partitions: &[crate::shadow::Partition; 2],
    casters: &[CasterDraw],
    visible: &[[bool; 2]],
    worlds: &[(glam::Mat4, Option<crate::scene::LightmapRef>, usize)],
    bones: &[Vec<[f32; 4]>],
) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for p in partitions {
        for v in p.view_proj.to_cols_array() {
            v.to_bits().hash(&mut h);
        }
    }
    for (c, v) in casters.iter().zip(visible) {
        if !v[0] && !v[1] {
            continue;
        }
        c.draw.hash(&mut h);
        v.hash(&mut h);
        for x in worlds[c.draw].0.to_cols_array() {
            x.to_bits().hash(&mut h);
        }
        if let Some(b) = bones.get(c.draw) {
            for r in b {
                for x in r {
                    x.to_bits().hash(&mut h);
                }
            }
        }
    }
    h.finish()
}

fn decl_usage(u: Usage) -> DeclUsage {
    match u {
        Usage::Position => DeclUsage::Position,
        Usage::BlendWeight => DeclUsage::BlendWeight,
        Usage::BlendIndices => DeclUsage::BlendIndices,
        Usage::Normal => DeclUsage::Normal,
        Usage::PSize => DeclUsage::PointSize,
        Usage::TexCoord => DeclUsage::TexCoord,
        Usage::Tangent => DeclUsage::Tangent,
        Usage::Binormal => DeclUsage::Binormal,
        Usage::TessFactor => DeclUsage::TessFactor,
        Usage::PositionT => DeclUsage::PositionT,
        Usage::Color => DeclUsage::Color,
        Usage::Fog => DeclUsage::Fog,
        Usage::Depth => DeclUsage::Depth,
        Usage::Sample => DeclUsage::Sample,
    }
}

/// Expands the vertex streams a shader reads into interleaved `Float32x4`s, the way D3D9
/// expands declared elements (missing components default to 0, 0, 0, 1). Elements the mesh
/// lacks read as (0, 0, 0, 1).
fn expand_vertices(mesh: &SceneMesh, inputs: &[IoSlot]) -> Vec<f32> {
    let c = &mesh.chunk;
    let n = mesh.vertices.len();
    let stride = c.vertex_size as usize;
    let mut out = Vec::with_capacity(n * inputs.len() * 4);
    for v in 0..n {
        for slot in inputs {
            let usage = decl_usage(slot.semantic.usage);
            if let (DeclUsage::Position, 0, Some(p)) =
                (usage, slot.semantic.index, mesh.positions.as_ref())
            {
                out.extend_from_slice(&p.get(v).copied().unwrap_or([0.0, 0.0, 0.0, 1.0]));
                continue;
            }
            // Dragon Age II's lighting baked per vertex: its three colours are texture
            // coordinates 1, 2 and 3 (`static_static_p1.vsh`, which cubes each and never
            // reads the fourth byte). Provisional: the bytes are taken in the order stored
            // as red, green and blue; so read, `lt_backalley_g`'s colours average the same
            // blue-grey as its lightmaps (72, 76, 90 against 73, 74, 85).
            if let (DeclUsage::TexCoord, i @ 1..=3, Some(light)) =
                (usage, slot.semantic.index, mesh.vertex_light.as_ref())
            {
                let c = light.get(v).map_or([0; 4], |l| l[i as usize - 1]);
                out.extend_from_slice(&c.map(|b| f32::from(b) / 255.0));
                continue;
            }
            let value = c
                .element(usage, slot.semantic.index as u32)
                .and_then(|e| {
                    let at = v * stride + e.offset as usize;
                    let bytes = mesh.raw_vertices.get(at..at + e.ty.size() as usize)?;
                    Some(e.ty.decode(bytes))
                })
                .or_else(|| {
                    // Meshes built in code (tests, tree billboards) have no raw streams;
                    // fall back to the stand-in vertex.
                    (stride == 0).then(|| {
                        let sv = &mesh.vertices[v];
                        match (usage, slot.semantic.index) {
                            (DeclUsage::Position, _) => {
                                [sv.position[0], sv.position[1], sv.position[2], 1.0]
                            }
                            (DeclUsage::Normal, _) => {
                                [sv.normal[0], sv.normal[1], sv.normal[2], 1.0]
                            }
                            (DeclUsage::TexCoord, 0) => [sv.uv0[0], sv.uv0[1], 0.0, 1.0],
                            (DeclUsage::TexCoord, 1) => [sv.uv1[0], sv.uv1[1], 0.0, 1.0],
                            // Built meshes (tree billboards) stand upright: binormal up,
                            // tangent along the surface.
                            (DeclUsage::Binormal, _) => [0.0, 0.0, 1.0, 1.0],
                            (DeclUsage::Tangent, _) => {
                                let n = Vec3::from_array(sv.normal);
                                let t = n.cross(Vec3::Z).normalize_or_zero();
                                [t.x, t.y, t.z, 1.0]
                            }
                            _ => [0.0, 0.0, 0.0, 1.0],
                        }
                    })
                })
                .unwrap_or([0.0, 0.0, 0.0, 1.0]);
            out.extend_from_slice(&value);
        }
    }
    out
}

fn compare(f: CompareFunc) -> wgpu::CompareFunction {
    match f {
        CompareFunc::Never => wgpu::CompareFunction::Never,
        CompareFunc::Less => wgpu::CompareFunction::Less,
        CompareFunc::Equal => wgpu::CompareFunction::Equal,
        CompareFunc::LessEqual => wgpu::CompareFunction::LessEqual,
        CompareFunc::Greater => wgpu::CompareFunction::Greater,
        CompareFunc::NotEqual => wgpu::CompareFunction::NotEqual,
        CompareFunc::GreaterEqual => wgpu::CompareFunction::GreaterEqual,
        CompareFunc::Always => wgpu::CompareFunction::Always,
    }
}

fn blend_factor(f: BlendFactor) -> wgpu::BlendFactor {
    use wgpu::BlendFactor as W;
    match f {
        BlendFactor::Zero => W::Zero,
        BlendFactor::One => W::One,
        BlendFactor::SrcColor => W::Src,
        BlendFactor::InvSrcColor => W::OneMinusSrc,
        BlendFactor::SrcAlpha => W::SrcAlpha,
        BlendFactor::InvSrcAlpha => W::OneMinusSrcAlpha,
        BlendFactor::DestAlpha => W::DstAlpha,
        BlendFactor::InvDestAlpha => W::OneMinusDstAlpha,
        BlendFactor::DestColor => W::Dst,
        BlendFactor::InvDestColor => W::OneMinusDst,
        BlendFactor::SrcAlphaSat => W::SrcAlphaSaturated,
    }
}

fn blend_op(o: BlendOp) -> wgpu::BlendOperation {
    match o {
        BlendOp::Add => wgpu::BlendOperation::Add,
        BlendOp::Subtract => wgpu::BlendOperation::Subtract,
        BlendOp::RevSubtract => wgpu::BlendOperation::ReverseSubtract,
        BlendOp::Min => wgpu::BlendOperation::Min,
        BlendOp::Max => wgpu::BlendOperation::Max,
    }
}

/// `border` says the device can clamp to a border colour; without it, clamping to the edge
/// is the closest match.
fn address(a: AddressMode, border: bool) -> wgpu::AddressMode {
    match a {
        AddressMode::Wrap => wgpu::AddressMode::Repeat,
        AddressMode::Mirror | AddressMode::MirrorOnce => wgpu::AddressMode::MirrorRepeat,
        AddressMode::Border if border => wgpu::AddressMode::ClampToBorder,
        AddressMode::Clamp | AddressMode::Border => wgpu::AddressMode::ClampToEdge,
    }
}

/// The D3D border colour (`SAMP_BORDERCOLOR`, ARGB) as the nearest colour wgpu offers:
/// transparent black, opaque black or opaque white.
fn border_color(argb: u32) -> wgpu::SamplerBorderColor {
    let alpha = argb >> 24;
    let rgb = argb & 0x00ff_ffff;
    match (alpha >= 0x80, rgb >= 0x0080_8080) {
        (false, _) => wgpu::SamplerBorderColor::TransparentBlack,
        (true, false) => wgpu::SamplerBorderColor::OpaqueBlack,
        (true, true) => wgpu::SamplerBorderColor::OpaqueWhite,
    }
}

fn filter(f: Filter) -> wgpu::FilterMode {
    match f {
        Filter::None | Filter::Point => wgpu::FilterMode::Nearest,
        Filter::Linear | Filter::Anisotropic => wgpu::FilterMode::Linear,
    }
}

fn sampler_key(s: &SamplerSettings) -> String {
    format!("{s:?}")
}

fn make_sampler(device: &wgpu::Device, s: &SamplerSettings) -> wgpu::Sampler {
    let border = device
        .features()
        .contains(wgpu::Features::ADDRESS_MODE_CLAMP_TO_BORDER);
    let modes = [s.address_u, s.address_v, s.address_w];
    let uses_border = border && modes.contains(&AddressMode::Border);
    device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("d3d sampler"),
        address_mode_u: address(s.address_u, border),
        address_mode_v: address(s.address_v, border),
        address_mode_w: address(s.address_w, border),
        border_color: uses_border.then(|| border_color(s.border)),
        mag_filter: filter(s.mag),
        min_filter: filter(s.min),
        mipmap_filter: match s.mip {
            Filter::Linear | Filter::Anisotropic => wgpu::MipmapFilterMode::Linear,
            _ => wgpu::MipmapFilterMode::Nearest,
        },
        // MIPFILTER NONE samples the top level only.
        lod_max_clamp: if s.mip == Filter::None { 0.0 } else { 32.0 },
        ..Default::default()
    })
}

fn view_dimension(t: TextureType) -> wgpu::TextureViewDimension {
    match t {
        TextureType::Cube => wgpu::TextureViewDimension::Cube,
        TextureType::Volume => wgpu::TextureViewDimension::D3,
        _ => wgpu::TextureViewDimension::D2,
    }
}

fn texture_entries(program: &D3dProgram) -> Vec<(Stage, u32, TextureType)> {
    let mut v = Vec::new();
    for (s, t) in &program.vs.interface.samplers {
        v.push((Stage::Vertex, *s, *t));
    }
    for (s, t) in &program.ps.interface.samplers {
        v.push((Stage::Pixel, *s, *t));
    }
    v
}

fn texture_layout(device: &wgpu::Device, program: &D3dProgram) -> wgpu::BindGroupLayout {
    let mut entries = Vec::new();
    for (stage, s, t) in texture_entries(program) {
        let vis = if stage == Stage::Vertex {
            wgpu::ShaderStages::VERTEX
        } else {
            wgpu::ShaderStages::FRAGMENT
        };
        let b = translate::binding_texture(stage, s);
        entries.push(wgpu::BindGroupLayoutEntry {
            binding: b,
            visibility: vis,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: view_dimension(t),
                multisampled: false,
            },
            count: None,
        });
        entries.push(wgpu::BindGroupLayoutEntry {
            binding: b + 1,
            visibility: vis,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        });
    }
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some(&program.name),
        entries: &entries,
    })
}

/// The constants of programs with per-instance constants: every slot of a chunk as one
/// storage array, and this frame's instance list.
fn instanced_const_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    let entry = |binding: u32, vis: wgpu::ShaderStages| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: vis,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    };
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("d3d instanced constants"),
        entries: &[
            entry(
                translate::BINDING_SLOTS,
                wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
            ),
            entry(translate::BINDING_INSTANCES, wgpu::ShaderStages::VERTEX),
        ],
    })
}

fn const_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    let entry = |binding: u32, vis: wgpu::ShaderStages| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: vis,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    };
    let v = wgpu::ShaderStages::VERTEX;
    let f = wgpu::ShaderStages::FRAGMENT;
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("d3d constants"),
        entries: &[
            entry(translate::binding_float(Stage::Vertex), v),
            entry(translate::binding_int(Stage::Vertex), v),
            entry(translate::binding_bool(Stage::Vertex), v),
            entry(translate::binding_float(Stage::Pixel), f),
            entry(translate::binding_int(Stage::Pixel), f),
            entry(translate::binding_bool(Stage::Pixel), f),
            // The fix-up vector: the vertex stage's half-pixel shift, and in `z` the alpha
            // reference of draws that set it per draw (the trees').
            entry(translate::BINDING_FIXUP, v | f),
        ],
    })
}

/// One draw's constants: the bindings of the slot at `slot` in `buf`.
fn const_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    buf: &wgpu::Buffer,
    slot: u64,
) -> wgpu::BindGroup {
    let range = |i: usize, size: u64| {
        wgpu::BindingResource::Buffer(wgpu::BufferBinding {
            buffer: buf,
            offset: slot + SLOT[i],
            size: wgpu::BufferSize::new(size),
        })
    };
    let entry = |binding, resource| wgpu::BindGroupEntry { binding, resource };
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("d3d draw constants"),
        layout,
        entries: &[
            entry(translate::binding_float(Stage::Vertex), range(0, FLOATS)),
            entry(translate::binding_int(Stage::Vertex), range(1, INTS)),
            entry(translate::binding_bool(Stage::Vertex), range(2, BOOLS)),
            entry(translate::binding_float(Stage::Pixel), range(3, FLOATS)),
            entry(translate::binding_int(Stage::Pixel), range(4, INTS)),
            entry(translate::binding_bool(Stage::Pixel), range(5, BOOLS)),
            entry(translate::BINDING_FIXUP, range(6, 16)),
        ],
    })
}

/// Multiplies a stage's engine fade (`mml_fOpacityMultiplier`) by `opacity`.
fn fade_constants(
    table: Option<&thedas_shader::ctab::ConstantTable>,
    c: &mut crate::d3d::StageConstants,
    opacity: f32,
) {
    let Some(t) = table else { return };
    for k in &t.constants {
        if k.name == "mml_fOpacityMultiplier"
            && k.set == thedas_shader::ctab::RegisterSet::Float4
            && let Some(r) = c.floats.get_mut(k.register as usize)
        {
            r[0] *= opacity;
        }
    }
}

/// Whether `program` draws into the distortion mask: `DADistortionMask.mat`'s passes, by
/// their state.
fn is_distortion(program: &D3dProgram) -> bool {
    program.name.ends_with("+distortionmask.sta")
}

/// The depth bias of a blended pass that tests depth without writing it: a hair's second
/// pass draws over the depth its first pass wrote, from another vertex shader, and our
/// translations of the two need not land on bit-identical depths, so without a nudge
/// towards the camera whole triangles of it fail the test (Hawke's beard showed
/// triangular holes). A few steps of the depth format's precision, more on steep slopes;
/// small enough not to bring a blended surface through anything in front of it.
/// Provisional: D3D9's invariance between the original's two shaders is not ours to rely
/// on.
const OVERLAY_DEPTH_BIAS: wgpu::DepthBiasState = wgpu::DepthBiasState {
    constant: -32,
    slope_scale: -1.0,
    clamp: 0.0,
};

fn create_pipeline(
    device: &wgpu::Device,
    program: &D3dProgram,
    consts: &wgpu::BindGroupLayout,
    textures: &wgpu::BindGroupLayout,
    color_format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    create_pipeline_faded(device, program, consts, textures, color_format, false)
}

/// [`create_pipeline`]; `faded` blends an opaque pass by its output alpha
/// (`SrcAlpha`, `InvSrcAlpha`), for an object drawn at an opacity below 1.
fn create_pipeline_faded(
    device: &wgpu::Device,
    program: &D3dProgram,
    consts: &wgpu::BindGroupLayout,
    textures: &wgpu::BindGroupLayout,
    color_format: wgpu::TextureFormat,
    faded: bool,
) -> wgpu::RenderPipeline {
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(&program.name),
        bind_group_layouts: &[Some(consts), Some(textures)],
        immediate_size: 0,
    });
    let vs = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(&program.name),
        source: wgpu::ShaderSource::Wgsl(program.vs.wgsl.clone().into()),
    });
    let ps = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(&program.name),
        source: wgpu::ShaderSource::Wgsl(program.ps.wgsl.clone().into()),
    });
    let inputs = &program.vs.interface.inputs;
    let attributes: Vec<wgpu::VertexAttribute> = inputs
        .iter()
        .enumerate()
        .map(|(i, s)| wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x4,
            offset: 16 * i as u64,
            shader_location: s.location,
        })
        .collect();
    let pass = &program.pass;
    let cull = match pass.cull {
        Cull::None => None,
        // wgpu culls `Back` faces relative to `front_face` (CCW here).
        Cull::Cw => Some(if D3D_CW_IS_WGPU_CW {
            wgpu::Face::Back
        } else {
            wgpu::Face::Front
        }),
        Cull::Ccw => Some(if D3D_CW_IS_WGPU_CW {
            wgpu::Face::Front
        } else {
            wgpu::Face::Back
        }),
    };
    let component = |(src, dst, op)| wgpu::BlendComponent {
        src_factor: blend_factor(src),
        dst_factor: blend_factor(dst),
        operation: blend_op(op),
    };
    let blend = pass.blend.map(|colour| wgpu::BlendState {
        color: component(colour),
        alpha: component(pass.blend_alpha.unwrap_or(colour)),
    });
    let blend = blend.or(faded.then_some(wgpu::BlendState::ALPHA_BLENDING));
    let mut write = wgpu::ColorWrites::empty();
    for (bit, w) in [
        (1, wgpu::ColorWrites::RED),
        (2, wgpu::ColorWrites::GREEN),
        (4, wgpu::ColorWrites::BLUE),
        (8, wgpu::ColorWrites::ALPHA),
    ] {
        if pass.color_write & bit != 0 {
            write |= w;
        }
    }
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(&program.name),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &vs,
            entry_point: Some(translate::ENTRY_POINT),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: (16 * inputs.len().max(1)) as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &attributes,
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
            depth_write_enabled: Some(pass.z_enable && pass.z_write),
            depth_compare: Some(if pass.z_enable {
                // An "equal" test (a later pass drawing exactly over an earlier one's depth,
                // as hair's passes do) becomes "less or equal": the passes' shaders are
                // translated separately and need not reach bit-identical depths
                // (provisional).
                match compare(pass.z_func) {
                    wgpu::CompareFunction::Equal => wgpu::CompareFunction::LessEqual,
                    c => c,
                }
            } else {
                wgpu::CompareFunction::Always
            }),
            stencil: wgpu::StencilState::default(),
            bias: if pass.z_enable && !pass.z_write && pass.blend.is_some() {
                OVERLAY_DEPTH_BIAS
            } else {
                wgpu::DepthBiasState::default()
            },
        }),
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &ps,
            entry_point: Some(translate::ENTRY_POINT),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: color_format,
                blend,
                write_mask: write,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

/// A draw's texture and sampler bindings: each sampler its program reads, bound to the
/// texture the material (or lightmap, or shadow partition) gives it.
#[allow(clippy::too_many_arguments)]
fn texture_group(
    ctx: &GpuContext,
    vfs: &Vfs,
    textures: &mut TextureCache,
    shadows: Option<&ShadowTargets>,
    samplers: &mut HashMap<String, wgpu::Sampler>,
    layout: &wgpu::BindGroupLayout,
    program: &D3dProgram,
    material: &SceneMaterial,
    lightmap: Option<&crate::scene::LightmapRef>,
    probe: Option<&str>,
) -> wgpu::BindGroup {
    let device = &ctx.device;
    let mut views: Vec<(u32, wgpu::TextureView, String)> = Vec::new();
    for (stage, s, ty) in texture_entries(program) {
        let table = if stage == Stage::Vertex {
            program.vs.ctab.as_ref()
        } else {
            program.ps.ctab.as_ref()
        };
        let (sampler_name, source) = sampler_texture(table, s, ty, material, lightmap, probe);
        let view = match &source {
            TextureSource::Resource(name) => textures.view(ctx, vfs, name, ty),
            TextureSource::ResourceOr(name, default) => textures
                .view(ctx, vfs, name, ty)
                .or_else(|| textures.view(ctx, vfs, default, ty)),
            TextureSource::Shadow(i) => shadows.and_then(|t| t.views.get(*i as usize).cloned()),
            TextureSource::White => None,
            TextureSource::Black => None,
        }
        .unwrap_or_else(|| {
            textures.solid(
                ctx,
                ty,
                if matches!(source, TextureSource::White | TextureSource::Shadow(_)) {
                    [255; 4]
                } else {
                    [0, 0, 0, 255]
                },
            )
        });
        // A state file names a sampler or a stage: a pixel shader's sampler register is its
        // stage (see `thedas_formats::state`); vertex samplers are named only.
        let at = (stage == Stage::Pixel).then_some(s);
        let settings = program
            .state
            .sampler_at(sampler_name.as_deref(), at)
            .unwrap_or_default();
        let key = sampler_key(&settings);
        samplers
            .entry(key.clone())
            .or_insert_with(|| make_sampler(device, &settings));
        views.push((translate::binding_texture(stage, s), view, key));
    }
    let mut entries = Vec::new();
    for (b, view, key) in &views {
        entries.push(wgpu::BindGroupEntry {
            binding: *b,
            resource: wgpu::BindingResource::TextureView(view),
        });
        entries.push(wgpu::BindGroupEntry {
            binding: *b + 1,
            resource: wgpu::BindingResource::Sampler(&samplers[key]),
        });
    }
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("d3d draw textures"),
        layout,
        entries: &entries,
    })
}

/// Particle quad corners laid out as `inputs` (a vertex shader's inputs) read them, one
/// `Float32x4` each: `POSITION` (w 1), `COLOR0`, `TEXCOORD0`; anything else (0, 0, 0, 1).
/// `basis` is the camera's (right, up, forward): the quads face it, so a shader that reads a
/// tangent frame (`DistortionVFX.vsh`) gets the normal towards the camera, the tangent to
/// its right and the binormal up. Provisional: which of right and up the game's sprites call
/// the tangent is not traced.
fn particle_vertices(
    quads: &[crate::particles::ParticleVertex],
    inputs: &[IoSlot],
    basis: (Vec3, Vec3, Vec3),
) -> Vec<f32> {
    let (right, up, forward) = basis;
    let direction = |d: Vec3| [d.x, d.y, d.z, 0.0];
    let mut out = Vec::with_capacity(quads.len() * inputs.len() * 4);
    for v in quads {
        for slot in inputs {
            let value = match (decl_usage(slot.semantic.usage), slot.semantic.index) {
                (DeclUsage::Position, 0) => [v.position[0], v.position[1], v.position[2], 1.0],
                (DeclUsage::Color, 0) => v.color,
                (DeclUsage::TexCoord, 0) => v.texcoord,
                (DeclUsage::Normal, 0) => direction(-forward),
                (DeclUsage::Tangent, 0) => direction(right),
                (DeclUsage::Binormal, 0) => direction(up),
                _ => [0.0, 0.0, 0.0, 1.0],
            };
            out.extend_from_slice(&value);
        }
    }
    out
}

/// The ambient light of each room for level geometry: the sum of the ambient lights its
/// light model holds. A draw in a room with none (or outside every room) takes the average
/// over the rooms that have one. Provisional (see [`ambient_of`]): the original's per-object
/// pass selects no ambient light node for level meshes, but drawing them with none leaves the
/// Highever hall far darker than the original's. A level with no ambient light nodes
/// (Lothering) gets none.
fn room_ambients(scene: &AreaScene) -> impl Fn(&str) -> Vec3 {
    let mut by_room: HashMap<String, Vec3> = HashMap::new();
    for l in &scene.lights {
        if let SceneLight::Ambient { color, room, .. } = l {
            *by_room
                .entry(room.to_ascii_lowercase())
                .or_insert(Vec3::ZERO) += *color;
        }
    }
    let average = if by_room.is_empty() {
        Vec3::ZERO
    } else {
        by_room.values().copied().sum::<Vec3>() / by_room.len() as f32
    };
    move |room: &str| {
        by_room
            .get(&room.to_ascii_lowercase())
            .copied()
            .unwrap_or(average)
    }
}

impl D3dPass {
    /// `material_programs[i]` holds the programs (one per pass) for scene material `i`;
    /// empty when it is not drawn with its original shaders.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        ctx: &GpuContext,
        vfs: &Vfs,
        scene: &AreaScene,
        material_programs: &[Vec<Arc<D3dProgram>>],
        caster_programs: &[Option<crate::d3d::CasterMaterial>],
        shadows: Option<Arc<ShadowTargets>>,
        textures: &mut TextureCache,
        cache: &mut D3dCache,
        color_format: wgpu::TextureFormat,
    ) -> Self {
        let device = &ctx.device;
        let consts_layout = cache
            .consts_layout
            .get_or_insert_with(|| const_layout(device))
            .clone();
        let instanced_layout = cache
            .instanced_layout
            .get_or_insert_with(|| instanced_const_layout(device))
            .clone();
        let mut pipelines: Vec<D3dPipeline> = Vec::new();
        let mut layouts: Vec<wgpu::BindGroupLayout> = Vec::new();
        let mut by_name: HashMap<String, usize> = HashMap::new();
        let mut samplers: HashMap<String, wgpu::Sampler> = HashMap::new();
        let ambient = room_ambients(scene);
        // The level's own meshes (placed props such as benches included) keep the old
        // rule whatever their render class (see `pick_level_lights`); the placed objects'
        // draws go by their render class.
        let classes: Vec<LightClass> = scene
            .draws
            .iter()
            .enumerate()
            .map(|(di, d)| {
                if di < scene.static_draws {
                    LightClass::Level
                } else {
                    LightClass::of(scene.materials[d.material].render_class.as_deref())
                }
            })
            .collect();

        let mut pending = Vec::new();
        for (di, d) in scene.draws.iter().enumerate() {
            let Some(passes) = material_programs.get(d.material) else {
                continue;
            };
            for program in passes {
                let pi = *by_name.entry(program.name.clone()).or_insert_with(|| {
                    let (pipeline, tl) = cache
                        .pipelines
                        .entry((program.name.clone(), color_format))
                        .or_insert_with(|| {
                            let tl = texture_layout(device, program);
                            let cl = if program.instanced {
                                &instanced_layout
                            } else {
                                &consts_layout
                            };
                            let pipeline = create_pipeline(device, program, cl, &tl, color_format);
                            (pipeline, tl)
                        })
                        .clone();
                    pipelines.push(D3dPipeline {
                        pipeline,
                        program: program.clone(),
                        blended: program.pass.blend.is_some(),
                    });
                    layouts.push(tl);
                    pipelines.len() - 1
                });
                pending.push((di, pi, passes.len() > 1));
            }
        }

        let slot_size = SLOT[7];
        let buffer = (!pending.is_empty()).then(|| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("d3d constants"),
                size: slot_size * pending.len() as u64,
                usage: wgpu::BufferUsages::UNIFORM
                    | wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        });
        // The instanced programs read the slots as storage arrays, as many slots a chunk as
        // one storage binding may hold, and their slot from the frame's instance list.
        let per_chunk = (device.limits().max_storage_buffer_binding_size / slot_size).max(1);
        let any_instanced = pending
            .iter()
            .any(|&(_, pi, _)| pipelines[pi].program.instanced);
        let instance_buffer = (any_instanced && !pending.is_empty()).then(|| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("d3d instance list"),
                size: 4 * pending.len() as u64,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        });
        let slot_groups: Vec<wgpu::BindGroup> = match (&buffer, &instance_buffer) {
            (Some(buf), Some(list)) => (0..(pending.len() as u64).div_ceil(per_chunk))
                .map(|c| {
                    let first = c * per_chunk;
                    let n = per_chunk.min(pending.len() as u64 - first);
                    device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("d3d instanced constants"),
                        layout: &instanced_layout,
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: translate::BINDING_SLOTS,
                                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                                    buffer: buf,
                                    offset: first * slot_size,
                                    size: wgpu::BufferSize::new(n * slot_size),
                                }),
                            },
                            wgpu::BindGroupEntry {
                                binding: translate::BINDING_INSTANCES,
                                resource: list.as_entire_binding(),
                            },
                        ],
                    })
                })
                .collect(),
            _ => Vec::new(),
        };

        let mut draws = Vec::new();
        let mut billboards: Vec<BillboardStream> = Vec::new();
        let mut mesh_spheres: HashMap<usize, (Vec3, f32)> = HashMap::new();
        for (k, (di, pi, multipass)) in pending.into_iter().enumerate() {
            let d = &scene.draws[di];
            let program = pipelines[pi].program.clone();
            let mesh = &scene.meshes[d.mesh];
            let inputs = &program.vs.interface.inputs;
            let sig: String = inputs
                .iter()
                .map(|s| format!("{:?}{}@{};", s.semantic.usage, s.semantic.index, s.location))
                .collect();
            let batch = scene.tree_billboards.get(&di);
            let vertices = cache
                .vertices
                .entry((mesh.key.clone(), sig))
                .or_insert_with(|| {
                    use wgpu::util::DeviceExt;
                    let data = expand_vertices(mesh, inputs);
                    // The billboard batch's fades are rewritten each frame.
                    let usage = match batch {
                        Some(_) => wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                        None => wgpu::BufferUsages::VERTEX,
                    };
                    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some(&mesh.key),
                        contents: bytemuck::cast_slice(&data),
                        usage,
                    })
                })
                .clone();
            if let Some(instances) = batch
                && !billboards
                    .iter()
                    .any(|b: &BillboardStream| b.buffer == vertices)
                && let Some(at) = inputs.iter().position(|s| {
                    decl_usage(s.semantic.usage) == DeclUsage::TexCoord && s.semantic.index == 0
                })
            {
                billboards.push(BillboardStream {
                    buffer: vertices.clone(),
                    data: std::sync::Mutex::new(expand_vertices(mesh, inputs)),
                    stride: inputs.len() * 4,
                    at: at * 4,
                    instances: instances.clone(),
                });
            }
            let buf = buffer.as_ref().expect("buffer exists when there are draws");
            let slot = slot_size * k as u64;
            let consts = const_group(device, &consts_layout, buf, slot);

            // Textures and samplers.
            let textures_group = texture_group(
                ctx,
                vfs,
                textures,
                shadows.as_deref(),
                &mut samplers,
                &layouts[pi],
                &program,
                &scene.materials[d.material],
                d.lightmap.as_ref(),
                d.probe
                    .and_then(|p| scene.probes.get(p))
                    .map(|p| p.name.as_str()),
            );
            let instanced = program.instanced;
            draws.push(D3dDraw {
                index: draws.len(),
                instanced,
                chunk: (k as u64 / per_chunk) as usize,
                local: (k as u64 % per_chunk) as u32,
                textures_key: (d.material, d.probe, d.lightmap.is_some().then_some(di)),
                pipeline: pi,
                vertices,
                mesh: d.mesh,
                consts,
                textures: textures_group,
                slot,
                draw: di,
                // By its material's render class: characters from the characters' lights,
                // placeables from the level's, among those whose room reaches the draw's.
                // Level-class draws keep the old rule (`pick_level_lights`): the level's own
                // meshes with the level's domain, an object's with the characters'.
                lights: pick_as_built(
                    &scene.lights,
                    &scene.light_rooms,
                    classes[di],
                    d.sphere,
                    d.room_id,
                    di < scene.static_draws,
                ),
                multipass,
                lod: d.lod,
                sphere: {
                    // The sky and its clouds follow the camera (`sky.vsh`, `clouds.vsh`):
                    // drawn whatever their bounds (the original draws them through their own
                    // submission queue, `sky.sub.xml`, not Umbra's visibility).
                    let sem = &scene.materials[d.material].semantic;
                    if crate::scene::sky_semantic(sem) {
                        None
                    } else {
                        Some(
                            *mesh_spheres
                                .entry(d.mesh)
                                .or_insert_with(|| mesh_sphere(mesh)),
                        )
                    }
                },
            });
        }
        // Shadow casters: the level's meshes whose model says they cast run-time shadows
        // (6176), and the objects' meshes unless their model says they do not; eyes never
        // (`exclude class eye`). Each is drawn with its material's `PSSM` program.
        let mut caster_pipelines = Vec::new();
        let mut caster_programs_used: Vec<Arc<D3dProgram>> = Vec::new();
        let mut caster_by_name: HashMap<String, usize> = HashMap::new();
        let mut pending_casters = Vec::new();
        let empty = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shadow caster textures"),
            entries: &[],
        });
        let empty_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("shadow caster textures"),
            layout: &empty,
            entries: &[],
        });
        if shadows.is_some() {
            for (di, d) in scene.draws.iter().enumerate() {
                let casts = d.cast_shadow.unwrap_or(di >= scene.static_draws);
                let eye = scene.materials[d.material]
                    .render_class
                    .as_deref()
                    .is_some_and(|c| c.split(',').any(|k| k.trim().eq_ignore_ascii_case("eye")));
                if !casts || eye {
                    continue;
                }
                let Some((program, _)) = caster_programs.get(d.material).and_then(Option::as_ref)
                else {
                    continue;
                };
                if !program.ps.interface.samplers.is_empty()
                    || !program.vs.interface.samplers.is_empty()
                {
                    continue;
                }
                let pi = *caster_by_name
                    .entry(program.name.clone())
                    .or_insert_with(|| {
                        caster_pipelines.push(create_pipeline(
                            device,
                            program,
                            &consts_layout,
                            &empty,
                            SHADOW_FORMAT,
                        ));
                        caster_programs_used.push(program.clone());
                        caster_pipelines.len() - 1
                    });
                pending_casters.push((di, pi));
            }
        }
        let caster_buffer = (!pending_casters.is_empty()).then(|| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("shadow caster constants"),
                size: slot_size * 2 * pending_casters.len() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        });
        let mut casters = Vec::new();
        for (k, (di, pi)) in pending_casters.into_iter().enumerate() {
            let d = &scene.draws[di];
            let program = &caster_programs_used[pi];
            let mesh = &scene.meshes[d.mesh];
            let inputs = &program.vs.interface.inputs;
            let sig: String = inputs
                .iter()
                .map(|s| format!("{:?}{}@{};", s.semantic.usage, s.semantic.index, s.location))
                .collect();
            let vertices = cache
                .vertices
                .entry((mesh.key.clone(), sig))
                .or_insert_with(|| {
                    use wgpu::util::DeviceExt;
                    let data = expand_vertices(mesh, inputs);
                    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some(&mesh.key),
                        contents: bytemuck::cast_slice(&data),
                        usage: wgpu::BufferUsages::VERTEX,
                    })
                })
                .clone();
            let buf = caster_buffer.as_ref().expect("buffer exists with casters");
            let slots = [slot_size * 2 * k as u64, slot_size * (2 * k as u64 + 1)];
            let consts = slots.map(|slot| const_group(device, &consts_layout, buf, slot));
            let local = d.transform.inverse().transform_point3(d.sphere.0);
            casters.push(CasterDraw {
                pipeline: pi,
                vertices,
                mesh: d.mesh,
                draw: di,
                consts,
                slots,
                sphere: (local, d.sphere.1),
            });
        }

        // Particle emitters: one draw each, with its material's first pass.
        let mut pending_particles = Vec::new();
        for (ei, e) in scene.emitters.iter().enumerate() {
            let Some(program) = material_programs.get(e.material).and_then(|p| p.first()) else {
                continue;
            };
            // The distortion programs draw into the mask.
            let format = if is_distortion(program) {
                crate::distortion::MASK_FORMAT
            } else {
                color_format
            };
            let pi = *by_name.entry(program.name.clone()).or_insert_with(|| {
                let (pipeline, tl) = cache
                    .pipelines
                    .entry((program.name.clone(), format))
                    .or_insert_with(|| {
                        let tl = texture_layout(device, program);
                        let pipeline =
                            create_pipeline(device, program, &consts_layout, &tl, format);
                        (pipeline, tl)
                    })
                    .clone();
                pipelines.push(D3dPipeline {
                    pipeline,
                    program: program.clone(),
                    blended: program.pass.blend.is_some(),
                });
                layouts.push(tl);
                pipelines.len() - 1
            });
            pending_particles.push((ei, pi));
        }
        let particle_buffer = (!pending_particles.is_empty()).then(|| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("particle constants"),
                size: slot_size * pending_particles.len() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        });
        let quad_indices = (!pending_particles.is_empty()).then(|| {
            use wgpu::util::DeviceExt;
            let quads = crate::particles::MAX_PARTICLES as u32;
            let indices: Vec<u32> = (0..quads)
                .flat_map(|q| [0, 1, 2, 0, 2, 3].map(|k| q * 4 + k))
                .collect();
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("particle quads"),
                contents: bytemuck::cast_slice(&indices),
                usage: wgpu::BufferUsages::INDEX,
            })
        });
        let mut particles = Vec::new();
        for (k, (ei, pi)) in pending_particles.into_iter().enumerate() {
            let e = &scene.emitters[ei];
            let program = pipelines[pi].program.clone();
            let stride = 16 * program.vs.interface.inputs.len().max(1) as u64;
            let vertices = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("particle vertices"),
                size: stride * 4 * crate::particles::MAX_PARTICLES as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let buf = particle_buffer
                .as_ref()
                .expect("buffer exists with particles");
            let slot = slot_size * k as u64;
            let consts = const_group(device, &consts_layout, buf, slot);
            let textures_group = texture_group(
                ctx,
                vfs,
                textures,
                shadows.as_deref(),
                &mut samplers,
                &layouts[pi],
                &program,
                &scene.materials[e.material],
                None,
                None,
            );
            particles.push(ParticleDraw {
                pipeline: pi,
                vertices,
                consts,
                textures: textures_group,
                slot,
                material: e.material,
                emitter: ei,
                count: std::sync::atomic::AtomicU32::new(0),
                distortion: is_distortion(&program),
            });
        }

        // Each object's draws, its attachments' included.
        let mut prop_of: Vec<Option<usize>> = vec![None; scene.draws.len()];
        for (pi, p) in scene.props.iter().enumerate() {
            for range in std::iter::once(&p.draws).chain(p.attachments.iter().map(|a| &a.draws)) {
                for di in range.clone() {
                    if let Some(slot) = prop_of.get_mut(di) {
                        *slot = Some(pi);
                    }
                }
            }
        }
        let mut prop_draws = vec![Vec::new(); scene.props.len()];
        for (k, d) in draws.iter().enumerate() {
            if let Some(Some(p)) = prop_of.get(d.draw) {
                prop_draws[*p].push(k);
            }
        }

        let programs = pipelines.iter().map(|p| p.program.name.clone()).collect();
        let celestial = scene
            .atmosphere
            .as_ref()
            .map(|a| crate::atmosphere::celestial(a, scene.sunlight_direction));
        D3dPass {
            faded: pipelines.iter().map(|_| None).collect(),
            pipelines,
            layouts,
            fade_consts: consts_layout,
            color_format,
            opacity: vec![1.0; scene.draws.len()],
            draws,
            buffer,
            materials: scene.materials.clone(),
            caster_materials: caster_programs
                .iter()
                .map(|c| c.as_ref().map(|(_, m)| m.clone()))
                .collect(),
            worlds: scene
                .draws
                .iter()
                .map(|d| (d.transform, d.lightmap.clone(), d.material))
                .collect(),
            probes: scene.draws.iter().map(|d| d.probe).collect(),
            bones: vec![Vec::new(); scene.draws.len()],
            scene_probes: scene.probes.iter().map(|p| p.probe.clone()).collect(),
            trees: (0..scene.draws.len())
                .map(|i| scene.tree_constants.get(&i).cloned())
                .collect(),
            tree_lods: (0..scene.draws.len())
                .map(|i| scene.tree_lods.get(&i).cloned())
                .collect(),
            tree_frame: std::sync::Mutex::new(vec![None; scene.draws.len()]),
            billboards,
            slot_groups,
            instance_buffer,
            instancing: std::sync::Mutex::new(Instancing::default()),
            wind_time: std::sync::Mutex::new(0.0),
            winds: std::sync::Mutex::new(match scene.wind {
                Some((strength, direction)) if !scene.tree_constants.is_empty() => (0..5)
                    .map(|_| thedas_formats::speedtree_gen::wind::Wind::dao(strength, direction))
                    .collect(),
                _ => Vec::new(),
            }),
            ambients: scene
                .draws
                .iter()
                .zip(&classes)
                .enumerate()
                .map(|(di, (d, &c))| {
                    // The level's own draws: the ambient light nodes reach only models flagged
                    // prop (DAOrigins.exe 0x464310's 4th argument, set for 0x10000), so terrain
                    // and static meshes get none, a placed prop the brightest.
                    if di < scene.static_draws {
                        let own =
                            LightClass::of(scene.materials[d.material].render_class.as_deref());
                        return ambient_of(&scene.lights, own);
                    }
                    match c {
                        LightClass::Level => ambient(&d.room),
                        c => ambient_of(&scene.lights, c),
                    }
                })
                .collect(),
            sun: scene.sun,
            fog: match &scene.layout_fog {
                Some(f) => crate::atmosphere::layout_fog_params(f),
                None => crate::atmosphere::fog_params(scene.atmosphere.as_ref()),
            },
            sky: celestial
                .as_ref()
                .zip(scene.atmosphere.as_ref())
                .map(|(c, a)| crate::atmosphere::sky_params(a, c)),
            clouds: crate::atmosphere::cloud_params(scene.clouds.as_ref(), celestial.as_ref()),
            programs,
            shadows,
            places_partitions: scene.static_draws > 0,
            // A Dragon Age II level (its layout holds its own fog) reads one partition.
            reach: if scene.layout_fog.is_some() {
                crate::shadow::SINGLE_PARTITION_REACH
            } else {
                crate::shadow::partition_reach(scene.detail)
            },
            blur_shadows: scene.detail != crate::scene::Detail::Low,
            caster_pipelines,
            caster_visible: std::sync::Mutex::new(Vec::new()),
            casters_filled: std::sync::Mutex::new(None),
            in_view: std::sync::Mutex::new(Vec::new()),
            statics: std::sync::Mutex::new(Vec::new()),
            written: std::sync::Mutex::new(Vec::new()),
            casters,
            caster_buffer,
            caster_programs: caster_programs_used,
            empty_group,
            particles,
            particle_buffer,
            quad_indices,
            scene_lights: scene.lights.clone(),
            own_lights: scene.lights.len(),
            placed: scene
                .draws
                .iter()
                .enumerate()
                .map(|(di, d)| (d.sphere, d.room_id, di < scene.static_draws))
                .collect(),
            extra_lit: Vec::new(),
            light_rooms: scene.light_rooms.clone(),
            room_boxes: scene.room_boxes.clone(),
            classes,
            prop_draws,
            prop_at: std::sync::Mutex::new(Vec::new()),
            relit: std::sync::Mutex::new(Vec::new()),
            rig: None,
        }
    }

    /// Fills each particle draw's vertices with its emitter's quads (`emitters` indexed as
    /// the scene's) and its constants for this frame. The quads are in world space.
    pub fn update_particles(
        &self,
        ctx: &GpuContext,
        camera: &Camera,
        width: u32,
        height: u32,
        time: f32,
        emitters: &[crate::particles::EmitterState],
    ) {
        let Some(buffer) = &self.particle_buffer else {
            return;
        };
        let aspect = width as f32 / height.max(1) as f32;
        let (view, projection) = (camera.view(), camera.projection(aspect));
        let inv = view.inverse();
        let (right, up) = (inv.x_axis.truncate(), inv.y_axis.truncate());
        let forward = camera.forward();
        let slot_size = SLOT[7] as usize;
        let lights = DrawLights::default();
        let mut quads = Vec::new();
        for d in &self.particles {
            let Some(e) = emitters.get(d.emitter) else {
                d.count.store(0, std::sync::atomic::Ordering::Relaxed);
                continue;
            };
            quads.clear();
            e.quads(right, up, forward, &mut quads);
            let program = &self.pipelines[d.pipeline].program;
            let n = quads.len() / 4;
            d.count
                .store(6 * n as u32, std::sync::atomic::Ordering::Relaxed);
            if n == 0 {
                continue;
            }
            let data =
                particle_vertices(&quads, &program.vs.interface.inputs, (right, up, forward));
            ctx.queue
                .write_buffer(&d.vertices, 0, bytemuck::cast_slice(&data));
            let cc = ConstantContext {
                view,
                projection,
                world: glam::Mat4::IDENTITY,
                camera: camera.position,
                material: &self.materials[d.material],
                lightmap: None,
                lights: &lights,
                ambient: Vec3::ZERO,
                bones: &[],
                irradiance: None,
                sun: self.sun,
                fog: self.fog,
                sky: self.sky.as_ref(),
                clouds: self.clouds,
                time,
                shadow: crate::d3d::NO_SHADOW_DATA,
                partition: None,
                tree: None,
                winds: &[],
            };
            let mut data = vec![0u8; slot_size];
            for (table, base) in [(program.vs.ctab.as_ref(), 0), (program.ps.ctab.as_ref(), 3)] {
                let mut c = StageConstants::default();
                cc.fill_into(table, &mut c, |_| true);
                let f: &[u8] = bytemuck::cast_slice(&c.floats);
                let i: &[u8] = bytemuck::cast_slice(&c.ints);
                let b: &[u8] = bytemuck::cast_slice(&c.bools);
                data[SLOT[base] as usize..][..f.len()].copy_from_slice(f);
                data[SLOT[base + 1] as usize..][..i.len()].copy_from_slice(i);
                data[SLOT[base + 2] as usize..][..b.len()].copy_from_slice(b);
            }
            let fix = [
                1.0 / width.max(1) as f32,
                1.0 / height.max(1) as f32,
                0.0,
                0.0,
            ];
            data[SLOT[6] as usize..][..16].copy_from_slice(bytemuck::cast_slice(&fix));
            ctx.queue.write_buffer(buffer, d.slot, &data);
        }
    }

    /// Draws the particle draws filled by [`D3dPass::update_particles`]: the frame's, or
    /// with `distortion` those of the distortion mask (see [`crate::distortion`]).
    pub fn draw_particles(&self, pass: &mut wgpu::RenderPass<'_>, distortion: bool) {
        let Some(indices) = &self.quad_indices else {
            return;
        };
        for d in &self.particles {
            let count = d.count.load(std::sync::atomic::Ordering::Relaxed);
            if count == 0 || d.distortion != distortion {
                continue;
            }
            pass.set_pipeline(&self.pipelines[d.pipeline].pipeline);
            pass.set_bind_group(0, &d.consts, &[]);
            pass.set_bind_group(1, &d.textures, &[]);
            pass.set_vertex_buffer(0, d.vertices.slice(..));
            pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..count, 0, 0..1);
        }
    }

    /// Whether any distortion particles are to be drawn this frame.
    pub fn has_distortion(&self) -> bool {
        self.particles
            .iter()
            .any(|d| d.distortion && d.count.load(std::sync::atomic::Ordering::Relaxed) > 0)
    }

    /// Sets scene draw `draw`'s skinning palette rows for the next frames.
    pub fn set_bones(&mut self, draw: usize, rows: Vec<[f32; 4]>) {
        if let Some(b) = self.bones.get_mut(draw) {
            *b = rows;
        }
    }

    /// Sets scene draw `draw`'s model-to-world transform for the next frames.
    /// A draw's model-to-world transform as last placed.
    pub fn world(&self, draw: usize) -> Option<glam::Mat4> {
        self.worlds.get(draw).map(|w| w.0)
    }

    pub fn set_world(&mut self, draw: usize, world: glam::Mat4) {
        if let Some(w) = self.worlds.get_mut(draw) {
            w.0 = world;
        }
    }

    /// Adds `extra` to the scene's lights (replacing what was added before; none takes
    /// them away again): a cutscene's own lights, which come, change and go while the pass
    /// stands. A draw's lights are picked again as they were when the pass was made, from
    /// the scene's lights and these; an object that has moved since has its lights
    /// re-picked on the next frame ([`D3dPass::repick_lights`]). The draws whose lights
    /// change have their fixed constants refilled.
    ///
    /// DAOrigins.exe's light manager holds a cutscene's light nodes with the level's and
    /// selects every visible object's lights from them all, keeping a still object's
    /// selection until the manager's version changes (0x464429), which a light coming or
    /// changing does [inferred: the stamps at manager +0xd0 and +0xd4 are not traced].
    pub fn set_extra_lights(&mut self, extra: &[SceneLight]) {
        if self.scene_lights[self.own_lights..] == *extra {
            return;
        }
        self.scene_lights.truncate(self.own_lights);
        self.scene_lights.extend_from_slice(extra);
        let relit = self.relit.get_mut().unwrap_or_else(|e| e.into_inner());
        relit.resize(self.draws.len(), None);
        self.extra_lit.resize(self.draws.len(), None);
        let statics = self.statics.get_mut().unwrap_or_else(|e| e.into_inner());
        for (k, d) in self.draws.iter().enumerate() {
            if relit[k].is_some() {
                continue;
            }
            let Some(&(sphere, room, level_own)) = self.placed.get(d.draw) else {
                continue;
            };
            let class = self
                .classes
                .get(d.draw)
                .copied()
                .unwrap_or(LightClass::Character);
            let lights = pick_as_built(
                &self.scene_lights,
                &self.light_rooms,
                class,
                sphere,
                room,
                level_own,
            );
            let now = (lights != d.lights).then_some(lights);
            if self.extra_lit[k] != now {
                self.extra_lit[k] = now;
                if let Some(s) = statics.get_mut(k) {
                    *s = None;
                }
            }
        }
        // The objects that have moved: where they stood is forgotten, so that the next
        // frame picks their lights again.
        let at = self.prop_at.get_mut().unwrap_or_else(|e| e.into_inner());
        for (p, ks) in self.prop_draws.iter().enumerate() {
            if ks
                .iter()
                .any(|&k| relit.get(k).is_some_and(Option::is_some))
                && let Some(here) = at.get_mut(p).and_then(Option::as_mut)
            {
                *here = Vec3::splat(f32::MAX);
            }
        }
    }

    /// Lights every draw with `rig` instead of the scene's lights, ambient, probe and sun
    /// (`None` goes back to them). The draws' fixed constants are refilled.
    pub fn set_rig(&mut self, rig: Option<crate::scene::LightRig>) {
        self.rig = rig;
        self.statics
            .get_mut()
            .unwrap_or_else(|e| e.into_inner())
            .iter_mut()
            .for_each(|s| *s = None);
    }

    /// Draws scene draw `draw` at `opacity` (0..1) of its material's: its shaders' engine
    /// fade (`mml_fOpacityMultiplier`, which the character pixel shaders write as their
    /// alpha, `Ch1.psh`: `mov oC0.w, c27.x`) is multiplied by it, and below 1 its opaque
    /// passes blend by that alpha. DAOrigins.exe fades a creature the camera is inside to
    /// 0.05 (0x6c4130, the object's `+0x190`); provisional: how the exe blends a faded
    /// opaque pass is not traced.
    pub fn set_opacity(&mut self, ctx: &GpuContext, draw: usize, opacity: f32) {
        let opacity = if opacity.is_finite() {
            opacity.clamp(0.0, 1.0)
        } else {
            1.0
        };
        let Some(o) = self.opacity.get_mut(draw) else {
            return;
        };
        *o = opacity;
        if opacity >= 1.0 {
            return;
        }
        for k in 0..self.draws.len() {
            let d = &self.draws[k];
            if d.draw != draw || self.pipelines[d.pipeline].blended {
                continue;
            }
            let pi = d.pipeline;
            if self.faded[pi].is_none() {
                self.faded[pi] = Some(create_pipeline_faded(
                    &ctx.device,
                    &self.pipelines[pi].program,
                    &self.fade_consts,
                    &self.layouts[pi],
                    self.color_format,
                    true,
                ));
            }
        }
    }

    /// Whether `d` draws faded this frame (see [`Self::set_opacity`]).
    pub fn is_faded(&self, d: &D3dDraw) -> bool {
        self.opacity.get(d.draw).is_some_and(|&o| o < 1.0)
    }

    fn faded_pipeline(&self, d: &D3dDraw) -> Option<&wgpu::RenderPipeline> {
        if !self.is_faded(d) {
            return None;
        }
        self.faded.get(d.pipeline)?.as_ref()
    }

    pub fn is_blended(&self, d: &D3dDraw) -> bool {
        self.pipelines[d.pipeline].blended
    }

    /// Fills this frame's constants of the draws in the camera's view (see [`in_view`]; the
    /// rest are not drawn); `time` is the shaders' `ApplicationTime` in seconds.
    pub fn update(&self, ctx: &GpuContext, camera: &Camera, width: u32, height: u32, time: f32) {
        self.update_with(ctx, camera, width, height, time, false);
    }

    /// Fills every draw's constants as if all were in view, without placing the shadow
    /// partitions or changing what the next frame draws: the first-use work (each draw's
    /// fixed constants, its whole slot) done once at load instead of on the first frames.
    pub fn warm(&self, ctx: &GpuContext, width: u32, height: u32) {
        let camera = Camera::new(Vec3::ZERO, 0.0, 0.0);
        self.update_with(ctx, &camera, width, height, 0.0, true);
    }

    fn update_with(
        &self,
        ctx: &GpuContext,
        camera: &Camera,
        width: u32,
        height: u32,
        time: f32,
        warm: bool,
    ) {
        let aspect = width as f32 / height.max(1) as f32;
        // The level's pass places this frame's partitions; the objects' pass reads them.
        let partitions = match (&self.shadows, self.sun) {
            _ if warm => None,
            (Some(t), Some((dir, _))) => {
                let mine: Vec<(Vec3, f32)> = self
                    .casters
                    .iter()
                    .filter_map(|c| {
                        let (world, _, _) = self.worlds.get(c.draw)?;
                        Some((world.transform_point3(c.sphere.0), c.sphere.1))
                    })
                    .collect();
                if self.places_partitions {
                    let mut casters = mine;
                    casters.extend(
                        t.others
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .iter()
                            .copied(),
                    );
                    let previous = *t.boxes.lock().unwrap_or_else(|e| e.into_inner());
                    let (p, kept) = crate::shadow::partitions_smoothed(
                        camera, aspect, dir, self.reach, &casters, &previous,
                    );
                    *t.boxes.lock().unwrap_or_else(|e| e.into_inner()) = kept;
                    let data = crate::shadow::shadow_data(&p, camera);
                    *t.frame.lock().unwrap_or_else(|e| e.into_inner()) = Some((p, data));
                } else {
                    *t.others.lock().unwrap_or_else(|e| e.into_inner()) = mine;
                }
                t.frame()
            }
            _ => None,
        };
        let shadow = partitions
            .map(|(_, d)| d)
            .unwrap_or(crate::d3d::NO_SHADOW_DATA);
        if let (Some(t), Some((p, _))) = (&self.shadows, partitions) {
            let pass = usize::from(!self.places_partitions);
            let visible: Vec<[bool; 2]> = self
                .casters
                .iter()
                .map(|c| {
                    let (world, _, _) = &self.worlds[c.draw];
                    let centre = world.transform_point3(c.sphere.0);
                    let radius = c.sphere.1 * max_scale(world);
                    // A faded object casts no shadow (DAOrigins.exe 0xa61280: below opacity 1
                    // its render nodes carry the fadeoutobject bit, which no shadow group
                    // draws).
                    let faded = self.opacity.get(c.draw).is_some_and(|&o| o < 1.0);
                    [0, 1].map(|k| !faded && in_partition(&p[k], centre, radius))
                })
                .collect();
            let key = shadow_key(&p, &self.casters, &visible, &self.worlds, &self.bones);
            {
                let mut keys = t.keys.lock().unwrap_or_else(|e| e.into_inner());
                // The level's pass comes first each frame: an objects' pass that no longer
                // runs leaves no key behind.
                if pass == 0 {
                    keys.current[1] = None;
                }
                keys.current[pass] = Some(key);
            }
            let mut filled = self
                .casters_filled
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if let Some(buf) = &self.caster_buffer
                && *filled != Some(key)
            {
                self.update_casters(ctx, buf, camera, &p, &visible);
                *filled = Some(key);
            }
            *self
                .caster_visible
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = visible;
        }
        let Some(buffer) = &self.buffer else { return };
        let (view, projection) = (camera.view(), camera.projection(aspect));
        // Only the draws in view get constants (and are drawn); a skinned draw's bounds
        // follow its bones (see [`skinned_sphere`]).
        let view_proj = projection * view;
        // The winds step to this frame's time, each 0.2 s after the last; a paused clock
        // leaves them where they are.
        let wind_from = {
            let mut last = self.wind_time.lock().unwrap_or_else(|e| e.into_inner());
            let from = *last;
            if !warm {
                *last = time;
            }
            from
        };
        let winds: Vec<crate::d3d::WindFrame> = self
            .winds
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter_mut()
            .enumerate()
            .map(|(k, w)| {
                if !warm {
                    // The simulation is built for frame-sized steps (its followers move
                    // by at most 0.03 s a step), so a jump in time (a headless capture at
                    // a late time, a long stall) is walked in thirtieths of a second, up
                    // to a minute of them.
                    let mut t = wind_from.max(time - 60.0);
                    while time - t > 0.05 {
                        t += 1.0 / 30.0;
                        w.advance(t + 0.2 * k as f32);
                    }
                    w.advance(time + 0.2 * k as f32);
                }
                let rad = std::f32::consts::PI / 180.0;
                crate::d3d::WindFrame {
                    matrices: w.matrices(),
                    leaf_angles: w
                        .leaf_angles()
                        .map(|(rock, rustle)| [rock * rad, rustle * rad]),
                }
            })
            .collect();
        // The trees' levels of detail for this camera.
        let mut tree_frame = self.tree_frame.lock().unwrap_or_else(|e| e.into_inner());
        if !warm {
            for (frame, lod) in tree_frame.iter_mut().zip(&self.tree_lods) {
                *frame = lod.as_ref().and_then(|l| l.frame(camera.position));
            }
            for b in &self.billboards {
                b.update(&ctx.queue, camera.position);
            }
        }
        let in_view: Vec<bool> = self
            .draws
            .iter()
            .map(|d| {
                if warm {
                    return true;
                }
                if self.tree_lods.get(d.draw).is_some_and(|l| l.is_some())
                    && tree_frame.get(d.draw).is_none_or(|f| f.is_none())
                {
                    return false;
                }
                // A draw faded all the way out is not drawn (a cut-away ceiling).
                self.opacity.get(d.draw).is_none_or(|&o| o > 0.0)
                    && d.lod.is_none_or(|l| l.shown(camera.position))
                    && self
                        .world_sphere(d)
                        .is_none_or(|(centre, radius)| in_view(&view_proj, centre, radius))
            })
            .collect();
        if !warm {
            self.group_instances(ctx, &in_view, &tree_frame);
        }
        let slot_size = SLOT[7] as usize;
        // The constants of consecutive slots written whole, in one go.
        let mut run: Vec<u8> = Vec::new();
        let mut run_start = 0u64;
        let put = |data: &mut [u8], c: &StageConstants, base: usize| {
            let f: &[u8] = bytemuck::cast_slice(&c.floats);
            let i: &[u8] = bytemuck::cast_slice(&c.ints);
            let b: &[u8] = bytemuck::cast_slice(&c.bools);
            data[SLOT[base] as usize..][..f.len()].copy_from_slice(f);
            data[SLOT[base + 1] as usize..][..i.len()].copy_from_slice(i);
            data[SLOT[base + 2] as usize..][..b.len()].copy_from_slice(b);
        };
        let mut statics = self.statics.lock().unwrap_or_else(|e| e.into_inner());
        statics.resize_with(self.draws.len(), || None);
        let mut written = self.written.lock().unwrap_or_else(|e| e.into_inner());
        written.resize(self.draws.len(), None);
        let mut relit = self.relit.lock().unwrap_or_else(|e| e.into_inner());
        self.repick_lights(&in_view, &mut statics, &mut relit);
        // The per-frame registers of slots that are otherwise current: (offset, bytes).
        let mut partial: Vec<u8> = Vec::new();
        let mut partial_spans: Vec<(u64, std::ops::Range<usize>)> = Vec::new();
        for (i, (d, _)) in self
            .draws
            .iter()
            .zip(&in_view)
            .enumerate()
            .filter(|(_, (_, v))| **v)
        {
            let (world, lightmap, material) = &self.worlds[d.draw];
            let program = &self.pipelines[d.pipeline].program;
            let rigged = self.rig.as_ref().map(|r| {
                let bound = self
                    .world_sphere(d)
                    .unwrap_or((world.w_axis.truncate(), 1.0));
                (r, pick_rig_lights(&r.lights, bound))
            });
            let cc = ConstantContext {
                view,
                projection,
                world: *world,
                camera: camera.position,
                material: &self.materials[*material],
                lightmap: lightmap.as_ref(),
                lights: match &rigged {
                    Some((_, l)) => l,
                    None => relit[i]
                        .as_ref()
                        .or(self.extra_lit.get(i).and_then(Option::as_ref))
                        .unwrap_or(&d.lights),
                },
                ambient: match &rigged {
                    Some((r, _)) => r.ambient,
                    None => self.ambients.get(d.draw).copied().unwrap_or(Vec3::ZERO),
                },
                bones: self.bones.get(d.draw).map_or(&[][..], |b| b.as_slice()),
                irradiance: match &rigged {
                    Some((r, _)) => r.probe.as_ref(),
                    None => self
                        .probes
                        .get(d.draw)
                        .copied()
                        .flatten()
                        .and_then(|p| self.scene_probes.get(p)),
                },
                sun: if rigged.is_some() { None } else { self.sun },
                fog: self.fog,
                sky: self.sky.as_ref(),
                clouds: self.clouds,
                time,
                shadow,
                partition: None,
                tree: self.trees.get(d.draw).and_then(|t| t.as_ref()),
                winds: &winds,
            };
            // The constants that stay as the draw was made are filled once (see
            // [`ConstantContext::per_frame`]); each frame refills the rest over them.
            let tables = [program.vs.ctab.as_ref(), program.ps.ctab.as_ref()];
            let fresh = statics[i].is_none();
            let interfaces = [&program.vs.interface, &program.ps.interface];
            let mut fixed = statics[i].take().unwrap_or_else(|| {
                let mut stage = 0;
                Box::new(tables.map(|t| {
                    let per_frame: Vec<bool> = t
                        .map(|t| t.constants.iter().map(|c| cc.per_frame(c)).collect())
                        .unwrap_or_default();
                    let mut st = crate::d3d::StageConstants::default();
                    cc.fill_into(t, &mut st, |k| per_frame.get(k) != Some(&true));
                    // A `def` constant holds its register, so a relatively addressed read
                    // (`c60[a0.x]`, the leaf-card corners) finds it there as in D3D9.
                    for &(r, v) in &interfaces[stage].float_defs {
                        if let Some(slot) = st.floats.get_mut(r as usize) {
                            *slot = v;
                        }
                    }
                    stage += 1;
                    (st, per_frame)
                }))
            });
            let opacity = self.opacity.get(d.draw).copied().unwrap_or(1.0);
            let state = (width, height, opacity);
            if !fresh && opacity >= 1.0 && written[i] == Some(state) {
                // Only the per-frame registers: filled over the kept constants (the fixed
                // ones are untouched) and written as one span per register file.
                for (stage, (table, base)) in tables.into_iter().zip([0, 3]).enumerate() {
                    let (st, per_frame) = &mut fixed[stage];
                    cc.fill_into(table, st, |k| per_frame.get(k) == Some(&true));
                    let Some(t) = table else { continue };
                    for (set, regs, offset) in [
                        (
                            thedas_shader::ctab::RegisterSet::Float4,
                            bytemuck::cast_slice::<_, u8>(&st.floats),
                            SLOT[base],
                        ),
                        (
                            thedas_shader::ctab::RegisterSet::Int4,
                            bytemuck::cast_slice::<_, u8>(&st.ints),
                            SLOT[base + 1],
                        ),
                        (
                            thedas_shader::ctab::RegisterSet::Bool,
                            bytemuck::cast_slice::<_, u8>(&st.bools),
                            SLOT[base + 2],
                        ),
                    ] {
                        let span = t
                            .constants
                            .iter()
                            .zip(per_frame.iter())
                            .filter(|(c, pf)| **pf && c.set == set)
                            .map(|(c, _)| {
                                let lo = c.register as usize;
                                lo..lo + c.count.max(1) as usize
                            })
                            .reduce(|a, b| a.start.min(b.start)..a.end.max(b.end));
                        let Some(span) = span else { continue };
                        let (lo, hi) = (span.start * 16, (span.end * 16).min(regs.len()));
                        if lo >= hi {
                            continue;
                        }
                        let at = partial.len();
                        partial.extend_from_slice(&regs[lo..hi]);
                        partial_spans.push((d.slot + offset + lo as u64, at..partial.len()));
                    }
                }
                // A tree's alpha reference moves with its LOD.
                if let Some(Some((_, _, alpha))) = tree_frame.get(d.draw) {
                    let fix = [
                        1.0 / width.max(1) as f32,
                        1.0 / height.max(1) as f32,
                        *alpha,
                        0.0,
                    ];
                    let at = partial.len();
                    partial.extend_from_slice(bytemuck::cast_slice(&fix));
                    partial_spans.push((d.slot + SLOT[6], at..partial.len()));
                }
                statics[i] = Some(fixed);
                continue;
            }
            if run_start + run.len() as u64 != d.slot {
                if !run.is_empty() {
                    ctx.queue.write_buffer(buffer, run_start, &run);
                }
                run.clear();
                run_start = d.slot;
            }
            let at = run.len();
            run.resize(at + slot_size, 0);
            let data = &mut run[at..];
            for (stage, (table, base)) in tables.into_iter().zip([0, 3]).enumerate() {
                let (st, per_frame) = &mut fixed[stage];
                if opacity < 1.0 {
                    let mut c = st.clone();
                    cc.fill_into(table, &mut c, |k| per_frame.get(k) == Some(&true));
                    fade_constants(table, &mut c, opacity);
                    put(data, &c, base);
                } else {
                    cc.fill_into(table, st, |k| per_frame.get(k) == Some(&true));
                    put(data, st, base);
                }
            }
            statics[i] = Some(fixed);
            written[i] = Some(state);
            let fix = [
                1.0 / width.max(1) as f32,
                1.0 / height.max(1) as f32,
                // A tree's alpha reference (84, the full-detail value, until its LOD is
                // worked out).
                match tree_frame.get(d.draw) {
                    Some(Some((_, _, alpha))) => *alpha,
                    _ => 84.0,
                },
                0.0,
            ];
            data[SLOT[6] as usize..][..16].copy_from_slice(bytemuck::cast_slice(&fix));
        }
        if !run.is_empty() {
            ctx.queue.write_buffer(buffer, run_start, &run);
        }
        // One staging upload, then a buffer copy per span: far cheaper than a queue write
        // per span.
        if !partial_spans.is_empty() {
            use wgpu::util::DeviceExt;
            let staging = ctx
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("per-frame constants"),
                    contents: &partial,
                    usage: wgpu::BufferUsages::COPY_SRC,
                });
            let mut encoder = ctx
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("per-frame constants"),
                });
            for (offset, range) in partial_spans {
                encoder.copy_buffer_to_buffer(
                    &staging,
                    range.start as u64,
                    buffer,
                    offset,
                    range.len() as u64,
                );
            }
            ctx.queue.submit([encoder.finish()]);
        }
        if !warm {
            *self.in_view.lock().unwrap_or_else(|e| e.into_inner()) = in_view;
        }
    }

    /// A draw's world-space bounds this frame (its bones' pose for a skinned draw); `None`
    /// for draws never culled.
    fn world_sphere(&self, d: &D3dDraw) -> Option<(Vec3, f32)> {
        let (world, _, _) = &self.worlds[d.draw];
        let sphere = d.sphere?;
        let (centre, radius) = match self.bones.get(d.draw) {
            Some(rows) if !rows.is_empty() => skinned_sphere(sphere, rows),
            _ => sphere,
        };
        Some((world.transform_point3(centre), radius * max_scale(world)))
    }

    /// Re-picks the point lights of the placed objects in view that moved since their
    /// lights were last picked, for all of an object's draws at once from the union of
    /// their bounds; a draw whose lights change has its fixed constants refilled.
    ///
    /// DAOrigins.exe re-selects a moving object's lights every frame it is visible
    /// (0x45bdb0 → 0x4654e0 → 0x464310 → 0x463e40) and keeps a static object's. Its room is
    /// found again from its position ([`crate::scene::room_id_at`]); its draws of one light
    /// class share one selection. Provisional: one bound for all of an object's meshes (the
    /// exe's light set is per render node), and the object's probe stays the one it was
    /// placed with.
    fn repick_lights(
        &self,
        in_view: &[bool],
        statics: &mut [Option<Box<StaticConstants>>],
        relit: &mut Vec<Option<DrawLights>>,
    ) {
        let mut at = self.prop_at.lock().unwrap_or_else(|e| e.into_inner());
        let changed = repick(
            &RepickInput {
                objects: &self.prop_draws,
                in_view,
                lights: &self.scene_lights,
                light_rooms: &self.light_rooms,
                room_at: &|p| crate::scene::room_id_at(&self.room_boxes, p),
                class: &|k| {
                    self.classes
                        .get(self.draws[k].draw)
                        .copied()
                        .unwrap_or(LightClass::Character)
                },
                position: &|k| self.worlds[self.draws[k].draw].0.w_axis.truncate(),
                sphere: &|k| self.world_sphere(&self.draws[k]),
                initial: &|k| {
                    self.extra_lit
                        .get(k)
                        .and_then(Option::as_ref)
                        .unwrap_or(&self.draws[k].lights)
                },
            },
            &mut at,
            relit,
        );
        for k in changed {
            if let Some(s) = statics.get_mut(k) {
                *s = None;
            }
        }
    }

    /// The casters' constants: their world and each partition's matrix.
    fn update_casters(
        &self,
        ctx: &GpuContext,
        buffer: &wgpu::Buffer,
        camera: &Camera,
        partitions: &[crate::shadow::Partition; 2],
        visible: &[[bool; 2]],
    ) {
        // One slot's constants at a time, for the casters a partition draws.
        let slot_size = SLOT[7] as usize;
        let mut data = vec![0u8; slot_size];
        let lights = DrawLights::default();
        let texel = 1.0
            / self
                .shadows
                .as_ref()
                .map_or(crate::shadow::PARTITION_SIZE, |t| t.size()) as f32;
        for (c, seen) in self.casters.iter().zip(visible) {
            let (world, _, material) = &self.worlds[c.draw];
            let program = &self.caster_programs[c.pipeline];
            let Some(Some(material)) = self.caster_materials.get(*material) else {
                continue;
            };
            for (k, slot) in c.slots.iter().enumerate() {
                if !seen[k] {
                    continue;
                }
                data.fill(0);
                let cc = ConstantContext {
                    view: glam::Mat4::IDENTITY,
                    projection: partitions[k].view_proj,
                    world: *world,
                    camera: camera.position,
                    material,
                    lightmap: None,
                    lights: &lights,
                    ambient: Vec3::ZERO,
                    bones: self.bones.get(c.draw).map_or(&[][..], |b| b.as_slice()),
                    irradiance: None,
                    sun: None,
                    fog: self.fog,
                    sky: None,
                    clouds: self.clouds,
                    time: 0.0,
                    shadow: crate::d3d::NO_SHADOW_DATA,
                    partition: Some(partitions[k].view_proj),
                    tree: None,
                    winds: &[],
                };
                for (table, base) in [(program.vs.ctab.as_ref(), 0), (program.ps.ctab.as_ref(), 3)]
                {
                    let c = cc.fill(table);
                    let o = 0;
                    let f: &[u8] = bytemuck::cast_slice(&c.floats);
                    let i: &[u8] = bytemuck::cast_slice(&c.ints);
                    let b: &[u8] = bytemuck::cast_slice(&c.bools);
                    data[o + SLOT[base] as usize..][..f.len()].copy_from_slice(f);
                    data[o + SLOT[base + 1] as usize..][..i.len()].copy_from_slice(i);
                    data[o + SLOT[base + 2] as usize..][..b.len()].copy_from_slice(b);
                }
                let fix = [texel, texel, 0.0, 0.0];
                data[SLOT[6] as usize..][..16].copy_from_slice(bytemuck::cast_slice(&fix));
                ctx.queue.write_buffer(buffer, *slot, &data);
            }
        }
    }

    /// Records this pass's shadow casters into the partitions; `clear` starts them afresh
    /// (the level's pass, before any objects' pass adds its casters). Does nothing without
    /// partitions or casters (but still clears when asked, so no stale shadows remain).
    pub fn encode_shadows(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        meshes: &[crate::gpu::GpuMesh],
        clear: bool,
    ) {
        let Some(t) = &self.shadows else { return };
        if t.frame().is_none() || (!clear && self.casters.is_empty()) {
            return;
        }
        {
            let mut keys = t.keys.lock().unwrap_or_else(|e| e.into_inner());
            if clear {
                // Nothing any pass draws changed since the partitions were last drawn:
                // they are kept as they are.
                keys.keep = keys.drawn == keys.current && keys.current[0].is_some();
                if !keys.keep {
                    keys.drawn = keys.current;
                }
            }
            if keys.keep {
                return;
            }
        }
        let visible = self
            .caster_visible
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        for k in 0..2 {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("shadow partition"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &t.views[k],
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: if clear {
                            wgpu::LoadOp::Clear(wgpu::Color::WHITE)
                        } else {
                            wgpu::LoadOp::Load
                        },
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &t.depth[k],
                    depth_ops: Some(wgpu::Operations {
                        load: if clear {
                            wgpu::LoadOp::Clear(1.0)
                        } else {
                            wgpu::LoadOp::Load
                        },
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            for (i, c) in self.casters.iter().enumerate() {
                if !visible.get(i).is_some_and(|v| v[k]) {
                    continue;
                }
                let Some(mesh) = meshes.get(c.mesh) else {
                    continue;
                };
                pass.set_pipeline(&self.caster_pipelines[c.pipeline]);
                pass.set_bind_group(0, &c.consts[k], &[]);
                pass.set_bind_group(1, &self.empty_group, &[]);
                pass.set_vertex_buffer(0, c.vertices.slice(..));
                pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..mesh.count, 0, 0..1);
            }
        }
    }

    /// After every renderer of the frame recorded its casters: the first partition's blur
    /// (the level's pass, at Medium and High).
    pub fn finish_shadows(&self, encoder: &mut wgpu::CommandEncoder) {
        if let (Some(t), true, true) = (&self.shadows, self.places_partitions, self.blur_shadows) {
            t.blur_first(encoder);
        }
    }

    /// Whether draw `i` (an index into `draws`) is in this frame's view (see
    /// [`D3dPass::update`]); before any update, every draw is.
    pub fn in_view(&self, i: usize) -> bool {
        let v = self.in_view.lock().unwrap_or_else(|e| e.into_inner());
        v.get(i).copied().unwrap_or(v.is_empty())
    }

    /// Groups this frame's instanced draws in view by pipeline, mesh, textures, slot chunk
    /// and index range, and writes the instance list: each group's slots, in draw order.
    fn group_instances(
        &self,
        ctx: &GpuContext,
        in_view: &[bool],
        tree_frame: &[Option<(u32, u32, f32)>],
    ) {
        let Some(list_buffer) = &self.instance_buffer else {
            return;
        };
        type Key = (
            usize,
            usize,
            usize,
            (usize, Option<usize>, Option<usize>),
            (u32, u32),
        );
        let mut by_key: HashMap<Key, usize> = HashMap::new();
        let mut members: Vec<(usize, (u32, u32), Vec<u32>)> = Vec::new();
        for (i, d) in self.draws.iter().enumerate() {
            if !d.instanced || !in_view.get(i).copied().unwrap_or(false) {
                continue;
            }
            let range = match tree_frame.get(d.draw) {
                Some(Some((first, n, _))) => (*first, *n),
                _ => (0, u32::MAX),
            };
            let key = (d.chunk, d.pipeline, d.mesh, d.textures_key, range);
            let g = *by_key.entry(key).or_insert_with(|| {
                members.push((i, range, Vec::new()));
                members.len() - 1
            });
            members[g].2.push(d.local);
        }
        let mut inst = self.instancing.lock().unwrap_or_else(|e| e.into_inner());
        inst.lead.clear();
        inst.lead.resize(self.draws.len(), None);
        inst.groups.clear();
        let mut list: Vec<u32> = Vec::new();
        for (leader, range, slots) in members {
            inst.lead[leader] = Some(inst.groups.len());
            inst.groups
                .push((list.len() as u32, slots.len() as u32, range));
            list.extend(slots);
        }
        if !list.is_empty() {
            ctx.queue
                .write_buffer(list_buffer, 0, bytemuck::cast_slice(&list));
        }
    }

    pub fn draw(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        d: &D3dDraw,
        indices: &wgpu::Buffer,
        count: u32,
    ) {
        if d.instanced {
            // Drawn by its group's leader, with the group's other draws as instances.
            let inst = self.instancing.lock().unwrap_or_else(|e| e.into_inner());
            let Some(&Some(g)) = inst.lead.get(d.index) else {
                return;
            };
            let (Some(&(start, n, (first, len))), Some(consts)) =
                (inst.groups.get(g), self.slot_groups.get(d.chunk))
            else {
                return;
            };
            pass.set_pipeline(&self.pipelines[d.pipeline].pipeline);
            pass.set_bind_group(0, consts, &[]);
            pass.set_bind_group(1, &d.textures, &[]);
            pass.set_vertex_buffer(0, d.vertices.slice(..));
            pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
            let end = first.saturating_add(len).min(count);
            pass.draw_indexed(first.min(end)..end, 0, start..start + n);
            return;
        }
        let faded = self
            .faded_pipeline(d)
            .unwrap_or(&self.pipelines[d.pipeline].pipeline);
        pass.set_pipeline(faded);
        pass.set_bind_group(0, &d.consts, &[]);
        pass.set_bind_group(1, &d.textures, &[]);
        pass.set_vertex_buffer(0, d.vertices.slice(..));
        pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
        // A tree part draws its current level's indices only.
        let range = match self
            .tree_frame
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(d.draw)
        {
            Some(Some((first, n, _))) => *first..(*first + *n).min(count),
            _ => 0..count,
        };
        pass.draw_indexed(range, 0, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn its_shaders_translate_for_every_backend() {
        crate::backend_check::translates("shadow blur", SHADOW_BLUR_WGSL);
    }

    #[test]
    fn lighting_baked_per_vertex_is_read_as_three_texture_coordinates() {
        use thedas_formats::mesh::{IndexFormat, MeshChunk};
        use thedas_shader::translate::Semantic;
        let vertex = |x: f32| crate::scene::Vertex {
            position: [x, 0.0, 0.0],
            normal: [0.0, 0.0, 1.0],
            uv0: [0.25, 0.5],
            uv1: [0.0, 0.0],
        };
        let mesh = SceneMesh {
            key: "piece".into(),
            vertices: vec![vertex(1.0), vertex(2.0)],
            indices: vec![0, 1, 0],
            chunk: MeshChunk {
                name: "piece".into(),
                vertex_size: 0,
                vertex_count: 2,
                index_count: 3,
                primitive: 0,
                index_format: IndexFormat::U16,
                base_vertex: 0,
                vertex_offset: 0,
                start_index: 0,
                elements: vec![],
                bounds: None,
            },
            raw_vertices: vec![],
            positions: None,
            // The second vertex is past the file's end: it reads as unlit.
            vertex_light: Some(vec![[
                [255, 0, 51, 186],
                [0, 255, 0, 186],
                [0, 0, 102, 186],
            ]]),
        };
        let slot = |usage, index, register| IoSlot {
            semantic: Semantic { usage, index },
            register,
            location: register,
        };
        let inputs = [
            slot(Usage::TexCoord, 0, 0),
            slot(Usage::TexCoord, 1, 1),
            slot(Usage::TexCoord, 3, 2),
        ];
        let out = expand_vertices(&mesh, &inputs);
        let near = |a: &[f32], b: [f32; 4]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-6);
        assert_eq!(out.len(), 2 * 3 * 4);
        assert!(near(&out[0..4], [0.25, 0.5, 0.0, 1.0]));
        assert!(near(&out[4..8], [1.0, 0.0, 0.2, 186.0 / 255.0]));
        assert!(near(&out[8..12], [0.0, 0.0, 0.4, 186.0 / 255.0]));
        assert!(near(&out[16..20], [0.0; 4]));
        assert!(near(&out[20..24], [0.0; 4]));
        // A mesh without such lighting reads its own second coordinates.
        let plain = SceneMesh {
            vertex_light: None,
            ..mesh
        };
        assert!(near(
            &expand_vertices(&plain, &inputs)[4..8],
            [0.0, 0.0, 0.0, 1.0]
        ));
    }

    #[test]
    fn fading_scales_only_the_engine_fade() {
        use thedas_shader::ctab::{Constant, ConstantTable, ParamClass, RegisterSet};
        let k = |name: &str, register| Constant {
            name: name.into(),
            set: RegisterSet::Float4,
            register,
            count: 1,
            class: ParamClass::Scalar,
            ty: 3,
            rows: 1,
            columns: 1,
            elements: 1,
            default: None,
        };
        let table = ConstantTable {
            creator: String::new(),
            target: String::new(),
            constants: vec![k("mml_fOpacityMultiplier", 27), k("mml_fSpecularPower", 3)],
        };
        let mut c = crate::d3d::StageConstants::default();
        c.floats[27] = [1.0, 0.0, 0.0, 0.0];
        c.floats[3] = [8.0, 0.0, 0.0, 0.0];
        fade_constants(Some(&table), &mut c, 0.05);
        assert_eq!(c.floats[27][0], 0.05);
        assert_eq!(c.floats[3][0], 8.0);
    }

    #[test]
    fn merged_spheres_hold_both() {
        let a = (Vec3::ZERO, 1.0);
        assert_eq!(merge_spheres(a, (Vec3::X * 0.5, 0.25)), a);
        let (c, r) = merge_spheres(a, (Vec3::X * 4.0, 1.0));
        assert!((c - Vec3::X * 2.0).length() < 1e-6 && (r - 3.0).abs() < 1e-6);
    }

    #[test]
    fn added_lights_join_a_draws_pick_by_its_class() {
        let light = |x: f32, domain: i64, intensity: f32| SceneLight::Point {
            position: Vec3::new(x, 0.0, 0.0),
            color: Vec3::ONE,
            radius: 10.0,
            name: String::new(),
            domain: Some(domain),
            is_static: true,
            version: 0,
            intensity,
            room: None,
        };
        let own = vec![
            light(1.0, DOMAIN_LEVEL, 1.0),
            light(2.0, DOMAIN_CHARACTERS, 1.0),
        ];
        // A cutscene's light model: one light for characters, one for the level.
        let mut all = own.clone();
        all.extend([
            light(3.0, DOMAIN_CHARACTERS, 0.5),
            light(3.0, DOMAIN_LEVEL, 0.5),
        ]);
        let rooms = HashMap::new();
        let sphere = (Vec3::ZERO, 1.0);
        let xs = |lights: &[SceneLight], class, level_own| -> Vec<(f32, f32)> {
            pick_as_built(lights, &rooms, class, sphere, None, level_own)
                .points
                .iter()
                .map(|p| (p.0.x, p.2.x))
                .collect()
        };
        // The level's own draws and the placeables take the level's; characters theirs.
        assert_eq!(xs(&own, LightClass::Level, true), [(1.0, 1.0)]);
        assert_eq!(xs(&all, LightClass::Level, true), [(1.0, 1.0), (3.0, 0.5)]);
        assert_eq!(xs(&all, LightClass::Prop, false), [(1.0, 1.0), (3.0, 0.5)]);
        assert_eq!(
            xs(&all, LightClass::Character, false),
            [(2.0, 1.0), (3.0, 0.5)]
        );
        // Taken away again, the pick is as the pass was made.
        assert_eq!(xs(&all[..2], LightClass::Character, false), [(2.0, 1.0)]);
    }

    #[test]
    fn a_moved_object_repicks_its_lights_and_a_still_one_does_not() {
        let light = |x: f32, name: &str| SceneLight::Point {
            position: Vec3::new(x, 0.0, 0.0),
            color: Vec3::ONE,
            radius: 3.0,
            name: name.into(),
            domain: Some(crate::scene::DOMAIN_CHARACTERS),
            is_static: true,
            version: 0,
            intensity: 1.0,
            room: None,
        };
        let lights = vec![light(0.0, "a"), light(20.0, "b")];
        let rooms = HashMap::new();
        let initial = pick_lights(
            &lights,
            &rooms,
            (Vec3::ZERO, 0.5),
            None,
            LightClass::Character,
        );
        assert_eq!(initial.points[0].0, Vec3::ZERO);
        // Object 0 owns draws 0 and 1; draw 2 is the level's.
        let objects = vec![vec![0, 1]];
        let pos = std::cell::Cell::new(Vec3::ZERO);
        let position = |_k: usize| pos.get();
        let sphere = |_k: usize| Some((pos.get(), 0.5));
        let init = |_k: usize| &initial;
        let input = RepickInput {
            objects: &objects,
            in_view: &[true, true, true],
            lights: &lights,
            light_rooms: &rooms,
            room_at: &|_| None,
            class: &|_| LightClass::Character,
            position: &position,
            sphere: &sphere,
            initial: &init,
        };
        let (mut at, mut relit) = (Vec::new(), Vec::new());
        // First sight: nothing changes; standing still: nothing either.
        assert!(repick(&input, &mut at, &mut relit).is_empty());
        assert!(repick(&input, &mut at, &mut relit).is_empty());
        // Walks next to light B: both its draws take B, the level's draw does not.
        pos.set(Vec3::new(19.0, 0.0, 0.0));
        assert_eq!(repick(&input, &mut at, &mut relit), vec![0, 1]);
        assert_eq!(
            relit[0].as_ref().unwrap().points[0].0,
            Vec3::new(20.0, 0.0, 0.0)
        );
        assert!(relit[2].is_none());
        // A small step that keeps B changes nothing.
        pos.set(Vec3::new(19.5, 0.0, 0.0));
        assert!(repick(&input, &mut at, &mut relit).is_empty());
        // Out of view, it is not re-picked.
        pos.set(Vec3::ZERO);
        let hidden = RepickInput {
            in_view: &[false, false, true],
            ..input
        };
        assert!(repick(&hidden, &mut at, &mut relit).is_empty());
    }

    #[test]
    fn draws_count_as_in_view_only_within_the_frustum() {
        let camera = Camera::new(Vec3::new(10.0, 20.0, 2.0), 0.3, -0.1);
        let vp = camera.view_proj(16.0 / 9.0);
        let ahead = camera.position + camera.forward() * 10.0;
        assert!(in_view(&vp, ahead, 0.5));
        // Behind the camera, or far off to the side: out.
        assert!(!in_view(
            &vp,
            camera.position - camera.forward() * 10.0,
            0.5
        ));
        let side = camera.forward().cross(Vec3::Z).normalize();
        assert!(!in_view(&vp, ahead + side * 100.0, 0.5));
        // A sphere whose centre lies outside but which reaches into the view: in.
        assert!(in_view(&vp, ahead + side * 100.0, 95.0));
        // Beyond the far plane: out.
        assert!(!in_view(
            &vp,
            camera.position + camera.forward() * (camera.far + 50.0),
            1.0
        ));
    }

    #[test]
    fn a_skinned_draws_bounds_follow_its_bones() {
        // Two bones: one at rest, one carrying its vertices 5 m along x.
        let rows = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0, 5.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
        ];
        let (c, r) = skinned_sphere((Vec3::ZERO, 1.0), &rows);
        // Both carried spheres lie within it.
        for at in [Vec3::ZERO, Vec3::new(5.0, 0.0, 0.0)] {
            assert!(c.distance(at) + 1.0 <= r + 1e-4);
        }
        // No bones: the rest sphere.
        assert_eq!(skinned_sphere((Vec3::ONE, 2.0), &[]), (Vec3::ONE, 2.0));
    }

    #[test]
    fn casters_count_for_a_partition_only_within_its_box() {
        let camera = Camera::new(Vec3::new(10.0, 20.0, 2.0), 0.3, -0.1);
        let sun = Vec3::new(-0.48, -0.83, -0.3).normalize();
        let p = crate::shadow::partitions(&camera, 16.0 / 9.0, sun, [5.0, 15.0], &[]);
        let ahead = camera.position + camera.forward() * 3.0;
        // Over the near slice: in the first partition.
        assert!(in_partition(&p[0], ahead, 0.5));
        // Up towards the sun from there, still within the box's reach: in.
        assert!(in_partition(&p[0], ahead - sun * 20.0, 0.5));
        // Far to the side, or behind the slice from the sun: out.
        let side = camera.forward().cross(Vec3::Z) * 100.0;
        assert!(!in_partition(&p[0], ahead + side, 0.5));
        assert!(!in_partition(&p[0], ahead + sun * 100.0, 0.5));
        // A big sphere reaches in from outside.
        assert!(in_partition(&p[0], ahead + side, 100.0));
        // Scale grows a sphere's radius by its largest axis.
        let m = glam::Mat4::from_scale(Vec3::new(1.0, 3.0, 2.0));
        assert_eq!(max_scale(&m), 3.0);
    }

    #[test]
    fn border_colours_map_to_the_nearest_wgpu_offers() {
        use wgpu::SamplerBorderColor as B;
        assert_eq!(border_color(0), B::TransparentBlack);
        assert_eq!(border_color(0xff00_0000), B::OpaqueBlack);
        assert_eq!(border_color(0xffff_ffff), B::OpaqueWhite);
        assert_eq!(
            address(AddressMode::Border, true),
            wgpu::AddressMode::ClampToBorder
        );
        assert_eq!(
            address(AddressMode::Border, false),
            wgpu::AddressMode::ClampToEdge
        );
    }

    #[test]
    fn constant_slot_is_aligned() {
        for off in SLOT {
            assert_eq!(off % ALIGN, 0);
        }
        assert!(SLOT[7] >= 2 * FLOATS + 2 * INTS + 2 * BOOLS + 16);
    }
}
