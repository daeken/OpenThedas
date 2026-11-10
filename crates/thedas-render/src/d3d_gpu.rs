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
use crate::scene::{AreaScene, SceneLight, SceneMaterial, SceneMesh};

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
}

pub(crate) struct D3dPass {
    pipelines: Vec<D3dPipeline>,
    pub draws: Vec<D3dDraw>,
    buffer: Option<wgpu::Buffer>,
    materials: Vec<SceneMaterial>,
    worlds: Vec<(glam::Mat4, Option<crate::scene::LightmapRef>, usize)>,
    /// Each scene draw's irradiance probe.
    probes: Vec<Option<usize>>,
    /// Each scene draw's skinning palette rows (empty: every bone at rest).
    bones: Vec<Vec<[f32; 4]>>,
    scene_probes: Vec<thedas_formats::probe::IrradianceProbe>,
    ambient: Vec3,
    pub programs: Vec<String>,
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

fn address(a: AddressMode) -> wgpu::AddressMode {
    match a {
        AddressMode::Wrap => wgpu::AddressMode::Repeat,
        AddressMode::Mirror | AddressMode::MirrorOnce => wgpu::AddressMode::MirrorRepeat,
        // Border needs a device feature; clamp is the closest portable match.
        AddressMode::Clamp | AddressMode::Border => wgpu::AddressMode::ClampToEdge,
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
    device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("d3d sampler"),
        address_mode_u: address(s.address_u),
        address_mode_v: address(s.address_v),
        address_mode_w: address(s.address_w),
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
                compare(pass.z_func)
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

impl D3dPass {
    /// `material_programs[i]` holds the programs (one per pass) for scene material `i`;
    /// empty when it is not drawn with its original shaders.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        ctx: &GpuContext,
        vfs: &Vfs,
        scene: &AreaScene,
        material_programs: &[Vec<Arc<D3dProgram>>],
        textures: &mut TextureCache,
        color_format: wgpu::TextureFormat,
    ) -> Self {
        let device = &ctx.device;
        let consts_layout = const_layout(device);
        let mut pipelines: Vec<D3dPipeline> = Vec::new();
        let mut layouts: Vec<wgpu::BindGroupLayout> = Vec::new();
        let mut by_name: HashMap<String, usize> = HashMap::new();
        let mut samplers: HashMap<String, wgpu::Sampler> = HashMap::new();
        let mut vbufs: HashMap<(usize, String), wgpu::Buffer> = HashMap::new();
        let ambient = scene
            .lights
            .iter()
            .filter_map(|l| match l {
                SceneLight::Ambient { color, .. } => Some(*color),
                _ => None,
            })
            .fold(Vec3::ZERO, |a, c| a + c);

        let mut pending = Vec::new();
        for (di, d) in scene.draws.iter().enumerate() {
            let Some(passes) = material_programs.get(d.material) else {
                continue;
            };
            for program in passes {
                let pi = *by_name.entry(program.name.clone()).or_insert_with(|| {
                    let tl = texture_layout(device, program);
                    let pipeline =
                        create_pipeline(device, program, &consts_layout, &tl, color_format);
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
        for (k, (di, pi, multipass)) in pending.into_iter().enumerate() {
            let d = &scene.draws[di];
            let program = pipelines[pi].program.clone();
            let mesh = &scene.meshes[d.mesh];
            let inputs = &program.vs.interface.inputs;
            let sig: String = inputs
                .iter()
                .map(|s| format!("{:?}{}@{};", s.semantic.usage, s.semantic.index, s.location))
                .collect();
            let vertices = vbufs
                .entry((d.mesh, sig))
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
            let range = |i: usize, size: u64| {
                wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: buf,
                    offset: slot + SLOT[i],
                    size: wgpu::BufferSize::new(size),
                })
            };
            let consts = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("d3d draw constants"),
                layout: &consts_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: translate::binding_float(Stage::Vertex),
                        resource: range(0, FLOATS),
                    },
                    wgpu::BindGroupEntry {
                        binding: translate::binding_int(Stage::Vertex),
                        resource: range(1, INTS),
                    },
                    wgpu::BindGroupEntry {
                        binding: translate::binding_bool(Stage::Vertex),
                        resource: range(2, BOOLS),
                    },
                    wgpu::BindGroupEntry {
                        binding: translate::binding_float(Stage::Pixel),
                        resource: range(3, FLOATS),
                    },
                    wgpu::BindGroupEntry {
                        binding: translate::binding_int(Stage::Pixel),
                        resource: range(4, INTS),
                    },
                    wgpu::BindGroupEntry {
                        binding: translate::binding_bool(Stage::Pixel),
                        resource: range(5, BOOLS),
                    },
                    wgpu::BindGroupEntry {
                        binding: translate::BINDING_FIXUP,
                        resource: range(6, 16),
                    },
                ],
            });

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
                    TextureSource::White => None,
                    TextureSource::Black => None,
                }
                .unwrap_or_else(|| {
                    textures.solid(
                        ctx,
                        ty,
                        if source == TextureSource::White {
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
                lights: pick_lights(&scene.lights, d.sphere),
                multipass,
            });
        }
        let programs = pipelines.iter().map(|p| p.program.name.clone()).collect();
        D3dPass {
            pipelines,
            draws,
            buffer,
            materials: scene.materials.clone(),
            worlds: scene
                .draws
                .iter()
                .map(|d| (d.transform, d.lightmap.clone(), d.material))
                .collect(),
            probes: scene.draws.iter().map(|d| d.probe).collect(),
            bones: vec![Vec::new(); scene.draws.len()],
            scene_probes: scene.probes.iter().map(|p| p.probe.clone()).collect(),
            ambient,
            programs,
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

    /// Fills every draw's constants for this frame.
    pub fn update(&self, ctx: &GpuContext, camera: &Camera, width: u32, height: u32) {
        let Some(buffer) = &self.buffer else { return };
        let aspect = width as f32 / height.max(1) as f32;
        let (view, projection) = (camera.view(), camera.projection(aspect));
        let mut data = vec![0u8; buffer.size() as usize];
        let put = |data: &mut [u8], at: u64, c: &StageConstants, base: usize| {
            let f: &[u8] = bytemuck::cast_slice(&c.floats);
            let i: &[u8] = bytemuck::cast_slice(&c.ints);
            let b: &[u8] = bytemuck::cast_slice(&c.bools);
            let o = at as usize;
            data[o + SLOT[base] as usize..][..f.len()].copy_from_slice(f);
            data[o + SLOT[base + 1] as usize..][..i.len()].copy_from_slice(i);
            data[o + SLOT[base + 2] as usize..][..b.len()].copy_from_slice(b);
        };
        for d in &self.draws {
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
                ambient: self.ambient,
                bones: self.bones.get(d.draw).map_or(&[][..], |b| b.as_slice()),
                irradiance: self
                    .probes
                    .get(d.draw)
                    .copied()
                    .flatten()
                    .and_then(|p| self.scene_probes.get(p)),
            };
            put(&mut data, d.slot, &cc.fill(program.vs.ctab.as_ref()), 0);
            put(&mut data, d.slot, &cc.fill(program.ps.ctab.as_ref()), 3);
            let fix = [
                1.0 / width.max(1) as f32,
                1.0 / height.max(1) as f32,
                0.0,
                0.0,
            ];
            data[(d.slot + SLOT[6]) as usize..][..16].copy_from_slice(bytemuck::cast_slice(&fix));
        }
        ctx.queue.write_buffer(buffer, 0, &data);
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
    fn constant_slot_is_aligned() {
        for off in SLOT {
            assert_eq!(off % ALIGN, 0);
        }
        assert!(SLOT[7] >= 2 * FLOATS + 2 * INTS + 2 * BOOLS + 16);
    }
}
