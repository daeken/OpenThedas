//! Recognize the Origins receiver's split selection by its named constants and data
//! flow, never by shader hash, instruction offset or temporary register number.
use crate::bytecode::{
    Decl, Dst, Instruction, Opcode, RegType, Shader, Src, SrcMod, Stage, TextureType,
};
use crate::ctab::RegisterSet;

pub(crate) struct Receiver {
    pub world_at: usize,
    pub world: Src,
    pub select_at: usize,
    pub base: u16,
    pub near: u16,
    pub far: u16,
}

fn scalar(s: &Src, dst: Dst) -> bool {
    s.reg == dst.reg && s.relative.is_none() && s.swizzle.iter().all(|c| dst.mask == 1 << c)
}

fn writes(ins: &Instruction, dst: Dst) -> bool {
    ins.dst
        .is_some_and(|d| d.reg == dst.reg && d.mask & dst.mask != 0)
}

fn boundary(ins: &Instruction) -> bool {
    use Opcode::*;
    ins.predicate.is_some()
        || matches!(
            ins.opcode,
            If | IfC
                | Else
                | EndIf
                | Loop
                | EndLoop
                | Rep
                | EndRep
                | Call
                | CallNz
                | Ret
                | Label
                | Break
                | BreakC
                | BreakP
        )
}

/// Follow the raw plane to its saturated sign mask, allowing independent fade math
/// to be scheduled between them. Stop at any clobber or control-flow boundary.
fn split_mask(shader: &Shader, start: usize, plane: Dst) -> Option<(usize, Dst)> {
    for (index, ins) in shader.instructions.iter().enumerate().skip(start) {
        if boundary(ins) {
            break;
        }
        if ins.opcode == Opcode::Mul
            && ins.src.len() == 2
            && ins
                .dst
                .is_some_and(|d| d.saturate && d.mask.count_ones() == 1)
            && ins
                .src
                .iter()
                .any(|s| scalar(s, plane) && s.modifier == SrcMod::None)
        {
            let scale = ins.src.iter().find(|s| !scalar(s, plane))?;
            if scale.reg.ty == RegType::Const && scale.modifier == SrcMod::None && scale.relative.is_none()
                && shader.instructions.iter().any(|i| matches!(i.def, Some((r, crate::bytecode::DefValue::Float(v))) if r == scale.reg && scale.swizzle.iter().all(|&c| v[c as usize] == 10000.0)))
            {
                return Some((index, ins.dst?));
            }
        }
        if writes(ins, plane) {
            break;
        }
    }
    None
}

pub(crate) fn recognize(shader: &Shader) -> Option<Receiver> {
    if shader.stage != Stage::Pixel {
        return None;
    }
    let table = shader.ctab.as_ref()?;
    let constant = |name, set, count| {
        table
            .constants
            .iter()
            .find(|c| c.name == name && c.set == set && c.count == count)
    };
    let base = constant("g_vShadowData", RegisterSet::Float4, 9)?.register;
    let near = constant("smpShadowPartition1", RegisterSet::Sampler, 1)?.register;
    let far = constant("smpShadowPartition2", RegisterSet::Sampler, 1)?.register;
    if u32::from(base) + 8 >= crate::translate::FLOAT_CONSTANTS
        || near == far
        || near >= 16
        || far >= 16
    {
        return None;
    }
    for sampler in [near, far] {
        if !shader.instructions.iter().any(|i| matches!(i.decl, Some(Decl::Sampler { ty: TextureType::Tex2D, dst }) if dst.reg.num == u32::from(sampler))) {
            return None;
        }
    }
    let mut found = None;
    for (at, pair) in shader.instructions.windows(2).enumerate() {
        let [dot, add] = pair else { continue };
        if dot.opcode != Opcode::Dp3 || dot.src.len() != 2 || add.opcode != Opcode::Add {
            continue;
        }
        let Some(plane_operand) = dot.src.iter().position(|s| {
            s.reg.ty == RegType::Const
                && s.reg.num == u32::from(base) + 8
                && s.modifier == SrcMod::None
                && s.relative.is_none()
        }) else {
            continue;
        };
        let world = *dot.src.get(1 - plane_operand)?;
        let plane = dot.dst?;
        if add.dst != Some(plane)
            || add.src.len() != 2
            || !add
                .src
                .iter()
                .any(|s| scalar(s, plane) && s.modifier == SrcMod::None)
            || !add.src.iter().any(|s| {
                s.reg.ty == RegType::Const
                    && s.reg.num == u32::from(base) + 8
                    && s.swizzle == [3; 4]
                    && s.modifier == SrcMod::None
            })
        {
            continue;
        }
        let Some((mask_at, mask)) = split_mask(shader, at + 2, plane) else {
            continue;
        };
        for (end, ins) in shader.instructions.iter().enumerate().skip(mask_at + 1) {
            if boundary(ins) {
                break;
            }
            if ins.opcode == Opcode::Cmp
                && ins.src.len() == 3
                && scalar(&ins.src[0], mask)
                && ins.src[0].modifier == SrcMod::Neg
            {
                // Both maps must actually be read before selection. Terrain/trees
                // schedule the far estimator before computing the split plane.
                let reads = |sampler| {
                    shader.instructions[..end].iter().any(|i| {
                        i.opcode == Opcode::Tex
                            && i.src.get(1).is_some_and(|s| {
                                s.reg.ty == RegType::Sampler && s.reg.num == u32::from(sampler)
                            })
                    })
                };
                if reads(near) && reads(far) {
                    if found.is_some() {
                        return None;
                    } // Ambiguous: leave the shader faithful.
                    found = Some(Receiver {
                        world_at: at,
                        world,
                        select_at: end,
                        base,
                        near,
                        far,
                    });
                }
                break;
            }
            if writes(ins, mask) {
                break;
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bytecode::tests::{XYZW, dst, ins, src};
    use crate::ctab::{Constant, ConstantTable, ParamClass};
    use crate::{Options, translate};

    fn fixture() -> Shader {
        // Synthetic receiver: unrelated registers, tiny estimators, same split data flow.
        let mut t = vec![0xffff0300];
        t.extend([ins(31, 2), 0x80000005, dst(1, 0, 15)]);
        for s in [3, 6] {
            t.extend([ins(31, 2), 0x90000000, dst(10, s, 15)]);
        }
        t.extend([ins(81, 5), dst(2, 30, 15), 10000.0f32.to_bits(), 0, 0, 0]);
        t.extend([
            ins(8, 3),
            dst(0, 0, 8),
            src(1, 0, XYZW, 0),
            src(2, 18, XYZW, 0),
        ]);
        t.extend([
            ins(2, 3),
            dst(0, 0, 8),
            src(0, 0, 255, 0),
            src(2, 18, 255, 0),
        ]);
        t.extend([
            ins(5, 3),
            dst(0, 1, 1) | 0x100000,
            src(0, 0, 255, 0),
            src(2, 30, 0, 0),
        ]);
        for (r, s) in [(2, 3), (3, 6)] {
            t.extend([
                ins(66, 3),
                dst(0, r, 15),
                src(1, 0, XYZW, 0),
                src(10, s, XYZW, 0),
            ]);
        }
        t.extend([
            ins(88, 4),
            dst(8, 0, 15),
            src(0, 1, 0, 1),
            src(0, 3, XYZW, 0),
            src(0, 2, XYZW, 0),
        ]);
        t.push(0xffff);
        let bytes: Vec<u8> = t.iter().flat_map(|t| t.to_le_bytes()).collect();
        let mut shader = Shader::parse(&bytes).unwrap();
        let c = |name: &str, set, register, count| Constant {
            name: name.into(),
            set,
            register,
            count,
            class: ParamClass::Vector,
            ty: 3,
            rows: 1,
            columns: 4,
            elements: count,
            default: None,
        };
        shader.ctab = Some(ConstantTable {
            creator: "synthetic".into(),
            target: "ps_3_0".into(),
            constants: vec![
                c("g_vShadowData", RegisterSet::Float4, 10, 9),
                c("smpShadowPartition1", RegisterSet::Sampler, 3, 1),
                c("smpShadowPartition2", RegisterSet::Sampler, 6, 1),
            ],
        });
        shader
    }

    #[test]
    fn opt_in_receiver_translates_for_uniform_and_instanced_constants() {
        let shader = fixture();
        assert!(recognize(&shader).is_some());
        let faithful = translate(&shader, &Options::default()).unwrap();
        assert!(!faithful.wgsl.contains("soft_shadow_visibility"));
        for instanced in [false, true] {
            let improved = translate(
                &shader,
                &Options {
                    soft_shadows: true,
                    instanced,
                    ..Default::default()
                },
            )
            .unwrap();
            assert!(
                improved
                    .wgsl
                    .contains("soft_shadow_visibility(soft_shadow_world")
            );
            assert!(improved.wgsl.contains(if instanced {
                "d3d_slots[d3d_inst].fx.w > 0.5"
            } else {
                "d3d_fixup.w > 0.5"
            }));
            #[cfg(feature = "backend-check")]
            {
                // Instancing already needs storage buffers, unavailable on the harness's
                // OpenGL 3.3/ES 3.0 targets. The new branch must retain baseline support.
                let baseline = translate(
                    &shader,
                    &Options {
                        instanced,
                        ..Default::default()
                    },
                )
                .unwrap();
                assert_eq!(
                    crate::backends::failures(&improved.module, &improved.info),
                    crate::backends::failures(&baseline.module, &baseline.info)
                );
            }
        }
    }

    #[test]
    fn unknown_or_changed_receiver_stays_faithful() {
        let mut shader = fixture();
        shader.ctab.as_mut().unwrap().constants[0].name = "different_shadow_system".into();
        assert!(recognize(&shader).is_none());
        let tr = translate(
            &shader,
            &Options {
                soft_shadows: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(!tr.wgsl.contains("soft_shadow_visibility"));
        let mut shader = fixture();
        shader
            .instructions
            .iter_mut()
            .find(|i| i.opcode == Opcode::Cmp)
            .unwrap()
            .src[0]
            .modifier = SrcMod::None;
        assert!(recognize(&shader).is_none());
    }

    #[test]
    fn scheduled_estimators_and_independent_fade_math_keep_the_receiver() {
        let mut shader = fixture();
        let far_at = shader
            .instructions
            .iter()
            .position(|i| i.opcode == Opcode::Tex && i.src[1].reg.num == 6)
            .unwrap();
        let far = shader.instructions.remove(far_at);
        let dot_at = shader
            .instructions
            .iter()
            .position(|i| i.opcode == Opcode::Dp3)
            .unwrap();
        shader.instructions.insert(dot_at, far);
        let add_at = shader
            .instructions
            .iter()
            .position(|i| i.opcode == Opcode::Add)
            .unwrap();
        let mut independent = shader.instructions[add_at].clone();
        independent.dst.as_mut().unwrap().reg.num = 9;
        independent.dst.as_mut().unwrap().mask = 1;
        shader.instructions.insert(add_at + 1, independent);
        assert!(recognize(&shader).is_some());
        for instanced in [false, true] {
            let tr = translate(
                &shader,
                &Options {
                    soft_shadows: true,
                    instanced,
                    ..Default::default()
                },
            )
            .unwrap();
            assert!(tr.wgsl.contains("soft_shadow_visibility(soft_shadow_world"));
        }
        // A changed plane is not the recognized split; leave such a shader faithful.
        let plane = shader.instructions[add_at].dst;
        shader.instructions[add_at + 1].dst = plane;
        assert!(recognize(&shader).is_none());
    }
}
