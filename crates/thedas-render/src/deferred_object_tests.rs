//! Asset-free execution of the same skin, probe and tint functions as real draws.

use super::*;
use bytemuck::Zeroable;
use wgpu::util::DeviceExt;

#[test]
fn skin_transforms_positions_and_normals_and_materials_keep_hdr_and_tint() {
    let ctx = match GpuContext::headless() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("skipping GPU test: {e}");
            return;
        }
    };
    let device = &ctx.device;
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("deferred skin/material fixture"),
        source: wgpu::ShaderSource::Wgsl(format!(r#"{DEFERRED_WGSL}
            @fragment fn fs_fixture(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {{
                let weights = vec4<f32>(1.0, 3.0, 0.0, 0.0);
                let indices = vec4<f32>(0.0, 1.0, 0.0, 0.0);
                if (p.x < 1.0) {{ return vec4<f32>(skin_vector(vec4<f32>(1.0, 2.0, 3.0, 1.0), weights, indices), 1.0); }}
                if (p.x < 2.0) {{ return vec4<f32>(skin_vector(vec4<f32>(1.0, 0.0, 0.0, 0.0), weights, indices), 1.0); }}
                if (p.x < 3.0) {{ return vec4<f32>(object_probe(vec3<f32>(0.0, 0.0, 1.0)), 1.0); }}
                if (p.x < 5.0) {{ return vec4<f32>(object_tint(vec3<f32>(0.2, 0.4, 0.6), vec4<f32>(0.5, 0.25, 0.0, 0.0), p.x > 4.0), 1.0); }}
                if (p.x < 7.0) {{ return vec4<f32>(tint_mode(vec3<f32>(0.2, 0.4, 0.6), vec4<f32>(0.5, 0.25, 0.0, 0.5), false, select(2.0, 3.0, p.x > 6.0)), 1.0); }}
                if (p.x < 8.0) {{ return vec4<f32>(select(0.0, 1.0, point_uses_baked_mask(true, 64u)),
                    select(0.0, 1.0, point_uses_baked_mask(true, 192u)),
                    select(0.0, 1.0, point_uses_baked_mask(true, 0u)),
                    select(0.0, 1.0, point_uses_baked_mask(true, 128u))); }}
                let face = compose_face(vec4<f32>(0.2, 0.4, 0.6, 0.8), vec4<f32>(0.6, 0.8, 1.0, 0.4),
                    vec4<f32>(0.5), vec4<f32>(0.5, 0.7, 0.5, 0.6), vec4<f32>(0.5, 0.6, 0.5, 0.7),
                    vec4<f32>(1.0, 0.0, 0.0, 0.0), vec4<f32>(1.0, 0.0, 0.0, 0.0),
                    vec4<f32>(0.0, 0.0, 0.0, 0.5), vec4<f32>(1.0, 0.0, 0.0, 0.0),
                    vec4<f32>(0.5, 0.6, 0.5, 0.7), vec4<f32>(0.5, 0.0, 0.0, 0.0));
                if (p.x < 9.0) {{ return face.albedo; }}
                if (p.x < 10.0) {{ return face.normal; }}
                if (p.x < 11.0) {{ return face.specular; }}
                let eye = compose_eye(vec4<f32>(0.2, 0.4, 0.6, select(0.0, 1.0, p.x < 14.0)),
                    vec4<f32>(0.5, 0.25, 0.0, 0.0), vec4<f32>(0.5), vec4<f32>(0.5, 0.6, 0.5, 0.7));
                if (p.x < 12.0) {{ return eye.albedo; }}
                if (p.x < 13.0 || p.x >= 15.0) {{ return eye.normal; }}
                return eye.specular;
            }}
        "#).into()),
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("deferred skin/material fixture"),
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
    let mut data = DrawData::zeroed();
    data.probe = thedas_formats::probe::IrradianceProbe::uniform([0.5, 2.0, -0.5]).rows();
    data.tint[0] = [1.0; 4];
    data.tint[1] = [0.0, 1.0, 0.0, 0.0];
    data.tint[2] = [1.0, 0.0, 0.0, 0.0];
    data.tint[4] = [0.0, 1.0, 0.0, 0.0];
    data.tint[5] = [1.0, 1.0, 0.0, 0.0];
    data.tint[8] = [0.5, 0.25, 0.75, 0.0];
    data.tint[9] = [1.0, 0.5, 0.25, 0.0];
    data.character.age[0][0] = 0.5;
    data.character.age[3][0] = 0.25;
    data.character.emotion[0][0] = 0.4;
    data.character.hair_weights[0][0] = 0.5;
    data.character.hair_weights[1][0] = 0.25;
    data.character.hair_colour = [0.1, 0.2, 0.3, 0.0];
    data.character.tattoo[0][0] = 1.0;
    data.character.tattoo[1] = [1.0, 0.5, 0.25, 0.0];
    data.character.specular[0][0] = 0.1;
    data.character.specular[1][0] = 0.2;
    data.character.eye = [30.0, 0.75, 75.0, 0.5];
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
    let mut rows = vec![0u8; SKIN_STRIDE];
    crate::deferred_mesh::write_palette(
        &mut rows,
        &[
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, -1.0, 0.0, 10.0],
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 1.0],
        ],
    );
    let skin_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: &rows,
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry {
                binding: 1,
                resource: draw_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: skin_buffer.as_entire_binding(),
            },
        ],
    });
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: 16,
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
        pass.set_bind_group(0, &group, &[]);
        pass.draw(0..3, 0..1);
    }
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &target,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
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
    readback.slice(..).map_async(wgpu::MapMode::Read, move |r| {
        tx.send(r).unwrap();
    });
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    rx.recv().unwrap().unwrap();
    let mapped = readback.slice(..).get_mapped_range().unwrap();
    let actual: &[f32] = bytemuck::cast_slice(&mapped[..256]);
    let expected = [
        [6.25, 1.25, 3.75, 1.0],
        [0.25, 0.75, 0.0, 1.0],
        [0.5, 2.0, 0.0, 1.0],
        [0.425, 0.2875, 0.15, 1.0],
        [0.2125, 0.3625, 0.45, 1.0],
        [0.09375, 0.109375, 0.13125, 1.0],
        [0.4125, 0.25, 0.13125, 1.0],
        [0.0, 0.0, 1.0, 1.0], // Object point lights must not inherit a sun/lightmap mask.
        [0.1625, 0.2171875, 0.346875, 0.6],
        [0.5, 0.64, 0.5, 0.705],
        [0.012, 0.009, 0.0075, (2.0 / 0.09 - 3.0) / 255.0],
        [0.425, 0.2875, 0.15, 1.0],
        [0.5, 0.6, 0.5, 0.7],
        [0.75, 0.75, 0.75, 29.0 / 255.0],
        [0.5, 0.5, 0.5, 74.0 / 255.0],
        [0.5; 4],
    ];
    for (actual, expected) in actual.iter().zip(expected.iter().flatten()) {
        assert!((actual - expected).abs() < 0.0001, "{actual} != {expected}");
    }
}
