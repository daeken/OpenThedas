//! Fitting the bake's unknowns to a shipped atlas: which lights the lightmap pass used and how
//! bright, and how well the shadow pass and the occlusion pass explain the alpha channel.
//!
//! For each atlas texel the camera ray finds the surface as the bake does; each light's
//! unshadowed-or-not term (falloff × cosine × visibility) and a converged occlusion estimate
//! become the features, and the shipped texel the target.

use glam::{Vec2, Vec3};

use crate::bake::{BakeLight, LightKind, Placement, Settings};
use crate::gather::{BakeScene, Region};

/// One texel's features.
#[derive(Clone, Debug)]
pub struct Row {
    /// The shipped RGBA.
    pub shipped: [u8; 4],
    /// Per light (index into the light list): falloff × |cos| × visibility, when positive.
    pub terms: Vec<(u32, f32)>,
    /// Same, ignoring visibility (for lights that might not cast shadows).
    pub unshadowed: Vec<(u32, f32)>,
    /// Per visible light: distance over radius (0 for a directional light).
    pub reach: Vec<(u32, f32)>,
    /// Converged occlusion: the fraction of 64 cosine rays that escape.
    pub open: f32,
    /// Occlusion ray hit distances (∞ = escaped) out to 5: 64 cosine-distributed rays, then 64
    /// uniform ones.
    pub ao_cos: Vec<f32>,
    pub ao_uni: Vec<f32>,
    /// The same within 45° of the normal.
    pub ao_cos45: Vec<f32>,
    pub ao_uni45: Vec<f32>,
    /// The room ambient this texel's piece takes (index into the ambient list), if any.
    pub room: Option<u32>,
    pub atlas_xy: (u32, u32),
}

/// Computes the features of every covered texel of one region (every `stride`-th texel).
#[allow(clippy::too_many_arguments)]
pub fn rows(
    scene: &BakeScene,
    region: &Region,
    pl: Placement,
    shipped: &[u8],
    atlas_w: u32,
    lights: &[BakeLight],
    room: Option<u32>,
    settings: &Settings,
    stride: u32,
) -> Vec<Row> {
    let mut out = Vec::new();
    for y in (0..pl.h).step_by(stride as usize) {
        for x in (0..pl.w).step_by(stride as usize) {
            let (ax, ay) = (pl.x0 + x as i32, pl.y0 + y as i32);
            if ax < 0 || ay < 0 || ax as u32 >= atlas_w {
                continue;
            }
            let at = ((ay as u32 * atlas_w + ax as u32) * 4) as usize;
            let Some(px) = shipped.get(at..at + 4) else {
                continue;
            };
            let uv = Vec2::new(
                (x as f32 + 0.5) / pl.w as f32,
                (y as f32 + 0.5) / pl.h as f32,
            );
            let Some((sp, ng, nf)) = crate::bake::surface(scene, region, uv, settings) else {
                continue;
            };
            let mut terms = Vec::new();
            let mut unshadowed = Vec::new();
            let mut reach = Vec::new();
            for (i, l) in lights.iter().enumerate() {
                let (dir, f, tmax, ratio) = match l.kind {
                    LightKind::Ambient => continue,
                    LightKind::Point { position, radius } => {
                        let to = position - sp;
                        let d = to.length();
                        if d == 0.0 || d >= radius {
                            continue;
                        }
                        (to / d, 1.0 - d / radius, d, d / radius)
                    }
                    LightKind::Directional { toward } => {
                        (toward.normalize_or_zero(), 1.0, f32::INFINITY, 0.0)
                    }
                };
                if nf.dot(dir) <= 0.0 {
                    continue;
                }
                let t = f * ng.dot(dir).abs();
                unshadowed.push((i as u32, t));
                if !crate::bake::shadowed(&scene.bvh, sp, dir, tmax, settings) {
                    terms.push((i as u32, t));
                    reach.push((i as u32, ratio));
                }
            }
            let open = crate::bake::occlusion_converged(&scene.bvh, sp, ng, nf, settings, 64);
            let hits = |cosine, cone| {
                crate::bake::occlusion_hits(&scene.bvh, sp, nf, settings, 64, 5.0, cosine, cone)
            };
            let (ao_cos, ao_uni) = (hits(true, 0.0), hits(false, 0.0));
            let c45 = std::f32::consts::FRAC_1_SQRT_2;
            let (ao_cos45, ao_uni45) = (hits(true, c45), hits(false, c45));
            out.push(Row {
                shipped: [px[0], px[1], px[2], px[3]],
                terms,
                unshadowed,
                reach,
                open,
                ao_cos,
                ao_uni,
                ao_cos45,
                ao_uni45,
                room,
                atlas_xy: (ax as u32, ay as u32),
            });
        }
    }
    out
}

/// Non-negative least squares on the normal equations by projected coordinate descent:
/// minimizes ½xᵀAx − bᵀx subject to x ≥ 0.
pub fn nnls(a: &[Vec<f64>], b: &[f64], iterations: usize) -> Vec<f64> {
    let n = b.len();
    let mut x = vec![0.0; n];
    for _ in 0..iterations {
        let mut moved = 0.0f64;
        for i in 0..n {
            if a[i][i] <= 0.0 {
                continue;
            }
            let r: f64 = b[i] - (0..n).map(|j| a[i][j] * x[j]).sum::<f64>();
            let new = (x[i] + r / a[i][i]).max(0.0);
            moved = moved.max((new - x[i]).abs());
            x[i] = new;
        }
        if moved < 1e-10 {
            break;
        }
    }
    x
}

/// The lightmap pass's linear value of a shipped texel channel: the film's 8-bit value undone.
pub fn linear(c: u8, gamma: f32) -> f64 {
    (c as f64 / 255.0).powf(gamma as f64)
}

/// The fitted lightmap model: per light a brightness, per room an ambient colour.
#[derive(Clone, Debug)]
pub struct LightFit {
    pub light_scale: Vec<f64>,
    pub ambient: Vec<Vec3>,
    pub texels: usize,
    pub rmse_levels: f64,
}

/// Fits `lm = ambient(room) + Σ k_l · colour_l · term_l`, per channel in linear space, to the
/// texels `use_row` picks (ones where occlusion is believed to be 1).
pub fn fit_lights(
    rows: &[Row],
    lights: &[BakeLight],
    rooms: usize,
    gamma: f32,
    use_row: &dyn Fn(&Row) -> bool,
) -> LightFit {
    let nl = lights.len();
    let n = nl + rooms * 3;
    let mut a = vec![vec![0.0f64; n]; n];
    let mut b = vec![0.0f64; n];
    let mut used = 0;
    for r in rows.iter().filter(|r| use_row(r)) {
        used += 1;
        for c in 0..3 {
            let mut feats: Vec<(usize, f64)> = r
                .terms
                .iter()
                .map(|&(i, t)| {
                    let col = lights[i as usize].color[c] as f64;
                    (i as usize, col * t as f64)
                })
                .filter(|f| f.1 != 0.0)
                .collect();
            if let Some(room) = r.room {
                feats.push((nl + room as usize * 3 + c, 1.0));
            }
            let y = linear(r.shipped[c], gamma);
            for &(i, fi) in &feats {
                b[i] += fi * y;
                for &(j, fj) in &feats {
                    a[i][j] += fi * fj;
                }
            }
        }
    }
    let x = nnls(&a, &b, 2000);
    let light_scale = x[..nl].to_vec();
    let ambient = (0..rooms)
        .map(|r| {
            Vec3::new(
                x[nl + r * 3] as f32,
                x[nl + r * 3 + 1] as f32,
                x[nl + r * 3 + 2] as f32,
            )
        })
        .collect::<Vec<_>>();
    // Residual in 8-bit levels.
    let mut se = 0.0;
    let mut count = 0;
    for r in rows.iter().filter(|r| use_row(r)) {
        for (c, &shipped) in r.shipped[..3].iter().enumerate() {
            let mut v = r
                .room
                .map(|room| ambient[room as usize][c] as f64)
                .unwrap_or(0.0);
            for &(i, t) in &r.terms {
                v += light_scale[i as usize] * lights[i as usize].color[c] as f64 * t as f64;
            }
            let pred = 255.0 * v.max(0.0).powf(1.0 / gamma as f64);
            let d = pred.min(255.0) - shipped as f64;
            se += d * d;
            count += 1;
        }
    }
    LightFit {
        light_scale,
        ambient,
        texels: used,
        rmse_levels: (se / count.max(1) as f64).sqrt(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nnls_solves_and_clamps() {
        // x0 + x1 = 3, x0 − x1 = 1 → (2, 1); a negative solution clamps to 0.
        let a = vec![vec![2.0, 0.0], vec![0.0, 2.0]];
        let b = vec![4.0, 2.0];
        let x = nnls(&a, &b, 100);
        assert!((x[0] - 2.0).abs() < 1e-9 && (x[1] - 1.0).abs() < 1e-9);
        let x = nnls(&[vec![1.0]], &[-1.0], 10);
        assert_eq!(x, vec![0.0]);
    }
}
