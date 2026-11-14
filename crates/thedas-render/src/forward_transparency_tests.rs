//! Coverage of the forward phases over shared single-sample and MSAA depth.
use super::*;
use crate::material::{MaterialProgram, Program, ProgramSource, STAND_IN_WGSL};
use crate::scene::{DrawItem, SceneMaterial, SceneMesh};
use glam::{Mat4, Vec3};
use thedas_formats::{
    MaterialObject,
    mesh::{IndexFormat, MeshChunk},
};

struct Colours;
impl ProgramSource for Colours {
    fn program(&self, m: &SceneMaterial) -> Option<Program> {
        let glass = m.semantic == "glass";
        Some(Program::StandIn(MaterialProgram {
            name: format!("transparency-fixture-{}", m.semantic),
            wgsl: format!(r#"{STAND_IN_WGSL}
                @fragment fn fs_red() -> @location(0) vec4<f32> {{ return vec4<f32>(1.0, 0.0, 0.0, 1.0); }}
                @fragment fn fs_glass() -> @location(0) vec4<f32> {{ return vec4<f32>(0.0, 1.0, 0.0, 0.5); }}"#).into(),
            vs_entry: "vs_main", fs_entry: if glass { "fs_glass" } else { "fs_red" },
            blend: if glass { BlendMode::AlphaBlend } else { BlendMode::Opaque }, cull_back: false,
        }))
    }
}

fn scene(x: f32, semantic: &str) -> AreaScene {
    AreaScene {
        meshes: vec![SceneMesh {
            key: format!("triangle-{x}"),
            vertices: [(-10.0, -10.0), (10.0, -10.0), (0.0, 10.0)]
                .map(|(y, z)| Vertex {
                    position: [x, y, z],
                    normal: [-1.0, 0.0, 0.0],
                    uv0: [0.0; 2],
                    uv1: [0.0; 2],
                })
                .to_vec(),
            indices: vec![0, 1, 2],
            chunk: MeshChunk {
                name: "triangle".into(),
                vertex_size: 0,
                vertex_count: 3,
                index_count: 3,
                primitive: 0,
                index_format: IndexFormat::U16,
                base_vertex: 0,
                vertex_offset: 0,
                start_index: 0,
                elements: vec![],
                bounds: None,
            },
            raw_vertices: vec![],
            positions: None,
            vertex_light: None,
        }],
        materials: vec![SceneMaterial {
            mao: "fixture.mao".into(),
            object: MaterialObject::parse(
                br#"<MaterialObject Name="fixture"><Material Name="static.mat"/></MaterialObject>"#,
            )
            .unwrap(),
            definition: None,
            semantic: semantic.into(),
            render_class: None,
            technique: None,
        }],
        draws: vec![DrawItem {
            mesh: 0,
            material: 0,
            transform: Mat4::IDENTITY,
            lightmap: None,
            sphere: (Vec3::new(x, 0.0, 0.0), 20.0),
            model: "triangle".into(),
            room: "room".into(),
            room_id: None,
            probe: None,
            skin: None,
            cast_shadow: None,
            lod: None,
        }],
        ..Default::default()
    }
}

#[test]
fn transparency_blends_over_forward_opaque_overlays_and_obeys_shared_depth() {
    let ctx = match GpuContext::headless() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("skipping GPU test: {e}");
            return;
        }
    };
    let mut programs = ProgramRegistry::default();
    programs.push_front(Box::new(Colours));
    for samples in [1, 4] {
        for (late, glass_x, expected) in [
            (false, 4.0, [255u8, 0, 0]),
            (true, 4.0, [127, 128, 0]),
            (true, 8.0, [255, 0, 0]),
        ] {
            let mut cache = RenderCache {
                samples,
                ..Default::default()
            };
            let mut glass = AreaRenderer::new_cached(
                &ctx,
                &Vfs::new(),
                &scene(glass_x, "glass"),
                &programs,
                wgpu::TextureFormat::Rgba8Unorm,
                &mut cache,
            );
            let mut solid = AreaRenderer::new_cached(
                &ctx,
                &Vfs::new(),
                &scene(6.0, "solid"),
                &programs,
                wgpu::TextureFormat::Rgba8Unorm,
                &mut cache,
            );
            let target = ctx.device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d {
                    width: 32,
                    height: 32,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let target_view = target.create_view(&Default::default());
            let frame = glass.frame_view(&ctx.device, &target_view, 32, 32);
            let depth = glass.depth(&ctx.device, 32, 32);
            let mut encoder = ctx.device.create_command_encoder(&Default::default());
            {
                let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: None,
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &frame,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &depth,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(1.0),
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
            }
            // The shared deferred pass sets this marker after it has filled opaque depth.
            glass.late_transparency = late;
            let camera = Camera::new(Vec3::ZERO, 0.0, 0.0);
            glass.render_over(&ctx, &mut encoder, &frame, 32, 32, &camera, &depth);
            solid.render_over(&ctx, &mut encoder, &frame, 32, 32, &camera, &depth);
            glass.finish_transparency(&mut encoder, &frame, &depth);
            assert!(!glass.late_transparency);
            // A second finish must not draw the same glass/particles twice.
            glass.finish_transparency(&mut encoder, &frame, &depth);
            glass.resolve(&ctx.device, &mut encoder, &target_view, 32, 32);
            let readback = ctx.device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: 256 * 32,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            encoder.copy_texture_to_buffer(
                target.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &readback,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(256),
                        rows_per_image: Some(32),
                    },
                },
                target.size(),
            );
            ctx.queue.submit([encoder.finish()]);
            let (tx, rx) = std::sync::mpsc::channel();
            readback
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
            ctx.device
                .poll(wgpu::PollType::wait_indefinitely())
                .unwrap();
            rx.recv().unwrap().unwrap();
            let mapped = readback.slice(..).get_mapped_range().unwrap();
            let actual = &mapped[16 * 256 + 16 * 4..16 * 256 + 16 * 4 + 3];
            for (a, e) in actual.iter().zip(expected) {
                assert!(
                    (*a as i32 - e as i32).abs() <= 1,
                    "{samples}x, late {late}, glass {glass_x}: {actual:?} != {expected:?}"
                );
            }
        }
    }
}
