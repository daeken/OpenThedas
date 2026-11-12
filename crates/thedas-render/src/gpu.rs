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
        // Clamping to a border colour (D3D's `TEXADD_BORDER`: the sky's celestial body) when
        // the adapter can; samplers fall back to clamping to the edge otherwise.
        let wanted =
            wgpu::Features::TEXTURE_COMPRESSION_BC | wgpu::Features::ADDRESS_MODE_CLAMP_TO_BORDER;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("thedas"),
                required_features: adapter.features() & wanted,
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
/// A volume texture (the smoke's `smpDecalVolume`), expanded to RGBA8 with its whole mip
/// chain.
fn upload_volume(ctx: &GpuContext, name: &str, dds: &Dds) -> Option<GpuTexture> {
    let texture = ctx.device.create_texture(&wgpu::TextureDescriptor {
        label: Some(name),
        size: wgpu::Extent3d {
            width: dds.width,
            height: dds.height,
            depth_or_array_layers: dds.depth,
        },
        mip_level_count: dds.mip_count.max(1),
        sample_count: 1,
        dimension: wgpu::TextureDimension::D3,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for mip in 0..dds.mip_count.max(1) {
        let (w, h, d) = dds.mip_size(mip);
        let data = dds.decode_rgba8(0, mip).ok()?;
        ctx.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: mip,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * 4),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: d,
            },
        );
    }
    Some(GpuTexture {
        view: texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D3),
            ..Default::default()
        }),
        kind: dds.kind,
    })
}

fn upload_dds(ctx: &GpuContext, name: &str, dds: &Dds) -> Option<GpuTexture> {
    if dds.kind == DdsKind::Volume {
        return upload_volume(ctx, name, dds);
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
            (T::Tex2D | T::Unknown, DdsKind::Texture2D)
                | (T::Cube, DdsKind::Cube)
                | (T::Volume, DdsKind::Volume)
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

#[derive(Clone)]
pub(crate) struct GpuMesh {
    vertices: wgpu::Buffer,
    pub(crate) indices: wgpu::Buffer,
    pub(crate) count: u32,
}

/// GPU resources kept between renderers built over the same resources (an area's objects,
/// rebuilt as creatures come and go): textures, pipelines and vertex buffers, by name. Clear
/// it (make a new one) when the resource stack changes.
#[derive(Default)]
pub struct RenderCache {
    pub(crate) textures: TextureCache,
    pub(crate) meshes: HashMap<String, GpuMesh>,
    pub(crate) d3d: crate::d3d_gpu::D3dCache,
    /// Translated programs by `vs+ps+state` key (`None`: did not translate).
    pub(crate) programs: HashMap<String, Option<std::sync::Arc<crate::d3d::D3dProgram>>>,
    /// The shadow partitions every renderer made with this cache draws into and reads.
    pub(crate) shadows: Option<std::sync::Arc<crate::d3d_gpu::ShadowTargets>>,
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
    /// The scene's particle emitters, and how many of them have a program to draw with.
    pub emitters: usize,
    pub particle_draws: usize,
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
    /// (scene draw index, draw) in drawing order.
    opaque: Vec<(usize, DrawRef)>,
    blended: Vec<(usize, DrawRef)>,
    /// When set, only these scene draws are drawn (a portrait draws its member alone).
    pub only_draws: Option<std::collections::HashSet<usize>>,
    /// What a frame drawn on its own starts as (black; a paperdoll draws over white too, to
    /// find its coverage).
    pub clear: wgpu::Color,
    depth: Option<(wgpu::TextureView, u32, u32)>,
    /// The level's frame buffer effects, for a renderer of the level's own meshes.
    effects: Option<crate::post::FrameEffects>,
    /// Whether [`AreaRenderer::finish_frame`] draws the level's frame buffer effects (the
    /// original's "Frame-buffer effects" option). On by default.
    pub frame_effects: bool,
    pub light: StandInLight,
    pub stats: RenderStats,
    /// The shaders' `ApplicationTime` in seconds: the graphics clock DAOrigins.exe advances
    /// by every frame's time (0x52ae80), which drifts the clouds.
    time: f64,
    /// Whether [`AreaRenderer::encode_shadows`] already filled this frame's constants.
    updated: bool,
    /// The scene's particle emitters, as [`AreaScene::emitters`] orders them.
    pub emitters: Vec<crate::particles::EmitterState>,
    /// The draws of `CutAway` meshes ([`AreaScene::cut_away`]).
    cut_away: Vec<usize>,
}

/// The colour curve `da_hsvmatrix` reads: `name` (an area's curve, `<name>.dds`) or by
/// default `da_defaultcc` (DAOrigins.exe loads it at 0x458cda, and binds it when no area curve
/// is set, 0x4560d0), its first row as RGBA texels. A curve that does not load falls back to
/// the default (provisional: the exe's failure path is not traced), and that to an identity
/// ramp.
fn color_curve(vfs: &Vfs, name: Option<&str>) -> Vec<u8> {
    let load = |name: &str| {
        let (d, _) = vfs
            .read(&format!("{}.dds", name.to_ascii_lowercase()))
            .ok()?;
        let dds = thedas_formats::dds::Dds::parse(&d).ok()?;
        let px = dds.decode_rgba8(0, 0).ok()?;
        let row = dds.width as usize * 4;
        (row > 0 && px.len() >= row).then(|| px[..row].to_vec())
    };
    name.and_then(load)
        .or_else(|| load("da_defaultcc"))
        .unwrap_or_else(crate::post::identity_curve)
}

/// The colour grading of the area whose `AREA_ID` is `area_id`, from `areadata.gda`.
pub fn area_grade(vfs: &Vfs, area_id: i64) -> crate::post::AreaGrade {
    vfs.read("areadata.gda")
        .ok()
        .and_then(|(d, _)| thedas_formats::gda::Gda::parse(&d).ok())
        .map(|t| crate::post::AreaGrade::from_areadata(&t, area_id))
        .unwrap_or_default()
}

/// The textures `da_fade` reads, `FadeSaturateMask` and `FadeEffectMask` (the materials'
/// defaults for its passes' `s2` and `s1`), or `None` when either does not load.
fn fade_masks(vfs: &Vfs) -> Option<crate::post::FadeMasks> {
    let load = |name: &str| {
        let (d, _) = vfs.read(name).ok()?;
        let dds = thedas_formats::dds::Dds::parse(&d).ok()?;
        Some(crate::post::Image {
            width: dds.width,
            height: dds.height,
            rgba: dds.decode_rgba8(0, 0).ok()?,
        })
    };
    Some(crate::post::FadeMasks {
        saturate: load("fadesaturatemask.dds")?,
        effect: load("fadeeffectmask.dds")?,
    })
}

impl AreaRenderer {
    pub fn new(
        ctx: &GpuContext,
        vfs: &Vfs,
        scene: &AreaScene,
        programs: &ProgramRegistry,
        color_format: wgpu::TextureFormat,
    ) -> Self {
        Self::new_cached(
            ctx,
            vfs,
            scene,
            programs,
            color_format,
            &mut RenderCache::default(),
        )
    }

    /// [`AreaRenderer::new`], taking what it can from `cache` and leaving what it makes there.
    pub fn new_cached(
        ctx: &GpuContext,
        vfs: &Vfs,
        scene: &AreaScene,
        programs: &ProgramRegistry,
        color_format: wgpu::TextureFormat,
        render_cache: &mut RenderCache,
    ) -> Self {
        let device = &ctx.device;
        let mut stats = RenderStats::default();

        // Meshes.
        let meshes: Vec<GpuMesh> = scene
            .meshes
            .iter()
            .map(|m| {
                render_cache
                    .meshes
                    .entry(m.key.clone())
                    .or_insert_with(|| GpuMesh {
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
                    .clone()
            })
            .collect();
        stats.meshes = meshes.len();

        // Shadow casters' programs (their materials' `PSSM` semantics) and the shadow
        // partitions the level's renderers share (see `crate::shadow`); only under a sun.
        let caster_programs: Vec<Option<crate::d3d::CasterMaterial>> = if scene.sun.is_some() {
            scene
                .materials
                .iter()
                .map(|m| crate::d3d::pssm_program(vfs, m, scene.detail.name(), render_cache))
                .collect()
        } else {
            Vec::new()
        };
        // The partitions' size follows the detail (0x455580); a change of size makes them
        // afresh, as the exe's quality setter does (0x4556e0 → 0x453ab0, 0x455580).
        let shadow_size = crate::shadow::partition_size(scene.detail);
        let shadows = scene.sun.is_some().then(|| {
            if render_cache
                .shadows
                .as_ref()
                .is_some_and(|t| t.size() != shadow_size)
            {
                render_cache.shadows = None;
            }
            render_cache
                .shadows
                .get_or_insert_with(|| {
                    std::sync::Arc::new(crate::d3d_gpu::ShadowTargets::new(device, shadow_size))
                })
                .clone()
        });

        // Textures.
        let RenderCache {
            textures: cache,
            d3d: d3d_cache,
            ..
        } = render_cache;
        let (loaded0, compressed0, missing0) =
            (cache.loaded, cache.compressed, cache.missing.len());
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
        let mut d3d_programs: Vec<Vec<std::sync::Arc<crate::d3d::D3dProgram>>> = Vec::new();
        for m in &scene.materials {
            let program = match programs.resolve(m) {
                Resolved::Draw(Program::StandIn(p)) => {
                    d3d_programs.push(Vec::new());
                    p
                }
                Resolved::Draw(Program::D3d(p)) => {
                    d3d_programs.push(p);
                    material_program.push(None);
                    material_group_index.push(0);
                    continue;
                }
                Resolved::Skip => {
                    d3d_programs.push(Vec::new());
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
                if d3d_programs.get(d.material).is_none_or(|p| p.is_empty()) {
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
        let d3d = crate::d3d_gpu::D3dPass::new(
            ctx,
            vfs,
            scene,
            &d3d_programs,
            &caster_programs,
            shadows,
            cache,
            d3d_cache,
            color_format,
        );
        for (k, d) in d3d.draws.iter().enumerate() {
            if d3d.is_blended(d) || d.multipass {
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
        stats.emitters = scene.emitters.len();
        stats.particle_draws = d3d.particles.len();
        stats.programs.extend(d3d.programs.iter().cloned());
        stats.textures_loaded = cache.loaded - loaded0;
        stats.textures_compressed = cache.compressed - compressed0;
        stats.textures_missing = cache.missing[missing0..].to_vec();
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
            opaque,
            blended,
            only_draws: None,
            clear: wgpu::Color::BLACK,
            depth: None,
            // Only a renderer of the level's own meshes runs its effects; the objects' scene
            // draws over it.
            effects: (scene.static_draws > 0)
                .then(|| {
                    crate::post::FrameEffects::new(
                        device,
                        &ctx.queue,
                        &scene.effects,
                        scene.detail,
                        color_format,
                        &color_curve(vfs, None),
                        crate::post::Fade::from_effects(&scene.effects)
                            .and_then(|_| fade_masks(vfs))
                            .as_ref(),
                    )
                })
                .flatten(),
            frame_effects: true,
            time: 0.0,
            updated: false,
            emitters: scene
                .emitters
                .iter()
                .enumerate()
                .map(|(i, e)| {
                    crate::particles::EmitterState::new(
                        e.emitter.clone(),
                        e.transform,
                        crate::particles::seed(i, e.transform),
                    )
                })
                .collect(),
            light: StandInLight::default(),
            stats,
            cut_away: scene.cut_away.clone(),
        }
    }

    /// Grades the frame as area `grade` says: its colour curve and `da_hsvmatrix` values
    /// (DAOrigins.exe swaps them when an area becomes active, 0x67b1b0).
    pub fn set_area_grade(&mut self, ctx: &GpuContext, vfs: &Vfs, grade: &crate::post::AreaGrade) {
        if let Some(effects) = self.effects.as_mut() {
            let curve = color_curve(vfs, grade.curve.as_deref());
            effects.set_grade(&ctx.device, &ctx.queue, &curve, grade);
        }
    }

    /// Moves scene draw `draw` to `transform` (model to world). The lights picked for it at
    /// load stay.
    /// A draw's model-to-world transform as last placed or posed.
    pub fn draw_world(&self, draw: usize) -> Option<glam::Mat4> {
        self.d3d.world(draw)
    }

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

    /// Places and poses `scene`'s prop `prop`: its draws move to `transform`; skinned draws
    /// take their skeleton's pose (`poses[skeleton]`: the rest and posed model-space matrices
    /// per node; a skeleton missing from it stays at rest) through their skinning palette, or, on the stand-in (which does
    /// not skin), follow their first palette bone rigidly; attachments follow their node.
    /// Draws every draw of `scene`'s prop `prop` at `opacity` (see
    /// [`crate::d3d_gpu::D3dPass::set_opacity`]): 1 as its materials say, below 1 faded,
    /// blended over what lies behind it.
    /// Does the first-use work of every draw now (its fixed constants and whole constant
    /// slot) and waits for the GPU to take this renderer's buffers, so the first frames
    /// after a load do not pay for it. `width` and `height` are the frame size expected.
    pub fn warm(&mut self, ctx: &GpuContext, width: u32, height: u32) {
        self.d3d.warm(ctx, width, height);
        ctx.queue.submit([]);
        let _ = ctx.device.poll(wgpu::PollType::wait_indefinitely());
    }

    /// Fades the scene's `CutAway` meshes (ceilings, roofs) to `value`: 1 draws them as
    /// made, between 0 and 1 translucent with that opacity multiplier, 0 not at all
    /// (DAOrigins.exe 0x4670c0 → 0x466930).
    pub fn set_cutaway(&mut self, ctx: &GpuContext, value: f32) {
        for d in self.cut_away.clone() {
            self.d3d.set_opacity(ctx, d, value);
        }
    }

    pub fn set_prop_opacity(
        &mut self,
        ctx: &GpuContext,
        scene: &AreaScene,
        prop: usize,
        opacity: f32,
    ) {
        let Some(p) = scene.props.get(prop) else {
            return;
        };
        for d in p.draws.clone() {
            self.d3d.set_opacity(ctx, d, opacity);
        }
    }

    pub fn pose_prop(
        &mut self,
        ctx: &GpuContext,
        scene: &AreaScene,
        prop: usize,
        transform: glam::Mat4,
        poses: &crate::skeleton::Poses,
    ) {
        let Some(p) = scene.props.get(prop) else {
            return;
        };
        let to_model = p.transform.inverse();
        let attached: std::collections::HashSet<usize> =
            p.attachments.iter().flat_map(|a| a.draws.clone()).collect();
        let pose_of = |s: usize| poses.get(&s);
        for d in p.draws.clone() {
            if attached.contains(&d) {
                continue;
            }
            let Some(item) = scene.draws.get(d) else {
                continue;
            };
            // The draw's placement in the model.
            let in_model = to_model * item.transform;
            let Some(skin) = &item.skin else {
                self.set_transform(ctx, d, transform * in_model);
                continue;
            };
            let Some((rest, pose)) = pose_of(skin.skeleton) else {
                self.set_transform(ctx, d, transform * in_model);
                self.d3d.set_bones(d, Vec::new());
                continue;
            };
            if self.stand_in_of.get(d).copied().flatten().is_some() {
                let follow = skin
                    .palette
                    .iter()
                    .flatten()
                    .next()
                    .filter(|&&n| n < rest.len() && n < pose.len())
                    .map_or(glam::Mat4::IDENTITY, |&n| pose[n] * rest[n].inverse());
                self.set_transform(ctx, d, transform * follow * in_model);
            } else {
                self.set_transform(ctx, d, transform * in_model);
                self.d3d.set_bones(
                    d,
                    crate::skeleton::palette_rows(rest, pose, &skin.palette, skin.mesh_node),
                );
            }
        }
        for a in &p.attachments {
            let Some((rest, pose)) = pose_of(a.skeleton) else {
                for d in a.draws.clone() {
                    if let Some(item) = scene.draws.get(d) {
                        self.set_transform(ctx, d, transform * to_model * item.transform);
                    }
                }
                continue;
            };
            let (Some(r), Some(q)) = (rest.get(a.node), pose.get(a.node)) else {
                continue;
            };
            let follow = *q * r.inverse();
            for d in a.draws.clone() {
                if let Some(item) = scene.draws.get(d) {
                    self.set_transform(ctx, d, transform * follow * to_model * item.transform);
                }
            }
        }
    }

    /// Advances the shaders' clock (`ApplicationTime`) and the particle emitters by a
    /// frame's time, in seconds.
    pub fn advance_time(&mut self, seconds: f64) {
        if seconds.is_finite() && seconds > 0.0 {
            self.time += seconds;
            for e in &mut self.emitters {
                e.advance(seconds as f32);
            }
        }
    }

    /// Lights every draw with `rig` instead of the scene's lights (`None` restores them).
    pub fn set_light_rig(&mut self, rig: Option<crate::scene::LightRig>) {
        self.d3d.set_rig(rig);
    }

    /// Sets the shaders' clock (`ApplicationTime`), in seconds. The particle emitters run
    /// on by the time it moved forward.
    pub fn set_time(&mut self, seconds: f64) {
        let t = if seconds.is_finite() { seconds } else { 0.0 };
        if t > self.time {
            for e in &mut self.emitters {
                e.advance((t - self.time) as f32);
            }
        }
        self.time = t;
    }

    /// The shaders' clock, in seconds.
    pub fn time(&self) -> f64 {
        self.time
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

    /// This renderer's depth buffer for a target of the given size, for a later pass to
    /// draw over (see [`AreaRenderer::render_over`]).
    pub fn depth(&mut self, device: &wgpu::Device, width: u32, height: u32) -> wgpu::TextureView {
        self.depth_view(device, width, height).clone()
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
        self.prepare_shadows(ctx, camera, width, height);
        self.encode_shadows(encoder, true);
        self.finish_shadows(encoder);
        self.render_pass(ctx, encoder, target, width, height, camera, None);
    }

    /// [`AreaRenderer::render`] after [`AreaRenderer::encode_shadows`] recorded this frame's
    /// shadows (with every other renderer's casters).
    pub fn render_main(
        &mut self,
        ctx: &GpuContext,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        width: u32,
        height: u32,
        camera: &Camera,
    ) {
        self.render_pass(ctx, encoder, target, width, height, camera, None);
    }

    /// Fills this frame's constants: placing the shadow partitions (a renderer of the
    /// level's own meshes) and choosing the casters each draws. Call it for every renderer
    /// of a frame, the level's first, before [`AreaRenderer::encode_shadows`] for any of
    /// them: whether the partitions need drawing again depends on all of them.
    pub fn prepare_shadows(&mut self, ctx: &GpuContext, camera: &Camera, width: u32, height: u32) {
        self.d3d
            .update(ctx, camera, width, height, self.time as f32);
        self.updated = true;
    }

    /// Records this renderer's shadow casters into the partitions: `clear` for the first
    /// renderer of a frame (the level's), not for the objects drawn over it. Nothing is
    /// drawn when no renderer's casters, nor the partitions, changed since they were last
    /// drawn: the partitions still hold them.
    pub fn encode_shadows(&mut self, encoder: &mut wgpu::CommandEncoder, clear: bool) {
        self.d3d.encode_shadows(encoder, &self.meshes, clear);
    }

    /// Once every renderer of a frame recorded its casters: what the partitions get then
    /// (the first one's blur at Medium and High), from the renderer of the level's meshes.
    pub fn finish_shadows(&mut self, encoder: &mut wgpu::CommandEncoder) {
        self.d3d.finish_shadows(encoder);
    }

    /// Records this renderer's draws over what an earlier pass drew into `target`, testing
    /// against (and writing) that pass's `depth` (see [`AreaRenderer::depth`]): a level
    /// drawn once, the creatures over it.
    #[allow(clippy::too_many_arguments)]
    pub fn render_over(
        &mut self,
        ctx: &GpuContext,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        width: u32,
        height: u32,
        camera: &Camera,
        depth: &wgpu::TextureView,
    ) {
        self.render_pass(ctx, encoder, target, width, height, camera, Some(depth));
    }

    #[allow(clippy::too_many_arguments)]
    fn render_pass(
        &mut self,
        ctx: &GpuContext,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        width: u32,
        height: u32,
        camera: &Camera,
        over: Option<&wgpu::TextureView>,
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
        if !std::mem::take(&mut self.updated) {
            self.d3d
                .update(ctx, camera, width, height, self.time as f32);
        }
        self.d3d
            .update_particles(ctx, camera, width, height, self.time as f32, &self.emitters);
        let depth = match over {
            Some(d) => d.clone(),
            None => self.depth_view(&ctx.device, width, height).clone(),
        };
        let (color_load, depth_load) = match over {
            Some(_) => (wgpu::LoadOp::Load, wgpu::LoadOp::Load),
            None => (wgpu::LoadOp::Clear(self.clear), wgpu::LoadOp::Clear(1.0)),
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("area"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: color_load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &depth,
                depth_ops: Some(wgpu::Operations {
                    load: depth_load,
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        // Faded objects' opaque draws go after the other opaque ones, so what lies behind
        // them is there to show through.
        let faded =
            |r: &DrawRef| matches!(*r, DrawRef::D3d(i) if self.d3d.is_faded(&self.d3d.draws[i]));
        let opaque = self.opaque.iter().filter(|(_, r)| !faded(r));
        let faded_opaque = self.opaque.iter().filter(|(_, r)| faded(r));
        for (di, r) in opaque.chain(faded_opaque).chain(&self.blended) {
            if self
                .only_draws
                .as_ref()
                .is_some_and(|only| !only.contains(di))
            {
                continue;
            }
            let d = match *r {
                DrawRef::StandIn(i) => &self.stand_in[i],
                DrawRef::D3d(i) => {
                    if !self.d3d.in_view(i) {
                        continue;
                    }
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
        // Particles last, over everything else (the emitters' render classes are
        // transparent).
        if self.only_draws.is_none() {
            self.d3d.draw_particles(&mut pass);
        }
    }

    /// Records the level's frame buffer effects over `target`, a frame this renderer (and
    /// any overlays) drew, before the interface goes on. `target` needs `COPY_SRC` usage.
    /// Does nothing when the level has none or [`AreaRenderer::frame_effects`] is off.
    pub fn finish_frame(
        &mut self,
        ctx: &GpuContext,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::Texture,
        width: u32,
        height: u32,
    ) {
        if !self.frame_effects {
            return;
        }
        if let Some(e) = self.effects.as_mut() {
            e.apply(
                &ctx.device,
                &ctx.queue,
                encoder,
                target,
                width,
                height,
                self.time as f32,
            );
        }
    }

    /// Renders one frame offscreen and returns tightly packed RGBA8 pixels.
    ///
    /// It writes this renderer's per-draw constants and submits at once: not to be called
    /// between [`AreaRenderer::render`] encoding a frame and that frame's submission, which
    /// would then draw with this capture's camera.
    pub fn capture(
        &mut self,
        ctx: &GpuContext,
        camera: &Camera,
        width: u32,
        height: u32,
    ) -> Result<Vec<u8>, GpuError> {
        self.capture_with(ctx, camera, width, height, &mut [])
    }

    /// [`AreaRenderer::capture`] with `overlays` drawn over this renderer's pass (see
    /// [`AreaRenderer::render_over`]).
    pub fn capture_with(
        &mut self,
        ctx: &GpuContext,
        camera: &Camera,
        width: u32,
        height: u32,
        overlays: &mut [&mut AreaRenderer],
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
        self.prepare_shadows(ctx, camera, width, height);
        for o in overlays.iter_mut() {
            o.prepare_shadows(ctx, camera, width, height);
        }
        self.encode_shadows(&mut encoder, true);
        for o in overlays.iter_mut() {
            o.encode_shadows(&mut encoder, false);
        }
        self.finish_shadows(&mut encoder);
        self.render_pass(ctx, &mut encoder, &view, width, height, camera, None);
        if !overlays.is_empty() {
            let depth = self.depth(&ctx.device, width, height);
            for o in overlays.iter_mut() {
                o.render_over(ctx, &mut encoder, &view, width, height, camera, &depth);
            }
        }
        self.finish_frame(ctx, &mut encoder, &target, width, height);
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

    /// A level can be built on a worker thread (the load screen's) and handed back: the
    /// renderer moves between threads and the GPU context is shared.
    #[test]
    fn renderers_build_on_worker_threads() {
        fn send<T: Send>() {}
        fn shared<T: Send + Sync>() {}
        send::<AreaRenderer>();
        send::<crate::scene::AreaScene>();
        send::<RenderCache>();
        shared::<GpuContext>();
        shared::<thedas_vfs::Vfs>();
    }
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
                positions: None,
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
                room_id: None,
                probe: None,
                skin: None,
                cast_shadow: None,
                lod: None,
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
