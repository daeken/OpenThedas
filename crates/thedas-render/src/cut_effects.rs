//! The frame buffer effects cutscenes run (their kind-19 actions, `AFBE`): depth of field
//! (`da_dof.ffx.xml`, about 2,900 actions across the game's cutscenes; no level lists it) and
//! the fades from and to a colour (`da_colorfade.ffx.xml`, about 1,000).
//!
//! **`da_dof`** (layer 5, at the end of the frame), as its effect file chains it, with the
//! shaders transcribed in [`SHADER`]:
//!
//! 1. `CreateDownsampledCoC` (`CoCDownsample.vsh`/`.psh`) into a half-size target: the
//!    colour is the mean of four taps of the frame (at `uv − o`, `uv + (o.x, −o.y)` and each
//!    of those half a pixel down, `o` a frame pixel), the alpha the largest near-field
//!    circle of confusion of the four, `(FocusDistance − FocalRadius − depth) / NearFade`.
//! 2. `HBlurCoC`, then `VBlurCoC` (`HorizontalBlur.vsh`/`VerticalBlur.vsh`,
//!    `TwoPassBlurCoCHorizontl.psh`/`TwoPassBlurCoCVertical.psh`): seven taps `WeightBlur`
//!    half-size pixels apart, weights 0.05, 0.1, 0.2, 0.3, 0.2, 0.1, 0.05; the alpha comes
//!    out as `2·max(blurred, sharp) − sharp`, `sharp` the pass's centre (the vertical pass
//!    takes it from step 1's target).
//! 3. `SmallBlur` (`SmallBlur.vsh`/`.psh`): the mean of four taps half of `ArtifactBlur`
//!    (1.3) pixels out to the corners.
//! 4. `ComposeOnScreenDoF` (`ComposeDoF.vsh`/`.psh`, `ComposeDoF.sta`: blended by its
//!    alpha): the blurred colour over the frame with alpha
//!    `max(blurred near circle, (depth − FocusDistance − FocalRadius) / FarFade) · MaxBlur`.
//!
//! Every sampler is point filtered and clamped. The depth the shaders read is the scene's
//! distance from the camera (the materials' second colour output, which is not drawn here);
//! it is worked out from the depth buffer instead.
//!
//! **`da_colorfade`** (layer 40): `LinearFade.psh` gives
//! `StartColour + (time / TimeToFade)·(EndColour − StartColour)`, blended over the frame by
//! its alpha (`PPQuadState.sta`); `time` is the effect's clock.
//!
//! Provisional: the effect clock's upper bound (the file gives the runtime value a range of
//! 0 to 6) is not applied, the ratio running on past 1 as the shader has it and the output
//! clamping; half-pixel offsets of the full-screen quads are left out.

use glam::Mat4;

/// `da_dof`'s exposed parameters, with the effect file's defaults.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DepthOfField {
    pub focus_distance: f32,
    pub focal_radius: f32,
    pub near_fade: f32,
    pub far_fade: f32,
    pub weight_blur: f32,
    pub artifact_blur: f32,
    pub max_blur: f32,
}

impl Default for DepthOfField {
    fn default() -> Self {
        DepthOfField {
            focus_distance: 5.0,
            focal_radius: 2.0,
            near_fade: 1.0,
            far_fade: 1.0,
            weight_blur: 2.0,
            artifact_blur: 1.3,
            max_blur: 1.0,
        }
    }
}

impl DepthOfField {
    /// From the values a cutscene gives the effect's parameters (by name; those it leaves
    /// out keep the file's defaults).
    pub fn from_parameters(get: impl Fn(&str) -> Option<f32>) -> Self {
        let d = DepthOfField::default();
        let v = |name, default: f32| get(name).filter(|v| v.is_finite()).unwrap_or(default);
        DepthOfField {
            focus_distance: v("FocusDistance", d.focus_distance),
            focal_radius: v("FocalRadius", d.focal_radius),
            near_fade: v("NearFade", d.near_fade),
            far_fade: v("FarFade", d.far_fade),
            weight_blur: v("WeightBlur", d.weight_blur),
            artifact_blur: v("ArtifactBlur", d.artifact_blur),
            max_blur: v("MaxBlur", d.max_blur),
        }
    }
}

/// A `da_colorfade` at a moment: its colours, its length and its clock.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColourFade {
    pub start: [f32; 4],
    pub end: [f32; 4],
    pub time_to_fade: f32,
    /// Seconds since the effect started.
    pub time: f32,
}

impl ColourFade {
    /// The colour laid over the frame, blended by its alpha (`LinearFade.psh`), as the
    /// frame buffer keeps it (each channel within 0 to 1).
    pub fn colour(&self) -> [f32; 4] {
        let ratio = self.time / self.time_to_fade;
        std::array::from_fn(|i| {
            let c = self.start[i] + ratio * (self.end[i] - self.start[i]);
            if c.is_finite() {
                c.clamp(0.0, 1.0)
            } else {
                self.end[i].clamp(0.0, 1.0)
            }
        })
    }
}

/// The effects a cutscene has running at a moment.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CutEffects {
    pub depth_of_field: Option<DepthOfField>,
    /// In the cutscene's order.
    pub fades: Vec<ColourFade>,
}

impl CutEffects {
    pub fn is_empty(&self) -> bool {
        self.depth_of_field.is_none() && self.fades.is_empty()
    }
}

const SHADER: &str = r#"
struct P {
    // The frame's width and height, then FocusDistance − FocalRadius and 1 / NearFade.
    window: vec2<f32>,
    near_edge: f32,
    inv_near: f32,
    // FocusDistance + FocalRadius, 1 / FarFade, WeightBlur, ArtifactBlur.
    far_edge: f32,
    inv_far: f32,
    weight_blur: f32,
    artifact_blur: f32,
    max_blur: f32,
    pad0: f32,
    pad1: f32,
    pad2: f32,
    // Clip space to view space.
    inv_projection: mat4x4<f32>,
    fade: vec4<f32>,
};
@group(0) @binding(0) var<uniform> p: P;
@group(0) @binding(1) var smp: sampler;
@group(0) @binding(2) var src: texture_2d<f32>;
@group(0) @binding(3) var other: texture_2d<f32>;
@group(0) @binding(4) var depth: texture_depth_2d;

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let t = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(t * 2.0 - 1.0, 0.0, 1.0);
}

// The scene's distance from the camera at a texture coordinate (what the game keeps in its
// scene depth texture), from the depth buffer.
fn distance_at(uv: vec2<f32>) -> f32 {
    let size = vec2<f32>(textureDimensions(depth));
    let at = clamp(vec2<i32>(floor(uv * size)), vec2<i32>(0), vec2<i32>(size) - vec2<i32>(1));
    let z = textureLoad(depth, at, 0);
    let ndc = vec4<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0, z, 1.0);
    let view = p.inv_projection * ndc;
    return length(view.xyz / view.w);
}

fn tap(t: texture_2d<f32>, uv: vec2<f32>) -> vec4<f32> {
    return textureSampleLevel(t, smp, uv, 0.0);
}

// CoCDownsample.vsh + CoCDownsample.psh; `src` is the frame.
@fragment
fn coc(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = pos.xy / vec2<f32>(textureDimensions(other));
    let o = vec2<f32>(1.0) / p.window;
    let a = uv - o;
    let b = uv + o * vec2<f32>(1.0, -1.0);
    let down = vec2<f32>(0.0, 0.5 / p.window.y);
    let rgb = (tap(src, a) + tap(src, b) + tap(src, a + down) + tap(src, b + down)).rgb * 0.25;
    let d = vec4<f32>(distance_at(a), distance_at(b), distance_at(a + down), distance_at(b + down));
    let c = (vec4<f32>(p.near_edge) - d) * p.inv_near;
    return vec4<f32>(rgb, max(max(c.x, c.y), max(c.z, c.w)));
}

fn seven(uv: vec2<f32>, step: vec2<f32>) -> vec4<f32> {
    var sum = tap(src, uv - 3.0 * step) * 0.05;
    sum += tap(src, uv - 2.0 * step) * 0.1;
    sum += tap(src, uv - step) * 0.2;
    sum += tap(src, uv) * 0.3;
    sum += tap(src, uv + step) * 0.2;
    sum += tap(src, uv + 2.0 * step) * 0.1;
    sum += tap(src, uv + 3.0 * step) * 0.05;
    return sum;
}

// HorizontalBlur.vsh + TwoPassBlurCoCHorizontl.psh; `src` is the downsampled target.
@fragment
fn blur_h(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let size = vec2<f32>(textureDimensions(src));
    let uv = pos.xy / size;
    let sum = seven(uv, vec2<f32>(p.weight_blur / size.x, 0.0));
    let sharp = tap(src, uv).a;
    return vec4<f32>(sum.rgb, max(sum.a, sharp) * 2.0 - sharp);
}

// VerticalBlur.vsh + TwoPassBlurCoCVertical.psh; `src` is the horizontal blur, `other` the
// downsampled target.
@fragment
fn blur_v(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let size = vec2<f32>(textureDimensions(src));
    let uv = pos.xy / size;
    let sum = seven(uv, vec2<f32>(0.0, p.weight_blur / size.y));
    let sharp = tap(other, uv).a;
    return vec4<f32>(sum.rgb, max(sum.a, sharp) * 2.0 - sharp);
}

// SmallBlur.vsh + SmallBlur.psh.
@fragment
fn small(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let size = vec2<f32>(textureDimensions(src));
    let uv = pos.xy / size;
    let o = 0.5 * p.artifact_blur / size;
    return (tap(src, uv - o) + tap(src, uv + vec2<f32>(o.x, -o.y))
        + tap(src, uv + vec2<f32>(-o.x, o.y)) + tap(src, uv + o)) * 0.25;
}

// ComposeDoF.vsh + ComposeDoF.psh; `src` is the blurred target. Blended by its alpha.
@fragment
fn compose(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = pos.xy / p.window;
    let far = (distance_at(uv) - p.far_edge) * p.inv_far;
    let blurred = tap(src, uv);
    return vec4<f32>(blurred.rgb, clamp(max(blurred.a, far) * p.max_blur, 0.0, 1.0));
}

// PassThrough.vsh + LinearFade.psh, the colour worked out on the CPU. Blended by its alpha.
@fragment
fn fade(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    return p.fade;
}
"#;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    window: [f32; 2],
    near_edge: f32,
    inv_near: f32,
    far_edge: f32,
    inv_far: f32,
    weight_blur: f32,
    artifact_blur: f32,
    max_blur: f32,
    pad: [f32; 3],
    inv_projection: [[f32; 4]; 4],
    fade: [f32; 4],
}

/// The effects' pipelines and targets, for frames of one size and format.
pub struct CutEffectsPass {
    size: (u32, u32),
    format: wgpu::TextureFormat,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// One uniform block per fade drawn in a frame, then the depth of field's.
    uniforms: Vec<wgpu::Buffer>,
    copy: wgpu::Texture,
    copy_view: wgpu::TextureView,
    /// The half-size targets: the downsampled frame with its circle of confusion, and the
    /// two the blurs go between.
    half: [wgpu::TextureView; 3],
    coc: wgpu::RenderPipeline,
    blur_h: wgpu::RenderPipeline,
    blur_v: wgpu::RenderPipeline,
    small: wgpu::RenderPipeline,
    compose: wgpu::RenderPipeline,
    fade: wgpu::RenderPipeline,
}

/// How many fades a frame draws at most (a cutscene has one or two running).
const MAX_FADES: usize = 4;

impl CutEffectsPass {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat, size: (u32, u32)) -> Self {
        let texture = |label, w: u32, h: u32, usage| {
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
        let sampled = wgpu::TextureUsages::TEXTURE_BINDING;
        let copy = texture(
            "cut effects frame copy",
            size.0,
            size.1,
            sampled | wgpu::TextureUsages::COPY_DST,
        );
        let copy_view = copy.create_view(&Default::default());
        let half = ["dof coc", "dof blur a", "dof blur b"].map(|label| {
            texture(
                label,
                size.0 / 2,
                size.1 / 2,
                sampled | wgpu::TextureUsages::RENDER_ATTACHMENT,
            )
            .create_view(&Default::default())
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor::default());
        let tex = |binding, sample_type| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type,
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let float = wgpu::TextureSampleType::Float { filterable: true };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("cut effects"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                tex(2, float),
                tex(3, float),
                tex(4, wgpu::TextureSampleType::Depth),
            ],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("cut effects"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("cut effects"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = |entry: &str, blend: Option<wgpu::BlendState>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
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
                multiview_mask: None,
                cache: None,
            })
        };
        // ComposeDoF.sta: source alpha over inverse source alpha, alpha as colour.
        // PPQuadState.sta: the same for colour; alpha one over inverse source alpha.
        let over = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::SrcAlpha,
            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
            operation: wgpu::BlendOperation::Add,
        };
        let quad = wgpu::BlendState {
            color: over,
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                ..over
            },
        };
        let uniforms = (0..=MAX_FADES)
            .map(|_| {
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("cut effects"),
                    size: std::mem::size_of::<Params>() as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                })
            })
            .collect();
        CutEffectsPass {
            size,
            format,
            sampler,
            uniforms,
            copy,
            copy_view,
            half,
            coc: pipeline("coc", None),
            blur_h: pipeline("blur_h", None),
            blur_v: pipeline("blur_v", None),
            small: pipeline("small", None),
            compose: pipeline(
                "compose",
                Some(wgpu::BlendState {
                    color: over,
                    alpha: over,
                }),
            ),
            fade: pipeline("fade", Some(quad)),
            layout,
        }
    }

    /// Whether it was made for frames of this size and format.
    pub fn fits(&self, format: wgpu::TextureFormat, size: (u32, u32)) -> bool {
        self.format == format && self.size == size
    }

    fn bindings(
        &self,
        device: &wgpu::Device,
        uniform: &wgpu::Buffer,
        src: &wgpu::TextureView,
        other: &wgpu::TextureView,
        depth: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        let view = |binding, v| wgpu::BindGroupEntry {
            binding,
            resource: wgpu::BindingResource::TextureView(v),
        };
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("cut effects"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                view(2, src),
                view(3, other),
                view(4, depth),
            ],
        })
    }

    fn pass(
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        pipeline: &wgpu::RenderPipeline,
        bindings: &wgpu::BindGroup,
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("cut effect"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, bindings, &[]);
        pass.draw(0..3, 0..1);
    }

    /// Records `da_dof` over `target`, a frame drawn with `projection` whose depth buffer
    /// is `depth` (which must allow sampling).
    #[allow(clippy::too_many_arguments)]
    pub fn depth_of_field(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::Texture,
        depth: &wgpu::TextureView,
        projection: Mat4,
        dof: &DepthOfField,
    ) {
        let params = Params {
            window: [self.size.0.max(1) as f32, self.size.1.max(1) as f32],
            near_edge: dof.focus_distance - dof.focal_radius,
            // The vertex shaders keep the fades from zero.
            inv_near: 1.0 / dof.near_fade.max(0.001),
            far_edge: dof.focus_distance + dof.focal_radius,
            inv_far: 1.0 / dof.far_fade.max(0.001),
            weight_blur: dof.weight_blur,
            artifact_blur: dof.artifact_blur,
            max_blur: dof.max_blur,
            pad: [0.0; 3],
            inv_projection: projection.inverse().to_cols_array_2d(),
            fade: [0.0; 4],
        };
        let uniform = &self.uniforms[MAX_FADES];
        queue.write_buffer(uniform, 0, bytemuck::bytes_of(&params));
        encoder.copy_texture_to_texture(
            target.as_image_copy(),
            self.copy.as_image_copy(),
            wgpu::Extent3d {
                width: self.size.0.max(1),
                height: self.size.1.max(1),
                depth_or_array_layers: 1,
            },
        );
        let [coc, a, b] = &self.half;
        let bind = |src, other| self.bindings(device, uniform, src, other, depth);
        // `other` only sizes the first pass's target; the copy stands in elsewhere.
        Self::pass(encoder, coc, &self.coc, &bind(&self.copy_view, a));
        Self::pass(encoder, a, &self.blur_h, &bind(coc, &self.copy_view));
        Self::pass(encoder, b, &self.blur_v, &bind(a, coc));
        Self::pass(encoder, a, &self.small, &bind(b, &self.copy_view));
        let view = target.create_view(&Default::default());
        Self::pass(encoder, &view, &self.compose, &bind(a, &self.copy_view));
    }

    /// Records `da_colorfade`s over `target`, in order.
    pub fn fades(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::Texture,
        depth: &wgpu::TextureView,
        fades: &[ColourFade],
    ) {
        let view = target.create_view(&Default::default());
        for (f, uniform) in fades.iter().zip(&self.uniforms[..MAX_FADES]) {
            let params = Params {
                window: [self.size.0.max(1) as f32, self.size.1.max(1) as f32],
                near_edge: 0.0,
                inv_near: 0.0,
                far_edge: 0.0,
                inv_far: 0.0,
                weight_blur: 0.0,
                artifact_blur: 0.0,
                max_blur: 0.0,
                pad: [0.0; 3],
                inv_projection: Mat4::IDENTITY.to_cols_array_2d(),
                fade: f.colour(),
            };
            queue.write_buffer(uniform, 0, bytemuck::bytes_of(&params));
            let bindings = self.bindings(device, uniform, &self.copy_view, &self.copy_view, depth);
            Self::pass(encoder, &view, &self.fade, &bindings);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fade_runs_from_its_start_colour_to_its_end_and_holds() {
        // From black: opaque black to clear over 1.2 s, as cutscenes open.
        let f = |time| ColourFade {
            start: [0.0, 0.0, 0.0, 1.0],
            end: [0.0, 0.0, 0.0, 0.0],
            time_to_fade: 1.2,
            time,
        };
        assert_eq!(f(0.0).colour(), [0.0, 0.0, 0.0, 1.0]);
        assert!((f(0.6).colour()[3] - 0.5).abs() < 1e-6);
        assert_eq!(f(1.2).colour()[3], 0.0);
        // Past its length the shader's ratio runs on; the frame buffer clamps.
        assert_eq!(f(5.0).colour()[3], 0.0);
    }

    #[test]
    fn depth_of_field_keeps_the_effect_files_defaults_for_what_is_not_given() {
        let d = DepthOfField::from_parameters(|n| match n {
            "FocusDistance" => Some(2.0),
            "FocalRadius" => Some(8.0),
            _ => None,
        });
        assert_eq!((d.focus_distance, d.focal_radius), (2.0, 8.0));
        assert_eq!((d.near_fade, d.far_fade, d.weight_blur), (1.0, 1.0, 2.0));
        assert_eq!((d.artifact_blur, d.max_blur), (1.3, 1.0));
    }
}
