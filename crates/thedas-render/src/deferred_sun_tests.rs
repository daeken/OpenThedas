//! Sample the actual sun filter across near/far, border, depth and grazing cases.
use super::*;
use bytemuck::Zeroable;
use wgpu::util::DeviceExt;

#[test]
fn sun_early_exits_preserve_filtered_visibility() {
    let ctx = match GpuContext::headless() {
        Ok(ctx) => ctx,
        Err(e) => {
            eprintln!("skipping GPU test: {e}");
            return;
        }
    };
    let device = &ctx.device;
    let sun_source = format!(
        "struct Sun {{{}",
        DEFERRED_WGSL
            .split_once("struct Sun {")
            .unwrap()
            .1
            .split_once("// How much of a point light")
            .unwrap()
            .0
    )
    .replace("@group(2)", "@group(0)");
    let reference = sun_source
        .split_once("fn sun_visibility(")
        .unwrap()
        .1
        .replace("sun.enabled.y != 0u", "false");
    let source = format!(
        r#"{sun_source}
        fn reference({reference}
        @vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {{
            return vec4<f32>(f32((i << 1u) & 2u) * 2.0 - 1.0, f32(i & 2u) * 2.0 - 1.0, 0.0, 1.0);
        }}
        @fragment fn fs(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {{
            let uv = (p.xy - vec2<f32>(4.5)) / vec2<f32>(55.0, 7.0);
            let depth = select(128.0 / 255.0, 0.9, (u32(p.x) % 3u) == 0u);
            let world = vec3<f32>(uv, depth);
            let normals = array<f32, 8>(-1.0, -0.000001, 0.0, 0.000001, 0.05, 0.1, 0.5, 1.0);
            let n = vec3<f32>(0.0, 0.0, normals[u32(p.x) % 8u]);
            let grazing = (u32(p.y) % 2u) == 0u;
            let a = sun_visibility(world, n, grazing);
            let b = reference(world, n, grazing);
            return vec4<f32>(a, b, abs(a - b), 1.0);
        }}"#
    );
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("sun early-exit fixture"),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("sun early-exit fixture"),
        layout: None,
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs"),
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
    let mut sun = SunUniforms::zeroed();
    sun.data[0] = [1.0, 0.0, 0.0, 0.0];
    sun.data[1] = [0.0, 1.0, 0.0, 0.0];
    sun.data[2] = [0.0, 0.0, 1.0, 0.0];
    sun.data[3] = [0.0, 0.0, 0.0, 1.0];
    sun.data[4] = sun.data[0];
    sun.data[5] = sun.data[1];
    sun.data[6] = sun.data[2];
    sun.data[7][3] = -20.0;
    sun.direction = [0.0, 0.0, -1.0, 1.0];
    sun.enabled = [1, 1, 0, 0];
    let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: bytemuck::bytes_of(&sun),
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    });
    let texture = |width, height, format, usage| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        })
    };
    let map = texture(
        4,
        4,
        wgpu::TextureFormat::Rgba8Unorm,
        wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
    );
    let pixels: Vec<u8> = (0..16)
        .flat_map(|i| [[0, 50, 0, 255], [128, 200, 0, 255], [255, 100, 0, 255]][i % 3])
        .collect();
    ctx.queue.write_texture(
        map.as_image_copy(),
        &pixels,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(16),
            rows_per_image: Some(4),
        },
        map.size(),
    );
    let map = map.create_view(&Default::default());
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&map),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(&map),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::Sampler(&sampler),
            },
        ],
    });
    let target = texture(
        64,
        16,
        wgpu::TextureFormat::Rgba32Float,
        wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
    );
    let view = target.create_view(&Default::default());
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 64 * 16 * 16,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut shadowed = false;
    for plane in [1.0, 0.0, -1.0, -19.0, -21.0] {
        sun.data[8][3] = plane;
        ctx.queue.write_buffer(&buffer, 0, bytemuck::bytes_of(&sun));
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.draw(0..3, 0..1);
        }
        encoder.copy_texture_to_buffer(
            target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(64 * 16),
                    rows_per_image: Some(16),
                },
            },
            target.size(),
        );
        ctx.queue.submit([encoder.finish()]);
        let slice = readback.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        rx.recv().unwrap().unwrap();
        {
            let data = slice.get_mapped_range().unwrap();
            for (i, px) in bytemuck::cast_slice::<u8, f32>(&data)
                .chunks_exact(4)
                .enumerate()
            {
                assert_eq!(px[2], 0.0, "plane {plane}, pixel {i}: {px:?}");
                assert!(px[0].is_finite() && (0.0..=1.0).contains(&px[0]));
                shadowed |= px[0] < 0.9;
            }
        }
        readback.unmap();
    }
    assert!(
        shadowed,
        "the fixture must exercise filtered shadow as well as lit pixels"
    );
}
