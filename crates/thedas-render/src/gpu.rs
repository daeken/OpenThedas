//! The wgpu side: device setup, texture upload, pipelines and the draw loop for an
//! [`AreaScene`], plus offscreen capture.

use std::collections::HashMap;

use thedas_formats::dds::{Dds, DdsFormat, DdsKind};
use thedas_vfs::Vfs;
use wgpu::util::DeviceExt;

use crate::camera::Camera;
use crate::material::{BlendMode, MaterialProgram, Program, ProgramRegistry, Resolved};
use crate::scene::{AreaScene, Vertex};

#[derive(Debug, thiserror::Error)]
pub enum GpuError {
    #[error("no suitable GPU adapter: {0}")]
    Adapter(String),
    #[error("device request failed: {0}")]
    Device(String),
    #[error("readback failed: {0}")]
    Readback(String),
}

/// Depth buffer format used by every area pass.
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// Format for offscreen captures. Not sRGB: D3D9 wrote shader output to the back buffer
/// unconverted, so we keep values as they come out of the shader.
pub const CAPTURE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

pub struct GpuContext {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    /// Whether BC1-3 textures can be uploaded compressed.
    pub bc: bool,
}

impl GpuContext {
    /// Creates a device; pass the surface a window will present to, or `None` for headless.
    pub async fn new(
        instance: wgpu::Instance,
        surface: Option<&wgpu::Surface<'_>>,
    ) -> Result<Self, GpuError> {
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: surface,
                ..Default::default()
            })
            .await
            .map_err(|e| GpuError::Adapter(e.to_string()))?;
        let bc = adapter
            .features()
            .contains(wgpu::Features::TEXTURE_COMPRESSION_BC);
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("thedas"),
                required_features: if bc {
                    wgpu::Features::TEXTURE_COMPRESSION_BC
                } else {
                    wgpu::Features::empty()
                },
                required_limits: wgpu::Limits::default(),
                ..Default::default()
            })
            .await
            .map_err(|e| GpuError::Device(e.to_string()))?;
        log::info!(
            "GPU: {} ({:?}), BC textures: {bc}",
            adapter.get_info().name,
            adapter.get_info().backend
        );
        Ok(Self {
            instance,
            adapter,
            device,
            queue,
            bc,
        })
    }

    /// A device with no window, for captures and tests.
    pub fn headless() -> Result<Self, GpuError> {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        pollster::block_on(Self::new(instance, None))
    }
}

pub(crate) struct GpuTexture {
    pub view: wgpu::TextureView,
    pub kind: DdsKind,
}

/// Uploads a 2D or cube DDS (all mips), compressed when the device allows it.
fn upload_dds(ctx: &GpuContext, name: &str, dds: &Dds) -> Option<GpuTexture> {
    if dds.kind == DdsKind::Volume {
        return None;
    }
    let faces = dds.faces();
    let compressed =
        ctx.bc && dds.format.is_compressed() && dds.width % 4 == 0 && dds.height % 4 == 0;
    let format = match (compressed, dds.format) {
        (true, DdsFormat::Bc1) => wgpu::TextureFormat::Bc1RgbaUnorm,
        (true, DdsFormat::Bc2) => wgpu::TextureFormat::Bc2RgbaUnorm,
        (true, DdsFormat::Bc3) => wgpu::TextureFormat::Bc3RgbaUnorm,
        _ => wgpu::TextureFormat::Rgba8Unorm,
    };
    // A compressed chain stops at the last level that is still a whole number of blocks in
    // wgpu's eyes; smaller levels are rarely sampled at our distances.
    let mips = if compressed {
        (0..dds.mip_count)
            .take_while(|&m| {
                let (w, h, _) = dds.mip_size(m);
                m == 0 || (w % 4 == 0 && h % 4 == 0)
            })
            .count() as u32
    } else {
        dds.mip_count
    };
    let texture = ctx.device.create_texture(&wgpu::TextureDescriptor {
        label: Some(name),
        size: wgpu::Extent3d {
            width: dds.width,
            height: dds.height,
            depth_or_array_layers: faces,
        },
        mip_level_count: mips.max(1),
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for (face, mip) in (0..faces).flat_map(|f| (0..mips.max(1)).map(move |m| (f, m))) {
        let (w, h, _) = dds.mip_size(mip);
        let (data, bytes_per_row, rows) = if compressed {
            let s = dds.surface(face, mip).ok()?;
            let bw = w.div_ceil(4);
            (
                s.data.to_vec(),
                bw * dds.format.unit_size() as u32,
                h.div_ceil(4),
            )
        } else {
            (dds.decode_rgba8(face, mip).ok()?, w * 4, h)
        };
        ctx.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: mip,
                origin: wgpu::Origin3d {
                    x: 0,
                    y: 0,
                    z: face,
                },
                aspect: wgpu::TextureAspect::All,
            },
            &data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: Some(rows),
            },
            wgpu::Extent3d {
                width: if compressed { w.div_ceil(4) * 4 } else { w },
                height: if compressed { h.div_ceil(4) * 4 } else { h },
                depth_or_array_layers: 1,
            },
        );
    }
    let dimension = if dds.kind == DdsKind::Cube {
        Some(wgpu::TextureViewDimension::Cube)
    } else {
        Some(wgpu::TextureViewDimension::D2)
    };
    Some(GpuTexture {
        view: texture.create_view(&wgpu::TextureViewDescriptor {
            dimension,
            ..Default::default()
        }),
        kind: dds.kind,
    })
}

/// Textures loaded from the resource stack, by name, plus solid fallbacks per texture type.
#[derive(Default)]
pub(crate) struct TextureCache {
    map: HashMap<String, Option<GpuTexture>>,
    solids: HashMap<(u8, [u8; 4]), wgpu::TextureView>,
    pub loaded: usize,
    pub compressed: usize,
    pub missing: Vec<String>,
}

impl TextureCache {
    pub fn get(&mut self, ctx: &GpuContext, vfs: &Vfs, name: &str) -> Option<&GpuTexture> {
        if !self.map.contains_key(name) {
            let tex = match vfs.read(name) {
                Ok((data, _)) => match Dds::parse(&data) {
                    Ok(dds) => {
                        let t = upload_dds(ctx, name, &dds);
                        if t.is_some() {
                            self.loaded += 1;
                            if ctx.bc && dds.format.is_compressed() {
                                self.compressed += 1;
                            }
                        }
                        t
                    }
                    Err(e) => {
                        log::warn!("{name}: {e}");
                        None
                    }
                },
                Err(_) => None,
            };
            if tex.is_none() {
                self.missing.push(name.to_string());
            }
            self.map.insert(name.to_string(), tex);
        }
        self.map.get(name).and_then(|t| t.as_ref())
    }

    /// A view of a texture if it exists and matches the sampler's texture type.
    pub fn view(
        &mut self,
        ctx: &GpuContext,
        vfs: &Vfs,
        name: &str,
        ty: thedas_shader::bytecode::TextureType,
    ) -> Option<wgpu::TextureView> {
        use thedas_shader::bytecode::TextureType as T;
        let t = self.get(ctx, vfs, name)?;
        let ok = matches!(
            (ty, t.kind),
            (T::Tex2D | T::Unknown, DdsKind::Texture2D) | (T::Cube, DdsKind::Cube)
        );
        ok.then(|| t.view.clone())
    }

    /// A 1x1 texture (all six faces for cubes) of one colour, matching a sampler type.
    pub fn solid(
        &mut self,
        ctx: &GpuContext,
        ty: thedas_shader::bytecode::TextureType,
        rgba: [u8; 4],
    ) -> wgpu::TextureView {
        use thedas_shader::bytecode::TextureType as T;
        let kind = match ty {
            T::Cube => 1u8,
            T::Volume => 2,
            _ => 0,
        };
        self.solids
            .entry((kind, rgba))
            .or_insert_with(|| {
                let (layers, dim, vdim) = match kind {
                    1 => (
                        6,
                        wgpu::TextureDimension::D2,
                        wgpu::TextureViewDimension::Cube,
                    ),
                    2 => (
                        1,
                        wgpu::TextureDimension::D3,
                        wgpu::TextureViewDimension::D3,
                    ),
                    _ => (
                        1,
                        wgpu::TextureDimension::D2,
                        wgpu::TextureViewDimension::D2,
                    ),
                };
                let data: Vec<u8> = (0..layers).flat_map(|_| rgba).collect();
                let texture = ctx.device.create_texture_with_data(
                    &ctx.queue,
                    &wgpu::TextureDescriptor {
                        label: Some("solid"),
                        size: wgpu::Extent3d {
                            width: 1,
                            height: 1,
                            depth_or_array_layers: layers,
                        },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: dim,
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        usage: wgpu::TextureUsages::TEXTURE_BINDING,
                        view_formats: &[],
                    },
                    wgpu::util::TextureDataOrder::LayerMajor,
                    &data,
                );
                texture.create_view(&wgpu::TextureViewDescriptor {
                    dimension: Some(vdim),
                    ..Default::default()
                })
            })
            .clone()
    }
}

fn solid_texture(ctx: &GpuContext, rgba: [u8; 4]) -> GpuTexture {
    let texture = ctx.device.create_texture_with_data(
        &ctx.queue,
        &wgpu::TextureDescriptor {
            label: Some("solid"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        &rgba,
    );
    GpuTexture {
        view: texture.create_view(&wgpu::TextureViewDescriptor::default()),
        kind: DdsKind::Texture2D,
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct FrameUniforms {
    view_proj: [[f32; 4]; 4],
    camera: [f32; 4],
    light: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct DrawUniforms {
    model: [[f32; 4]; 4],
    lightmap: [f32; 4],
    flags: [f32; 4],
}

/// Dynamic uniform offsets must be multiples of this on every backend we target.
const DRAW_STRIDE: u64 = 256;

/// Stand-in lighting constants (provisional, see `standin.wgsl`).
#[derive(Clone, Copy, Debug)]
pub struct StandInLight {
    pub lightmap_rgb_scale: f32,
    pub lightmap_alpha_weight: f32,
    pub unlit_level: f32,
}

impl Default for StandInLight {
    fn default() -> Self {
        Self {
            lightmap_rgb_scale: 2.0,
            lightmap_alpha_weight: 0.6,
            unlit_level: 0.8,
        }
    }
}

struct GpuMesh {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    count: u32,
}

#[derive(Clone, Copy)]
enum DrawRef {
    StandIn(usize),
    D3d(usize),
}

struct Draw {
    mesh: usize,
    pipeline: usize,
    material_group: usize,
    lightmap_group: usize,
    uniform_offset: u32,
}

/// Counts from loading, for logs and reports.
#[derive(Clone, Debug, Default)]
pub struct RenderStats {
    pub meshes: usize,
    pub textures_loaded: usize,
    pub textures_compressed: usize,
    pub textures_missing: Vec<String>,
    pub draws: usize,
    pub draws_skipped: usize,
    pub programs: Vec<String>,
}

pub struct AreaRenderer {
    color_format: wgpu::TextureFormat,
    frame_buffer: wgpu::Buffer,
    frame_group: wgpu::BindGroup,
    draw_buffer: wgpu::Buffer,
    /// Each scene draw's stand-in draw, if it has one.
    stand_in_of: Vec<Option<usize>>,
    draw_group: wgpu::BindGroup,
    meshes: Vec<GpuMesh>,
    pipelines: Vec<(wgpu::RenderPipeline, BlendMode)>,
    material_groups: Vec<wgpu::BindGroup>,
    lightmap_groups: Vec<wgpu::BindGroup>,
    stand_in: Vec<Draw>,
    d3d: crate::d3d_gpu::D3dPass,
    opaque: Vec<DrawRef>,
    blended: Vec<DrawRef>,
    depth: Option<(wgpu::TextureView, u32, u32)>,
    pub light: StandInLight,
    pub stats: RenderStats,
}

impl AreaRenderer {
    pub fn new(
        ctx: &GpuContext,
        vfs: &Vfs,
        scene: &AreaScene,
        programs: &ProgramRegistry,
        color_format: wgpu::TextureFormat,
    ) -> Self {
        let device = &ctx.device;
        let mut stats = RenderStats::default();

        // Meshes.
        let meshes: Vec<GpuMesh> = scene
            .meshes
            .iter()
            .map(|m| GpuMesh {
                vertices: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some(&m.key),
                    contents: bytemuck::cast_slice(&m.vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                }),
                indices: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some(&m.key),
                    contents: bytemuck::cast_slice(&m.indices),
                    usage: wgpu::BufferUsages::INDEX,
                }),
                count: m.indices.len() as u32,
            })
            .collect();
        stats.meshes = meshes.len();

        // Textures.
        let mut cache = TextureCache::default();
        let mut textures: HashMap<String, Option<wgpu::TextureView>> = HashMap::new();
        for name in scene.textures() {
            let view = cache.get(ctx, vfs, &name).map(|t| t.view.clone());
            textures.insert(name, view);
        }
        let grey = solid_texture(ctx, [160, 160, 160, 255]);
        let white = solid_texture(ctx, [255, 255, 255, 255]);

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("wrap"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            anisotropy_clamp: 8,
            ..Default::default()
        });
        let lm_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("lightmap"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        // Layouts.
        let uniform_entry = |dynamic: bool| wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: dynamic,
                min_binding_size: None,
            },
            count: None,
        };
        let frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("frame"),
            entries: &[uniform_entry(false)],
        });
        let draw_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("draw"),
            entries: &[uniform_entry(true)],
        });
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("texture"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("area"),
            bind_group_layouts: &[
                Some(&frame_layout),
                Some(&texture_layout),
                Some(&texture_layout),
                Some(&draw_layout),
            ],
            immediate_size: 0,
        });

        let frame_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("frame"),
            size: std::mem::size_of::<FrameUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let frame_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("frame"),
            layout: &frame_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: frame_buffer.as_entire_binding(),
            }],
        });
        let texture_group = |view: &wgpu::TextureView, smp: &wgpu::Sampler| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &texture_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(smp),
                    },
                ],
            })
        };

        // Programs and pipelines, one per distinct program name.
        let mut pipeline_index: HashMap<String, usize> = HashMap::new();
        let mut pipelines = Vec::new();
        let mut modules: HashMap<String, wgpu::ShaderModule> = HashMap::new();
        let mut material_program: Vec<Option<usize>> = Vec::new();
        let mut material_groups = Vec::new();
        let mut material_group_index: Vec<usize> = Vec::new();
        let mut d3d_programs: Vec<Option<std::sync::Arc<crate::d3d::D3dProgram>>> = Vec::new();
        for m in &scene.materials {
            let program = match programs.resolve(m) {
                Resolved::Draw(Program::StandIn(p)) => {
                    d3d_programs.push(None);
                    p
                }
                Resolved::Draw(Program::D3d(p)) => {
                    d3d_programs.push(Some(p));
                    material_program.push(None);
                    material_group_index.push(0);
                    continue;
                }
                Resolved::Skip => {
                    d3d_programs.push(None);
                    material_program.push(None);
                    material_group_index.push(0);
                    continue;
                }
            };
            let idx = *pipeline_index
                .entry(program.name.clone())
                .or_insert_with(|| {
                    let module = modules.entry(program.wgsl.to_string()).or_insert_with(|| {
                        device.create_shader_module(wgpu::ShaderModuleDescriptor {
                            label: Some(&program.name),
                            source: wgpu::ShaderSource::Wgsl(program.wgsl.clone()),
                        })
                    });
                    pipelines.push((
                        create_pipeline(device, &pipeline_layout, module, &program, color_format),
                        program.blend,
                    ));
                    stats.programs.push(program.name.clone());
                    pipelines.len() - 1
                });
            material_program.push(Some(idx));
            let diffuse = m
                .texture("mml_tDiffuse")
                .and_then(|n| textures.get(&n).and_then(|t| t.as_ref()))
                .unwrap_or(&grey.view);
            material_groups.push(texture_group(diffuse, &sampler));
            material_group_index.push(material_groups.len() - 1);
        }

        // Lightmap groups: one per atlas, plus a white fallback at index 0.
        let mut lightmap_groups = vec![texture_group(&white.view, &lm_sampler)];
        let mut lightmap_index: HashMap<String, usize> = HashMap::new();

        // Draws and their uniforms.
        let mut uniforms: Vec<u8> = Vec::new();
        // (scene draw index, draw) per list.
        let mut opaque: Vec<(usize, DrawRef)> = Vec::new();
        let mut blended: Vec<(usize, DrawRef)> = Vec::new();
        let mut stand_in: Vec<Draw> = Vec::new();
        let mut stand_in_of = vec![None; scene.draws.len()];
        for (di, d) in scene.draws.iter().enumerate() {
            let Some(pipeline) = material_program.get(d.material).copied().flatten() else {
                if !matches!(d3d_programs.get(d.material), Some(Some(_))) {
                    stats.draws_skipped += 1;
                }
                continue;
            };
            let (lightmap_group, lm, has_lm) = match &d.lightmap {
                Some(l) => match textures.get(&l.texture).and_then(|t| t.as_ref()) {
                    Some(t) => {
                        let g = *lightmap_index.entry(l.texture.clone()).or_insert_with(|| {
                            lightmap_groups.push(texture_group(t, &lm_sampler));
                            lightmap_groups.len() - 1
                        });
                        (g, [l.offset[0], l.offset[1], l.scale[0], l.scale[1]], 1.0)
                    }
                    None => (0, [0.0, 0.0, 1.0, 1.0], 0.0),
                },
                None => (0, [0.0, 0.0, 1.0, 1.0], 0.0),
            };
            let offset = uniforms.len() as u32;
            let u = DrawUniforms {
                model: d.transform.to_cols_array_2d(),
                lightmap: lm,
                flags: [has_lm, 0.0, 0.0, 0.0],
            };
            uniforms.extend_from_slice(bytemuck::bytes_of(&u));
            uniforms.resize(uniforms.len().next_multiple_of(DRAW_STRIDE as usize), 0);
            let draw = Draw {
                mesh: d.mesh,
                pipeline,
                material_group: material_group_index[d.material],
                lightmap_group,
                uniform_offset: offset,
            };
            stand_in_of[di] = Some(stand_in.len());
            if pipelines[pipeline].1 == BlendMode::AlphaBlend {
                blended.push((di, DrawRef::StandIn(stand_in.len())));
            } else {
                opaque.push((di, DrawRef::StandIn(stand_in.len())));
            }
            stand_in.push(draw);
        }
        let d3d =
            crate::d3d_gpu::D3dPass::new(ctx, vfs, scene, &d3d_programs, &mut cache, color_format);
        for (k, d) in d3d.draws.iter().enumerate() {
            if d3d.is_blended(d) {
                blended.push((d.draw, DrawRef::D3d(k)));
            } else {
                opaque.push((d.draw, DrawRef::D3d(k)));
            }
        }
        // Blended draws keep scene order; opaque ones are grouped to cut state changes.
        blended.sort_by_key(|(di, _)| *di);
        opaque.sort_by_key(|(_, r)| match r {
            DrawRef::StandIn(i) => (0, stand_in[*i].pipeline, stand_in[*i].material_group),
            DrawRef::D3d(i) => (1, d3d.draws[*i].pipeline, 0),
        });
        stats.draws = opaque.len() + blended.len();
        stats.programs.extend(d3d.programs.iter().cloned());
        stats.textures_loaded = cache.loaded;
        stats.textures_compressed = cache.compressed;
        stats.textures_missing = cache.missing.clone();
        if uniforms.is_empty() {
            uniforms.resize(DRAW_STRIDE as usize, 0);
        }
        let draw_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("draws"),
            contents: &uniforms,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let draw_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("draws"),
            layout: &draw_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &draw_buffer,
                    offset: 0,
                    size: wgpu::BufferSize::new(std::mem::size_of::<DrawUniforms>() as u64),
                }),
            }],
        });

        AreaRenderer {
            color_format,
            frame_buffer,
            frame_group,
            draw_buffer,
            stand_in_of,
            draw_group,
            meshes,
            pipelines,
            material_groups,
            lightmap_groups,
            stand_in,
            d3d,
            opaque: opaque.into_iter().map(|(_, r)| r).collect(),
            blended: blended.into_iter().map(|(_, r)| r).collect(),
            depth: None,
            light: StandInLight::default(),
            stats,
        }
    }

    /// Moves scene draw `draw` to `transform` (model to world). The lights picked for it at
    /// load stay.
    pub fn set_transform(&mut self, ctx: &GpuContext, draw: usize, transform: glam::Mat4) {
        if let Some(Some(i)) = self.stand_in_of.get(draw) {
            let model = transform.to_cols_array_2d();
            ctx.queue.write_buffer(
                &self.draw_buffer,
                self.stand_in[*i].uniform_offset as u64,
                bytemuck::bytes_of(&model),
            );
        }
        self.d3d.set_world(draw, transform);
    }

    /// Moves every draw of `scene`'s prop `prop` from where the scene placed it to
    /// `transform`.
    pub fn move_prop(
        &mut self,
        ctx: &GpuContext,
        scene: &AreaScene,
        prop: usize,
        transform: glam::Mat4,
    ) {
        let Some(p) = scene.props.get(prop) else {
            return;
        };
        let delta = transform * p.transform.inverse();
        for d in p.draws.clone() {
            if let Some(item) = scene.draws.get(d) {
                self.set_transform(ctx, d, delta * item.transform);
            }
        }
    }

    pub fn color_format(&self) -> wgpu::TextureFormat {
        self.color_format
    }

    fn depth_view(&mut self, device: &wgpu::Device, width: u32, height: u32) -> &wgpu::TextureView {
        let stale = !matches!(&self.depth, Some((_, w, h)) if *w == width && *h == height);
        if stale {
            let tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("depth"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: DEPTH_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            self.depth = Some((
                tex.create_view(&wgpu::TextureViewDescriptor::default()),
                width,
                height,
            ));
        }
        &self.depth.as_ref().expect("depth view just created").0
    }

    /// Records the area pass into `encoder`, drawing into `target` of the given size.
    pub fn render(
        &mut self,
        ctx: &GpuContext,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        width: u32,
        height: u32,
        camera: &Camera,
    ) {
        let frame = FrameUniforms {
            view_proj: camera
                .view_proj(width as f32 / height.max(1) as f32)
                .to_cols_array_2d(),
            camera: camera.position.extend(1.0).to_array(),
            light: [
                self.light.lightmap_rgb_scale,
                self.light.lightmap_alpha_weight,
                self.light.unlit_level,
                0.0,
            ],
        };
        ctx.queue
            .write_buffer(&self.frame_buffer, 0, bytemuck::bytes_of(&frame));
        self.d3d.update(ctx, camera, width, height);
        let depth = self.depth_view(&ctx.device, width, height).clone();
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("area"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        for r in self.opaque.iter().chain(&self.blended) {
            let d = match *r {
                DrawRef::StandIn(i) => &self.stand_in[i],
                DrawRef::D3d(i) => {
                    let d = &self.d3d.draws[i];
                    let mesh = &self.meshes[d.mesh];
                    self.d3d.draw(&mut pass, d, &mesh.indices, mesh.count);
                    continue;
                }
            };
            let mesh = &self.meshes[d.mesh];
            pass.set_bind_group(0, &self.frame_group, &[]);
            pass.set_pipeline(&self.pipelines[d.pipeline].0);
            pass.set_bind_group(1, &self.material_groups[d.material_group], &[]);
            pass.set_bind_group(2, &self.lightmap_groups[d.lightmap_group], &[]);
            pass.set_bind_group(3, &self.draw_group, &[d.uniform_offset]);
            pass.set_vertex_buffer(0, mesh.vertices.slice(..));
            pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..mesh.count, 0, 0..1);
        }
    }

    /// Renders one frame offscreen and returns tightly packed RGBA8 pixels.
    pub fn capture(
        &mut self,
        ctx: &GpuContext,
        camera: &Camera,
        width: u32,
        height: u32,
    ) -> Result<Vec<u8>, GpuError> {
        let target = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("capture"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.color_format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        let row = (width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let readback = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: row as u64 * height as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = ctx
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("capture"),
            });
        self.render(ctx, &mut encoder, &view, width, height, camera);
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &target,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        ctx.queue.submit([encoder.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        readback.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        ctx.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| GpuError::Readback(e.to_string()))?;
        rx.recv()
            .map_err(|e| GpuError::Readback(e.to_string()))?
            .map_err(|e| GpuError::Readback(e.to_string()))?;
        let mapped = readback
            .slice(..)
            .get_mapped_range()
            .map_err(|e| GpuError::Readback(format!("{e:?}")))?;
        let bgra = matches!(
            self.color_format,
            wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
        );
        let mut out = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height as usize {
            let line = &mapped[y * row as usize..y * row as usize + width as usize * 4];
            if bgra {
                for px in line.chunks_exact(4) {
                    out.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
                }
            } else {
                out.extend_from_slice(line);
            }
        }
        // Captures are opaque images.
        for px in out.chunks_exact_mut(4) {
            px[3] = 255;
        }
        Ok(out)
    }
}

fn create_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::PipelineLayout,
    module: &wgpu::ShaderModule,
    program: &MaterialProgram,
    color_format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    let blend = match program.blend {
        BlendMode::AlphaBlend => Some(wgpu::BlendState::ALPHA_BLENDING),
        _ => None,
    };
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(&program.name),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some(program.vs_entry),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<Vertex>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![
                    0 => Float32x3, 1 => Float32x3, 2 => Float32x2, 3 => Float32x2
                ],
            })],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            cull_mode: program.cull_back.then_some(wgpu::Face::Back),
            ..Default::default()
        },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(program.blend != BlendMode::AlphaBlend),
            depth_compare: Some(wgpu::CompareFunction::LessEqual),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: Some(program.fs_entry),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: color_format,
                blend,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::{DrawItem, SceneMaterial, SceneMesh};
    use glam::{Mat4, Vec3};
    use thedas_formats::MaterialObject;
    use thedas_formats::mesh::{IndexFormat, MeshChunk};

    /// One triangle in front of the camera, drawn with the stand-in and no textures (so the
    /// grey fallback and the flat light level apply). Skipped when no GPU adapter exists, as
    /// on most CI runners.
    #[test]
    fn renders_a_synthetic_triangle() {
        let ctx = match GpuContext::headless() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("skipping GPU test: {e}");
                return;
            }
        };
        let v = |x: f32, y: f32| Vertex {
            position: [5.0, x, y],
            normal: [-1.0, 0.0, 0.0],
            uv0: [0.0, 0.0],
            uv1: [0.0, 0.0],
        };
        let chunk = MeshChunk {
            name: "tri".into(),
            vertex_size: 0,
            vertex_count: 3,
            index_count: 3,
            primitive: 0,
            index_format: IndexFormat::U16,
            base_vertex: 0,
            vertex_offset: 0,
            start_index: 0,
            elements: vec![],
            bounds: None,
        };
        let xml = br#"<MaterialObject Name="t"><Material Name="static.mat"/></MaterialObject>"#;
        let scene = AreaScene {
            level: "test".into(),
            meshes: vec![SceneMesh {
                key: "tri".into(),
                vertices: vec![v(-2.0, -2.0), v(2.0, -2.0), v(0.0, 2.0)],
                indices: vec![0, 1, 2],
                chunk,
                raw_vertices: vec![],
            }],
            materials: vec![SceneMaterial {
                mao: "t.mao".into(),
                object: MaterialObject::parse(xml).unwrap(),
                definition: None,
                semantic: "Default".into(),
                render_class: None,
                technique: None,
            }],
            draws: vec![DrawItem {
                mesh: 0,
                material: 0,
                transform: Mat4::IDENTITY,
                lightmap: None,
                sphere: (Vec3::new(5.0, 0.0, 0.0), 3.0),
                model: "tri".into(),
                room: "room".into(),
                probe: None,
            }],
            ..Default::default()
        };
        let mut r = AreaRenderer::new(
            &ctx,
            &Vfs::new(),
            &scene,
            &ProgramRegistry::default(),
            CAPTURE_FORMAT,
        );
        assert_eq!(r.stats.draws, 1);
        let camera = Camera::new(Vec3::ZERO, 0.0, 0.0);
        let px = r.capture(&ctx, &camera, 32, 32).unwrap();
        let at = |x: usize, y: usize| &px[(y * 32 + x) * 4..(y * 32 + x) * 4 + 4];
        // Grey 160 times the flat light level 0.8 = 128.
        let centre = at(16, 16);
        assert!((120..=136).contains(&centre[0]), "centre {centre:?}");
        assert_eq!(at(1, 1), &[0, 0, 0, 255], "corner is the clear colour");
    }
}
