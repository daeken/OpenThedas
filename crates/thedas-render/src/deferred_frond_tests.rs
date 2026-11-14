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

#[test]
fn frond_wrappers_keep_both_constant_layouts_and_reject_other_materials() {
    for instanced in [false, true] {
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
            struct StageOut {{ @builtin(position) position: vec4<f32>, @location(0) o1: vec4<f32> }}
            @vertex
            fn main(input: StageIn) -> StageOut {{
                {instance_body}
                return StageOut(input.v0, vec4<f32>(input.v0.xy, 0.0, 0.0));
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
        vs.interface.outputs.push(IoSlot {
            semantic: Semantic {
                usage: Usage::TexCoord,
                index: 0,
            },
            register: 1,
            location: 0,
        });
        let ps = translation(
            "@fragment fn main() -> @location(0) vec4<f32> { return vec4<f32>(0.0); }".into(),
            Stage::Pixel,
            vec![],
            vec![
                constant("g_vColorMultiplier", 12, RegisterSet::Float4),
                constant("g_fAlphaMultiplier", 13, RegisterSet::Float4),
                constant("smpDiffuseMap", 0, RegisterSet::Sampler),
            ],
        );
        let mut pass = thedas_formats::state::MaterialState::default()
            .pass()
            .unwrap();
        pass.alpha_test = Some((CompareFunc::Greater, 84));
        let mut program = D3dProgram {
            name: "spt_fronds.vsh+spt_fronds.psh+fixture.sta".into(),
            vs,
            ps,
            pass,
            state: Default::default(),
            instanced,
            ps_coverage: None,
        };
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
                failures
                    .iter()
                    .all(|(backend, _)| { *backend == thedas_shader::backends::Backend::OpenGl }),
                "{failures:?}"
            );
        } else {
            crate::backend_check::translates("deferred fronds", &shader);
        }
        assert!(shader.contains("let original = main(input)"));
        assert!(shader.contains("input.v1.xyz"));
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
        program.name = "spt_branch.vsh+spt_branch.psh+fixture.sta".into();
        assert!(source(&program).is_none());
    }
}
