//! Tree G-buffer materials over the faithful wind, LOD and instance streams.

use super::{FrameUniforms, GBUFFER};
use crate::{d3d::D3dProgram, d3d_gpu::D3dPass, gpu::GpuMesh, scene::AreaScene};
use std::collections::HashMap;
use thedas_formats::state::{CompareFunc, Cull};
use thedas_shader::{bytecode::Usage, ctab::RegisterSet};

struct Variant {
    family: Family,
    source: String,
    layouts: [wgpu::BindGroupLayout; 2],
    attributes: Vec<wgpu::VertexAttribute>,
    cull: Option<wgpu::Face>,
}

#[cfg(test)]
#[path = "deferred_tree_tests.rs"]
pub(super) mod tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Family {
    Fronds,
    Branches,
    MeshLeaves,
    LeafCards,
}

impl Family {
    pub(super) const NEW: [Self; 3] = [Self::Branches, Self::MeshLeaves, Self::LeafCards];

    pub(super) fn of(program: &D3dProgram) -> Option<Self> {
        let mut names = program.name.split('+');
        match (names.next()?, names.next()?) {
            ("spt_fronds.vsh", "spt_fronds.psh") => Some(Self::Fronds),
            ("spt_branch.vsh", "spt_branch.psh") => Some(Self::Branches),
            ("spt_leafmeshes.vsh", "spt_leafmeshes.psh") => Some(Self::MeshLeaves),
            ("spt_leafcards.vsh", "spt_leafcards.psh") => Some(Self::LeafCards),
            _ => None,
        }
    }

    fn enabled(self) -> bool {
        let config = &crate::config::CONFIG;
        match self {
            Self::Fronds => config.fronds,
            Self::Branches => config.branches,
            Self::MeshLeaves => config.mesh_leaves,
            Self::LeafCards => config.leaf_cards,
        }
    }
}

#[derive(Default)]
pub(super) struct Plan {
    variants: Vec<Variant>,
    draws: Vec<(usize, usize)>,
    active: [bool; 4],
    enabled: [Option<bool>; 4],
}

pub(super) const ALPHA_WGSL: &str = r#"
fn tree_alpha_kept(alpha: f32, opacity: f32, reference: f32) -> bool {
    return round(clamp(alpha * opacity, 0.0, 1.0) * 255.0) > reference;
}
"#;

pub(super) struct Gpu {
    pipelines: Vec<wgpu::RenderPipeline>,
    frame: wgpu::BindGroup,
}

pub(super) fn source(program: &D3dProgram) -> Option<String> {
    let family = Family::of(program)?;
    let p = &program.pass;
    let alpha_supported = match family {
        Family::Branches => p.alpha_test.is_none(),
        // D3dSource resolves SPT's unnamed/Always alpha function to Greater.
        _ => matches!(
            p.alpha_test,
            Some((CompareFunc::Greater | CompareFunc::Always, _))
        ),
    };
    if p.blend.is_some()
        || !alpha_supported
        || !p.z_enable
        || !p.z_write
        || p.z_func != CompareFunc::LessEqual
        || p.color_write != 15
    {
        return None;
    }
    let register = |stage: &thedas_shader::Translation, name, set| {
        let constant = stage.ctab.as_ref()?.get(name)?;
        (constant.set == set).then_some(constant.register)
    };
    let ambient = register(&program.ps, "g_vColorMultiplier", RegisterSet::Float4)?;
    let opacity = register(&program.ps, "g_fAlphaMultiplier", RegisterSet::Float4)?;
    let diffuse = register(&program.ps, "smpDiffuseMap", RegisterSet::Sampler)? * 2;
    let output = |index| {
        program
            .vs
            .interface
            .outputs
            .iter()
            .find(|s| s.semantic.usage == Usage::TexCoord && s.semantic.index == index)
            .map(|s| s.register)
    };
    let uv = output(if family == Family::Branches { 3 } else { 0 })?;
    // The translator emits a regular WGSL function body. Calling it retains the
    // complete authored wind deformation and D3D9 clip-space fixup.
    let vertex = program.vs.wgsl.replacen("@vertex\n", "", 1);
    if vertex == program.vs.wgsl {
        return None;
    }
    let (constants, ambient_value, opacity_value, reference) = if program.instanced {
        (
            String::new(),
            format!("d3d_slots[in.slot].pf[{ambient}].rgb"),
            format!("d3d_slots[in.slot].pf[{opacity}].x"),
            "d3d_slots[in.slot].fx.z".to_string(),
        )
    } else {
        (
            "@group(0) @binding(3) var<uniform> tree_pixel_constants: FloatConstants;".to_string(),
            format!("tree_pixel_constants.c[{ambient}].rgb"),
            format!("tree_pixel_constants.c[{opacity}].x"),
            "d3d_fixup.z".to_string(),
        )
    };
    let (basis, normal_declaration, normal_value) = match family {
        Family::Branches => {
            let rows = [output(0)?, output(1)?, output(2)?];
            let normal = register(&program.ps, "smpNormalMap", RegisterSet::Sampler)? * 2;
            (
                format!("original.o{}.xyz, original.o{}.xyz, original.o{}.xyz", rows[0], rows[1], rows[2]),
                format!("@group(1) @binding({normal}) var tree_normal: texture_2d<f32>;\n@group(1) @binding({}) var tree_normal_sampler: sampler;", normal + 1),
                "let mapped = textureSample(tree_normal, tree_normal_sampler, in.uv).xyz * 2.0 - 1.0;\n    let normal = normalize(vec3<f32>(dot(in.basis0, mapped), dot(in.basis1, mapped), dot(in.basis2, mapped)));".to_string(),
            )
        }
        Family::MeshLeaves | Family::LeafCards => (
            format!(
                "original.o{}.xyz, vec3<f32>(0.0), vec3<f32>(0.0)",
                output(1)?
            ),
            String::new(),
            "let normal = normalize(in.basis0);".to_string(),
        ),
        Family::Fronds => {
            let rotation = register(&program.vs, "g_vRotationTrigValues", RegisterSet::Float4)?;
            let input = program
                .vs
                .interface
                .inputs
                .iter()
                .find(|s| s.semantic.usage == Usage::Normal && s.semantic.index == 0)?
                .register;
            let rotation = if program.instanced {
                format!("d3d_slots[d3d_inst].vf[{rotation}]")
            } else {
                format!("cf.c[{rotation}]")
            };
            (
                format!(
                    "normalize(vec3<f32>(dot({rotation}.yzw, input.v{input}.xyz), dot({rotation}.xyw, input.v{input}.xyz), input.v{input}.z)), vec3<f32>(0.0), vec3<f32>(0.0)"
                ),
                String::new(),
                "let normal = normalize(in.basis0);".to_string(),
            )
        }
    };
    let alpha_test = if family == Family::Branches {
        String::new()
    } else {
        format!("if (!tree_alpha_kept(diffuse.a, {opacity_value}, {reference})) {{ discard; }}")
    };
    let slot = if program.instanced { "d3d_inst" } else { "0u" };
    Some(format!(
        r#"{vertex}
{ALPHA_WGSL}
{constants}
@group(1) @binding({diffuse}) var tree_diffuse: texture_2d<f32>;
@group(1) @binding({sampler}) var tree_sampler: sampler;
{normal_declaration}

struct TreeFrame {{ eye: vec4<f32>, fixup: vec4<f32>, fog: vec4<f32> }}
@group(2) @binding(0) var<uniform> tree_frame: TreeFrame;
struct TreeVertex {{
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) basis0: vec3<f32>,
    @location(2) basis1: vec3<f32>,
    @location(3) basis2: vec3<f32>,
    @location(4) @interpolate(flat) slot: u32,
}}
@vertex fn vs_tree(input: StageIn) -> TreeVertex {{
    let original = main(input);
    return TreeVertex(original.position, original.o{uv}.xy, {basis}, {slot});
}}
struct TreeBuffer {{
    @location(0) albedo: vec4<f32>,
    @location(1) specular: vec4<f32>,
    @location(2) normal: vec4<f32>,
    @location(3) lightmap: vec4<f32>,
}}
fn tree_oct_encode(n: vec3<f32>) -> vec2<f32> {{
    let p = n.xy / (abs(n.x) + abs(n.y) + abs(n.z));
    if (n.z >= 0.0) {{ return p; }}
    return (1.0 - abs(p.yx)) * select(vec2<f32>(-1.0), vec2<f32>(1.0), p >= vec2<f32>(0.0));
}}
@fragment fn fs_tree(in: TreeVertex) -> TreeBuffer {{
    let diffuse = textureSample(tree_diffuse, tree_sampler, in.uv);
    // Cutout families use the current LOD's reference; opaque bark keeps every texel.
    {alpha_test}
    {normal_value}
    let ambient = {ambient_value};
    let base = select(ambient, floor(clamp(diffuse.rgb, vec3<f32>(0.0), vec3<f32>(1.0)) * 255.0 + 0.5) / 255.0 * quantizeToF16(ambient), tree_frame.fixup.z > 0.5);
    return TreeBuffer(vec4<f32>(diffuse.rgb, 64.0 / 255.0), vec4<f32>(0.0),
        vec4<f32>(tree_oct_encode(normal), 0.0, 1.0), vec4<f32>(base, 1.0));
}}
"#,
        sampler = diffuse + 1
    ))
}

impl Plan {
    pub(super) fn new(scene: &AreaScene, d3d: &D3dPass, takes: &mut [bool]) -> Self {
        let mut plan = Self::default();
        let mut variants = HashMap::new();
        for (k, d) in d3d.draws.iter().enumerate() {
            let variant = variants.entry(d.pipeline).or_insert_with(|| {
                let program = d3d.program(d);
                let family = Family::of(program)?;
                if !family.enabled() {
                    return None;
                }
                let source = source(program)?;
                // The faithful instance group may contain several placements. Keep
                // the complete pipeline forward if any member needs unsupported data.
                if d3d
                    .draws
                    .iter()
                    .filter(|other| other.pipeline == d.pipeline)
                    .any(|other| {
                        other.multipass
                            || scene
                                .draws
                                .get(other.draw)
                                .is_none_or(|item| item.room_id.is_some() || item.skin.is_some())
                    })
                {
                    return None;
                }
                let attributes = program
                    .vs
                    .interface
                    .inputs
                    .iter()
                    .enumerate()
                    .map(|(i, s)| wgpu::VertexAttribute {
                        format: wgpu::VertexFormat::Float32x4,
                        offset: 16 * i as u64,
                        shader_location: s.location,
                    })
                    .collect();
                let variant = plan.variants.len();
                plan.variants.push(Variant {
                    family,
                    source,
                    layouts: d3d.geometry_layouts(d),
                    attributes,
                    cull: match program.pass.cull {
                        Cull::None => None,
                        Cull::Cw => Some(wgpu::Face::Back),
                        Cull::Ccw => Some(wgpu::Face::Front),
                    },
                });
                Some(variant)
            });
            if let Some(variant) = *variant {
                takes[k] = true;
                plan.draws.push((k, variant));
            }
        }
        plan
    }

    pub(super) fn is_empty(&self) -> bool {
        self.draws.is_empty()
    }

    pub(super) fn len(&self) -> usize {
        self.draws.len()
    }

    pub(super) fn prepare(&mut self, d3d: &D3dPass, takes: &mut [bool]) {
        // Faded instances share their group's submission. Falling back together
        // preserves the faithful opacity and ordering of every member.
        self.active = self.enabled.map(|enabled| enabled.unwrap_or(true));
        for &(k, variant) in &self.draws {
            if d3d.is_faded(&d3d.draws[k]) {
                self.active[self.variants[variant].family as usize] = false;
            }
        }
        for &(k, variant) in &self.draws {
            takes[k] = self.active[self.variants[variant].family as usize];
        }
    }

    pub(super) fn set_enabled(&mut self, family: Family, enabled: Option<bool>) {
        self.enabled[family as usize] = enabled;
    }

    pub(super) fn build(&self, device: &wgpu::Device, frame: &wgpu::Buffer) -> Gpu {
        let frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("deferred tree frame"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(
                        std::mem::size_of::<FrameUniforms>() as u64
                    ),
                },
                count: None,
            }],
        });
        let frame = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("deferred tree frame"),
            layout: &frame_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: frame.as_entire_binding(),
            }],
        });
        let pipelines = self
            .variants
            .iter()
            .map(|variant| {
                let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some("deferred trees"),
                    source: wgpu::ShaderSource::Wgsl(variant.source.clone().into()),
                });
                let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("deferred trees"),
                    bind_group_layouts: &[
                        Some(&variant.layouts[0]),
                        Some(&variant.layouts[1]),
                        Some(&frame_layout),
                    ],
                    immediate_size: 0,
                });
                let targets: Vec<_> = GBUFFER
                    .iter()
                    .map(|&format| {
                        Some(wgpu::ColorTargetState {
                            format,
                            blend: None,
                            write_mask: wgpu::ColorWrites::ALL,
                        })
                    })
                    .collect();
                device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("deferred trees"),
                    layout: Some(&layout),
                    vertex: wgpu::VertexState {
                        module: &shader,
                        entry_point: Some("vs_tree"),
                        compilation_options: Default::default(),
                        buffers: &[Some(wgpu::VertexBufferLayout {
                            array_stride: (16 * variant.attributes.len()) as u64,
                            step_mode: wgpu::VertexStepMode::Vertex,
                            attributes: &variant.attributes,
                        })],
                    },
                    primitive: wgpu::PrimitiveState {
                        cull_mode: variant.cull,
                        ..Default::default()
                    },
                    depth_stencil: Some(wgpu::DepthStencilState {
                        format: crate::gpu::DEPTH_FORMAT,
                        depth_write_enabled: Some(true),
                        depth_compare: Some(wgpu::CompareFunction::LessEqual),
                        stencil: Default::default(),
                        bias: Default::default(),
                    }),
                    multisample: Default::default(),
                    fragment: Some(wgpu::FragmentState {
                        module: &shader,
                        entry_point: Some("fs_tree"),
                        compilation_options: Default::default(),
                        targets: &targets,
                    }),
                    multiview_mask: None,
                    cache: None,
                })
            })
            .collect();
        Gpu { pipelines, frame }
    }

    pub(super) fn draw(
        &self,
        gpu: &Gpu,
        pass: &mut wgpu::RenderPass<'_>,
        d3d: &D3dPass,
        meshes: &[GpuMesh],
    ) {
        if !self.active.iter().any(|active| *active) {
            return;
        }
        pass.set_bind_group(2, &gpu.frame, &[]);
        for &(k, variant) in &self.draws {
            if !self.active[self.variants[variant].family as usize] {
                continue;
            }
            let d = &d3d.draws[k];
            if !d3d.in_view(k) || d3d.is_faded(d) {
                continue;
            }
            let Some(mesh) = meshes.get(d.mesh) else {
                continue;
            };
            let Some(geometry) = d3d.geometry(d, mesh.count) else {
                continue;
            };
            pass.set_pipeline(&gpu.pipelines[variant]);
            pass.set_bind_group(0, geometry.constants, &[]);
            pass.set_bind_group(1, geometry.textures, &[]);
            pass.set_vertex_buffer(0, geometry.vertices.slice(..));
            pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(geometry.indices, 0, geometry.instances);
        }
    }
}
