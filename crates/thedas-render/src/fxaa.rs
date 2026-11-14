//! Post-process anti-aliasing over the finished frame: edges found by the luma of each pixel
//! and its neighbours are smoothed by sampling along them.
//!
//! The original anti-aliases with the device's multisampling (its `AntialiasingLevel`
//! option sets the D3D9 sample count). Multisampling every pass here is a larger change
//! (every pipeline, the shared depth buffers the frame effects read), so this pass stands
//! in: a fast approximate filter of the usual kind (the luma contrast around a pixel picks
//! where to smooth; the blur runs along the edge's direction, spread by how sharp it is),
//! written for this renderer. Provisional and not the original's: it softens differently
//! from multisampling (it also smooths high-contrast texture detail a little and leaves
//! sub-pixel geometry as it was drawn).

const SHADER: &str = r#"
@group(0) @binding(0) var frame: texture_2d<f32>;
@group(0) @binding(1) var smp: sampler;

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let t = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(t * 2.0 - 1.0, 0.0, 1.0);
}

fn luma(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.299, 0.587, 0.114));
}

fn at(uv: vec2<f32>) -> vec3<f32> {
    return textureSampleLevel(frame, smp, uv, 0.0).rgb;
}

// Below this contrast (absolute, and relative to the brightest neighbour) nothing is
// smoothed; the spread of the blur along an edge is at most SPAN pixels.
const EDGE_MIN: f32 = 1.0 / 32.0;
const EDGE_RELATIVE: f32 = 1.0 / 8.0;
const REDUCE_MIN: f32 = 1.0 / 128.0;
const REDUCE_MUL: f32 = 1.0 / 8.0;
const SPAN: f32 = 8.0;

@fragment
fn fs(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let size = vec2<f32>(textureDimensions(frame));
    let px = 1.0 / size;
    let uv = pos.xy * px;
    let centre = textureSampleLevel(frame, smp, uv, 0.0);
    let m = luma(centre.rgb);
    let nw = luma(at(uv + vec2<f32>(-1.0, -1.0) * px));
    let ne = luma(at(uv + vec2<f32>(1.0, -1.0) * px));
    let sw = luma(at(uv + vec2<f32>(-1.0, 1.0) * px));
    let se = luma(at(uv + vec2<f32>(1.0, 1.0) * px));
    let lo = min(m, min(min(nw, ne), min(sw, se)));
    let hi = max(m, max(max(nw, ne), max(sw, se)));
    if (hi - lo < max(EDGE_MIN, hi * EDGE_RELATIVE)) {
        return centre;
    }
    // The edge runs across the steepest change of luma.
    var dir = vec2<f32>(-((nw + ne) - (sw + se)), (nw + sw) - (ne + se));
    let reduce = max((nw + ne + sw + se) * 0.25 * REDUCE_MUL, REDUCE_MIN);
    let scale = 1.0 / (min(abs(dir.x), abs(dir.y)) + reduce);
    dir = clamp(dir * scale, vec2<f32>(-SPAN), vec2<f32>(SPAN)) * px;
    let near = 0.5 * (at(uv + dir * (1.0 / 3.0 - 0.5)) + at(uv + dir * (2.0 / 3.0 - 0.5)));
    let far = near * 0.5 + 0.25 * (at(uv - dir * 0.5) + at(uv + dir * 0.5));
    let lf = luma(far);
    // The wider blur, unless it reached past the neighbourhood's range (across another edge).
    if (lf < lo || lf > hi) {
        return vec4<f32>(near, centre.a);
    }
    return vec4<f32>(far, centre.a);
}
"#;

/// The pass and its copy of the frame, for frames of one size and format.
pub struct Fxaa {
    size: (u32, u32),
    format: wgpu::TextureFormat,
    copy: wgpu::Texture,
    bindings: wgpu::BindGroup,
    pipeline: wgpu::RenderPipeline,
}

impl Fxaa {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat, size: (u32, u32)) -> Self {
        let copy = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("anti-aliasing frame copy"),
            size: wgpu::Extent3d {
                width: size.0.max(1),
                height: size.1.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = copy.create_view(&Default::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("anti-aliasing"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("anti-aliasing"),
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
        let bindings = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("anti-aliasing"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("anti-aliasing"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("anti-aliasing"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("anti-aliasing"),
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
                    format,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        Fxaa {
            size,
            format,
            copy,
            bindings,
            pipeline,
        }
    }

    /// Whether it was made for frames of this size and format.
    pub fn fits(&self, format: wgpu::TextureFormat, size: (u32, u32)) -> bool {
        self.format == format && self.size == size
    }

    /// Smooths the edges of `target`, a finished frame (it needs `COPY_SRC` usage).
    pub fn apply(&self, encoder: &mut wgpu::CommandEncoder, target: &wgpu::Texture) {
        let extent = wgpu::Extent3d {
            width: self.size.0.max(1),
            height: self.size.1.max(1),
            depth_or_array_layers: 1,
        };
        encoder.copy_texture_to_texture(target.as_image_copy(), self.copy.as_image_copy(), extent);
        let view = target.create_view(&Default::default());
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("anti-aliasing"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
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
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bindings, &[]);
        pass.draw(0..3, 0..1);
    }
}
