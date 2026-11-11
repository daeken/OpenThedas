//! A level's frame buffer effects: the passes the original runs over each finished frame of
//! the world, before the interface.
//!
//! A level's layout lists its effects ([`thedas_formats::level::FrameEffect`]); each names a
//! `.ffx.xml` effect and gives values for its exposed parameters. Only `da_bloom` is drawn
//! here: it is on all but five of the 216 layouts. `da_hsvmatrix` (204 layouts, identity on
//! all but about sixteen) needs the engine's `ColorCurve` texture, which the data does not
//! hold; `da_fade` and `da_dof` are on fourteen layouts between them.
//!
//! `da_bloom` (`da_bloom.ffx.xml`, `dabloom.mat.xml`, `gaussianblur.mat.xml`) runs four
//! filters, each mirrored here from its shader:
//!
//! 1. `IlluminationMaskMult.psh` into a half-size target: the frame times
//!    `(luminance + Brightness) * (1 + Contrast)`, alpha too, clamped by the 8-bit target.
//! 2. `TwoPassBlurBloom7.psh` across, then down (`HorizontalBlur7.vsh`, `VerticalBlur7.vsh`):
//!    seven taps `BlurWidth / backbuffer width (height)` apart in texture coordinates, equal
//!    weights; a texel whose alpha is at least the average keeps its own value. At Low
//!    detail, five taps (`TwoPassBlurBloom5.psh`).
//! 3. `BloomBlend.psh` over the frame: the blurred mask times `GlowIntensity` as its alpha,
//!    blended `SRCALPHA, ONE`.
//!
//! The mask and blur read with point filtering (`PPQuadStateNoBlend.sta`), the blend with
//! linear filtering (`BloomBlend.sta`). Every pass maps target pixel `(i, j)` to texture
//! coordinate `(i, j) / target size`, as D3D9 does a full-screen quad with no half-texel
//! correction. All in the frame's stored (gamma) values, as D3D9 had them.
//!
//! No gamma pass follows: DAOrigins.exe applies its Gamma option only as the device's gamma
//! ramp (0x53ab10 → `SetGammaRamp` at 0x869c30), `clamp((x^(1/g) − 0.5) · tan(π/4) + 0.5)`
//! with `g` the option mapped by 0x5ddb00 (`option + 0.5` up to 0.5): at the default 0.50
//! the ramp is the identity.
//!
//! Provisional: the frame's alpha is taken as 1 (an `X8R8G8B8` back buffer, so its copy
//! reads opaque); the original's back buffer format is not traced. The level gives the
//! parameters; the defaults below are the `.ffx.xml` file's.

use thedas_formats::level::FrameEffect;

/// The format of the original's `A8R8G8B8` half-size targets.
const HALF_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// `da_bloom`'s parameters, as the level gives them (or `da_bloom.ffx.xml`'s defaults).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bloom {
    pub brightness: f32,
    pub contrast: f32,
    pub glow_intensity: f32,
    pub h_blur_width: f32,
    pub v_blur_width: f32,
}

impl Bloom {
    /// The level's `da_bloom`, when it lists one.
    pub fn from_effects(effects: &[FrameEffect]) -> Option<Bloom> {
        let e = effects
            .iter()
            .find(|e| e.name.eq_ignore_ascii_case("da_bloom"))?;
        let v = |name: &str, default: f32| {
            e.parameter(name)
                .filter(|v| v.is_finite())
                .unwrap_or(default)
        };
        Some(Bloom {
            brightness: v("Brightness", 0.5),
            contrast: v("Contrast", 0.5),
            glow_intensity: v("GlowIntensity", 0.25),
            h_blur_width: v("HBlurWidth", 2.0),
            v_blur_width: v("VBlurWidth", 2.0),
        })
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    brightness: f32,
    contrast: f32,
    glow: f32,
    blur: f32,
    /// The back buffer's size (`WindowWidth`, `WindowHeight`).
    window: [f32; 2],
    /// The blur's axis.
    direction: [f32; 2],
    /// The size of the target being drawn.
    target: [f32; 2],
    /// The blur's taps each side of the centre (3, or 2 at Low detail).
    taps: f32,
    _pad: f32,
}

const SHADER: &str = r#"
struct Params {
    brightness: f32,
    contrast: f32,
    glow: f32,
    blur: f32,
    window: vec2<f32>,
    direction: vec2<f32>,
    dest: vec2<f32>,
    taps: f32,
    pad: f32,
};

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var smp: sampler;
@group(0) @binding(2) var<uniform> p: Params;

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let t = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(t * 2.0 - 1.0, 0.0, 1.0);
}

// D3D9 samples a full-screen quad's pixel (i, j) at texture coordinate (i, j) / size.
fn d3d_uv(pos: vec4<f32>) -> vec2<f32> {
    return (pos.xy - vec2<f32>(0.5)) / p.dest;
}

// Point filtering: the texel holding `uv`, edges clamped.
fn point(uv: vec2<f32>) -> vec4<f32> {
    let size = vec2<i32>(textureDimensions(src));
    let t = vec2<i32>(floor(uv * vec2<f32>(size) + vec2<f32>(1e-3)));
    return textureLoad(src, clamp(t, vec2<i32>(0), size - vec2<i32>(1)), 0);
}

// IlluminationMaskMult.psh. The frame's alpha reads 1 (see the module docs).
@fragment
fn mask(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let c = vec4<f32>(point(d3d_uv(pos)).rgb, 1.0);
    let lum = dot(c.rgb, vec3<f32>(0.2125, 0.7154, 0.0721));
    return c * ((lum + p.brightness) * (1.0 + p.contrast));
}

// TwoPassBlurBloom7.psh with HorizontalBlur7.vsh / VerticalBlur7.vsh's offsets, or the
// five-tap TwoPassBlurBloom5.psh / HorizontalBlur5.vsh / VerticalBlur5.vsh (`taps` 2).
@fragment
fn blur(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = d3d_uv(pos);
    let step = p.direction * p.blur / p.window;
    let c = point(uv);
    var sum = c;
    let taps = i32(p.taps);
    for (var k = 1; k <= taps; k++) {
        sum += point(uv + step * f32(k)) + point(uv - step * f32(k));
    }
    let avg = sum * (1.0 / f32(2 * taps + 1));
    return select(avg, c, c.a - avg.a >= 0.0);
}

// BloomBlend.psh, blended SRCALPHA, ONE.
@fragment
fn blend(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let m = textureSample(src, smp, d3d_uv(pos));
    return vec4<f32>(m.rgb, m.a * p.glow);
}
"#;

struct Targets {
    size: (u32, u32),
    copy: wgpu::Texture,
    /// Bind groups: mask (reads the copy), across (reads A), down (reads B), blend (reads A).
    groups: [wgpu::BindGroup; 4],
    a: wgpu::TextureView,
    b: wgpu::TextureView,
}

/// The frame buffer effects of one level, drawn over frames of `color_format`.
pub struct FrameEffects {
    pub bloom: Bloom,
    /// Taps each side of the blur's centre: three (`TwoPassBlurBloom7`) above Low detail, two
    /// (`TwoPassBlurBloom5`) at Low, as `gaussianblur.mat.xml`'s logic tests choose.
    pub blur_taps: u32,
    color_format: wgpu::TextureFormat,
    layout: wgpu::BindGroupLayout,
    mask: wgpu::RenderPipeline,
    blur: wgpu::RenderPipeline,
    blend: wgpu::RenderPipeline,
    point: wgpu::Sampler,
    linear: wgpu::Sampler,
    /// Uniforms for the four passes, in order.
    params: [wgpu::Buffer; 4],
    targets: Option<Targets>,
}

impl FrameEffects {
    /// The effects `effects` lists that are drawn here, or `None` when there are none.
    pub fn new(
        device: &wgpu::Device,
        effects: &[FrameEffect],
        detail: crate::scene::Detail,
        color_format: wgpu::TextureFormat,
    ) -> Option<Self> {
        let bloom = Bloom::from_effects(effects)?;
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("frame effects"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("frame effect"),
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
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("frame effect"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = |entry: &str, format, blend| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(entry),
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
                        format,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        // SRCALPHA, ONE on colour; the frame's alpha is left as it is.
        let add = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::SrcAlpha,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Zero,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
        };
        let mask = pipeline("mask", HALF_FORMAT, None);
        let blur = pipeline("blur", HALF_FORMAT, None);
        let blend = pipeline("blend", color_format, Some(add));
        let sampler = |filter| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("frame effect"),
                address_mode_u: wgpu::AddressMode::ClampToEdge,
                address_mode_v: wgpu::AddressMode::ClampToEdge,
                mag_filter: filter,
                min_filter: filter,
                ..Default::default()
            })
        };
        let params = std::array::from_fn(|_| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("frame effect"),
                size: std::mem::size_of::<Params>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        });
        Some(FrameEffects {
            bloom,
            blur_taps: if detail == crate::scene::Detail::Low {
                2
            } else {
                3
            },
            color_format,
            layout,
            mask,
            blur,
            blend,
            point: sampler(wgpu::FilterMode::Nearest),
            linear: sampler(wgpu::FilterMode::Linear),
            params,
            targets: None,
        })
    }

    fn make_targets(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        if self.targets.as_ref().map(|t| t.size) != Some((width, height)) {
            let texture = |label, w: u32, h: u32, format, usage| {
                device.create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width: w.max(1),
                        height: h.max(1),
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage,
                    view_formats: &[],
                })
            };
            let half =
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
            let copy = texture(
                "back buffer copy",
                width,
                height,
                self.color_format,
                wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
            );
            let a = texture("light mask", width / 2, height / 2, HALF_FORMAT, half);
            let b = texture("downsampled", width / 2, height / 2, HALF_FORMAT, half);
            let view = |t: &wgpu::Texture| t.create_view(&wgpu::TextureViewDescriptor::default());
            let (copy_view, a, b) = (view(&copy), view(&a), view(&b));
            let group =
                |src: &wgpu::TextureView, sampler: &wgpu::Sampler, params: &wgpu::Buffer| {
                    device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("frame effect"),
                        layout: &self.layout,
                        entries: &[
                            wgpu::BindGroupEntry {
                                binding: 0,
                                resource: wgpu::BindingResource::TextureView(src),
                            },
                            wgpu::BindGroupEntry {
                                binding: 1,
                                resource: wgpu::BindingResource::Sampler(sampler),
                            },
                            wgpu::BindGroupEntry {
                                binding: 2,
                                resource: params.as_entire_binding(),
                            },
                        ],
                    })
                };
            let groups = [
                group(&copy_view, &self.point, &self.params[0]),
                group(&a, &self.point, &self.params[1]),
                group(&b, &self.point, &self.params[2]),
                group(&a, &self.linear, &self.params[3]),
            ];
            self.targets = Some(Targets {
                size: (width, height),
                copy,
                groups,
                a,
                b,
            });
        }
    }

    /// Records the effects over `target` (a frame of this effect set's colour format, with
    /// `COPY_SRC` usage) of the given size.
    pub fn apply(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::Texture,
        width: u32,
        height: u32,
    ) {
        if width < 2 || height < 2 {
            return;
        }
        let b = self.bloom;
        let window = [width as f32, height as f32];
        let half = [(width / 2) as f32, (height / 2) as f32];
        let params = |blur: f32, direction: [f32; 2], target: [f32; 2]| Params {
            brightness: b.brightness,
            contrast: b.contrast,
            glow: b.glow_intensity,
            blur,
            window,
            direction,
            target,
            taps: self.blur_taps as f32,
            _pad: 0.0,
        };
        let all = [
            params(0.0, [0.0; 2], half),
            params(b.h_blur_width, [1.0, 0.0], half),
            params(b.v_blur_width, [0.0, 1.0], half),
            params(0.0, [0.0; 2], window),
        ];
        for (buffer, p) in self.params.iter().zip(&all) {
            queue.write_buffer(buffer, 0, bytemuck::bytes_of(p));
        }
        self.make_targets(device, width, height);
        let t = self.targets.as_ref().expect("targets just made");
        encoder.copy_texture_to_texture(
            target.as_image_copy(),
            t.copy.as_image_copy(),
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        let frame = target.create_view(&wgpu::TextureViewDescriptor::default());
        let passes: [(&wgpu::RenderPipeline, &wgpu::TextureView, usize, bool); 4] = [
            (&self.mask, &t.a, 0, false),
            (&self.blur, &t.b, 1, false),
            (&self.blur, &t.a, 2, false),
            (&self.blend, &frame, 3, true),
        ];
        for (pipeline, view, group, keep) in passes {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("frame effect"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: if keep {
                            wgpu::LoadOp::Load
                        } else {
                            wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT)
                        },
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &t.groups[group], &[]);
            pass.draw(0..3, 0..1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shader_validates() {
        let module = naga::front::wgsl::parse_str(SHADER).expect("parses");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .expect("validates");
    }

    #[test]
    fn bloom_takes_the_levels_values() {
        let effects = vec![
            FrameEffect {
                name: "da_hsvmatrix".into(),
                parameters: vec![("Brightness".into(), 1.0)],
            },
            FrameEffect {
                name: "da_bloom".into(),
                parameters: vec![
                    ("Brightness".into(), 0.25),
                    ("GlowIntensity".into(), 0.1),
                    ("HBlurWidth".into(), 5.0),
                    ("VBlurWidth".into(), f32::NAN),
                ],
            },
        ];
        let b = Bloom::from_effects(&effects).unwrap();
        assert_eq!(b.brightness, 0.25);
        assert_eq!(b.contrast, 0.5);
        assert_eq!(b.glow_intensity, 0.1);
        assert_eq!(b.h_blur_width, 5.0);
        assert_eq!(b.v_blur_width, 2.0);
        assert_eq!(Bloom::from_effects(&effects[..1]), None);
    }
}
