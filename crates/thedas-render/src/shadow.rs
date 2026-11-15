//! The engine's run-time sun shadows: two shadow partitions (parallel-split shadow maps)
//! around the camera, drawn from the sun each frame and read by the level and character
//! shaders through `g_vShadowData` and `smpShadowPartition1`/`2`.
//!
//! What DAOrigins.exe does, and what is mirrored here:
//!
//! - The Graphics Detail setter hands its mode to the shadow quality (0x458ff7 → 0x4556e0):
//!   at Low and Medium the partitions are 512 texels square, at High 1024 (0x4555ae,
//!   0x45569e), and they reach 5 and 15 m (Low), 7 and 20 m (Medium) or 10 and 30 m (High)
//!   from the camera (the table at 0x455731).
//! - The light's view is a rotation only, looking along the sunlight (0x452a90, into
//!   0xc1e830). Each partition fits an orthographic box to it (0x4524d0 from the bounds
//!   0x453240 gathers: the casters' spheres, 0x452570, clipped to the camera slice) and
//!   its texture matrix is that projection times the bias `(0.5, −0.5)` (0x4523b0).
//!   After smoothing, the X/Y minima are floored and maxima ceiled to whole light-space
//!   metres (0x453309–0x4533b9); only the projection is rounded, not the saved bounds.
//! - The submission queue (`dadefaultsub.sub.xml`'s `Shadow0_visible`/`Shadow1_visible`
//!   groups) draws every shadow caster's material with its `PSSM` semantic into the
//!   partition: `PSSMMesh.vsh` with `PSSMStatic.psh` (level geometry: depth and its square)
//!   or `PSSM.psh` (props, characters: depth only), writing red and green only. Level
//!   meshes cast when their model says so (`CastRuntimeShadow`, 6176); creatures and
//!   props cast through their own materials' `PSSM` semantics; eyes and fading objects
//!   never do.
//! - `g_vShadowData` is the first partition's texture matrix (`c12`-`c15`), the second's
//!   (`c16`-`c19`, its `w` replaced by the first partition's reach less the second's) and
//!   the plane `c20` that is positive inside the first partition (0x452d40).
//!
//! - At Medium and High the first partition is blurred (`BlurShadowTargets` → 0x4518e0,
//!   `[0xbf22c8] >= 2`; see `d3d_gpu::ShadowBlur`); at High both partitions and the blur's
//!   targets are 1024 texels a side ([`partition_size`]).
//!
//! Provisional: the second partition
//! starts where the first ends; the engine's elevation clamp of the light (0x452ac2) is
//! not applied; the bias drops
//! D3D9's half-texel offset (wgpu samples texel centres at half-texel coordinates); the
//! level's pass fits the partitions to the objects' casters a frame late.

use glam::{Mat4, Vec3, Vec4};

/// Texels on a side of each partition at Low and Medium detail (0x45569e).
pub const PARTITION_SIZE: u32 = 512;

/// Texels on a side of each partition at High detail (0x4555ae).
pub const PARTITION_SIZE_HIGH: u32 = 1024;

/// Texels on a side of each partition at each Graphics Detail setting: 0x455580 picks the
/// size through the jump table at 0x4556cc from the shadow quality `[0xbf20e0]` (Low and
/// Medium 512 at 0x45569e, High 1024 at 0x4555ae) before it makes the targets (0x455420),
/// and the variance blur's chain takes the same size (0x454572).
pub fn partition_size(detail: crate::scene::Detail) -> u32 {
    match detail {
        crate::scene::Detail::Low | crate::scene::Detail::Medium => PARTITION_SIZE,
        crate::scene::Detail::High => PARTITION_SIZE_HIGH,
    }
}

/// How far each partition reaches from the camera at each Graphics Detail setting
/// (0x455731: Low, Medium, High).
pub fn partition_reach(detail: crate::scene::Detail) -> [f32; 2] {
    match detail {
        crate::scene::Detail::Low => [5.0, 15.0],
        crate::scene::Detail::Medium => [7.0, 20.0],
        crate::scene::Detail::High => [10.0, 30.0],
    }
}

/// The partitions' reach for a Dragon Age II level, whose shaders read only the first
/// partition (`smpShadowPartition1`, see [`crate::d3d::DEBUG_CONTROL`]): it must hold all
/// the shadows a frame shows, so it reaches 50 m (the second, unread, then reaches 100).
/// Provisional: Dragon Age II's own shadow distance is not traced; 50 m keeps a 1024-texel
/// map at about 5 cm a texel across a street.
pub const SINGLE_PARTITION_REACH: [f32; 2] = [50.0, 100.0];

/// How far towards the sun a partition's box may reach beyond its camera slice, for the
/// casters standing outside it: this many times the slice's own depth along the light
/// (DAOrigins.exe's slot `+0x6ac`, 3.0 from 0x453dca; the reach `+0x6b0` computed at
/// 0x453689 from the slice's eight corners in light space, and the near plane taken as the
/// nearer of the casters' and that limit at 0x45273a; notes/shadow-box-depth-trace.md).
pub const SUNWARD_SPANS: f32 = 3.0;

/// One shadow partition.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Partition {
    /// World to the partition's clip space (`PSSMMesh.vsh`'s `g_mPSSMPartitionMatrix`).
    pub view_proj: Mat4,
    /// World to the partition's texture coordinates and depth (`g_vShadowData`).
    pub texture: Mat4,
    /// How far the partition reaches from the camera.
    pub reach: f32,
}

/// The light's view: a rotation that looks along the sunlight `dir` (0x452a90).
pub fn light_view(dir: Vec3) -> Mat4 {
    let f = dir.normalize_or(Vec3::NEG_Z);
    let up = if f.z.abs() > 0.99 { Vec3::Y } else { Vec3::Z };
    let r = up.cross(f).normalize();
    let u = f.cross(r);
    // Rows: x along r, y along u, z along the light's travel (deeper is further from the
    // sun).
    Mat4::from_cols(
        Vec4::new(r.x, u.x, f.x, 0.0),
        Vec4::new(r.y, u.y, f.y, 0.0),
        Vec4::new(r.z, u.z, f.z, 0.0),
        Vec4::W,
    )
}

/// The corners of the camera frustum between view depths `near` and `far`.
fn slice_corners(camera: &crate::Camera, aspect: f32, near: f32, far: f32) -> [Vec3; 8] {
    let f = camera.forward();
    let r = f.cross(Vec3::Z).normalize_or(Vec3::X);
    let u = r.cross(f);
    let t = (camera.fov_y * 0.5).tan();
    let mut out = [Vec3::ZERO; 8];
    for (i, d) in [near, far].into_iter().enumerate() {
        let (h, w) = (t * d, t * d * aspect);
        let c = camera.position + f * d;
        for (j, (sx, sy)) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]
            .into_iter()
            .enumerate()
        {
            out[i * 4 + j] = c + r * (w * sx) + u * (h * sy);
        }
    }
    out
}

/// An orthographic projection of the light-space box to D3D clip space (z in 0..1)
/// (0x4524d0).
fn ortho(min: Vec3, max: Vec3) -> Mat4 {
    let s = (max - min).max(Vec3::splat(1e-3));
    Mat4::from_cols(
        Vec4::new(2.0 / s.x, 0.0, 0.0, 0.0),
        Vec4::new(0.0, 2.0 / s.y, 0.0, 0.0),
        Vec4::new(0.0, 0.0, 1.0 / s.z, 0.0),
        Vec4::new(
            -(max.x + min.x) / s.x,
            -(max.y + min.y) / s.y,
            -min.z / s.z,
            1.0,
        ),
    )
}

/// Clip space to texture coordinates: `u = 0.5 x + 0.5`, `v = −0.5 y + 0.5`, depth kept
/// (0x4523b0, without the half-texel offset).
fn bias() -> Mat4 {
    Mat4::from_cols(
        Vec4::new(0.5, 0.0, 0.0, 0.0),
        Vec4::new(0.0, -0.5, 0.0, 0.0),
        Vec4::new(0.0, 0.0, 1.0, 0.0),
        Vec4::new(0.5, 0.5, 0.0, 1.0),
    )
}

/// The two partitions for a camera and the sunlight's direction, fitted to the shadow
/// casters (world bounding spheres: centre, radius).
///
/// As DAOrigins.exe gathers them (0x452570 for each caster, 0x453240 then the box): in the
/// light's view each partition's box grows to cover the casters, but never past its camera
/// slice's box across the light (`x`, `y`: casters off the slice cannot shadow it); along
/// the light (`z`) it reaches the casters nearer the sun, up to [`SUNWARD_SPANS`] times the
/// slice's own depth beyond the
/// slice, and no deeper than the slice. A box at least a metre a side (0x452824). Without a
/// caster over its slice, a partition covers the slice.
///
/// Provisional: the box's starting extent and the engine's own limit towards the sun (the
/// slot's 0x6b0 term) are not traced. See [`partitions_smoothed`] for the boxes' smoothing
/// over frames.
pub fn partitions(
    camera: &crate::Camera,
    aspect: f32,
    sun_dir: Vec3,
    reach: [f32; 2],
    casters: &[(Vec3, f32)],
) -> [Partition; 2] {
    partitions_smoothed(camera, aspect, sun_dir, reach, casters, &[None, None]).0
}

/// A partition's box in the light's view (minimum, maximum corners), as the next frame
/// smooths towards it.
pub type LightBox = (Vec3, Vec3);

/// How far a partition's box moves each frame from the last frame's towards the one fitted
/// now (`[0xbf2088]`, 0.35, never written).
pub const BOX_SMOOTHING: f32 = 0.35;

/// [`partitions`], smoothed over frames as DAOrigins.exe does (0x453266): when a partition
/// had casters the frame before (its slot's flag at 0x678, set from the caster count by
/// 0x4517d0), its box this frame is the last frame's moved [`BOX_SMOOTHING`] of the way to
/// the box fitted now; the result is kept for the next frame (0x4533fa copies it to the
/// slot's previous box). `previous` is what the last call returned: each partition's box
/// when it had casters.
/// The projection rounds the smoothed X/Y bounds out to whole metres (0x453309: maxima
/// through `ceil` at 0x8e51d0, minima through `floor` at 0x8e6110). This keeps the shadow
/// map from sliding by fractional texels as the camera moves. Depth and the saved box
/// stay unrounded; quantizing that history instead would stall the smoothing.
pub fn partitions_smoothed(
    camera: &crate::Camera,
    aspect: f32,
    sun_dir: Vec3,
    reach: [f32; 2],
    casters: &[(Vec3, f32)],
    previous: &[Option<LightBox>; 2],
) -> ([Partition; 2], [Option<LightBox>; 2]) {
    let view = light_view(sun_dir);
    let near = camera.near.max(0.01);
    let ranges = [(near, reach[0]), (reach[0], reach[1])];
    let lit: Vec<(Vec3, f32)> = casters
        .iter()
        .filter(|(c, r)| c.is_finite() && r.is_finite())
        .map(|(c, r)| (view.transform_point3(*c), r.max(0.0)))
        .collect();
    let mut kept = [None, None];
    let parts = [0, 1].map(|k| {
        let (n, f) = ranges[k];
        let corners = slice_corners(camera, aspect, n, f);
        let mut lo = Vec3::splat(f32::INFINITY);
        let mut hi = Vec3::splat(f32::NEG_INFINITY);
        for c in corners {
            let p = view.transform_point3(c);
            lo = lo.min(p);
            hi = hi.max(p);
        }
        let mut min = Vec3::splat(f32::INFINITY);
        let mut max = Vec3::splat(f32::NEG_INFINITY);
        for (c, r) in &lit {
            let (a, b) = (*c - Vec3::splat(*r), *c + Vec3::splat(*r));
            // Off the slice across the light, or wholly beyond it: no shadow on it.
            if b.x < lo.x || a.x > hi.x || b.y < lo.y || a.y > hi.y || a.z > hi.z {
                continue;
            }
            min = min.min(a);
            max = max.max(b);
        }
        let had_casters = min.x.is_finite();
        let (min, max) = if had_casters {
            (
                Vec3::new(
                    min.x.max(lo.x),
                    min.y.max(lo.y),
                    min.z.max(lo.z - SUNWARD_SPANS * (hi.z - lo.z)),
                ),
                Vec3::new(max.x.min(hi.x), max.y.min(hi.y), max.z.min(hi.z)),
            )
        } else {
            (
                Vec3::new(lo.x, lo.y, lo.z - SUNWARD_SPANS * (hi.z - lo.z)),
                hi,
            )
        };
        let max = max.max(min + Vec3::ONE);
        let (min, max) = match previous[k] {
            Some((pmin, pmax)) => (
                pmin + (min - pmin) * BOX_SMOOTHING,
                pmax + (max - pmax) * BOX_SMOOTHING,
            ),
            None => (min, max),
        };
        if had_casters {
            kept[k] = Some((min, max));
        }
        let (min, max) = if had_casters {
            (
                Vec3::new(min.x.floor(), min.y.floor(), min.z),
                Vec3::new(max.x.ceil(), max.y.ceil(), max.z),
            )
        } else {
            (min, max)
        };
        let view_proj = ortho(min, max) * view;
        Partition {
            view_proj,
            texture: bias() * view_proj,
            reach: f,
        }
    });
    (parts, kept)
}

/// `g_vShadowData` for two partitions seen from `camera` (0x452d40).
pub fn shadow_data(p: &[Partition; 2], camera: &crate::Camera) -> [[f32; 4]; 9] {
    let rows = |m: &Mat4| [0, 1, 2, 3].map(|i| m.row(i).to_array());
    let a = rows(&p[0].texture);
    let mut b = rows(&p[1].texture);
    b[3][3] = p[0].reach - p[1].reach;
    let f = camera.forward();
    let plane = [-f.x, -f.y, -f.z, p[0].reach + f.dot(camera.position)];
    [a[0], a[1], a[2], a[3], b[0], b[1], b[2], b[3], plane]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Camera;

    fn cam() -> Camera {
        Camera::new(Vec3::new(10.0, 20.0, 2.0), 0.3, -0.1)
    }

    #[test]
    fn low_detail_reaches_5_and_15_metres() {
        assert_eq!(partition_reach(crate::scene::Detail::Low), [5.0, 15.0]);
        assert_eq!(partition_reach(crate::scene::Detail::High), [10.0, 30.0]);
    }

    #[test]
    fn only_high_detail_doubles_the_partitions() {
        use crate::scene::Detail;
        assert_eq!(partition_size(Detail::Low), 512);
        assert_eq!(partition_size(Detail::Medium), 512);
        assert_eq!(partition_size(Detail::High), 1024);
    }

    #[test]
    fn the_light_view_looks_along_the_sunlight() {
        let d = Vec3::new(-0.48, -0.83, -0.3).normalize();
        let v = light_view(d);
        // Moving along the light deepens z and leaves x, y alone.
        let a = v.transform_point3(Vec3::new(1.0, 2.0, 3.0));
        let b = v.transform_point3(Vec3::new(1.0, 2.0, 3.0) + d * 4.0);
        assert!((b.z - a.z - 4.0).abs() < 1e-4);
        assert!((b.x - a.x).abs() < 1e-4 && (b.y - a.y).abs() < 1e-4);
        // Straight down still makes a rotation.
        let down = light_view(Vec3::NEG_Z);
        assert!((down.determinant() - 1.0).abs() < 1e-4);
    }

    #[test]
    fn partitions_cover_their_camera_slices() {
        let c = cam();
        let d = Vec3::new(-0.48, -0.83, -0.3).normalize();
        let p = partitions(&c, 16.0 / 9.0, d, [5.0, 15.0], &[]);
        for (k, (n, f)) in [(c.near.max(0.01), 5.0), (5.0, 15.0)]
            .into_iter()
            .enumerate()
        {
            for corner in slice_corners(&c, 16.0 / 9.0, n, f) {
                let t = p[k].texture.project_point3(corner);
                assert!((-1e-3..=1.0 + 1e-3).contains(&t.x), "{k} {t}");
                assert!((-1e-3..=1.0 + 1e-3).contains(&t.y), "{k} {t}");
                assert!((-1e-3..=1.0 + 1e-3).contains(&t.z), "{k} {t}");
            }
        }
        // A caster towards the sun from a point lands nearer (smaller depth) in the map.
        let ground = c.position + c.forward() * 3.0;
        let caster = ground - d * 2.0;
        let (g, k) = (
            p[0].texture.project_point3(ground),
            p[0].texture.project_point3(caster),
        );
        assert!(k.z < g.z);
        assert!((k.x - g.x).abs() < 1e-4 && (k.y - g.y).abs() < 1e-4);
    }

    #[test]
    fn shadow_data_marks_the_first_partition() {
        let c = cam();
        let p = partitions(&c, 1.5, Vec3::new(0.0, -0.6, -0.8), [5.0, 15.0], &[]);
        let sd = shadow_data(&p, &c);
        let depth_plane = |q: Vec3| sd[8][0] * q.x + sd[8][1] * q.y + sd[8][2] * q.z + sd[8][3];
        assert!(depth_plane(c.position + c.forward() * 4.0) > 0.0);
        assert!(depth_plane(c.position + c.forward() * 6.0) < 0.0);
        assert_eq!(sd[7][3], -10.0);
        assert_eq!(sd[0], p[0].texture.row(0).to_array());
    }

    #[test]
    fn partitions_fit_the_casters_over_their_slices() {
        let c = cam();
        let d = Vec3::new(-0.48, -0.83, -0.3).normalize();
        let open = partitions(&c, 16.0 / 9.0, d, [5.0, 15.0], &[]);
        // A caster two metres ahead, between the sun and the ground there.
        let ground = c.position + c.forward() * 2.0;
        let caster = (ground - d * 1.5, 0.5);
        let fit = partitions(&c, 16.0 / 9.0, d, [5.0, 15.0], &[caster]);
        // The box hugs the caster: it starts at the caster's near side, so the caster
        // stands near the front of the depth range rather than at its far end.
        let depth = |p: &Partition, q: Vec3| p.texture.project_point3(q).z;
        assert!(
            depth(&fit[0], caster.0) <= 0.5 + 1e-4,
            "{}",
            depth(&fit[0], caster.0)
        );
        assert!(depth(&fit[0], caster.0 - d * caster.1) < 1e-3);
        assert!(
            depth(&open[0], caster.0) > 0.8,
            "{}",
            depth(&open[0], caster.0)
        );
        // Across the light the fitted box is no larger than the slice's.
        let width = |p: &Partition| 2.0 / p.view_proj.x_axis.x;
        assert!(width(&fit[0]) <= width(&open[0]) + 1e-3);
        // A caster far off to the side does not count.
        let far = (c.position + c.forward().cross(Vec3::Z) * 200.0, 1.0);
        let same = partitions(&c, 16.0 / 9.0, d, [5.0, 15.0], &[far]);
        assert_eq!(same, open);
    }

    #[test]
    fn boxes_move_part_of_the_way_each_frame_after_a_frame_with_casters() {
        let c = cam();
        let d = Vec3::new(-0.48, -0.83, -0.3).normalize();
        let caster = (c.position + c.forward() * 2.0 - d * 1.5, 0.5);
        let (first, kept) =
            partitions_smoothed(&c, 16.0 / 9.0, d, [5.0, 15.0], &[caster], &[None, None]);
        assert_eq!(first, partitions(&c, 16.0 / 9.0, d, [5.0, 15.0], &[caster]));
        let (lo, hi) = kept[0].unwrap();
        // The caster moves a metre along the light: the box follows 0.35 of the way.
        let moved = (caster.0 + d, caster.1);
        let (_, next) = partitions_smoothed(&c, 16.0 / 9.0, d, [5.0, 15.0], &[moved], &kept);
        let fresh = partitions_smoothed(&c, 16.0 / 9.0, d, [5.0, 15.0], &[moved], &[None, None]).1;
        let (nlo, nhi) = next[0].unwrap();
        let (flo, fhi) = fresh[0].unwrap();
        assert!((nlo - (lo + (flo - lo) * BOX_SMOOTHING)).length() < 1e-4);
        assert!((nhi - (hi + (fhi - hi) * BOX_SMOOTHING)).length() < 1e-4);
        // No casters over a slice: nothing kept, so the next frame starts afresh.
        let (_, none) = partitions_smoothed(&c, 16.0 / 9.0, d, [5.0, 15.0], &[], &kept);
        assert_eq!(none, [None, None]);
    }

    #[test]
    fn slow_camera_walk_keeps_world_shadow_texels_stationary() {
        // A large fixed caster keeps both slices populated. The camera walks 5 cm
        // across the light, well inside the same whole-metre bounds. Previously a
        // fixed receiver slid continuously through the map despite static geometry.
        let mut c = cam();
        let sun = Vec3::NEG_Z;
        let casters = [(c.position, 100.0)];
        let mut previous = [None, None];
        let receiver = c.position + c.forward() * 3.0;
        let mut first = None;
        for _ in 0..60 {
            let (parts, kept) =
                partitions_smoothed(&c, 16.0 / 9.0, sun, [7.0, 20.0], &casters, &previous);
            let uv = parts.map(|p| p.texture.project_point3(receiver).truncate());
            if let Some(initial) = first {
                assert_eq!(uv, initial);
            } else {
                first = Some(uv);
            }
            previous = kept;
            c.position.x += 0.0008;
        }
    }

    #[test]
    fn projection_rounding_covers_negative_bounds_without_quantizing_history_or_depth() {
        let c = cam();
        let sun = Vec3::new(-0.48, -0.83, -0.3).normalize();
        let caster = (c.position + c.forward() * 2.0 - sun * 1.5, 0.63);
        let (parts, kept) =
            partitions_smoothed(&c, 1.5, sun, [7.0, 20.0], &[caster], &[None, None]);
        let view = light_view(sun);
        for (part, bounds) in parts.iter().zip(kept) {
            let Some((min, max)) = bounds else { continue };
            assert!(min.x < 0.0 || min.y < 0.0);
            assert_ne!(min, min.floor());
            // The rounded projection contains the unrounded fit, including negative
            // coordinates; z must still map the exact smoothed limits to 0 and 1.
            let projected_min = part
                .texture
                .project_point3(view.inverse().transform_point3(min));
            let projected_max = part
                .texture
                .project_point3(view.inverse().transform_point3(max));
            assert!((0.0..=1.0).contains(&projected_min.x));
            assert!((0.0..=1.0).contains(&projected_max.x));
            assert!((0.0..=1.0).contains(&projected_min.y));
            assert!((0.0..=1.0).contains(&projected_max.y));
            assert!(projected_min.z.abs() < 1e-4);
            assert!((projected_max.z - 1.0).abs() < 1e-4);
        }
    }
}
