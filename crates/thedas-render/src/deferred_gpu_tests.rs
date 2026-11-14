//! Asset-free checks of the actual deferred WGSL probe and SSAO functions.

use super::*;
use bytemuck::Zeroable;
use wgpu::util::DeviceExt;

#[test]
fn probes_preserve_direction_and_hdr_and_ao_rejects_flat_or_distant_surfaces() {
    let ctx = match GpuContext::headless() {
        Ok(ctx) => ctx,
        Err(e) => {
            eprintln!("skipping GPU test: {e}");
            return;
        }
    };
    let device = &ctx.device;
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("deferred probe/AO fixture"),
        source: wgpu::ShaderSource::Wgsl(format!(
            "{DEFERRED_WGSL}\n\
            @fragment fn fs_fixture(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {{\n\
                if (p.x < 1.0) {{ return vec4<f32>(probe_irradiance(vec3<f32>(0.0, 0.0, 1.0)), 1.0); }}\n\
                if (p.x < 2.0) {{ return vec4<f32>(probe_irradiance(vec3<f32>(0.0, 0.0, -1.0)), 1.0); }}\n\
                if (p.x < 3.0) {{ return vec4<f32>(probe_irradiance(vec3<f32>(1.0, 0.0, 0.0)), 1.0); }}\n\
                return vec4<f32>(vec3<f32>(ambient_visibility(vec3<f32>(0.0), vec3<f32>(0.0, 0.0, 1.0))), 1.0);\n\
            }}"
        ).into()),
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("deferred probe/AO fixture"),
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
    let mut probe = thedas_formats::probe::IrradianceProbe::uniform([0.5, 2.0, -0.5]);
    probe.red[2][3] = 0.25;
    probe.red[3][2] = 0.25;
    probe.green[0][0] = 0.75;
    probe.blue[3][3] = -0.5; // Negative SH lobes must clamp, rather than wrap.
    let mut data = DrawData::zeroed();
    data.probe = probe.rows();
    let draw = DrawUniforms {
        mvp: Mat4::IDENTITY.to_cols_array_2d(),
        world: Mat4::IDENTITY.to_cols_array_2d(),
        data,
    };
    let draw_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: bytemuck::bytes_of(&draw),
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let camera = Vec3::new(0.0, 0.0, 2.0);
    let view_proj = Mat4::orthographic_rh(-1.0, 1.0, -1.0, 1.0, 0.1, 10.0)
        * Mat4::look_at_rh(camera, Vec3::ZERO, Vec3::Y);
    let mut lighting = LightingUniforms::zeroed();
    lighting.inv_view_proj = view_proj.inverse().to_cols_array_2d();
    lighting.view_proj = view_proj.to_cols_array_2d();
    lighting.eye = camera.extend(1.0).to_array();
    lighting.size = [64.0, 64.0, 0.0, 0.0];
    lighting.counts[3] = 2;
    let lighting_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: std::mem::size_of::<LightingUniforms>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let uniforms = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: lighting_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: draw_buffer.as_entire_binding(),
            },
        ],
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
    let depth = texture(
        64,
        64,
        wgpu::TextureFormat::R32Float,
        wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
    );
    let depth_view = depth.create_view(&Default::default());
    let targets = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(1),
        entries: &[wgpu::BindGroupEntry {
            binding: 4,
            resource: wgpu::BindingResource::TextureView(&depth_view),
        }],
    });
    let out = texture(
        4,
        1,
        wgpu::TextureFormat::Rgba32Float,
        wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
    );
    let out_view = out.create_view(&Default::default());
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 256,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut render = |heights: &[f32], ao_enabled: bool| -> Vec<f32> {
        lighting.counts[3] = if ao_enabled { 2 } else { 1 };
        ctx.queue
            .write_buffer(&lighting_buffer, 0, bytemuck::bytes_of(&lighting));
        let depths: Vec<f32> = heights
            .iter()
            .map(|&z| (view_proj * Vec3::new(0.0, 0.0, z).extend(1.0)).z)
            .collect();
        ctx.queue.write_texture(
            depth.as_image_copy(),
            bytemuck::cast_slice(&depths),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(64 * 4),
                rows_per_image: Some(64),
            },
            depth.size(),
        );
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &out_view,
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
            pass.set_bind_group(0, &uniforms, &[]);
            pass.set_bind_group(1, &targets, &[]);
            pass.draw(0..3, 0..1);
        }
        encoder.copy_texture_to_buffer(
            out.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(256),
                    rows_per_image: Some(1),
                },
            },
            out.size(),
        );
        ctx.queue.submit([encoder.finish()]);
        let slice = readback.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            tx.send(r).unwrap();
        });
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        rx.recv().unwrap().unwrap();
        let values =
            bytemuck::cast_slice::<u8, f32>(&slice.get_mapped_range().unwrap())[..16].to_vec();
        readback.unmap();
        values
    };
    let flat = vec![0.0; 64 * 64];
    let px = render(&flat, true);
    assert!((px[0] - 1.0).abs() < 1e-4, "+Z red: {px:?}");
    assert!(px[4].abs() < 1e-4, "-Z red: {px:?}");
    assert!(
        (px[9] - 2.75).abs() < 1e-4,
        "+X green preserves HDR: {px:?}"
    );
    assert_eq!(px[2], 0.0, "negative irradiance clamps");
    assert!((px[12] - 1.0).abs() < 1e-4, "a flat plane must stay open");
    let raised = vec![0.3; 64 * 64];
    let ao = render(&raised, true)[12];
    assert!(
        (0.25..0.95).contains(&ao),
        "nearby geometry must occlude: {ao}"
    );
    assert_eq!(render(&raised, false)[12], 1.0, "SSAO comparison switch");
    assert_eq!(
        render(&vec![1.5; 64 * 64], true)[12],
        1.0,
        "distant silhouettes cannot halo"
    );
}
