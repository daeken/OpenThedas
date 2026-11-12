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
//! Provisional: the boxes' limits towards the sun ([`SUN_REACH`]); the second partition
//! starts where the first ends; the engine's elevation clamp of the light (0x452ac2) and
//! its variance blur (`BlurShadowTargets`, which 0x4518e0 runs on the first partition only
//! at Medium and High, `[0xbf22c8] >= 2`; Low has none) are not applied; the bias drops
//! D3D9's half-texel offset (wgpu samples texel centres at half-texel coordinates); the
//! level's pass fits the partitions to the objects' casters a frame late.

use glam::{Mat4, Vec3, Vec4};

/// Texels on a side of each partition at Low (and Medium) detail (0x45569e).
pub const PARTITION_SIZE: u32 = 512;

/// How far each partition reaches from the camera at each Graphics Detail setting
/// (0x455731: Low, Medium, High).
pub fn partition_reach(detail: crate::scene::Detail) -> [f32; 2] {
    match detail {
        crate::scene::Detail::Low => [5.0, 15.0],
        crate::scene::Detail::Medium => [7.0, 20.0],
        crate::scene::Detail::High => [10.0, 30.0],
    }
}

/// How far towards the sun a partition's box may reach beyond its camera slice, for the
/// casters standing outside the slice (provisional, see [`partitions`]).
pub const SUN_REACH: f32 = 60.0;

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
/// the light (`z`) it reaches the casters nearer the sun, up to [`SUN_REACH`] beyond the
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
                    min.z.max(lo.z - SUN_REACH),
                ),
                Vec3::new(max.x.min(hi.x), max.y.min(hi.y), max.z.min(hi.z)),
            )
        } else {
            (Vec3::new(lo.x, lo.y, lo.z - SUN_REACH), hi)
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
}
