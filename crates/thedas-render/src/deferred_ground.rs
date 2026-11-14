//! Preserve already-lit characters while depth-disabled ground marks draw over terrain.
//!
//! Keep the actual pre-overlay colour, including forward occluders and every MSAA sample.
//! Restoring only the G-buffer's character pixels preserves arbitrary authored ring blends.

const SHADER: &str = r#"
@group(0) @binding(0) var colour: COLOUR_TYPE;
@group(0) @binding(1) var material: texture_2d<f32>;
struct Out { @location(0) colour: vec4<f32>, @builtin(sample_mask) mask: u32 };
@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let t = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(t * 2.0 - 1.0, 0.0, 1.0);
}
@fragment
fn copy(@builtin(position) p: vec4<f32>) -> Out {
    return Out(textureLoad(colour, vec2<i32>(p.xy), SAMPLE_INDEX), SAMPLE_MASK);
}
@fragment
fn restore(@builtin(position) p: vec4<f32>) -> Out {
    // Material code 192/255, distinct from props (64), terrain (128/255), level (0)
    // and the cleared background (0). Coverage is the G-buffer's single sample.
    let code = textureLoad(material, vec2<i32>(p.xy), 0).a;
    if (u32(code * 255.0 + 0.5) != 192u) { discard; }
    return Out(textureLoad(colour, vec2<i32>(p.xy), SAMPLE_INDEX), SAMPLE_MASK);
}
"#;

fn shader(samples: u32, sample: u32) -> String {
    SHADER
        .replace(
            "COLOUR_TYPE",
            if samples > 1 {
                "texture_multisampled_2d<f32>"
            } else {
                "texture_2d<f32>"
            },
        )
        .replace("SAMPLE_INDEX", &sample.to_string())
        .replace("SAMPLE_MASK", &format!("{}u", 1u32 << sample))
}

pub(crate) struct GroundMask {
    saved: wgpu::TextureView,
    layout: wgpu::BindGroupLayout,
    copy: Vec<wgpu::RenderPipeline>,
    restore: Vec<wgpu::RenderPipeline>,
    material: wgpu::TextureView,
}

impl GroundMask {
    pub(crate) fn fits(&self, frame: &wgpu::TextureView, material: &wgpu::TextureView) -> bool {
        let (a, b) = (self.saved.texture(), frame.texture());
        a.size() == b.size()
            && a.format() == b.format()
            && a.sample_count() == b.sample_count()
            && self.material == *material
    }

    pub(crate) fn new(
        device: &wgpu::Device,
        frame: &wgpu::TextureView,
        material: &wgpu::TextureView,
    ) -> Self {
        let texture = frame.texture();
        let samples = texture.sample_count();
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("deferred ground mask"),
            entries: &[0, 1].map(|binding| wgpu::BindGroupLayoutEntry {
                binding,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: binding == 0 && samples > 1,
                },
                count: None,
            }),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("deferred ground mask"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = |entry, sample| {
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("deferred ground mask"),
                source: wgpu::ShaderSource::Wgsl(shader(samples, sample).into()),
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("deferred ground mask"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: texture.format(),
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: Default::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState {
                    count: samples,
                    ..Default::default()
                },
                multiview_mask: None,
                cache: None,
            })
        };
        let saved = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("deferred pre-ground colour"),
                size: texture.size(),
                mip_level_count: 1,
                sample_count: samples,
                dimension: wgpu::TextureDimension::D2,
                format: texture.format(),
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING
                    | if samples == 1 {
                        wgpu::TextureUsages::COPY_DST
                    } else {
                        wgpu::TextureUsages::empty()
                    },
                view_formats: &[],
            })
            .create_view(&Default::default());
        Self {
            saved,
            copy: (0..samples).map(|s| pipeline("copy", s)).collect(),
            restore: (0..samples).map(|s| pipeline("restore", s)).collect(),
            layout,
            material: material.clone(),
        }
    }

    fn draw(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        source: &wgpu::TextureView,
        target: &wgpu::TextureView,
        restore: bool,
    ) {
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("deferred ground mask"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(source),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&self.material),
                },
            ],
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("deferred ground mask"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: if restore {
                        wgpu::LoadOp::Load
                    } else {
                        wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT)
                    },
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        pass.set_bind_group(0, &group, &[]);
        // Fragment-output masks work on GL 3.3, which lacks sample_index.
        // Metal does not apply wgpu's pipeline sample mask, so write it in WGSL.
        for pipeline in if restore { &self.restore } else { &self.copy } {
            pass.set_pipeline(pipeline);
            pass.draw(0..3, 0..1);
        }
    }

    pub(crate) fn save(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        frame: &wgpu::TextureView,
    ) {
        if frame.texture().sample_count() > 1 {
            self.draw(device, encoder, frame, &self.saved, false);
        } else {
            encoder.copy_texture_to_texture(
                frame.texture().as_image_copy(),
                self.saved.texture().as_image_copy(),
                frame.texture().size(),
            );
        }
    }

    pub(crate) fn restore(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        frame: &wgpu::TextureView,
    ) {
        self.draw(device, encoder, &self.saved, frame, true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restores_only_characters_and_preserves_each_msaa_sample() {
        let ctx = crate::gpu::GpuContext::headless().unwrap();
        let device = &ctx.device;
        for samples in [1, 4] {
            let texture = |width, count, usage| {
                device.create_texture(&wgpu::TextureDescriptor {
                    label: None,
                    size: wgpu::Extent3d {
                        width,
                        height: 1,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: count,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    usage,
                    view_formats: &[],
                })
            };
            let mut usage =
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
            if samples == 1 {
                usage |= wgpu::TextureUsages::COPY_SRC;
            }
            let frame = texture(7, samples, usage).create_view(&Default::default());
            let material = texture(
                7,
                1,
                wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            );
            let pixels: Vec<u8> = [0, 64, 128, 191, 192, 193, 255]
                .into_iter()
                .flat_map(|a| [0, 0, 0, a])
                .collect();
            ctx.queue.write_texture(
                material.as_image_copy(),
                &pixels,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(28),
                    rows_per_image: Some(1),
                },
                material.size(),
            );
            let material = material.create_view(&Default::default());
            let mask = GroundMask::new(device, &frame, &material);
            assert!(mask.fits(&frame, &material));
            let different = texture(7, 1, wgpu::TextureUsages::TEXTURE_BINDING)
                .create_view(&Default::default());
            assert!(!mask.fits(&frame, &different));
            let smaller = texture(3, samples, usage).create_view(&Default::default());
            assert!(!mask.fits(&smaller, &material));
            let pipeline = |module: &wgpu::ShaderModule,
                            entry,
                            layout: Option<&wgpu::PipelineLayout>,
                            count,
                            sample_mask| {
                device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: None,
                    layout,
                    vertex: wgpu::VertexState {
                        module,
                        entry_point: Some("vs"),
                        compilation_options: Default::default(),
                        buffers: &[],
                    },
                    fragment: Some(wgpu::FragmentState {
                        module,
                        entry_point: Some(entry),
                        compilation_options: Default::default(),
                        targets: &[Some(wgpu::ColorTargetState {
                            format: wgpu::TextureFormat::Rgba8Unorm,
                            blend: None,
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                    }),
                    primitive: Default::default(),
                    depth_stencil: None,
                    multisample: wgpu::MultisampleState {
                        count,
                        mask: sample_mask,
                        ..Default::default()
                    },
                    multiview_mask: None,
                    cache: None,
                })
            };
            let mut encoder = device.create_command_encoder(&Default::default());
            // Give every sample a different pre-overlay colour, as at a forward edge.
            for sample in 0..samples {
                let source = format!(
                    "{}\n@fragment fn initial() -> Out {{ return Out(vec4<f32>({}, 0.25, 0.5, 1.0), {}u); }}",
                    shader(samples, sample),
                    (sample + 1) as f32 / 8.0,
                    1u32 << sample
                );
                let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: None,
                    source: wgpu::ShaderSource::Wgsl(source.into()),
                });
                let initial = pipeline(&module, "initial", None, samples, 1 << sample);
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &frame,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: if sample == 0 {
                                wgpu::LoadOp::Clear(wgpu::Color::BLACK)
                            } else {
                                wgpu::LoadOp::Load
                            },
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                });
                pass.set_pipeline(&initial);
                pass.draw(0..3, 0..1);
            }
            mask.save(device, &mut encoder, &frame);
            // A depth-disabled, fully covering green ground mark.
            drop(encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &frame,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::GREEN),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                ..Default::default()
            }));
            mask.restore(device, &mut encoder, &frame);
            // Read every sample separately rather than only its resolved average.
            let dump_source = format!("{}\n@fragment fn dump(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {{ return textureLoad(colour, vec2<i32>(i32(p.x) / {samples}, 0), DUMP_SAMPLE); }}", shader(samples, 0))
                .replace("DUMP_SAMPLE", if samples == 1 { "0" } else { "i32(p.x) % 4" });
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: None,
                source: wgpu::ShaderSource::Wgsl(dump_source.into()),
            });
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: None,
                bind_group_layouts: &[Some(&mask.layout)],
                immediate_size: 0,
            });
            let dump = pipeline(&module, "dump", Some(&layout), 1, !0);
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &mask.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&frame),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&material),
                    },
                ],
            });
            let output = texture(
                7 * samples,
                1,
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            );
            let output_view = output.create_view(&Default::default());
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &output_view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                });
                pass.set_pipeline(&dump);
                pass.set_bind_group(0, &group, &[]);
                pass.draw(0..3, 0..1);
            }
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: 256,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            encoder.copy_texture_to_buffer(
                output.as_image_copy(),
                wgpu::TexelCopyBufferInfo {
                    buffer: &buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(256),
                        rows_per_image: Some(1),
                    },
                },
                output.size(),
            );
            ctx.queue.submit([encoder.finish()]);
            let (tx, rx) = std::sync::mpsc::channel();
            buffer
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
            device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            rx.recv().unwrap().unwrap();
            let data = buffer.slice(..).get_mapped_range().unwrap();
            for x in 0..7 {
                for sample in 0..samples {
                    let offset = ((x * samples + sample) * 4) as usize;
                    let pixel = &data[offset..offset + 4];
                    if x == 4 {
                        let expected = [
                            ((sample + 1) as f32 * 255.0 / 8.0).round() as u8,
                            64,
                            128,
                            255,
                        ];
                        assert_eq!(pixel, expected, "lost sample {sample} at {samples}x");
                    } else {
                        assert_eq!(pixel, [0, 255, 0, 255], "masked material at {x}");
                    }
                }
            }
        }
    }

    #[test]
    fn mask_shaders_translate_for_single_and_multisampled_frames() {
        for samples in [1, 4] {
            for sample in 0..samples {
                crate::backend_check::translates(
                    "deferred ground mask",
                    &super::shader(samples, sample),
                );
            }
        }
    }
}
