use super::*;

/// Four distinct authored mips per face; no level can be substituted by its neighbour.
fn bc_fixture(fourcc: &[u8; 4], cube: bool) -> Dds {
    // 12x8 also exercises a non-block-aligned intermediate 6x4 level.
    let width = if cube { 8u32 } else { 12 };
    let mut bytes = vec![0; 128];
    bytes[..4].copy_from_slice(b"DDS ");
    for (offset, value) in [(4, 124u32), (12, 8), (16, width), (28, 4), (80, 4)] {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    bytes[84..88].copy_from_slice(fourcc);
    if cube {
        bytes[112..116].copy_from_slice(&0xFE00u32.to_le_bytes());
    }
    for face in 0..if cube { 6 } else { 1 } {
        for mip in 0..4 {
            let red_endpoint = (1 + face * 4 + mip) << 11;
            for _ in 0..(width >> mip).max(1).div_ceil(4) * (8u32 >> mip).max(1).div_ceil(4) {
                if fourcc != b"DXT1" {
                    bytes.extend([255; 8]); // Opaque BC2/BC3 alpha.
                }
                bytes.extend((red_endpoint as u16).to_le_bytes());
                bytes.extend([0; 6]); // Endpoint 0 at every texel.
            }
        }
    }
    Dds::parse(&bytes).unwrap()
}

/// Sample the uploaded production view, using the last logical texel for 2D (including
/// the second block of the 6x4 mip) and sampling each cube face on its axis.
fn sample_mips(ctx: &GpuContext, uploaded: &GpuTexture) -> Vec<u32> {
    let cube = uploaded.kind == DdsKind::Cube;
    let (kind, sample) = if cube {
        (
            "cube",
            "textureSampleLevel(tex, smp, axes[i / 4u], f32(level))",
        )
    } else {
        (
            "2d",
            "textureLoad(tex, vec2<i32>(textureDimensions(tex, level)) - 1, level)",
        )
    };
    let module = ctx
        .device
        .create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sample uploaded DDS mips"),
            source: wgpu::ShaderSource::Wgsl(
                format!(
                    r"
            @group(0) @binding(0) var tex: texture_{kind}<f32>;
            @group(0) @binding(1) var smp: sampler;
            @group(0) @binding(2) var<storage, read_write> result: array<u32>;
            @compute @workgroup_size(1) fn main(@builtin(global_invocation_id) id: vec3<u32>) {{
                let i = id.x;
                let level = i32(i % 4u);
                let axes = array(vec3(1., 0., 0.), vec3(-1., 0., 0.), vec3(0., 1., 0.),
                    vec3(0., -1., 0.), vec3(0., 0., 1.), vec3(0., 0., -1.));
                result[i] = pack4x8unorm({sample});
            }}
        "
                )
                .into(),
            ),
        });
    let pipeline = ctx
        .device
        .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: None,
            layout: None,
            module: &module,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
    let count = if cube { 24 } else { 4 };
    let size = u64::from(count) * 4;
    let buffer = |usage| {
        ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size,
            usage,
            mapped_at_creation: false,
        })
    };
    let result = buffer(wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC);
    let readback = buffer(wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ);
    let sampler = ctx
        .device
        .create_sampler(&wgpu::SamplerDescriptor::default());
    let mut entries = vec![
        wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::TextureView(&uploaded.view),
        },
        wgpu::BindGroupEntry {
            binding: 2,
            resource: result.as_entire_binding(),
        },
    ];
    if cube {
        entries.push(wgpu::BindGroupEntry {
            binding: 1,
            resource: wgpu::BindingResource::Sampler(&sampler),
        });
    }
    let group = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &entries,
    });
    let mut encoder = ctx.device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.dispatch_workgroups(count, 1, 1);
    }
    encoder.copy_buffer_to_buffer(&result, 0, &readback, 0, Some(size));
    ctx.queue.submit([encoder.finish()]);
    let (tx, rx) = std::sync::mpsc::channel();
    let slice = readback.slice(..);
    slice.map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
    ctx.device
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
    rx.recv().unwrap().unwrap();
    slice
        .get_mapped_range()
        .unwrap()
        .chunks_exact(4)
        .map(|bytes| u32::from_le_bytes(bytes.try_into().unwrap()))
        .collect()
}

#[test]
fn uploads_and_samples_every_authored_bc_mip() {
    let mut ctx = match GpuContext::headless() {
        Ok(ctx) => ctx,
        Err(e) => {
            eprintln!("skipping DDS GPU test: {e}");
            return;
        }
    };
    let native_bc = ctx.bc;
    for compressed in [native_bc, false] {
        ctx.bc = compressed;
        for (fourcc, cube, bc_format) in [
            (b"DXT1", false, wgpu::TextureFormat::Bc1RgbaUnorm),
            (b"DXT3", false, wgpu::TextureFormat::Bc2RgbaUnorm),
            (b"DXT5", true, wgpu::TextureFormat::Bc3RgbaUnorm),
        ] {
            let dds = bc_fixture(fourcc, cube);
            let uploaded = upload_dds(&ctx, "authored mip fixture", &dds).unwrap();
            let texture = uploaded.view.texture();
            assert_eq!(texture.mip_level_count(), 4);
            assert_eq!((texture.width(), texture.height()), (dds.width, dds.height));
            assert_eq!(
                texture.format(),
                if compressed {
                    bc_format
                } else {
                    wgpu::TextureFormat::Rgba8Unorm
                }
            );
            for (i, rgba) in sample_mips(&ctx, &uploaded).into_iter().enumerate() {
                let (face, mip) = (i as u32 / 4, i as u32 % 4);
                let expected = dds.decode_rgba8(face, mip).unwrap();
                // The hardware and the reference unpacker may round a 5-bit endpoint
                // differently by one. A neighbouring mip differs by at least eight.
                let actual = rgba.to_le_bytes();
                assert!(
                    actual[0].abs_diff(expected[0]) <= 1,
                    "BC={compressed}, face {face}, mip {mip}: {actual:?} != {:?}",
                    &expected[..4]
                );
                assert_eq!(&actual[1..], &expected[1..4]);
            }
        }
        if !native_bc {
            eprintln!("adapter has no BC support: only the RGBA fallback was exercised");
            break;
        }
    }
}
