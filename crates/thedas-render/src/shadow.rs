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
//! Provisional: the light-space bounds (the orthographic box covers the camera slice's
//! corners and reaches towards the sun by [`SUN_REACH`] for casters outside it, where the
//! engine gathers the casters' spheres and smooths the box over frames, 0x453266); the
//! second partition starts where the first ends; the engine's elevation clamp of the light
//! (0x452ac2) and its variance blur (`BlurShadowTargets`) are not applied; the bias drops
//! D3D9's half-texel offset (wgpu samples texel centres at half-texel coordinates).

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

/// How far towards the sun a partition's box reaches beyond its camera slice, for the
/// casters standing outside the slice (provisional, see the module docs).
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

/// The two partitions for a camera and the sunlight's direction.
pub fn partitions(
    camera: &crate::Camera,
    aspect: f32,
    sun_dir: Vec3,
    reach: [f32; 2],
) -> [Partition; 2] {
    let view = light_view(sun_dir);
    let near = camera.near.max(0.01);
    let ranges = [(near, reach[0]), (reach[0], reach[1])];
    ranges.map(|(n, f)| {
        let corners = slice_corners(camera, aspect, n, f);
        let mut min = Vec3::splat(f32::INFINITY);
        let mut max = Vec3::splat(f32::NEG_INFINITY);
        for c in corners {
            let p = view.transform_point3(c);
            min = min.min(p);
            max = max.max(p);
        }
        min.z -= SUN_REACH;
        let view_proj = ortho(min, max) * view;
        Partition {
            view_proj,
            texture: bias() * view_proj,
            reach: f,
        }
    })
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
        let p = partitions(&c, 16.0 / 9.0, d, [5.0, 15.0]);
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
        let p = partitions(&c, 1.5, Vec3::new(0.0, -0.6, -0.8), [5.0, 15.0]);
        let sd = shadow_data(&p, &c);
        let depth_plane = |q: Vec3| sd[8][0] * q.x + sd[8][1] * q.y + sd[8][2] * q.z + sd[8][3];
        assert!(depth_plane(c.position + c.forward() * 4.0) > 0.0);
        assert!(depth_plane(c.position + c.forward() * 6.0) < 0.0);
        assert_eq!(sd[7][3], -10.0);
        assert_eq!(sd[0], p[0].texture.row(0).to_array());
    }
}
