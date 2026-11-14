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

const SHADER: &str = r#"
struct Draw {
    m0: vec4<f32>,
    m1: vec4<f32>,
    g0: vec4<f32>,
    g1: vec4<f32>,
    mul: vec4<f32>,
    add: vec4<f32>,
    mode: vec4<f32>,
};
@group(0) @binding(0) var<uniform> d: Draw;
@group(1) @binding(0) var tex: texture_2d<f32>;
@group(1) @binding(1) var smp: sampler;
@group(2) @binding(0) var backdrop: texture_2d<f32>;

struct V {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) local: vec2<f32>,
};

@vertex
fn vs(@location(0) pos: vec2<f32>, @location(1) uv: vec2<f32>, @location(2) color: vec4<f32>) -> V {
    var o: V;
    let x = d.m0.x * pos.x + d.m0.y * pos.y + d.m0.z;
    let y = d.m1.x * pos.x + d.m1.y * pos.y + d.m1.z;
    o.pos = vec4<f32>(x, y, 0.0, 1.0);
    o.uv = uv;
    o.color = color;
    o.local = pos;
    return o;
}

fn shade(i: V) -> vec4<f32> {
    var c = i.color;
    let mode = i32(d.mode.x + 0.5);
    if (mode == 1) {
        let t = textureSample(tex, smp, i.uv);
        c = vec4<f32>(t.rgb / max(t.a, 0.00001), t.a);
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
        let r = textureSample(tex, smp, vec2<f32>(clamp(t, 0.0, 1.0) * (255.0 / 256.0) + 0.5 / 256.0, 0.5));
        c = vec4<f32>(r.rgb / max(r.a, 0.00001), r.a);
    } else {
        let t = textureSample(tex, smp, vec2<f32>(0.5, 0.5));
        c = c * t.a;
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
    let mode = i32(d.mode.y + 0.5);
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
}

const UNIFORM_STRIDE: u64 = 256;
const SAMPLES: u32 = 4;

struct GpuMesh {
    vb: wgpu::Buffer,
    ib: wgpu::Buffer,
    count: u32,
    mode: f32,
    to_gradient: Matrix,
    /// The texture binding (ramp or bitmap); `None` for solid colours.
    binding: Option<Arc<wgpu::BindGroup>>,
    /// Scaleform text shadows keep shadowColor when the text's parent is tinted.
    fixed_rgb: bool,
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
type Bitmap = (Arc<wgpu::BindGroup>, f32, f32);

struct EngineImage {
    texture: wgpu::Texture,
    binding: Arc<wgpu::BindGroup>,
}

pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    format: wgpu::TextureFormat,
    pipelines: HashMap<(Blend, Stencil), wgpu::RenderPipeline>,
    uniform_layout: wgpu::BindGroupLayout,
    texture_layout: wgpu::BindGroupLayout,
    shader: wgpu::ShaderModule,
    pipeline_layout: wgpu::PipelineLayout,
    uniforms: Option<(wgpu::Buffer, wgpu::BindGroup, u64)>,
    white: Arc<wgpu::BindGroup>,
    clamp: wgpu::Sampler,
    repeat: wgpu::Sampler,
    shapes: HashMap<(u16, u16), Vec<Arc<GpuMesh>>>,
    cpu_shapes: HashMap<(u16, u16), Vec<CpuShape>>,
    scale9: HashMap<(DisplayId, [u32; 8]), Vec<Arc<GpuMesh>>>,
    glyphs: HashMap<(usize, usize), Arc<GlyphMesh>>,
    texts: HashMap<DisplayId, (u64, Vec<Arc<GpuMesh>>)>,
    drawings: HashMap<DisplayId, Vec<Arc<GpuMesh>>>,
    bitmaps: HashMap<(String, bool), Option<Bitmap>>,
    engine_images: HashMap<String, Option<EngineImage>>,
    targets: Option<Targets>,
    /// Draws in the last frame.
    pub draws: usize,
    filters: crate::filters::Filters,
    filter_stack: Vec<DisplayId>,
    masking: bool,
}

struct Targets {
    size: (u32, u32),
    output: wgpu::Texture,
    msaa: wgpu::TextureView,
    stencil: wgpu::TextureView,
}

impl Renderer {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("gfx"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("gfx draw"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(std::mem::size_of::<DrawU>() as u64),
                },
                count: None,
            }],
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
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("gfx"),
            bind_group_layouts: &[
                Some(&uniform_layout),
                Some(&texture_layout),
                Some(&texture_layout),
            ],
            immediate_size: 0,
        });
        let sampler = |mode| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("gfx"),
                address_mode_u: mode,
                address_mode_v: mode,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            })
        };
        let clamp = sampler(wgpu::AddressMode::ClampToEdge);
        let repeat = sampler(wgpu::AddressMode::Repeat);
        let white_tex = upload_texture(device, queue, 1, 1, &[255, 255, 255, 255]);
        let white = Arc::new(texture_binding(device, &texture_layout, &white_tex, &clamp));
        Self {
            device: device.clone(),
            queue: queue.clone(),
            format,
            pipelines: HashMap::new(),
            uniform_layout,
            texture_layout,
            shader,
            pipeline_layout,
            uniforms: None,
            white,
            clamp,
            repeat,
            shapes: HashMap::new(),
            cpu_shapes: HashMap::new(),
            scale9: HashMap::new(),
            glyphs: HashMap::new(),
            texts: HashMap::new(),
            drawings: HashMap::new(),
            bitmaps: HashMap::new(),
            engine_images: HashMap::new(),
            targets: None,
            draws: 0,
            filters: crate::filters::Filters::new(device, format),
            filter_stack: Vec::new(),
            masking: false,
        }
    }

    fn pipeline(&mut self, blend: Blend, stencil: Stencil) -> &wgpu::RenderPipeline {
        let key = (blend, stencil);
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
                        buffers: &[Some(wgpu::VertexBufferLayout {
                            array_stride: std::mem::size_of::<Vertex>() as u64,
                            step_mode: wgpu::VertexStepMode::Vertex,
                            attributes: &wgpu::vertex_attr_array![
                                0 => Float32x2, 1 => Float32x2, 2 => Unorm8x4
                            ],
                        })],
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
                        count: SAMPLES,
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
                            format: self.format,
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
        let msaa = tex(self.format, SAMPLES, wgpu::TextureUsages::RENDER_ATTACHMENT)
            .create_view(&Default::default());
        let stencil = tex(
            wgpu::TextureFormat::Depth24PlusStencil8,
            SAMPLES,
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

    /// The texture [`Renderer::render`] draws into.
    pub fn output(&self) -> Option<&wgpu::Texture> {
        self.targets.as_ref().map(|t| &t.output)
    }

    /// Registers or refreshes an engine render target (e.g. `paperdoll`). Shapes filled
    /// only with that external image display the complete target in their authored bounds.
    /// Same-size refreshes keep the texture and all cached shape bindings alive.
    pub fn set_engine_image(&mut self, name: &str, image: Option<(u32, u32, &[u8])>) {
        let key = engine_image_name(name);
        let old = self.engine_images.entry(key).or_default();
        if let Some((w, h, rgba)) = image {
            if w == 0 || h == 0 || rgba.len() != w as usize * h as usize * 4 {
                return;
            }
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
            let binding = Arc::new(texture_binding(
                &self.device,
                &self.texture_layout,
                &texture,
                &self.clamp,
            ));
            *old = Some(EngineImage { texture, binding });
        } else if let Some(old) = old {
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
        };
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
        if let Some(targets) = self.targets.take() {
            self.draw_build(&b, &targets);
            self.targets = Some(targets);
        }
        // Drop text meshes of fields that are gone.
        self.texts
            .retain(|d, _| player.vm.display.get(*d).is_some());
        self.drawings
            .retain(|d, _| player.vm.display.get(*d).is_some());
    }

    fn draw_build(&mut self, b: &Build, t: &Targets) {
        // Upload the uniforms.
        let need = (b.uniforms.len().max(1) as u64) * UNIFORM_STRIDE;
        if self.uniforms.as_ref().is_none_or(|u| u.2 < need) {
            let cap = need.next_power_of_two().max(UNIFORM_STRIDE * 64);
            let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("gfx uniforms"),
                size: cap,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("gfx uniforms"),
                layout: &self.uniform_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &buf,
                        offset: 0,
                        size: wgpu::BufferSize::new(std::mem::size_of::<DrawU>() as u64),
                    }),
                }],
            });
            self.uniforms = Some((buf, bg, cap));
        }
        let mut bytes = vec![0u8; b.uniforms.len() * UNIFORM_STRIDE as usize];
        for (i, u) in b.uniforms.iter().enumerate() {
            let at = i * UNIFORM_STRIDE as usize;
            bytes[at..at + std::mem::size_of::<DrawU>()].copy_from_slice(bytemuck::bytes_of(u));
        }
        if let Some((buf, _, _)) = &self.uniforms {
            if !bytes.is_empty() {
                self.queue.write_buffer(buf, 0, &bytes);
            }
        }
        for op in &b.ops {
            self.pipeline(op.blend, op.stencil);
        }
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("gfx") });
        // Resolve before a destination-dependent blend, then sample a snapshot rather
        // than the active attachment. Keep the multisample colour/stencil between passes
        // so later siblings and open clip-depth masks retain their state.
        let snapshot = b.ops.iter().any(|op| op.blend.shader_mode() != 0).then(|| {
            let tex = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("gfx blend backdrop"),
                size: t.output.size(),
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: self.format,
                usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let binding = texture_binding(&self.device, &self.texture_layout, &tex, &self.clamp);
            (tex, binding)
        });
        // Even a first draw that needs a backdrop must see a cleared target.
        let mut start = 0;
        let mut first = true;
        loop {
            let end = (start + usize::from(!first)..b.ops.len())
                .find(|&i| b.ops[i].blend.shader_mode() != 0)
                .unwrap_or(b.ops.len());
            if !first && start < b.ops.len() && b.ops[start].blend.shader_mode() != 0 {
                let (texture, _) = snapshot.as_ref().unwrap();
                encoder.copy_texture_to_texture(
                    t.output.as_image_copy(),
                    texture.as_image_copy(),
                    t.output.size(),
                );
            }
            let out_view = t.output.create_view(&Default::default());
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("gfx"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &t.msaa,
                    resolve_target: Some(&out_view),
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: if first {
                            wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT)
                        } else {
                            wgpu::LoadOp::Load
                        },
                        store: if snapshot.is_some() {
                            wgpu::StoreOp::Store
                        } else {
                            wgpu::StoreOp::Discard
                        },
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &t.stencil,
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
            let Some((_, ubg, _)) = &self.uniforms else {
                return;
            };
            // All pipelines share this layout; ordinary draws do not read group 2.
            pass.set_bind_group(
                2,
                snapshot.as_ref().map(|(_, bg)| bg).unwrap_or(&self.white),
                &[],
            );
            for op in &b.ops[start..end] {
                let Some(p) = self.pipelines.get(&(op.blend, op.stencil)) else {
                    continue;
                };
                pass.set_pipeline(p);
                pass.set_stencil_reference(op.reference);
                pass.set_bind_group(0, ubg, &[op.uniform * UNIFORM_STRIDE as u32]);
                pass.set_bind_group(1, &**op.mesh.binding.as_ref().unwrap_or(&self.white), &[]);
                pass.set_vertex_buffer(0, op.mesh.vb.slice(..));
                pass.set_index_buffer(op.mesh.ib.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..op.mesh.count, 0, 0..1);
            }
            drop(pass);
            if end == b.ops.len() {
                break;
            }
            start = end;
            first = false;
        }
        self.queue.submit(Some(encoder.finish()));
    }

    fn upload(
        &self,
        m: &Mesh,
        binding: Option<Arc<wgpu::BindGroup>>,
        mode: f32,
        to_gradient: Matrix,
    ) -> Option<Arc<GpuMesh>> {
        self.upload_colored(m, binding, mode, to_gradient, false)
    }

    fn upload_colored(
        &self,
        m: &Mesh,
        binding: Option<Arc<wgpu::BindGroup>>,
        mode: f32,
        to_gradient: Matrix,
        fixed_rgb: bool,
    ) -> Option<Arc<GpuMesh>> {
        if m.indices.is_empty() {
            return None;
        }
        let vb = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("gfx vertices"),
                contents: bytemuck::cast_slice(&m.vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });
        let ib = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("gfx indices"),
                contents: bytemuck::cast_slice(&m.indices),
                usage: wgpu::BufferUsages::INDEX,
            });
        Some(Arc::new(GpuMesh {
            vb,
            ib,
            count: m.indices.len() as u32,
            mode,
            to_gradient,
            binding,
            fixed_rgb,
        }))
    }

    fn bitmap(&mut self, images: &mut dyn Images, name: &str, repeat: bool) -> Option<Bitmap> {
        let key = (name.to_string(), repeat);
        if let Some(b) = self.bitmaps.get(&key) {
            return b.clone();
        }
        let r = images.image(name).map(|(w, h, mut px)| {
            premultiply(&mut px);
            let t = upload_texture(&self.device, &self.queue, w, h, &px);
            let s = if repeat { &self.repeat } else { &self.clamp };
            (
                Arc::new(texture_binding(&self.device, &self.texture_layout, &t, s)),
                w as f32,
                h as f32,
            )
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
        if let Some(m) = self.shapes.get(&(id, ratio)) {
            return m.clone();
        }
        let movie = player.vm.movie.clone();
        if let Some(Character::Shape(shape)) = movie.characters.get(&id)
            && let Some(name) = engine_shape(shape, &movie)
            && let Some(image) = self.engine_images.get(&name)
        {
            let out: Vec<_> = image
                .as_ref()
                .and_then(|image| {
                    self.upload(
                        &image_quad(shape.bounds),
                        Some(image.binding.clone()),
                        1.0,
                        Matrix::IDENTITY,
                    )
                })
                .into_iter()
                .collect();
            self.shapes.insert((id, ratio), out.clone());
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
                tess::tessellate_shape(&s.fills, &s.lines, &s.records, &size)
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
                    id: bid, repeat, ..
                } => {
                    let file = match movie.characters.get(bid) {
                        Some(Character::Image(img)) => img.file.clone(),
                        _ => String::new(),
                    };
                    let tex = self.bitmap(images, &file, *repeat);
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
                    let bg = Arc::new(texture_binding(
                        &self.device,
                        &self.texture_layout,
                        &t,
                        &self.clamp,
                    ));
                    self.upload(&m, Some(bg), *kind as f32, *to_gradient)
                }
            };
            if let Some(g) = &g {
                self.cpu_shapes
                    .entry((id, ratio))
                    .or_default()
                    .push((Arc::new(m.clone()), g.clone()));
            }
            out.extend(g);
        }
        self.shapes.insert((id, ratio), out.clone());
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

    fn text_meshes(&mut self, player: &mut Player, d: DisplayId) -> Vec<Arc<GpuMesh>> {
        let fonts = player.vm.fonts.clone();
        let Some(t) = player.vm.display.get_mut(d).and_then(|o| o.text_mut()) else {
            return Vec::new();
        };
        let rev = t.revision;
        if let Some((r, m)) = self.texts.get(&d) {
            if *r == rev {
                return m.clone();
            }
        }
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
        let shadow = shadow_offsets(&t.shadow_style);
        let shadow_color = t.shadow_color;
        let top_skip = layout.lines.get(scroll).map(|l| l.top).unwrap_or(0.0) - crate::text::GUTTER;
        let mut glyphs = Vec::new();
        for line in layout.lines.iter().skip(scroll) {
            let y = line.top - top_skip;
            if y > bh + 1.0 {
                break;
            }
            for g in &line.glyphs {
                glyphs.push((g.clone(), y - line.top));
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
        let mut out: Vec<_> = self
            .upload(&m, None, 0.0, Matrix::IDENTITY)
            .into_iter()
            .collect();
        m.vertices.clear();
        m.indices.clear();
        let mut shadow_mesh = m.clone();
        for pass in 0..=shadow.len() {
            for (g, dy) in &glyphs {
                let mesh = self.glyph(&g.font, g.glyph);
                let scale = g.size * 20.0 / g.font.em;
                let (ox, oy, c) = if pass < shadow.len() {
                    (shadow[pass].0, shadow[pass].1, shadow_color)
                } else {
                    (0.0, 0.0, g.color)
                };
                let target = if pass < shadow.len() {
                    &mut shadow_mesh
                } else {
                    &mut m
                };
                let base = target.vertices.len() as u32;
                for p in &mesh.0 {
                    target.vertices.push(Vertex {
                        pos: [
                            (bx + g.x + ox) * 20.0 + p[0] * scale,
                            (by + g.y + dy + oy) * 20.0 + p[1] * scale,
                        ],
                        uv: [0.0; 2],
                        color: [c[0], c[1], c[2], 255],
                    });
                }
                target.indices.extend(mesh.1.iter().map(|i| i + base));
            }
        }
        out.extend(self.upload_colored(&shadow_mesh, None, 0.0, Matrix::IDENTITY, true));
        out.extend(self.upload(&m, None, 0.0, Matrix::IDENTITY));
        self.texts.insert(d, (rev, out.clone()));
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
        if !o.visible {
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
            let targets = self.create_targets((view.w as u32, view.h as u32));
            self.draw_build(&source, &targets);
            let output = self.filters.apply(
                &self.device,
                &self.queue,
                targets.output,
                &filters,
                view.k * 20.0,
            );
            let binding = Arc::new(texture_binding(
                &self.device,
                &self.texture_layout,
                &output,
                &self.clamp,
            ));
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
                })
                .to_vec(),
                indices: vec![0, 1, 2, 0, 2, 3],
            };
            if let Some(mesh) = self.upload(&mesh, Some(binding), 1.0, Matrix::IDENTITY) {
                let u = b.uniforms.len() as u32;
                b.uniforms.push(DrawU {
                    m0: [1.0, 0.0, 0.0, 0.0],
                    m1: [0.0, 1.0, 0.0, 0.0],
                    mul: [1.0; 4],
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
            Kind::Text(_) => self.text_meshes(player, d),
            Kind::Bitmap { url, width, height } => {
                let (url, w, h) = (url.clone(), *width, *height);
                match self.bitmap(images, &url, false) {
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
                            });
                        }
                        self.upload(&m, Some(bg), 1.0, Matrix::IDENTITY)
                            .into_iter()
                            .collect()
                    }
                    None => Vec::new(),
                }
            }
            Kind::Clip(c) => match &c.drawing {
                Some(dr) => {
                    if let Some(m) = self.drawings.get(&d) {
                        m.clone()
                    } else {
                        let mut out = Vec::new();
                        for (color, pts) in dr.fills.iter().chain(dr.current.iter()) {
                            if let Some(m) = tess::tessellate_polygon(pts, *color) {
                                out.extend(self.upload(&m, None, 0.0, Matrix::IDENTITY));
                            }
                        }
                        // Drawings change rarely; drop the cache when the clip is dirty.
                        out
                    }
                }
                None => Vec::new(),
            },
        };
        let u = b.uniform(&world, &cx, view);
        for m in &meshes {
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
        // Open masks: (until depth, mask object, its matrix, stencil level inside).
        let mut masks: Vec<(i32, DisplayId)> = Vec::new();
        let mut level = stencil;
        let grid = self.grid_for(player, d, parent);
        for (depth, c, clip) in kids {
            while let Some((until, m)) = masks.last().copied() {
                if depth > until {
                    self.mask_ops(player, images, m, world, view, level, Stencil::Pop, b);
                    level -= 1;
                    masks.pop();
                } else {
                    break;
                }
            }
            if let Some(until) = clip {
                self.mask_ops(player, images, c, world, view, level, Stencil::Push, b);
                level += 1;
                masks.push((until, c));
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
        while let Some((_, m)) = masks.pop() {
            self.mask_ops(player, images, m, world, view, level, Stencil::Pop, b);
            level -= 1;
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
    ) {
        let mut tmp = Build {
            ops: Vec::new(),
            uniforms: std::mem::take(&mut b.uniforms),
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
        let reference = level;
        for mut op in tmp.ops {
            op.stencil = mode;
            op.reference = reference;
            op.blend = Blend::Normal;
            b.ops.push(op);
        }
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
        let (w, h) = t.size;
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
                texture: &t.output,
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

/// Scaleform's text shadow style (`shadowStyle`): offsets in pixels, e.g. `s{1,1}`.
/// Provisional: the syntax is inferred from the movies' values.
fn shadow_offsets(style: &str) -> Vec<(f32, f32)> {
    let mut out = Vec::new();
    let mut rest = style;
    while let Some(i) = rest.find('{') {
        let Some(j) = rest[i..].find('}') else { break };
        let inner = &rest[i + 1..i + j];
        let mut it = inner.split(',').map(|v| v.trim().parse::<f32>().ok());
        if let (Some(Some(x)), Some(Some(y))) = (it.next(), it.next()) {
            out.push((x, y));
        }
        rest = &rest[i + j + 1..];
    }
    out
}

struct Viewport {
    k: f32,
    ox: f32,
    oy: f32,
    w: f32,
    h: f32,
}

struct Build {
    ops: Vec<Op>,
    uniforms: Vec<DrawU>,
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
    fn destination_blends_handle_transparent_pictures_and_flatten_the_source() {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
        let (device, queue) =
            pollster::block_on(adapter.request_device(&Default::default())).unwrap();
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

    #[test]
    fn destination_snapshots_preserve_open_masks_and_later_draws() {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
        let (device, queue) =
            pollster::block_on(adapter.request_device(&Default::default())).unwrap();
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
    fn alpha_and_erase_affect_only_the_enclosing_layer() {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
        let (device, queue) =
            pollster::block_on(adapter.request_device(&Default::default())).unwrap();
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
    fn tinted_text_keeps_black_shadows_and_inherits_alpha() {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
        let (device, queue) =
            pollster::block_on(adapter.request_device(&Default::default())).unwrap();
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
