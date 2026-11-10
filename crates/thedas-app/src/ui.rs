//! The Scaleform interface layer: movies played by `thedas-ui` into offscreen textures,
//! composited over the scene with premultiplied alpha.

use std::sync::Arc;

use anyhow::{Context, Result};
use thedas_render::GpuContext;
use thedas_ui::protocol::{GameBridge, GameMessage, START_SCENE};
use thedas_ui::{Gpu, ImageSource, ResourceLookup, Rgba, UiMovie};
use thedas_vfs::Vfs;

/// Movie sets by name; anything else is a comma-separated list of movie names.
pub fn movie_names(spec: &str) -> Vec<String> {
    let list: &[&str] = match spec {
        "menu" => &["mainmenu"],
        "hud" => &["portraits", "quickbar", "minimap"],
        _ => &[],
    };
    let names: Vec<String> = if list.is_empty() {
        spec.split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect()
    } else {
        list.iter().map(|s| s.to_string()).collect()
    };
    names
        .into_iter()
        .map(|n| {
            if n.contains('.') {
                n.to_ascii_lowercase()
            } else {
                format!("{}.gfx", n.to_ascii_lowercase())
            }
        })
        .collect()
}

pub fn vfs_lookup(vfs: &Arc<Vfs>) -> ResourceLookup {
    let vfs = vfs.clone();
    Arc::new(move |name: &str| vfs.read(name).ok().map(|x| x.0))
}

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

/// English talk-table strings from every `*en-us.tlk` in the stack.
fn talk_table(vfs: &Vfs) -> thedas_ui::protocol::StringLookup {
    let mut map = std::collections::HashMap::new();
    for name in vfs.names().into_iter().filter(|n| n.ends_with("en-us.tlk")) {
        if let Ok((data, _)) = vfs.read(&name) {
            if let Ok(t) = thedas_formats::Tlk::parse(&data) {
                map.extend(
                    t.strings
                        .into_iter()
                        .map(|(k, v)| (k, v.trim_end_matches('\0').to_string())),
                );
            }
        }
    }
    log::info!("ui: {} talk-table strings", map.len());
    Arc::new(move |id| map.get(&id).cloned())
}

pub struct UiLayer {
    pub movies: Vec<UiMovie>,
    pub bridge: GameBridge,
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
        let lookup = vfs_lookup(vfs);
        let atlases = vfs
            .names()
            .into_iter()
            .filter(|n| n.starts_with("atl_") && n.ends_with("_dat.xml"));
        let mut images = ImageSource::new(lookup.clone(), atlases);
        log::info!("ui: {} atlas images indexed", images.len());
        let bridge = GameBridge::new(talk_table(vfs));
        let mut movies = Vec::new();
        for name in names {
            let made = thedas_ui::load_movie(name, &lookup, &mut images).and_then(|(swf, rep)| {
                log::info!(
                    "ui: {name}: {} images inlined, {} missing, Scaleform tags dropped {:?}",
                    rep.images_inlined.len(),
                    rep.images_missing.len(),
                    rep.dropped
                );
                for m in &rep.images_missing {
                    log::warn!("ui: {name}: image {m} not found");
                }
                UiMovie::new(
                    &gpu,
                    name,
                    &swf,
                    size,
                    lookup.clone(),
                    Some(bridge.responder()),
                )
            });
            match made {
                Ok(mut m) => {
                    // Run the first frame (class definitions, InitMovie), then start the
                    // movie the way the game does.
                    for _ in 0..5 {
                        if !m.callbacks().is_empty() {
                            break;
                        }
                        m.run_frame();
                    }
                    let r = m.call(START_SCENE, &[]);
                    log::debug!("ui: {name}: started ({r:?})");
                    movies.push(m)
                }
                Err(e) => log::warn!("ui: {name}: {e}"),
            }
        }
        Self {
            movies,
            bridge,
            composite: None,
            device: ctx.device.clone(),
        }
    }

    /// Advances every movie; returns the messages meant for the rest of the engine.
    pub fn tick(&mut self, dt_ms: f64) -> Vec<GameMessage> {
        let mut calls = Vec::new();
        for m in &mut self.movies {
            m.tick(dt_ms);
            calls.extend(
                m.drain_calls()
                    .into_iter()
                    .filter_map(|c| self.bridge.handle(c)),
            );
            for line in m.drain_log() {
                log::debug!("ui: {}: {line}", m.name);
            }
        }
        calls
    }

    pub fn resize(&mut self, size: (u32, u32)) {
        for m in &mut self.movies {
            m.resize(size);
        }
    }

    pub fn mouse_move(&mut self, x: f64, y: f64) {
        for m in &mut self.movies {
            m.mouse_move(x, y);
        }
    }

    pub fn mouse_button(&mut self, x: f64, y: f64, down: bool) {
        for m in &mut self.movies {
            m.mouse_button(x, y, down);
        }
    }

    /// Renders every movie into its texture and draws them over `target`, in order.
    pub fn composite(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        format: wgpu::TextureFormat,
    ) {
        if self.movies.is_empty() {
            return;
        }
        let comp = self
            .composite
            .get_or_insert_with(|| Composite::new(&self.device, format));
        let mut groups = Vec::new();
        for m in &mut self.movies {
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
        for m in &self.movies {
            for (url, ok) in m.fetches() {
                if !ok {
                    log::warn!("ui: {}: load of {url} failed", m.name);
                }
            }
        }
        let gaps = self.bridge.unanswered();
        if !gaps.is_empty() {
            log::info!(
                "ui: {} engine values unanswered: {:?}",
                gaps.len(),
                gaps.keys().take(40).collect::<Vec<_>>()
            );
        }
    }

    /// Renders every movie and blends the frames over `base` (straight RGBA) on the CPU.
    pub fn capture_over(&mut self, base: &mut Rgba) -> Result<()> {
        for m in &mut self.movies {
            m.render();
            let frame = m
                .capture()
                .with_context(|| format!("capturing {}", m.name))?;
            blend_premultiplied(base, &frame);
        }
        Ok(())
    }
}

/// `base = src + base * (1 - src.a)` for a premultiplied `src` of the same size.
pub fn blend_premultiplied(base: &mut Rgba, src: &Rgba) {
    if (base.width, base.height) != (src.width, src.height) {
        return;
    }
    for (b, s) in base
        .pixels
        .chunks_exact_mut(4)
        .zip(src.pixels.chunks_exact(4))
    {
        let inv = 255 - s[3] as u32;
        for c in 0..3 {
            b[c] = (s[c] as u32 + (b[c] as u32 * inv + 127) / 255).min(255) as u8;
        }
        b[3] = (s[3] as u32 + (b[3] as u32 * inv + 127) / 255).min(255) as u8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_movies() {
        assert_eq!(movie_names("menu"), vec!["mainmenu.gfx"]);
        assert_eq!(
            movie_names("Quickbar, x.swf,"),
            vec!["quickbar.gfx", "x.swf"]
        );
        assert_eq!(movie_names("hud").len(), 3);
    }

    #[test]
    fn blends_premultiplied() {
        let mut base = Rgba {
            width: 1,
            height: 1,
            pixels: vec![200, 100, 0, 255],
        };
        let src = Rgba {
            width: 1,
            height: 1,
            pixels: vec![50, 0, 0, 128],
        };
        blend_premultiplied(&mut base, &src);
        assert_eq!(base.pixels, vec![50 + 100, 50, 0, 255]);
    }
}
