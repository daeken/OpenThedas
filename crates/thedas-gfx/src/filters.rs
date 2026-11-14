//! Bitmap filters operate on the flattened, premultiplied subtree, before it is blended
//! into its parent. Blur sizes are kernel widths, and quality repeats the two box passes.

use wgpu::util::DeviceExt;

use crate::swf::{Color, Filter};

const SHADER: &str = include_str!("filters.wgsl");

#[repr(C)]
#[derive(Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct Args {
    color: [f32; 4],
    // Offset in pixels; strength; inner.
    effect: [f32; 4],
    // Knockout; composite source; blur width; blur axis (0 = composite, 1 = x, 2 = y).
    mode: [f32; 4],
}

pub(crate) struct Filters {
    layout: wgpu::BindGroupLayout,
    pipeline: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
}

pub(crate) fn texture(
    device: &wgpu::Device,
    size: (u32, u32),
    format: wgpu::TextureFormat,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("gfx filter"),
        size: wgpu::Extent3d {
            width: size.0.max(1),
            height: size.1.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    })
}

impl Filters {
    pub(crate) fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let image = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("gfx filter"),
            entries: &[
                image(0),
                image(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
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
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("gfx filter"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("gfx filter"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("gfx filter"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                targets: &[Some(format.into())],
                compilation_options: Default::default(),
            }),
            multiview_mask: None,
            cache: None,
        });
        Self {
            layout,
            pipeline,
            sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }),
        }
    }

    pub(crate) fn supported(filter: &Filter) -> bool {
        !matches!(filter, Filter::Other(_))
    }

    pub(crate) fn apply(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        mut source: wgpu::Texture,
        filters: &[Filter],
        scale: f32,
    ) -> wgpu::Texture {
        let mut encoder = device.create_command_encoder(&Default::default());
        for filter in filters {
            let (blur, passes, args) = match *filter {
                Filter::Blur { blur, passes } => (blur, passes, None),
                Filter::Glow {
                    color,
                    blur,
                    strength,
                    inner,
                    knockout,
                    composite_source,
                    passes,
                } => (
                    blur,
                    passes,
                    Some(effect(
                        color,
                        strength,
                        inner,
                        knockout,
                        composite_source,
                        (0.0, 0.0),
                    )),
                ),
                Filter::DropShadow {
                    color,
                    blur,
                    angle,
                    distance,
                    strength,
                    inner,
                    knockout,
                    hide_object,
                    passes,
                } => (
                    blur,
                    passes,
                    Some(effect(
                        color,
                        strength,
                        inner,
                        knockout,
                        !hide_object,
                        (
                            angle.cos() * distance * scale,
                            angle.sin() * distance * scale,
                        ),
                    )),
                ),
                Filter::Other(_) => continue,
            };
            let mut blurred = source.clone();
            for _ in 0..passes {
                for (width, axis) in [(blur.0, 1.0), (blur.1, 2.0)] {
                    let width = ((width - 1.0) * scale + 1.0).clamp(1.0, 255.0);
                    if width <= 1.0 {
                        continue;
                    }
                    blurred = self.pass(
                        device,
                        &mut encoder,
                        &blurred,
                        &blurred,
                        Args {
                            mode: [0.0, 0.0, width, axis],
                            ..Default::default()
                        },
                    );
                }
            }
            source = match args {
                Some(args) => self.pass(device, &mut encoder, &source, &blurred, args),
                None => blurred,
            };
        }
        queue.submit(Some(encoder.finish()));
        source
    }

    fn pass(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        source: &wgpu::Texture,
        blurred: &wgpu::Texture,
        args: Args,
    ) -> wgpu::Texture {
        let output = texture(device, (source.width(), source.height()), source.format());
        let output_view = output.create_view(&Default::default());
        let source_view = source.create_view(&Default::default());
        let blur_view = blurred.create_view(&Default::default());
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("gfx filter args"),
            contents: bytemuck::bytes_of(&args),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let binding = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("gfx filter"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&source_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&blur_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: uniform.as_entire_binding(),
                },
            ],
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("gfx filter"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &output_view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &binding, &[]);
            pass.draw(0..3, 0..1);
        }
        output
    }
}

fn effect(
    color: Color,
    strength: f32,
    inner: bool,
    knockout: bool,
    composite: bool,
    offset: (f32, f32),
) -> Args {
    Args {
        color: [color.r, color.g, color.b, color.a].map(|x| x as f32 / 255.0),
        effect: [offset.0, offset.1, strength, u8::from(inner) as f32],
        mode: [
            u8::from(knockout) as f32,
            u8::from(composite) as f32,
            0.0,
            0.0,
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Read back real GPU output: these cases distinguish source-over from additive
    /// copies, exercise premultiplied colour, and check both sides of the silhouette.
    #[test]
    fn filter_pixels() {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
        let (device, queue) =
            pollster::block_on(adapter.request_device(&Default::default())).unwrap();
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let filters = Filters::new(&device, format);
        let mut pixels = vec![0; 32 * 32 * 4];
        for y in 8..24 {
            for x in 8..24 {
                pixels[(y * 32 + x) * 4..(y * 32 + x + 1) * 4].copy_from_slice(&[128, 0, 0, 128]);
            }
        }
        let source = device.create_texture_with_data(
            &queue,
            &wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d {
                    width: 32,
                    height: 32,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &pixels,
        );
        let run = |filter: Filter| {
            let out = filters.apply(&device, &queue, source.clone(), &[filter], 1.0);
            let read = device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: 256 * 32,
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let mut encoder = device.create_command_encoder(&Default::default());
            encoder.copy_texture_to_buffer(
                out.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &read,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(256),
                        rows_per_image: None,
                    },
                },
                out.size(),
            );
            queue.submit(Some(encoder.finish()));
            let (tx, rx) = std::sync::mpsc::channel();
            read.slice(..)
                .map_async(wgpu::MapMode::Read, move |result| tx.send(result).unwrap());
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            rx.recv().unwrap().unwrap();
            let bytes = read.slice(..).get_mapped_range().unwrap().to_vec();
            move |x: usize, y: usize| -> [u8; 4] {
                bytes[y * 256 + x * 4..y * 256 + x * 4 + 4]
                    .try_into()
                    .unwrap()
            }
        };
        let glow = |inner, knockout, composite_source, passes, strength| Filter::Glow {
            color: Color {
                r: 0,
                g: 255,
                b: 0,
                a: 128,
            },
            blur: (5.0, 1.0),
            strength,
            inner,
            knockout,
            composite_source,
            passes,
        };
        let plain = run(glow(false, false, true, 1, 1.0));
        assert_eq!(plain(16, 16), [128, 32, 0, 160]);
        assert_eq!(plain(7, 16), [0, 26, 0, 26]);
        assert_eq!(plain(16, 7), [0; 4]); // independent X/Y widths
        let strong = run(glow(false, false, false, 1, 8.0));
        assert_eq!(strong(16, 16), [0, 128, 0, 128]); // clamp coverage before colour alpha
        let knockout = run(glow(false, true, true, 0, 1.0));
        assert_eq!(knockout(16, 16), [0, 32, 0, 32]);
        let inner = run(glow(true, true, true, 1, 1.0));
        assert_eq!(inner(7, 16), [0; 4]);
        assert_eq!(inner(16, 16), [0, 32, 0, 32]);
        let quality = run(glow(false, true, false, 2, 1.0));
        assert!(quality(4, 16)[3] > 0); // second pass extends support
        assert_eq!(plain(4, 16), [0; 4]);
        let shadow = run(Filter::DropShadow {
            color: Color {
                r: 0,
                g: 0,
                b: 255,
                a: 255,
            },
            blur: (0.0, 0.0),
            angle: std::f32::consts::FRAC_PI_2,
            distance: 4.0,
            strength: 1.0,
            inner: false,
            knockout: false,
            hide_object: true,
            passes: 1,
        });
        assert_eq!(shadow(16, 9), [0; 4]);
        assert_eq!(shadow(16, 26), [0, 0, 128, 128]);
    }
}
