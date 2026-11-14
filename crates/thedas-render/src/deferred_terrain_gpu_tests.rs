//! Exhaust the sampled layer-code range against the original eight comparisons.

use super::*;

#[test]
fn terrain_direct_selection_preserves_all_unorm_codes_and_tolerance_boundaries() {
    let ctx = match GpuContext::headless() {
        Ok(ctx) => ctx,
        Err(e) => {
            eprintln!("skipping GPU test: {e}");
            return;
        }
    };
    let device = &ctx.device;
    let source = format!(
        r#"{DEFERRED_WGSL}
        @fragment fn fs_selection(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {{
            let x = floor(p.x);
            let row = u32(p.y);
            var encoded = x / 255.0;
            if (row > 0u) {{
                let layer = u32(x) % 8u;
                let side = select(-1.0, 1.0, (u32(x) / 8u) % 2u != 0u);
                let delta = select(-0.000001, 0.000001, row == 2u);
                encoded = clamp(f32(layer) * (2.0 / 15.0) + side * (0.0625 + delta), 0.0, 1.0);
            }}
            let weights = array<f32, 8>(0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8);
            let scales = array<vec4<f32>, 2>(vec4<f32>(1.0, 2.0, 3.0, 4.0), vec4<f32>(5.0, 6.0, 7.0, 8.0));
            let direct = terrain_selection(encoded, vec4<f32>(0.1, 0.2, 0.3, 0.4),
                vec4<f32>(0.5, 0.6, 0.7, 0.8), scales);
            var reference = vec2<f32>(0.0);
            for (var layer = 0u; layer < 8u; layer += 1u) {{
                if (abs(encoded - f32(layer) * (2.0 / 15.0)) <= 0.0625) {{
                    reference += vec2<f32>(weights[layer], scales[layer / 4u][layer % 4u]);
                }}
            }}
            return vec4<f32>(direct, reference);
        }}"#
    );
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("terrain selection fixture"),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: None,
        layout: None,
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs_fullscreen"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs_selection"),
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
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: 256,
            height: 3,
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
        size: 256 * 3 * 16,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
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
        pass.draw(0..3, 0..1);
    }
    encoder.copy_texture_to_buffer(
        target.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(256 * 16),
                rows_per_image: Some(3),
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
    {
        let mapped = readback.slice(..).get_mapped_range().unwrap();
        for (i, pixel) in bytemuck::cast_slice::<u8, f32>(&mapped)
            .chunks_exact(4)
            .enumerate()
        {
            assert_eq!(
                &pixel[..2],
                &pixel[2..],
                "layer code {}, row {}",
                i % 256,
                i / 256
            );
        }
    }
    readback.unmap();
}
