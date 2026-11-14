//! Exercise actual G-buffer/forward handoffs, including a fade back to opaque.
use super::*;
use crate::d3d::D3dProgram;
use crate::material::{Program, ProgramSource};
use crate::scene::SceneMaterial;
use glam::Vec3;
use std::sync::Arc;
use thedas_formats::mesh::{DeclType, DeclUsage, VertexElement};
use thedas_shader::{
    Translation,
    bytecode::{Stage, Usage},
    ctab::{Constant, ConstantTable, ParamClass, RegisterSet},
    translate::{Interface, IoSlot, Semantic},
};

pub(super) fn translation(wgsl: &str, stage: Stage, constant: Constant) -> Translation {
    let module = naga::front::wgsl::parse_str(wgsl).unwrap();
    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .unwrap();
    Translation {
        wgsl: wgsl.into(),
        module,
        info,
        stage,
        ctab: Some(ConstantTable {
            creator: String::new(),
            target: String::new(),
            constants: vec![constant],
        }),
        interface: Interface {
            inputs: if stage == Stage::Vertex {
                vec![IoSlot {
                    semantic: Semantic {
                        usage: Usage::Position,
                        index: 0,
                    },
                    register: 0,
                    location: 0,
                }]
            } else {
                vec![]
            },
            ..Default::default()
        },
    }
}

struct Fixture;
impl ProgramSource for Fixture {
    fn program(&self, m: &SceneMaterial) -> Option<Program> {
        let black = m.semantic == "black";
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
        let vs = translation(
            r#"
            struct Constants { c: array<vec4<f32>, 256> }
            @group(0) @binding(0) var<uniform> cf: Constants;
            @group(0) @binding(6) var<uniform> fixup: vec4<f32>;
            @vertex fn main(@location(0) v: vec4<f32>) -> @builtin(position) vec4<f32> {
                let p = cf.c[0] * v.x + cf.c[1] * v.y + cf.c[2] * v.z + cf.c[3];
                return vec4<f32>(p.xy + fixup.xy * p.w, p.zw);
            }
        "#,
            Stage::Vertex,
            constant("aur_mWorldViewProj", 4),
        );
        let ps = translation(
            if black {
                "@fragment fn main() -> @location(0) vec4<f32> { return vec4<f32>(0.0, 0.0, 0.0, 1.0); }"
            } else {
                r#"
            struct Constants { c: array<vec4<f32>, 224> }
            @group(0) @binding(3) var<uniform> cf: Constants;
            @fragment fn main() -> @location(0) vec4<f32> { return vec4<f32>(1.0, 0.0, 0.0, cf.c[0].x); }
        "#
            },
            Stage::Pixel,
            constant("mml_fOpacityMultiplier", 1),
        );
        let mut pass = thedas_formats::state::MaterialState::default()
            .pass()
            .unwrap();
        pass.cull = thedas_formats::state::Cull::None;
        let name = if black {
            "black.vsh+black.psh+fixture.sta"
        } else {
            "static1.vsh+static1.psh+fixture.sta"
        };
        Some(Program::D3d(vec![Arc::new(D3dProgram {
            name: name.into(),
            vs,
            ps,
            pass,
            state: Default::default(),
            instanced: false,
            ps_coverage: None,
        })]))
    }
}

#[test]
fn cutaways_return_to_forward_during_fades_and_black_stays_unlit() {
    let ctx = match GpuContext::headless() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("skipping GPU test: {e}");
            return;
        }
    };
    let vfs = Vfs::new();
    let mut programs = ProgramRegistry::default();
    programs.push_front(Box::new(Fixture));
    let camera = Camera::new(Vec3::ZERO, 0.0, 0.0);
    for black in [false, true] {
        let mut scene =
            super::forward_transparency_tests::scene(6.0, if black { "black" } else { "static" });
        scene.static_draws = 1;
        scene.cut_away = vec![0];
        scene.sun = Some((Vec3::X, Vec3::splat(10.0)));
        scene.atmosphere = Some(thedas_formats::level::Atmosphere {
            fog_color: [1.0, 0.5, 1.0],
            fog_intensity: 1.0,
            fog_cap: 1.0,
            sun_color: [1.0; 3],
            sun_intensity: 1.0,
            turbidity: 0.0,
            earth_reflectance: 0.0,
            mie: 0.0,
            rayleigh: 0.0,
            distance: 0.0,
            alpha: 1.0,
            fog_zenith: 0.0,
            fog_tactical: 1.0,
            water_fog_intensity: 0.0,
            water_fog_cap: 0.0,
            moon_scale: 10.0,
            moon_alpha: 0.0,
            moon_cloud_alpha: 0.0,
            moon_rotation: 0.0,
        });
        let mesh = &mut scene.meshes[0];
        mesh.chunk.vertex_size = std::mem::size_of::<Vertex>() as u32;
        mesh.chunk.elements = vec![VertexElement {
            stream: 0,
            offset: 0,
            ty: DeclType::Float3,
            usage: DeclUsage::Position,
            usage_index: 0,
        }];
        mesh.raw_vertices = bytemuck::cast_slice(&mesh.vertices).to_vec();
        for samples in [1, 4] {
            let mut cache = RenderCache {
                samples,
                ..Default::default()
            };
            let mut renderer = AreaRenderer::new_cached_with_path(
                &ctx,
                &vfs,
                &scene,
                &programs,
                wgpu::TextureFormat::Rgba8Unorm,
                &mut cache,
                crate::RenderPath::Deferred,
            );
            assert!(renderer.deferred.as_ref().unwrap().takes(0));
            for (fused, opacity) in [true, false]
                .into_iter()
                .flat_map(|fused| [1.0, 0.5, 0.999, 0.0, 1.0].map(|opacity| (fused, opacity)))
            {
                renderer.set_deferred_fused_reflections(Some(fused));
                renderer.set_cutaway(&ctx, opacity);
                renderer.path = crate::RenderPath::Faithful;
                let faithful = renderer.capture(&ctx, &camera, 32, 32).unwrap();
                renderer.path = crate::RenderPath::Deferred;
                let deferred = renderer.capture(&ctx, &camera, 32, 32).unwrap();
                assert!(!renderer.deferred.as_ref().unwrap().refused());
                if opacity < 1.0 || black {
                    assert_eq!(
                        faithful, deferred,
                        "black={black}, {samples}x, opacity={opacity}"
                    );
                } else {
                    assert_ne!(faithful, deferred, "opaque cutaway must enter G-buffer");
                }
                if black && opacity > 0.0 {
                    assert_eq!(&deferred[(16 * 32 + 16) * 4..][..4], &[0, 0, 0, 255]);
                }
            }
            // A moved draw must refresh its retained uniform slot when it re-enters
            // the frustum, even after several frames spent entirely out of view.
            let away = Camera::new(Vec3::ZERO, std::f32::consts::PI, 0.0);
            renderer.capture(&ctx, &away, 32, 32).unwrap();
            let shifted = glam::Mat4::from_translation(Vec3::new(0.0, 5.0, 0.0));
            renderer.set_transform(&ctx, 0, shifted);
            renderer.capture(&ctx, &away, 32, 32).unwrap();
            let reentered = renderer.capture(&ctx, &camera, 32, 32).unwrap();
            let mut moved_scene = scene.clone();
            moved_scene.draws[0].transform = shifted;
            let mut fresh = AreaRenderer::new_cached_with_path(
                &ctx,
                &vfs,
                &moved_scene,
                &programs,
                wgpu::TextureFormat::Rgba8Unorm,
                &mut cache,
                crate::RenderPath::Deferred,
            );
            fresh.set_deferred_fused_reflections(Some(false));
            assert_eq!(reentered, fresh.capture(&ctx, &camera, 32, 32).unwrap());
        }
    }
}

#[test]
fn baked_lighting_specialization_keeps_light_word_boundaries_and_fallbacks() {
    let ctx = match GpuContext::headless() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("skipping GPU test: {e}");
            return;
        }
    };
    let mut scene = super::forward_transparency_tests::scene(6.0, "static");
    scene.static_draws = 1;
    let mesh = &mut scene.meshes[0];
    mesh.chunk.vertex_size = std::mem::size_of::<Vertex>() as u32;
    mesh.chunk.elements = vec![VertexElement {
        stream: 0,
        offset: 0,
        ty: DeclType::Float3,
        usage: DeclUsage::Position,
        usage_index: 0,
    }];
    mesh.raw_vertices = bytemuck::cast_slice(&mesh.vertices).to_vec();
    let mut programs = ProgramRegistry::default();
    programs.push_front(Box::new(Fixture));
    let mut renderer = AreaRenderer::new_cached_with_path(
        &ctx,
        &Vfs::new(),
        &scene,
        &programs,
        wgpu::TextureFormat::Rgba8Unorm,
        &mut RenderCache::default(),
        crate::RenderPath::Deferred,
    );
    assert!(renderer.deferred.as_ref().unwrap().takes(0));
    let camera = Camera::new(Vec3::ZERO, 0.0, 0.0);
    let mut colours = Vec::new();
    // Include empty words and both sides of every boundary in the 192-light array.
    for count in [
        0, 1, 31, 32, 33, 63, 64, 65, 95, 96, 97, 127, 128, 129, 159, 160, 161, 191, 192,
    ] {
        renderer.set_effect_lights(
            (0..count)
                .map(|i| crate::scene::SceneLight::Point {
                    position: Vec3::new(2.0, (i % 7) as f32 * 0.1, (i % 11) as f32 * 0.1),
                    color: Vec3::new(0.002, 0.001, 0.0005),
                    radius: 10.0,
                    name: format!("fixture {i}"),
                    domain: None,
                    is_static: false,
                    version: 0,
                    intensity: 1.0,
                    room: None,
                })
                .collect(),
        );
        for fused in [true, false] {
            renderer.set_deferred_fused_reflections(Some(fused));
            renderer.set_deferred_specialized_lighting(Some(false));
            let general = renderer.capture(&ctx, &camera, 32, 32).unwrap();
            renderer.set_deferred_specialized_lighting(Some(true));
            let specialized = renderer.capture(&ctx, &camera, 32, 32).unwrap();
            assert!(!renderer.deferred.as_ref().unwrap().refused());
            assert_eq!(general, specialized, "{count} lights, fused={fused}");
            if fused {
                colours.push(specialized[(16 * 32 + 16) * 4]);
            }
        }
    }
    assert_eq!(colours[0], 0);
    assert!(
        colours.last().unwrap() > &20,
        "point lighting must contribute: {colours:?}"
    );
    assert!(colours.windows(2).all(|p| p[0] <= p[1]), "{colours:?}");
    for feature in 0..3 {
        renderer.set_deferred_fused_reflections(Some(true));
        match feature {
            0 => renderer.set_deferred_light_masks(Some(false)),
            1 => renderer.set_deferred_depth_lights(Some(false)),
            _ => renderer.set_deferred_early_light_reject(Some(false)),
        }
        renderer.set_deferred_specialized_lighting(Some(false));
        let general = renderer.capture(&ctx, &camera, 32, 32).unwrap();
        renderer.set_deferred_specialized_lighting(Some(true));
        assert_eq!(general, renderer.capture(&ctx, &camera, 32, 32).unwrap());
        renderer.set_deferred_light_masks(None);
        renderer.set_deferred_depth_lights(None);
        renderer.set_deferred_early_light_reject(None);
    }
}
