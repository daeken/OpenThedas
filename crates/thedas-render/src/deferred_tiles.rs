//! Conservative screen tiles for the portable deferred point-light uniform array.

use glam::{Mat4, Vec2, Vec3, Vec4};

pub(crate) const COLUMNS: usize = 16;
pub(crate) const ROWS: usize = 9;
pub(crate) const TILES: usize = COLUMNS * ROWS;
/// Six 32-bit light masks, padded to two uniform vec4s per tile (192 lights).
pub(crate) type Masks = [[u32; 8]; TILES];

pub(crate) const DEPTH_SLICES: usize = 16;
pub(crate) const DEPTH_STEP: f32 = 4.0;
pub(crate) type DepthMasks = [[u32; 8]; DEPTH_SLICES];

/// View-depth intervals complement screen tiles without multiplying their uniform
/// storage. The last slice is unbounded; invalid spheres conservatively reach all.
pub(crate) fn depth_masks(
    plane: Vec4,
    lights: impl IntoIterator<Item = [f32; 4]>,
    radius_scale: f32,
    enabled: bool,
) -> DepthMasks {
    let mut masks = [[0; 8]; DEPTH_SLICES];
    for (i, light) in lights.into_iter().take(192).enumerate() {
        let centre = Vec3::from_slice(&light[..3]);
        let radius = light[3] * radius_scale;
        let depth = plane.dot(centre.extend(1.0));
        let (lo, hi) = if enabled && depth.is_finite() && radius.is_finite() && radius >= 0.0 {
            // Include boundary neighbours despite inverse-projection/half-float noise.
            let pad = 1e-3 * (depth.abs() + radius).max(1.0);
            let slice = |d: f32| ((d.max(0.0) / DEPTH_STEP) as usize).min(DEPTH_SLICES - 1);
            (slice(depth - radius - pad), slice(depth + radius + pad))
        } else {
            (0, DEPTH_SLICES - 1)
        };
        for mask in &mut masks[lo..=hi] {
            mask[i / 32] |= 1 << (i % 32);
        }
    }
    masks
}

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

/// Tile side planes can reject spheres even when their enclosing AABB crosses the eye
/// plane. The rectangle remains a cheap broad phase; the exact shader radius test stays.
pub(crate) fn build(
    view_proj: Mat4,
    size: (u32, u32),
    lights: impl IntoIterator<Item = [f32; 4]>,
    radius_scale: f32,
    enabled: bool,
    tight: bool,
) -> Masks {
    let planes: Vec<_> = if enabled && tight {
        (0..TILES)
            .map(|tile| tile_planes(view_proj, size, tile))
            .collect()
    } else {
        Vec::new()
    };
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
        let centre = Vec3::from_slice(&light[..3]).extend(1.0);
        let radius = light[3] * radius_scale;
        let valid_sphere = centre.is_finite() && radius.is_finite() && radius >= 0.0;
        for y in lo[1]..=hi[1] {
            for x in lo[0]..=hi[0] {
                let tile = y * COLUMNS + x;
                if !planes.is_empty()
                    && valid_sphere
                    && planes[tile].iter().any(|p| p.dot(centre) < -radius)
                {
                    continue;
                }
                masks[tile][i / 32] |= 1 << (i % 32);
            }
        }
    }
    masks
}

fn tile_planes(vp: Mat4, size: (u32, u32), tile: usize) -> [Vec4; 4] {
    let x = (tile % COLUMNS) as f32 / COLUMNS as f32;
    let y = (tile / COLUMNS) as f32 / ROWS as f32;
    // One pixel covers half-pixel reconstruction and floating-point tile boundaries.
    let dx = 1.0 / size.0.max(1) as f32;
    let dy = 1.0 / size.1.max(1) as f32;
    let left = (x - dx) * 2.0 - 1.0;
    let right = (x + 1.0 / COLUMNS as f32 + dx) * 2.0 - 1.0;
    let top = 1.0 - (y - dy) * 2.0;
    let bottom = 1.0 - (y + 1.0 / ROWS as f32 + dy) * 2.0;
    let (r0, r1, r3) = (vp.row(0), vp.row(1), vp.row(3));
    [
        r0 - left * r3,
        right * r3 - r0,
        r1 - bottom * r3,
        top * r3 - r1,
    ]
    .map(|p| {
        let len = p.truncate().length();
        if p.is_finite() && len > 0.0 {
            p / len
        } else {
            Vec4::ZERO
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Camera;

    #[test]
    fn depth_masks_keep_sphere_samples_boundaries_wrap_and_unbounded_distance() {
        assert_eq!(std::mem::size_of::<DepthMasks>(), 512);
        for camera in [
            Camera::new(Vec3::ZERO, 0.0, 0.0),
            Camera::new(Vec3::new(298.2, 235.0, 3.0), 1.2, -0.4),
        ] {
            let plane = -camera.view().row(2);
            for distance in [-20.0, 0.0, 0.001, 3.999, 4.0, 8.0, 59.999, 60.0, 5000.0] {
                for radius in [0.0, 0.01, 1.0, 4.0, 30.0] {
                    let centre = camera.position + camera.forward() * distance;
                    for scale in [1.0, 2.0] {
                        let masks =
                            depth_masks(plane, [centre.extend(radius).to_array()], scale, true);
                        for step in -16..=16 {
                            let p =
                                centre + camera.forward() * (step as f32 / 16.0 * radius * scale);
                            let depth = plane.dot(p.extend(1.0));
                            let slice =
                                ((depth.max(0.0) / DEPTH_STEP) as usize).min(DEPTH_SLICES - 1);
                            assert_ne!(
                                masks[slice][0] & 1,
                                0,
                                "lost {distance}, {radius}, {scale}, {depth}"
                            );
                        }
                    }
                }
            }
        }
        for enabled in [false, true] {
            let masks = depth_masks(Vec4::X, [[f32::NAN, 0.0, 0.0, 1.0]; 192], 1.0, enabled);
            assert!(
                masks
                    .iter()
                    .all(|m| m[..6] == [u32::MAX; 6] && m[6..] == [0; 2])
            );
        }
        let near = depth_masks(Vec4::X, [[2.0, 0.0, 0.0, 0.1]; 192], 1.0, true);
        assert_eq!(near[0][..6], [u32::MAX; 6]);
        assert!(near[1..].iter().all(|m| *m == [0; 8]));
    }

    #[test]
    fn tiles_keep_visible_sphere_samples_edges_near_crossings_and_wrap_radius() {
        let camera = Camera::new(Vec3::new(2.0, 3.0, 4.0), 0.3, -0.1);
        for size in [(1920, 1080), (2560, 1440), (321, 200), (1, 1)] {
            let vp = camera.view_proj(size.0 as f32 / size.1 as f32);
            for centre in [
                Vec3::new(10.0, 3.0, 4.0),
                Vec3::new(8.0, 8.0, 7.0),
                camera.position,
                camera.position + camera.forward() + Vec3::Y,
                camera.position - camera.forward() + Vec3::Z,
                Vec3::new(-2.0, 0.0, 4.0),
            ] {
                for radius in [0.01, 1.0, 1.1, 5.0, 20.0] {
                    for scale in [1.0, 2.0] {
                        for tight in [false, true] {
                            let masks = build(
                                vp,
                                size,
                                [centre.extend(radius).to_array()],
                                scale,
                                true,
                                tight,
                            );
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
                                        let tx = (uv.x * COLUMNS as f32)
                                            .floor()
                                            .min((COLUMNS - 1) as f32)
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
    }

    #[test]
    fn sparse_tiles_and_full_fallback_preserve_all_192_indices_in_order() {
        assert_eq!(std::mem::size_of::<Masks>(), 4608);
        let vp = Camera::new(Vec3::ZERO, 0.0, 0.0).view_proj(16.0 / 9.0);
        let lights = vec![[10.0, 0.0, 0.0, 0.01]; 192];
        let sparse = build(vp, (1920, 1080), lights.clone(), 1.0, true, true);
        assert!(sparse.contains(&[0; 8]));
        assert!(sparse.iter().any(|m| m[..6] == [u32::MAX; 6]));
        let full = build(vp, (1920, 1080), lights, 1.0, false, true);
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
        let near = build(vp, (1920, 1080), [[0.0, 0.0, 0.0, 1.0]], 1.0, true, true);
        assert!(near.iter().all(|m| m[0] == 1));
    }

    #[test]
    fn tile_frusta_reject_eye_crossing_box_false_positives_and_keep_invalid_fallbacks() {
        let vp = Camera::new(Vec3::ZERO, 0.0, 0.0).view_proj(16.0 / 9.0);
        let lights = [[1.0, 1.0, 0.0, 1.1]; 192];
        let reference = build(vp, (1920, 1080), lights, 1.0, true, false);
        let tight = build(vp, (1920, 1080), lights, 1.0, true, true);
        let count = |m: &Masks| {
            m.iter()
                .flatten()
                .map(|word| word.count_ones())
                .sum::<u32>()
        };
        assert_eq!(count(&reference), TILES as u32 * 192);
        assert!(count(&tight) > 0 && count(&tight) < count(&reference));
        for (a, b) in reference.iter().zip(tight.iter()) {
            for (a, b) in a.iter().zip(b) {
                assert_eq!(a & b, *b);
            }
        }
        for light in [
            [f32::NAN, 0.0, 0.0, 1.0],
            [1.0, 0.0, 0.0, -1.0],
            [1.0, 0.0, 0.0, f32::INFINITY],
        ] {
            let masks = build(vp, (1920, 1080), [light], 1.0, true, true);
            assert!(masks.iter().all(|m| m[0] == 1));
        }
    }
}
