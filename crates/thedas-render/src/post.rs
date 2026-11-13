//! A level's frame buffer effects: the passes the original runs over each finished frame of
//! the world, before the interface.
//!
//! A level's layout lists its effects ([`thedas_formats::level::FrameEffect`]); each names a
//! `.ffx.xml` effect and gives values for its exposed parameters. `da_bloom` (on all but five
//! of the 216 layouts), `da_hsvmatrix` (204 layouts, identity on all but about sixteen) and
//! `da_fade` (the Fade's layouts) are drawn here; `da_dof`, which no layout lists, is
//! drawn for cutscenes ([`crate::cut_effects`]).
//!
//! Order: the effects run by their `.ffx.xml` `layer` (`da_dof` 5, `da_bloom` 10, `da_fade`
//! 10, `da_hsvmatrix` 20), effects of one layer in the order the layout lists them (see
//! [`order`]). Each reads the frame the one before left, as their `Finish` copies keep it
//! (`copy BACKBUFFER to BackBufferCopy`); here each effect takes that copy as it starts,
//! which is the same frame. So on `oth000d`, which lists bloom, hsvmatrix, fade: bloom,
//! then fade, then the matrix. Provisional: the order within a layer (a stable sort on the
//! layer, keeping the list's order) is the one the effects' data implies, not traced in
//! the executable's effect manager; no layout seen puts bloom after fade, so the tie only
//! ever orders bloom before fade.
//!
//! `da_hsvmatrix` (`hslmatrix.mat.xml`) runs `HSLMatrix.vsh`/`HSLMatrix.psh` over the frame:
//! the vertex shader turns the effect's Hue, Saturation, Brightness and Contrast into three
//! rows (see [`HsvMatrix::rows`]), and the pixel shader looks each channel's
//! `dot((r, g, b, 1), row)` up in the `ColorCurve` texture (point filtered, clamped, at
//! `v = 0.5`), taking that channel of the texel; alpha is 1. The engine binds `ColorCurve`
//! (usage `ColorCurveTexture`, 0x4f8d65 → 0x4560d0) to an override when one is set (0x458990,
//! from a game object's resource name, 0x67b1b0) and otherwise to `da_defaultcc` (loaded at
//! 0x458cda), an identity ramp in the data: with the default curve and a level's identity
//! values (Hue 0, Saturation 1, Brightness 1, Contrast 1) the pass changes nothing.
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
//! `da_fade` (`da_fade.ffx.xml`, layer 10, `minlod` 1; `gaussianblur.mat.xml`) runs two
//! filters over the frame, each mirrored from its shader (see [`Fade`]):
//!
//! 1. "Radial Blur" (semantic `RadialBlur`: `RadialBlur.vsh`/`RadialBlur.psh`,
//!    `GaussianBlur.sta`): seven taps of the frame copy along the line to the screen's
//!    centre, weighted 0.3 (the pixel's own), 0.25, 0.15, 0.12, 0.08, 0.06, 0.04, then mixed
//!    towards their luminance `(0.3, 0.59, 0.11)` by one minus `FadeSaturateMask`'s alpha.
//! 2. "Distortion" (semantic `FadeDistortion`: `PassThrough.vsh`/`FadeDistortion.psh`,
//!    `FadeDistortionBlend.sta`, no blending), over a fresh copy: the average of the pixel
//!    and the pixel at an offset waving with `ApplicationTime`, the offset scaled by one
//!    minus `FadeEffectMask`'s alpha.
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

/// `da_hsvmatrix`'s parameters, as the level gives them (or `da_hsvmatrix.ffx.xml`'s
/// defaults: Hue 0, Saturation 1, Brightness 1, Contrast 1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HsvMatrix {
    /// Degrees.
    pub hue: f32,
    pub saturation: f32,
    pub brightness: f32,
    pub contrast: f32,
}

/// An area's colour grading from `areadata.gda` (the row of the area's `AREA_ID`): its colour
/// curve's name (column hash 0xb3cba486) and the `da_hsvmatrix` values it overrides.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AreaGrade {
    pub curve: Option<String>,
    pub brightness: Option<f32>,
    pub contrast: Option<f32>,
    pub hue: Option<f32>,
    pub saturation: Option<f32>,
}

impl AreaGrade {
    /// Row `area_id` of `areadata` (DAOrigins.exe 0x67afd0). A missing or empty cell leaves
    /// that value unset.
    pub fn from_areadata(areadata: &thedas_formats::gda::Gda, area_id: i64) -> AreaGrade {
        use thedas_formats::gda::Cell;
        let Some(row) = areadata.row_by_id(area_id) else {
            return AreaGrade::default();
        };
        let cell = |hash: u32| {
            let c = areadata.columns.iter().position(|c| c.hash == hash)?;
            row.get(c)
        };
        let float = |hash| match cell(hash)? {
            Cell::Float(v) => Some(*v as f32),
            Cell::Int(v) => Some(*v as f32),
            _ => None,
        };
        AreaGrade {
            curve: match cell(0xb3cb_a486) {
                Some(Cell::String(s)) if !s.trim_matches(['\0', ' ']).is_empty() => {
                    Some(s.trim_matches(['\0', ' ']).to_string())
                }
                _ => None,
            },
            brightness: float(0xc202_deda),
            contrast: float(0x1fc1_7917),
            hue: float(0x8206_386e),
            saturation: float(0xb385_6d57),
        }
    }
}

impl HsvMatrix {
    /// The level's `da_hsvmatrix`, when it lists one.
    pub fn from_effects(effects: &[FrameEffect]) -> Option<HsvMatrix> {
        let e = effects
            .iter()
            .find(|e| e.name.eq_ignore_ascii_case("da_hsvmatrix"))?;
        let v = |name: &str, default: f32| {
            e.parameter(name)
                .filter(|v| v.is_finite())
                .unwrap_or(default)
        };
        Some(HsvMatrix {
            hue: v("Hue", 0.0),
            saturation: v("Saturation", 1.0),
            brightness: v("Brightness", 1.0),
            contrast: v("Contrast", 1.0),
        })
    }

    /// The rows `HSLMatrix.vsh` hands the pixel shader (`TEXCOORD1`-`3`), red, green and
    /// blue: a saturation matrix about the luminance weights `(0.2125, 0.7154, 0.0721)`,
    /// scaled by `contrast · brightness`, turned by `hue` about the grey axis, with
    /// `0.5 (1 − contrast)` added. Transcribed from the shader's arithmetic.
    pub fn rows(&self) -> [[f32; 4]; 3] {
        let lum = [0.2125f32, 0.7154, 0.0721];
        let (s, cb) = (self.saturation, self.contrast * self.brightness);
        // Off-diagonal (r1) and diagonal (r3) saturation terms, scaled.
        let off = lum.map(|l| (1.0 - s) * l * cb);
        let diag = lum.map(|l| ((1.0 - s) * l + s) * cb);
        // The hue angle, wrapped to [-π, π) as the shader's frc does.
        let turn = (self.hue / 360.0 + 0.5).rem_euclid(1.0);
        let a = turn * std::f32::consts::TAU - std::f32::consts::PI;
        let (sin, cos) = a.sin_cos();
        let k = sin * 0.577_350_26;
        let h = [
            (1.0 - cos) / 3.0 + k,
            (1.0 - cos) / 3.0 - k,
            1.0 / 3.0 + 2.0 / 3.0 * cos,
        ];
        let dot = |v: [f32; 3]| v[0] * h[0] + v[1] * h[1] + v[2] * h[2];
        let o = 0.5 * (1.0 - self.contrast);
        // Columns of the saturation matrix as the shader swizzles them into each row.
        let (d, f) = (diag, off);
        [
            [
                dot([f[0], f[0], d[0]]),
                dot([d[1], f[1], f[1]]),
                dot([f[2], d[2], f[2]]),
                o,
            ],
            [
                dot([f[0], d[0], f[0]]),
                dot([f[1], f[1], d[1]]),
                dot([d[2], f[2], f[2]]),
                o,
            ],
            [
                dot([d[0], f[0], f[0]]),
                dot([f[1], d[1], f[1]]),
                dot([f[2], f[2], d[2]]),
                o,
            ],
        ]
    }
}

/// `da_fade`'s parameters, as the level gives them (or `da_fade.ffx.xml`'s default:
/// BlurStrength −3.5; the Fade's layouts give 5).
///
/// The radial blur (`RadialBlur.vsh`) takes `fBlurSize = BlurStrength` and steps
/// `o = d · BlurStrength / (back buffer width, height)` in texture coordinates, its taps at
/// `uv − k·o` for `k` 0 to 6, where `d` is `normalize(uv − 0.5)` *at the full-screen quad's
/// corners*: the vertex shader works it there and the pixels get it interpolated, so across
/// the screen `d = √2 · (uv − 0.5)` — no step at the centre, the longest at the corners (a
/// zoom blur), towards the centre for a positive strength. The mask is read at `uv`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fade {
    pub blur_strength: f32,
}

impl Fade {
    /// The level's `da_fade`, when it lists one.
    pub fn from_effects(effects: &[FrameEffect]) -> Option<Fade> {
        let e = effects
            .iter()
            .find(|e| e.name.eq_ignore_ascii_case("da_fade"))?;
        Some(Fade {
            blur_strength: e
                .parameter("BlurStrength")
                .filter(|v| v.is_finite())
                .unwrap_or(-3.5),
        })
    }

    /// Whether `da_fade` runs at `detail`: its `minlod` is 1, the others' 0. Provisional:
    /// `minlod` read as the Graphics Detail option counted from Low (0), not traced in the
    /// executable.
    pub fn runs_at(detail: crate::scene::Detail) -> bool {
        detail != crate::scene::Detail::Low
    }
}

/// One of the effects drawn here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    Bloom,
    Fade,
    HsvMatrix,
}

impl Effect {
    /// The effect `name` (a `.ffx.xml` id) names, when it is drawn here.
    pub fn named(name: &str) -> Option<Effect> {
        [
            ("da_bloom", Effect::Bloom),
            ("da_fade", Effect::Fade),
            ("da_hsvmatrix", Effect::HsvMatrix),
        ]
        .into_iter()
        .find(|(n, _)| name.eq_ignore_ascii_case(n))
        .map(|(_, e)| e)
    }

    /// The `layer` its `.ffx.xml` gives.
    pub fn layer(self) -> u32 {
        match self {
            Effect::Bloom | Effect::Fade => 10,
            Effect::HsvMatrix => 20,
        }
    }
}

/// The effects `effects` lists that are drawn here, in the order they run: by layer, then
/// in the list's order (see the module docs); each once, where it is first listed.
pub fn order(effects: &[FrameEffect]) -> Vec<Effect> {
    let mut out: Vec<Effect> = Vec::new();
    for e in effects.iter().filter_map(|e| Effect::named(&e.name)) {
        if !out.contains(&e) {
            out.push(e);
        }
    }
    out.sort_by_key(|e| e.layer());
    out
}

/// An RGBA8 image: a mask texture the effects read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    /// `width · height` RGBA texels, rows top down.
    pub rgba: Vec<u8>,
}

impl Image {
    /// A 1×1 white image, for a mask that does not load. Provisional: the material's
    /// fallback is `Default_Missing`, not reproduced; an opaque mask leaves the frame as it
    /// is but for the radial blur.
    pub fn white() -> Image {
        Image {
            width: 1,
            height: 1,
            rgba: vec![255; 4],
        }
    }
}

/// The textures `da_fade` reads: `FadeSaturateMask` (the radial blur's `s2`) and
/// `FadeEffectMask` (the distortion's `s1`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FadeMasks {
    pub saturate: Image,
    pub effect: Image,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct FadeParams {
    /// The back buffer's size (`fWindowWidth`, `fWindowHeight`), which is the frame's.
    window: [f32; 2],
    /// `fBlurSize`.
    blur: f32,
    /// `g_fApplicationTime`, in seconds.
    time: f32,
}

const FADE_SHADER: &str = r#"
struct FadeParams {
    window: vec2<f32>,
    blur: f32,
    time: f32,
};

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var mask: texture_2d<f32>;
@group(0) @binding(2) var smp: sampler;
@group(0) @binding(3) var<uniform> p: FadeParams;

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let t = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(t * 2.0 - 1.0, 0.0, 1.0);
}

// D3D9 samples a full-screen quad's pixel (i, j) at texture coordinate (i, j) / size.
fn d3d_uv(pos: vec4<f32>) -> vec2<f32> {
    return (pos.xy - vec2<f32>(0.5)) / p.window;
}

// The frame copy, linearly filtered and clamped (GaussianBlur.sta, FadeDistortionBlend.sta
// stage 0).
fn frame(uv: vec2<f32>) -> vec3<f32> {
    return textureSample(src, smp, uv).rgb;
}

// RadialBlur.vsh + RadialBlur.psh. The vertex shader's normalize(uv - 0.5) is taken at the
// quad's corners, where it is (±1, ±1) / √2, and interpolated: √2 (uv - 0.5) here. Stage 2
// (the mask) has no state in GaussianBlur.sta: D3D9's defaults, point filtered and wrapped.
@fragment
fn radial(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = d3d_uv(pos);
    let d = (uv - vec2<f32>(0.5)) * 1.4142135;
    let o = d * p.blur / p.window;
    var rgb = frame(uv - o) * 0.25;
    rgb += frame(uv) * 0.3;
    rgb += frame(uv - 2.0 * o) * 0.15;
    rgb += frame(uv - 3.0 * o) * 0.12;
    rgb += frame(uv - 4.0 * o) * 0.08;
    rgb += frame(uv - 5.0 * o) * 0.06;
    rgb += frame(uv - 6.0 * o) * 0.04;
    let lum = dot(rgb, vec3<f32>(0.3, 0.59, 0.11));
    let size = vec2<i32>(textureDimensions(mask));
    let t = vec2<i32>(floor(uv * vec2<f32>(size) + vec2<f32>(1e-3)));
    let m = textureLoad(mask, ((t % size) + size) % size, 0).a;
    return vec4<f32>(mix(vec3<f32>(lum), rgb, m), 1.0);
}

// FadeDistortion.psh's angle reduction: frc, then to [-π, π).
fn wrap(v: f32) -> f32 {
    return fract(v) * 6.28319 - 3.14159;
}

// PassThrough.vsh + FadeDistortion.psh (sincos's .x is the cosine, .y the sine).
@fragment
fn distort(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = d3d_uv(pos);
    let k = 1.0 - textureSample(mask, smp, uv).a;
    let fy = fract(p.time * 0.042 + uv.y);
    let fx = fract(p.time * 0.048 + uv.x);
    let off = vec2<f32>(
        cos(wrap(fy * 0.999811 + 0.5)) * 0.003 + sin(wrap(fy * 39.9925 + 0.5)) * 0.002,
        cos(wrap(fx * 31.994 + 0.5)) * 0.002 + sin(wrap(fx * 0.999811 + 0.5)) * 0.003,
    ) * k;
    return vec4<f32>(0.5 * frame(uv) + 0.5 * frame(uv + off), 1.0);
}
"#;

/// An identity colour curve, for an install without `da_defaultcc`: 256 RGBA texels.
/// Provisional (the engine would bind no texture).
pub fn identity_curve() -> Vec<u8> {
    (0..=255u8).flat_map(|v| [v, v, v, 255]).collect()
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct HsvParams {
    rows: [[f32; 4]; 3],
    /// The frame's size.
    dest: [f32; 2],
    _pad: [f32; 2],
}

const HSV_SHADER: &str = r#"
struct HsvParams {
    rows: array<vec4<f32>, 3>,
    dest: vec2<f32>,
    pad: vec2<f32>,
};

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var curve: texture_2d<f32>;
@group(0) @binding(2) var<uniform> p: HsvParams;
@group(0) @binding(3) var smp: sampler;

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let t = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(t * 2.0 - 1.0, 0.0, 1.0);
}

// The curve texel at u (point filtering, clamped), row 0.
fn lookup(u: f32) -> vec4<f32> {
    let w = i32(textureDimensions(curve).x);
    let t = clamp(i32(floor(u * f32(w) + 1e-4)), 0, w - 1);
    return textureLoad(curve, vec2<i32>(t, 0), 0);
}

// HSLMatrix.psh: each channel looked up through the colour curve. The input is sampled
// linearly (HSVMatrixBlend.sta) at D3D9's (i, j) / size, as the bloom's passes are.
@fragment
fn hsv(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let c = vec4<f32>(textureSample(src, smp, (pos.xy - vec2<f32>(0.5)) / p.dest).rgb, 1.0);
    let r = lookup(dot(c, p.rows[0])).r;
    let g = lookup(dot(c, p.rows[1])).g;
    let b = lookup(dot(c, p.rows[2])).b;
    return vec4<f32>(r, g, b, 1.0);
}
"#;

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

/// `da_hsvmatrix`'s GPU state: its pipeline, layout, uniforms and colour curve.
struct HsvPass {
    matrix: HsvMatrix,
    /// The layout's own parameters, before an area's overrides.
    layout_matrix: HsvMatrix,
    layout: wgpu::BindGroupLayout,
    pipeline: wgpu::RenderPipeline,
    params: wgpu::Buffer,
    curve: wgpu::TextureView,
    /// Reads the frame copy taken after the bloom; rebuilt with the targets.
    group: Option<((u32, u32), wgpu::BindGroup)>,
}

/// `da_fade`'s GPU state: its two pipelines, layout, uniforms and masks.
struct FadePass {
    fade: Fade,
    layout: wgpu::BindGroupLayout,
    radial: wgpu::RenderPipeline,
    distort: wgpu::RenderPipeline,
    params: wgpu::Buffer,
    saturate: wgpu::TextureView,
    effect: wgpu::TextureView,
    /// Read the frame copy: the radial blur's, then the distortion's; rebuilt with the
    /// targets.
    groups: Option<((u32, u32), [wgpu::BindGroup; 2])>,
}

/// The frame buffer effects of one level, drawn over frames of `color_format`.
pub struct FrameEffects {
    /// `da_bloom`'s parameters; with no bloom the bloom passes are skipped.
    pub bloom: Option<Bloom>,
    hsv: Option<HsvPass>,
    fade: Option<FadePass>,
    /// The effects drawn, in the order they run.
    order: Vec<Effect>,
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
    /// `curve` is the colour curve `da_hsvmatrix` reads: RGBA texels of its first row
    /// (`da_defaultcc`, or [`identity_curve`]). `fade_masks` are the textures `da_fade`
    /// reads (white when not given).
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        effects: &[FrameEffect],
        detail: crate::scene::Detail,
        color_format: wgpu::TextureFormat,
        curve: &[u8],
        fade_masks: Option<&FadeMasks>,
    ) -> Option<Self> {
        let mut order = order(effects);
        if !Fade::runs_at(detail) {
            order.retain(|e| *e != Effect::Fade);
        }
        if order.is_empty() {
            return None;
        }
        let bloom = Bloom::from_effects(effects);
        let matrix = HsvMatrix::from_effects(effects);
        let fade = Fade::from_effects(effects).filter(|_| order.contains(&Effect::Fade));
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
        let hsv = matrix.map(|m| hsv_pass(device, queue, m, color_format, curve));
        let fade = fade.map(|f| {
            let white = FadeMasks {
                saturate: Image::white(),
                effect: Image::white(),
            };
            fade_pass(device, queue, f, color_format, fade_masks.unwrap_or(&white))
        });
        Some(FrameEffects {
            bloom,
            hsv,
            fade,
            order,
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
    /// `COPY_SRC` usage) of the given size, in their order (see [`order`]). `time` is the
    /// shaders' clock (`ApplicationTime`), in seconds.
    #[allow(clippy::too_many_arguments)]
    pub fn apply(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::Texture,
        width: u32,
        height: u32,
        time: f32,
    ) {
        if width < 2 || height < 2 {
            return;
        }
        self.make_targets(device, width, height);
        for e in self.order.clone() {
            match e {
                Effect::Bloom => {
                    if let Some(b) = self.bloom {
                        self.apply_bloom(queue, encoder, target, b, width, height);
                    }
                }
                Effect::HsvMatrix => self.apply_hsv(device, queue, encoder, target, width, height),
                Effect::Fade => {
                    self.apply_fade(device, queue, encoder, target, width, height, time)
                }
            }
        }
    }

    /// `da_fade` over the frame as the effects before it left it: the radial blur, a fresh
    /// copy (`CopyBackBuffer`), the distortion.
    #[allow(clippy::too_many_arguments)]
    fn apply_fade(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::Texture,
        width: u32,
        height: u32,
        time: f32,
    ) {
        let Some(f) = self.fade.as_mut() else { return };
        let t = self.targets.as_ref().expect("targets made");
        let p = FadeParams {
            window: [width as f32, height as f32],
            blur: f.fade.blur_strength,
            time,
        };
        queue.write_buffer(&f.params, 0, bytemuck::bytes_of(&p));
        if f.groups.as_ref().map(|(s, _)| *s) != Some(t.size) {
            let copy = t.copy.create_view(&wgpu::TextureViewDescriptor::default());
            let group = |mask: &wgpu::TextureView| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("fade"),
                    layout: &f.layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(&copy),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(mask),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::Sampler(&self.linear),
                        },
                        wgpu::BindGroupEntry {
                            binding: 3,
                            resource: f.params.as_entire_binding(),
                        },
                    ],
                })
            };
            f.groups = Some((t.size, [group(&f.saturate), group(&f.effect)]));
        }
        let frame = target.create_view(&wgpu::TextureViewDescriptor::default());
        let Some(f) = self.fade.as_ref() else { return };
        let groups = &f.groups.as_ref().expect("groups just made").1;
        for (pipeline, group) in [(&f.radial, &groups[0]), (&f.distort, &groups[1])] {
            self.copy_frame(encoder, target);
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("fade"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &frame,
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
            pass.set_bind_group(0, group, &[]);
            pass.draw(0..3, 0..1);
        }
    }

    fn copy_frame(&self, encoder: &mut wgpu::CommandEncoder, target: &wgpu::Texture) {
        let t = self.targets.as_ref().expect("targets made");
        let (width, height) = t.size;
        encoder.copy_texture_to_texture(
            target.as_image_copy(),
            t.copy.as_image_copy(),
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
    }

    /// Gives `da_hsvmatrix` an area's colour grading: its colour curve (RGBA texels of the
    /// first row) and its `Brightness`, `Contrast`, `Hue` and `Saturation` where the area
    /// sets them, over the layout's values (DAOrigins.exe 0x67afd0, 0x67b1b0). Nothing
    /// changes when the layout lists no `da_hsvmatrix`.
    pub fn set_grade(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        curve: &[u8],
        grade: &AreaGrade,
    ) {
        let Some(h) = self.hsv.as_mut() else { return };
        let base = h.layout_matrix;
        h.matrix = HsvMatrix {
            hue: grade.hue.unwrap_or(base.hue),
            saturation: grade.saturation.unwrap_or(base.saturation),
            brightness: grade.brightness.unwrap_or(base.brightness),
            contrast: grade.contrast.unwrap_or(base.contrast),
        };
        h.curve = curve_view(device, queue, curve);
        h.group = None;
    }

    /// `da_hsvmatrix` over the frame as the bloom left it.
    fn apply_hsv(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::Texture,
        width: u32,
        height: u32,
    ) {
        self.copy_frame(encoder, target);
        let t = self.targets.as_ref().expect("targets made");
        let Some(h) = self.hsv.as_mut() else { return };
        let p = HsvParams {
            rows: h.matrix.rows(),
            dest: [width as f32, height as f32],
            _pad: [0.0; 2],
        };
        queue.write_buffer(&h.params, 0, bytemuck::bytes_of(&p));
        if h.group.as_ref().map(|(s, _)| *s) != Some(t.size) {
            let copy = t.copy.create_view(&wgpu::TextureViewDescriptor::default());
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("hsv matrix"),
                layout: &h.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&copy),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&h.curve),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: h.params.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::Sampler(&self.linear),
                    },
                ],
            });
            h.group = Some((t.size, group));
        }
        let frame = target.create_view(&wgpu::TextureViewDescriptor::default());
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("hsv matrix"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &frame,
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
        pass.set_pipeline(&h.pipeline);
        pass.set_bind_group(0, &h.group.as_ref().expect("group just made").1, &[]);
        pass.draw(0..3, 0..1);
    }

    fn apply_bloom(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::Texture,
        b: Bloom,
        width: u32,
        height: u32,
    ) {
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
        self.copy_frame(encoder, target);
        let t = self.targets.as_ref().expect("targets made");
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

/// A colour curve's texture: its RGBA texels as one row (an identity ramp when empty).
fn curve_view(device: &wgpu::Device, queue: &wgpu::Queue, curve: &[u8]) -> wgpu::TextureView {
    let texels = (curve.len() / 4).max(1) as u32;
    let identity;
    let curve = if curve.len() >= 4 {
        &curve[..texels as usize * 4]
    } else {
        identity = identity_curve();
        &identity[..]
    };
    let texels = (curve.len() / 4) as u32;
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("colour curve"),
        size: wgpu::Extent3d {
            width: texels,
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
    queue.write_texture(
        texture.as_image_copy(),
        curve,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(texels * 4),
            rows_per_image: Some(1),
        },
        wgpu::Extent3d {
            width: texels,
            height: 1,
            depth_or_array_layers: 1,
        },
    );
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}

/// Builds `da_hsvmatrix`'s pipeline over frames of `color_format`, with `curve` (RGBA
/// texels) as its colour curve.
fn hsv_pass(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    matrix: HsvMatrix,
    color_format: wgpu::TextureFormat,
    curve: &[u8],
) -> HsvPass {
    let curve = curve_view(device, queue, curve);
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("hsv matrix"),
        source: wgpu::ShaderSource::Wgsl(HSV_SHADER.into()),
    });
    let tex = |binding| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    };
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("hsv matrix"),
        entries: &[
            tex(0),
            tex(1),
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
            wgpu::BindGroupLayoutEntry {
                binding: 3,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("hsv matrix"),
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("hsv matrix"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("hsv"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format: color_format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    });
    let params = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("hsv matrix"),
        size: std::mem::size_of::<HsvParams>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    HsvPass {
        matrix,
        layout_matrix: matrix,
        layout,
        pipeline,
        params,
        curve,
        group: None,
    }
}

/// Uploads `image` as an RGBA8 texture (a white texel when it is empty or short).
fn mask_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    image: &Image,
    label: &str,
) -> wgpu::TextureView {
    let fallback;
    let len = image.width as usize * image.height as usize * 4;
    let image = if image.width > 0 && image.height > 0 && image.rgba.len() >= len {
        image
    } else {
        fallback = Image::white();
        &fallback
    };
    let size = wgpu::Extent3d {
        width: image.width,
        height: image.height,
        depth_or_array_layers: 1,
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        texture.as_image_copy(),
        &image.rgba[..image.width as usize * image.height as usize * 4],
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(image.width * 4),
            rows_per_image: Some(image.height),
        },
        size,
    );
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}

/// Builds `da_fade`'s pipelines over frames of `color_format`, reading `masks`.
fn fade_pass(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    fade: Fade,
    color_format: wgpu::TextureFormat,
    masks: &FadeMasks,
) -> FadePass {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("fade"),
        source: wgpu::ShaderSource::Wgsl(FADE_SHADER.into()),
    });
    let tex = |binding| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    };
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("fade"),
        entries: &[
            tex(0),
            tex(1),
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 3,
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
        label: Some("fade"),
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    let pipeline = |entry: &str| {
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
                    format: color_format,
                    blend: None,
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
    let params = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("fade"),
        size: std::mem::size_of::<FadeParams>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    FadePass {
        fade,
        radial: pipeline("radial"),
        distort: pipeline("distort"),
        layout,
        params,
        saturate: mask_texture(device, queue, &masks.saturate, "fade saturate mask"),
        effect: mask_texture(device, queue, &masks.effect, "fade effect mask"),
        groups: None,
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
    fn fade_shader_validates() {
        let module = naga::front::wgsl::parse_str(FADE_SHADER).expect("parses");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .expect("validates");
    }

    fn effect(name: &str, parameters: &[(&str, f32)]) -> FrameEffect {
        FrameEffect {
            name: name.into(),
            parameters: parameters
                .iter()
                .map(|(n, v)| (n.to_string(), *v))
                .collect(),
        }
    }

    #[test]
    fn fade_takes_the_levels_strength() {
        let f = Fade::from_effects(&[effect("DA_Fade", &[("BlurStrength", 5.0)])]).unwrap();
        assert_eq!(f.blur_strength, 5.0);
        let f = Fade::from_effects(&[effect("da_fade", &[("BlurStrength", f32::NAN)])]).unwrap();
        assert_eq!(f.blur_strength, -3.5);
        assert_eq!(Fade::from_effects(&[effect("da_bloom", &[])]), None);
        assert!(!Fade::runs_at(crate::scene::Detail::Low));
        assert!(Fade::runs_at(crate::scene::Detail::Medium));
    }

    #[test]
    fn effects_run_by_layer_then_list_order() {
        let list =
            |names: &[&str]| -> Vec<FrameEffect> { names.iter().map(|n| effect(n, &[])).collect() };
        // oth000d's list: bloom and fade share layer 10, the matrix is 20.
        assert_eq!(
            order(&list(&["da_bloom", "da_hsvmatrix", "da_fade"])),
            [Effect::Bloom, Effect::Fade, Effect::HsvMatrix]
        );
        assert_eq!(
            order(&list(&[
                "da_hsvmatrix",
                "da_fade",
                "da_dof",
                "da_bloom",
                "da_fade"
            ])),
            [Effect::Fade, Effect::Bloom, Effect::HsvMatrix]
        );
        assert_eq!(order(&list(&["da_dof"])), []);
    }

    #[test]
    fn hsv_shader_validates() {
        let module = naga::front::wgsl::parse_str(HSV_SHADER).expect("parses");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .expect("validates");
    }

    fn apply(rows: &[[f32; 4]; 3], c: [f32; 3]) -> [f32; 3] {
        rows.map(|r| r[0] * c[0] + r[1] * c[1] + r[2] * c[2] + r[3])
    }

    #[test]
    fn hsv_matrix_is_the_identity_at_its_defaults() {
        let m = HsvMatrix::from_effects(&[FrameEffect {
            name: "da_hsvmatrix".into(),
            parameters: vec![("Brightness".into(), 1.0), ("Hue".into(), f32::NAN)],
        }])
        .unwrap();
        assert_eq!(
            m,
            HsvMatrix {
                hue: 0.0,
                saturation: 1.0,
                brightness: 1.0,
                contrast: 1.0
            }
        );
        let rows = m.rows();
        for (i, r) in rows.iter().enumerate() {
            for (j, v) in r.iter().enumerate() {
                let want = if i == j { 1.0 } else { 0.0 };
                assert!((v - want).abs() < 1e-5, "{rows:?}");
            }
        }
        assert_eq!(HsvMatrix::from_effects(&[]), None);
    }

    #[test]
    fn hsv_matrix_desaturates_scales_and_turns() {
        let base = HsvMatrix {
            hue: 0.0,
            saturation: 1.0,
            brightness: 1.0,
            contrast: 1.0,
        };
        // Saturation 0: every channel the luminance.
        let grey = HsvMatrix {
            saturation: 0.0,
            ..base
        }
        .rows();
        let c = apply(&grey, [1.0, 0.0, 0.0]);
        assert!(c.iter().all(|v| (v - 0.2125).abs() < 1e-5), "{c:?}");
        // Brightness scales; contrast scales about 0.5.
        let bright = apply(
            &HsvMatrix {
                brightness: 2.0,
                ..base
            }
            .rows(),
            [0.25; 3],
        );
        assert!(bright.iter().all(|v| (v - 0.5).abs() < 1e-5));
        let flat = apply(
            &HsvMatrix {
                contrast: 0.5,
                ..base
            }
            .rows(),
            [1.0, 0.0, 0.5],
        );
        assert!((flat[0] - 0.75).abs() < 1e-5 && (flat[1] - 0.25).abs() < 1e-5);
        assert!((flat[2] - 0.5).abs() < 1e-5);
        // A hue turn keeps grey grey; at 120 degrees the shader's turn takes red to blue.
        let turned = HsvMatrix { hue: 120.0, ..base }.rows();
        let g = apply(&turned, [0.4; 3]);
        assert!(g.iter().all(|v| (v - 0.4).abs() < 1e-5), "{g:?}");
        let r = apply(&turned, [1.0, 0.0, 0.0]);
        assert!(
            r[2] > 0.99 && r[0].abs() < 1e-5 && r[1].abs() < 1e-5,
            "{r:?}"
        );
        assert_eq!(identity_curve().len(), 1024);
        assert_eq!(&identity_curve()[4 * 200..4 * 201], &[200, 200, 200, 255]);
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
