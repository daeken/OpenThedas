use super::*;
use thedas_shader::{
    Translation,
    bytecode::Stage,
    ctab::{Constant, ConstantTable, ParamClass},
    translate::{Interface, IoSlot, Semantic},
};

fn translation(
    wgsl: String,
    stage: Stage,
    inputs: Vec<IoSlot>,
    constants: Vec<Constant>,
) -> Translation {
    let module = naga::front::wgsl::parse_str(&wgsl).unwrap();
    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .unwrap();
    Translation {
        wgsl,
        module,
        info,
        stage,
        ctab: Some(ConstantTable {
            creator: String::new(),
            target: String::new(),
            constants,
        }),
        interface: Interface {
            inputs,
            ..Default::default()
        },
    }
}

fn constant(name: &str, register: u16, set: RegisterSet) -> Constant {
    Constant {
        name: name.into(),
        register,
        set,
        count: 1,
        class: ParamClass::Vector,
        ty: 3,
        rows: 1,
        columns: 4,
        elements: 1,
        default: None,
    }
}

fn program(family: Family, instanced: bool) -> D3dProgram {
    let declarations = if instanced {
        format!(
            r#"{}
            @group(0) @binding(7) var<storage, read> d3d_slots: array<D3dSlot>;
            @group(0) @binding(8) var<storage, read> d3d_instances: array<u32>;
            var<private> d3d_inst: u32;
        "#,
            thedas_shader::translate::slot_struct()
        )
    } else {
        "struct FloatConstants { c: array<vec4<f32>, 256> }
         @group(0) @binding(0) var<uniform> cf: FloatConstants;
         @group(0) @binding(6) var<uniform> d3d_fixup: vec4<f32>;"
            .into()
    };
    let instance_input = if instanced {
        "@builtin(instance_index) d3d_instance: u32,"
    } else {
        ""
    };
    let instance_body = if instanced {
        "d3d_inst = d3d_instances[input.d3d_instance];"
    } else {
        ""
    };
    let wgsl = format!(
        r#"{declarations}
        struct StageIn {{ @location(0) v0: vec4<f32>, @location(1) v1: vec4<f32>, {instance_input} }}
        struct StageOut {{
            @builtin(position) position: vec4<f32>,
            @location(0) o1: vec4<f32>, @location(1) o2: vec4<f32>,
            @location(2) o3: vec4<f32>, @location(3) o4: vec4<f32>,
        }}
        @vertex
        fn main(input: StageIn) -> StageOut {{
            {instance_body}
            return StageOut(input.v0, vec4<f32>(1.0, 0.0, 0.0, 0.0),
                vec4<f32>(0.0, 1.0, 0.0, 0.0), vec4<f32>(0.0, 0.0, 1.0, 0.0),
                vec4<f32>(0.5));
        }}
    "#
    );
    let mut vs = translation(
        wgsl,
        Stage::Vertex,
        vec![IoSlot {
            semantic: Semantic {
                usage: Usage::Normal,
                index: 0,
            },
            register: 1,
            location: 1,
        }],
        vec![constant("g_vRotationTrigValues", 39, RegisterSet::Float4)],
    );
    for index in 0..4 {
        vs.interface.outputs.push(IoSlot {
            semantic: Semantic {
                usage: Usage::TexCoord,
                index,
            },
            register: u32::from(index) + 1,
            location: u32::from(index),
        });
    }
    let ps = translation(
        "@fragment fn main() -> @location(0) vec4<f32> { return vec4<f32>(0.0); }".into(),
        Stage::Pixel,
        vec![],
        vec![
            constant("g_vColorMultiplier", 12, RegisterSet::Float4),
            constant("g_fAlphaMultiplier", 13, RegisterSet::Float4),
            constant("smpDiffuseMap", 0, RegisterSet::Sampler),
            constant("smpNormalMap", 2, RegisterSet::Sampler),
        ],
    );
    let mut pass = thedas_formats::state::MaterialState::default()
        .pass()
        .unwrap();
    pass.alpha_test = (family != Family::Branches).then_some((CompareFunc::Greater, 84));
    let name = match family {
        Family::Fronds => "fronds",
        Family::Branches => "branch",
        Family::MeshLeaves => "leafmeshes",
        Family::LeafCards => "leafcards",
    };
    D3dProgram {
        name: format!("spt_{name}.vsh+spt_{name}.psh+fixture.sta"),
        vs,
        ps,
        pass,
        state: Default::default(),
        instanced,
        ps_coverage: None,
    }
}

#[test]
fn tree_wrappers_keep_both_constant_layouts_and_reject_incompatible_materials() {
    for family in [
        Family::Fronds,
        Family::Branches,
        Family::MeshLeaves,
        Family::LeafCards,
    ] {
        for instanced in [false, true] {
            let mut program = program(family, instanced);
            let shader = source(&program).unwrap();
            if instanced {
                let module = naga::front::wgsl::parse_str(&shader).unwrap();
                let info = naga::valid::Validator::new(
                    naga::valid::ValidationFlags::all(),
                    naga::valid::Capabilities::all(),
                )
                .validate(&module)
                .unwrap();
                let failures = thedas_shader::backends::failures(&module, &info);
                // The faithful instanced stream already requires storage buffers.
                assert!(
                    failures.iter().all(|(backend, _)| {
                        *backend == thedas_shader::backends::Backend::OpenGl
                    }),
                    "{family:?}: {failures:?}"
                );
            } else {
                crate::backend_check::translates("deferred trees", &shader);
            }
            assert!(shader.contains("let original = main(input)"));
            if family != Family::Branches {
                assert!(shader.contains(if instanced {
                    "d3d_slots[in.slot].fx.z"
                } else {
                    "d3d_fixup.z"
                }));
                program.pass.alpha_test = Some((CompareFunc::Always, 90));
                assert!(source(&program).is_some());
                program.pass.alpha_test = Some((CompareFunc::GreaterEqual, 84));
                assert!(source(&program).is_none());
                program.pass.alpha_test = Some((CompareFunc::Greater, 84));
            } else {
                program
                    .ps
                    .ctab
                    .as_mut()
                    .unwrap()
                    .constants
                    .retain(|c| c.name != "smpNormalMap");
                assert!(source(&program).is_none());
                program = self::program(family, instanced);
            }
            program.pass.z_write = false;
            assert!(source(&program).is_none());
            program.pass.z_write = true;
            program.pass.color_write = 7;
            assert!(source(&program).is_none());
            program.pass.color_write = 15;
            program.name = "spt_billboards0.vsh+spt_billboards0.psh+fixture.sta".into();
            assert!(source(&program).is_none());
        }
    }
}

#[test]
fn sampled_tree_materials_keep_leaf_holes_and_bark_normal_mapping() {
    use wgpu::util::DeviceExt;
    let ctx = crate::gpu::GpuContext::headless().unwrap();
    let device = &ctx.device;
    let source_texture = |rgba: [u8; 4]| {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: 1,
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
        ctx.queue.write_texture(
            texture.as_image_copy(),
            &rgba,
            Default::default(),
            texture.size(),
        );
        texture.create_view(&Default::default())
    };
    let normal = source_texture([255, 128, 128, 255]);
    let sampler = device.create_sampler(&Default::default());
    let mut constants = [[0.0f32; 4]; 256];
    constants[12] = [0.2, 0.3, 0.4, 1.0];
    constants[13] = [1.0; 4];
    let pixel = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: bytemuck::cast_slice(&constants),
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let reference = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: bytemuck::cast_slice(&[0.0f32, 0.0, 84.0, 0.0]),
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let frame = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: &[0; std::mem::size_of::<FrameUniforms>()],
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let targets: Vec<_> = (0..4)
        .map(|_| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            })
        })
        .collect();
    let views: Vec<_> = targets
        .iter()
        .map(|t| t.create_view(&Default::default()))
        .collect();
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 256,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    for family in Family::NEW {
        let shader = source(&program(family, false)).unwrap()
            + r#"
            @vertex fn vs_fixture(@builtin(vertex_index) i: u32) -> TreeVertex {
                let xy = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
                return TreeVertex(vec4<f32>(xy[i], 0.0, 1.0), vec2<f32>(0.5),
                    vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(0.0, 0.0, 1.0), 0u);
            }
        "#;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: None,
            source: wgpu::ShaderSource::Wgsl(shader.into()),
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: None,
            layout: None,
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_fixture"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_tree"),
                compilation_options: Default::default(),
                targets: &vec![
                    Some(wgpu::ColorTargetState {
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL
                    });
                    4
                ],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let mut entries = vec![wgpu::BindGroupEntry {
            binding: 3,
            resource: pixel.as_entire_binding(),
        }];
        if family != Family::Branches {
            entries.push(wgpu::BindGroupEntry {
                binding: 6,
                resource: reference.as_entire_binding(),
            });
        }
        let constants = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(0),
            entries: &entries,
        });
        let frame = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipeline.get_bind_group_layout(2),
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: frame.as_entire_binding(),
            }],
        });
        // At reference 84, 84 rejects and 85 survives. Bark is opaque even at alpha 0.
        for alpha in [0, 84, 85, 255] {
            let diffuse = source_texture([128, 192, 64, alpha]);
            let mut entries = vec![
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&diffuse),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ];
            if family == Family::Branches {
                entries.extend([
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::TextureView(&normal),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: wgpu::BindingResource::Sampler(&sampler),
                    },
                ]);
            }
            let material = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &pipeline.get_bind_group_layout(1),
                entries: &entries,
            });
            let mut encoder = device.create_command_encoder(&Default::default());
            {
                let attachments: Vec<_> = views
                    .iter()
                    .map(|view| {
                        Some(wgpu::RenderPassColorAttachment {
                            view,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                                store: wgpu::StoreOp::Store,
                            },
                        })
                    })
                    .collect();
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: None,
                    color_attachments: &attachments,
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                pass.set_pipeline(&pipeline);
                pass.set_bind_group(0, &constants, &[]);
                pass.set_bind_group(1, &material, &[]);
                pass.set_bind_group(2, &frame, &[]);
                pass.draw(0..3, 0..1);
            }
            encoder.copy_texture_to_buffer(
                targets[2].as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &readback,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(256),
                        rows_per_image: Some(1),
                    },
                },
                targets[2].size(),
            );
            ctx.queue.submit([encoder.finish()]);
            let (tx, rx) = std::sync::mpsc::channel();
            readback
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |result| tx.send(result).unwrap());
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            rx.recv().unwrap().unwrap();
            let mapped = readback.slice(..).get_mapped_range().unwrap();
            let expected = if family != Family::Branches && alpha <= 84 {
                [0, 0, 0, 0]
            } else if family == Family::Branches {
                // The fixture swaps tangent-space X/Y. The red normal map becomes +Y.
                [1, 253, 0, 255]
            } else {
                [0, 255, 0, 255]
            };
            for (value, expected) in mapped[..4].iter().zip(expected) {
                assert!(
                    value.abs_diff(expected) <= 1,
                    "{family:?}, alpha {alpha}: {:?}",
                    &mapped[..4]
                );
            }
            drop(mapped);
            readback.unmap();
        }
    }
}
