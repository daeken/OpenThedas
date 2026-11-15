//! The bake itself, following the toolset's lightmapper (see `docs/lightmaps/baker-spec.md`).
//!
//! Each piece's lightmap is rendered three times by the same ray tracer: the lightmap pass
//! (ambient plus the lights' direct light on a white diffuse surface), the shadow pass (white
//! wherever any shadow-casting light arrives) and the occlusion pass (the fraction of five
//! hemisphere rays that escape). Every texel takes eight fixed subpixel samples, which a
//! Gaussian film filter spreads over the texel and its right and lower neighbours. The films
//! are gamma-corrected, truncated to 8 bits and multiplied together into the atlas's RGBA.

use glam::{Vec2, Vec3};

use crate::bvh::Bvh;
use crate::gather::{BakeScene, Region};

/// A light as the toolset's job files give it to the ray tracer.
#[derive(Clone, Debug, PartialEq)]
pub struct BakeLight {
    pub kind: LightKind,
    pub color: Vec3,
    pub brightness: f32,
    /// In the lightmap (RGB) pass.
    pub in_lightmap: bool,
    /// In the shadow (alpha) pass: the toolset's `CastShadows`.
    pub cast_shadows: bool,
    /// The share of a point light's radius the shadow pass reaches (1 unless fitted: some
    /// shipped lights were baked with a smaller radius than the one they ship with).
    pub shadow_reach: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LightKind {
    /// pointlight.dll illuminate (0x10001250): colour × brightness × max(0, 1 − d / radius), no
    /// inverse square.
    Point { position: Vec3, radius: f32 },
    /// directional.dll (0x10001590): `toward` points from the surface to the light; no falloff.
    Directional { toward: Vec3 },
    /// The host's ambient light (eclipseRay 0x403d60): added flat, without shadow or cosine
    /// (directlight.dll 0x10004150).
    Ambient,
}

/// The ray tracer's knobs; the defaults are the toolset's.
#[derive(Clone, Debug)]
pub struct Settings {
    /// The film gamma of the lightmap and occlusion passes (LightMapper.py `GAMMA`); the shadow
    /// pass uses 1.
    pub gamma: f32,
    /// The camera ray's start above the surface (eclipseRay 0x40d040, constant 0x413000).
    pub camera_offset: f32,
    /// Shadow and occlusion ray bias (directlight.dll 0x10007320).
    pub shadow_bias: f32,
    /// Occlusion rays per sample and their length (LightMapper.py: 5 and 5).
    pub ao_rays: u32,
    pub ao_length: f32,
    /// Primary samples per texel (eclipseRay's scene default, 0x405d60).
    pub samples: u32,
    /// Experimental: texels to grow each part's covered area by (see [`Films::dilate`]).
    pub dilate: u32,
    /// Instead of the toolset's few rays per sample, a converged estimate with this many rays
    /// (for comparing shapes without the noise).
    pub ao_converged: Option<u32>,
    /// Experimental occlusion model from fitting the shipped atlases (overrides the above).
    pub ao_model: Option<AoModel>,
    /// Experimental soft shadows for point lights: (sphere radius, visibility samples).
    pub light_size: Option<(f32, u32)>,
    pub shadow_model: ShadowModel,
    pub threads: usize,
}

/// An occlusion model fitted to the shipped atlases: 64 cosine rays out to `length`; a hit at
/// distance t occludes (1 − t/length)^`exponent`; with v the unoccluded share, the pass value
/// is `hi`·clamp(lo + (1 − lo)·v, 0, 1)^`power` after the film's gamma.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AoModel {
    pub length: f32,
    pub exponent: f32,
    pub lo: f32,
    pub power: f32,
    /// The value of a fully open texel.
    pub hi: f32,
}

/// What the shadow pass records per sample.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ShadowModel {
    /// The toolset's: 1 wherever any shadow-casting light arrives (directlight.dll's
    /// `monochrome`).
    Monochrome,
    /// Experimental, from fitting the shipped atlases: min(1, k · Σ falloff · |cos|)^e over the
    /// shadow-casting lights that arrive, ignoring their colour.
    Intensity { k: f32, e: f32 },
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            gamma: 1.5,
            camera_offset: 0.05,
            shadow_bias: 0.0005,
            ao_rays: 5,
            ao_length: 5.0,
            samples: 8,
            dilate: 0,
            ao_converged: None,
            ao_model: None,
            light_size: None,
            shadow_model: ShadowModel::Monochrome,
            threads: 4,
        }
    }
}

/// One piece's three passes as 8-bit films (what the toolset's TGA files hold), row-major.
#[derive(Clone, Debug)]
pub struct Films {
    pub w: u32,
    pub h: u32,
    pub light: Vec<[u8; 3]>,
    pub shadow: Vec<u8>,
    pub occlusion: Vec<u8>,
    /// Whether any sample reached the texel (a zero-weight texel is black in every film).
    pub covered: Vec<bool>,
}

impl Films {
    /// BakedMapProcessor (0x407b60): RGB = ⌊lm · ao / 255⌋, A = ⌊sm · ao / 255⌋.
    pub fn rgba(&self, i: usize) -> [u8; 4] {
        let ao = self.occlusion[i] as f64 / 255.0;
        let m = |c: u8| (c as f64 * ao) as u8;
        let l = self.light[i];
        [m(l[0]), m(l[1]), m(l[2]), m(self.shadow[i])]
    }
}

/// A region's lightmap image in the atlas.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    /// Top-left atlas texel.
    pub x0: i32,
    pub y0: i32,
    /// The piece's image size.
    pub w: u32,
    pub h: u32,
}

/// Recovers where a region's image sits from its offset and scale: CreateAtlas's `-halftexel`
/// (0x40beb0) writes offset = (x + ½) / A and scale = w² / (A (w + 1)) for a w-texel image at
/// texel x of an A-texel atlas.
pub fn placement(offset: Vec2, scale: Vec2, atlas_w: u32, atlas_h: u32) -> Option<Placement> {
    let axis = |off: f32, sc: f32, size: u32| -> Option<(i32, u32)> {
        let texels = sc * size as f32;
        let n = (1..=4096u32).find(|&n| {
            let f = (n * n) as f32 / (n + 1) as f32;
            (f - texels).abs() <= 2e-3 * texels.max(1.0)
        })?;
        Some(((off * size as f32 - 0.5).round() as i32, n))
    };
    let (x0, w) = axis(offset.x, scale.x, atlas_w)?;
    let (y0, h) = axis(offset.y, scale.y, atlas_h)?;
    Some(Placement { x0, y0, w, h })
}

/// FNV-1a over the four little-endian bytes of `x` (yafraycore 0x1003d50f).
fn fnv(x: u32) -> u32 {
    let mut h = 0x811c9dc5u32;
    for b in x.to_le_bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(0x01000193);
    }
    h
}

/// The per-pixel sequence offset of texel (x, y) (renderTile 0x1003d360).
pub fn pixel_offset(x: u32, y: u32) -> u32 {
    fnv(y.wrapping_mul(fnv(x)))
}

/// The Larcher–Pillichshammer radical inverse (yafraycore 0x1003d6f2).
pub fn lp(m: u32) -> f32 {
    let (mut v, mut r) = (0x8000_0000u32, 0u32);
    for k in 0..32 {
        if (m >> k) & 1 == 1 {
            r ^= v;
        }
        v |= v >> 1;
    }
    (r as f64 / 4294967296.0) as f32
}

/// The base-2 van der Corput radical inverse (directlight.dll 0x10001270).
pub fn vdc(m: u32) -> f32 {
    (m.reverse_bits() as f64 / 4294967296.0) as f32
}

/// The base-3 Halton radical inverse (directlight.dll 0x100011c0 / 0x10001210).
pub fn halton3(mut m: u32) -> f32 {
    let (mut r, mut f) = (0.0f64, 1.0f64 / 3.0);
    while m > 0 {
        r += f * (m % 3) as f64;
        m /= 3;
        f /= 3.0;
    }
    r as f32
}

/// The film's 16×16 Gaussian table (yafraycore 0x10028de0) for radius 1.
fn gauss_table() -> [[f32; 16]; 16] {
    let mut t = [[0.0; 16]; 16];
    let e6 = (-6.0f32).exp();
    for (k, row) in t.iter_mut().enumerate() {
        for (l, v) in row.iter_mut().enumerate() {
            let a = (k as f32 + 0.5) / 16.0;
            let b = (l as f32 + 0.5) / 16.0;
            *v = ((-6.0 * (a * a + b * b)).exp() - e6).max(0.0);
        }
    }
    t
}

/// One primary sample's results.
#[derive(Clone, Copy)]
struct Sample {
    light: Vec3,
    lit: f32,
    open: f32,
}

/// Bakes one region's films, `w × h` texels.
pub fn bake_region(
    scene: &BakeScene,
    region: &Region,
    w: u32,
    h: u32,
    lights: &[BakeLight],
    settings: &Settings,
) -> Films {
    // Every sample, then the film splats in order (the splat is not separable by rows).
    let n = (w * h) as usize;
    let per = settings.samples as usize;
    let mut samples: Vec<Option<Sample>> = vec![None; n * per];
    let threads = settings.threads.max(1);
    let chunk = (n * per).div_ceil(threads).max(1);
    std::thread::scope(|s| {
        for (c, out) in samples.chunks_mut(chunk).enumerate() {
            s.spawn(move || {
                for (k, slot) in out.iter_mut().enumerate() {
                    let idx = c * chunk + k;
                    let (px, si) = (idx / per, (idx % per) as u32);
                    let (x, y) = ((px as u32) % w, (px as u32) / w);
                    let o = pixel_offset(x, y);
                    let dx = (si as f32 + 0.5) / settings.samples as f32;
                    let dy = lp(si.wrapping_add(o));
                    let uv = Vec2::new((x as f32 + dx) / w as f32, (y as f32 + dy) / h as f32);
                    let q = (settings.ao_rays * si).wrapping_add(o);
                    *slot = sample(scene, region, uv, lights, settings, q);
                }
            });
        }
    });

    let table = gauss_table();
    let mut acc = vec![(Vec3::ZERO, 0.0f32, 0.0f32, 0.0f32); n];
    for (idx, s) in samples.iter().enumerate() {
        let Some(s) = s else { continue };
        let (px, si) = (idx / per, (idx % per) as u32);
        let (x, y) = ((px as u32) % w, (px as u32) / w);
        let dx = (si as f64 + 0.5) / settings.samples as f64;
        let dy = lp(si.wrapping_add(pixel_offset(x, y))) as f64;
        // addSample (yafraycore 0x1002a0d0): offsets R(d − 1) ..= R(d), R(z) = ⌊z + 0.4999…⌋.
        let reach = |d: f64| (d + 0.499999999986) as i64;
        for oy in 0..=reach(dy).min(1) {
            for ox in 0..=reach(dx).min(1) {
                let (tx, ty) = (x as i64 + ox, y as i64 + oy);
                if tx >= w as i64 || ty >= h as i64 {
                    continue;
                }
                let k = ((ox as f64 - (dx - 0.5)).abs() * 15.9984) as usize;
                let l = ((oy as f64 - (dy - 0.5)).abs() as f32 * 15.9984) as usize;
                let g = table[k.min(15)][l.min(15)];
                if g <= 0.0 {
                    continue;
                }
                let a = &mut acc[(ty as u32 * w + tx as u32) as usize];
                a.0 += s.light * g;
                a.1 += s.lit * g;
                a.2 += s.open * g;
                a.3 += g;
            }
        }
    }
    let e = 1.0f32 / settings.gamma;
    let film = |c: f32, gamma: bool| -> u8 {
        let c = c.max(0.0);
        let c = if gamma { c.powf(e) } else { c };
        (255.0f64 * c.clamp(0.0, 1.0) as f64) as u8
    };
    let mut films = Films {
        w,
        h,
        light: vec![[0; 3]; n],
        shadow: vec![0; n],
        occlusion: vec![0; n],
        covered: vec![false; n],
    };
    for (i, a) in acc.iter().enumerate() {
        if a.3 <= 0.0 {
            continue;
        }
        let inv = 1.0 / a.3;
        let l = a.0 * inv;
        films.light[i] = [film(l.x, true), film(l.y, true), film(l.z, true)];
        films.shadow[i] = film(a.1 * inv, false);
        films.occlusion[i] = film(a.2 * inv, true);
        films.covered[i] = true;
    }
    for _ in 0..settings.dilate {
        films.dilate();
    }
    films
}

impl Films {
    /// Grows the covered area by one texel: each uncovered texel next to covered ones (of its
    /// eight neighbours) takes their rounded mean. Experimental: the shipped atlases are covered
    /// one texel beyond what the toolset's film reaches.
    pub fn dilate(&mut self) {
        let (w, h) = (self.w as i64, self.h as i64);
        let old = self.clone();
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) as usize;
                if old.covered[i] {
                    continue;
                }
                let (mut n, mut l, mut sm, mut ao) = (0u32, [0u32; 3], 0u32, 0u32);
                for dy in -1..=1 {
                    for dx in -1..=1 {
                        let (nx, ny) = (x + dx, y + dy);
                        if nx < 0 || ny < 0 || nx >= w || ny >= h {
                            continue;
                        }
                        let j = (ny * w + nx) as usize;
                        if !old.covered[j] {
                            continue;
                        }
                        n += 1;
                        for (acc, &v) in l.iter_mut().zip(&old.light[j]) {
                            *acc += v as u32;
                        }
                        sm += old.shadow[j] as u32;
                        ao += old.occlusion[j] as u32;
                    }
                }
                if n == 0 {
                    continue;
                }
                let avg = |v: u32| ((v + n / 2) / n) as u8;
                self.light[i] = [avg(l[0]), avg(l[1]), avg(l[2])];
                self.shadow[i] = avg(sm);
                self.occlusion[i] = avg(ao);
                self.covered[i] = true;
            }
        }
    }
}

fn sample(
    scene: &BakeScene,
    region: &Region,
    uv: Vec2,
    lights: &[BakeLight],
    settings: &Settings,
    q: u32,
) -> Option<Sample> {
    let (sp, ng, nf) = surface(scene, region, uv, settings)?;
    let mut light = Vec3::ZERO;
    let mut lit = false;
    let mut reach = 0.0f32;
    for l in lights {
        let c = l.color * l.brightness;
        let (dir, c, tmax, f) = match l.kind {
            LightKind::Ambient => {
                if l.in_lightmap {
                    light += c;
                }
                lit |= l.cast_shadows && c.max_element() > 0.0;
                continue;
            }
            LightKind::Point { position, radius } => {
                let to = position - sp;
                let d = to.length();
                if d == 0.0 {
                    continue;
                }
                let f = (1.0 - d / radius).max(0.0);
                (to / d, c * f, d, f)
            }
            LightKind::Directional { toward } => {
                (toward.normalize_or_zero(), c, f32::INFINITY, 1.0)
            }
        };
        // The RGB pass needs a positive colour; the shadow pass only the light's reach (its
        // brightness there is the shadow job's own, not the RGB pass's fitted one).
        let rgb = l.in_lightmap && c.max_element() > 0.0;
        if f <= 0.0 || nf.dot(dir) <= 0.0 || !(rgb || l.cast_shadows) {
            continue;
        }
        let vis = match (l.kind, settings.light_size) {
            (LightKind::Point { position, .. }, Some((size, n))) if size > 0.0 => {
                soft_visibility(&scene.bvh, sp, position, size, n, settings)
            }
            _ => {
                if shadowed(&scene.bvh, sp, dir, tmax, settings) {
                    0.0
                } else {
                    1.0
                }
            }
        };
        if vis <= 0.0 {
            continue;
        }
        if rgb {
            light += c * ng.dot(dir).abs() * vis;
        }
        lit |= l.cast_shadows && f > 1.0 - l.shadow_reach;
        if l.cast_shadows {
            reach += f * ng.dot(dir).abs() * vis;
        }
    }
    let open = match (settings.ao_model, settings.ao_converged) {
        (Some(m), _) => {
            let hits = occlusion_hits(&scene.bvh, sp, nf, settings, 64, m.length, true, 0.0);
            let occ: f32 = hits
                .iter()
                .map(|&t| {
                    if t >= m.length {
                        0.0
                    } else {
                        (1.0 - t / m.length).powf(m.exponent)
                    }
                })
                .sum::<f32>()
                / hits.len() as f32;
            // The film raises the pass to 1/gamma; undo that so `power` is the final one.
            (m.hi
                * (m.lo + (1.0 - m.lo) * (1.0 - occ))
                    .clamp(0.0, 1.0)
                    .powf(m.power))
            .powf(settings.gamma)
        }
        (None, Some(n)) => occlusion_converged(&scene.bvh, sp, ng, nf, settings, n),
        (None, None) => occlusion(&scene.bvh, sp, ng, nf, settings, q),
    };
    Some(Sample {
        light,
        lit: match settings.shadow_model {
            ShadowModel::Monochrome => {
                if lit {
                    1.0
                } else {
                    0.0
                }
            }
            ShadowModel::Intensity { k, e } => (k * reach).min(1.0).powf(e),
        },
        open,
    })
}

/// The surface the camera ray finds for a lightmap uv: the point, its geometric normal and that
/// normal faced toward the camera (shinydiffuse's face-forward).
pub fn surface(
    scene: &BakeScene,
    region: &Region,
    uv: Vec2,
    settings: &Settings,
) -> Option<(Vec3, Vec3, Vec3)> {
    // The piece's surface under this sample (provisional: the first triangle containing it;
    // the camera's strip table, eclipseRay 0x40dd10, differs only on shared edges and overlaps).
    let (p, n) = region.tris.iter().find_map(|(pos, tuv)| {
        let (b1, b2) = barycentric(uv, tuv)?;
        let p = pos[0] + (pos[1] - pos[0]) * b1 + (pos[2] - pos[0]) * b2;
        let n = (pos[1] - pos[0]).cross(pos[2] - pos[0]).normalize_or_zero();
        (n != Vec3::ZERO).then_some((p, n))
    })?;
    // The camera ray: from just above, straight down onto whatever surface is there.
    let from = p + n * settings.camera_offset;
    let hit = scene.bvh.closest(from, -n, 0.0, f32::MAX)?;
    let sp = from - n * hit.t;
    let ng = scene.bvh.tris[hit.tri as usize].normal();
    let nf = if ng.dot(n) < 0.0 { -ng } else { ng };
    Some((sp, ng, nf))
}

/// A converged occlusion estimate: `count` cosine-distributed rays (a Hammersley set).
pub fn occlusion_converged(
    bvh: &Bvh,
    p: Vec3,
    _ng: Vec3,
    nf: Vec3,
    settings: &Settings,
    count: u32,
) -> f32 {
    let a = if nf.x.abs() > 0.9 { Vec3::Y } else { Vec3::X };
    let t = nf.cross(a).normalize();
    let b = nf.cross(t);
    let mut open = 0;
    for i in 0..count {
        let u1 = (i as f32 + 0.5) / count as f32;
        let phi = std::f32::consts::TAU * vdc(i);
        let dir = nf * u1.sqrt() + (t * phi.cos() + b * phi.sin()) * (1.0 - u1).sqrt();
        if !shadowed(bvh, p, dir, settings.ao_length, settings) {
            open += 1;
        }
    }
    open as f32 / count as f32
}

#[allow(clippy::too_many_arguments)]
/// Occlusion ray hit distances (∞ for an escape) for `count` rays out to `length`, cosine
/// distributed (or uniform over the hemisphere), for fitting occlusion models.
pub fn occlusion_hits(
    bvh: &Bvh,
    p: Vec3,
    nf: Vec3,
    settings: &Settings,
    count: u32,
    length: f32,
    cosine: bool,
    cone_cos: f32,
) -> Vec<f32> {
    let a = if nf.x.abs() > 0.9 { Vec3::Y } else { Vec3::X };
    let t = nf.cross(a).normalize();
    let b = nf.cross(t);
    let bias = settings.shadow_bias;
    (0..count)
        .map(|i| {
            let u1 = (i as f32 + 0.5) / count as f32;
            let phi = std::f32::consts::TAU * vdc(i);
            // Within the cone z ≥ cone_cos: cosine-weighted (z² uniform) or uniform in solid
            // angle (z uniform).
            let z = if cosine {
                (cone_cos * cone_cos + (1.0 - cone_cos * cone_cos) * u1).sqrt()
            } else {
                cone_cos + (1.0 - cone_cos) * u1
            };
            let r = (1.0 - z * z).max(0.0).sqrt();
            let dir = nf * z + (t * phi.cos() + b * phi.sin()) * r;
            bvh.closest_casting(p + dir * bias, dir, 0.0, length - 2.0 * bias)
                .map(|h| h.t + bias)
                .unwrap_or(f32::INFINITY)
        })
        .collect()
}

/// Experimental soft shadows: the share of `n` points spread over a sphere of radius `size`
/// around the light (a Fibonacci set) that the surface point sees.
pub fn soft_visibility(
    bvh: &Bvh,
    p: Vec3,
    light: Vec3,
    size: f32,
    n: u32,
    settings: &Settings,
) -> f32 {
    let n = n.max(1);
    let golden = std::f32::consts::PI * (3.0 - 5.0f32.sqrt());
    let mut seen = 0;
    for i in 0..n {
        let z = 1.0 - 2.0 * (i as f32 + 0.5) / n as f32;
        let r = (1.0 - z * z).max(0.0).sqrt();
        let a = golden * i as f32;
        let q = light + Vec3::new(r * a.cos(), r * a.sin(), z) * size;
        let to = q - p;
        let d = to.length();
        if d > 0.0 && !shadowed(bvh, p, to / d, d, settings) {
            seen += 1;
        }
    }
    seen as f32 / n as f32
}

/// `scene_t::isShadowed` (yafraycore 0x10022e90): the origin moves `b` along the ray and hits
/// count between `b` and `tmax − b`.
pub fn shadowed(bvh: &Bvh, p: Vec3, dir: Vec3, tmax: f32, settings: &Settings) -> bool {
    let b = settings.shadow_bias;
    bvh.occluded(p + dir * b, dir, 0.0, tmax - 2.0 * b)
}

/// `directLighting_t::sampleAO` (directlight.dll 0x100015a0) with the white diffuse material's
/// cosine sampling (shinydiffuse.dll 0x10002810): the fraction of rays that escape.
fn occlusion(bvh: &Bvh, p: Vec3, ng: Vec3, nf: Vec3, settings: &Settings, q: u32) -> f32 {
    let count = settings.ao_rays.max(1);
    // The tangent frame from the geometric normal (yafraycore 0x10011280).
    let (nu, nv) = if ng.x == 0.0 && ng.y == 0.0 {
        let s = if ng.z >= 0.0 { 1.0 } else { -1.0 };
        (Vec3::new(s, 0.0, 0.0), Vec3::Y)
    } else {
        let nu = Vec3::new(ng.y, -ng.x, 0.0) / (ng.x * ng.x + ng.y * ng.y).sqrt();
        (nu, ng.cross(nu))
    };
    let mut open = 0;
    for k in 0..count {
        let s1 = vdc(q.wrapping_add(k));
        let s2 = halton3(q.wrapping_add(k));
        let phi = std::f32::consts::TAU * s2;
        let dir = nf * s1.sqrt() + (nu * phi.cos() + nv * phi.sin()) * (1.0 - s1).sqrt();
        if !shadowed(bvh, p, dir, settings.ao_length, settings) {
            open += 1;
        }
    }
    open as f32 / count as f32
}

/// Explains one sample at `uv` for debugging: the surface, each light's term and what blocks
/// the occlusion rays (64 of them, cosine-distributed).
pub fn probe(
    scene: &BakeScene,
    region: &Region,
    uv: Vec2,
    lights: &[BakeLight],
    settings: &Settings,
    name: &dyn Fn(u32) -> String,
) -> String {
    use std::fmt::Write;
    let mut s = String::new();
    let Some((p, n)) = region.tris.iter().find_map(|(pos, tuv)| {
        let (b1, b2) = barycentric(uv, tuv)?;
        let p = pos[0] + (pos[1] - pos[0]) * b1 + (pos[2] - pos[0]) * b2;
        let n = (pos[1] - pos[0]).cross(pos[2] - pos[0]).normalize_or_zero();
        Some((p, n))
    }) else {
        return "no triangle".into();
    };
    let from = p + n * settings.camera_offset;
    let Some(hit) = scene.bvh.closest(from, -n, 0.0, f32::MAX) else {
        return format!("P {p} N {n}: camera ray hits nothing");
    };
    let sp = from - n * hit.t;
    let tri = scene.bvh.tris[hit.tri as usize];
    let ng = tri.normal();
    let nf = if ng.dot(n) < 0.0 { -ng } else { ng };
    let _ = writeln!(
        s,
        "P {p} N {n} -> hit draw {} {} at t {:.4} Ng {ng}",
        tri.draw,
        name(tri.draw),
        hit.t
    );
    for l in lights {
        if let LightKind::Directional { toward } = l.kind {
            let dir = toward.normalize_or_zero();
            let b = settings.shadow_bias;
            let hit = scene
                .bvh
                .closest_casting(sp + dir * b, dir, 0.0, f32::INFINITY);
            let _ = writeln!(
                s,
                "  sun toward {dir} cos {:.3} blocked by {}",
                nf.dot(dir),
                hit.map(|h| format!(
                    "{} at {:.2}",
                    name(scene.bvh.tris[h.tri as usize].draw),
                    h.t
                ))
                .unwrap_or_else(|| "nothing".into())
            );
        }
        if let LightKind::Point { position, radius } = l.kind {
            let to = position - sp;
            let d = to.length();
            let dir = to / d;
            let blocked = shadowed(&scene.bvh, sp, dir, d, settings);
            let _ = writeln!(
                s,
                "  point {position} r {radius:.2} d {d:.2} atten {:.3} cos {:.3} blocked {blocked} col {}",
                (1.0 - d / radius).max(0.0),
                nf.dot(dir),
                l.color
            );
        }
    }
    let (t, b) = {
        let a = if nf.x.abs() > 0.9 { Vec3::Y } else { Vec3::X };
        let t = nf.cross(a).normalize();
        (t, nf.cross(t))
    };
    let mut open = 0;
    let mut blockers: std::collections::BTreeMap<u32, (u32, f32)> = Default::default();
    for i in 0..64u32 {
        let u1 = (i as f32 + 0.5) / 64.0;
        let phi = std::f32::consts::TAU * vdc(i);
        let dir = nf * u1.sqrt() + (t * phi.cos() + b * phi.sin()) * (1.0 - u1).sqrt();
        let o = sp + dir * settings.shadow_bias;
        match scene
            .bvh
            .closest(o, dir, 0.0, settings.ao_length - 2.0 * settings.shadow_bias)
        {
            None => open += 1,
            Some(h) => {
                let e = blockers
                    .entry(scene.bvh.tris[h.tri as usize].draw)
                    .or_insert((0, f32::MAX));
                e.0 += 1;
                e.1 = e.1.min(h.t);
            }
        }
    }
    let _ = writeln!(s, "  AO open {open}/64; blockers (draw: rays, nearest):");
    for (d, (c, t)) in blockers {
        let _ = writeln!(s, "    {d} {}: {c} rays, {t:.2}", name(d));
    }
    s
}

/// Barycentric weights of `tuv[1]` and `tuv[2]` at `p`, if it is inside the triangle.
fn barycentric(p: Vec2, tuv: &[Vec2; 3]) -> Option<(f32, f32)> {
    let e1 = tuv[1] - tuv[0];
    let e2 = tuv[2] - tuv[0];
    let det = e1.perp_dot(e2);
    if det == 0.0 {
        return None;
    }
    let d = p - tuv[0];
    let b1 = d.perp_dot(e2) / det;
    let b2 = e1.perp_dot(d) / det;
    (b1 >= 0.0 && b2 >= 0.0 && b1 + b2 <= 1.0).then_some((b1, b2))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovers_half_texel_placements() {
        // A 128-texel image at x 128 of a 1024 atlas, a 16-texel one at y 384 of a 512 atlas.
        let off = Vec2::new((128.0 + 0.5) / 1024.0, (384.0 + 0.5) / 512.0);
        let sc = Vec2::new(128.0 * 128.0 / 129.0 / 1024.0, 256.0 / 17.0 / 512.0);
        let p = placement(off, sc, 1024, 512).unwrap();
        assert_eq!(
            p,
            Placement {
                x0: 128,
                y0: 384,
                w: 128,
                h: 16
            }
        );
    }

    #[test]
    fn sequences() {
        assert_eq!(vdc(1), 0.5);
        assert_eq!(vdc(3), 0.75);
        assert!((halton3(1) - 1.0 / 3.0).abs() < 1e-7);
        assert!((halton3(5) - (2.0 / 3.0 + 1.0 / 9.0)).abs() < 1e-7);
        assert_eq!(lp(0), 0.0);
        assert_eq!(lp(1), 0.5);
        assert_eq!(lp(2), 0.75);
        assert_eq!(lp(3), 0.25);
    }

    #[test]
    fn combines_passes_like_baked_map_processor() {
        let f = Films {
            w: 1,
            h: 1,
            light: vec![[255, 100, 3]],
            shadow: vec![255],
            occlusion: vec![128],
            covered: vec![true],
        };
        // 100 · 128/255 = 50.19…, truncated.
        assert_eq!(f.rgba(0), [128, 50, 1, 128]);
    }
}
