//! Real sampled opacity/discard coverage using the G-buffer's alpha rule.
use super::*;
use bytemuck::Zeroable;
use thedas_formats::state::CompareFunc;
use wgpu::util::DeviceExt;

#[test]
fn punch_opacity_keeps_holes_comparisons_rounding_and_shader_clip() {
    let ctx = match GpuContext::headless() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("skipping GPU test: {e}");
            return;
        }
    };
    let device = &ctx.device;
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("deferred punch fixture"),
        source: wgpu::ShaderSource::Wgsl(
            format!(
                r#"{DEFERRED_WGSL}
            @fragment fn fs_fixture(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {{
                let opacity = textureSample(diffuse_tex, wrap_smp, vec2<f32>(p.x / 16.0, 0.5)).a;
                if (!alpha_kept(opacity)) {{ discard; }}
                return vec4<f32>(1.0);
            }}"#
            )
            .into(),
        ),
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: None,
        layout: None,
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_fullscreen"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_fixture"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::Rgba8Unorm,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        multiview_mask: None,
        cache: None,
    });
    let mut draw = DrawUniforms::zeroed();
    let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: bytemuck::bytes_of(&draw),
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    });
    let uniforms = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[wgpu::BindGroupEntry {
            binding: 1,
            resource: buffer.as_entire_binding(),
        }],
    });
    let source = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: 16,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let alpha: [u8; 16] = [
        0, 1, 64, 76, 77, 126, 127, 128, 129, 160, 192, 200, 240, 253, 254, 255,
    ];
    let bytes: Vec<u8> = alpha.iter().flat_map(|a| [255, 255, 255, *a]).collect();
    ctx.queue.write_texture(
        source.as_image_copy(),
        &bytes,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(64),
            rows_per_image: Some(1),
        },
        source.size(),
    );
    let view = source.create_view(&Default::default());
    let sampler = device.create_sampler(&Default::default());
    let material = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(1),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::Sampler(&sampler),
            },
        ],
    });
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: source.size(),
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let target_view = target.create_view(&Default::default());
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 256,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    for cmp in [
        CompareFunc::Never,
        CompareFunc::Less,
        CompareFunc::Equal,
        CompareFunc::LessEqual,
        CompareFunc::Greater,
        CompareFunc::NotEqual,
        CompareFunc::GreaterEqual,
        CompareFunc::Always,
    ] {
        for (reference, multiplier, clip) in [
            (127, 1.0, 0.3),
            (255, 0.999, 0.3),
            (0, 0.25, 0.3),
            (128, 2.0, 0.3),
            (128, 1.0, -1.0),
            (0, 0.25, -1.0),
            (255, 0.999, -1.0),
        ] {
            draw.data.alpha = [
                crate::deferred_alpha::comparison(cmp),
                reference as f32,
                multiplier,
                clip,
            ];
            ctx.queue
                .write_buffer(&buffer, 0, bytemuck::bytes_of(&draw));
            let mut encoder = device.create_command_encoder(&Default::default());
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: None,
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &target_view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                pass.set_pipeline(&pipeline);
                pass.set_bind_group(0, &uniforms, &[]);
                pass.set_bind_group(1, &material, &[]);
                pass.draw(0..3, 0..1);
            }
            encoder.copy_texture_to_buffer(
                target.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &readback,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(256),
                        rows_per_image: Some(1),
                    },
                },
                target.size(),
            );
            ctx.queue.submit([encoder.finish()]);
            let (tx, rx) = std::sync::mpsc::channel();
            readback
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            rx.recv().unwrap().unwrap();
            let mapped = readback.slice(..).get_mapped_range().unwrap();
            for (i, a) in alpha.iter().enumerate() {
                let opacity = *a as f32 / 255.0 * multiplier;
                let rounded = (opacity.clamp(0.0, 1.0) * 255.0 + 0.5).floor() as u32;
                let expected = opacity >= clip
                    && match cmp {
                        CompareFunc::Never => false,
                        CompareFunc::Less => rounded < reference,
                        CompareFunc::Equal => rounded == reference,
                        CompareFunc::LessEqual => rounded <= reference,
                        CompareFunc::Greater => rounded > reference,
                        CompareFunc::NotEqual => rounded != reference,
                        CompareFunc::GreaterEqual => rounded >= reference,
                        CompareFunc::Always => true,
                    };
                assert_eq!(
                    mapped[i * 4] == 255,
                    expected,
                    "{cmp:?}, alpha {a}, multiplier {multiplier}"
                );
            }
            drop(mapped);
            readback.unmap();
        }
    }
}
