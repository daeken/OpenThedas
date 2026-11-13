//! A level's atmosphere as the shaders read it: the fog (`mml_vFogParams`, usage
//! `FogParameters`), the sky (`mml_vSkyParams`, usage `SkyParameters`) and the clouds
//! (`mml_vCloudParams`, usage `CloudParameters`).
//!
//! All three are blocks of the atmosphere object that DAOrigins.exe's atmosphere update
//! (0x55bfa0) fills from the level's `ATMO` fields (see
//! [`thedas_formats::level::Atmosphere`]) and the layout's sunlight direction. The engine's
//! usage dispatch (0x4f8a10) hands each usage its array of the environment (0x5397a0 sky,
//! 0x539880 clouds, 0x539810 fog), which the environment update (0x538fa0, from 0x5394b0)
//! copies out of the atmosphere block:
//!
//! - sky: block offsets `0x90, 0xa0, 0xb0, 0xc0` (scattering), `0x110` and the 4x4 at
//!   `0x120` (the celestial body's projection), nine registers;
//! - clouds: `0xd0, 0xe0` (the `CLDS` values) and `0x160` (the celestial body's up axis);
//! - fog: `0xf0, 0x100`.
//!
//! The scattering is Hoffman and Preetham's daylight model, with Preetham's constants:
//!
//! - Rayleigh extinction `βR = (6.95e-6, 1.18e-5, 2.44e-5)` (per metre, at 650, 570 and
//!   475 nm) times the Rayleigh multiplier;
//! - Mie extinction `βM = 0.434 · c · 4π³ · K / λ²` with `c = (0.6544 T − 0.6510) · 1e-16`
//!   for turbidity `T`, `K = 0.67`, times the Mie multiplier;
//! - the Henyey–Greenstein eccentricity `g` is −0.5: the loader never reads it, so it keeps
//!   the atmosphere's preset (0x55c910, preset 0), which the loader applies first.
//!
//! The registers (vec4s):
//!
//! - sky `c0 = (1 + g², 2g, earth reflectance, distance multiplier)`,
//!   `c1 = (βR + βM, sun red · intensity)`, `c2 = (βM · (1 − g²) / 4π, sun green ·
//!   intensity)`, `c3 = (3 βR / 16π, sun blue · intensity)`, `c4 = (celestial alpha,
//!   celestial cloud alpha, water fog intensity², 1 − water fog cap)` (the sky pixel shader
//!   scales the celestial body's texture by `c4.x`), `c5`-`c8` the celestial body's
//!   projection (see [`celestial`]), which the sky vertex shader applies to the dome's
//!   vertices;
//! - clouds `[0] = (density, sharpness, depth, range 1)`, `[1] = (colour, range 2)`, from
//!   the level's `CLDS` (loaded at 0x5d07bc), or the preset's `(1, 0.2, −120, 2)`,
//!   `(1, 1, 1, 2)` when the level has none; `[2]` the celestial body's up axis;
//! - fog `[0] = (fog colour, x²)` with `x = lerp(tactical · intensity, intensity, t)`,
//!   `[1] = (1 − cap, alpha, zenith in radians, 0)`. The shaders fog by
//!   `exp(−d² · [0].w)` down to a floor of `[1].x`.
//!
//! The shaders' `ApplicationTime` (the clouds' drift) is the graphics object's clock
//! (0x52ae80: every frame adds the frame's time, unless its pause flag at `+0xca` is set),
//! read by the usage dispatch at 0x4f88da; [`crate::AreaRenderer::advance_time`] keeps it.
//!
//! Provisional: `t` is taken as 1 (the game camera; the engine reads it from the camera
//! each frame, 0 being the tactical view); the clock's pause flag is not traced (the time
//! always runs).

use glam::Vec3;
use thedas_formats::level::{Atmosphere, Clouds};

/// Rayleigh extinction per metre at 650, 570 and 475 nm.
const BETA_RAYLEIGH: [f32; 3] = [6.95e-6, 1.18e-5, 2.44e-5];
/// 1 / λ for the same wavelengths (per metre).
const INV_WAVELENGTH: [f64; 3] = [1538461.5, 1754386.0, 2105263.25];
const MIE_K: f32 = 0.67;

/// The fog registers for the game camera (`t` = 1). No atmosphere means no fog.
pub fn fog_params(a: Option<&Atmosphere>) -> [[f32; 4]; 2] {
    let Some(a) = a else {
        return [[0.0; 4], [1.0, 1.0, 0.0, 0.0]];
    };
    let t = 1.0;
    let tactical = a.fog_tactical * a.fog_intensity;
    let x = tactical + (a.fog_intensity - tactical) * t;
    [
        [a.fog_color[0], a.fog_color[1], a.fog_color[2], x * x],
        [1.0 - a.fog_cap, a.alpha, a.fog_zenith.to_radians(), 0.0],
    ]
}

/// The fog registers of a Dragon Age II level (its shaders' `mml_vFogParams`, two vectors):
/// the colour with the most the fog covers, then the distances at which it starts and is
/// full and the heights of its second fog's none and full (see
/// [`thedas_formats::level::LayoutFog`]). A range of no length is given a little, which the
/// shaders divide by.
pub fn layout_fog_params(f: &thedas_formats::level::LayoutFog) -> [[f32; 4]; 2] {
    let apart = |from: f32, to: f32| if to == from { from + 1e-3 } else { to };
    [
        [f.color[0], f.color[1], f.color[2], f.cap],
        [
            f.start,
            apart(f.start, f.end),
            f.height_start,
            apart(f.height_start, f.height_end),
        ],
    ]
}

/// The cloud registers (`CloudParameters`): the `CLDS` values and the celestial body's up
/// axis (no atmosphere: the update's `(0, 1, 0, 0)`).
pub fn cloud_params(c: Option<&Clouds>, celestial: Option<&Celestial>) -> [[f32; 4]; 3] {
    let up = celestial.map_or([0.0, 1.0, 0.0, 0.0], |c| c.up);
    match c {
        Some(c) => [
            [c.density, c.sharpness, c.depth, c.range_1],
            [c.color[0], c.color[1], c.color[2], c.range_2],
            up,
        ],
        None => [[1.0, 0.2, -120.0, 2.0], [1.0, 1.0, 1.0, 2.0], up],
    }
}

/// The preset's Henyey–Greenstein eccentricity (see the module docs).
pub const PHASE_ECCENTRICITY: f32 = -0.5;

/// Where the sky draws its celestial body (the sky material's `CelestialBody` texture),
/// as the atmosphere update builds it (0x55c4e2-0x55c87a).
///
/// With `N` the unit vector against the sunlight direction (towards the sun), `C` the unit
/// vector of `(0, 0, 1) × N` and `B` that of `N × C`, the projection is the product
/// `X · R` (row-major, as the registers hold it) of
///
/// ```text
/// X = [ C/s  0.5 ]      R = the rotation by the body's rotation r about N
///     [ B/s  0.5 ]          (0x4d3bd0: Rodrigues' matrix, rotating column vectors
///     [ N/s  0   ]           anticlockwise)
///     [ 0    1   ]
/// ```
///
/// where `s` is the body's scale. The sky vertex shader applies it to the dome's vertex
/// (`dp4` with each row) and the pixel shader samples at `xy / w`: an orthographic
/// projection along `N`, the texture's centre towards the sun, one texture width per `s`
/// of the dome's coordinates. The sampler's border (black, transparent) hides the rest.
///
/// `up` (the clouds' third register, 0x160) is the unit vector of `N × D`, `D` being `C`
/// turned by `−r` about `N` (0x4cbc40, 0x4cb6d0), with `w` 1.
///
/// A vector whose squared length is 1e-6 or less normalizes to `(1, 0, 0)`, as the engine's
/// normalize (0x451a30) does. Provisional: a scale of 0 (the engine would divide by it)
/// gives a zero projection.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Celestial {
    /// The projection's rows: the sky's `c5`-`c8`.
    pub projection: [[f32; 4]; 4],
    pub up: [f32; 4],
}

fn normalize(v: Vec3) -> Vec3 {
    let l2 = v.length_squared();
    if l2 > 1e-6 {
        v * (1.0 / l2.sqrt())
    } else {
        Vec3::X
    }
}

/// Rodrigues' rotation by `angle` (radians) about the unit `axis`, as rows (0x4d3bd0).
fn rotation(axis: Vec3, angle: f32) -> [[f32; 4]; 4] {
    let (s, c) = angle.sin_cos();
    let t = 1.0 - c;
    let Vec3 { x, y, z } = axis;
    [
        [t * x * x + c, t * x * y - s * z, t * x * z + s * y, 0.0],
        [t * x * y + s * z, t * y * y + c, t * y * z - s * x, 0.0],
        [t * x * z - s * y, t * y * z + s * x, t * z * z + c, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

fn apply(m: &[[f32; 4]; 4], v: Vec3) -> Vec3 {
    let row = |r: &[f32; 4]| r[0] * v.x + r[1] * v.y + r[2] * v.z;
    Vec3::new(row(&m[0]), row(&m[1]), row(&m[2]))
}

/// The celestial body for an atmosphere and the layout's sunlight direction (absent reads
/// as zero, as the layout loader leaves it).
pub fn celestial(a: &Atmosphere, sunlight_direction: Option<Vec3>) -> Celestial {
    let n = normalize(-sunlight_direction.unwrap_or(Vec3::ZERO));
    let c = normalize(Vec3::Z.cross(n));
    let b = normalize(n.cross(c));
    let k = 1.0 / a.moon_scale;
    let k = if k.is_finite() { k } else { 0.0 };
    let (c_k, b_k, n_k) = (c * k, b * k, n * k);
    let x = [
        [c_k.x, c_k.y, c_k.z, 0.5],
        [b_k.x, b_k.y, b_k.z, 0.5],
        [n_k.x, n_k.y, n_k.z, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ];
    let r = rotation(n, a.moon_rotation);
    let mut projection = [[0.0f32; 4]; 4];
    for (i, row) in projection.iter_mut().enumerate() {
        for (j, out) in row.iter_mut().enumerate() {
            *out = (0..4).map(|m| x[i][m] * r[m][j]).sum();
        }
    }
    let d = normalize(apply(&rotation(n, -a.moon_rotation), c));
    let up = normalize(n.cross(d));
    Celestial {
        projection,
        up: [up.x, up.y, up.z, 1.0],
    }
}

/// The sky registers `c0`-`c8`.
pub fn sky_params(a: &Atmosphere, celestial: &Celestial) -> [[f32; 4]; 9] {
    use std::f64::consts::PI;
    let g = PHASE_ECCENTRICITY;
    let c = (a.turbidity as f64 * 6.544 - 6.51) * 1e-17;
    let mie_base = (0.434 * c * 4.0 * PI.powi(3)) as f32;
    let beta_r = BETA_RAYLEIGH.map(|b| b * a.rayleigh);
    let beta_m = INV_WAVELENGTH.map(|l| (l * l) as f32 * MIE_K * mie_base * a.mie);
    let sun = a.sun_color.map(|c| c * a.sun_intensity);
    let four_pi = 4.0 * std::f32::consts::PI;
    let sixteen_pi = 16.0 * std::f32::consts::PI;
    let mut r = [[0.0f32; 4]; 9];
    r[0] = [1.0 + g * g, 2.0 * g, a.earth_reflectance, a.distance];
    for i in 0..3 {
        r[1][i] = beta_r[i] + beta_m[i];
        r[2][i] = beta_m[i] * (1.0 - g * g) / four_pi;
        r[3][i] = beta_r[i] * 3.0 / sixteen_pi;
    }
    r[1][3] = sun[0];
    r[2][3] = sun[1];
    r[3][3] = sun[2];
    r[4] = [
        a.moon_alpha,
        a.moon_cloud_alpha,
        a.water_fog_intensity * a.water_fog_intensity,
        1.0 - a.water_fog_cap,
    ];
    r[5..9].copy_from_slice(&celestial.projection);
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn atmo() -> Atmosphere {
        Atmosphere {
            sun_color: [0.74, 0.32, 0.14],
            sun_intensity: 17.378,
            turbidity: 1.03041,
            earth_reflectance: 0.0,
            mie: 0.01,
            rayleigh: 0.680769,
            distance: 0.94,
            alpha: 0.01,
            fog_color: [0.16, 0.19, 0.16],
            fog_intensity: 0.008255,
            fog_cap: 0.65,
            fog_zenith: 42.0,
            fog_tactical: 0.5,
            water_fog_intensity: 0.02,
            water_fog_cap: 0.25,
            moon_scale: 4.0,
            moon_alpha: 0.8,
            moon_cloud_alpha: 0.3,
            moon_rotation: 0.0,
        }
    }

    /// What the sky shaders compute for a dome vertex `v`: the texture coordinate.
    fn uv(c: &Celestial, v: Vec3) -> (f32, f32) {
        let o = c
            .projection
            .map(|r| r[0] * v.x + r[1] * v.y + r[2] * v.z + r[3]);
        (o[0] / o[3], o[1] / o[3])
    }

    #[test]
    fn fog_is_the_intensity_squared_down_to_one_less_the_cap() {
        let f = fog_params(Some(&atmo()));
        assert_eq!(&f[0][..3], &[0.16, 0.19, 0.16]);
        assert!((f[0][3] - 0.008255f32 * 0.008255).abs() < 1e-9);
        assert!((f[1][0] - 0.35).abs() < 1e-6);
        assert!((f[1][2] - 42f32.to_radians()).abs() < 1e-6);
        // No atmosphere: nothing fogs.
        assert_eq!(fog_params(None)[0][3], 0.0);
        assert_eq!(fog_params(None)[1][0], 1.0);
    }

    #[test]
    fn a_layouts_fog_is_its_colour_and_two_ranges() {
        let fog = thedas_formats::level::LayoutFog {
            color: [0.61, 0.65, 0.68],
            start: 16.0,
            end: 64.0,
            cap: 0.3,
            height_start: 1.5,
            height_end: -3.0,
        };
        assert_eq!(
            layout_fog_params(&fog),
            [[0.61, 0.65, 0.68, 0.3], [16.0, 64.0, 1.5, -3.0]]
        );
        // A range of no length is never divided by.
        let flat = thedas_formats::level::LayoutFog {
            end: 16.0,
            height_end: 1.5,
            ..fog
        };
        let p = layout_fog_params(&flat);
        assert!(p[1][1] > p[1][0] && p[1][3] > p[1][2]);
    }

    #[test]
    fn sky_follows_preetham() {
        let a = atmo();
        let s = sky_params(&a, &celestial(&a, Some(Vec3::new(0.0, -0.6, -0.8))));
        // Rayleigh's angular term is 3/16π of its extinction.
        let br = 6.95e-6 * a.rayleigh;
        assert!((s[3][0] - br * 3.0 / (16.0 * std::f32::consts::PI)).abs() < 1e-12);
        // Mie at 650 nm: 0.434 c 4π³ K / λ², c = (0.6544 T − 0.6510) 1e-16.
        let c = (0.6544 * a.turbidity as f64 - 0.6510) * 1e-16;
        let bm = 0.434 * c * 4.0 * std::f64::consts::PI.powi(3) * 0.67 / (650e-9f64).powi(2)
            * a.mie as f64;
        assert!(
            ((s[1][0] - br) as f64 - bm).abs() < bm * 1e-3,
            "{} vs {bm}",
            s[1][0] - br
        );
        assert_eq!(s[0][0], 1.25);
        assert_eq!(s[0][1], -1.0);
        assert!((s[1][3] - 0.74 * 17.378).abs() < 1e-4);
    }

    #[test]
    fn sky_c4_is_the_celestial_alphas_and_the_water_fog() {
        let a = atmo();
        let s = sky_params(&a, &celestial(&a, None));
        assert_eq!(s[4][0], 0.8);
        assert_eq!(s[4][1], 0.3);
        assert!((s[4][2] - 0.0004).abs() < 1e-9);
        assert_eq!(s[4][3], 0.75);
    }

    #[test]
    fn the_celestial_body_sits_against_the_sunlight() {
        let a = atmo();
        let light = Vec3::new(0.0, -0.6, -0.8);
        let c = celestial(&a, Some(light));
        // Towards the sun, at any distance on the dome: the texture's centre.
        let (u, v) = uv(&c, -light * 1000.0);
        assert!((u - 0.5).abs() < 1e-4 && (v - 0.5).abs() < 1e-4, "{u} {v}");
        // One scale sideways: half a texture off centre, along one axis only.
        let side = Vec3::Z.cross(-light).normalize();
        let (u, v) = uv(&c, -light * 1000.0 + side * a.moon_scale * 0.5);
        assert!((u - 1.0).abs() < 1e-3 && (v - 0.5).abs() < 1e-3, "{u} {v}");
        // The projection's last row keeps w at the vertex's w.
        assert_eq!(c.projection[3], [0.0, 0.0, 0.0, 1.0]);
        // The third row is the distance along N, scaled.
        let along = c.projection[2];
        assert!((along[1] - 0.6 / 4.0).abs() < 1e-6 && (along[2] - 0.8 / 4.0).abs() < 1e-6);
    }

    #[test]
    fn rotation_turns_the_texture_about_the_sun() {
        let mut a = atmo();
        let light = Vec3::new(0.0, -0.6, -0.8);
        let toward = -light;
        let side = Vec3::Z.cross(toward).normalize();
        a.moon_rotation = std::f32::consts::FRAC_PI_2;
        let c = celestial(&a, Some(light));
        // The centre stays put.
        let (u, v) = uv(&c, toward * 500.0);
        assert!((u - 0.5).abs() < 1e-4 && (v - 0.5).abs() < 1e-4);
        // A quarter turn moves the sideways offset from u to v.
        let (u, v) = uv(&c, toward * 500.0 + side * a.moon_scale * 0.5);
        assert!(
            (u - 0.5).abs() < 1e-3 && ((v - 0.5).abs() - 0.5).abs() < 1e-3,
            "{u} {v}"
        );
    }

    #[test]
    fn degenerate_directions_fall_back_like_the_engine() {
        let a = atmo();
        // Straight down: (0, 0, 1) × N vanishes, so C is (1, 0, 0).
        let c = celestial(&a, Some(Vec3::new(0.0, 0.0, -1.0)));
        assert_eq!(&c.projection[0][..3], &[0.25, 0.0, 0.0]);
        // No direction: N itself falls back to (1, 0, 0).
        let none = celestial(&a, None);
        assert_eq!(&none.projection[2][..3], &[0.25, 0.0, 0.0]);
        // A zero scale gives no projection rather than infinities.
        let mut z = a;
        z.moon_scale = 0.0;
        let c = celestial(&z, Some(Vec3::new(0.0, -0.6, -0.8)));
        assert!(c.projection.iter().flatten().all(|v| v.is_finite()));
    }

    #[test]
    fn clouds_carry_the_celestial_up_axis() {
        let a = atmo();
        let light = Vec3::new(0.0, -0.6, -0.8);
        let c = celestial(&a, Some(light));
        let up = Vec3::new(c.up[0], c.up[1], c.up[2]);
        assert!((up.length() - 1.0).abs() < 1e-5);
        assert!(up.dot(-light).abs() < 1e-5, "perpendicular to the sun");
        assert_eq!(c.up[3], 1.0);
        let p = cloud_params(None, Some(&c));
        assert_eq!(p[0], [1.0, 0.2, -120.0, 2.0]);
        assert_eq!(p[2], c.up);
        assert_eq!(cloud_params(None, None)[2], [0.0, 1.0, 0.0, 0.0]);
    }
}
