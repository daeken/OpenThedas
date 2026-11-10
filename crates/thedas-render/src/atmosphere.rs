//! A level's atmosphere as the shaders read it: the fog (`mml_vFogParams`, usage
//! `FogParameters`) and the sky's scattering (`mml_vSkyParams`, usage `SkyParameters`).
//!
//! Both are what DAOrigins.exe's atmosphere update (0x55bfa0) computes from the level's
//! `ATMO` fields (see [`thedas_formats::level::Atmosphere`]); it is Hoffman and Preetham's
//! daylight scattering, with Preetham's constants:
//!
//! - Rayleigh extinction `βR = (6.95e-6, 1.18e-5, 2.44e-5)` (per metre, at 650, 570 and
//!   475 nm) times the Rayleigh multiplier;
//! - Mie extinction `βM = 0.434 · c · 4π³ · K / λ²` with `c = (0.6544 T − 0.6510) · 1e-16`
//!   for turbidity `T`, `K = 0.67`, times the Mie multiplier;
//! - the Henyey–Greenstein eccentricity `g` is 0: the loader never reads it.
//!
//! The registers (vec4s):
//!
//! - sky `c0 = (1 + g², 2g, earth reflectance, distance multiplier)`,
//!   `c1 = (βR + βM, sun red · intensity)`, `c2 = (βM · (1 − g²) / 4π, sun green ·
//!   intensity)`, `c3 = (3 βR / 16π, sun blue · intensity)`, `c4` the celestial body's
//!   (zero: nothing sets it from the level), `c5`-`c8` the celestial body's projection;
//! - fog `[0] = (fog colour, x²)` with `x = lerp(tactical · intensity, intensity, t)`,
//!   `[1] = (1 − cap, alpha, zenith in radians, 0)`. The shaders fog by
//!   `exp(−d² · [0].w)` down to a floor of `[1].x`.
//!
//! Provisional: `t` is taken as 1 (the game camera; the engine reads it from the camera
//! each frame, 0 being the tactical view); the celestial body (`c4`-`c8`) is not drawn;
//! how the engine packs the vertex shader's nine sky vectors is not traced.

use thedas_formats::level::Atmosphere;

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

/// The sky registers `c0`-`c8`.
pub fn sky_params(a: &Atmosphere) -> [[f32; 4]; 9] {
    use std::f64::consts::PI;
    let g = 0.0f32;
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
    // No celestial body: its projection keeps w at 1 so the shader's divide stays finite.
    r[8] = [0.0, 0.0, 0.0, 1.0];
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
        }
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
    fn sky_follows_preetham() {
        let s = sky_params(&atmo());
        let a = atmo();
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
        assert_eq!(s[0][0], 1.0);
        assert!((s[1][3] - 0.74 * 17.378).abs() < 1e-4);
        assert_eq!(s[8][3], 1.0);
    }
}
