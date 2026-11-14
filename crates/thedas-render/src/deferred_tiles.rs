//! Conservative screen tiles for the portable deferred point-light uniform array.

use glam::{Mat4, Vec2, Vec3};

pub(crate) const COLUMNS: usize = 16;
pub(crate) const ROWS: usize = 9;
pub(crate) const TILES: usize = COLUMNS * ROWS;
/// Six 32-bit light masks, padded to two uniform vec4s per tile (192 lights).
pub(crate) type Masks = [[u32; 8]; TILES];

/// A sphere's world AABB projects conservatively while all corners have positive w.
/// A box crossing the eye plane uses every tile; dividing those corners could otherwise
/// miss an arbitrarily large projection. One pixel covers the G-buffer half-pixel fixup
/// and rounding at tile boundaries. Room/domain and exact radius tests remain in WGSL.
fn rectangle(
    view_proj: Mat4,
    size: (u32, u32),
    centre: Vec3,
    radius: f32,
) -> Option<([usize; 2], [usize; 2])> {
    let full = Some(([0, 0], [COLUMNS - 1, ROWS - 1]));
    if !centre.is_finite() || !radius.is_finite() || radius < 0.0 {
        return full;
    }
    let mut lo = Vec2::splat(f32::INFINITY);
    let mut hi = Vec2::splat(f32::NEG_INFINITY);
    for x in [-1.0, 1.0] {
        for y in [-1.0, 1.0] {
            for z in [-1.0, 1.0] {
                let clip = view_proj * (centre + Vec3::new(x, y, z) * radius).extend(1.0);
                if !clip.is_finite() || clip.w <= 1e-4 {
                    return full;
                }
                let uv = Vec2::new(clip.x / clip.w * 0.5 + 0.5, 0.5 - clip.y / clip.w * 0.5);
                lo = lo.min(uv);
                hi = hi.max(uv);
            }
        }
    }
    let pad = Vec2::new(1.0 / size.0.max(1) as f32, 1.0 / size.1.max(1) as f32);
    lo -= pad;
    hi += pad;
    if hi.x < 0.0 || hi.y < 0.0 || lo.x > 1.0 || lo.y > 1.0 {
        return None;
    }
    let tile = |p: Vec2| {
        [
            (p.x.clamp(0.0, 1.0) * COLUMNS as f32)
                .floor()
                .min((COLUMNS - 1) as f32) as usize,
            (p.y.clamp(0.0, 1.0) * ROWS as f32)
                .floor()
                .min((ROWS - 1) as f32) as usize,
        ]
    };
    Some((tile(lo), tile(hi)))
}

pub(crate) fn build(
    view_proj: Mat4,
    size: (u32, u32),
    lights: impl IntoIterator<Item = [f32; 4]>,
    radius_scale: f32,
    enabled: bool,
) -> Masks {
    let mut masks = [[0; 8]; TILES];
    for (i, light) in lights.into_iter().take(192).enumerate() {
        let range = if enabled {
            rectangle(
                view_proj,
                size,
                Vec3::from_array([light[0], light[1], light[2]]),
                light[3] * radius_scale,
            )
        } else {
            Some(([0, 0], [COLUMNS - 1, ROWS - 1]))
        };
        let Some((lo, hi)) = range else {
            continue;
        };
        for y in lo[1]..=hi[1] {
            for x in lo[0]..=hi[0] {
                masks[y * COLUMNS + x][i / 32] |= 1 << (i % 32);
            }
        }
    }
    masks
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Camera;

    #[test]
    fn tiles_keep_visible_sphere_samples_edges_near_crossings_and_wrap_radius() {
        let camera = Camera::new(Vec3::new(2.0, 3.0, 4.0), 0.3, -0.1);
        for size in [(1920, 1080), (2560, 1440), (321, 200), (1, 1)] {
            let vp = camera.view_proj(size.0 as f32 / size.1 as f32);
            for centre in [
                Vec3::new(10.0, 3.0, 4.0),
                Vec3::new(8.0, 8.0, 7.0),
                camera.position,
                Vec3::new(-2.0, 0.0, 4.0),
            ] {
                for radius in [0.01, 1.0, 5.0, 20.0] {
                    for scale in [1.0, 2.0] {
                        let masks =
                            build(vp, size, [centre.extend(radius).to_array()], scale, true);
                        for z in -4i32..=4 {
                            for y in -4i32..=4 {
                                for x in -4i32..=4 {
                                    let d = Vec3::new(x as f32, y as f32, z as f32) / 4.0;
                                    if d.length_squared() > 1.0 {
                                        continue;
                                    }
                                    let p = vp * (centre + d * radius * scale).extend(1.0);
                                    if p.w <= 0.0 || p.z < 0.0 || p.z > p.w {
                                        continue;
                                    }
                                    let uv =
                                        Vec2::new(p.x / p.w * 0.5 + 0.5, 0.5 - p.y / p.w * 0.5);
                                    if uv.min_element() < 0.0 || uv.max_element() > 1.0 {
                                        continue;
                                    }
                                    let tx =
                                        (uv.x * COLUMNS as f32).floor().min((COLUMNS - 1) as f32)
                                            as usize;
                                    let ty = (uv.y * ROWS as f32).floor().min((ROWS - 1) as f32)
                                        as usize;
                                    assert_ne!(
                                        masks[ty * COLUMNS + tx][0] & 1,
                                        0,
                                        "lost {centre:?} radius {radius}, {uv:?}"
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn sparse_tiles_and_full_fallback_preserve_all_192_indices_in_order() {
        assert_eq!(std::mem::size_of::<Masks>(), 4608);
        let vp = Camera::new(Vec3::ZERO, 0.0, 0.0).view_proj(16.0 / 9.0);
        let lights = vec![[10.0, 0.0, 0.0, 0.01]; 192];
        let sparse = build(vp, (1920, 1080), lights.clone(), 1.0, true);
        assert!(sparse.contains(&[0; 8]));
        assert!(sparse.iter().any(|m| m[..6] == [u32::MAX; 6]));
        let full = build(vp, (1920, 1080), lights, 1.0, false);
        for mask in full {
            assert_eq!(
                mask,
                [
                    u32::MAX,
                    u32::MAX,
                    u32::MAX,
                    u32::MAX,
                    u32::MAX,
                    u32::MAX,
                    0,
                    0
                ]
            );
        }
        let near = build(vp, (1920, 1080), [[0.0, 0.0, 0.0, 1.0]], 1.0, true);
        assert!(near.iter().all(|m| m[0] == 1));
    }
}
