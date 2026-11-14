//! Shadows for the deferred path's point lights (ours: the original shadows only its sun).
//!
//! A pool of [`SLOTS`] cube shadow maps, given each frame to the dynamic lights that matter
//! most (the glowing emitters', which have no baked visibility in the lightmaps; see
//! [`crate::deferred`]). Each face is a 90° view from the light, its texels the distance to
//! the nearest caster (`R32Float`); the lighting pass picks a point's face by the major axis
//! of its direction from the light and compares distances with a bilinear 2×2 filter.
//!
//! Casters are eligible deferred level, terrain and object draws, including skinned ones.
//! Forward materials and trees do not cast these shadows yet.

use glam::{Mat4, Vec3};

use crate::deferred_mesh::{DeferredVertex, ROWS};
use crate::gpu::{DEPTH_FORMAT, GpuMesh};

pub const SHADOW_WGSL: &str = concat!(
    include_str!("deferred_skin.wgsl"),
    include_str!("deferred_shadow.wgsl")
);

/// Shadowed lights per frame.
pub(crate) const SLOTS: usize = 4;
/// Texels on a side of a cube face.
pub(crate) const SIZE: u32 = 512;
/// The faces' near plane, metres. Geometry nearer the light than this casts nothing: a
/// flame's light sits inside its torch's bowl or brazier, which would otherwise shadow
/// everything below it (provisional, by eye on the Tower of Ishal's torches).
const NEAR: f32 = 0.35;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R32Float;
/// Dynamic uniform offsets must be multiples of this on every backend.
const STRIDE: usize = 256;

/// The cube's faces, in the order the lighting shader picks them: +X, −X, +Y, −Y, +Z, −Z
/// (each with an up that is not along it).
const FACES: [(Vec3, Vec3); 6] = [
    (Vec3::X, Vec3::Z),
    (Vec3::NEG_X, Vec3::Z),
    (Vec3::Y, Vec3::Z),
    (Vec3::NEG_Y, Vec3::Z),
    (Vec3::Z, Vec3::Y),
    (Vec3::NEG_Z, Vec3::Y),
];

/// The view-projections of a light's six faces, reaching `radius` from `position`.
pub(crate) fn faces(position: Vec3, radius: f32) -> [Mat4; 6] {
    let projection = Mat4::perspective_rh(
        std::f32::consts::FRAC_PI_2,
        1.0,
        NEAR,
        radius.max(NEAR * 2.0),
    );
    FACES.map(|(forward, up)| projection * Mat4::look_to_rh(position, forward, up))
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct DrawUniforms {
    mvp: [[f32; 4]; 4],
    world: [[f32; 4]; 4],
    light: [f32; 4],
    alpha: [f32; 4],
    clip: [f32; 4],
}

/// One caster: its mesh, model-to-world and world bounding sphere.
pub(crate) struct Caster<'a> {
    pub mesh: &'a GpuMesh,
    pub vertices: &'a wgpu::Buffer,
    pub bones: &'a wgpu::Buffer,
    pub bone_offset: u32,
    /// Current palette values; buffer identity alone cannot detect an in-place update.
    pub pose: &'a [[f32; 4]],
    pub world: Mat4,
    pub sphere: (Vec3, f32),
    pub alpha: [f32; 4],
    pub clip: f32,
    pub material: Option<&'a crate::deferred_cutout::Material>,
}

/// Exact inputs to a cube, independent of the viewing camera. Material textures and
/// mesh buffers are immutable for a renderer's lifetime; reloads create new handles.
#[derive(PartialEq)]
struct CubeInputs {
    light: (Vec3, f32),
    casters: Vec<CasterInputs>,
}

#[derive(PartialEq)]
struct CasterInputs {
    indices: wgpu::Buffer,
    vertices: wgpu::Buffer,
    count: u32,
    bones: wgpu::Buffer,
    bone_offset: u32,
    pose: Vec<[f32; 4]>,
    world: Mat4,
    sphere: (Vec3, f32),
    alpha: [f32; 4],
    clip: f32,
    material: Option<(wgpu::TextureView, wgpu::Sampler)>,
}

impl CasterInputs {
    fn new(c: &Caster<'_>) -> Self {
        Self {
            indices: c.mesh.indices.clone(),
            vertices: c.vertices.clone(),
            count: c.mesh.count,
            bones: c.bones.clone(),
            bone_offset: c.bone_offset,
            pose: c.pose.to_vec(),
            world: c.world,
            sphere: c.sphere,
            alpha: c.alpha,
            clip: c.clip,
            material: c.material.map(|m| (m.view.clone(), m.sampler.clone())),
        }
    }
}

pub(crate) struct ShadowPool {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    buffer: wgpu::Buffer,
    /// Slots of [`STRIDE`] the buffer holds.
    capacity: usize,
    material_layout: wgpu::BindGroupLayout,
    opaque: crate::deferred_cutout::Material,
    previous: Vec<CubeInputs>,
    #[cfg(test)]
    rendered_cubes: usize,
    /// Per layer (slot · 6 + face), the colour and depth views drawn into.
    layers: Vec<(wgpu::TextureView, wgpu::TextureView)>,
    /// Every layer, for the lighting pass.
    pub(crate) view: wgpu::TextureView,
    #[cfg(test)]
    texture: wgpu::Texture,
}

impl ShadowPool {
    pub(crate) fn new(device: &wgpu::Device) -> ShadowPool {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("deferred shadows"),
            source: wgpu::ShaderSource::Wgsl(SHADOW_WGSL.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("deferred shadows"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let material_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("deferred shadow opacity"),
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
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("deferred opaque shadow binding"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let opaque = crate::deferred_cutout::Material {
            view: texture.create_view(&Default::default()),
            sampler: device.create_sampler(&Default::default()),
        };
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("deferred shadows"),
            bind_group_layouts: &[Some(&layout), Some(&material_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("deferred shadows"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_shadow"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<DeferredVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x2,
                            offset: 24,
                            shader_location: 2,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x3,
                            offset: 0,
                            shader_location: 0,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x4,
                            offset: 40,
                            shader_location: 4,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x4,
                            offset: 56,
                            shader_location: 5,
                        },
                    ],
                })],
            },
            // Both faces cast: walls seen from behind still block.
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_shadow"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let texture = |label, format, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: SIZE,
                    height: SIZE,
                    depth_or_array_layers: (SLOTS * 6) as u32,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let colour = texture(
            "deferred shadow distances",
            FORMAT,
            wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
        );
        let depth = texture(
            "deferred shadow depth",
            DEPTH_FORMAT,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
        );
        let layer = |t: &wgpu::Texture, i: u32| {
            t.create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2),
                base_array_layer: i,
                array_layer_count: Some(1),
                ..Default::default()
            })
        };
        let layers = (0..(SLOTS * 6) as u32)
            .map(|i| (layer(&colour, i), layer(&depth, i)))
            .collect();
        let view = colour.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        });
        let buffer = Self::slots(device, 1);
        ShadowPool {
            pipeline,
            layout,
            buffer,
            capacity: 1,
            material_layout,
            opaque,
            previous: Vec::new(),
            #[cfg(test)]
            rendered_cubes: 0,
            layers,
            view,
            #[cfg(test)]
            texture: colour,
        }
    }

    fn slots(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("deferred shadow draws"),
            size: (STRIDE * capacity) as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    fn group(&self, device: &wgpu::Device, bones: &wgpu::Buffer) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("deferred shadow draws"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &self.buffer,
                        offset: 0,
                        size: wgpu::BufferSize::new(std::mem::size_of::<DrawUniforms>() as u64),
                    }),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: bones,
                        offset: 0,
                        size: wgpu::BufferSize::new((ROWS * 16) as u64),
                    }),
                },
            ],
        })
    }

    /// Records the shadow maps of `lights` (position, radius; one slot each, at most
    /// [`SLOTS`]) cast by `casters`.
    pub(crate) fn encode(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        lights: &[(Vec3, f32)],
        casters: &[Caster<'_>],
    ) {
        let reuse = std::env::var("THEDAS_DEFERRED_SHADOW_REUSE").as_deref() != Ok("0");
        let inputs: Vec<_> = if reuse {
            lights
                .iter()
                .take(SLOTS)
                .map(|&(position, radius)| CubeInputs {
                    light: (position, radius),
                    casters: casters
                        .iter()
                        .filter(|c| c.sphere.0.distance(position) < c.sphere.1 + radius)
                        .map(CasterInputs::new)
                        .collect(),
                })
                .collect()
        } else {
            Vec::new()
        };
        let dirty: Vec<_> = (0..lights.len().min(SLOTS))
            .map(|i| !reuse || self.previous.get(i) != inputs.get(i))
            .collect();
        self.previous = inputs;
        if !dirty.iter().any(|d| *d) {
            return;
        }
        #[cfg(test)]
        {
            self.rendered_cubes += dirty.iter().filter(|d| **d).count();
        }
        // Per layer, its draws: (caster, slot in the buffer).
        let mut data: Vec<u8> = Vec::new();
        let mut layers: Vec<Vec<(usize, u32)>> = Vec::new();
        for (slot, &(position, radius)) in lights.iter().take(SLOTS).enumerate() {
            if !dirty[slot] {
                layers.extend((0..6).map(|_| Vec::new()));
                continue;
            }
            let near: Vec<usize> = casters
                .iter()
                .enumerate()
                .filter(|(_, c)| c.sphere.0.distance(position) < c.sphere.1 + radius)
                .map(|(i, _)| i)
                .collect();
            for view_proj in faces(position, radius) {
                let mut draws = Vec::with_capacity(near.len());
                for &i in &near {
                    let c = &casters[i];
                    let u = DrawUniforms {
                        mvp: (view_proj * c.world).to_cols_array_2d(),
                        world: c.world.to_cols_array_2d(),
                        light: position.extend(radius).to_array(),
                        alpha: c.alpha,
                        clip: [c.clip, 0.0, 0.0, 0.0],
                    };
                    draws.push((i, (data.len() / STRIDE) as u32));
                    data.extend_from_slice(bytemuck::bytes_of(&u));
                    data.resize(data.len().next_multiple_of(STRIDE), 0);
                }
                layers.push(draws);
            }
        }
        if !data.is_empty() {
            let need = data.len() / STRIDE;
            if need > self.capacity {
                let capacity = need.next_power_of_two();
                self.buffer = Self::slots(device, capacity);
                self.capacity = capacity;
            }
            queue.write_buffer(&self.buffer, 0, &data);
        }
        let mut groups = Vec::<(&wgpu::Buffer, wgpu::BindGroup)>::new();
        let caster_groups: Vec<usize> = casters
            .iter()
            .map(|c| {
                if let Some(i) = groups.iter().position(|(b, _)| *b == c.bones) {
                    return i;
                }
                groups.push((c.bones, self.group(device, c.bones)));
                groups.len() - 1
            })
            .collect();
        let mut materials = Vec::<(&crate::deferred_cutout::Material, wgpu::BindGroup)>::new();
        let caster_materials: Vec<usize> = casters
            .iter()
            .map(|c| {
                let material = c.material.unwrap_or(&self.opaque);
                if let Some(i) = materials
                    .iter()
                    .position(|(m, _)| std::ptr::eq(*m, material))
                {
                    return i;
                }
                let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("deferred shadow opacity"),
                    layout: &self.material_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(&material.view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(&material.sampler),
                        },
                    ],
                });
                materials.push((material, group));
                materials.len() - 1
            })
            .collect();
        for (layer, draws) in layers.iter().enumerate() {
            if !dirty[layer / 6] {
                continue;
            }
            let Some((colour, depth)) = self.layers.get(layer) else {
                continue;
            };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("deferred shadow face"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: colour,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        // Nothing in the way: as far as can be.
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 1.0e9,
                            g: 0.0,
                            b: 0.0,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipeline);
            for &(i, slot) in draws {
                let c = &casters[i];
                let mesh = c.mesh;
                pass.set_bind_group(
                    0,
                    &groups[caster_groups[i]].1,
                    &[slot * STRIDE as u32, c.bone_offset],
                );
                pass.set_bind_group(1, &materials[caster_materials[i]].1, &[]);
                pass.set_vertex_buffer(0, c.vertices.slice(..));
                pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..mesh.count, 0, 0..1);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wgpu::util::DeviceExt;

    #[test]
    fn reused_cubes_follow_slot_changes_and_reappearing_lights() {
        let ctx = crate::gpu::GpuContext::headless().unwrap();
        let device = &ctx.device;
        let mut pool = ShadowPool::new(device);
        let check = |pool: &mut ShadowPool, lights: &[(Vec3, f32)]| {
            let readback = device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: 512,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let mut encoder = device.create_command_encoder(&Default::default());
            pool.encode(device, &ctx.queue, &mut encoder, lights, &[]);
            for slot in 0..lights.len() {
                encoder.copy_texture_to_buffer(
                    wgpu::TexelCopyTextureInfo {
                        texture: &pool.texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d {
                            x: 0,
                            y: 0,
                            z: slot as u32 * 6,
                        },
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::TexelCopyBufferInfo {
                        buffer: &readback,
                        layout: wgpu::TexelCopyBufferLayout {
                            offset: slot as u64 * 256,
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
            for (slot, light) in lights.iter().enumerate() {
                let offset = slot * 256;
                assert_eq!(
                    *bytemuck::from_bytes::<f32>(&bytes[offset..offset + 4]),
                    light.1,
                    "stale cube slot {slot}"
                );
            }
        };
        let a = (Vec3::ZERO, 10.0);
        let b = (Vec3::X * 100.0, 20.0);
        check(&mut pool, &[a, b]);
        assert_eq!(pool.rendered_cubes, 2);
        check(&mut pool, &[a, b]);
        assert_eq!(pool.rendered_cubes, 2);
        let moved = (Vec3::X, 10.0);
        check(&mut pool, &[moved, b]);
        assert_eq!(pool.rendered_cubes, 3);
        check(&mut pool, &[b, moved]);
        assert_eq!(pool.rendered_cubes, 5);
        check(&mut pool, &[b]);
        assert_eq!(pool.rendered_cubes, 5);
        check(&mut pool, &[b, moved]);
        assert_eq!(pool.rendered_cubes, 6);
        check(&mut pool, &[]);
        assert_eq!(pool.rendered_cubes, 6);
        check(&mut pool, &[b, moved]);
        assert_eq!(pool.rendered_cubes, 8);
    }

    #[test]
    fn textured_shadow_faces_keep_holes_comparisons_blue_opacity_and_skin_motion() {
        assert!(std::mem::size_of::<DrawUniforms>() <= STRIDE);
        let ctx = crate::gpu::GpuContext::headless().unwrap();
        let device = &ctx.device;
        let mut pool = ShadowPool::new(device);
        let vertex = |y, z, u, v| {
            [
                2.0f32, y, z, -1.0, 0.0, 0.0, u, v, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
                0.0,
            ]
        };
        let stream = [
            vertex(0.75, 0.75, 0.0, 0.0),
            vertex(-0.75, 0.75, 1.0, 0.0),
            vertex(-0.75, -0.75, 1.0, 1.0),
            vertex(0.75, -0.75, 0.0, 1.0),
        ];
        let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&stream),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let mesh = GpuMesh {
            vertices: vertices.clone(),
            count: 6,
            indices: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(&[0u32, 1, 2, 0, 2, 3]),
                usage: wgpu::BufferUsages::INDEX,
            }),
        };
        let mut rows = vec![0u8; crate::deferred_mesh::SKIN_STRIDE];
        crate::deferred_mesh::write_palette(&mut rows, &[]);
        let bones = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: &rows,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
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
        let write_opacity = |right: u8| {
            ctx.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &[0, 0, 255, 0, 0, 0, 0, right],
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(8),
                    rows_per_image: Some(1),
                },
                texture.size(),
            )
        };
        write_opacity(255);
        let material = crate::deferred_cutout::Material {
            view: texture.create_view(&Default::default()),
            sampler: device.create_sampler(&Default::default()),
        };
        let pose = std::cell::RefCell::<Vec<[f32; 4]>>::default();
        let world = std::cell::Cell::new(Mat4::IDENTITY);
        let light = std::cell::Cell::new((Vec3::ZERO, 10.0));
        let present = std::cell::Cell::new(true);
        let read = |pool: &mut ShadowPool, alpha: [f32; 4], clip: f32| {
            let pose = pose.borrow();
            let caster = Caster {
                mesh: &mesh,
                vertices: &vertices,
                bones: &bones,
                bone_offset: 0,
                pose: &pose,
                world: world.get(),
                sphere: (world.get().transform_point3(Vec3::new(2.0, 0.0, 0.0)), 3.0),
                alpha,
                clip,
                material: Some(&material),
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
                &[light.get()],
                if present.get() { &casters } else { &[] },
            );
            for (i, x) in [210, 300].into_iter().enumerate() {
                encoder.copy_texture_to_buffer(
                    wgpu::TexelCopyTextureInfo {
                        texture: &pool.texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d {
                            x,
                            y: SIZE / 2,
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
            readback.slice(..).map_async(wgpu::MapMode::Read, move |r| {
                tx.send(r).unwrap();
            });
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            rx.recv().unwrap().unwrap();
            let bytes = readback.slice(..).get_mapped_range().unwrap();
            [0, 256].map(|i| *bytemuck::from_bytes::<f32>(&bytes[i..i + 4]) < light.get().1)
        };
        assert_eq!(read(&mut pool, [7.0, 255.0, 3.0, 1.0], 0.0), [false, true]);
        let rendered = pool.rendered_cubes;
        assert_eq!(read(&mut pool, [7.0, 255.0, 3.0, 1.0], 0.0), [false, true]);
        assert_eq!(
            pool.rendered_cubes, rendered,
            "unchanged cube should be reused"
        );
        assert_eq!(read(&mut pool, [7.0, 255.0, 2.0, 1.0], 0.0), [true, false]);
        // Match D3D9's 8-bit rounding: a fraction below one still passes >= 255.
        assert_eq!(
            read(&mut pool, [7.0, 255.0, 3.0, 0.999], 0.0),
            [false, true]
        );
        write_opacity(128);
        // Fixture-only in-place texture editing: real renderer material textures are
        // immutable, and resource reloads replace the renderer and its pool.
        pool.previous.clear();
        use thedas_formats::state::CompareFunc;
        for (cmp, expected) in [
            (CompareFunc::Never, [false, false]),
            (CompareFunc::Less, [true, false]),
            (CompareFunc::Equal, [false, true]),
            (CompareFunc::LessEqual, [true, true]),
            (CompareFunc::Greater, [false, false]),
            (CompareFunc::NotEqual, [true, false]),
            (CompareFunc::GreaterEqual, [false, true]),
            (CompareFunc::Always, [true, true]),
        ] {
            assert_eq!(
                read(
                    &mut pool,
                    [crate::deferred_cutout::comparison(cmp), 128.0, 3.0, 1.0],
                    0.0
                ),
                expected,
                "comparison {cmp:?}"
            );
        }
        assert_eq!(read(&mut pool, [8.0, 0.0, 3.0, 1.0], 0.3), [false, true]);
        write_opacity(255);
        pool.previous.clear();
        assert_eq!(read(&mut pool, [7.0, 255.0, 3.0, 1.0], 0.0), [false, true]);
        crate::deferred_mesh::write_palette(
            &mut rows,
            &[
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 1.0],
                [0.0, 0.0, 1.0, 0.0],
            ],
        );
        ctx.queue.write_buffer(&bones, 0, &rows);
        *pose.borrow_mut() = bytemuck::cast_slice(&rows).to_vec();
        let rendered = pool.rendered_cubes;
        assert_eq!(read(&mut pool, [7.0, 255.0, 3.0, 1.0], 0.0), [true, false]);
        assert_eq!(
            pool.rendered_cubes,
            rendered + 1,
            "in-place palette update must redraw"
        );
        world.set(Mat4::from_translation(Vec3::Y * 10.0));
        assert_eq!(read(&mut pool, [7.0, 255.0, 3.0, 1.0], 0.0), [false, false]);
        world.set(Mat4::IDENTITY);
        assert_eq!(read(&mut pool, [7.0, 255.0, 3.0, 1.0], 0.0), [true, false]);
        present.set(false);
        assert_eq!(read(&mut pool, [7.0, 255.0, 3.0, 1.0], 0.0), [false, false]);
        present.set(true);
        assert_eq!(read(&mut pool, [7.0, 255.0, 3.0, 1.0], 0.0), [true, false]);
        for change in [(Vec3::new(0.0, -3.0, 0.0), 10.0), (Vec3::ZERO, 12.0)] {
            light.set(change);
            let rendered = pool.rendered_cubes;
            let output = read(&mut pool, [7.0, 255.0, 3.0, 1.0], 0.0);
            assert_eq!(
                pool.rendered_cubes,
                rendered + 1,
                "changed light must redraw"
            );
            pool.previous.clear();
            assert_eq!(read(&mut pool, [7.0, 255.0, 3.0, 1.0], 0.0), output);
        }
    }

    #[test]
    fn shadow_shader_translates_everywhere() {
        crate::backend_check::translates("deferred shadows", SHADOW_WGSL);
    }

    /// The lighting shader picks a face by the major axis of the direction from the light;
    /// that face's frustum must hold the point.
    #[test]
    fn each_direction_lands_in_its_major_axis_face() {
        let at = Vec3::new(3.0, -2.0, 1.0);
        let f = faces(at, 10.0);
        for d in [
            Vec3::new(5.0, 1.0, -2.0),
            Vec3::new(-4.0, 3.0, 1.0),
            Vec3::new(0.5, 6.0, 2.0),
            Vec3::new(1.0, -7.0, -3.0),
            Vec3::new(-1.0, 2.0, 8.0),
            Vec3::new(2.0, 1.0, -6.0),
        ] {
            let a = d.abs();
            let face = if a.x >= a.y && a.x >= a.z {
                usize::from(d.x <= 0.0)
            } else if a.y >= a.z {
                2 + usize::from(d.y <= 0.0)
            } else {
                4 + usize::from(d.z <= 0.0)
            };
            let c = f[face] * (at + d).extend(1.0);
            let ndc = c.truncate() / c.w;
            assert!(c.w > 0.0, "{d}: behind face {face}");
            assert!(
                ndc.x.abs() <= 1.0 && ndc.y.abs() <= 1.0,
                "{d}: {ndc} off face {face}"
            );
            assert!((0.0..=1.0).contains(&ndc.z), "{d}: depth {}", ndc.z);
        }
    }
}
