//! Point-shadow trees use their translated vertex shader with an identity camera
//! matrix. A private uniform stream includes offscreen placements and never edits
//! the faithful pass's camera-visible instance list or constant buffers.

use std::{collections::HashMap, ops::Range, sync::Arc};

use glam::Vec3;
use thedas_shader::{bytecode::Usage, translate};

use super::trees::{self, Family};
use crate::{camera::Camera, d3d::D3dProgram, d3d_gpu::D3dPass, gpu::GpuMesh, scene::AreaScene};

const BYTES: usize =
    ((translate::FLOAT_CONSTANTS + translate::INT_CONSTANTS + translate::BOOL_CONSTANTS) * 16 + 16)
        as usize;
const STRIDE: usize = BYTES.next_multiple_of(256);

struct Variant {
    source: String,
    textures: wgpu::BindGroupLayout,
    attributes: Vec<wgpu::VertexAttribute>,
}

#[derive(Default)]
pub(super) struct Plan {
    variants: Vec<Variant>,
    draws: Vec<(usize, usize)>,
}

pub(super) struct Gpu {
    pipelines: Vec<wgpu::RenderPipeline>,
    layout: wgpu::BindGroupLayout,
    buffer: wgpu::Buffer,
    group: wgpu::BindGroup,
    capacity: usize,
    previous: Vec<u8>,
    logged: bool,
}

/// Immutable identities and exact deformed-shape inputs to the shadow cache.
#[derive(Clone, PartialEq)]
pub(crate) struct Inputs {
    pipeline: wgpu::RenderPipeline,
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    textures: wgpu::BindGroup,
    range: Range<u32>,
    constants: Arc<[u8]>,
    pub(crate) sphere: (Vec3, f32),
}

pub(crate) struct Caster {
    pub(crate) inputs: Inputs,
    group: wgpu::BindGroup,
    offset: u32,
}

impl Caster {
    pub(crate) fn draw(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        face: &wgpu::BindGroup,
        offset: u32,
    ) {
        pass.set_pipeline(&self.inputs.pipeline);
        pass.set_bind_group(0, &self.group, &[self.offset]);
        pass.set_bind_group(1, &self.inputs.textures, &[]);
        pass.set_bind_group(2, face, &[offset]);
        pass.set_vertex_buffer(0, self.inputs.vertices.slice(..));
        pass.set_index_buffer(self.inputs.indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(self.inputs.range.clone(), 0, 0..1);
    }
}

fn source(program: &D3dProgram) -> Option<String> {
    // Match the same supported states and vertex interfaces as G-buffer trees.
    trees::source(program)?;
    let family = Family::of(program)?;
    let uv = program
        .vs
        .interface
        .outputs
        .iter()
        .find(|slot| {
            slot.semantic.usage == Usage::TexCoord
                && slot.semantic.index == if family == Family::Branches { 3 } else { 0 }
        })?
        .register;
    let diffuse = program.ps.ctab.as_ref()?.get("smpDiffuseMap")?.register * 2;
    let mut vertex = program.vs.wgsl.replacen("@vertex\n", "", 1);
    // The translator owns these declarations. Collapse either constant layout to
    // one portable uniform slot; shadow draws have one placement each, not the
    // camera-visible storage-buffer instance list.
    vertex = vertex
        .lines()
        .filter(|line| !(line.contains("@group(0)") && line.contains("var<")))
        .collect::<Vec<_>>()
        .join("\n");
    for (from, to) in [
        (
            "d3d_inst = d3d_instances[input.d3d_instance];",
            "d3d_inst = 0u;",
        ),
        ("d3d_slots[d3d_inst].vf[", "tree_slot.vf["),
        ("d3d_slots[d3d_inst].vi[", "tree_slot.vi["),
        ("d3d_slots[d3d_inst].vb[", "tree_slot.vb["),
        ("d3d_slots[d3d_inst].fx", "vec4<f32>(0.0)"),
        ("cf.c[", "tree_slot.vf["),
        ("ci_u.c[", "tree_slot.vi["),
        ("cb_u.c[", "tree_slot.vb["),
        ("d3d_fixup", "vec4<f32>(0.0)"),
    ] {
        vertex = vertex.replace(from, to);
    }
    let alpha = if family == Family::Branches {
        ""
    } else {
        "let alpha = textureSample(tree_diffuse, tree_sampler, in.uv).a;\n    if (!tree_alpha_kept(alpha, tree_slot.alpha.w, tree_slot.alpha.x)) { discard; }"
    };
    Some(format!(
        r#"{vertex}
{alpha_wgsl}
struct TreeShadowSlot {{
    vf: array<vec4<f32>, {floats}>,
    vi: array<vec4<i32>, {ints}>,
    vb: array<vec4<u32>, {bools}>,
    alpha: vec4<f32>,
}}
@group(0) @binding(0) var<uniform> tree_slot: TreeShadowSlot;
@group(1) @binding({diffuse}) var tree_diffuse: texture_2d<f32>;
@group(1) @binding({sampler}) var tree_sampler: sampler;
struct TreeShadowFace {{ view_proj: mat4x4<f32>, light: vec4<f32> }}
@group(2) @binding(0) var<uniform> tree_face: TreeShadowFace;
struct TreeShadowVertex {{
    @builtin(position) position: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) uv: vec2<f32>,
}}
@vertex fn vs_tree_shadow(input: StageIn) -> TreeShadowVertex {{
    let original = main(input);
    return TreeShadowVertex(tree_face.view_proj * original.position,
        original.position.xyz / original.position.w, original.o{uv}.xy);
}}
@fragment fn fs_tree_shadow(in: TreeShadowVertex) -> @location(0) vec4<f32> {{
    {alpha}
    return vec4<f32>(length(in.world - tree_face.light.xyz), 0.0, 0.0, 1.0);
}}
"#,
        alpha_wgsl = trees::ALPHA_WGSL,
        floats = translate::FLOAT_CONSTANTS,
        ints = translate::INT_CONSTANTS,
        bools = translate::BOOL_CONSTANTS,
        sampler = diffuse + 1,
    ))
}

impl Plan {
    pub(super) fn new(scene: &AreaScene, d3d: &D3dPass) -> Self {
        let mut plan = Self::default();
        if !crate::config::CONFIG.tree_shadows {
            return plan;
        }
        let mut variants = HashMap::new();
        for (k, draw) in d3d.draws.iter().enumerate() {
            let Some(item) = scene.draws.get(draw.draw) else {
                continue;
            };
            if item.room_id.is_some()
                || item.skin.is_some()
                || draw.multipass
                || scene.cut_away.contains(&draw.draw)
            {
                continue;
            }
            // Scene trees opt out of the faithful sun casters. Point-tree shadows
            // are our separate renderer feature, including those same placements.
            let variant = variants.entry(draw.pipeline).or_insert_with(|| {
                let program = d3d.program(draw);
                let source = source(program)?;
                let index = plan.variants.len();
                plan.variants.push(Variant {
                    source,
                    textures: d3d.geometry_layouts(draw)[1].clone(),
                    attributes: program
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
                        .collect(),
                });
                Some(index)
            });
            if let Some(variant) = *variant {
                plan.draws.push((k, variant));
            }
        }
        log::info!(
            "deferred: {} planned tree point-shadow casters",
            plan.draws.len()
        );
        plan
    }

    pub(super) fn build(&self, device: &wgpu::Device) -> Gpu {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("tree shadow constants"),
            entries: &[uniform(0, BYTES as u64)],
        });
        let face = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("tree shadow face"),
            entries: &[uniform(0, 80)],
        });
        let pipelines = self
            .variants
            .iter()
            .map(|variant| {
                let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some("tree point shadows"),
                    source: wgpu::ShaderSource::Wgsl(variant.source.clone().into()),
                });
                let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("tree point shadows"),
                    bind_group_layouts: &[Some(&layout), Some(&variant.textures), Some(&face)],
                    immediate_size: 0,
                });
                device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("tree point shadows"),
                    layout: Some(&layout),
                    vertex: wgpu::VertexState {
                        module: &shader,
                        entry_point: Some("vs_tree_shadow"),
                        compilation_options: Default::default(),
                        buffers: &[Some(wgpu::VertexBufferLayout {
                            array_stride: 16 * variant.attributes.len() as u64,
                            step_mode: wgpu::VertexStepMode::Vertex,
                            attributes: &variant.attributes,
                        })],
                    },
                    // Both sides block point lights, as with the other point casters.
                    primitive: Default::default(),
                    depth_stencil: Some(wgpu::DepthStencilState {
                        format: crate::gpu::DEPTH_FORMAT,
                        depth_write_enabled: Some(true),
                        depth_compare: Some(wgpu::CompareFunction::Less),
                        stencil: Default::default(),
                        bias: Default::default(),
                    }),
                    multisample: Default::default(),
                    fragment: Some(wgpu::FragmentState {
                        module: &shader,
                        entry_point: Some("fs_tree_shadow"),
                        compilation_options: Default::default(),
                        targets: &[Some(wgpu::ColorTargetState {
                            format: wgpu::TextureFormat::R32Float,
                            blend: None,
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                    }),
                    multiview_mask: None,
                    cache: None,
                })
            })
            .collect();
        let buffer = buffer(device, 1);
        let group = group(device, &layout, &buffer);
        Gpu {
            pipelines,
            layout,
            buffer,
            group,
            capacity: 1,
            previous: Vec::new(),
            logged: false,
        }
    }

    pub(super) fn casters(
        &self,
        gpu: &mut Gpu,
        ctx: &crate::gpu::GpuContext,
        d3d: &D3dPass,
        meshes: &[GpuMesh],
        camera: &Camera,
        lights: &[(Vec3, f32)],
    ) -> Vec<Caster> {
        let mut data = Vec::new();
        let mut result = Vec::new();
        let mut offscreen = 0;
        let winds = d3d.tree_shadow_winds();
        for &(k, variant) in &self.draws {
            let draw = &d3d.draws[k];
            let sphere = d3d.draw_sphere(draw).unwrap_or((Vec3::ZERO, f32::INFINITY));
            if !lights.iter().any(|&(p, r)| reaches(sphere, p, r)) {
                continue;
            }
            let Some(mesh) = meshes.get(draw.mesh) else {
                continue;
            };
            let Some(geometry) = d3d.tree_shadow_geometry(draw, mesh.count, camera) else {
                continue;
            };
            let Some((constants, reference)) = d3d.tree_shadow_constants(draw, camera, &winds)
            else {
                continue;
            };
            if crate::config::CONFIG.log_lights && !gpu.logged && !d3d.in_view(k) {
                offscreen += 1;
            }
            let mut bytes = Vec::with_capacity(STRIDE);
            bytes.extend_from_slice(bytemuck::cast_slice(&constants.floats));
            bytes.extend_from_slice(bytemuck::cast_slice(&constants.ints));
            bytes.extend_from_slice(bytemuck::cast_slice(&constants.bools));
            bytes.extend_from_slice(bytemuck::cast_slice(&[reference, 0.0, 0.0, 1.0f32]));
            debug_assert_eq!(bytes.len(), BYTES);
            result.push((
                Inputs {
                    pipeline: gpu.pipelines[variant].clone(),
                    vertices: geometry.vertices.clone(),
                    indices: mesh.indices.clone(),
                    textures: geometry.textures.clone(),
                    range: geometry.indices,
                    constants: Arc::from(bytes.as_slice()),
                    sphere,
                },
                data.len() as u32,
            ));
            data.extend_from_slice(&bytes);
            data.resize(data.len().next_multiple_of(STRIDE), 0);
        }
        if result.len() > gpu.capacity {
            gpu.capacity = result.len().next_power_of_two();
            gpu.buffer = buffer(&ctx.device, gpu.capacity);
            gpu.group = group(&ctx.device, &gpu.layout, &gpu.buffer);
        }
        if data != gpu.previous {
            if !data.is_empty() {
                ctx.queue.write_buffer(&gpu.buffer, 0, &data);
            }
            gpu.previous = data;
        }
        if crate::config::CONFIG.log_lights && !gpu.logged {
            log::info!(
                "deferred tree shadows: {} active placements ({} offscreen)",
                result.len(),
                offscreen
            );
            gpu.logged = true;
        }
        result
            .into_iter()
            .map(|(inputs, offset)| Caster {
                inputs,
                offset,
                group: gpu.group.clone(),
            })
            .collect()
    }
}

pub(crate) fn reaches((centre, radius): (Vec3, f32), light: Vec3, reach: f32) -> bool {
    !centre.is_finite()
        || !radius.is_finite()
        || radius < 0.0
        || centre.distance(light) < radius + reach
}

pub(crate) fn uniform(binding: u32, size: u64) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: true,
            min_binding_size: wgpu::BufferSize::new(size),
        },
        count: None,
    }
}

fn buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("tree shadow constants"),
        size: (STRIDE * capacity) as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    buffer: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("tree shadow constants"),
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer,
                offset: 0,
                size: wgpu::BufferSize::new(BYTES as u64),
            }),
        }],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_and_instanced_tree_shadow_wrappers_are_portable() {
        for family in [
            Family::Fronds,
            Family::Branches,
            Family::MeshLeaves,
            Family::LeafCards,
        ] {
            for instanced in [false, true] {
                let program = trees::tests::program(family, instanced);
                let shader = source(&program).unwrap();
                crate::backend_check::translates("tree point shadows", &shader);
                assert!(!shader.contains("var<storage"));
                assert!(!shader.contains("d3d_instances"));
                assert!(!shader.contains("d3d_slots["));
            }
        }
    }

    #[test]
    fn tree_cubes_keep_cutouts_wind_ranges_and_cache_invalidation() {
        use wgpu::util::DeviceExt;
        let ctx = crate::gpu::GpuContext::headless().unwrap();
        let device = &ctx.device;
        for family in [
            Family::Fronds,
            Family::Branches,
            Family::MeshLeaves,
            Family::LeafCards,
        ] {
            for instanced in [false, true] {
                let mut program = trees::tests::program(family, instanced);
                program.vs.wgsl = program
                    .vs
                    .wgsl
                    .replace(
                        "return StageOut(input.v0,",
                        "return StageOut(vec4<f32>(input.v0.xyz + cf.c[0].xyz, 1.0),",
                    )
                    .replace("vec4<f32>(1.0, 0.0, 0.0, 0.0)", "input.v1")
                    .replace("vec4<f32>(0.5)", "input.v1");
                let textures = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                    label: None,
                    entries: &[
                        wgpu::BindGroupLayoutEntry {
                            binding: 0,
                            visibility: wgpu::ShaderStages::FRAGMENT,
                            ty: wgpu::BindingType::Texture {
                                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                                view_dimension: wgpu::TextureViewDimension::D2,
                                multisampled: false,
                            },
                            count: None,
                        },
                        wgpu::BindGroupLayoutEntry {
                            binding: 1,
                            visibility: wgpu::ShaderStages::FRAGMENT,
                            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                            count: None,
                        },
                    ],
                });
                let plan = Plan {
                    variants: vec![Variant {
                        source: source(&program).unwrap(),
                        textures: textures.clone(),
                        attributes: vec![
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x4,
                                offset: 0,
                                shader_location: 0,
                            },
                            wgpu::VertexAttribute {
                                format: wgpu::VertexFormat::Float32x4,
                                offset: 16,
                                shader_location: 1,
                            },
                        ],
                    }],
                    draws: Vec::new(),
                };
                let gpu = plan.build(device);
                let texture = device.create_texture(&wgpu::TextureDescriptor {
                    label: None,
                    size: wgpu::Extent3d {
                        width: 2,
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
                    &[0, 0, 0, 0, 0, 0, 0, 128],
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(8),
                        rows_per_image: Some(1),
                    },
                    texture.size(),
                );
                let textures = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: None,
                    layout: &textures,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(
                                &texture.create_view(&Default::default()),
                            ),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(
                                &device.create_sampler(&Default::default()),
                            ),
                        },
                    ],
                });
                let vertex = |y, z, u, v| [2.0f32, y, z, 1.0, u, v, 0.0, 0.0];
                let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: None,
                    usage: wgpu::BufferUsages::VERTEX,
                    contents: bytemuck::cast_slice(&[
                        vertex(0.75, 0.75, 0.0, 0.0),
                        vertex(-0.75, 0.75, 1.0, 0.0),
                        vertex(-0.75, -0.75, 1.0, 1.0),
                        vertex(0.75, -0.75, 0.0, 1.0),
                    ]),
                });
                let indices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: None,
                    usage: wgpu::BufferUsages::INDEX,
                    contents: bytemuck::cast_slice(&[0u32, 1, 2, 0, 2, 3]),
                });
                let mut pool = crate::deferred_shadow::ShadowPool::new(device);
                let read = |pool: &mut crate::deferred_shadow::ShadowPool,
                            wind: f32,
                            reference: f32,
                            range: Range<u32>,
                            present: bool| {
                    let mut constants = vec![0u8; BYTES];
                    constants[4..8].copy_from_slice(&wind.to_le_bytes());
                    constants[BYTES - 16..]
                        .copy_from_slice(bytemuck::cast_slice(&[reference, 0.0, 0.0, 1.0f32]));
                    ctx.queue.write_buffer(&gpu.buffer, 0, &constants);
                    let caster = Caster {
                        inputs: Inputs {
                            pipeline: gpu.pipelines[0].clone(),
                            vertices: vertices.clone(),
                            indices: indices.clone(),
                            textures: textures.clone(),
                            range,
                            constants: Arc::from(constants),
                            sphere: (Vec3::new(2.0, 0.0, 0.0), 3.0),
                        },
                        group: gpu.group.clone(),
                        offset: 0,
                    };
                    let readback = device.create_buffer(&wgpu::BufferDescriptor {
                        label: None,
                        size: 512,
                        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                        mapped_at_creation: false,
                    });
                    let mut encoder = device.create_command_encoder(&Default::default());
                    let casters = [caster];
                    pool.encode(
                        device,
                        &ctx.queue,
                        &mut encoder,
                        &[(Vec3::ZERO, 10.0)],
                        &[],
                        if present { &casters } else { &[] },
                    );
                    for (i, x) in [210, 300].into_iter().enumerate() {
                        encoder.copy_texture_to_buffer(
                            wgpu::TexelCopyTextureInfo {
                                texture: &pool.texture,
                                mip_level: 0,
                                origin: wgpu::Origin3d {
                                    x,
                                    y: crate::deferred_shadow::SIZE / 2,
                                    z: 0,
                                },
                                aspect: wgpu::TextureAspect::All,
                            },
                            wgpu::TexelCopyBufferInfo {
                                buffer: &readback,
                                layout: wgpu::TexelCopyBufferLayout {
                                    offset: i as u64 * 256,
                                    bytes_per_row: Some(256),
                                    rows_per_image: Some(1),
                                },
                            },
                            wgpu::Extent3d {
                                width: 1,
                                height: 1,
                                depth_or_array_layers: 1,
                            },
                        );
                    }
                    ctx.queue.submit([encoder.finish()]);
                    let (tx, rx) = std::sync::mpsc::channel();
                    readback
                        .slice(..)
                        .map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
                    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
                    rx.recv().unwrap().unwrap();
                    let bytes = readback.slice(..).get_mapped_range().unwrap();
                    [0, 256].map(|i| *bytemuck::from_bytes::<f32>(&bytes[i..i + 4]))
                };
                let expected = if family == Family::Branches {
                    [true, true]
                } else {
                    [false, true]
                };
                let sample = read(&mut pool, 0.0, 127.0, 0..6, true);
                assert_eq!(
                    sample.map(|d| d < 10.0),
                    expected,
                    "{family:?} instanced={instanced}"
                );
                assert!(
                    (sample[1] - 2.03).abs() < 0.05,
                    "radial world distance: {sample:?}"
                );
                let rendered = pool.rendered_cubes;
                assert_eq!(read(&mut pool, 0.0, 127.0, 0..6, true), sample);
                assert_eq!(
                    pool.rendered_cubes, rendered,
                    "identical shape should reuse"
                );
                assert_eq!(
                    read(&mut pool, 0.0, 128.0, 0..6, true).map(|d| d < 10.0),
                    if family == Family::Branches {
                        [true, true]
                    } else {
                        [false, false]
                    }
                );
                assert_eq!(
                    pool.rendered_cubes,
                    rendered + 1,
                    "LOD alpha must invalidate"
                );
                let moved = read(&mut pool, 1.0, 127.0, 0..6, true);
                assert_eq!(
                    moved.map(|d| d < 10.0),
                    [true, false],
                    "wind moves the shadow"
                );
                assert_eq!(pool.rendered_cubes, rendered + 2, "wind must invalidate");
                assert_eq!(
                    read(&mut pool, 0.0, 127.0, 0..0, true).map(|d| d < 10.0),
                    [false, false]
                );
                assert_eq!(
                    read(&mut pool, 0.0, 127.0, 0..6, true),
                    sample,
                    "restored LOD range"
                );
                assert_eq!(
                    read(&mut pool, 0.0, 127.0, 0..6, false).map(|d| d < 10.0),
                    [false, false],
                    "removed or faded tree clears"
                );
                assert_eq!(read(&mut pool, 0.0, 127.0, 0..6, true), sample);
                pool.reuse_override = Some(false);
                assert_eq!(
                    read(&mut pool, 0.0, 127.0, 0..6, true),
                    sample,
                    "uncached cube matches"
                );
            }
        }
    }
}
