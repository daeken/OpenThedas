//! Actual ordered forward draws sharing the level's lights, depth and MSAA frame.
use super::*;
use crate::d3d::D3dProgram;
use crate::material::ProgramSource;
use crate::scene::{SceneLight, SceneMaterial};
use glam::Vec3;
use std::sync::Arc;
use thedas_formats::mesh::{DeclType, DeclUsage, VertexElement};
use thedas_formats::state::{BlendFactor, BlendOp, CompareFunc, Cull, MaterialState};
use thedas_shader::{
    Stage,
    ctab::{Constant, ParamClass, RegisterSet},
};

struct Fixture;
impl ProgramSource for Fixture {
    fn program(&self, m: &SceneMaterial) -> Option<Program> {
        let constant = |name: &str, count| Constant {
            name: name.into(),
            set: RegisterSet::Float4,
            register: 0,
            count,
            class: ParamClass::Vector,
            ty: 3,
            rows: 1,
            columns: 4,
            elements: 1,
            default: None,
        };
        let vs = || {
            super::deferred_cutaway_tests::translation(
                r#"
            struct Constants {c: array<vec4<f32>, 256>}
            @group(0) @binding(0) var<uniform> cf: Constants;
            @group(0) @binding(6) var<uniform> fixup: vec4<f32>;
            @vertex fn main(@location(0) v: vec4<f32>) -> @builtin(position) vec4<f32> {
                let p = cf.c[0] * v.x + cf.c[1] * v.y + cf.c[2] * v.z + cf.c[3];
                return vec4<f32>(p.xy + fixup.xy * p.w, p.zw);
            }"#,
                Stage::Vertex,
                constant("aur_mWorldViewProj", 4),
            )
        };
        let ps = || {
            super::deferred_cutaway_tests::translation(
                r#"
            struct Constants {c: array<vec4<f32>, 224>}
            @group(0) @binding(3) var<uniform> cf: Constants;
            @fragment fn main() -> @location(0) vec4<f32> {return vec4<f32>(1.0, 0.0, 0.0, cf.c[0].x);}
            "#,
                Stage::Pixel,
                constant("mml_fOpacityMultiplier", 1),
            )
        };
        let base = MaterialState::default().pass().unwrap();
        let blend = (
            BlendFactor::SrcAlpha,
            BlendFactor::InvSrcAlpha,
            BlendOp::Add,
        );
        let programs = if m.semantic.starts_with("hair") {
            (0..if m.semantic == "hair-incomplete" {
                3
            } else {
                4
            })
                .map(|i| {
                    let mut p = base;
                    p.cull = [Cull::None, Cull::None, Cull::Ccw, Cull::Cw][i];
                    p.color_write = if i == 0 { 0 } else { 15 };
                    p.z_write = i == 0 || i == 3;
                    p.z_func = if i == 1 {
                        CompareFunc::Equal
                    } else {
                        CompareFunc::Less
                    };
                    p.alpha_test = (i == 0).then_some((CompareFunc::GreaterEqual, 255));
                    p.blend = (i >= 2).then_some(blend);
                    (
                        if i == 0 {
                            "hair0_pass1.vsh+hair0_pass1.psh+hairalphapass1.sta".into()
                        } else {
                            format!("hair0.vsh+hair0.psh+hairalphapass{}.sta", i + 1)
                        },
                        p,
                    )
                })
                .collect::<Vec<_>>()
        } else {
            let mut p = base;
            p.cull = Cull::None;
            if m.semantic == "lash" {
                p.blend = Some(blend);
                p.alpha_test = Some((CompareFunc::Greater, 20));
                vec![("eye0.vsh+eyelash0.psh+eyelashpnch.sta".into(), p)]
            } else {
                vec![("black.vsh+black.psh+fixture.sta".into(), p)]
            }
        };
        Some(Program::D3d(
            programs
                .into_iter()
                .map(|(name, pass)| {
                    Arc::new(D3dProgram {
                        name,
                        pass,
                        vs: vs(),
                        ps: ps(),
                        state: Default::default(),
                        instanced: false,
                        ps_coverage: None,
                    })
                })
                .collect(),
        ))
    }
}

fn scene(x: f32, family: &str) -> AreaScene {
    let mut scene = super::forward_transparency_tests::scene(x, family);
    scene.draws[0].room_id = Some(1);
    scene.materials[0].render_class = Some("Character".into());
    scene.materials[0].object = thedas_formats::MaterialObject::parse(br#"
        <MaterialObject Name="fixture"><Material Name="fixture.mat"/>
        <Texture Name="mml_tDiffuse" ResName="packed.dds"/>
        <Texture Name="mml_tPackedTexture" ResName="packed.dds"/>
        <Texture Name="mml_tTintNoise" ResName="noise.dds"/>
        <Vector4f Name="mml_vHairParameters" value="0 200 30 10"/>
        <Vector4fArray Name="mml_vTintMaskColours" value="0 0 0 0 0 0 0 0 0.5 0.5 0.5 0 0 0 0 0 0 0 0 0 0 0 0 0 0.5 0.5 0.5 0 0 0 0 0 0 0 0 0 0 0 0 0"/>
        </MaterialObject>"#).unwrap();
    let mesh = &mut scene.meshes[0];
    // Perspective-varying depth exercises equality between independently compiled
    // depth-only and colour pipelines, rather than only a constant-depth plane.
    for (i, vertex) in mesh.vertices.iter_mut().enumerate() {
        vertex.position[0] += [0.5, -0.5, 0.0][i];
    }
    mesh.chunk.vertex_size = (std::mem::size_of::<Vertex>() + 48) as u32;
    mesh.chunk.elements = vec![
        VertexElement {
            stream: 0,
            offset: 0,
            ty: DeclType::Float3,
            usage: DeclUsage::Position,
            usage_index: 0,
        },
        VertexElement {
            stream: 0,
            offset: (std::mem::size_of::<Vertex>() + 16) as u32,
            ty: DeclType::Float4,
            usage: DeclUsage::BlendWeight,
            usage_index: 0,
        },
        VertexElement {
            stream: 0,
            offset: (std::mem::size_of::<Vertex>() + 32) as u32,
            ty: DeclType::Float4,
            usage: DeclUsage::BlendIndices,
            usage_index: 0,
        },
        VertexElement {
            stream: 0,
            offset: std::mem::size_of::<Vertex>() as u32,
            ty: DeclType::Float4,
            usage: DeclUsage::Tangent,
            usage_index: 0,
        },
    ];
    for vertex in &mesh.vertices {
        mesh.raw_vertices
            .extend_from_slice(bytemuck::bytes_of(vertex));
        mesh.raw_vertices
            .extend_from_slice(bytemuck::bytes_of(&[0.0f32, 1.0, 0.0, 0.0]));
        mesh.raw_vertices
            .extend_from_slice(bytemuck::bytes_of(&[0.3f32, 0.7, 0.0, 0.0]));
        mesh.raw_vertices
            .extend_from_slice(bytemuck::bytes_of(&[0.0f32, 1.0, 0.0, 0.0]));
    }
    if family != "black" {
        scene.draws[0].skin = Some(crate::scene::DrawSkin {
            skeleton: 0,
            palette: vec![Some(0), Some(1)],
            mesh_node: glam::Mat4::IDENTITY,
        });
    }
    scene
}

#[test]
fn hair_core_equal_colour_and_fractional_faces_share_lights_and_depth_at_one_and_four_samples() {
    let ctx = match GpuContext::headless() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("skipping GPU test: {e}");
            return;
        }
    };
    let mut programs = ProgramRegistry::default();
    programs.push_front(Box::new(Fixture));
    let camera = Camera::new(Vec3::ZERO, 0.0, 0.0);
    for family in ["hair", "lash"] {
        for samples in [1, 4] {
            let mut cache = RenderCache {
                samples,
                ..Default::default()
            };
            let packed = ctx.device.create_texture(&wgpu::TextureDescriptor {
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
            cache.textures.map.insert(
                "packed.dds".into(),
                Some(GpuTexture {
                    view: packed.create_view(&Default::default()),
                    kind: DdsKind::Texture2D,
                }),
            );
            cache.textures.map.insert(
                "noise.dds".into(),
                Some(solid_texture(&ctx, [0, 0, 0, 255])),
            );
            let vfs = Vfs::new();
            let build = |cache: &mut RenderCache, x, family| {
                let mut scene = scene(x, family);
                if family == "black" {
                    // Register a second real room, with its geometry out of view.
                    // Unknown room ids deliberately use the renderer's broad fallback.
                    let mut other = scene.draws[0].clone();
                    other.room_id = Some(2);
                    other.transform = glam::Mat4::from_translation(Vec3::Z * 1000.0);
                    other.sphere.0 += Vec3::Z * 1000.0;
                    scene.draws.push(other);
                }
                AreaRenderer::new_cached_with_path(
                    &ctx,
                    &vfs,
                    &scene,
                    &programs,
                    wgpu::TextureFormat::Rgba8Unorm,
                    cache,
                    crate::RenderPath::Deferred,
                )
            };
            let mut level = build(&mut cache, 10.0, "black");
            let mut object = build(&mut cache, 6.0, family);
            object.d3d.set_bones(
                0,
                vec![
                    [0.999, 0.01, 0.0, 0.02],
                    [-0.01, 0.999, 0.0, -0.03],
                    [0.0, 0.0, 1.0, 0.01],
                    [0.998, -0.02, 0.0, -0.01],
                    [0.02, 0.998, 0.0, 0.04],
                    [0.0, 0.0, 1.0, -0.02],
                ],
            );
            level.clear = wgpu::Color::BLACK;
            // More than the faithful object's original light limit, and a different
            // domain/room. Every compatible receiver must use the level's shared set.
            let light = |domain, room| SceneLight::Point {
                name: "fixture".into(),
                position: Vec3::ZERO,
                color: Vec3::splat(0.1),
                radius: 20.0,
                is_static: false,
                room: Some(room),
                domain: Some(domain),
                version: 0,
                intensity: 1.0,
            };
            level.set_extra_lights(
                &(0..8)
                    .map(|_| light(crate::scene::DOMAIN_CHARACTERS, 1))
                    .collect::<Vec<_>>(),
            );
            let mut full = 0;
            let mut fractional = Vec::new();
            for alpha in [0u8, 20, 21, 127, 254, 255] {
                let rgba = if family == "hair" {
                    [128, 128, alpha, 0]
                } else {
                    [128, 128, 128, alpha]
                };
                ctx.queue.write_texture(
                    packed.as_image_copy(),
                    &rgba,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(4),
                        rows_per_image: Some(1),
                    },
                    packed.size(),
                );
                let pixels = level
                    .capture_with(&ctx, &camera, 32, 32, &mut [&mut object], None)
                    .unwrap();
                let at = (16 * 32 + 16) * 4;
                let pixel = &pixels[at..at + 3];
                let kept = alpha > if family == "lash" { 20 } else { 0 };
                if !kept {
                    assert_eq!(pixel, [0, 0, 0], "{family}, {samples}x, alpha {alpha}");
                } else {
                    assert!(
                        pixel[0] > 0
                            && pixel[0].abs_diff(pixel[1]) <= 1
                            && pixel[1].abs_diff(pixel[2]) <= 1,
                        "{family}, {samples}x, alpha {alpha}: {pixel:?}"
                    );
                }
                if alpha == 127 {
                    full = pixel[0];
                    fractional = pixels.clone();
                }
                if alpha == 255 {
                    assert!(
                        pixel[0] > full * 3 / 2,
                        "opaque core must draw colour once at equal depth: {pixel:?}, fractional={full}"
                    );
                    assert!(
                        pixels.chunks_exact(4).zip(fractional.chunks_exact(4)).all(
                            |(opaque, fractional)| opaque[0].saturating_add(1) >= fractional[0]
                        ),
                        "opaque skin/card pixels must not lose colour between depth and equal-colour passes"
                    );
                }
            }
            // A late contributor remains on the new lighting path after its overlay
            // pass. Removing lights changes it; incompatible domains contribute none.
            let lit = level
                .capture_with(&ctx, &camera, 32, 32, &mut [&mut object], None)
                .unwrap();
            level.set_extra_lights(&[light(crate::scene::DOMAIN_LEVEL, 1)]);
            let unlit = level
                .capture_with(&ctx, &camera, 32, 32, &mut [&mut object], None)
                .unwrap();
            let at = (16 * 32 + 16) * 4;
            assert!(lit[at] > unlit[at] + 20);
            level.set_extra_lights(&[light(crate::scene::DOMAIN_CHARACTERS, 2)]);
            let other_room = level
                .capture_with(&ctx, &camera, 32, 32, &mut [&mut object], None)
                .unwrap();
            assert!(
                unlit == other_room,
                "a light from an unreachable room must contribute nothing"
            );
            for opacity in [0.5, 0.0, 1.0] {
                object.d3d.set_opacity(&ctx, 0, opacity);
                level.set_deferred_forward_lighting(Some(false));
                let old = level
                    .capture_with(&ctx, &camera, 32, 32, &mut [&mut object], None)
                    .unwrap();
                level.set_deferred_forward_lighting(Some(true));
                let new = level
                    .capture_with(&ctx, &camera, 32, 32, &mut [&mut object], None)
                    .unwrap();
                if opacity < 1.0 {
                    assert_eq!(old, new, "faded {family} must return as a complete family");
                } else {
                    assert_ne!(
                        old, new,
                        "opacity one must restore deferred forward lighting"
                    );
                }
            }
        }
    }
}

#[test]
fn incomplete_hair_missing_noise_and_missing_tangents_keep_the_complete_family_faithful() {
    let ctx = match GpuContext::headless() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("skipping GPU test: {e}");
            return;
        }
    };
    let mut programs = ProgramRegistry::default();
    programs.push_front(Box::new(Fixture));
    for case in ["hair-incomplete", "missing-noise", "missing-tangent"] {
        let mut cache = RenderCache::default();
        cache.textures.map.insert(
            "packed.dds".into(),
            Some(solid_texture(&ctx, [128, 128, 255, 0])),
        );
        if case != "missing-noise" {
            cache.textures.map.insert(
                "noise.dds".into(),
                Some(solid_texture(&ctx, [0, 0, 0, 255])),
            );
        }
        let mut scene = scene(
            6.0,
            if case == "hair-incomplete" {
                case
            } else {
                "hair"
            },
        );
        if case == "missing-tangent" {
            scene.meshes[0]
                .chunk
                .elements
                .retain(|e| e.usage != DeclUsage::Tangent);
        }
        let renderer = AreaRenderer::new_cached_with_path(
            &ctx,
            &Vfs::new(),
            &scene,
            &programs,
            wgpu::TextureFormat::Rgba8Unorm,
            &mut cache,
            crate::RenderPath::Deferred,
        );
        // The independent packed cutout point-caster may still keep a DeferredPass.
        assert!(
            renderer
                .deferred
                .as_ref()
                .is_none_or(|p| p.forward_draw_count() == 0),
            "{case}"
        );
    }
}
