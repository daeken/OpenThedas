//! The wgpu renderer.
//!
//! Geometry is tessellated once per shape character and once per text layout and kept on the
//! GPU; a frame only writes one small uniform per draw. A movie whose display list did not
//! change since the last render is not drawn again (see [`Renderer::render_if_dirty`]).

use std::collections::HashMap;
use std::sync::Arc;

use wgpu::util::DeviceExt;

use crate::display::{DisplayId, Kind};
use crate::player::Player;
use crate::swf::{BlendMode, Character, ColorTransform, Matrix};
use crate::tess::{self, Mesh, Paint, Vertex};

/// Supplies images by name (an atlas file name such as `Quickbar_I50.dds`, or a URL a movie
/// loads): straight RGBA8 with its size.
pub trait Images {
    fn image(&mut self, name: &str) -> Option<(u32, u32, Vec<u8>)>;
}

/// The draw shader. `{TEXTURES}` and `{SAMPLE}` are filled in for adapters with and without
/// texture arrays (see [`shader_source`]).
const SHADER: &str = r#"
struct Draw {
    m0: vec4<f32>,
    m1: vec4<f32>,
    g0: vec4<f32>,
    g1: vec4<f32>,
    mul: vec4<f32>,
    add: vec4<f32>,
    // x: paint (0 solid, 1 bitmap, 2-4 gradients); y: destination blend; z: texture slot;
    // w: sampler.
    mode: vec4<f32>,
    // A rectangular mask in target pixels (x0, y0, x1, y1); all zeros for none.
    clip: vec4<f32>,
    // The target's size in pixels (for edge anti-aliasing); zero to skip it.
    view: vec4<f32>,
};
struct Vert {
    pos: vec2<f32>,
    uv: vec2<f32>,
    extrude: vec2<f32>,
    color: u32,
    pad: u32,
};
@group(0) @binding(0) var<storage, read> draws: array<Draw>;
@group(0) @binding(1) var<storage, read> verts: array<Vert>;
@group(0) @binding(2) var<storage, read> stream: array<vec2<u32>>;
{TEXTURES}
@group(2) @binding(0) var backdrop: texture_2d<f32>;

struct V {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) local: vec2<f32>,
    @location(3) @interpolate(flat) rec: u32,
};

@vertex
fn vs(@builtin(vertex_index) vi: u32) -> V {
    let e = stream[vi];
    let v = verts[e.x];
    let d = draws[e.y];
    var o: V;
    var x = d.m0.x * v.pos.x + d.m0.y * v.pos.y + d.m0.z;
    var y = d.m1.x * v.pos.x + d.m1.y * v.pos.y + d.m1.z;
    // Edge anti-aliasing: an outer fringe vertex moves one pixel out along its normal.
    if (any(v.extrude != vec2<f32>(0.0)) && d.view.x > 0.0) {
        let half = d.view.xy * 0.5;
        let dir = vec2<f32>(
            d.m0.x * v.extrude.x + d.m0.y * v.extrude.y,
            d.m1.x * v.extrude.x + d.m1.y * v.extrude.y) * half;
        let len = length(dir);
        if (len > 0.0) {
            let step = dir / len / half;
            x = x + step.x;
            y = y + step.y;
        }
    }
    // Atlas glyphs land on whole pixels, as they were rasterised (no bilinear smear).
    if (i32(d.mode.x + 0.5) == 5 && d.view.x > 0.0) {
        x = (round((x * 0.5 + 0.5) * d.view.x) / d.view.x) * 2.0 - 1.0;
        y = (round((y * 0.5 + 0.5) * d.view.y) / d.view.y) * 2.0 - 1.0;
    }
    o.pos = vec4<f32>(x, y, 0.0, 1.0);
    o.uv = v.uv;
    o.color = unpack4x8unorm(v.color);
    o.local = v.pos;
    o.rec = e.y;
    return o;
}

{SAMPLE}

fn shade(i: V) -> vec4<f32> {
    let d = draws[i.rec];
    // A mask's stencil takes only the shape itself, not its anti-aliasing fringe.
    if (d.mode.y < -0.5 && i.color.a < 0.5) {
        discard;
    }
    if (any(d.clip != vec4<f32>(0.0))
        && (i.pos.x < d.clip.x || i.pos.y < d.clip.y || i.pos.x >= d.clip.z || i.pos.y >= d.clip.w)) {
        discard;
    }
    var c = i.color;
    let mode = i32(d.mode.x + 0.5);
    let slot = u32(d.mode.z + 0.5);
    let smp_index = u32(d.mode.w + 0.5);
    // Derivatives come before any branch on the draw's paint.
    let gx = dpdx(i.uv);
    let gy = dpdy(i.uv);
    if (mode == 1) {
        let t = sample_tex(slot, smp_index, i.uv, gx, gy);
        c = vec4<f32>(t.rgb / max(t.a, 0.00001), t.a * i.color.a);
    } else if (mode == 5) {
        let t = sample_tex(slot, smp_index, i.uv, gx, gy);
        c = vec4<f32>(i.color.rgb, i.color.a * t.a);
    } else if (mode >= 2) {
        let g = vec2<f32>(
            d.g0.x * i.local.x + d.g0.y * i.local.y + d.g0.z,
            d.g1.x * i.local.x + d.g1.y * i.local.y + d.g1.z);
        var t = 0.0;
        if (mode == 2) {
            t = (g.x + 16384.0) / 32768.0;
        } else {
            t = length(g) / 16384.0;
        }
        let r = sample_tex(slot, 0u, vec2<f32>(clamp(t, 0.0, 1.0) * (255.0 / 256.0) + 0.5 / 256.0, 0.5), vec2<f32>(0.0), vec2<f32>(0.0));
        c = vec4<f32>(r.rgb / max(r.a, 0.00001), r.a * i.color.a);
    }
    c = clamp(c * d.mul + d.add, vec4<f32>(0.0), vec4<f32>(1.0));
    return vec4<f32>(c.rgb * c.a, c.a);
}

@fragment
fn fs(i: V) -> @location(0) vec4<f32> { return shade(i); }

// Both pictures are premultiplied. Blend straight colours only in their overlap;
// preserve the source/destination contributions outside that overlap.
@fragment
fn fs_blend(i: V) -> @location(0) vec4<f32> {
    let src = shade(i);
    if (src.a == 0.0) { discard; }
    let dst = textureLoad(backdrop, vec2<i32>(i.pos.xy), 0);
    let s = src.rgb / max(src.a, 0.00001);
    let dc = dst.rgb / max(dst.a, 0.00001);
    var mixed = abs(dc - s);
    let mode = i32(draws[i.rec].mode.y + 0.5);
    if (mode == 2) { mixed = 1.0 - dc; }
    if (mode == 3 || mode == 4) {
        let threshold = select(dc, s, mode == 4);
        mixed = select(2.0 * s * dc, 1.0 - 2.0 * (1.0 - s) * (1.0 - dc),
                       threshold > vec3<f32>(0.5));
    }
    return vec4<f32>(src.rgb * (1.0 - dst.a) + dst.rgb * (1.0 - src.a)
                     + src.a * dst.a * mixed, src.a + dst.a * (1.0 - src.a));
}
"#;

/// The shader for an adapter with (`bindless`) or without texture arrays. With them every
/// texture is one binding indexed by the draw's slot; without, each draw binds its own.
fn shader_source(bindless: bool) -> String {
    let (textures, sample) = if bindless {
        (
            "@group(1) @binding(0) var textures: binding_array<texture_2d<f32>>;
@group(1) @binding(1) var s0: sampler;
@group(1) @binding(2) var s1: sampler;
@group(1) @binding(3) var s2: sampler;
@group(1) @binding(4) var s3: sampler;",
            "fn sample_tex(slot: u32, s: u32, uv: vec2<f32>, gx: vec2<f32>, gy: vec2<f32>) -> vec4<f32> {
    switch s {
        case 1u: { return textureSampleGrad(textures[slot], s1, uv, gx, gy); }
        case 2u: { return textureSampleGrad(textures[slot], s2, uv, gx, gy); }
        case 3u: { return textureSampleGrad(textures[slot], s3, uv, gx, gy); }
        default: { return textureSampleGrad(textures[slot], s0, uv, gx, gy); }
    }
}",
        )
    } else {
        (
            "@group(1) @binding(0) var tex: texture_2d<f32>;
@group(1) @binding(1) var smp: sampler;",
            "fn sample_tex(slot: u32, s: u32, uv: vec2<f32>, gx: vec2<f32>, gy: vec2<f32>) -> vec4<f32> {
    return textureSampleGrad(tex, smp, uv, gx, gy);
}",
        )
    };
    let source = SHADER
        .replace("{TEXTURES}", textures)
        .replace("{SAMPLE}", sample);
    if bindless {
        format!("enable wgpu_binding_array;\n{source}")
    } else {
        source
    }
}

/// Glyph atlas pages are this many pixels square.
const GLYPH_PAGE: u32 = 1024;
/// The paint of atlas glyphs: the texture's coverage times the vertex colour.
const GLYPH_PAINT: f32 = 5.0;

/// The most texture slots a bindless table holds.
const MAX_SLOTS: u32 = 4096;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct DrawU {
    m0: [f32; 4],
    m1: [f32; 4],
    g0: [f32; 4],
    g1: [f32; 4],
    mul: [f32; 4],
    add: [f32; 4],
    mode: [f32; 4],
    /// A rectangle (x0, y0, x1, y1, target pixels) outside which nothing draws: a mask that
    /// is an axis-aligned rectangle, without the stencil. All zeros: none.
    clip: [f32; 4],
    /// The target's size in pixels, for edge anti-aliasing (zeros: none).
    view: [f32; 4],
}

struct GpuMesh {
    bounds: [f32; 4],
    /// Where the vertices are in the arena, and the triangles (relative to that start).
    verts: crate::gpu::VRange,
    indices: Arc<[u32]>,
    mode: f32,
    to_gradient: Matrix,
    /// The texture (ramp or bitmap); `None` for solid colours.
    binding: Option<crate::gpu::Tex>,
    /// Scaleform text shadows keep shadowColor when the text's parent is tinted.
    fixed_rgb: bool,
    /// The mesh is exactly this axis-aligned rectangle (local twips: x0, y0, x1, y1), so as
    /// a mask it can clip by rectangle instead of the stencil.
    rect: Option<[f32; 4]>,
}

/// Whether `m` covers exactly its bounding rectangle (every vertex on a corner, the
/// triangles' area that of the rectangle).
fn mesh_rect(m: &Mesh) -> Option<[f32; 4]> {
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for v in &m.vertices {
        x0 = x0.min(v.pos[0]);
        y0 = y0.min(v.pos[1]);
        x1 = x1.max(v.pos[0]);
        y1 = y1.max(v.pos[1]);
    }
    let eps = 0.5;
    let on = |a: f32, lo: f32, hi: f32| (a - lo).abs() < eps || (a - hi).abs() < eps;
    if x1 - x0 < eps
        || y1 - y0 < eps
        || !m
            .vertices
            .iter()
            .all(|v| on(v.pos[0], x0, x1) && on(v.pos[1], y0, y1))
    {
        return None;
    }
    let mut area = 0.0f32;
    for t in m.indices.chunks_exact(3) {
        let p = |i: u32| {
            m.vertices
                .get(i as usize)
                .map(|v| v.pos)
                .unwrap_or_default()
        };
        let (a, b, c) = (p(t[0]), p(t[1]), p(t[2]));
        area += ((b[0] - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (b[1] - a[1])).abs() / 2.0;
    }
    let want = (x1 - x0) * (y1 - y0);
    ((area - want).abs() <= want * 1e-3).then_some([x0, y0, x1, y1])
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Stencil {
    Content,
    Push,
    Pop,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Blend {
    Normal,
    Multiply,
    Screen,
    Add,
    Subtract,
    Lighten,
    Darken,
    Alpha,
    Erase,
    Difference,
    Invert,
    Overlay,
    HardLight,
}

impl Blend {
    fn of(m: BlendMode) -> Self {
        match m {
            BlendMode::Multiply => Blend::Multiply,
            BlendMode::Screen => Blend::Screen,
            BlendMode::Add => Blend::Add,
            BlendMode::Subtract => Blend::Subtract,
            BlendMode::Lighten => Blend::Lighten,
            BlendMode::Darken => Blend::Darken,
            BlendMode::Alpha => Blend::Alpha,
            BlendMode::Erase => Blend::Erase,
            BlendMode::Difference => Blend::Difference,
            BlendMode::Invert => Blend::Invert,
            BlendMode::Overlay => Blend::Overlay,
            BlendMode::HardLight => Blend::HardLight,
            _ => Blend::Normal,
        }
    }

    fn shader_mode(self) -> u8 {
        match self {
            Self::Difference => 1,
            Self::Invert => 2,
            Self::Overlay => 3,
            Self::HardLight => 4,
            _ => 0,
        }
    }

    fn state(self) -> wgpu::BlendState {
        use wgpu::{BlendComponent as C, BlendFactor as F, BlendOperation as O};
        let alpha = C {
            src_factor: if matches!(self, Self::Alpha | Self::Erase) {
                F::Zero
            } else {
                F::One
            },
            dst_factor: if self == Self::Alpha {
                F::SrcAlpha
            } else {
                F::OneMinusSrcAlpha
            },
            operation: O::Add,
        };
        let color = match self {
            Blend::Normal
            | Blend::Alpha
            | Blend::Erase
            | Blend::Difference
            | Blend::Invert
            | Blend::Overlay
            | Blend::HardLight => alpha,
            // Over an opaque picture: src * dst + dst * (1 - src alpha).
            Blend::Multiply => C {
                src_factor: F::Dst,
                dst_factor: F::OneMinusSrcAlpha,
                operation: O::Add,
            },
            Blend::Screen => C {
                src_factor: F::One,
                dst_factor: F::OneMinusSrc,
                operation: O::Add,
            },
            Blend::Add => C {
                src_factor: F::One,
                dst_factor: F::One,
                operation: O::Add,
            },
            Blend::Subtract => C {
                src_factor: F::One,
                dst_factor: F::One,
                operation: O::ReverseSubtract,
            },
            Blend::Lighten => C {
                src_factor: F::One,
                dst_factor: F::One,
                operation: O::Max,
            },
            Blend::Darken => C {
                src_factor: F::One,
                dst_factor: F::One,
                operation: O::Min,
            },
        };
        wgpu::BlendState { color, alpha }
    }
}

#[derive(Clone)]
struct Op {
    mesh: Arc<GpuMesh>,
    uniform: u32,
    blend: Blend,
    stencil: Stencil,
    reference: u32,
}

/// A shape's mesh on the CPU (for scaling grids) and on the GPU.
type CpuShape = (Arc<Mesh>, Arc<GpuMesh>);
/// A glyph's triangles in EM units.
type GlyphMesh = (Vec<[f32; 2]>, Vec<u32>);
/// An uploaded image: its binding and size.
type Bitmap = (crate::gpu::Tex, f32, f32);

struct EngineImage {
    texture: wgpu::Texture,
    binding: crate::gpu::Tex,
}

pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    format: wgpu::TextureFormat,
    pipelines: HashMap<PipelineKey, wgpu::RenderPipeline>,
    /// Group 0: draw records, the vertex arena and the per-render vertex stream.
    draw_layout: wgpu::BindGroupLayout,
    /// One texture and its sampler (each draw's own without texture arrays; the backdrop).
    texture_layout: wgpu::BindGroupLayout,
    /// Every texture slot and the four samplers (with texture arrays).
    table_layout: Option<wgpu::BindGroupLayout>,
    table_group: Option<(u64, wgpu::BindGroup)>,
    bindless: bool,
    arena: crate::gpu::Arena,
    textures: crate::gpu::Textures,
    /// The draw records and the vertex stream (buffer and its size in bytes).
    records: Option<(wgpu::Buffer, u64)>,
    stream: Option<(wgpu::Buffer, u64)>,
    white_view: wgpu::TextureView,
    /// Draw calls in the last render, after merging.
    pub calls: usize,
    /// Samples per pixel (1: edge anti-aliasing and the glyph atlas do the smoothing;
    /// `THEDAS_UI_MSAA=4` brings multisampling back for comparison).
    samples: u32,
    shader: wgpu::ShaderModule,
    pipeline_layout: wgpu::PipelineLayout,
    white: wgpu::BindGroup,
    clamp: wgpu::Sampler,
    repeat: wgpu::Sampler,
    clamp_nearest: wgpu::Sampler,
    repeat_nearest: wgpu::Sampler,
    shapes: HashMap<(u16, u16, bool), Vec<Arc<GpuMesh>>>,
    cpu_shapes: HashMap<(u16, u16), Vec<CpuShape>>,
    scale9: HashMap<(DisplayId, [u32; 8]), Vec<Arc<GpuMesh>>>,
    glyphs: HashMap<(usize, usize), Arc<GlyphMesh>>,
    texts: HashMap<DisplayId, (u64, u64, Vec<Arc<GpuMesh>>)>,
    /// The glyph atlas: packing, and each page's texture.
    glyph_packer: crate::glyphs::Packer,
    glyph_pages: Vec<(wgpu::Texture, crate::gpu::Tex)>,
    /// Drawing-API meshes by clip, with a hash of the drawing they were made from.
    drawings: HashMap<DisplayId, (u64, Vec<Arc<GpuMesh>>)>,
    /// Quads of images loaded into clips, by URL and size.
    bitmap_quads: HashMap<(String, u32, u32), Arc<GpuMesh>>,
    /// What the last render did (see [`RenderStats`]).
    pub stats: RenderStats,
    bitmaps: HashMap<(String, bool, bool), Option<Bitmap>>,
    engine_images: HashMap<String, Option<EngineImage>>,
    targets: Option<Targets>,
    /// Drawing straight into a host's picture (see [`Renderer::draw_into`]): the last
    /// build, uploaded, and the stencil buffer it draws with.
    direct: Option<Direct>,
    /// The copy of the picture that destination-dependent blends sample, by size and format.
    backdrop: Option<(
        (u32, u32),
        wgpu::TextureFormat,
        wgpu::Texture,
        wgpu::BindGroup,
    )>,
    /// Draws in the last frame.
    pub draws: usize,
    filters: crate::filters::Filters,
    filter_stack: Vec<DisplayId>,
    /// Flattened filter/layer pictures, reused when their actual draw inputs agree.
    filtered: HashMap<DisplayId, Filtered>,
    filtered_seen: std::collections::HashSet<DisplayId>,
    masking: bool,
}

struct Filtered {
    source: Build,
    filters: Vec<crate::swf::Filter>,
    size: (u32, u32),
    scale: u32,
    clip: [f32; 4],
    mesh: Option<Arc<GpuMesh>>,
}

/// A pipeline: blend, stencil use, target format, samples.
type PipelineKey = (Blend, Stencil, wgpu::TextureFormat, u32);

/// A build uploaded for drawing: its records and stream are in the renderer's buffers.
struct Prepared {
    ops: Vec<Op>,
    /// Each op's place in the stream.
    ranges: Vec<(u32, u32)>,
    draw_group: wgpu::BindGroup,
    table: Option<wgpu::BindGroup>,
}

/// Where an encode draws: the picture (texture for backdrop copies, view to draw into),
/// its multisampled stand-in, the stencil, and whether to clear the picture first.
struct Target<'a> {
    texture: &'a wgpu::Texture,
    view: &'a wgpu::TextureView,
    msaa: Option<&'a wgpu::TextureView>,
    stencil: &'a wgpu::TextureView,
    samples: u32,
    clear: bool,
}

/// The state of drawing straight into a host's picture.
struct Direct {
    size: (u32, u32),
    stencil: wgpu::TextureView,
    prepared: Option<Prepared>,
}

struct Targets {
    size: (u32, u32),
    output: wgpu::Texture,
    /// The multisampled picture resolved into `output` (none at one sample).
    msaa: Option<wgpu::TextureView>,
    stencil: wgpu::TextureView,
}

impl Renderer {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let wanted = wgpu::Features::TEXTURE_BINDING_ARRAY
            | wgpu::Features::SAMPLED_TEXTURE_AND_STORAGE_BUFFER_ARRAY_NON_UNIFORM_INDEXING
            | wgpu::Features::PARTIALLY_BOUND_BINDING_ARRAY;
        let bindless = device.features().contains(wanted)
            && device.limits().max_binding_array_elements_per_shader_stage >= MAX_SLOTS
            && std::env::var_os("THEDAS_UI_NO_BINDLESS").is_none();
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("gfx"),
            source: wgpu::ShaderSource::Wgsl(shader_source(bindless).into()),
        });
        let storage = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let draw_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("gfx draw"),
            entries: &[storage(0), storage(1), storage(2)],
        });
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("gfx texture"),
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
        let table_layout = bindless.then(|| {
            let sampler = |binding| wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            };
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("gfx texture table"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: std::num::NonZeroU32::new(MAX_SLOTS),
                    },
                    sampler(1),
                    sampler(2),
                    sampler(3),
                    sampler(4),
                ],
            })
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("gfx"),
            bind_group_layouts: &[
                Some(&draw_layout),
                Some(table_layout.as_ref().unwrap_or(&texture_layout)),
                Some(&texture_layout),
            ],
            immediate_size: 0,
        });
        let sampler = |mode, filter| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("gfx"),
                address_mode_u: mode,
                address_mode_v: mode,
                mag_filter: filter,
                min_filter: filter,
                ..Default::default()
            })
        };
        let clamp = sampler(wgpu::AddressMode::ClampToEdge, wgpu::FilterMode::Linear);
        let repeat = sampler(wgpu::AddressMode::Repeat, wgpu::FilterMode::Linear);
        let clamp_nearest = sampler(wgpu::AddressMode::ClampToEdge, wgpu::FilterMode::Nearest);
        let repeat_nearest = sampler(wgpu::AddressMode::Repeat, wgpu::FilterMode::Nearest);
        let white_tex = upload_texture(device, queue, 1, 1, &[255, 255, 255, 255]);
        let white = texture_binding(device, &texture_layout, &white_tex, &clamp);
        let white_view = white_tex.create_view(&Default::default());
        Self {
            device: device.clone(),
            queue: queue.clone(),
            format,
            pipelines: HashMap::new(),
            draw_layout,
            texture_layout,
            table_layout,
            table_group: None,
            bindless,
            arena: crate::gpu::Arena::new(device),
            textures: crate::gpu::Textures::new(),
            records: None,
            stream: None,
            white_view,
            calls: 0,
            samples: std::env::var("THEDAS_UI_MSAA")
                .ok()
                .and_then(|v| v.parse().ok())
                .filter(|n| [1, 2, 4].contains(n))
                .unwrap_or(1),
            shader,
            pipeline_layout,
            white,
            clamp,
            repeat,
            clamp_nearest,
            repeat_nearest,
            shapes: HashMap::new(),
            cpu_shapes: HashMap::new(),
            scale9: HashMap::new(),
            glyphs: HashMap::new(),
            texts: HashMap::new(),
            glyph_packer: crate::glyphs::Packer::new(GLYPH_PAGE),
            glyph_pages: Vec::new(),
            drawings: HashMap::new(),
            bitmap_quads: HashMap::new(),
            stats: RenderStats::default(),
            bitmaps: HashMap::new(),
            engine_images: HashMap::new(),
            targets: None,
            direct: None,
            backdrop: None,
            draws: 0,
            filters: crate::filters::Filters::new(device, format),
            filter_stack: Vec::new(),
            filtered: HashMap::new(),
            filtered_seen: Default::default(),
            masking: false,
        }
    }

    fn pipeline(
        &mut self,
        blend: Blend,
        stencil: Stencil,
        format: wgpu::TextureFormat,
        samples: u32,
    ) -> &wgpu::RenderPipeline {
        let key = (blend, stencil, format, samples);
        if !self.pipelines.contains_key(&key) {
            let pass_op = match stencil {
                Stencil::Content => wgpu::StencilOperation::Keep,
                Stencil::Push => wgpu::StencilOperation::IncrementClamp,
                Stencil::Pop => wgpu::StencilOperation::DecrementClamp,
            };
            let face = wgpu::StencilFaceState {
                compare: wgpu::CompareFunction::Equal,
                fail_op: wgpu::StencilOperation::Keep,
                depth_fail_op: wgpu::StencilOperation::Keep,
                pass_op,
            };
            let p = self
                .device
                .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("gfx"),
                    layout: Some(&self.pipeline_layout),
                    vertex: wgpu::VertexState {
                        module: &self.shader,
                        entry_point: Some("vs"),
                        compilation_options: Default::default(),
                        buffers: &[],
                    },
                    primitive: wgpu::PrimitiveState::default(),
                    depth_stencil: Some(wgpu::DepthStencilState {
                        format: wgpu::TextureFormat::Depth24PlusStencil8,
                        depth_write_enabled: Some(false),
                        depth_compare: Some(wgpu::CompareFunction::Always),
                        stencil: wgpu::StencilState {
                            front: face,
                            back: face,
                            read_mask: 0xff,
                            write_mask: 0xff,
                        },
                        bias: Default::default(),
                    }),
                    multisample: wgpu::MultisampleState {
                        count: samples,
                        ..Default::default()
                    },
                    fragment: Some(wgpu::FragmentState {
                        module: &self.shader,
                        entry_point: Some(if blend.shader_mode() == 0 {
                            "fs"
                        } else {
                            "fs_blend"
                        }),
                        compilation_options: Default::default(),
                        targets: &[Some(wgpu::ColorTargetState {
                            format,
                            blend: (blend.shader_mode() == 0).then(|| blend.state()),
                            write_mask: if stencil == Stencil::Content {
                                wgpu::ColorWrites::ALL
                            } else {
                                wgpu::ColorWrites::empty()
                            },
                        })],
                    }),
                    multiview_mask: None,
                    cache: None,
                });
            self.pipelines.insert(key, p);
        }
        &self.pipelines[&key]
    }

    fn ensure_targets(&mut self, size: (u32, u32)) {
        if self.targets.as_ref().is_some_and(|t| t.size == size) {
            return;
        }
        self.targets = Some(self.create_targets(size));
    }

    fn create_targets(&self, size: (u32, u32)) -> Targets {
        let tex = |format, samples, usage| {
            self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("gfx target"),
                size: wgpu::Extent3d {
                    width: size.0.max(1),
                    height: size.1.max(1),
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: samples,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let output = tex(
            self.format,
            1,
            wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
        );
        let msaa = (self.samples > 1).then(|| {
            tex(
                self.format,
                self.samples,
                wgpu::TextureUsages::RENDER_ATTACHMENT,
            )
            .create_view(&Default::default())
        });
        let stencil = tex(
            wgpu::TextureFormat::Depth24PlusStencil8,
            self.samples,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
        )
        .create_view(&Default::default());
        Targets {
            size,
            output,
            msaa,
            stencil,
        }
    }

    /// Whether every texture is sampled from one bound table (merging draws across
    /// textures); without texture arrays each texture breaks a batch.
    pub fn bindless(&self) -> bool {
        self.bindless
    }

    /// The texture [`Renderer::render`] draws into.
    pub fn output(&self) -> Option<&wgpu::Texture> {
        self.targets.as_ref().map(|t| &t.output)
    }

    /// Registers or refreshes an engine render target (e.g. `paperdoll`). Shapes filled
    /// only with that external image display the complete target in their authored bounds.
    /// Same-size refreshes keep the texture and all cached shape bindings alive.
    pub fn set_engine_image(&mut self, name: &str, image: Option<(u32, u32, &[u8])>) {
        let key = engine_image_name(name);
        let old = self.engine_images.entry(key.clone()).or_default();
        if let Some((w, h, rgba)) = image {
            if w == 0 || h == 0 || rgba.len() != w as usize * h as usize * 4 {
                return;
            }
            // Same-size updates keep their bindings, but change the pixels sampled by
            // a flattened subtree (a portrait or paperdoll, including nested layers).
            self.filtered.clear();
            let mut pixels = rgba.to_vec();
            premultiply(&mut pixels);
            if let Some(old) = old
                .as_ref()
                .filter(|old| old.texture.width() == w && old.texture.height() == h)
            {
                write_image(&self.queue, &old.texture, &pixels);
                return;
            }
            let texture = upload_texture(&self.device, &self.queue, w, h, &pixels);
            let binding = self.make_tex(&texture, crate::gpu::SAMPLER_CLAMP);
            if let Some(slot) = self.engine_images.get_mut(&key) {
                *slot = Some(EngineImage { texture, binding });
            }
        } else if let Some(old) = old {
            self.filtered.clear();
            // Keep existing meshes valid while clearing the target, and reuse its storage
            // if the engine supplies another frame of the same size later.
            write_image(
                &self.queue,
                &old.texture,
                &vec![0; (old.texture.width() * old.texture.height() * 4) as usize],
            );
            return;
        }
        // Only first allocation or a size change needs new bindings. Ordinary frames do
        // not invalidate atlas textures or retessellate the rest of the movie.
        self.shapes.clear();
        self.cpu_shapes.clear();
        self.scale9.clear();
    }

    /// Renders when the movie changed (or the size did); returns whether it drew.
    pub fn render_if_dirty(
        &mut self,
        player: &mut Player,
        images: &mut dyn Images,
        size: (u32, u32),
    ) -> bool {
        let changed = self.targets.as_ref().is_none_or(|t| t.size != size);
        if !player.take_dirty() && !changed {
            return false;
        }
        self.render(player, images, size);
        true
    }

    /// Draws the movie into [`Renderer::output`] (transparent background).
    pub fn render(&mut self, player: &mut Player, images: &mut dyn Images, size: (u32, u32)) {
        self.ensure_targets(size);
        let b = self.build(player, images, size);
        let started = std::time::Instant::now();
        if let Some(targets) = self.targets.take() {
            self.draw_build(b, &targets);
            self.targets = Some(targets);
        }
        self.stats.submit_us = started.elapsed().as_micros() as u64;
    }

    /// Walks the movie into a draw list for a picture of `size` (stats reset).
    fn build(&mut self, player: &mut Player, images: &mut dyn Images, size: (u32, u32)) -> Build {
        let (authored_w, authored_h) = player.vm.movie.stage_size();
        let s = (size.0 as f32 / authored_w).min(size.1 as f32 / authored_h);
        let ox = (size.0 as f32 - authored_w * s) / 2.0;
        let oy = (size.1 as f32 - authored_h * s) / 2.0;
        let view = Viewport {
            k: s / 20.0,
            ox,
            oy,
            w: size.0 as f32,
            h: size.1 as f32,
        };
        let mut b = Build {
            ops: Vec::new(),
            uniforms: Vec::new(),
            clip: [0.0; 4],
        };
        self.stats = RenderStats::default();
        self.filtered_seen.clear();
        let started = std::time::Instant::now();
        let root = player.vm.root;
        self.walk(
            player,
            images,
            root,
            Matrix::IDENTITY,
            ColorTransform::IDENTITY,
            Blend::Normal,
            &view,
            0,
            &mut b,
        );
        self.draws = b.ops.len();
        self.stats.draws = b.ops.len();
        self.stats.build_us = started.elapsed().as_micros() as u64;
        // Drop text meshes of fields that are gone.
        self.texts
            .retain(|d, _| player.vm.display.get(*d).is_some());
        self.drawings
            .retain(|d, _| player.vm.display.get(*d).is_some());
        self.filtered.retain(|d, _| self.filtered_seen.contains(d));
        b
    }

    /// Makes sure `slot` is a storage buffer of at least `need` bytes.
    fn storage(
        device: &wgpu::Device,
        slot: &mut Option<(wgpu::Buffer, u64)>,
        need: u64,
        label: &str,
    ) {
        if slot.as_ref().is_none_or(|(_, cap)| *cap < need) {
            let cap = need.next_power_of_two().max(1 << 16);
            *slot = Some((
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some(label),
                    size: cap,
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }),
                cap,
            ));
        }
    }

    /// The bindless texture table's bind group, made again when slots changed.
    fn table_group(&mut self) -> Option<wgpu::BindGroup> {
        let layout = self.table_layout.as_ref()?;
        let generation = self.textures.generation();
        if self
            .table_group
            .as_ref()
            .is_none_or(|(g, _)| *g != generation)
        {
            let slots = self.textures.slots.lock().ok()?;
            let mut views: Vec<&wgpu::TextureView> = slots
                .views()
                .iter()
                .map(|v| v.as_ref().unwrap_or(&self.white_view))
                .take(MAX_SLOTS as usize)
                .collect();
            if views.is_empty() {
                views.push(&self.white_view);
            }
            let group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("gfx texture table"),
                layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureViewArray(&views),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.clamp),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&self.repeat),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::Sampler(&self.clamp_nearest),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::Sampler(&self.repeat_nearest),
                    },
                ],
            });
            drop(slots);
            self.table_group = Some((generation, group));
        }
        self.table_group.as_ref().map(|(_, g)| g.clone())
    }

    /// Draws `b` into the renderer's own targets `t` (cleared first) and submits.
    fn draw_build(&mut self, b: Build, t: &Targets) {
        let Some(prepared) = self.prepare_build(b) else {
            return;
        };
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("gfx") });
        let view = t.output.create_view(&Default::default());
        let format = self.format;
        self.calls = self.encode(
            &prepared,
            &mut encoder,
            Target {
                texture: &t.output,
                view: &view,
                msaa: t.msaa.as_ref(),
                stencil: &t.stencil,
                samples: self.samples,
                clear: true,
            },
            format,
        );
        self.queue.submit(Some(encoder.finish()));
    }

    /// Uploads `b`'s records and vertex stream and makes the bind groups that draw them.
    fn prepare_build(&mut self, b: Build) -> Option<Prepared> {
        // Records: one per op (`Build::push` gives each its own), read by index.
        let mut records = b.uniforms;
        for op in &b.ops {
            if let (Some(tex), Some(r)) = (&op.mesh.binding, records.get_mut(op.uniform as usize)) {
                r.mode[2] = tex.slot() as f32;
                r.mode[3] = f32::from(tex.sampler());
            }
        }
        if records.is_empty() {
            records.push(DrawU::default());
        }
        // The stream: each op's triangles as (arena vertex, record) pairs, in draw order.
        let mut stream: Vec<[u32; 2]> = Vec::new();
        let mut ranges: Vec<(u32, u32)> = Vec::with_capacity(b.ops.len());
        for op in &b.ops {
            let start = stream.len() as u32;
            let base = op.mesh.verts.start;
            stream.extend(op.mesh.indices.iter().map(|&i| [base + i, op.uniform]));
            ranges.push((start, stream.len() as u32 - start));
        }
        if stream.is_empty() {
            stream.push([0, 0]);
        }
        let record_bytes: &[u8] = bytemuck::cast_slice(&records);
        let stream_bytes: &[u8] = bytemuck::cast_slice(&stream);
        Self::storage(
            &self.device,
            &mut self.records,
            record_bytes.len() as u64,
            "gfx records",
        );
        Self::storage(
            &self.device,
            &mut self.stream,
            stream_bytes.len() as u64,
            "gfx stream",
        );
        let table = self.table_group();
        if std::env::var_os("GFX_BATCH_DEBUG").is_some() {
            let mut runs: Vec<(String, usize)> = Vec::new();
            for op in &b.ops {
                let k = format!("{:?}/{:?}/{}", op.blend, op.stencil, op.reference);
                match runs.last_mut() {
                    Some((last, n)) if *last == k => *n += 1,
                    _ => runs.push((k, 1)),
                }
            }
            eprintln!("gfx batches: {runs:?}");
        }
        let (Some((record_buf, _)), Some((stream_buf, _))) = (&self.records, &self.stream) else {
            return None;
        };
        self.queue.write_buffer(record_buf, 0, record_bytes);
        self.queue.write_buffer(stream_buf, 0, stream_bytes);
        let draw_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("gfx draw"),
            layout: &self.draw_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: record_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: self.arena.buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: stream_buf.as_entire_binding(),
                },
            ],
        });
        Some(Prepared {
            ops: b.ops,
            ranges,
            draw_group,
            table,
        })
    }

    /// The backdrop texture for destination-dependent blends over a picture of `size` and
    /// `format`, made again when either changes.
    fn backdrop(
        &mut self,
        size: (u32, u32),
        format: wgpu::TextureFormat,
    ) -> (wgpu::Texture, wgpu::BindGroup) {
        if !self
            .backdrop
            .as_ref()
            .is_some_and(|(s, f, _, _)| *s == size && *f == format)
        {
            let tex = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("gfx blend backdrop"),
                size: wgpu::Extent3d {
                    width: size.0.max(1),
                    height: size.1.max(1),
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let binding = texture_binding(&self.device, &self.texture_layout, &tex, &self.clamp);
            self.backdrop = Some((size, format, tex, binding));
        }
        let (_, _, t, b) = self.backdrop.as_ref().expect("made above");
        (t.clone(), b.clone())
    }

    /// Records `p`'s draws into `encoder`, onto `t`; returns the draw calls made.
    fn encode(
        &mut self,
        p: &Prepared,
        encoder: &mut wgpu::CommandEncoder,
        t: Target<'_>,
        format: wgpu::TextureFormat,
    ) -> usize {
        for op in &p.ops {
            self.pipeline(op.blend, op.stencil, format, t.samples);
        }
        // Resolve before a destination-dependent blend, then sample a snapshot rather
        // than the active attachment. Keep the multisample colour/stencil between passes
        // so later siblings and open clip-depth masks retain their state. A picture that
        // cannot be copied gives those blends a white backdrop.
        let copyable = t.texture.usage().contains(wgpu::TextureUsages::COPY_SRC);
        let size = (t.texture.width(), t.texture.height());
        let snapshot = p
            .ops
            .iter()
            .any(|op| op.blend.shader_mode() != 0)
            .then(|| self.backdrop(size, t.texture.format()));
        let table = p.table.as_ref();
        let mut calls = 0usize;
        let mut start = 0;
        let mut first = true;
        loop {
            let end = (start + usize::from(!first)..p.ops.len())
                .find(|&i| p.ops[i].blend.shader_mode() != 0)
                .unwrap_or(p.ops.len());
            // A first pass runs before any destination blend (it may draw nothing), so the
            // picture copied here holds what was drawn (or cleared) before.
            if !first && start < p.ops.len() && p.ops[start].blend.shader_mode() != 0 && copyable {
                if let Some((texture, _)) = snapshot.as_ref() {
                    encoder.copy_texture_to_texture(
                        t.texture.as_image_copy(),
                        texture.as_image_copy(),
                        t.texture.size(),
                    );
                }
            }
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("gfx"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: t.msaa.unwrap_or(t.view),
                    resolve_target: t.msaa.map(|_| t.view),
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: if first && t.clear {
                            wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT)
                        } else {
                            wgpu::LoadOp::Load
                        },
                        // Without multisampling the attachment is the picture itself.
                        store: if snapshot.is_some() || t.msaa.is_none() {
                            wgpu::StoreOp::Store
                        } else {
                            wgpu::StoreOp::Discard
                        },
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: t.stencil,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: Some(wgpu::Operations {
                        load: if first {
                            wgpu::LoadOp::Clear(0)
                        } else {
                            wgpu::LoadOp::Load
                        },
                        store: if snapshot.is_some() {
                            wgpu::StoreOp::Store
                        } else {
                            wgpu::StoreOp::Discard
                        },
                    }),
                }),
                ..Default::default()
            });
            // All pipelines share this layout; ordinary draws do not read group 2.
            pass.set_bind_group(
                2,
                snapshot.as_ref().map(|(_, bg)| bg).unwrap_or(&self.white),
                &[],
            );
            pass.set_bind_group(0, &p.draw_group, &[]);
            if let Some(table) = table {
                pass.set_bind_group(1, table, &[]);
            }
            // Consecutive ops that share a pipeline, a stencil reference and (without
            // texture arrays) a texture are one draw: their stream ranges are adjacent.
            let key = |o: &Op| {
                (
                    o.blend,
                    o.stencil,
                    o.reference,
                    if table.is_some() {
                        None
                    } else {
                        o.mesh.binding.as_ref().map(|t| (t.slot(), t.sampler()))
                    },
                )
            };
            let mut i = start;
            while i < end {
                let op = &p.ops[i];
                let k = key(op);
                let mut j = i + 1;
                while j < end && key(&p.ops[j]) == k {
                    j += 1;
                }
                let Some(pl) = self
                    .pipelines
                    .get(&(op.blend, op.stencil, format, t.samples))
                else {
                    i = j;
                    continue;
                };
                pass.set_pipeline(pl);
                pass.set_stencil_reference(op.reference);
                if table.is_none() {
                    let single = op.mesh.binding.as_ref().map(|t| t.single());
                    pass.set_bind_group(1, single.unwrap_or(&self.white), &[]);
                }
                let first = p.ranges[i].0;
                let last = p.ranges[j - 1].0 + p.ranges[j - 1].1;
                pass.draw(first..last, 0..1);
                calls += 1;
                i = j;
            }
            drop(pass);
            if end == p.ops.len() {
                break;
            }
            start = end;
            first = false;
        }
        calls
    }

    /// Brings the movie's draw list up to date for drawing straight into a host's picture
    /// of `size` (see [`Renderer::draw_into`]): walks and uploads only when the movie or
    /// the size changed. Returns whether it rebuilt.
    pub fn prepare_into(
        &mut self,
        player: &mut Player,
        images: &mut dyn Images,
        size: (u32, u32),
    ) -> bool {
        let resized = self.direct.as_ref().is_none_or(|d| d.size != size);
        let dirty = player.take_dirty();
        if !dirty && !resized && self.direct.as_ref().is_some_and(|d| d.prepared.is_some()) {
            return false;
        }
        if resized {
            let stencil = self
                .device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some("gfx stencil"),
                    size: wgpu::Extent3d {
                        width: size.0.max(1),
                        height: size.1.max(1),
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Depth24PlusStencil8,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &[],
                })
                .create_view(&Default::default());
            self.direct = Some(Direct {
                size,
                stencil,
                prepared: None,
            });
        }
        let b = self.build(player, images, size);
        let prepared = self.prepare_build(b);
        if let Some(d) = self.direct.as_mut() {
            d.prepared = prepared;
        }
        true
    }

    /// Draws the movie's prepared draw list (see [`Renderer::prepare_into`]) over `target`,
    /// a picture of the prepared size, into `encoder`. Nothing is re-walked or uploaded:
    /// an unchanged movie costs its few draw calls. Blends act on what is already in the
    /// picture, as the original's did on the game's frame.
    pub fn draw_into(&mut self, encoder: &mut wgpu::CommandEncoder, target: &wgpu::Texture) {
        let Some(direct) = self.direct.take() else {
            return;
        };
        if direct.size == (target.width(), target.height()) {
            if let Some(prepared) = direct.prepared.as_ref() {
                let view = target.create_view(&Default::default());
                self.calls = self.encode(
                    prepared,
                    encoder,
                    Target {
                        texture: target,
                        view: &view,
                        msaa: None,
                        stencil: &direct.stencil,
                        samples: 1,
                        clear: false,
                    },
                    target.format(),
                );
            }
        }
        self.direct = Some(direct);
    }

    fn upload(
        &mut self,
        m: &Mesh,
        binding: Option<crate::gpu::Tex>,
        mode: f32,
        to_gradient: Matrix,
    ) -> Option<Arc<GpuMesh>> {
        self.upload_colored(m, binding, mode, to_gradient, false)
    }

    fn upload_colored(
        &mut self,
        m: &Mesh,
        binding: Option<crate::gpu::Tex>,
        mode: f32,
        to_gradient: Matrix,
        fixed_rgb: bool,
    ) -> Option<Arc<GpuMesh>> {
        if m.indices.is_empty() || m.vertices.is_empty() {
            return None;
        }
        let verts = self.arena.alloc(&self.device, &self.queue, &m.vertices);
        let rect = mesh_rect(m);
        let bounds = m.vertices.iter().fold(
            [
                f32::INFINITY,
                f32::INFINITY,
                f32::NEG_INFINITY,
                f32::NEG_INFINITY,
            ],
            |b, v| {
                [
                    b[0].min(v.pos[0]),
                    b[1].min(v.pos[1]),
                    b[2].max(v.pos[0]),
                    b[3].max(v.pos[1]),
                ]
            },
        );
        Some(Arc::new(GpuMesh {
            bounds,
            rect,
            verts,
            indices: Arc::from(m.indices.as_slice()),
            mode,
            to_gradient,
            binding,
            fixed_rgb,
        }))
    }

    /// Registers a texture for drawing with sampler `sampler` (see `gpu::SAMPLER_*`).
    fn make_tex(&self, texture: &wgpu::Texture, sampler: u8) -> crate::gpu::Tex {
        let s = match sampler {
            crate::gpu::SAMPLER_REPEAT => &self.repeat,
            crate::gpu::SAMPLER_CLAMP_NEAREST => &self.clamp_nearest,
            crate::gpu::SAMPLER_REPEAT_NEAREST => &self.repeat_nearest,
            _ => &self.clamp,
        };
        let single = texture_binding(&self.device, &self.texture_layout, texture, s);
        self.textures
            .add(texture.create_view(&Default::default()), sampler, single)
    }

    fn bitmap(
        &mut self,
        images: &mut dyn Images,
        name: &str,
        repeat: bool,
        smooth: bool,
    ) -> Option<Bitmap> {
        let key = (name.to_string(), repeat, smooth);
        if let Some(b) = self.bitmaps.get(&key) {
            return b.clone();
        }
        let r = images.image(name).map(|(w, h, mut px)| {
            premultiply(&mut px);
            let t = upload_texture(&self.device, &self.queue, w, h, &px);
            let s = match (repeat, smooth) {
                (false, false) => crate::gpu::SAMPLER_CLAMP_NEAREST,
                (true, false) => crate::gpu::SAMPLER_REPEAT_NEAREST,
                (false, true) => crate::gpu::SAMPLER_CLAMP,
                (true, true) => crate::gpu::SAMPLER_REPEAT,
            };
            (self.make_tex(&t, s), w as f32, h as f32)
        });
        self.bitmaps.insert(key, r.clone());
        r
    }

    fn shape_meshes(
        &mut self,
        player: &Player,
        images: &mut dyn Images,
        id: u16,
        ratio: u16,
    ) -> Vec<Arc<GpuMesh>> {
        let key = (id, ratio, self.masking);
        if let Some(m) = self.shapes.get(&key) {
            return m.clone();
        }
        self.stats.shapes_tessellated += 1;
        let movie = player.vm.movie.clone();
        if let Some(Character::Shape(shape)) = movie.characters.get(&id)
            && let Some(name) = engine_shape(shape, &movie)
            && let Some(image) = self.engine_images.get(&name)
        {
            let binding = image.as_ref().map(|image| image.binding.clone());
            let out: Vec<_> = binding
                .and_then(|binding| {
                    self.upload(
                        &image_quad(shape.bounds),
                        Some(binding),
                        1.0,
                        Matrix::IDENTITY,
                    )
                })
                .into_iter()
                .collect();
            self.shapes.insert(key, out.clone());
            return out;
        }
        let size = |bid: u16| -> Option<(f32, f32)> {
            match movie.characters.get(&bid) {
                Some(Character::Image(img)) => Some((img.width as f32, img.height as f32)),
                _ => None,
            }
        };
        let meshes = match movie.characters.get(&id) {
            Some(Character::Shape(s)) => {
                if self.masking {
                    let mut records = s.records.clone();
                    for record in &mut records {
                        if let crate::swf::Record::Style {
                            line, new_styles, ..
                        } = record
                        {
                            *line = None;
                            if let Some((_, lines)) = new_styles {
                                lines.clear();
                            }
                        }
                    }
                    tess::tessellate_shape(&s.fills, &[], &records, &size)
                } else {
                    tess::tessellate_shape(&s.fills, &s.lines, &s.records, &size)
                }
            }
            Some(Character::Morph(m)) => {
                let t = ratio as f32 / 65535.0;
                let records = lerp_records(&m.start, &m.end, t);
                let fills: Vec<_> = m.fills.iter().map(|(a, b)| lerp_fill(a, b, t)).collect();
                tess::tessellate_shape(&fills, &[], &records, &size)
            }
            _ => Vec::new(),
        };
        let mut out = Vec::new();
        for m in meshes {
            let g = match &m.paint {
                Paint::Solid => self.upload(&m, None, 0.0, Matrix::IDENTITY),
                Paint::Bitmap {
                    id: bid,
                    repeat,
                    smooth,
                } => {
                    let file = match movie.characters.get(bid) {
                        Some(Character::Image(img)) => img.file.clone(),
                        _ => String::new(),
                    };
                    let tex = self.bitmap(images, &file, *repeat, *smooth);
                    match tex {
                        Some((bg, _, _)) => self.upload(&m, Some(bg), 1.0, Matrix::IDENTITY),
                        None => {
                            log::debug!("gfx: image {file} missing");
                            None
                        }
                    }
                }
                Paint::Gradient {
                    kind,
                    to_gradient,
                    ramp,
                    ..
                } => {
                    let px: Vec<u8> = ramp
                        .iter()
                        .flat_map(|c| {
                            let a = c[3] as u32;
                            [
                                ((c[0] as u32 * a + 127) / 255) as u8,
                                ((c[1] as u32 * a + 127) / 255) as u8,
                                ((c[2] as u32 * a + 127) / 255) as u8,
                                c[3],
                            ]
                        })
                        .collect();
                    let t = upload_texture(&self.device, &self.queue, 256, 1, &px);
                    let bg = self.make_tex(&t, crate::gpu::SAMPLER_CLAMP);
                    self.upload(&m, Some(bg), *kind as f32, *to_gradient)
                }
            };
            if let Some(g) = &g
                && !self.masking
            {
                self.cpu_shapes
                    .entry((id, ratio))
                    .or_default()
                    .push((Arc::new(m.clone()), g.clone()));
            }
            out.extend(g);
        }
        self.shapes.insert(key, out.clone());
        out
    }

    /// A shape child of a scaling-grid clip, its outline moved as the grid lays it out: the
    /// corners keep their size, the edges stretch one way and the middle both
    /// (`DefineScalingGrid`). `place` maps the child's space into the clip's, `remap` the
    /// clip's unscaled space to its scaled one.
    #[allow(clippy::too_many_arguments)]
    fn scale9_meshes(
        &mut self,
        player: &Player,
        images: &mut dyn Images,
        id: u16,
        key: (DisplayId, [u32; 8]),
        place: Matrix,
        grid: crate::swf::Rect,
        remap: &dyn Fn(f32, f32) -> (f32, f32),
    ) -> Vec<Arc<GpuMesh>> {
        if let Some(m) = self.scale9.get(&key) {
            return m.clone();
        }
        self.shape_meshes(player, images, id, 0);
        let src = self.cpu_shapes.get(&(id, 0)).cloned().unwrap_or_default();
        let mut out = Vec::new();
        for (mesh, gpu) in src {
            let m = slice_mesh(&mesh, place, grid, remap);
            out.extend(self.upload(&m, gpu.binding.clone(), gpu.mode, gpu.to_gradient));
        }
        if self.scale9.len() > 4096 {
            self.scale9.clear();
        }
        self.scale9.insert(key, out.clone());
        out
    }

    /// Glyph `index` of `font` rasterised at `px` pixels per em into the atlas; `None` when
    /// it has no pixels or is too large for it (the outline is drawn instead).
    fn atlas_glyph(
        &mut self,
        font: &Arc<crate::swf::Font>,
        index: usize,
        px: (f32, f32),
    ) -> Option<crate::glyphs::Placed> {
        if !(1.0..=160.0).contains(&px.0) || !(1.0..=160.0).contains(&px.1) {
            return None;
        }
        let key = (
            Arc::as_ptr(font) as usize,
            index,
            (px.0 * 4.0).round() as u32,
            (px.1 * 4.0).round() as u32,
        );
        if let Some(p) = self.glyph_packer.get(key) {
            return p;
        }
        let records = font.glyphs.get(index).map(|g| g.outline.as_slice())?;
        let bitmap = crate::glyphs::rasterize_xy(records, px.0 / font.em, px.1 / font.em);
        let placed = bitmap.and_then(|b| {
            let (page, x, y) = self.glyph_packer.place(b.w, b.h)?;
            while self.glyph_pages.len() <= page {
                let texture = self.device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("gfx glyph atlas"),
                    size: wgpu::Extent3d {
                        width: GLYPH_PAGE,
                        height: GLYPH_PAGE,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                });
                let tex = self.make_tex(&texture, crate::gpu::SAMPLER_CLAMP);
                self.glyph_pages.push((texture, tex));
            }
            // White, premultiplied by coverage.
            let px: Vec<u8> = b.alpha.iter().flat_map(|&a| [a, a, a, a]).collect();
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.glyph_pages[page].0,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x, y, z: 0 },
                    aspect: wgpu::TextureAspect::All,
                },
                &px,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(b.w * 4),
                    rows_per_image: Some(b.h),
                },
                wgpu::Extent3d {
                    width: b.w,
                    height: b.h,
                    depth_or_array_layers: 1,
                },
            );
            let s = GLYPH_PAGE as f32;
            Some(crate::glyphs::Placed {
                page,
                uv: [
                    x as f32 / s,
                    y as f32 / s,
                    (x + b.w) as f32 / s,
                    (y + b.h) as f32 / s,
                ],
                left: b.left as f32,
                top: b.top as f32,
                w: b.w as f32,
                h: b.h as f32,
            })
        });
        self.glyph_packer.insert(key, placed);
        placed
    }

    fn glyph(&mut self, font: &Arc<crate::swf::Font>, index: usize) -> Arc<GlyphMesh> {
        let key = (Arc::as_ptr(font) as usize, index);
        self.glyphs
            .entry(key)
            .or_insert_with(|| {
                let recs = font
                    .glyphs
                    .get(index)
                    .map(|g| g.outline.as_slice())
                    .unwrap_or(&[]);
                Arc::new(tess::tessellate_glyph(recs))
            })
            .clone()
    }

    /// `pxs`: target pixels per local pixel, horizontally and vertically, when the field is
    /// drawn upright (glyphs come from the atlas, rasterised at that size); `None` draws the
    /// glyphs' outlines.
    fn text_meshes(
        &mut self,
        player: &mut Player,
        images: &mut dyn Images,
        d: DisplayId,
        pxs: Option<(f32, f32)>,
    ) -> Vec<Arc<GpuMesh>> {
        let fonts = player.vm.fonts.clone();
        let Some(t) = player.vm.display.get_mut(d).and_then(|o| o.text_mut()) else {
            return Vec::new();
        };
        // Quarter-pixel steps: a field that only moves keeps its glyphs.
        let px_key = pxs
            .map(|(x, y)| ((x * 64.0).round() as u64) << 32 | (y * 64.0).round() as u64)
            .unwrap_or(u64::MAX);
        let rev = t.revision;
        if let Some((r, k, m)) = self.texts.get(&d) {
            if *r == rev && *k == px_key {
                return m.clone();
            }
        }
        self.stats.texts_rebuilt += 1;
        let (bx, by, bw, bh) = t.bounds;
        let mut m = Mesh {
            paint: Paint::Solid,
            vertices: Vec::new(),
            indices: Vec::new(),
        };
        let quad = |m: &mut Mesh, x0: f32, y0: f32, x1: f32, y1: f32, c: [u8; 4]| {
            let base = m.vertices.len() as u32;
            for (x, y) in [(x0, y0), (x1, y0), (x1, y1), (x0, y1)] {
                m.vertices.push(Vertex {
                    pos: [x * 20.0, y * 20.0],
                    uv: [0.0; 2],
                    color: c,
                    extrude: [0.0; 2],
                });
            }
            m.indices
                .extend([base, base + 1, base + 2, base, base + 2, base + 3]);
        };
        if t.background {
            let c = t.background_color;
            quad(&mut m, bx, by, bx + bw, by + bh, [c[0], c[1], c[2], 255]);
        }
        if t.border {
            let c = t.border_color;
            let c = [c[0], c[1], c[2], 255];
            quad(&mut m, bx, by, bx + bw, by + 1.0, c);
            quad(&mut m, bx, by + bh - 1.0, bx + bw, by + bh, c);
            quad(&mut m, bx, by, bx + 1.0, by + bh, c);
            quad(&mut m, bx + bw - 1.0, by, bx + bw, by + bh, c);
        }
        let layout = t.layout(&*fonts).clone();
        let scroll = t.scroll.max(1) as usize - 1;
        let style = text_shadow_style(&t.shadow_style);
        let shadow_color = t.shadow_color;
        let top_skip = layout.lines.get(scroll).map(|l| l.top).unwrap_or(0.0) - crate::text::GUTTER;
        let mut glyphs = Vec::new();
        let mut inline_images = Vec::new();
        let selection = t.caret.map(|_| t.selection());
        let caret_position = t.caret.and_then(|c| layout.caret_position(c));
        let mut selection_mesh = m.clone();
        selection_mesh.vertices.clear();
        selection_mesh.indices.clear();
        let mut caret_mesh = selection_mesh.clone();
        for (line_index, line) in layout.lines.iter().enumerate().skip(scroll) {
            let y = line.top - top_skip;
            if y > bh + 1.0 {
                break;
            }
            if let Some((begin, end)) = selection {
                let selected: Vec<_> = line
                    .characters
                    .iter()
                    .filter(|c| c.index >= begin && c.index < end)
                    .collect();
                if let (Some(first), Some(last)) = (selected.first(), selected.last()) {
                    let x0 = first.x.max(crate::text::GUTTER);
                    let x1 = (last.x + last.advance).min(bw - crate::text::GUTTER);
                    let y1 = (y
                        + line.ascent
                        + line.descent
                        + if end > line.end {
                            line.height - line.ascent - line.descent
                        } else {
                            0.0
                        })
                    .min(bh);
                    if x1 > x0 && y1 > y.max(0.0) {
                        quad(
                            &mut selection_mesh,
                            bx + x0,
                            by + y.max(0.0),
                            bx + x1,
                            by + y1,
                            [0, 0, 0, 255],
                        );
                    }
                }
                if begin == end
                    && t.input
                    && t.caret_visible
                    && caret_position.is_some_and(|(i, _)| i == line_index)
                {
                    let x = caret_position.unwrap().1;
                    let c = t.format_at(begin).color;
                    if x >= 0.0 && x <= bw && y < bh {
                        quad(
                            &mut caret_mesh,
                            bx + x - 0.5,
                            by + y - 0.5,
                            bx + x + 0.5,
                            by + (y + line.ascent + line.descent).min(bh) - 0.5,
                            [c[0], c[1], c[2], 255],
                        );
                    }
                }
            }
            for g in &line.glyphs {
                glyphs.push((g.clone(), y - line.top));
            }
            for (image, x, y) in &line.images {
                inline_images.push((image.clone(), *x, *y - top_skip));
            }
            for (x0, x1, uy, c) in &line.underlines {
                let yy = uy - top_skip;
                quad(
                    &mut m,
                    bx + x0,
                    by + yy,
                    bx + x1,
                    by + yy + 1.0,
                    [c[0], c[1], c[2], 255],
                );
            }
        }
        // Background/border/underlines are drawn first. Keep shadow geometry separate
        // from the glyphs: a button's RGB replacement must not recolour black shadows.
        tess::fringe_mesh(&mut m);
        tess::fringe_mesh(&mut selection_mesh);
        let mut out: Vec<_> = self
            .upload(&m, None, 0.0, Matrix::IDENTITY)
            .into_iter()
            .collect();
        out.extend(self.upload_colored(&selection_mesh, None, 0.0, Matrix::IDENTITY, true));
        for (image, x, y) in inline_images {
            let (width, height) = image.size();
            if width <= 0.0 || height <= 0.0 {
                continue;
            }
            let left = x.max(0.0);
            let top = y.max(0.0);
            let right = (x + width).min(bw);
            let bottom = (y + height).min(bh);
            if right <= left || bottom <= top {
                continue;
            }
            let url = image
                .resource
                .as_ref()
                .map_or(image.src.as_str(), |r| &r.url);
            let Some((binding, _, _)) = self.bitmap(images, url, false, true) else {
                continue;
            };
            let mesh = Mesh {
                paint: Paint::Solid,
                vertices: [(left, top), (right, top), (right, bottom), (left, bottom)]
                    .map(|(px, py)| Vertex {
                        pos: [(bx + px) * 20.0, (by + py) * 20.0],
                        uv: [(px - x) / width, (py - y) / height],
                        color: [255; 4],
                        extrude: [0.0; 2],
                    })
                    .to_vec(),
                indices: vec![0, 1, 2, 0, 2, 3],
            };
            out.extend(self.upload(&mesh, Some(binding), 1.0, Matrix::IDENTITY));
        }
        m.vertices.clear();
        m.indices.clear();
        let mut shadow_mesh = m.clone();
        let mut text_pages: std::collections::BTreeMap<usize, Mesh> = Default::default();
        let mut shadow_pages: std::collections::BTreeMap<usize, Mesh> = Default::default();
        for (offset, is_shadow) in style
            .shadows
            .iter()
            .map(|offset| (offset, true))
            .chain(style.text.iter().map(|offset| (offset, false)))
        {
            for (g, dy) in &glyphs {
                let placed = pxs.and_then(|(px, py)| {
                    self.atlas_glyph(&g.font, g.glyph, (g.size * px, g.size * py))
                });
                let (ox, oy, c) = if is_shadow {
                    (offset.0, offset.1, shadow_color)
                } else {
                    (
                        offset.0,
                        offset.1,
                        if selection.is_some_and(|(b, e)| g.index >= b && g.index < e) {
                            [255; 3]
                        } else {
                            g.color
                        },
                    )
                };
                if let (Some(pl), Some((px, py))) = (placed, pxs) {
                    let pages = if is_shadow {
                        &mut shadow_pages
                    } else {
                        &mut text_pages
                    };
                    let target = pages.entry(pl.page).or_insert_with(|| Mesh {
                        paint: Paint::Solid,
                        vertices: Vec::new(),
                        indices: Vec::new(),
                    });
                    let x0 = (bx + g.x + ox) * 20.0 + pl.left / px * 20.0;
                    let y0 = (by + g.y + dy + oy) * 20.0 + pl.top / py * 20.0;
                    let (x1, y1) = (x0 + pl.w / px * 20.0, y0 + pl.h / py * 20.0);
                    let base = target.vertices.len() as u32;
                    for (x, y, u, v) in [
                        (x0, y0, pl.uv[0], pl.uv[1]),
                        (x1, y0, pl.uv[2], pl.uv[1]),
                        (x1, y1, pl.uv[2], pl.uv[3]),
                        (x0, y1, pl.uv[0], pl.uv[3]),
                    ] {
                        target.vertices.push(Vertex {
                            pos: [x, y],
                            uv: [u, v],
                            color: [c[0], c[1], c[2], 255],
                            extrude: [0.0; 2],
                        });
                    }
                    target
                        .indices
                        .extend([base, base + 1, base + 2, base, base + 2, base + 3]);
                    continue;
                }
                let mesh = self.glyph(&g.font, g.glyph);
                let scale = g.size * 20.0 / g.font.em;
                let target = if is_shadow { &mut shadow_mesh } else { &mut m };
                let base = target.vertices.len() as u32;
                for p in &mesh.0 {
                    target.vertices.push(Vertex {
                        pos: [
                            (bx + g.x + ox) * 20.0 + p[0] * scale,
                            (by + g.y + dy + oy) * 20.0 + p[1] * scale,
                        ],
                        uv: [0.0; 2],
                        color: [c[0], c[1], c[2], 255],
                        extrude: [0.0; 2],
                    });
                }
                target.indices.extend(mesh.1.iter().map(|i| i + base));
            }
        }
        tess::fringe_mesh(&mut shadow_mesh);
        tess::fringe_mesh(&mut caret_mesh);
        out.extend(self.upload_colored(&shadow_mesh, None, 0.0, Matrix::IDENTITY, true));
        for (page, mesh) in &shadow_pages {
            let tex = self.glyph_pages.get(*page).map(|p| p.1.clone());
            out.extend(self.upload_colored(mesh, tex, GLYPH_PAINT, Matrix::IDENTITY, true));
        }
        out.extend(self.upload(&m, None, 0.0, Matrix::IDENTITY));
        for (page, mesh) in &text_pages {
            let tex = self.glyph_pages.get(*page).map(|p| p.1.clone());
            out.extend(self.upload(mesh, tex, GLYPH_PAINT, Matrix::IDENTITY));
        }
        out.extend(self.upload(&caret_mesh, None, 0.0, Matrix::IDENTITY));
        self.texts.insert(d, (rev, px_key, out.clone()));
        out
    }

    #[allow(clippy::too_many_arguments)]
    fn walk(
        &mut self,
        player: &mut Player,
        images: &mut dyn Images,
        d: DisplayId,
        parent: Matrix,
        parent_cx: ColorTransform,
        parent_blend: Blend,
        view: &Viewport,
        stencil: u32,
        b: &mut Build,
    ) {
        let Some(o) = player.vm.display.get(d) else {
            return;
        };
        if !self.masking && o.mask_for.is_some() {
            return;
        }
        let mask = o.mask.filter(|m| player.vm.display.get(*m).is_some());
        if let Some(mask) = mask.filter(|_| !self.masking && !self.filter_stack.contains(&d)) {
            let mask_parent = player
                .vm
                .display
                .get(mask)
                .and_then(|o| o.parent)
                .map(|p| player.vm.display.world_matrix(p))
                .unwrap_or(Matrix::IDENTITY);
            let rect = self.mask_ops(
                player,
                images,
                mask,
                mask_parent,
                view,
                stencil,
                Stencil::Push,
                b,
            );
            if let Some(rect) = rect {
                let saved = b.clip;
                b.clip = clip_intersect(saved, rect);
                self.walk_content(
                    player,
                    images,
                    d,
                    parent,
                    parent_cx,
                    parent_blend,
                    view,
                    stencil,
                    b,
                );
                b.clip = saved;
                return;
            }
            self.walk_content(
                player,
                images,
                d,
                parent,
                parent_cx,
                parent_blend,
                view,
                stencil + 1,
                b,
            );
            self.mask_ops(
                player,
                images,
                mask,
                mask_parent,
                view,
                stencil + 1,
                Stencil::Pop,
                b,
            );
        } else {
            self.walk_content(
                player,
                images,
                d,
                parent,
                parent_cx,
                parent_blend,
                view,
                stencil,
                b,
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn walk_content(
        &mut self,
        player: &mut Player,
        images: &mut dyn Images,
        d: DisplayId,
        parent: Matrix,
        parent_cx: ColorTransform,
        parent_blend: Blend,
        view: &Viewport,
        stencil: u32,
        b: &mut Build,
    ) {
        let Some(o) = player.vm.display.get(d) else {
            return;
        };
        if !o.visible && !self.masking {
            return;
        }
        // Alpha/erase operate on an enclosing layer's isolated picture. Outside a
        // layer Flash draws neither the source nor an opaque stand-in for its mask.
        if !self.masking
            && matches!(o.blend, BlendMode::Alpha | BlendMode::Erase)
            && !self.filter_stack.iter().any(|d| {
                player
                    .vm
                    .display
                    .get(*d)
                    .is_some_and(|o| o.blend == BlendMode::Layer)
            })
        {
            return;
        }
        if !self.masking
            && !self.filter_stack.contains(&d)
            && (o.filters.iter().any(crate::filters::Filters::supported)
                || matches!(
                    o.blend,
                    BlendMode::Layer
                        | BlendMode::Alpha
                        | BlendMode::Erase
                        | BlendMode::Difference
                        | BlendMode::Invert
                        | BlendMode::Overlay
                        | BlendMode::HardLight
                ))
        {
            let filters = o.filters.clone();
            let blend = if o.blend == BlendMode::Normal {
                parent_blend
            } else {
                Blend::of(o.blend)
            };
            // Isolate just this object. Descendant filters/layers still run, and the
            // parent's stencil clips the final result rather than the filter input.
            self.filter_stack.push(d);
            let mut source = Build {
                ops: Vec::new(),
                uniforms: Vec::new(),
                clip: [0.0; 4],
            };
            self.walk(
                player,
                images,
                d,
                parent,
                parent_cx,
                Blend::Normal,
                view,
                0,
                &mut source,
            );
            self.filter_stack.pop();
            self.filtered_seen.insert(d);
            let size = (view.w as u32, view.h as u32);
            let scale = (view.k * 20.0).to_bits();
            let hit = self.filtered.get(&d).filter(|f| {
                f.size == size
                    && f.scale == scale
                    && f.filters == filters
                    && f.source.same_picture(&source)
            });
            let (mesh, clip) = if let Some(hit) = hit {
                (hit.mesh.clone(), hit.clip)
            } else {
                let clip = crate::filters::picture_bounds(
                    source.bounds(size),
                    &filters,
                    f32::from_bits(scale),
                );
                let targets = self.create_targets(size);
                self.draw_build(source.clone(), &targets);
                let output = self.filters.apply(
                    &self.device,
                    &self.queue,
                    targets.output,
                    &filters,
                    f32::from_bits(scale),
                );
                let binding = self.make_tex(&output, crate::gpu::SAMPLER_CLAMP);
                let mesh = Mesh {
                    paint: Paint::Solid,
                    vertices: [
                        (-1.0, 1.0, 0.0, 0.0),
                        (1.0, 1.0, 1.0, 0.0),
                        (1.0, -1.0, 1.0, 1.0),
                        (-1.0, -1.0, 0.0, 1.0),
                    ]
                    .map(|(x, y, u, v)| Vertex {
                        pos: [x, y],
                        uv: [u, v],
                        color: [255; 4],
                        extrude: [0.0; 2],
                    })
                    .to_vec(),
                    indices: vec![0, 1, 2, 0, 2, 3],
                };
                let mesh = self.upload(&mesh, Some(binding), 1.0, Matrix::IDENTITY);
                self.filtered.insert(
                    d,
                    Filtered {
                        source,
                        filters,
                        size,
                        scale,
                        clip,
                        mesh: mesh.clone(),
                    },
                );
                (mesh, clip)
            };
            if let Some(mesh) = mesh {
                let u = b.uniforms.len() as u32;
                b.uniforms.push(DrawU {
                    m0: [1.0, 0.0, 0.0, 0.0],
                    m1: [0.0, 1.0, 0.0, 0.0],
                    mul: [1.0; 4],
                    // Darken's MIN blend can change the destination for a transparent source.
                    // Other blends can omit the filtered picture's transparent exterior.
                    clip: if blend == Blend::Darken {
                        b.clip
                    } else {
                        clip_intersect(b.clip, clip)
                    },
                    ..Default::default()
                });
                b.push(&mesh, u, blend, stencil);
            }
            return;
        }
        let Some(o) = player.vm.display.get(d) else {
            return;
        };
        let world = parent.mul(&o.matrix);
        let cx = parent_cx.concat(&o.color);
        let blend = if self.filter_stack.last() == Some(&d) {
            Blend::Normal
        } else if o.blend == BlendMode::Normal {
            parent_blend
        } else {
            Blend::of(o.blend)
        };
        let meshes: Vec<Arc<GpuMesh>> = match &o.kind {
            Kind::Shape(_, id) => {
                let id = *id;
                self.shape_meshes(player, images, id, 0)
            }
            Kind::Morph(_, id) => {
                let (id, r) = (*id, o.ratio);
                self.shape_meshes(player, images, id, r)
            }
            Kind::Text(_) => {
                // Upright fields (stretched or not) take their glyphs from the atlas.
                let upright =
                    world.b.abs() < 1e-5 && world.c.abs() < 1e-5 && world.a > 0.0 && world.d > 0.0;
                let pxs = upright.then_some((world.a * view.k * 20.0, world.d * view.k * 20.0));
                self.text_meshes(player, images, d, pxs)
            }
            Kind::Bitmap { url, width, height } => {
                let (url, w, h) = (url.clone(), *width, *height);
                let key = (url.clone(), w.to_bits(), h.to_bits());
                if let Some(q) = self.bitmap_quads.get(&key) {
                    vec![q.clone()]
                } else {
                    match self.bitmap(images, &url, false, true) {
                        Some((bg, _, _)) => {
                            let mut m = Mesh {
                                paint: Paint::Solid,
                                vertices: Vec::new(),
                                indices: vec![0, 1, 2, 0, 2, 3],
                            };
                            for (x, y) in [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)] {
                                m.vertices.push(Vertex {
                                    pos: [x * w * 20.0, y * h * 20.0],
                                    uv: [x, y],
                                    color: [255; 4],
                                    extrude: [0.0; 2],
                                });
                            }
                            self.stats.uploads += 1;
                            let q = self.upload(&m, Some(bg), 1.0, Matrix::IDENTITY);
                            if let Some(q) = &q {
                                self.bitmap_quads.insert(key, q.clone());
                            }
                            q.into_iter().collect()
                        }
                        None => Vec::new(),
                    }
                }
            }
            Kind::Clip(c) => match &c.drawing {
                Some(dr) => {
                    let hash = drawing_hash(dr);
                    match self.drawings.get(&d) {
                        Some((h, m)) if *h == hash => m.clone(),
                        _ => {
                            let mut out = Vec::new();
                            for (color, pts) in dr.fills.iter().chain(dr.current.iter()) {
                                if let Some(m) = tess::tessellate_polygon(pts, *color) {
                                    self.stats.uploads += 1;
                                    out.extend(self.upload(&m, None, 0.0, Matrix::IDENTITY));
                                }
                            }
                            self.drawings.insert(d, (hash, out.clone()));
                            out
                        }
                    }
                }
                None => Vec::new(),
            },
        };
        let u = b.uniform(&world, &cx, view);
        for m in &meshes {
            if self.masking && m.fixed_rgb {
                continue;
            }
            b.push(m, u, blend, stencil);
        }
        let Some(o) = player.vm.display.get(d) else {
            return;
        };
        if o.children.is_empty() {
            return;
        }
        let kids: Vec<(i32, DisplayId, Option<i32>)> = o
            .children
            .iter()
            .map(|(k, c)| (*k, *c, player.vm.display.get(*c).and_then(|x| x.clip_depth)))
            .collect();
        // Open masks: (until depth, mask object, the clip before a rectangular mask).
        let mut masks: Vec<(i32, DisplayId, Option<[f32; 4]>)> = Vec::new();
        let mut level = stencil;
        let grid = self.grid_for(player, d, parent);
        for (depth, c, clip) in kids {
            while let Some((until, m, saved)) = masks.last().copied() {
                if depth > until {
                    match saved {
                        Some(saved) => b.clip = saved,
                        None => {
                            self.mask_ops(player, images, m, world, view, level, Stencil::Pop, b);
                            level -= 1;
                        }
                    }
                    masks.pop();
                } else {
                    break;
                }
            }
            if let Some(until) = clip {
                match self.mask_ops(player, images, c, world, view, level, Stencil::Push, b) {
                    Some(rect) => {
                        masks.push((until, c, Some(b.clip)));
                        b.clip = clip_intersect(b.clip, rect);
                    }
                    None => {
                        level += 1;
                        masks.push((until, c, None));
                    }
                }
                continue;
            }
            if let Some((noscale, remap)) = &grid {
                let shape = player.vm.display.get(c).and_then(|co| match &co.kind {
                    Kind::Shape(_, id)
                        if co.visible && co.filters.is_empty() && co.blend == BlendMode::Normal =>
                    {
                        Some((*id, co.matrix, co.color))
                    }
                    _ => None,
                });
                let shape = shape.filter(|_| {
                    !self.masking
                        && player
                            .vm
                            .display
                            .get(c)
                            .is_some_and(|co| co.mask.is_none() && co.mask_for.is_none())
                });
                if let Some((id, place, ccx)) = shape {
                    let key = (
                        c,
                        [
                            remap.sx.to_bits(),
                            remap.sy.to_bits(),
                            place.a.to_bits(),
                            place.b.to_bits(),
                            place.c.to_bits(),
                            place.d.to_bits(),
                            place.tx.to_bits(),
                            place.ty.to_bits(),
                        ],
                    );
                    let r = remap.clone();
                    let g = r.grid;
                    let meshes =
                        self.scale9_meshes(player, images, id, key, place, g, &move |x, y| {
                            r.apply(x, y)
                        });
                    let ccx = cx.concat(&ccx);
                    let u = b.uniform(noscale, &ccx, view);
                    for m in &meshes {
                        b.push(m, u, blend, level);
                    }
                    continue;
                }
            }
            self.walk(player, images, c, world, cx, blend, view, level, b);
        }
        while let Some((_, m, saved)) = masks.pop() {
            match saved {
                Some(saved) => b.clip = saved,
                None => {
                    self.mask_ops(player, images, m, world, view, level, Stencil::Pop, b);
                    level -= 1;
                }
            }
        }
    }

    /// Draws mask `m`'s shapes into the stencil (pushing or popping a level).
    #[allow(clippy::too_many_arguments)]
    fn mask_ops(
        &mut self,
        player: &mut Player,
        images: &mut dyn Images,
        m: DisplayId,
        parent: Matrix,
        view: &Viewport,
        level: u32,
        mode: Stencil,
        b: &mut Build,
    ) -> Option<[f32; 4]> {
        let mut tmp = Build {
            ops: Vec::new(),
            uniforms: std::mem::take(&mut b.uniforms),
            clip: b.clip,
        };
        // A mask counts however its clip is set to show.
        let vis = player.vm.display.get(m).map(|o| o.visible);
        if let Some(o) = player.vm.display.get_mut(m) {
            o.visible = true;
        }
        let was_masking = self.masking;
        self.masking = true;
        self.walk(
            player,
            images,
            m,
            parent,
            ColorTransform::IDENTITY,
            Blend::Normal,
            view,
            0,
            &mut tmp,
        );
        self.masking = was_masking;
        if let (Some(v), Some(o)) = (vis, player.vm.display.get_mut(m)) {
            o.visible = v;
        }
        b.uniforms = tmp.uniforms;
        // A mask that is one axis-aligned rectangle clips by rectangle: no stencil writes,
        // and the masked content keeps batching with its neighbours.
        if mode == Stencil::Push && tmp.ops.len() == 1 {
            let op = &tmp.ops[0];
            if let (Some(r), Some(u)) = (op.mesh.rect, b.uniforms.get(op.uniform as usize)) {
                if u.m0[1].abs() < 1e-7 && u.m1[0].abs() < 1e-7 {
                    let to_px = |x: f32, y: f32| {
                        let nx = u.m0[0] * x + u.m0[2];
                        let ny = u.m1[1] * y + u.m1[2];
                        ((nx + 1.0) * 0.5 * view.w, (1.0 - ny) * 0.5 * view.h)
                    };
                    let (ax, ay) = to_px(r[0], r[1]);
                    let (bx, by) = to_px(r[2], r[3]);
                    return Some([ax.min(bx), ay.min(by), ax.max(bx), ay.max(by)]);
                }
            }
        }
        let reference = level;
        for op in &tmp.ops {
            if let Some(r) = b.uniforms.get_mut(op.uniform as usize) {
                r.mode[1] = -1.0;
            }
        }
        for mut op in tmp.ops {
            op.stencil = mode;
            op.reference = reference;
            op.blend = Blend::Normal;
            b.ops.push(op);
        }
        None
    }

    /// For a clip with a scaling grid that is scaled: its matrix with the scale taken out
    /// (combined with `parent`), and how its points move.
    fn grid_for(&self, player: &Player, d: DisplayId, parent: Matrix) -> Option<(Matrix, Grid)> {
        let o = player.vm.display.get(d)?;
        let character = o.clip()?.character;
        let grid = *player.vm.movie.scale9.get(&character)?;
        let (sx, sy, rot) = o.scale_rotation();
        if (sx - 1.0).abs() < 1e-4 && (sy - 1.0).abs() < 1e-4 {
            return None;
        }
        let bounds = crate::avm1::display_api::local_bounds(&player.vm, d)?;
        let (s, c) = (rot.to_radians() as f32).sin_cos();
        let m = Matrix {
            a: c,
            b: s,
            c: -s,
            d: c,
            tx: o.matrix.tx,
            ty: o.matrix.ty,
        };
        Some((
            parent.mul(&m),
            Grid {
                sx: sx as f32,
                sy: sy as f32,
                bounds,
                grid,
            },
        ))
    }

    /// Reads the output back (straight RGBA), for tests and captures.
    pub fn capture(&self) -> Option<(u32, u32, Vec<u8>)> {
        let t = self.targets.as_ref()?;
        self.read_back(&t.output)
    }

    /// The prepared draw list (see [`Renderer::prepare_into`]) drawn over a transparent
    /// picture and read back (straight RGBA), for captures of a movie drawn that way.
    pub fn capture_prepared(&mut self) -> Option<(u32, u32, Vec<u8>)> {
        let (w, h) = self.direct.as_ref()?.size;
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("gfx capture"),
            size: wgpu::Extent3d {
                width: w.max(1),
                height: h.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        // New textures start cleared (transparent).
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        self.draw_into(&mut encoder, &texture);
        self.queue.submit(Some(encoder.finish()));
        self.read_back(&texture)
    }

    /// Reads an RGBA8 picture back, un-premultiplied.
    fn read_back(&self, texture: &wgpu::Texture) -> Option<(u32, u32, Vec<u8>)> {
        let (w, h) = (texture.width(), texture.height());
        let row = (w * 4).div_ceil(256) * 256;
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gfx capture"),
            size: (row * h) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(row),
                    rows_per_image: Some(h),
                },
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit(Some(enc.finish()));
        let slice = buf.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
        rx.recv().ok()?.ok()?;
        let data = slice.get_mapped_range().ok()?;
        let mut out = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            let s = (y * row) as usize;
            out.extend_from_slice(&data[s..s + (w * 4) as usize]);
        }
        drop(data);
        buf.unmap();
        // Un-premultiply.
        for p in out.chunks_exact_mut(4) {
            let a = p[3] as u32;
            if a > 0 && a < 255 {
                for c in &mut p[..3] {
                    *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
                }
            }
        }
        Some((w, h, out))
    }

    /// Forgets cached drawings of `d` (the drawing API changed it).
    pub fn forget_drawing(&mut self, d: DisplayId) {
        self.drawings.remove(&d);
    }
}

struct TextShadowStyle {
    shadows: Vec<(f32, f32)>,
    text: Vec<(f32, f32)>,
}

/// Scaleform's `s{...}t{...}` shadow and text passes, in pixels. The movie's
/// s-only styles retain the ordinary text pass at the origin. Repeated points
/// are retained, including the duplicate in the authored eight-offset outline.
fn text_shadow_style(style: &str) -> TextShadowStyle {
    let mut out = TextShadowStyle {
        shadows: Vec::new(),
        text: vec![(0.0, 0.0)],
    };
    let mut section = "";
    let mut explicit_text = false;
    let mut rest = style;
    while let Some(i) = rest.find('{') {
        let Some(j) = rest[i..].find('}') else { break };
        let prefix = rest[..i].trim();
        if !prefix.is_empty() {
            section = prefix;
        }
        let inner = &rest[i + 1..i + j];
        let point = inner.split_once(',').and_then(|(x, y)| {
            let (x, y) = (x.trim().parse::<f32>().ok()?, y.trim().parse::<f32>().ok()?);
            let limit = i32::MAX as f32 / 20.0;
            (x.is_finite() && y.is_finite() && x.abs() <= limit && y.abs() <= limit)
                .then_some((x, y))
        });
        if let Some(point) = point {
            match section {
                "s" => out.shadows.push(point),
                "t" => {
                    if !explicit_text {
                        out.text.clear();
                        explicit_text = true;
                    }
                    out.text.push(point);
                }
                _ => {}
            }
        }
        rest = &rest[i + j + 1..];
    }
    out
}

/// What one render did, for profiling (`THEDAS_UI_PROFILE`).
#[derive(Clone, Copy, Debug, Default)]
pub struct RenderStats {
    /// Draw calls.
    pub draws: usize,
    /// Meshes uploaded to the GPU (each a pair of new buffers).
    pub uploads: usize,
    /// Text fields whose mesh was rebuilt.
    pub texts_rebuilt: usize,
    /// Shapes tessellated.
    pub shapes_tessellated: usize,
    /// Time spent walking the display list and building meshes, and recording/submitting.
    pub build_us: u64,
    pub submit_us: u64,
}

/// A hash of a drawing's fills, to know when its meshes must be made again.
fn drawing_hash(d: &crate::display::Drawing) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for (c, pts) in d.fills.iter().chain(d.current.iter()) {
        c.hash(&mut h);
        pts.len().hash(&mut h);
        for (x, y) in pts {
            x.to_bits().hash(&mut h);
            y.to_bits().hash(&mut h);
        }
    }
    h.finish()
}

struct Viewport {
    k: f32,
    ox: f32,
    oy: f32,
    w: f32,
    h: f32,
}

#[derive(Clone)]
struct Build {
    ops: Vec<Op>,
    uniforms: Vec<DrawU>,
    /// The rectangular mask records made now get (see `DrawU::clip`).
    clip: [f32; 4],
}

/// The intersection of two clip rectangles (zeros meaning none); an empty result clips
/// everything.
fn clip_intersect(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    if a == [0.0; 4] {
        return b;
    }
    if b == [0.0; 4] {
        return a;
    }
    let r = [
        a[0].max(b[0]),
        a[1].max(b[1]),
        a[2].min(b[2]),
        a[3].min(b[3]),
    ];
    if r[2] <= r[0] || r[3] <= r[1] {
        [1.0, 1.0, 0.0, 0.0]
    } else {
        r
    }
}

/// Cuts `mesh` (moved by `place`) along the grid's lines so each piece lies in one cell, then
/// moves every point with `remap`: a cell's content then stretches only as its cell does.
fn slice_mesh(
    mesh: &Mesh,
    place: Matrix,
    grid: crate::swf::Rect,
    remap: &dyn Fn(f32, f32) -> (f32, f32),
) -> Mesh {
    let xs = [f32::MIN, grid.x_min as f32, grid.x_max as f32, f32::MAX];
    let ys = [f32::MIN, grid.y_min as f32, grid.y_max as f32, f32::MAX];
    let mut out = Mesh {
        paint: mesh.paint.clone(),
        vertices: Vec::new(),
        indices: Vec::new(),
    };
    let lerp = |a: &Vertex, b: &Vertex, t: f32| Vertex {
        pos: [
            a.pos[0] + (b.pos[0] - a.pos[0]) * t,
            a.pos[1] + (b.pos[1] - a.pos[1]) * t,
        ],
        uv: [
            a.uv[0] + (b.uv[0] - a.uv[0]) * t,
            a.uv[1] + (b.uv[1] - a.uv[1]) * t,
        ],
        color: a.color,
        extrude: [0.0; 2],
    };
    // Clips a polygon to `coord(v) >= edge` (keep_above) or `<= edge`.
    let clip = |poly: Vec<Vertex>, axis: usize, edge: f32, above: bool| -> Vec<Vertex> {
        let inside = |v: &Vertex| {
            if above {
                v.pos[axis] >= edge
            } else {
                v.pos[axis] <= edge
            }
        };
        let mut r = Vec::with_capacity(poly.len() + 2);
        for i in 0..poly.len() {
            let a = &poly[i];
            let b = &poly[(i + 1) % poly.len()];
            let (ia, ib) = (inside(a), inside(b));
            if ia {
                r.push(*a);
            }
            if ia != ib {
                let t = (edge - a.pos[axis]) / (b.pos[axis] - a.pos[axis]);
                r.push(lerp(a, b, t));
            }
        }
        r
    };
    let placed: Vec<Vertex> = mesh
        .vertices
        .iter()
        .map(|v| {
            let (x, y) = place.apply(v.pos[0], v.pos[1]);
            Vertex { pos: [x, y], ..*v }
        })
        .collect();
    for tri in mesh.indices.chunks_exact(3) {
        let t: Vec<Vertex> = tri
            .iter()
            .filter_map(|&i| placed.get(i as usize).copied())
            .collect();
        if t.len() != 3 {
            continue;
        }
        for cy in 0..3 {
            for cx in 0..3 {
                let mut p = t.clone();
                if xs[cx] > f32::MIN {
                    p = clip(p, 0, xs[cx], true);
                }
                if xs[cx + 1] < f32::MAX {
                    p = clip(p, 0, xs[cx + 1], false);
                }
                if ys[cy] > f32::MIN {
                    p = clip(p, 1, ys[cy], true);
                }
                if ys[cy + 1] < f32::MAX {
                    p = clip(p, 1, ys[cy + 1], false);
                }
                if p.len() < 3 {
                    continue;
                }
                let base = out.vertices.len() as u32;
                for v in &p {
                    let (x, y) = remap(v.pos[0], v.pos[1]);
                    out.vertices.push(Vertex { pos: [x, y], ..*v });
                }
                for k in 1..p.len() as u32 - 1 {
                    out.indices.extend([base, base + k, base + k + 1]);
                }
            }
        }
    }
    out
}

/// How a scaling grid moves a clip's unscaled points (twips) to its scaled ones.
#[derive(Clone, Debug)]
struct Grid {
    sx: f32,
    sy: f32,
    bounds: crate::swf::Rect,
    grid: crate::swf::Rect,
}

impl Grid {
    fn axis(v: f32, lo: f32, hi: f32, g0: f32, g1: f32, s: f32) -> f32 {
        let left = lo * s;
        let right = hi * s;
        let (a, b) = (left + (g0 - lo), right - (hi - g1));
        if b < a {
            // Too small for the fixed parts: scale everything (Flash does the same).
            return v * s;
        }
        if v <= g0 {
            left + (v - lo)
        } else if v >= g1 {
            right - (hi - v)
        } else if g1 > g0 {
            a + (v - g0) / (g1 - g0) * (b - a)
        } else {
            a
        }
    }

    fn apply(&self, x: f32, y: f32) -> (f32, f32) {
        let (b, g) = (&self.bounds, &self.grid);
        (
            Self::axis(
                x,
                b.x_min as f32,
                b.x_max as f32,
                g.x_min as f32,
                g.x_max as f32,
                self.sx,
            ),
            Self::axis(
                y,
                b.y_min as f32,
                b.y_max as f32,
                g.y_min as f32,
                g.y_max as f32,
                self.sy,
            ),
        )
    }
}

impl Build {
    /// Conservative screen bounds of colour draws, including the vertex shader's
    /// one-pixel fringe and glyph snapping. Stencil-only draws add no colour.
    fn bounds(&self, size: (u32, u32)) -> [f32; 4] {
        let mut out = [
            f32::INFINITY,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::NEG_INFINITY,
        ];
        for op in &self.ops {
            if op.stencil != Stencil::Content {
                continue;
            }
            let u = &self.uniforms[op.uniform as usize];
            let [x0, y0, x1, y1] = op.mesh.bounds;
            let mut r = [
                f32::INFINITY,
                f32::INFINITY,
                f32::NEG_INFINITY,
                f32::NEG_INFINITY,
            ];
            for (x, y) in [(x0, y0), (x1, y0), (x1, y1), (x0, y1)] {
                let px = ((u.m0[0] * x + u.m0[1] * y + u.m0[2]) * 0.5 + 0.5) * size.0 as f32;
                let py = (0.5 - (u.m1[0] * x + u.m1[1] * y + u.m1[2]) * 0.5) * size.1 as f32;
                if !px.is_finite() || !py.is_finite() {
                    return [0.0, 0.0, size.0 as f32, size.1 as f32];
                }
                r = [
                    r[0].min(px - 2.0),
                    r[1].min(py - 2.0),
                    r[2].max(px + 2.0),
                    r[3].max(py + 2.0),
                ];
            }
            if u.clip != [0.0; 4] {
                r = clip_intersect(r, u.clip);
            }
            if r[0] >= r[2] || r[1] >= r[3] {
                continue;
            }
            out = [
                out[0].min(r[0]),
                out[1].min(r[1]),
                out[2].max(r[2]),
                out[3].max(r[3]),
            ];
        }
        if out[0] >= out[2] || out[1] >= out[3] {
            [1.0, 1.0, 0.0, 0.0]
        } else {
            out
        }
    }

    /// Compare the ordered draw inputs, retaining the meshes so their identities cannot
    /// be recycled underneath a cache entry. Uniforms include transforms, tint, masks
    /// and viewport; nested filters contribute their own cached output meshes.
    fn same_picture(&self, other: &Self) -> bool {
        bytemuck::cast_slice::<DrawU, u8>(&self.uniforms)
            == bytemuck::cast_slice::<DrawU, u8>(&other.uniforms)
            && self.ops.len() == other.ops.len()
            && self.ops.iter().zip(&other.ops).all(|(a, b)| {
                Arc::ptr_eq(&a.mesh, &b.mesh)
                    && a.uniform == b.uniform
                    && a.blend == b.blend
                    && a.stencil == b.stencil
                    && a.reference == b.reference
            })
    }

    fn push(&mut self, m: &Arc<GpuMesh>, u: u32, blend: Blend, stencil: u32) {
        let mut un = self.uniforms[u as usize];
        if m.fixed_rgb {
            // The shadow still follows alpha (including fades) and the object's matrix.
            un.mul[..3].fill(1.0);
            un.add[..3].fill(0.0);
        }
        un.mode = [m.mode, blend.shader_mode() as f32, 0.0, 0.0];
        if m.mode >= 2.0 {
            let g = &m.to_gradient;
            un.g0 = [g.a, g.c, g.tx, 0.0];
            un.g1 = [g.b, g.d, g.ty, 0.0];
        }
        let idx = self.uniforms.len() as u32;
        self.uniforms.push(un);
        self.ops.push(Op {
            mesh: m.clone(),
            uniform: idx,
            blend,
            stencil: Stencil::Content,
            reference: stencil,
        });
    }

    fn uniform(&mut self, m: &Matrix, cx: &ColorTransform, v: &Viewport) -> u32 {
        let sx = 2.0 / v.w;
        let sy = -2.0 / v.h;
        let u = DrawU {
            m0: [
                sx * v.k * m.a,
                sx * v.k * m.c,
                sx * (v.k * m.tx + v.ox) - 1.0,
                0.0,
            ],
            m1: [
                sy * v.k * m.b,
                sy * v.k * m.d,
                sy * (v.k * m.ty + v.oy) + 1.0,
                0.0,
            ],
            g0: [1.0, 0.0, 0.0, 0.0],
            g1: [0.0, 1.0, 0.0, 0.0],
            mul: cx.mul,
            add: [
                cx.add[0] / 255.0,
                cx.add[1] / 255.0,
                cx.add[2] / 255.0,
                cx.add[3] / 255.0,
            ],
            mode: [0.0; 4],
            clip: self.clip,
            view: [v.w, v.h, 0.0, 0.0],
        };
        self.uniforms.push(u);
        self.uniforms.len() as u32 - 1
    }
}

fn premultiply(px: &mut [u8]) {
    for p in px.chunks_exact_mut(4) {
        let a = p[3] as u32;
        for c in &mut p[..3] {
            *c = ((*c as u32 * a + 127) / 255) as u8;
        }
    }
}

fn engine_image_name(name: &str) -> String {
    let name = name.to_ascii_lowercase();
    name.rsplit_once('.')
        .map(|(stem, _)| stem.to_string())
        .unwrap_or(name)
}

/// The exporter uses a small placeholder bitmap and sometimes an empty 0xffff fill.
/// A target replaces only a shape whose fills all reference the same engine image.
fn engine_shape(shape: &crate::swf::Shape, movie: &crate::swf::Movie) -> Option<String> {
    let mut name = None;
    if !shape.lines.is_empty() {
        return None;
    }
    for fill in &shape.fills {
        let crate::swf::Fill::Bitmap { id, .. } = fill else {
            return None;
        };
        if *id == u16::MAX {
            continue;
        }
        let Some(Character::Image(image)) = movie.characters.get(id) else {
            return None;
        };
        let next = engine_image_name(&image.file);
        if name.as_ref().is_some_and(|old| *old != next) {
            return None;
        }
        name = Some(next);
    }
    name
}

fn image_quad(bounds: crate::swf::Rect) -> Mesh {
    let x = [bounds.x_min as f32, bounds.x_max as f32];
    let y = [bounds.y_min as f32, bounds.y_max as f32];
    Mesh {
        paint: Paint::Solid,
        vertices: [(0, 0), (1, 0), (1, 1), (0, 1)]
            .map(|(u, v)| Vertex {
                pos: [x[u], y[v]],
                uv: [u as f32, v as f32],
                color: [255; 4],
                extrude: [0.0; 2],
            })
            .to_vec(),
        indices: vec![0, 1, 2, 0, 2, 3],
    }
}

fn write_image(queue: &wgpu::Queue, texture: &wgpu::Texture, pixels: &[u8]) {
    queue.write_texture(
        texture.as_image_copy(),
        pixels,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(texture.width() * 4),
            rows_per_image: None,
        },
        texture.size(),
    );
}

fn upload_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    w: u32,
    h: u32,
    px: &[u8],
) -> wgpu::Texture {
    device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: Some("gfx image"),
            size: wgpu::Extent3d {
                width: w.max(1),
                height: h.max(1),
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
        px,
    )
}

fn texture_binding(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    t: &wgpu::Texture,
    s: &wgpu::Sampler,
) -> wgpu::BindGroup {
    let view = t.create_view(&Default::default());
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("gfx texture"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(s),
            },
        ],
    })
}

fn lerp_records(
    a: &[crate::swf::Record],
    b: &[crate::swf::Record],
    t: f32,
) -> Vec<crate::swf::Record> {
    use crate::swf::Record as R;
    let l = |x: i32, y: i32| (x as f32 + (y - x) as f32 * t).round() as i32;
    let lp = |p: (i32, i32), q: (i32, i32)| (l(p.0, q.0), l(p.1, q.1));
    // Edges pair up in order; style records come from the start shape.
    let b_edges: Vec<&R> = b.iter().filter(|r| !matches!(r, R::Style { .. })).collect();
    let b_moves: Vec<(i32, i32)> = b
        .iter()
        .filter_map(|r| match r {
            R::Style {
                move_to: Some(m), ..
            } => Some(*m),
            _ => None,
        })
        .collect();
    let (mut ei, mut mi) = (0, 0);
    a.iter()
        .map(|r| match r {
            R::Style {
                move_to,
                fill0,
                fill1,
                line,
                new_styles,
            } => {
                let m = move_to.map(|m| {
                    let q = b_moves.get(mi).copied().unwrap_or(m);
                    mi += 1;
                    lp(m, q)
                });
                R::Style {
                    move_to: m,
                    fill0: *fill0,
                    fill1: *fill1,
                    line: *line,
                    new_styles: new_styles.clone(),
                }
            }
            R::Line { to } => {
                let q = match b_edges.get(ei) {
                    Some(R::Line { to: q }) | Some(R::Curve { to: q, .. }) => *q,
                    _ => *to,
                };
                ei += 1;
                R::Line { to: lp(*to, q) }
            }
            R::Curve { control, to } => {
                let (qc, qt) = match b_edges.get(ei) {
                    Some(R::Curve { control: c, to: q }) => (*c, *q),
                    Some(R::Line { to: q }) => (*q, *q),
                    _ => (*control, *to),
                };
                ei += 1;
                R::Curve {
                    control: lp(*control, qc),
                    to: lp(*to, qt),
                }
            }
        })
        .collect()
}

fn lerp_fill(a: &crate::swf::Fill, b: &crate::swf::Fill, t: f32) -> crate::swf::Fill {
    use crate::swf::Fill;
    match (a, b) {
        (Fill::Solid(x), Fill::Solid(y)) => {
            let l = |p: u8, q: u8| (p as f32 + (q as f32 - p as f32) * t).round() as u8;
            Fill::Solid(crate::swf::Color {
                r: l(x.r, y.r),
                g: l(x.g, y.g),
                b: l(x.b, y.b),
                a: l(x.a, y.a),
            })
        }
        _ => a.clone(),
    }
}

#[cfg(test)]
mod shadow_tests {
    use super::*;
    use crate::avm1::{Host, HostValue};
    use crate::swf::text::Glyph;
    use crate::swf::{EditText, Font, Movie, Record, Rect};
    use crate::text::{Fonts, Layout, Line, PlacedGlyph, TextField};

    #[test]
    fn shadow_styles_keep_every_movie_offset_and_separate_text_passes() {
        for (style, expected) in [
            ("s{1,1}", vec![(1.0, 1.0)]),
            ("s{0,1}{1,1}{1,0}", vec![(0.0, 1.0), (1.0, 1.0), (1.0, 0.0)]),
            (
                "s{0,-1}{1,0}{0,1}{0,-1}{-1,-1}{1,-1}{1,1}{-1,1}",
                vec![
                    (0.0, -1.0),
                    (1.0, 0.0),
                    (0.0, 1.0),
                    (0.0, -1.0),
                    (-1.0, -1.0),
                    (1.0, -1.0),
                    (1.0, 1.0),
                    (-1.0, 1.0),
                ],
            ),
        ] {
            let parsed = text_shadow_style(style);
            assert_eq!(parsed.shadows, expected);
            assert_eq!(parsed.text, [(0.0, 0.0)]);
        }
        let parsed = text_shadow_style("s{1.5, -2}{3,4} t{0,0}{8,0}");
        assert_eq!(parsed.shadows, [(1.5, -2.0), (3.0, 4.0)]);
        assert_eq!(parsed.text, [(0.0, 0.0), (8.0, 0.0)]);
        for style in [
            "",
            "s",
            "s{NaN,1}{0,inf}{1e38,0}{1,2,3}",
            "unknown{1,2}",
            "é{1,2}",
            "s{1,2",
            "t{NaN,0}",
        ] {
            let parsed = text_shadow_style(style);
            assert!(parsed.shadows.is_empty(), "{style}");
            assert_eq!(parsed.text, [(0.0, 0.0)], "{style}");
        }
    }

    struct Empty;
    impl Host for Empty {
        fn fscommand(&mut self, _: &str, _: &str) {}
        fn external_call(&mut self, _: &str, _: &[HostValue]) -> HostValue {
            HostValue::Undefined
        }
    }
    impl Fonts for Empty {
        fn font(&self, _: &str, _: bool, _: bool) -> Option<Arc<Font>> {
            None
        }
    }
    impl Images for Empty {
        fn image(&mut self, _: &str) -> Option<(u32, u32, Vec<u8>)> {
            None
        }
    }

    fn blend_player() -> Player {
        let bytes = b"FWS\x08\x12\0\0\0\x08\0\0\x1e\x01\0\x40\0\0\0";
        let mut p =
            Player::new(bytes, "blend-test", Box::new(Empty), Arc::new(Empty), None).unwrap();
        p.vm.movie = Arc::new(Movie {
            stage: Rect {
                x_max: 640,
                y_max: 640,
                ..Default::default()
            },
            ..Default::default()
        });
        p
    }

    fn quad(
        p: &mut Player,
        parent: DisplayId,
        depth: i32,
        color: [u8; 4],
        width: f32,
    ) -> DisplayId {
        let id = crate::player::create_empty(&mut p.vm, parent, "quad", depth).unwrap();
        p.vm.display
            .get_mut(id)
            .unwrap()
            .clip_mut()
            .unwrap()
            .drawing = Some(crate::display::Drawing {
            fills: vec![(
                color,
                vec![(0.0, 0.0), (width, 0.0), (width, 640.0), (0.0, 640.0)],
            )],
            ..Default::default()
        });
        id
    }

    #[test]
    fn filtered_pictures_reuse_draw_inputs_and_follow_changes() {
        let (device, queue) = crate::gpu::test_device();
        let mut p = blend_player();
        let root = p.vm.root;
        let group = crate::player::create_empty(&mut p.vm, root, "filtered", 1).unwrap();
        p.vm.display.get_mut(group).unwrap().filters = vec![crate::swf::Filter::Blur {
            blur: (3.0, 3.0),
            passes: 1,
        }];
        let child = quad(&mut p, group, 0, [255, 0, 0, 255], 320.0);
        let sibling = quad(&mut p, root, 2, [0, 0, 255, 255], 80.0);
        let mut r = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8Unorm);
        r.render(&mut p, &mut Empty, (32, 32));
        let cached = r.filtered[&group].mesh.clone().unwrap();
        let first = r.capture().unwrap();
        r.render(&mut p, &mut Empty, (32, 32));
        assert!(Arc::ptr_eq(
            &cached,
            r.filtered[&group].mesh.as_ref().unwrap()
        ));
        assert_eq!(r.capture().unwrap(), first);
        // A changing sibling (e.g. the name field's caret) must not redo the filter.
        p.vm.display.get_mut(sibling).unwrap().matrix.tx = 480.0;
        r.render(&mut p, &mut Empty, (32, 32));
        assert!(Arc::ptr_eq(
            &cached,
            r.filtered[&group].mesh.as_ref().unwrap()
        ));

        for change in 0..7 {
            let old = r.filtered[&group].mesh.clone().unwrap();
            let size = if change == 6 { (64, 48) } else { (32, 32) };
            match change {
                0 => p.vm.display.get_mut(group).unwrap().matrix.tx = 80.0,
                1 => p.vm.display.get_mut(child).unwrap().color.mul[0] = 0.5,
                2 => p.vm.display.get_mut(child).unwrap().matrix.ty = 80.0,
                3 => {
                    p.vm.display
                        .get_mut(child)
                        .unwrap()
                        .clip_mut()
                        .unwrap()
                        .drawing
                        .as_mut()
                        .unwrap()
                        .fills[0]
                        .0 = [0, 255, 0, 255];
                }
                4 => {
                    p.vm.display.get_mut(group).unwrap().filters = vec![crate::swf::Filter::Blur {
                        blur: (7.0, 7.0),
                        passes: 2,
                    }]
                }
                5 => p.vm.display.get_mut(child).unwrap().visible = false,
                _ => {}
            }
            r.render(&mut p, &mut Empty, size);
            assert!(
                !Arc::ptr_eq(&old, r.filtered[&group].mesh.as_ref().unwrap()),
                "change {change}"
            );
            let cached_pixels = r.capture().unwrap();
            // Recompute the same frame without retained filter pictures, pixel for pixel.
            r.filtered.clear();
            r.render(&mut p, &mut Empty, size);
            assert_eq!(r.capture().unwrap(), cached_pixels, "change {change}");
        }
        p.vm.display.get_mut(group).unwrap().visible = false;
        r.render(&mut p, &mut Empty, (32, 32));
        assert!(
            !r.filtered.contains_key(&group),
            "hidden pictures release their targets"
        );
    }

    #[test]
    fn nested_filtered_pictures_follow_same_size_engine_image_updates() {
        use crate::swf::{ExternalImage, Fill, Shape};

        let (device, queue) = crate::gpu::test_device();
        let mut p = blend_player();
        let root = p.vm.root;
        let layer = crate::player::create_empty(&mut p.vm, root, "layer", 0).unwrap();
        p.vm.display.get_mut(layer).unwrap().blend = BlendMode::Layer;
        let child = crate::player::create_empty(&mut p.vm, layer, "portrait", 0).unwrap();
        let shape = Arc::new(Shape {
            bounds: Rect {
                x_max: 640,
                y_max: 640,
                ..Default::default()
            },
            fills: vec![Fill::Bitmap {
                id: 1,
                matrix: Matrix::IDENTITY,
                repeat: false,
                smooth: true,
            }],
            ..Default::default()
        });
        let mut movie = Movie {
            stage: p.vm.movie.stage,
            ..Default::default()
        };
        movie.characters.insert(
            1,
            Character::Image(ExternalImage {
                format: 14,
                width: 2,
                height: 2,
                export_name: String::new(),
                file: "paperdoll.dds".into(),
            }),
        );
        movie.characters.insert(2, Character::Shape(shape.clone()));
        p.vm.movie = Arc::new(movie);
        let o = p.vm.display.get_mut(child).unwrap();
        o.kind = Kind::Shape(shape, 2);
        o.filters = vec![crate::swf::Filter::Blur {
            blur: (3.0, 3.0),
            passes: 1,
        }];
        let mut r = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8Unorm);
        for color in [[255, 0, 0, 255], [0, 0, 255, 255]] {
            r.set_engine_image("paperdoll", Some((2, 2, &color.repeat(4))));
            r.render(&mut p, &mut Empty, (32, 32));
            let (_, _, pixels) = r.capture().unwrap();
            assert_eq!(&pixels[(16 * 32 + 16) * 4..(16 * 32 + 17) * 4], &color);
            let cached = r.filtered[&layer].mesh.clone().unwrap();
            r.render(&mut p, &mut Empty, (32, 32));
            assert!(Arc::ptr_eq(
                &cached,
                r.filtered[&layer].mesh.as_ref().unwrap()
            ));
        }
        r.set_engine_image("paperdoll", None);
        r.render(&mut p, &mut Empty, (32, 32));
        assert!(r.capture().unwrap().2.iter().all(|b| *b == 0));
    }

    #[test]
    fn filtered_bounds_preserve_full_picture_pixels() {
        use crate::swf::{Color, Filter};
        let (device, queue) = crate::gpu::test_device();
        let filters = [
            Filter::Blur {
                blur: (3.5, 5.5),
                passes: 2,
            },
            Filter::Glow {
                color: Color {
                    r: 0,
                    g: 255,
                    b: 0,
                    a: 180,
                },
                blur: (5.0, 3.0),
                strength: 1.5,
                inner: false,
                knockout: false,
                composite_source: true,
                passes: 1,
            },
            Filter::DropShadow {
                color: Color {
                    r: 20,
                    g: 30,
                    b: 40,
                    a: 200,
                },
                blur: (3.0, 5.0),
                angle: 0.7,
                distance: 4.5,
                strength: 1.0,
                inner: false,
                knockout: false,
                hide_object: false,
                passes: 1,
            },
        ];
        for filter in filters {
            for size in [(32, 32), (64, 48)] {
                let mut p = blend_player();
                let root = p.vm.root;
                quad(&mut p, root, 0, [40, 80, 120, 255], 640.0);
                let d = quad(&mut p, root, 1, [255, 0, 0, 180], 160.0);
                let o = p.vm.display.get_mut(d).unwrap();
                o.filters = vec![filter.clone()];
                o.matrix = Matrix {
                    a: 0.7,
                    b: 0.2,
                    c: -0.1,
                    d: 0.2,
                    tx: 200.0,
                    ty: 160.0,
                };
                let mut r = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8Unorm);
                r.render(&mut p, &mut Empty, size);
                let bounded = r.capture().unwrap();
                let clip = r.filtered[&d].clip;
                assert!((clip[2] - clip[0]) * (clip[3] - clip[1]) < (size.0 * size.1) as f32);
                r.filtered.get_mut(&d).unwrap().clip = [0.0; 4];
                r.render(&mut p, &mut Empty, size);
                assert_eq!(r.capture().unwrap(), bounded, "{filter:?} {size:?}");
                // MIN darken is deliberately kept full-screen: transparent samples
                // still affect its destination, unlike ordinary source-over blending.
                p.vm.display.get_mut(d).unwrap().blend = BlendMode::Darken;
                r.render(&mut p, &mut Empty, size);
                let dark = r.capture().unwrap();
                r.filtered.get_mut(&d).unwrap().clip = clip;
                r.render(&mut p, &mut Empty, size);
                assert_eq!(r.capture().unwrap(), dark);
            }
        }
    }

    #[test]
    fn native_inline_images_render_clip_scroll_and_reuse_cached_text_meshes() {
        use crate::avm1::value::Value;

        struct Pixels(usize);
        impl Images for Pixels {
            fn image(&mut self, name: &str) -> Option<(u32, u32, Vec<u8>)> {
                self.0 += 1;
                let color = match name {
                    "coin" => [120, 200, 60, 255],
                    "other" => [5, 30, 240, 255],
                    _ => return None,
                };
                Some((2, 2, color.repeat(4)))
            }
        }
        let (device, queue) = crate::gpu::test_device();
        let mut player = blend_player();
        let root = player.vm.root;
        let field =
            crate::player::create_text(&mut player.vm, root, "images", 1, [0.0, 0.0, 10.0, 8.0])
                .unwrap();
        let value = player.vm.display_object(field);
        player.vm.set(&value, "html", Value::Bool(true));
        player
            .vm
            .set(&value, "textColor", Value::Number(0xff0000 as f64));
        player.vm.set(
            &value,
            "htmlText",
            Value::from("<img src='coin' width='16' height='12' hspace='2' vspace='-2'>"),
        );
        player.vm.display.get_mut(field).unwrap().color.mul[3] = 0.5;
        let mut images = Pixels(0);
        let mut renderer = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8Unorm);
        assert!(renderer.render_if_dirty(&mut player, &mut images, (32, 32)));
        let pixel = |renderer: &Renderer, x: usize, y: usize| {
            let (_, _, pixels) = renderer.capture().unwrap();
            <[u8; 4]>::try_from(&pixels[(y * 32 + x) * 4..(y * 32 + x + 1) * 4]).unwrap()
        };
        let sample = pixel(&renderer, 5, 3);
        for (actual, expected) in sample.into_iter().zip([120, 200, 60, 128]) {
            assert!((actual as i32 - expected).abs() <= 2, "{sample:?}");
        }
        assert_eq!(pixel(&renderer, 11, 3)[3], 0);
        assert_eq!(pixel(&renderer, 5, 9)[3], 0);
        assert!(!renderer.render_if_dirty(&mut player, &mut images, (32, 32)));
        assert_eq!(images.0, 1);

        player.vm.set(&value, "_width", Value::Number(24.0));
        player.vm.set(&value, "_height", Value::Number(24.0));
        renderer.render_if_dirty(&mut player, &mut images, (32, 32));
        assert_eq!(pixel(&renderer, 17, 10)[3], 128);
        assert_eq!(images.0, 1);
        player.vm.set(&value, "multiline", Value::Bool(true));
        player.vm.set(
            &value,
            "htmlText",
            Value::from(
                "<img src='coin' width='6' height='4'><br><img src='other' width='6' height='4'>",
            ),
        );
        player.vm.set(&value, "scroll", Value::Number(2.0));
        renderer.render_if_dirty(&mut player, &mut images, (32, 32));
        let sample = pixel(&renderer, 3, 10);
        for (actual, expected) in sample.into_iter().zip([5, 30, 240, 128]) {
            assert!((actual as i32 - expected).abs() <= 2, "{sample:?}");
        }
        assert_eq!(images.0, 2);
    }

    #[test]
    fn bitmap_fills_keep_authored_sampling_and_cache_each_mode() {
        use crate::display::{Kind, Object};
        use crate::swf::{ExternalImage, Fill, Shape};

        struct Pixels;
        impl Images for Pixels {
            fn image(&mut self, _: &str) -> Option<(u32, u32, Vec<u8>)> {
                Some((2, 1, vec![255, 0, 0, 255, 0, 0, 255, 255]))
            }
        }
        let (device, queue) = crate::gpu::test_device();
        let mut player = blend_player();
        let root = player.vm.root;
        let bounds = Rect {
            x_max: 640,
            y_max: 160,
            ..Default::default()
        };
        let modes = [(false, false), (false, true), (true, false), (true, true)];
        let mut movie = Movie {
            stage: player.vm.movie.stage,
            ..Default::default()
        };
        movie.characters.insert(
            1,
            Character::Image(ExternalImage {
                file: "same-sampling-image".into(),
                width: 2,
                height: 1,
                format: 0,
                export_name: String::new(),
            }),
        );
        for (row, (repeat, smooth)) in modes.into_iter().enumerate() {
            let id = row as u16 + 2;
            let shape = Arc::new(Shape {
                bounds,
                fills: vec![Fill::Bitmap {
                    id: 1,
                    repeat,
                    smooth,
                    matrix: Matrix {
                        a: 160.0,
                        d: 160.0,
                        ..Matrix::IDENTITY
                    },
                }],
                records: vec![
                    Record::Style {
                        move_to: Some((0, 0)),
                        fill0: None,
                        fill1: Some(1),
                        line: None,
                        new_styles: None,
                    },
                    Record::Line { to: (640, 0) },
                    Record::Line { to: (640, 160) },
                    Record::Line { to: (0, 160) },
                    Record::Line { to: (0, 0) },
                ],
                ..Default::default()
            });
            movie.characters.insert(id, Character::Shape(shape.clone()));
            let mut object = Object::new(Kind::Shape(shape, id));
            object.parent = Some(root);
            object.matrix.ty = row as f32 * 160.0;
            let display = player.vm.display.add(object);
            player
                .vm
                .display
                .get_mut(root)
                .unwrap()
                .children
                .insert(row as i32, display);
        }
        player.vm.movie = Arc::new(movie);
        let mut renderer = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8Unorm);
        renderer.render(&mut player, &mut Pixels, (32, 32));
        let (_, _, pixels) = renderer.capture().unwrap();
        let pixel = |x: usize, row: usize| {
            &pixels[((row * 8 + 4) * 32 + x) * 4..((row * 8 + 4) * 32 + x + 1) * 4]
        };
        for row in [0, 2] {
            assert_eq!(pixel(7, row), [255, 0, 0, 255]);
            assert_eq!(pixel(8, row), [0, 0, 255, 255]);
        }
        for row in [1, 3] {
            let blended = pixel(7, row);
            assert!(blended[0] > 100 && blended[0] < 200, "{blended:?}");
            assert!(blended[2] > 50 && blended[2] < 150, "{blended:?}");
            assert_eq!(blended[3], 255);
        }
        assert_eq!(pixel(19, 0), [0, 0, 255, 255]);
        assert_eq!(pixel(19, 2), [255, 0, 0, 255]);
        assert_eq!(pixel(0, 1), [255, 0, 0, 255]);
        assert!(pixel(0, 3)[2] > 50);
        assert_eq!(renderer.bitmaps.len(), 4);
        // The cached bindings retain their individual samplers on later renders.
        renderer.render(&mut player, &mut Pixels, (32, 32));
        assert_eq!(renderer.capture().unwrap().2, pixels);
    }

    #[test]
    fn destination_blends_handle_transparent_pictures_and_flatten_the_source() {
        let (device, queue) = crate::gpu::test_device();
        let mut renderer = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8Unorm);
        for mode in [
            BlendMode::Difference,
            BlendMode::Invert,
            BlendMode::Overlay,
            BlendMode::HardLight,
        ] {
            for da in [0, 128, 255] {
                for sa in [0, 128, 255] {
                    let mut p = blend_player();
                    let root = p.vm.root;
                    let dst = [204, 77, 26, da];
                    let src = [64, 192, 128, sa];
                    if da != 0 {
                        quad(&mut p, root, 0, dst, 640.0);
                    }
                    let group = crate::player::create_empty(&mut p.vm, root, "group", 1).unwrap();
                    p.vm.display.get_mut(group).unwrap().blend = mode;
                    // The second, opaque child replaces the first before the group blends.
                    quad(&mut p, group, 0, [255, 0, 0, 255], 320.0);
                    quad(&mut p, group, 1, [src[0], src[1], src[2], 255], 320.0);
                    p.vm.display.get_mut(group).unwrap().color.mul[3] = sa as f32 / 255.0;
                    // Inherited alpha is applied per child in this player. A single child
                    // provides the exact alpha fixture; overlap at full opacity tests flattening.
                    if sa != 255 {
                        let first = p.vm.display.get(group).unwrap().children[&0];
                        p.vm.display.get_mut(first).unwrap().visible = false;
                    }
                    renderer.render(&mut p, &mut Empty, (32, 32));
                    let (_, _, pixels) = renderer.capture().unwrap();
                    let pixel = &pixels[(16 * 32 + 8) * 4..(16 * 32 + 9) * 4];
                    let (s_alpha, d_alpha) = (sa as f32 / 255.0, da as f32 / 255.0);
                    let alpha = s_alpha + d_alpha * (1.0 - s_alpha);
                    for c in 0..3 {
                        let (s, d) = (src[c] as f32 / 255.0, dst[c] as f32 / 255.0);
                        let mixed = match mode {
                            BlendMode::Difference => (d - s).abs(),
                            BlendMode::Invert => 1.0 - d,
                            BlendMode::Overlay | BlendMode::HardLight => {
                                let threshold = if mode == BlendMode::Overlay { d } else { s };
                                if threshold <= 0.5 {
                                    2.0 * s * d
                                } else {
                                    1.0 - 2.0 * (1.0 - s) * (1.0 - d)
                                }
                            }
                            _ => unreachable!(),
                        };
                        let expected = if alpha == 0.0 {
                            0.0
                        } else {
                            (s * s_alpha * (1.0 - d_alpha)
                                + d * d_alpha * (1.0 - s_alpha)
                                + s_alpha * d_alpha * mixed)
                                / alpha
                                * 255.0
                        };
                        assert!(
                            (pixel[c] as f32 - expected).abs() <= 3.0,
                            "{mode:?} sa={sa} da={da}: {pixel:?}, channel {c} expected {expected}"
                        );
                    }
                    assert!((pixel[3] as f32 - alpha * 255.0).abs() <= 1.0);
                    let right = &pixels[(16 * 32 + 24) * 4..(16 * 32 + 25) * 4];
                    assert_eq!(right[3], da);
                    if da != 0 {
                        for c in 0..3 {
                            assert!((right[c] as i32 - dst[c] as i32).abs() <= 1);
                        }
                    }
                }
            }
        }
    }

    /// A picture to draw into, filled with `rgba`, and a way to read it back.
    fn picture(device: &wgpu::Device, queue: &wgpu::Queue, rgba: [u8; 4]) -> wgpu::Texture {
        let size = wgpu::Extent3d {
            width: 32,
            height: 32,
            depth_or_array_layers: 1,
        };
        let t = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Bgra8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let bgra = [rgba[2], rgba[1], rgba[0], rgba[3]];
        queue.write_texture(
            t.as_image_copy(),
            &bgra.repeat(32 * 32),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(32 * 4),
                rows_per_image: Some(32),
            },
            size,
        );
        t
    }

    fn read_picture(device: &wgpu::Device, queue: &wgpu::Queue, t: &wgpu::Texture) -> Vec<[u8; 4]> {
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 256 * 32,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = device.create_command_encoder(&Default::default());
        enc.copy_texture_to_buffer(
            t.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(256),
                    rows_per_image: Some(32),
                },
            },
            t.size(),
        );
        queue.submit(Some(enc.finish()));
        buf.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        let data = buf.slice(..).get_mapped_range().unwrap();
        data.chunks_exact(4)
            .map(|p| [p[2], p[1], p[0], p[3]])
            .collect()
    }

    #[test]
    fn drawing_into_a_picture_blends_with_it_and_reuses_the_draw_list() {
        let (device, queue) = crate::gpu::test_device();
        let mut p = blend_player();
        let root = p.vm.root;
        // Left half: grey multiplied onto the picture. Right quarter: inverted.
        let grey = quad(&mut p, root, 0, [128, 128, 128, 255], 320.0);
        p.vm.display.get_mut(grey).unwrap().blend = BlendMode::Multiply;
        let inv = quad(&mut p, root, 1, [255; 4], 160.0);
        p.vm.display.get_mut(inv).unwrap().blend = BlendMode::Invert;
        p.vm.display.get_mut(inv).unwrap().matrix.tx = 480.0;
        let mut renderer = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8Unorm);
        assert!(renderer.prepare_into(&mut p, &mut Empty, (32, 32)));
        let target = picture(&device, &queue, [200, 100, 50, 255]);
        let draw = |r: &mut Renderer| {
            let mut enc = device.create_command_encoder(&Default::default());
            r.draw_into(&mut enc, &target);
            queue.submit(Some(enc.finish()));
        };
        draw(&mut renderer);
        let px = read_picture(&device, &queue, &target);
        let at = |x: usize| px[16 * 32 + x];
        let near = |a: [u8; 4], b: [u8; 4]| {
            a.iter()
                .zip(b)
                .all(|(x, y)| (*x as i32 - y as i32).abs() <= 1)
        };
        assert!(near(at(8), [100, 50, 25, 255]), "{:?}", at(8));
        assert!(near(at(20), [200, 100, 50, 255]), "{:?}", at(20));
        assert!(near(at(28), [55, 155, 205, 255]), "{:?}", at(28));
        // Unchanged: nothing to rebuild, and the same list draws again on a fresh picture.
        assert!(!renderer.prepare_into(&mut p, &mut Empty, (32, 32)));
        let target2 = picture(&device, &queue, [200, 100, 50, 255]);
        let mut enc = device.create_command_encoder(&Default::default());
        renderer.draw_into(&mut enc, &target2);
        queue.submit(Some(enc.finish()));
        assert_eq!(read_picture(&device, &queue, &target2), px);
    }

    #[test]
    fn destination_snapshots_preserve_open_masks_and_later_draws() {
        let (device, queue) = crate::gpu::test_device();
        let mut p = blend_player();
        let root = p.vm.root;
        quad(&mut p, root, 0, [64, 128, 192, 255], 640.0);
        let mask = quad(&mut p, root, 1, [255; 4], 320.0);
        p.vm.display.get_mut(mask).unwrap().clip_depth = Some(3);
        for depth in [2, 3] {
            let id = quad(&mut p, root, depth, [128; 4], 640.0);
            p.vm.display.get_mut(id).unwrap().color.mul[3] = 255.0 / 128.0;
            p.vm.display.get_mut(id).unwrap().blend = BlendMode::Invert;
        }
        // Both inversions cancel inside the mask; neither may affect its exterior.
        let last = quad(&mut p, root, 4, [255, 0, 0, 255], 80.0);
        p.vm.display.get_mut(last).unwrap().matrix.tx = 480.0;
        let mut renderer = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8Unorm);
        renderer.render(&mut p, &mut Empty, (32, 32));
        let (_, _, pixels) = renderer.capture().unwrap();
        for x in [8, 20, 30] {
            assert_eq!(
                &pixels[(16 * 32 + x) * 4..(16 * 32 + x + 1) * 4],
                &[64, 128, 192, 255]
            );
        }
        assert_eq!(
            &pixels[(16 * 32 + 25) * 4..(16 * 32 + 26) * 4],
            &[255, 0, 0, 255]
        );
    }

    #[test]
    fn moving_script_mask_clips_filtered_content_in_stage_space() {
        let (device, queue) = crate::gpu::test_device();
        let bytes = b"FWS\x08\x12\0\0\0\x08\0\0\x1e\x01\0\x40\0\0\0";
        let mut p =
            Player::new(bytes, "mask-test", Box::new(Empty), Arc::new(Empty), None).unwrap();
        p.vm.movie = Arc::new(Movie {
            stage: Rect {
                x_max: 640,
                y_max: 640,
                ..Default::default()
            },
            ..Default::default()
        });
        let root = p.vm.root;
        let quad = |p: &mut Player, parent, depth, color, width| {
            let id = crate::player::create_empty(&mut p.vm, parent, "quad", depth).unwrap();
            p.vm.display
                .get_mut(id)
                .unwrap()
                .clip_mut()
                .unwrap()
                .drawing = Some(crate::display::Drawing {
                fills: vec![(
                    color,
                    vec![(0.0, 0.0), (width, 0.0), (width, 640.0), (0.0, 640.0)],
                )],
                ..Default::default()
            });
            id
        };
        quad(&mut p, root, 0, [0, 0, 255, 255], 640.0);
        let target = quad(&mut p, root, 1, [255, 0, 0, 255], 640.0);
        p.vm.display.get_mut(target).unwrap().filters = vec![crate::swf::Filter::Blur {
            blur: (3.0, 3.0),
            passes: 1,
        }];
        let container = crate::player::create_empty(&mut p.vm, root, "elsewhere", 2).unwrap();
        let mask = quad(&mut p, container, 0, [255; 4], 160.0);
        let o = p.vm.display.get_mut(mask).unwrap();
        o.visible = false;
        o.color.mul[3] = 0.0;
        // Overlapping mask children form a union, rather than incrementing twice.
        quad(&mut p, mask, 0, [255; 4], 80.0);
        assert!(p.vm.display.set_mask(target, Some(mask)));
        let mut renderer = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8Unorm);
        for offset in [160.0, 320.0] {
            p.vm.display.get_mut(container).unwrap().matrix.tx = offset;
            renderer.render(&mut p, &mut Empty, (32, 32));
            let (_, _, pixels) = renderer.capture().unwrap();
            for x in [4, 12, 20, 28] {
                let expected = if (x as f32 * 20.0) > offset && (x as f32 * 20.0) < offset + 160.0 {
                    [255, 0, 0, 255]
                } else {
                    [0, 0, 255, 255]
                };
                assert_eq!(&pixels[(16 * 32 + x) * 4..(16 * 32 + x + 1) * 4], &expected);
            }
        }
        p.vm.display.set_mask(target, None);
        renderer.render(&mut p, &mut Empty, (32, 32));
        let (_, _, pixels) = renderer.capture().unwrap();
        assert_eq!(
            &pixels[(16 * 32 + 12) * 4..(16 * 32 + 13) * 4],
            &[255, 0, 0, 255]
        );
        let shape = Arc::new(crate::swf::Shape {
            bounds: Rect {
                x_max: 640,
                y_max: 640,
                ..Default::default()
            },
            lines: vec![crate::swf::Line {
                width: 320,
                color: crate::swf::Color {
                    r: 255,
                    g: 255,
                    b: 255,
                    a: 255,
                },
                fill: None,
                no_scale: (false, false),
                start_cap: crate::swf::LineCap::Round,
                end_cap: crate::swf::LineCap::Round,
                join: crate::swf::LineJoin::Round,
                no_close: false,
            }],
            records: vec![
                Record::Style {
                    move_to: Some((0, 320)),
                    fill0: None,
                    fill1: None,
                    line: Some(1),
                    new_styles: None,
                },
                Record::Line { to: (640, 320) },
            ],
            ..Default::default()
        });
        Arc::get_mut(&mut p.vm.movie)
            .unwrap()
            .characters
            .insert(1, Character::Shape(shape.clone()));
        let mut object = crate::display::Object::new(Kind::Shape(shape, 1));
        object.parent = Some(root);
        let stroke = p.vm.display.add(object);
        p.vm.display
            .get_mut(root)
            .unwrap()
            .children
            .insert(3, stroke);
        p.vm.display.set_mask(target, Some(stroke));
        renderer.render(&mut p, &mut Empty, (32, 32));
        let (_, _, pixels) = renderer.capture().unwrap();
        assert_eq!(
            &pixels[(16 * 32 + 12) * 4..(16 * 32 + 13) * 4],
            &[0, 0, 255, 255]
        );
    }

    #[test]
    fn alpha_and_erase_affect_only_the_enclosing_layer() {
        let (device, queue) = crate::gpu::test_device();
        let bytes = b"FWS\x08\x12\0\0\0\x08\0\0\x1e\x01\0\x40\0\0\0";
        let mut p =
            Player::new(bytes, "blend-test", Box::new(Empty), Arc::new(Empty), None).unwrap();
        p.vm.movie = Arc::new(Movie {
            stage: Rect {
                x_max: 640,
                y_max: 640,
                ..Default::default()
            },
            ..Default::default()
        });
        let root = p.vm.root;
        let quad = |p: &mut Player, parent, depth, color, width| {
            let id = crate::player::create_empty(&mut p.vm, parent, "quad", depth).unwrap();
            p.vm.display
                .get_mut(id)
                .unwrap()
                .clip_mut()
                .unwrap()
                .drawing = Some(crate::display::Drawing {
                fills: vec![(
                    color,
                    vec![(0.0, 0.0), (width, 0.0), (width, 640.0), (0.0, 640.0)],
                )],
                ..Default::default()
            });
            id
        };
        quad(&mut p, root, 0, [255, 0, 0, 255], 640.0);
        let layer = crate::player::create_empty(&mut p.vm, root, "layer", 1).unwrap();
        p.vm.display.get_mut(layer).unwrap().blend = BlendMode::Layer;
        quad(&mut p, layer, 0, [0, 255, 0, 255], 640.0);
        let mask = quad(&mut p, layer, 1, [0, 0, 0, 128], 320.0);
        // This full-screen mask has no enclosing layer and must not cover the scene.
        let outside = quad(&mut p, root, 2, [0, 0, 0, 255], 640.0);
        let mut renderer = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8Unorm);
        for mode in [BlendMode::Erase, BlendMode::Alpha] {
            p.vm.display.get_mut(mask).unwrap().blend = mode;
            p.vm.display.get_mut(outside).unwrap().blend = mode;
            renderer.render(&mut p, &mut Empty, (32, 32));
            let (_, _, pixels) = renderer.capture().unwrap();
            let pixel = |x: usize| &pixels[(16 * 32 + x) * 4..(16 * 32 + x + 1) * 4];
            let left = pixel(8);
            assert!((left[0] as i32 - 128).abs() <= 2 && (left[1] as i32 - 128).abs() <= 2);
            assert_eq!(left[3], 255);
            assert_eq!(
                pixel(24),
                if mode == BlendMode::Erase {
                    &[0, 255, 0, 255]
                } else {
                    &[255, 0, 0, 255]
                }
            );
        }
    }

    #[test]
    fn selection_and_caret_refresh_cached_text_on_blink_and_focus_changes() {
        let (device, queue) = crate::gpu::test_device();
        let mut p = blend_player();
        let font = Arc::new(Font {
            em: 100.0,
            glyphs: vec![Glyph {
                outline: vec![
                    Record::Style {
                        move_to: Some((0, 0)),
                        fill0: None,
                        fill1: Some(1),
                        line: None,
                        new_styles: None,
                    },
                    Record::Line { to: (100, 0) },
                    Record::Line { to: (100, 100) },
                    Record::Line { to: (0, 100) },
                    Record::Line { to: (0, 0) },
                ],
                ..Default::default()
            }],
            ..Default::default()
        });
        let mut text = TextField::new(Arc::new(EditText::default()), None);
        text.bounds = (0.0, 0.0, 24.0, 20.0);
        text.background = true;
        text.background_color = [100; 3];
        text.default_format.color = [255, 0, 0];
        text.set_text("a ");
        text.layout = Some(Layout {
            lines: vec![Line {
                start: 0,
                end: 2,
                x: 2.0,
                top: 2.0,
                height: 10.0,
                ascent: 8.0,
                descent: 2.0,
                characters: vec![
                    crate::text::CharacterBox {
                        index: 0,
                        x: 2.0,
                        advance: 5.0,
                    },
                    crate::text::CharacterBox {
                        index: 1,
                        x: 7.0,
                        advance: 5.0,
                    },
                ],
                glyphs: vec![PlacedGlyph {
                    index: 0,
                    font,
                    glyph: 0,
                    x: 2.0,
                    y: 3.0,
                    size: 4.0,
                    color: [255, 0, 0],
                }],
                ..Default::default()
            }],
            ..Default::default()
        });
        let mut object = crate::display::Object::new(Kind::Text(Box::new(text)));
        object.parent = Some(p.vm.root);
        let field = p.vm.display.add(object);
        p.vm.display
            .get_mut(p.vm.root)
            .unwrap()
            .children
            .insert(1, field);
        crate::player::set_focus(&mut p.vm, Some(field));
        p.vm.display
            .get_mut(field)
            .unwrap()
            .text_mut()
            .unwrap()
            .set_selection(0, 2);
        let mut renderer = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8Unorm);
        renderer.render_if_dirty(&mut p, &mut Empty, (32, 32));
        let pixel = |renderer: &Renderer, x: usize, y: usize| {
            let (_, _, pixels) = renderer.capture().unwrap();
            pixels[(y * 32 + x) * 4..(y * 32 + x + 1) * 4].to_vec()
        };
        assert_eq!(pixel(&renderer, 4, 5), [255, 255, 255, 255]);
        assert_eq!(pixel(&renderer, 10, 5), [0, 0, 0, 255]);
        p.vm.display
            .get_mut(field)
            .unwrap()
            .text_mut()
            .unwrap()
            .set_selection(2, 2);
        p.vm.dirty = true;
        renderer.render_if_dirty(&mut p, &mut Empty, (32, 32));
        assert_eq!(pixel(&renderer, 4, 5), [255, 0, 0, 255]);
        assert_ne!(pixel(&renderer, 12, 5), [100, 100, 100, 255]);
        p.tick(499.0);
        assert!(!p.take_dirty());
        p.tick(1.0);
        renderer.render_if_dirty(&mut p, &mut Empty, (32, 32));
        assert_eq!(pixel(&renderer, 12, 5), [100, 100, 100, 255]);
        p.tick(500.0);
        renderer.render_if_dirty(&mut p, &mut Empty, (32, 32));
        assert_ne!(pixel(&renderer, 12, 5), [100, 100, 100, 255]);
        crate::player::set_focus(&mut p.vm, None);
        renderer.render_if_dirty(&mut p, &mut Empty, (32, 32));
        assert_eq!(pixel(&renderer, 12, 5), [100, 100, 100, 255]);
    }

    #[test]
    fn tinted_text_keeps_black_shadows_and_inherits_alpha() {
        let (device, queue) = crate::gpu::test_device();
        let bytes = b"FWS\x08\x12\0\0\0\x08\0\0\x1e\x01\0\x40\0\0\0";
        let mut player =
            Player::new(bytes, "shadow-test", Box::new(Empty), Arc::new(Empty), None).unwrap();
        player.vm.movie = Arc::new(Movie {
            stage: Rect {
                x_min: 0,
                y_min: 0,
                x_max: 640,
                y_max: 640,
            },
            ..Default::default()
        });
        let font = Arc::new(Font {
            em: 100.0,
            glyphs: vec![Glyph {
                outline: vec![
                    Record::Style {
                        move_to: Some((0, 0)),
                        fill0: None,
                        fill1: Some(1),
                        line: None,
                        new_styles: None,
                    },
                    Record::Line { to: (100, 0) },
                    Record::Line { to: (100, 100) },
                    Record::Line { to: (0, 100) },
                    Record::Line { to: (0, 0) },
                ],
                ..Default::default()
            }],
            ..Default::default()
        });
        let mut text = TextField::new(
            Arc::new(EditText {
                bounds: Rect {
                    x_min: 0,
                    y_min: 0,
                    x_max: 640,
                    y_max: 640,
                },
                ..Default::default()
            }),
            None,
        );
        text.shadow_style = "s{8,0}".into();
        text.layout = Some(Layout {
            lines: vec![Line {
                glyphs: vec![PlacedGlyph {
                    index: 0,
                    font,
                    glyph: 0,
                    x: 2.0,
                    y: 10.0,
                    size: 6.0,
                    color: [255; 3],
                }],
                ..Default::default()
            }],
            ..Default::default()
        });
        let root = player.vm.root;
        let mut object = crate::display::Object::new(Kind::Text(Box::new(text)));
        object.parent = Some(root);
        let id = player.vm.display.add(object);
        let root = player.vm.display.get_mut(root).unwrap();
        root.children.insert(1, id);
        root.color = ColorTransform {
            mul: [0.0, 0.0, 0.0, 0.5],
            add: [255.0, 200.0, 0.0, 0.0],
        };
        let mut renderer = Renderer::new(&device, &queue, wgpu::TextureFormat::Rgba8Unorm);
        renderer.render(&mut player, &mut Empty, (32, 32));
        let (_, _, pixels) = renderer.capture().unwrap();
        let pixel = |x: usize, y: usize| &pixels[(y * 32 + x) * 4..(y * 32 + x + 1) * 4];
        let glyph = pixel(4, 14);
        assert!(glyph[0] > 250 && (glyph[1] as i32 - 200).abs() < 3 && glyph[2] == 0);
        assert!((glyph[3] as i32 - 128).abs() < 2);
        let shadow = pixel(12, 14);
        assert_eq!(&shadow[..3], &[0, 0, 0]);
        assert!((shadow[3] as i32 - 128).abs() < 2);

        let text = player.vm.display.get_mut(id).unwrap().text_mut().unwrap();
        text.shadow_style = "s{8,0}{8,8}t{0,0}{0,8}".into();
        text.revision += 1;
        player.vm.dirty = true;
        renderer.render_if_dirty(&mut player, &mut Empty, (32, 32));
        let (_, _, pixels) = renderer.capture().unwrap();
        for y in [14, 22] {
            let glyph = &pixels[(y * 32 + 4) * 4..(y * 32 + 5) * 4];
            assert!(glyph[0] > 250 && (glyph[1] as i32 - 200).abs() < 3 && glyph[2] == 0);
            assert!((glyph[3] as i32 - 128).abs() < 2);
            let shadow = &pixels[(y * 32 + 12) * 4..(y * 32 + 13) * 4];
            assert_eq!(&shadow[..3], &[0, 0, 0]);
            assert!((shadow[3] as i32 - 128).abs() < 2);
        }
        assert!(!renderer.render_if_dirty(&mut player, &mut Empty, (32, 32)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::swf::{Color, ExternalImage, Fill, Movie, Rect, Shape};

    #[test]
    fn engine_slot_uses_the_authored_rect_and_only_its_own_image() {
        let mut movie = Movie::default();
        for (id, file) in [(1, "PaperDoll.dds"), (2, "other.dds")] {
            movie.characters.insert(
                id,
                Character::Image(ExternalImage {
                    format: 14,
                    width: 16,
                    height: 16,
                    export_name: String::new(),
                    file: file.into(),
                }),
            );
        }
        let bitmap = |id| Fill::Bitmap {
            id,
            matrix: Matrix::IDENTITY,
            repeat: false,
            smooth: true,
        };
        let mut shape = Shape {
            bounds: Rect {
                x_min: -100,
                x_max: 5300,
                y_min: 200,
                y_max: 9380,
            },
            fills: vec![bitmap(u16::MAX), bitmap(1)],
            ..Default::default()
        };
        assert_eq!(engine_shape(&shape, &movie).as_deref(), Some("paperdoll"));
        let quad = image_quad(shape.bounds);
        assert_eq!(quad.vertices[0].pos, [-100.0, 200.0]);
        assert_eq!(quad.vertices[2].pos, [5300.0, 9380.0]);
        assert_eq!(quad.vertices[2].uv, [1.0, 1.0]);
        shape.fills.push(bitmap(2));
        assert_eq!(engine_shape(&shape, &movie), None);
        shape.fills = vec![
            Fill::Solid(Color {
                r: 255,
                g: 255,
                b: 255,
                a: 255,
            }),
            bitmap(1),
        ];
        assert_eq!(engine_shape(&shape, &movie), None);
    }
}
