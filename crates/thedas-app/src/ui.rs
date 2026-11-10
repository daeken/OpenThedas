//! The Scaleform interface layer: movies played by `thedas-ui` into offscreen textures,
//! composited over the scene with premultiplied alpha.

use std::sync::Arc;

use anyhow::Result;
use thedas_render::GpuContext;
use thedas_ui::protocol::GameMessage;
use thedas_ui::stage::Stage;
use thedas_ui::{Gpu, Rgba};
use thedas_vfs::Vfs;

const COMPOSITE_WGSL: &str = r#"
@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;

struct Out {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs(@builtin(vertex_index) i: u32) -> Out {
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var o: Out;
    o.pos = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, 0.0, 1.0);
    o.uv = uv;
    return o;
}

@fragment
fn fs(i: Out) -> @location(0) vec4<f32> {
    return textureSample(tex, samp, i.uv);
}
"#;

struct Composite {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
}

impl Composite {
    fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ui composite"),
            source: wgpu::ShaderSource::Wgsl(COMPOSITE_WGSL.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ui composite"),
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
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ui composite"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("ui composite"),
            layout: Some(&pl),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self {
            pipeline,
            layout,
            sampler,
        }
    }
}

pub struct UiLayer {
    pub stage: Stage,
    composite: Option<Composite>,
    device: wgpu::Device,
}

impl UiLayer {
    /// Loads the movies at `size`; failures are logged and the movie is skipped.
    pub fn new(ctx: &GpuContext, vfs: &Arc<Vfs>, names: &[String], size: (u32, u32)) -> Self {
        let gpu = Gpu {
            instance: ctx.instance.clone(),
            adapter: ctx.adapter.clone(),
            device: ctx.device.clone(),
            queue: ctx.queue.clone(),
        };
        let mut stage = Stage::new(gpu, vfs, size);
        stage.open_all(names);
        Self {
            stage,
            composite: None,
            device: ctx.device.clone(),
        }
    }

    pub fn tick(&mut self, dt_ms: f64) -> Vec<GameMessage> {
        self.stage.tick(dt_ms)
    }

    pub fn resize(&mut self, size: (u32, u32)) {
        self.stage.resize(size);
    }

    pub fn mouse_move(&mut self, x: f64, y: f64) {
        self.stage.mouse_move(x, y);
    }

    pub fn mouse_button(&mut self, x: f64, y: f64, down: bool) {
        self.stage.mouse_button(x, y, down);
    }

    /// Renders every movie into its texture and draws them over `target`, in order.
    pub fn composite(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        format: wgpu::TextureFormat,
    ) {
        if self.stage.movies.is_empty() {
            return;
        }
        let comp = self
            .composite
            .get_or_insert_with(|| Composite::new(&self.device, format));
        let mut groups = Vec::new();
        for m in &mut self.stage.movies {
            m.render();
            let Some(tex) = m.texture() else { continue };
            let view = tex.create_view(&Default::default());
            groups.push(self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("ui movie"),
                layout: &comp.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&comp.sampler),
                    },
                ],
            }));
        }
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("ui composite"),
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
        pass.set_pipeline(&comp.pipeline);
        for g in &groups {
            pass.set_bind_group(0, g, &[]);
            pass.draw(0..3, 0..1);
        }
    }

    /// Logs what the movies asked for and did not get: failed loads and unanswered values.
    pub fn log_gaps(&self) {
        for (movie, url) in self.stage.failed_loads() {
            log::warn!("ui: {movie}: load of {url} failed");
        }
        let gaps = self.stage.bridge.unanswered();
        if !gaps.is_empty() {
            log::info!(
                "ui: {} engine values unanswered: {:?}",
                gaps.len(),
                gaps.keys().take(40).collect::<Vec<_>>()
            );
        }
    }

    pub fn capture_over(&mut self, base: &mut Rgba) -> Result<()> {
        Ok(self.stage.capture_over(base)?)
    }
}
