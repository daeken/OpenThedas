//! Deferred lighting for ordered character passes; faithful pipelines stay independent.

use super::*;
use thedas_formats::state::{BlendFactor, BlendOp, CompareFunc, PassState};
use wgpu::util::DeviceExt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Family {
    Eyelash,
    Hair(u8),
}

pub(super) fn family(name: &str) -> Option<Family> {
    let mut parts = name.split('+');
    match (parts.next()?, parts.next()?, parts.next()?) {
        ("eye0.vsh", "eyelash0.psh", _) => Some(Family::Eyelash),
        ("hair0_pass1.vsh", "hair0_pass1.psh", "hairalphapass1.sta") => Some(Family::Hair(0)),
        ("hair0.vsh", "hair0.psh", "hairalphapass2.sta") => Some(Family::Hair(1)),
        ("hair0.vsh", "hair0.psh", "hairalphapass3.sta") => Some(Family::Hair(2)),
        ("hair0.vsh", "hair0.psh", "hairalphapass4.sta") => Some(Family::Hair(3)),
        _ => None,
    }
}

fn supported(family: Family, p: &PassState) -> bool {
    let alpha_blend = Some((
        BlendFactor::SrcAlpha,
        BlendFactor::InvSrcAlpha,
        BlendOp::Add,
    ));
    p.z_enable
        && !p.srgb_write
        && p.blend_alpha.is_none_or(|b| Some(b) == p.blend)
        && match family {
            Family::Eyelash => {
                p.z_write && p.color_write == 15 && p.alpha_test.is_some() && p.blend == alpha_blend
            }
            Family::Hair(0) => {
                p.z_write
                    && p.color_write == 0
                    && p.blend.is_none()
                    && p.alpha_test == Some((CompareFunc::GreaterEqual, 255))
                    && p.cull == Cull::None
                    && p.z_func == CompareFunc::Less
            }
            Family::Hair(1) => {
                !p.z_write
                    && p.color_write == 15
                    && p.blend.is_none()
                    && p.alpha_test.is_none()
                    && p.cull == Cull::None
                    && p.z_func == CompareFunc::Equal
            }
            Family::Hair(index @ (2 | 3)) => {
                p.z_write == (index == 3)
                    && p.color_write == 15
                    && p.blend == alpha_blend
                    && p.alpha_test.is_none()
                    && p.z_func == CompareFunc::Less
                    && p.cull == if index == 2 { Cull::Ccw } else { Cull::Cw }
            }
            _ => false,
        }
}

pub(super) struct Material {
    pub family: Family,
    pub alpha: crate::deferred_alpha::Material,
    pub noise: Option<wgpu::TextureView>,
    pub noise_sampler: Option<wgpu::Sampler>,
}

pub(super) fn materials(
    ctx: &GpuContext,
    vfs: &Vfs,
    cache: &mut TextureCache,
    scene: &AreaScene,
    d3d: &D3dPass,
) -> HashMap<usize, Material> {
    let mut result = HashMap::new();
    if !crate::config::CONFIG.forward_lighting || !crate::config::CONFIG.objects {
        return result;
    }
    let mut hair_sets: HashMap<usize, (u8, usize)> = HashMap::new();
    for d in &d3d.draws {
        let program = d3d.program(d);
        let entry = hair_sets.entry(d.draw).or_default();
        entry.1 += 1;
        if let Some(f @ Family::Hair(index)) = family(&program.name)
            && supported(f, &program.pass)
        {
            entry.0 |= 1 << index;
        }
    }
    for (k, d) in d3d.draws.iter().enumerate() {
        let program = d3d.program(d);
        let Some(family) = family(&program.name) else {
            continue;
        };
        if program.instanced
            || !supported(family, &program.pass)
            || (matches!(family, Family::Hair(_)) && hair_sets.get(&d.draw) != Some(&(15, 4)))
        {
            continue;
        }
        let Some(m) = scene
            .draws
            .get(d.draw)
            .and_then(|d| scene.materials.get(d.material))
        else {
            continue;
        };
        let hair = matches!(family, Family::Hair(_));
        if hair {
            let parameters = crate::d3d::material_vector(m, "mml_vHairParameters")
                .unwrap_or([1.0, 200.0, 30.0, 10.0]);
            if !parameters.iter().all(|v| v.is_finite())
                || parameters[1..3].iter().any(|v| *v < 0.0)
            {
                continue;
            }
        }
        let parameter = if hair {
            "mml_tPackedTexture"
        } else {
            "mml_tDiffuse"
        };
        let view = |cache: &mut TextureCache, parameter| {
            [
                m.texture(parameter),
                crate::d3d::texture_default(m, parameter),
            ]
            .into_iter()
            .flatten()
            .filter(|n| !n.eq_ignore_ascii_case("default_missing.dds"))
            .find_map(|n| cache.view(ctx, vfs, &n, TextureType::Tex2D))
        };
        let Some(diffuse) = view(cache, parameter) else {
            continue;
        };
        let Some(sampler) = crate::deferred_alpha::sampler(
            ctx,
            program,
            if hair { "smpHairTexture" } else { "smpDecal" },
            0,
        ) else {
            continue;
        };
        let noise = if hair {
            view(cache, "mml_tTintNoise")
        } else {
            None
        };
        let noise_sampler = if hair {
            crate::deferred_alpha::sampler(ctx, program, "smpNoiseTexture", 1)
        } else {
            None
        };
        // Pass one declares only the packed texture. The same material's colour passes
        // supply noise; missing noise must reject the complete hair family below.
        let opacity =
            crate::d3d::material_vector(m, "mml_fOpacityMultiplier").map_or(1.0, |v| v[0]);
        if !opacity.is_finite() {
            continue;
        }
        let (cmp, reference) = program.pass.alpha_test.unwrap_or((CompareFunc::Always, 0));
        result.insert(
            k,
            Material {
                family,
                alpha: crate::deferred_alpha::Material {
                    diffuse,
                    sampler,
                    test: [
                        crate::deferred_alpha::comparison(cmp),
                        reference as f32,
                        opacity,
                        -1.0,
                    ],
                },
                noise,
                noise_sampler,
            },
        );
    }
    let mut complete: HashMap<usize, u8> = HashMap::new();
    for (&k, m) in &result {
        if let Family::Hair(index) = m.family
            && (index == 0 || (m.noise.is_some() && m.noise_sampler.is_some()))
        {
            *complete.entry(d3d.draws[k].draw).or_default() |= 1 << index;
        }
    }
    result.retain(|&k, m| {
        !matches!(m.family, Family::Hair(_)) || complete.get(&d3d.draws[k].draw) == Some(&15)
    });
    result
}

pub(super) fn log_materials(d3d: &D3dPass, draws: &[Option<usize>], visible: bool) {
    let mut counts: std::collections::BTreeMap<&str, usize> = Default::default();
    for (k, draw) in d3d.draws.iter().enumerate() {
        if draws[k].is_some() && (!visible || (d3d.in_view(k) && !d3d.is_faded(draw))) {
            *counts.entry(&d3d.program(draw).name).or_default() += 1;
        }
    }
    for (program, count) in counts {
        log::info!("deferred-forward-material visible={visible} draws={count} program={program}");
    }
}

pub(super) fn tangents(mesh: &crate::scene::SceneMesh, hair: bool) -> Option<Vec<[f32; 4]>> {
    if !hair {
        return Some(vec![[1.0, 0.0, 0.0, 0.0]; mesh.vertices.len()]);
    }
    let e = mesh
        .chunk
        .element(thedas_formats::mesh::DeclUsage::Tangent, 0)?;
    (0..mesh.vertices.len())
        .map(|i| {
            let at = i * mesh.chunk.vertex_size as usize + e.offset as usize;
            let tangent =
                e.ty.decode(mesh.raw_vertices.get(at..at + e.ty.size() as usize)?);
            tangent.iter().all(|x| x.is_finite()).then_some(tangent)
        })
        .collect()
}

pub(super) fn shader() -> String {
    // Each entry point only reaches its own resources. G-buffer sampling remains
    // unreachable from fs_forward, which uses the material textures at groups 0/1.
    let (materials, lighting) = DEFERRED_WGSL
        .split_once("struct Lighting {")
        .expect("lighting section");
    let lighting = lighting
        // The bounce result (group 3 in the deferred lighting pass) is unreachable from
        // fs_forward; keep it clear of the sun group's new place.
        .replace(
            "@group(3) @binding(0) var bounce_tex",
            "@group(3) @binding(4) var bounce_tex",
        )
        .replace("@group(2)", "@group(3)")
        .replace("@group(0)", "@group(2)")
        .replace(
            "@group(1) @binding(5) var shadow_maps",
            "@group(2) @binding(5) var shadow_maps",
        );
    format!(
        "{materials}\nstruct Lighting {{{lighting}\n{}",
        include_str!("deferred_forward.wgsl")
    )
}

pub(super) struct Gpu {
    pub pipelines: Vec<wgpu::RenderPipeline>,
    pub tangents: HashMap<usize, wgpu::Buffer>,
    pub layout: wgpu::BindGroupLayout,
    pub lights: Option<wgpu::BindGroup>,
    pub own_lights: Option<wgpu::BindGroup>,
    pub sun: Option<wgpu::BindGroup>,
}

impl Gpu {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        samples: u32,
        scene: &wgpu::BindGroupLayout,
        materials: &wgpu::BindGroupLayout,
        lights: &wgpu::BindGroupLayout,
        sun: &wgpu::BindGroupLayout,
        states: &[PassState],
        tangents: &HashMap<usize, Vec<[f32; 4]>>,
    ) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("deferred forward lighting"),
            source: wgpu::ShaderSource::Wgsl(shader().into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("deferred forward lighting"),
            bind_group_layouts: &[Some(scene), Some(materials), Some(lights), Some(sun)],
            immediate_size: 0,
        });
        let pipelines = states.iter().map(|p| device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("deferred ordered forward"), layout: Some(&layout),
            vertex: wgpu::VertexState {module: &module, entry_point: Some("vs_forward"), compilation_options: Default::default(),
                buffers: &[
                    Some(wgpu::VertexBufferLayout { array_stride: std::mem::size_of::<DeferredVertex>() as u64, step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0=>Float32x3,1=>Float32x3,2=>Float32x2,3=>Float32x2,4=>Float32x4,5=>Float32x4]}),
                    Some(wgpu::VertexBufferLayout { array_stride: 16, step_mode: wgpu::VertexStepMode::Vertex, attributes: &wgpu::vertex_attr_array![8=>Float32x4] }),
                ]},
            primitive: wgpu::PrimitiveState {cull_mode: cull_of(p.cull), ..Default::default()},
            depth_stencil: Some(wgpu::DepthStencilState { format: DEPTH_FORMAT, depth_write_enabled: Some(p.z_write), depth_compare: Some(compare(p.z_func)), stencil: Default::default(), bias: Default::default() }),
            multisample: wgpu::MultisampleState {count: samples, ..Default::default()},
            fragment: Some(wgpu::FragmentState {module: &module, entry_point: Some(if p.color_write == 0 {"fs_forward_depth"} else {"fs_forward"}), compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {format, blend: p.blend.map(|_| wgpu::BlendState {color: wgpu::BlendComponent {src_factor: wgpu::BlendFactor::SrcAlpha, dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha, operation: wgpu::BlendOperation::Add}, alpha: wgpu::BlendComponent {src_factor: wgpu::BlendFactor::SrcAlpha, dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha, operation: wgpu::BlendOperation::Add}}), write_mask: wgpu::ColorWrites::from_bits_truncate(p.color_write as u32)})]}),
            multiview_mask: None, cache: None,
        })).collect();
        Self {
            pipelines,
            tangents: tangents
                .iter()
                .map(|(&mesh, data)| {
                    (
                        mesh,
                        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some("deferred hair tangents"),
                            contents: bytemuck::cast_slice(data),
                            usage: wgpu::BufferUsages::VERTEX,
                        }),
                    )
                })
                .collect(),
            layout: lights.clone(),
            lights: None,
            own_lights: None,
            sun: None,
        }
    }
}

fn compare(c: CompareFunc) -> wgpu::CompareFunction {
    match c {
        CompareFunc::Never => wgpu::CompareFunction::Never,
        CompareFunc::Less => wgpu::CompareFunction::Less,
        CompareFunc::Equal => wgpu::CompareFunction::Equal,
        CompareFunc::LessEqual => wgpu::CompareFunction::LessEqual,
        CompareFunc::Greater => wgpu::CompareFunction::Greater,
        CompareFunc::NotEqual => wgpu::CompareFunction::NotEqual,
        CompareFunc::GreaterEqual => wgpu::CompareFunction::GreaterEqual,
        CompareFunc::Always => wgpu::CompareFunction::Always,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn forward_shaders_translate_everywhere() {
        crate::backend_check::translates("deferred ordered forward", &super::shader());
    }
}
