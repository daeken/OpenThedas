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

use glam::{Mat4, Vec3, Vec4};

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

/// Posed world bounds are conservative. Keep unknown bounds and a millimetre margin
/// at face/near/far seams rather than risk removing a visible shadow fragment.
fn intersects_face(planes: &[Vec4; 6], (centre, radius): (Vec3, f32)) -> bool {
    if !centre.is_finite() || !radius.is_finite() || radius < 0.0 {
        return true;
    }
    planes.iter().all(|p| {
        let distance = p.truncate().dot(centre) + p.w;
        !distance.is_finite() || distance >= -radius - 0.001
    })
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct DrawUniforms {
    mvp: [[f32; 4]; 4],
    world: [[f32; 4]; 4],
    light: [f32; 4],
}

/// One caster: its mesh, model-to-world and world bounding sphere.
pub(crate) struct Caster<'a> {
    pub mesh: &'a GpuMesh,
    pub vertices: &'a wgpu::Buffer,
    pub bones: &'a wgpu::Buffer,
    pub bone_offset: u32,
    pub world: Mat4,
    pub sphere: (Vec3, f32),
}

pub(crate) struct ShadowPool {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    buffer: wgpu::Buffer,
    /// Slots of [`STRIDE`] the buffer holds.
    capacity: usize,
    /// Per layer (slot · 6 + face), the colour and depth views drawn into.
    layers: Vec<(wgpu::TextureView, wgpu::TextureView)>,
    /// Every layer, for the lighting pass.
    pub(crate) view: wgpu::TextureView,
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
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("deferred shadows"),
            bind_group_layouts: &[Some(&layout)],
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
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
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
            layers,
            view,
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
        // Per layer, its draws: (caster, slot in the buffer).
        let mut data: Vec<u8> = Vec::new();
        let mut layers: Vec<Vec<(usize, u32)>> = Vec::new();
        let cull = std::env::var("THEDAS_DEFERRED_SHADOW_CULL").as_deref() != Ok("0");
        let mut before = 0;
        for &(position, radius) in lights.iter().take(SLOTS) {
            let near: Vec<usize> = casters
                .iter()
                .enumerate()
                .filter(|(_, c)| c.sphere.0.distance(position) < c.sphere.1 + radius)
                .map(|(i, _)| i)
                .collect();
            for view_proj in faces(position, radius) {
                let planes = crate::deferred::frustum(view_proj);
                before += near.len();
                let mut draws = Vec::with_capacity(near.len());
                for &i in &near {
                    let c = &casters[i];
                    if cull && !intersects_face(&planes, c.sphere) {
                        continue;
                    }
                    let u = DrawUniforms {
                        mvp: (view_proj * c.world).to_cols_array_2d(),
                        world: c.world.to_cols_array_2d(),
                        light: position.extend(radius).to_array(),
                    };
                    draws.push((i, (data.len() / STRIDE) as u32));
                    data.extend_from_slice(bytemuck::bytes_of(&u));
                    data.resize(data.len().next_multiple_of(STRIDE), 0);
                }
                layers.push(draws);
            }
        }
        if std::env::var_os("THEDAS_DEFERRED_LOG_LIGHTS").is_some() {
            static ONCE: std::sync::Once = std::sync::Once::new();
            ONCE.call_once(|| {
                log::info!(
                    "deferred shadows: {} draw submissions ({} before face culling)",
                    layers.iter().map(Vec::len).sum::<usize>(),
                    before
                )
            });
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
        for (layer, draws) in layers.iter().enumerate() {
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

    #[test]
    fn face_culling_keeps_seams_near_crossings_and_unknown_bounds() {
        let planes = faces(Vec3::ZERO, 10.0).map(crate::deferred::frustum);
        assert!(intersects_face(&planes[0], (Vec3::new(2.0, 0.0, 0.0), 0.1)));
        assert!(!intersects_face(
            &planes[1],
            (Vec3::new(2.0, 0.0, 0.0), 0.1)
        ));
        for face in [0, 2] {
            assert!(intersects_face(
                &planes[face],
                (Vec3::new(2.0, 2.0, 0.0), 0.1)
            ));
        }
        assert!(intersects_face(&planes[0], (Vec3::new(0.3, 0.0, 0.0), 0.1)));
        assert!(!intersects_face(
            &planes[0],
            (Vec3::new(0.1, 0.0, 0.0), 0.01)
        ));
        for p in &planes {
            assert!(intersects_face(p, (Vec3::ZERO, 2.0)));
            assert!(intersects_face(p, (Vec3::splat(f32::NAN), 1.0)));
            assert!(intersects_face(p, (Vec3::ZERO, f32::INFINITY)));
        }
        // Every point inside each face's true clip volume must retain its enclosing
        // sphere, including at non-origin light positions and face boundaries.
        let at = Vec3::new(1000.0, -500.0, 20.0);
        for vp in faces(at, 10.0) {
            let p = crate::deferred::frustum(vp);
            for z in [0.0, 0.01, 0.5, 0.99, 1.0] {
                for x in [-1.0, 0.0, 1.0] {
                    for y in [-1.0, 0.0, 1.0] {
                        let world = vp.inverse().project_point3(Vec3::new(x, y, z));
                        assert!(intersects_face(&p, (world, 0.01)), "lost {world:?}");
                    }
                }
            }
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
