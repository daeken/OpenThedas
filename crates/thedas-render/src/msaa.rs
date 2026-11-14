//! Multisampled anti-aliasing: the world drawn into colour and depth targets of several
//! samples a pixel, then resolved into the frame (the colour averaged, the depth taken from
//! each pixel's first sample) before the frame buffer effects, which read single-sampled
//! colour and depth.
//!
//! The original anti-aliases this way: its options' anti-aliasing slider lists the sample
//! counts the device supports (`ClientOptions.AntialiasList`, index 0 "Off"), kept as
//! `AntialiasingLevel`. Provisional: resolving depth by its first sample (the effects that
//! read it, the depth of field and the distortion mask, are not traced against a
//! multisampled original).

use crate::gpu::DEPTH_FORMAT;

/// The sample counts a pixel the adapter can draw `format` colour and our depth with: 1
/// (off) and every larger count both support, in order.
pub fn supported_samples(adapter: &wgpu::Adapter, format: wgpu::TextureFormat) -> Vec<u32> {
    let colour = adapter.get_texture_format_features(format).flags;
    let depth = adapter.get_texture_format_features(DEPTH_FORMAT).flags;
    let mut out = vec![1];
    for n in [2, 4, 8, 16] {
        if colour.sample_count_supported(n) && depth.sample_count_supported(n) {
            out.push(n);
        }
    }
    out
}

const RESOLVE_DEPTH: &str = r#"
@group(0) @binding(0) var depth: texture_depth_multisampled_2d;

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let t = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(t * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs(@builtin(position) pos: vec4<f32>) -> @builtin(frag_depth) f32 {
    return textureLoad(depth, vec2<i32>(pos.xy), 0);
}
"#;

const FILL: &str = r#"
@group(0) @binding(0) var colour: texture_2d<f32>;
@group(0) @binding(1) var depth: texture_depth_2d;

struct Out {
    @location(0) colour: vec4<f32>,
    @builtin(frag_depth) depth: f32,
}

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let t = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(t * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs(@builtin(position) pos: vec4<f32>) -> Out {
    let p = vec2<i32>(pos.xy);
    return Out(textureLoad(colour, p, 0), textureLoad(depth, p, 0));
}
"#;

/// The multisampled targets of one size, format and sample count, with the passes that
/// resolve them and fill them from single-sampled ones.
pub struct MsaaTargets {
    pub size: (u32, u32),
    pub format: wgpu::TextureFormat,
    pub samples: u32,
    pub colour: wgpu::TextureView,
    pub depth: wgpu::TextureView,
    /// A single-sampled colour target the deferred path draws its share into, to be
    /// filled from ([`MsaaTargets::fill`]).
    pub single: wgpu::TextureView,
    resolve_depth: wgpu::RenderPipeline,
    resolve_layout: wgpu::BindGroupLayout,
    fill: wgpu::RenderPipeline,
    fill_layout: wgpu::BindGroupLayout,
}

fn fullscreen(
    device: &wgpu::Device,
    label: &str,
    source: &str,
    layout: &wgpu::BindGroupLayout,
    colour: Option<wgpu::TextureFormat>,
    samples: u32,
) -> wgpu::RenderPipeline {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: &[Some(layout)],
        immediate_size: 0,
    });
    let targets = [colour.map(|format| wgpu::ColorTargetState {
        format,
        blend: None,
        write_mask: wgpu::ColorWrites::ALL,
    })];
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH_FORMAT,
            depth_write_enabled: Some(true),
            depth_compare: Some(wgpu::CompareFunction::Always),
            stencil: wgpu::StencilState::default(),
            bias: wgpu::DepthBiasState::default(),
        }),
        multisample: wgpu::MultisampleState {
            count: samples,
            ..Default::default()
        },
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            targets: if colour.is_some() { &targets } else { &[] },
        }),
        multiview_mask: None,
        cache: None,
    })
}

fn texture_entry(
    binding: u32,
    sample_type: wgpu::TextureSampleType,
    multisampled: bool,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type,
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled,
        },
        count: None,
    }
}

impl MsaaTargets {
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        size: (u32, u32),
        samples: u32,
    ) -> Self {
        let make = |label: &str, format, usage| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
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
                .create_view(&Default::default())
        };
        let colour = make(
            "multisampled colour",
            format,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
        );
        let depth = make(
            "multisampled depth",
            DEPTH_FORMAT,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        );
        let single = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("single-sampled colour"),
                size: wgpu::Extent3d {
                    width: size.0.max(1),
                    height: size.1.max(1),
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
            .create_view(&Default::default());
        let resolve_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("depth resolve"),
            entries: &[texture_entry(0, wgpu::TextureSampleType::Depth, true)],
        });
        let resolve_depth = fullscreen(
            device,
            "depth resolve",
            RESOLVE_DEPTH,
            &resolve_layout,
            None,
            1,
        );
        let fill_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("multisample fill"),
            entries: &[
                texture_entry(
                    0,
                    wgpu::TextureSampleType::Float { filterable: false },
                    false,
                ),
                texture_entry(1, wgpu::TextureSampleType::Depth, false),
            ],
        });
        let fill = fullscreen(
            device,
            "multisample fill",
            FILL,
            &fill_layout,
            Some(format),
            samples,
        );
        MsaaTargets {
            size,
            format,
            samples,
            colour,
            depth,
            single,
            resolve_depth,
            resolve_layout,
            fill,
            fill_layout,
        }
    }

    /// Whether these are the targets for frames of this size, format and sample count.
    pub fn fits(&self, format: wgpu::TextureFormat, size: (u32, u32), samples: u32) -> bool {
        self.format == format && self.size == size && self.samples == samples
    }

    /// Resolves the colour into `target` and the depth into `depth` (single-sampled).
    pub fn resolve(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        depth: &wgpu::TextureView,
    ) {
        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("colour resolve"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &self.colour,
                depth_slice: None,
                resolve_target: Some(target),
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("depth resolve"),
            layout: &self.resolve_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&self.depth),
            }],
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("depth resolve"),
            color_attachments: &[],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.resolve_depth);
        pass.set_bind_group(0, &group, &[]);
        pass.draw(0..3, 0..1);
    }

    /// Fills every sample of the targets from a single-sampled frame (`colour`, `depth`), for
    /// drawing on over what a single-sampled pass drew (the deferred path's share).
    pub fn fill(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        colour: &wgpu::TextureView,
        depth: &wgpu::TextureView,
    ) {
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("multisample fill"),
            layout: &self.fill_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(colour),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(depth),
                },
            ],
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("multisample fill"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &self.colour,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.depth,
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
        pass.set_pipeline(&self.fill);
        pass.set_bind_group(0, &group, &[]);
        pass.draw(0..3, 0..1);
    }
}
