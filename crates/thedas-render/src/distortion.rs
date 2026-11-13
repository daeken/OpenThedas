//! The distortion frame buffer effect (`da_distortion.ffx.xml`): heat haze over fires, the
//! shimmer of spells.
//!
//! As the effect file lays it out: the geometry of the `Distortion` render class (materials
//! of `DADistortionMask.mat`: `DistortionVFX.vsh`/`DistortionGeom.psh` for particles) is
//! drawn into a mask the size of the frame, cleared to (0.5, 0.5, 0.5, 0.705) and sharing the
//! frame's depth buffer (`DistortionMask.sta`: depth tested, not written; alpha tested; each
//! layer blended in by the alpha already there). Red and green hold an offset in u and v
//! around a half. Then `DADistortion.mat`'s `ApplyDistortion` (`PassThrough.vsh`,
//! `Distortion.psh`, `PPQuadNoBlendMirror.sta`) redraws the frame from a copy of it, each
//! pixel taken from where the mask sends it.
//!
//! `Distortion.psh` (ps_2_0), transcribed in [`SHADER`]: with `m` the mask at the pixel,
//! the offset is `((m.r·2 − 1)·0.1·height/width, (m.g·2 − 1)·0.1)`; the pixel takes the
//! frame at `uv + offset` when the mask *there* is off its neutral value (127/255) in red or
//! green, so nothing undistorted is pulled in, and keeps its own otherwise.
//!
//! Provisional: the original copies the frame and applies the effect when its static
//! geometry is done (`StaticGeomEnd`, `DistortionMaskEnd`), before what is drawn after it;
//! here it runs on the finished frame, so the particles drawn over a haze are distorted with
//! it. The clear's bytes are taken as the floats truncated (127, 127, 127, 179), which
//! matches the shader's neutral constant.

/// The mask's format (`A8R8G8B8`).
pub const MASK_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// The mask's clear colour: `autoclear="0.5 0.5 0.5 0.705"` as bytes.
const CLEAR: wgpu::Color = wgpu::Color {
    r: 127.0 / 255.0,
    g: 127.0 / 255.0,
    b: 127.0 / 255.0,
    a: 179.0 / 255.0,
};

const SHADER: &str = r#"
@group(0) @binding(0) var frame: texture_2d<f32>;
@group(0) @binding(1) var mask: texture_2d<f32>;
// Stage 0: point filtered, mirrored. Stage 1: point filtered, clamped.
@group(0) @binding(2) var frame_smp: sampler;
@group(0) @binding(3) var mask_smp: sampler;

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let t = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(t * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let size = vec2<f32>(textureDimensions(frame));
    let uv = pos.xy / size;
    let m = textureSampleLevel(mask, mask_smp, uv, 0.0);
    // c0 = fWindowWidth, c1 = fWindowHeight.
    let offset = vec2<f32>((m.x * 2.0 - 1.0) * 0.1 * (size.y / size.x), (m.y * 2.0 - 1.0) * 0.1);
    let to = uv + offset;
    let there = textureSampleLevel(mask, mask_smp, to, 0.0);
    let moved = textureSampleLevel(frame, frame_smp, to, 0.0);
    let own = textureSampleLevel(frame, frame_smp, uv, 0.0);
    let neutral_x = abs(there.x - 0.498039) - 0.001 < 0.0;
    let neutral_y = abs(there.y - 0.498039) - 0.001 < 0.0;
    if (neutral_x && neutral_y) {
        return own;
    }
    return moved;
}
"#;

/// The effect's targets and its second pass, for frames of one size and format.
pub struct Distortion {
    size: (u32, u32),
    format: wgpu::TextureFormat,
    mask: wgpu::TextureView,
    copy: wgpu::Texture,
    bindings: wgpu::BindGroup,
    apply: wgpu::RenderPipeline,
}

impl Distortion {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat, size: (u32, u32)) -> Self {
        let extent = wgpu::Extent3d {
            width: size.0.max(1),
            height: size.1.max(1),
            depth_or_array_layers: 1,
        };
        let target = |label, format, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: extent,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let sampled = wgpu::TextureUsages::TEXTURE_BINDING;
        let mask = target(
            "distortion mask",
            MASK_FORMAT,
            sampled | wgpu::TextureUsages::RENDER_ATTACHMENT,
        );
        let copy = target(
            "distortion frame copy",
            format,
            sampled | wgpu::TextureUsages::COPY_DST,
        );
        let mask = mask.create_view(&Default::default());
        let copy_view = copy.create_view(&Default::default());
        let sampler = |address| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                address_mode_u: address,
                address_mode_v: address,
                ..Default::default()
            })
        };
        let frame_smp = sampler(wgpu::AddressMode::MirrorRepeat);
        let mask_smp = sampler(wgpu::AddressMode::ClampToEdge);
        let texture = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let smp = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("distortion"),
            entries: &[texture(0), texture(1), smp(2), smp(3)],
        });
        let bindings = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("distortion"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&copy_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&mask),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&frame_smp),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&mask_smp),
                },
            ],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("distortion"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("distortion"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let apply = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("distortion"),
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
        Distortion {
            size,
            format,
            mask,
            copy,
            bindings,
            apply,
        }
    }

    /// Whether it was made for frames of this size and format.
    pub fn fits(&self, format: wgpu::TextureFormat, size: (u32, u32)) -> bool {
        self.format == format && self.size == size
    }

    /// Records the effect over `target`, a finished frame whose depth buffer is `depth`:
    /// `draw` draws the distortion geometry into the mask's pass.
    pub fn apply(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::Texture,
        depth: &wgpu::TextureView,
        draw: impl FnOnce(&mut wgpu::RenderPass<'_>),
    ) {
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("distortion mask"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.mask,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(CLEAR),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                // The frame's depth, tested and left as it is.
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: depth,
                    depth_ops: None,
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            draw(&mut pass);
        }
        let extent = wgpu::Extent3d {
            width: self.size.0.max(1),
            height: self.size.1.max(1),
            depth_or_array_layers: 1,
        };
        encoder.copy_texture_to_texture(target.as_image_copy(), self.copy.as_image_copy(), extent);
        let view = target.create_view(&Default::default());
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("distortion"),
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
        pass.set_pipeline(&self.apply);
        pass.set_bind_group(0, &self.bindings, &[]);
        pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod gl_tests {
    use super::*;

    #[test]
    fn its_shaders_translate_for_opengl() {
        crate::gl_check::translates("distortion", SHADER);
    }
}
