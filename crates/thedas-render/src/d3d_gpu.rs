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
    ConstantContext, D3dProgram, DrawLights, StageConstants, TextureSource, pick_lights,
    sampler_texture,
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
}

/// Pipelines and vertex buffers kept between passes (see [`crate::gpu::RenderCache`]).
#[derive(Default)]
pub(crate) struct D3dCache {
    /// The constants layout every cached pipeline was made with.
    consts_layout: Option<wgpu::BindGroupLayout>,
    /// By program name and target format: the pipeline and its texture layout.
    pipelines:
        HashMap<(String, wgpu::TextureFormat), (wgpu::RenderPipeline, wgpu::BindGroupLayout)>,
    /// By mesh key and vertex input signature.
    vertices: HashMap<(String, String), wgpu::Buffer>,
}

pub(crate) struct D3dPass {
    pipelines: Vec<D3dPipeline>,
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
    /// Each scene draw's ambient light (its room's).
    ambients: Vec<Vec3>,
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
    /// (anything else that changes an object's look rebuilds its pass). A setter for any of
    /// the fixed inputs must clear the draw's entry here.
    statics: std::sync::Mutex<Vec<Option<Box<StaticConstants>>>>,
    caster_programs: Vec<Arc<D3dProgram>>,
    /// The (empty) texture bindings of the casters' programs.
    empty_group: wgpu::BindGroup,
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
            let value = c
                .element(usage, slot.semantic.index as u32)
                .and_then(|e| {
                    let at = v * stride + e.offset as usize;
                    let bytes = mesh.raw_vertices.get(at..at + e.ty.size() as usize)?;
                    Some(e.ty.decode(bytes))
                })
                .or_else(|| {
                    // Meshes built in code (tests) have no raw streams; fall back to the
                    // stand-in vertex.
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
            entry(translate::BINDING_FIXUP, v),
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

fn create_pipeline(
    device: &wgpu::Device,
    program: &D3dProgram,
    consts: &wgpu::BindGroupLayout,
    textures: &wgpu::BindGroupLayout,
    color_format: wgpu::TextureFormat,
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
    let blend = pass.blend.map(|(src, dst, op)| {
        let c = wgpu::BlendComponent {
            src_factor: blend_factor(src),
            dst_factor: blend_factor(dst),
            operation: blend_op(op),
        };
        wgpu::BlendState { color: c, alpha: c }
    });
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
            bias: wgpu::DepthBiasState::default(),
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

/// The ambient light of each room: the sum of the ambient lights its light model holds. A
/// draw in a room with none (or outside every room) takes the average over the rooms that
/// have one. Provisional: the original sums the ambient light nodes its light manager picks
/// for the object (see [`crate::d3d`]), filtered by a light visibility list not traced
/// here; summing every room's ambient into one, as before, washed out levels of many rooms.
/// A level with no ambient light nodes (Lothering) gets none, as in the original.
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
        let mut pipelines: Vec<D3dPipeline> = Vec::new();
        let mut layouts: Vec<wgpu::BindGroupLayout> = Vec::new();
        let mut by_name: HashMap<String, usize> = HashMap::new();
        let mut samplers: HashMap<String, wgpu::Sampler> = HashMap::new();
        let ambient = room_ambients(scene);

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
                            let pipeline =
                                create_pipeline(device, program, &consts_layout, &tl, color_format);
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
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        });

        let mut draws = Vec::new();
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
            let buf = buffer.as_ref().expect("buffer exists when there are draws");
            let slot = slot_size * k as u64;
            let consts = const_group(device, &consts_layout, buf, slot);

            // Textures and samplers.
            let material = &scene.materials[d.material];
            let mut views: Vec<(u32, wgpu::TextureView, String)> = Vec::new();
            for (stage, s, ty) in texture_entries(&program) {
                let table = if stage == Stage::Vertex {
                    program.vs.ctab.as_ref()
                } else {
                    program.ps.ctab.as_ref()
                };
                let (sampler_name, source) =
                    sampler_texture(table, s, ty, material, d.lightmap.as_ref());
                let view = match &source {
                    TextureSource::Resource(name) => textures.view(ctx, vfs, name, ty),
                    TextureSource::Shadow(i) => shadows
                        .as_ref()
                        .and_then(|t| t.views.get(*i as usize).cloned()),
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
                let settings = sampler_name
                    .as_deref()
                    .and_then(|n| program.state.sampler(n).ok())
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
            let textures_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("d3d draw textures"),
                layout: &layouts[pi],
                entries: &entries,
            });
            draws.push(D3dDraw {
                pipeline: pi,
                vertices,
                mesh: d.mesh,
                consts,
                textures: textures_group,
                slot,
                draw: di,
                // The level's own meshes come first; the placed objects and creatures
                // after them are lit as characters.
                lights: pick_lights(
                    &scene.lights,
                    d.sphere,
                    if di < scene.static_draws {
                        DOMAIN_LEVEL
                    } else {
                        DOMAIN_CHARACTERS
                    },
                ),
                multipass,
                sphere: {
                    // The sky and its clouds follow the camera (`sky.vsh`, `clouds.vsh`):
                    // drawn whatever their bounds (the original draws them through their own
                    // submission queue, `sky.sub.xml`, not Umbra's visibility).
                    let sem = &scene.materials[d.material].semantic;
                    if sem.eq_ignore_ascii_case("Sky") || sem.eq_ignore_ascii_case("Clouds") {
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

        let programs = pipelines.iter().map(|p| p.program.name.clone()).collect();
        let celestial = scene
            .atmosphere
            .as_ref()
            .map(|a| crate::atmosphere::celestial(a, scene.sunlight_direction));
        D3dPass {
            pipelines,
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
            ambients: scene.draws.iter().map(|d| ambient(&d.room)).collect(),
            sun: scene.sun,
            fog: crate::atmosphere::fog_params(scene.atmosphere.as_ref()),
            sky: celestial
                .as_ref()
                .zip(scene.atmosphere.as_ref())
                .map(|(c, a)| crate::atmosphere::sky_params(a, c)),
            clouds: crate::atmosphere::cloud_params(scene.clouds.as_ref(), celestial.as_ref()),
            programs,
            shadows,
            places_partitions: scene.static_draws > 0,
            reach: crate::shadow::partition_reach(scene.detail),
            blur_shadows: scene.detail != crate::scene::Detail::Low,
            caster_pipelines,
            caster_visible: std::sync::Mutex::new(Vec::new()),
            casters_filled: std::sync::Mutex::new(None),
            in_view: std::sync::Mutex::new(Vec::new()),
            statics: std::sync::Mutex::new(Vec::new()),
            casters,
            caster_buffer,
            caster_programs: caster_programs_used,
            empty_group,
        }
    }

    /// Sets scene draw `draw`'s skinning palette rows for the next frames.
    pub fn set_bones(&mut self, draw: usize, rows: Vec<[f32; 4]>) {
        if let Some(b) = self.bones.get_mut(draw) {
            *b = rows;
        }
    }

    /// Sets scene draw `draw`'s model-to-world transform for the next frames.
    pub fn set_world(&mut self, draw: usize, world: glam::Mat4) {
        if let Some(w) = self.worlds.get_mut(draw) {
            w.0 = world;
        }
    }

    pub fn is_blended(&self, d: &D3dDraw) -> bool {
        self.pipelines[d.pipeline].blended
    }

    /// Fills this frame's constants of the draws in the camera's view (see [`in_view`]; the
    /// rest are not drawn); `time` is the shaders' `ApplicationTime` in seconds.
    pub fn update(&self, ctx: &GpuContext, camera: &Camera, width: u32, height: u32, time: f32) {
        let aspect = width as f32 / height.max(1) as f32;
        // The level's pass places this frame's partitions; the objects' pass reads them.
        let partitions = match (&self.shadows, self.sun) {
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
                    [0, 1].map(|k| in_partition(&p[k], centre, radius))
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
        let in_view: Vec<bool> = self
            .draws
            .iter()
            .map(|d| {
                let (world, _, _) = &self.worlds[d.draw];
                let Some(sphere) = d.sphere else {
                    return true;
                };
                let (centre, radius) = match self.bones.get(d.draw) {
                    Some(rows) if !rows.is_empty() => skinned_sphere(sphere, rows),
                    _ => sphere,
                };
                in_view(
                    &view_proj,
                    world.transform_point3(centre),
                    radius * max_scale(world),
                )
            })
            .collect();
        let slot_size = SLOT[7] as usize;
        // The constants of consecutive slots in view, written in one go.
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
        for (i, (d, _)) in self
            .draws
            .iter()
            .zip(&in_view)
            .enumerate()
            .filter(|(_, (_, v))| **v)
        {
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
            let (world, lightmap, material) = &self.worlds[d.draw];
            let program = &self.pipelines[d.pipeline].program;
            let cc = ConstantContext {
                view,
                projection,
                world: *world,
                camera: camera.position,
                material: &self.materials[*material],
                lightmap: lightmap.as_ref(),
                lights: &d.lights,
                ambient: self.ambients.get(d.draw).copied().unwrap_or(Vec3::ZERO),
                bones: self.bones.get(d.draw).map_or(&[][..], |b| b.as_slice()),
                irradiance: self
                    .probes
                    .get(d.draw)
                    .copied()
                    .flatten()
                    .and_then(|p| self.scene_probes.get(p)),
                sun: self.sun,
                fog: self.fog,
                sky: self.sky.as_ref(),
                clouds: self.clouds,
                time,
                shadow,
                partition: None,
            };
            // The constants that stay as the draw was made are filled once (see
            // [`ConstantContext::per_frame`]); each frame refills the rest over them.
            let tables = [program.vs.ctab.as_ref(), program.ps.ctab.as_ref()];
            let fixed = statics[i].take().unwrap_or_else(|| {
                Box::new(tables.map(|t| {
                    let per_frame: Vec<bool> = t
                        .map(|t| t.constants.iter().map(|c| cc.per_frame(c)).collect())
                        .unwrap_or_default();
                    let mut st = crate::d3d::StageConstants::default();
                    cc.fill_into(t, &mut st, |k| per_frame.get(k) != Some(&true));
                    (st, per_frame)
                }))
            });
            for (stage, (table, base)) in tables.into_iter().zip([0, 3]).enumerate() {
                let (st, per_frame) = &fixed[stage];
                let mut c = st.clone();
                cc.fill_into(table, &mut c, |k| per_frame.get(k) == Some(&true));
                put(data, &c, base);
            }
            statics[i] = Some(fixed);
            let fix = [
                1.0 / width.max(1) as f32,
                1.0 / height.max(1) as f32,
                0.0,
                0.0,
            ];
            data[SLOT[6] as usize..][..16].copy_from_slice(bytemuck::cast_slice(&fix));
        }
        if !run.is_empty() {
            ctx.queue.write_buffer(buffer, run_start, &run);
        }
        *self.in_view.lock().unwrap_or_else(|e| e.into_inner()) = in_view;
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

    pub fn draw(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        d: &D3dDraw,
        indices: &wgpu::Buffer,
        count: u32,
    ) {
        pass.set_pipeline(&self.pipelines[d.pipeline].pipeline);
        pass.set_bind_group(0, &d.consts, &[]);
        pass.set_bind_group(1, &d.textures, &[]);
        pass.set_vertex_buffer(0, d.vertices.slice(..));
        pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..count, 0, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
