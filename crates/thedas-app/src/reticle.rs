//! The aiming reticle: where an ability aimed at a place would land, drawn on the ground
//! under the mouse while the player picks it. A circle the size of its area (a fireball's
//! blast), a cone from the user (a cone of cold), or a small ring for a single point. It is
//! drawn with the world, after the level and before the creatures, as the selection rings
//! are (`crate::rings`), so creatures stand over it.
//! Provisional: the game's own reticle art is not used; this is a plain outline.

use glam::{Vec2, Vec3};
use thedas_render::Camera;
use thedas_sim::quickbar::AbilityArea;

/// The outline's colour (RGBA, 0..1) and width (pixels).
pub const COLOUR: [f32; 4] = [1.0, 0.82, 0.3, 1.0];
pub const WIDTH: f32 = 3.0;
/// Raised this far above the ground so the floor does not hide it (metres).
const LIFT: f32 = 0.08;
/// A single point's ring (metres).
const POINT_RADIUS: f32 = 0.6;
const SEGMENTS: usize = 48;

/// The reticle's outline in the world: line segments.
pub fn outline(area: AbilityArea, from: Vec3, at: Vec3) -> Vec<[Vec3; 2]> {
    let at = at + Vec3::Z * LIFT;
    let ring = |r: f32| {
        (0..SEGMENTS)
            .map(|i| {
                let a = |k: usize| k as f32 / SEGMENTS as f32 * std::f32::consts::TAU;
                let p = |k| at + Vec3::new(a(k).cos(), a(k).sin(), 0.0) * r;
                [p(i), p(i + 1)]
            })
            .collect::<Vec<_>>()
    };
    match area {
        AbilityArea::Sphere(r) => ring(r),
        AbilityArea::None => ring(POINT_RADIUS),
        AbilityArea::Cone(angle, length) => {
            let from = Vec3::new(from.x, from.y, at.z);
            let dir = (at - from).truncate();
            let heading = if dir.length_squared() > 1e-6 {
                dir.y.atan2(dir.x)
            } else {
                0.0
            };
            let half = angle.to_radians() * 0.5;
            let edge = |a: f32| from + Vec3::new(a.cos(), a.sin(), 0.0) * length;
            let steps = 16;
            let mut out = vec![[from, edge(heading - half)], [from, edge(heading + half)]];
            for i in 0..steps {
                let a = |k: usize| heading - half + 2.0 * half * k as f32 / steps as f32;
                out.push([edge(a(i)), edge(a(i + 1))]);
            }
            out
        }
    }
}

/// The outline on a `width` x `height` view, in pixels; segments behind the camera are left
/// out.
pub fn project(
    camera: &Camera,
    (width, height): (u32, u32),
    lines: &[[Vec3; 2]],
) -> Vec<[Vec2; 2]> {
    let (w, h) = (width as f32, height as f32);
    let vp = camera.view_proj(w / h.max(1.0));
    let to_pixel = |p: Vec3| {
        let c = vp * p.extend(1.0);
        (c.w > 1e-4).then(|| {
            let n = c.truncate() / c.w;
            Vec2::new((n.x + 1.0) * 0.5 * w, (1.0 - n.y) * 0.5 * h)
        })
    };
    lines
        .iter()
        .filter_map(|[a, b]| Some([to_pixel(*a)?, to_pixel(*b)?]))
        .collect()
}

/// The outline as triangles (pixel coordinates), each segment a quad `WIDTH` wide.
pub fn triangles(lines: &[[Vec2; 2]]) -> Vec<Vec2> {
    let mut out = Vec::with_capacity(lines.len() * 6);
    for [a, b] in lines {
        let d = *b - *a;
        if d.length_squared() < 1e-6 {
            continue;
        }
        let n = Vec2::new(-d.y, d.x).normalize() * (WIDTH * 0.5);
        out.extend([*a - n, *a + n, *b + n, *a - n, *b + n, *b - n]);
    }
    out
}

/// Draws the outline onto RGBA8 `pixels` of a `width` x `height` frame (captures).
pub fn draw_rgba(pixels: &mut [u8], (width, height): (u32, u32), lines: &[[Vec2; 2]]) {
    let colour = COLOUR.map(|c| (c * 255.0) as u8);
    let r = (WIDTH * 0.5).ceil() as i32;
    for [a, b] in lines {
        let steps = (*b - *a).length().ceil().max(1.0) as i32;
        for s in 0..=steps {
            let p = *a + (*b - *a) * (s as f32 / steps as f32);
            for dy in -r..=r {
                for dx in -r..=r {
                    let (x, y) = (p.x as i32 + dx, p.y as i32 + dy);
                    if x < 0 || y < 0 || x >= width as i32 || y >= height as i32 {
                        continue;
                    }
                    let i = (y as usize * width as usize + x as usize) * 4;
                    if let Some(px) = pixels.get_mut(i..i + 4) {
                        px.copy_from_slice(&colour);
                    }
                }
            }
        }
    }
}

const WGSL: &str = r#"
struct Out { @builtin(position) pos: vec4<f32> };
@group(0) @binding(0) var<uniform> colour: vec4<f32>;
@vertex
fn vs(@location(0) p: vec2<f32>) -> Out {
    var o: Out;
    o.pos = vec4<f32>(p, 0.0, 1.0);
    return o;
}
@fragment
fn fs() -> @location(0) vec4<f32> { return colour; }
"#;

/// Draws reticle lines (pixels) into a frame: the aiming reticle under the creatures, the
/// pad's marks over the world.
#[derive(Clone)]
pub struct ReticlePass {
    pipeline: wgpu::RenderPipeline,
    group: wgpu::BindGroup,
}

impl ReticlePass {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        use wgpu::util::DeviceExt;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("reticle"),
            source: wgpu::ShaderSource::Wgsl(WGSL.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("reticle"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let colour = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("reticle colour"),
            contents: bytemuck::cast_slice(&COLOUR),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("reticle"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: colour.as_entire_binding(),
            }],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("reticle"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("reticle"),
            layout: Some(&pl),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: 8,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x2],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        Self { pipeline, group }
    }

    /// Draws `lines` (pixels of a `size` view) over `target`.
    pub fn draw(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        (width, height): (u32, u32),
        lines: &[[Vec2; 2]],
    ) {
        use wgpu::util::DeviceExt;
        let (w, h) = (width.max(1) as f32, height.max(1) as f32);
        let vertices: Vec<[f32; 2]> = triangles(lines)
            .into_iter()
            .map(|p| [p.x / w * 2.0 - 1.0, 1.0 - p.y / h * 2.0])
            .collect();
        if vertices.is_empty() {
            return;
        }
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("reticle"),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("reticle"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.group, &[]);
        pass.set_vertex_buffer(0, buffer.slice(..));
        pass.draw(0..vertices.len() as u32, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outlines_fit_their_areas() {
        let (from, at) = (Vec3::ZERO, Vec3::new(5.0, 0.0, 0.0));
        let ring = outline(AbilityArea::Sphere(7.5), from, at);
        assert_eq!(ring.len(), SEGMENTS);
        assert!(
            ring.iter()
                .all(|[a, _]| ((a.truncate() - at.truncate()).length() - 7.5).abs() < 1e-3)
        );
        let cone = outline(AbilityArea::Cone(90.0, 10.0), from, at);
        // Both edges start at the user and reach the cone's length, 45 degrees either side.
        assert_eq!(cone[0][0].truncate(), Vec2::ZERO);
        assert!((cone[0][1].truncate().length() - 10.0).abs() < 1e-3);
        assert!((cone[0][1].y + cone[1][1].y).abs() < 1e-3 && cone[1][1].y > 0.0);
        assert_eq!(triangles(&[[Vec2::ZERO, Vec2::new(10.0, 0.0)]]).len(), 6);
    }

    #[test]
    fn projects_what_the_camera_sees_and_draws_it() {
        let camera = Camera::new(Vec3::new(0.0, 0.0, 10.0), 0.0, -0.8);
        let lines = outline(
            AbilityArea::Sphere(2.0),
            Vec3::ZERO,
            Vec3::new(8.0, 0.0, 0.0),
        );
        let px = project(&camera, (64, 64), &lines);
        assert_eq!(px.len(), lines.len());
        let mut frame = vec![0u8; 64 * 64 * 4];
        draw_rgba(&mut frame, (64, 64), &px);
        assert!(
            frame
                .chunks(4)
                .any(|p| p == COLOUR.map(|c| (c * 255.0) as u8))
        );
        // Behind the camera: nothing.
        let behind = outline(
            AbilityArea::Sphere(1.0),
            Vec3::ZERO,
            Vec3::new(-20.0, 0.0, 0.0),
        );
        assert!(project(&camera, (64, 64), &behind).is_empty());
    }
}
