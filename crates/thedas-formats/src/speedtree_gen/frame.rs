//! Vectors and rotation frames for tree generation (generation.md 2.1, 2.2, 2.8).
//!
//! All arithmetic is 32-bit float in the order the spec writes it, so results match the
//! original to the last bit where the spec is exact. Provisional until checked against the
//! executable.

/// A 3-D vector. Z is up.
pub type Vec3 = [f32; 3];

/// Degrees per radian as the original divides by it for sines and cosines.
const DEGREES_PER_RADIAN: f32 = 57.29578;
/// Radians to degrees, written as the spec gives it (it rounds to the same float).
#[allow(clippy::excessive_precision)]
const RADIANS_TO_DEGREES: f32 = 57.29577951308232;

/// `sin` of radians, always as its own library call.
///
/// An optimized build would otherwise merge a `sin` and a `cos` of the same value into one
/// `sincos` call, whose results differ from the separate calls in the last bit on some
/// platforms; keeping them apart makes debug and release builds compute the same trees.
pub fn sin(x: f32) -> f32 {
    std::hint::black_box(x).sin()
}

/// `cos` of radians, always as its own library call (see [`sin`]).
pub fn cos(x: f32) -> f32 {
    std::hint::black_box(x).cos()
}

/// `sin` of an angle in degrees.
pub fn sin_deg(a: f32) -> f32 {
    sin(a / DEGREES_PER_RADIAN)
}

/// `cos` of an angle in degrees.
pub fn cos_deg(a: f32) -> f32 {
    cos(a / DEGREES_PER_RADIAN)
}

/// Radians to degrees.
pub fn degrees(radians: f32) -> f32 {
    radians * RADIANS_TO_DEGREES
}

/// `a + (b − a) · t`.
pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Component-wise [`lerp`].
pub fn lerp3(a: Vec3, b: Vec3, t: f32) -> Vec3 {
    [
        lerp(a[0], b[0], t),
        lerp(a[1], b[1], t),
        lerp(a[2], b[2], t),
    ]
}

pub fn add(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

pub fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub fn scale(a: Vec3, s: f32) -> Vec3 {
    [a[0] * s, a[1] * s, a[2] * s]
}

/// Dot product, summed x, then y, then z.
pub fn dot(a: Vec3, b: Vec3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// The right-handed cross product `a × b`.
pub fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// Normalizes as the original does (2.8): one inverse length, three multiplications; a
/// squared length at or below `100 · FLT_EPSILON` uses an inverse of 100.
pub fn normalize(v: Vec3) -> Vec3 {
    let s = dot(v, v);
    let inverse = if s > 100.0 * f32::EPSILON {
        1.0 / s.sqrt()
    } else {
        1.0 / 0.01
    };
    scale(v, inverse)
}

/// Angle between two unit vectors in radians: `acos(clamp(dot, −1, 1))`.
pub fn angle_between(a: Vec3, b: Vec3) -> f32 {
    dot(a, b).clamp(-1.0, 1.0).acos()
}

/// The fast approximate square root the original uses for branch volume (2.9 answer 7): the
/// float's bits as an int, shifted right by one (arithmetically), plus `0x1FC00000`.
pub fn fast_sqrt(x: f32) -> f32 {
    let i = x.to_bits() as i32;
    f32::from_bits(((i >> 1) + 0x1FC0_0000) as u32)
}

/// A 3×3 rotation frame stored as rows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame(pub [[f32; 3]; 3]);

impl Frame {
    pub const IDENTITY: Frame = Frame([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);

    /// The frame's growth direction: `(1, 0, 0) · M`, its first row.
    pub fn direction(&self) -> Vec3 {
        self.0[0]
    }

    /// The column form `M · v`.
    pub fn apply(&self, v: Vec3) -> Vec3 {
        let m = &self.0;
        [dot(m[0], v), dot(m[1], v), dot(m[2], v)]
    }

    /// The matrix product `self · other`.
    pub fn then(&self, other: &Frame) -> Frame {
        let (a, b) = (&self.0, &other.0);
        let mut out = [[0.0; 3]; 3];
        for (i, row) in out.iter_mut().enumerate() {
            for (j, cell) in row.iter_mut().enumerate() {
                *cell = a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j];
            }
        }
        Frame(out)
    }

    /// Rotation about Z by `a` degrees.
    pub fn about_z(a: f32) -> Frame {
        let (c, s) = (cos_deg(a), sin_deg(a));
        Frame([[c, s, 0.0], [-s, c, 0.0], [0.0, 0.0, 1.0]])
    }

    /// The combined pitch-and-yaw rotation by `y` and `z` degrees.
    pub fn pitch_yaw(y: f32, z: f32) -> Frame {
        let (cy, sy, cz, sz) = (cos_deg(y), sin_deg(y), cos_deg(z), sin_deg(z));
        Frame([
            [cy * cz, cy * sz, -sy],
            [-sz, cz, 0.0],
            [sy * cz, sy * sz, cy],
        ])
    }

    /// Rotation about the unit `axis` by `a` degrees. A zero axis degenerates to a scale by
    /// `cos a`, as in the original.
    pub fn about_axis(axis: Vec3, a: f32) -> Frame {
        let (c, s) = (cos_deg(a), sin_deg(a));
        let t = 1.0 - c;
        let [x, y, z] = axis;
        Frame([
            [t * x * x + c, t * x * y + s * z, t * x * z - s * y],
            [t * x * y - s * z, t * y * y + c, t * y * z + s * x],
            [t * x * z + s * y, t * y * z - s * x, t * z * z + c],
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Vec3, b: Vec3) -> bool {
        a.iter().zip(&b).all(|(x, y)| (x - y).abs() < 1e-5)
    }

    #[test]
    fn rotations_carry_the_growth_direction() {
        // A pitch of −90 turns the growth axis X straight up.
        assert!(close(
            Frame::pitch_yaw(-90.0, 0.0).direction(),
            [0.0, 0.0, 1.0]
        ));
        // A yaw of 90 turns X to Y.
        assert!(close(
            Frame::pitch_yaw(0.0, 90.0).direction(),
            [0.0, 1.0, 0.0]
        ));
        assert!(close(Frame::about_z(90.0).direction(), [0.0, 1.0, 0.0]));
        // About Z by an axis rotation agrees with the Z rotation.
        let a = Frame::about_axis([0.0, 0.0, 1.0], 37.0);
        let b = Frame::about_z(37.0);
        for (ra, rb) in a.0.iter().zip(&b.0) {
            assert!(close(*ra, *rb));
        }
    }

    #[test]
    fn composition_is_the_matrix_product() {
        let a = Frame::pitch_yaw(20.0, 30.0);
        let b = Frame::about_z(50.0);
        let ab = a.then(&b);
        // Row-vector convention: (v · A) · B = v · (A·B).
        let v = a.direction();
        let vb = [
            v[0] * b.0[0][0] + v[1] * b.0[1][0] + v[2] * b.0[2][0],
            v[0] * b.0[0][1] + v[1] * b.0[1][1] + v[2] * b.0[2][1],
            v[0] * b.0[0][2] + v[1] * b.0[1][2] + v[2] * b.0[2][2],
        ];
        assert!(close(ab.direction(), vb));
        assert_eq!(Frame::IDENTITY.then(&a), a);
    }

    #[test]
    fn normalize_follows_the_small_length_rule() {
        assert_eq!(normalize([0.0, 0.0, 0.0]), [0.0, 0.0, 0.0]);
        assert_eq!(normalize([1e-4, 0.0, 0.0]), [1e-4 * 100.0, 0.0, 0.0]);
        let n = normalize([3.0, 4.0, 0.0]);
        assert!(close(n, [0.6, 0.8, 0.0]));
    }

    #[test]
    fn a_zero_axis_scales_by_cos() {
        let m = Frame::about_axis([0.0; 3], 60.0);
        let c = cos_deg(60.0);
        assert_eq!(m.0, [[c, 0.0, 0.0], [0.0, c, 0.0], [0.0, 0.0, c]]);
    }

    #[test]
    fn fast_sqrt_is_close() {
        for x in [0.25f32, 1.0, 2.0, 9.0, 100.0, 12345.0] {
            let r = fast_sqrt(x);
            assert!((r - x.sqrt()).abs() / x.sqrt() < 0.07, "{x}: {r}");
        }
        assert_eq!(fast_sqrt(4.0), 2.0);
    }
}
