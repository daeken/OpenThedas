//! Execute the fused base shader with sampled cubemaps/emission and HDR bounce.

use super::*;
use bytemuck::Zeroable;

#[test]
fn fused_base_keeps_hdr_visibility_and_material_reflection_rules() {
    let ctx = match GpuContext::headless() {
        Ok(ctx) => ctx,
        Err(e) => {
            eprintln!("skipping GPU test: {e}");
            return;
        }
    };
    let device = &ctx.device;
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("fused base fixture"),
        source: wgpu::ShaderSource::Wgsl(
            format!(
                r#"{DEFERRED_WGSL}
            @fragment fn fs_fixture() -> @location(0) vec4<f32> {{
                var input: VsOut;
                input.world = vec3<f32>(0.0);
                input.uv0 = vec2<f32>(0.5);
                var g: GBuffer;
                g.albedo = select(vec4<f32>(0.2, 0.4, 0.6, 0.0),
                    vec4<f32>(2.0, -0.5, 0.6, 0.0), frame.fixup.w > 0.5);
                g.specular = vec4<f32>(0.5, 0.25, 0.125, 1.0);
                g.lightmap = vec4<f32>(2.0, 3.0, 4.0, 0.25);
                let s = Surface(vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(0.0, 0.0, 1.0), 0.5);
                return baked_base(input, g, s).lightmap;
            }}
        "#
            )
            .into(),
        ),
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("fused base fixture"),
        layout: None,
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs_fullscreen"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs_fixture"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: wgpu::TextureFormat::Rgba32Float,
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
    let buffer = |size| {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    };
    let frame_buffer = buffer(std::mem::size_of::<FrameUniforms>() as u64);
    let draw_buffer = buffer(std::mem::size_of::<DrawUniforms>() as u64);
    let sun_buffer = buffer(std::mem::size_of::<SunUniforms>() as u64);
    ctx.queue
        .write_buffer(&sun_buffer, 0, bytemuck::bytes_of(&SunUniforms::zeroed()));
    let uniforms = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: frame_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: draw_buffer.as_entire_binding(),
            },
        ],
    });
    let texture = |layers, pixel: [u8; 4]| {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: layers,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        ctx.queue.write_texture(
            texture.as_image_copy(),
            &pixel.repeat(layers as usize),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4),
                rows_per_image: Some(1),
            },
            texture.size(),
        );
        texture
    };
    let cube = texture(6, [64, 128, 255, 255]).create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::Cube),
        ..Default::default()
    });
    let emission = texture(1, [128, 64, 32, 255]).create_view(&Default::default());
    let sampler = device.create_sampler(&Default::default());
    let material = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(1),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::Sampler(&sampler),
            },
            wgpu::BindGroupEntry {
                binding: 6,
                resource: wgpu::BindingResource::TextureView(&cube),
            },
            wgpu::BindGroupEntry {
                binding: 14,
                resource: wgpu::BindingResource::TextureView(&emission),
            },
        ],
    });
    let sun = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(2),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: sun_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&emission),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(&emission),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::Sampler(&sampler),
            },
        ],
    });
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba32Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 256,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    // Level multiplier, terrain's visibility plus additive tint, emissive objects,
    // faces/eyes with no cubemap, a zero reflection multiplier, and unfused mode.
    let cases = [
        (
            true,
            [0.0, 0.0, 0.0, 0.0],
            [0.0, 2.0, 0.0, 0.0],
            [0.5254902, 1.3254902, 2.525, 0.25],
        ),
        (
            true,
            [0.0; 4],
            [0.0, 2.0, 1.0, 0.0],
            [0.447451, 1.387451, 2.6073437, 0.25],
        ),
        (
            true,
            [2.0, 2.0, 0.0, 0.0],
            [0.0, 2.0, 0.0, 0.0],
            [1.5294118, 1.827451, 2.7759805, 0.25],
        ),
        (
            true,
            [2.0, 2.0, 0.0, 1.0],
            [0.0, 2.0, 0.0, 0.0],
            [0.4, 1.2, 2.4, 0.25],
        ),
        (true, [0.0; 4], [0.0; 4], [0.4, 1.2, 2.4, 0.25]),
        (false, [0.0; 4], [0.0, 2.0, 0.0, 0.0], [2.0, 3.0, 4.0, 0.25]),
        (true, [2.0, 0.0, 0.0, 1.0], [0.0; 4], [2.0, 0.0, 2.4, 0.25]),
    ];
    for (i, (fused, material_data, room, expected)) in cases.into_iter().enumerate() {
        let mut frame = FrameUniforms::zeroed();
        frame.fixup[2] = if fused { 1.0 } else { 0.0 };
        frame.fixup[3] = if i == 6 { 1.0 } else { 0.0 };
        ctx.queue
            .write_buffer(&frame_buffer, 0, bytemuck::bytes_of(&frame));
        let mut draw = DrawUniforms::zeroed();
        draw.data.material = material_data;
        draw.data.room = room;
        draw.data.terrain.tint = [0.1, 0.4, 0.3, 0.0];
        ctx.queue
            .write_buffer(&draw_buffer, 0, bytemuck::bytes_of(&draw));
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
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
            pass.set_bind_group(2, &sun, &[]);
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
            .map_async(wgpu::MapMode::Read, move |result| {
                tx.send(result).unwrap();
            });
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        rx.recv().unwrap().unwrap();
        {
            let mapped = readback.slice(..).get_mapped_range().unwrap();
            let actual: &[f32] = bytemuck::cast_slice(&mapped[..16]);
            for (actual, expected) in actual.iter().zip(expected) {
                assert!(
                    (actual - expected).abs() < 0.0001,
                    "material={material_data:?} room={room:?}: {actual} != {expected}"
                );
            }
        }
        readback.unmap();
    }
}
