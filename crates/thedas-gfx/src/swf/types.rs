//! Basic SWF records: rectangles, matrices, colour transforms, colours.

use super::read::{Reader, Result};

/// A rectangle in twips (1/20 pixel).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x_min: i32,
    pub x_max: i32,
    pub y_min: i32,
    pub y_max: i32,
}

impl Rect {
    pub fn read(r: &mut Reader) -> Result<Self> {
        r.align();
        let n = r.ub(5)?;
        let v = Self {
            x_min: r.sb(n)?,
            x_max: r.sb(n)?,
            y_min: r.sb(n)?,
            y_max: r.sb(n)?,
        };
        r.align();
        Ok(v)
    }

    pub fn width(&self) -> i32 {
        self.x_max - self.x_min
    }

    pub fn height(&self) -> i32 {
        self.y_max - self.y_min
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub fn rgb(r: &mut Reader) -> Result<Self> {
        Ok(Self {
            r: r.u8()?,
            g: r.u8()?,
            b: r.u8()?,
            a: 255,
        })
    }

    pub fn rgba(r: &mut Reader) -> Result<Self> {
        Ok(Self {
            r: r.u8()?,
            g: r.u8()?,
            b: r.u8()?,
            a: r.u8()?,
        })
    }

    /// Flash's filter colours are stored as RGBA too.
    pub fn from_rgb_u32(v: u32) -> Self {
        Self {
            r: (v >> 16) as u8,
            g: (v >> 8) as u8,
            b: v as u8,
            a: 255,
        }
    }
}

/// A 2x3 affine matrix: `x' = a*x + c*y + tx`, `y' = b*x + d*y + ty`; translation in twips.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Matrix {
    pub a: f32,
    pub b: f32,
    pub c: f32,
    pub d: f32,
    pub tx: f32,
    pub ty: f32,
}

impl Default for Matrix {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Matrix {
    pub const IDENTITY: Self = Self {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        tx: 0.0,
        ty: 0.0,
    };

    pub fn read(r: &mut Reader) -> Result<Self> {
        r.align();
        let mut m = Self::IDENTITY;
        if r.flag()? {
            let n = r.ub(5)?;
            m.a = r.fb(n)?;
            m.d = r.fb(n)?;
        }
        if r.flag()? {
            let n = r.ub(5)?;
            m.b = r.fb(n)?;
            m.c = r.fb(n)?;
        }
        let n = r.ub(5)?;
        m.tx = r.sb(n)? as f32;
        m.ty = r.sb(n)? as f32;
        r.align();
        Ok(m)
    }

    pub fn translate(tx: f32, ty: f32) -> Self {
        Self {
            tx,
            ty,
            ..Self::IDENTITY
        }
    }

    pub fn scale(sx: f32, sy: f32) -> Self {
        Self {
            a: sx,
            d: sy,
            ..Self::IDENTITY
        }
    }

    /// `self * other`: apply `other` first, then `self`.
    pub fn mul(&self, o: &Self) -> Self {
        Self {
            a: self.a * o.a + self.c * o.b,
            b: self.b * o.a + self.d * o.b,
            c: self.a * o.c + self.c * o.d,
            d: self.b * o.c + self.d * o.d,
            tx: self.a * o.tx + self.c * o.ty + self.tx,
            ty: self.b * o.tx + self.d * o.ty + self.ty,
        }
    }

    pub fn apply(&self, x: f32, y: f32) -> (f32, f32) {
        (
            self.a * x + self.c * y + self.tx,
            self.b * x + self.d * y + self.ty,
        )
    }

    pub fn invert(&self) -> Option<Self> {
        let det = self.a * self.d - self.b * self.c;
        if det.abs() < 1e-12 {
            return None;
        }
        let (a, b, c, d) = (self.d / det, -self.b / det, -self.c / det, self.a / det);
        Some(Self {
            a,
            b,
            c,
            d,
            tx: -(a * self.tx + c * self.ty),
            ty: -(b * self.tx + d * self.ty),
        })
    }

    /// The bounds of `rect` under this matrix.
    pub fn transform_rect(&self, r: &Rect) -> Rect {
        let pts = [
            self.apply(r.x_min as f32, r.y_min as f32),
            self.apply(r.x_max as f32, r.y_min as f32),
            self.apply(r.x_min as f32, r.y_max as f32),
            self.apply(r.x_max as f32, r.y_max as f32),
        ];
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for (x, y) in pts {
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
        Rect {
            x_min: x0.floor() as i32,
            x_max: x1.ceil() as i32,
            y_min: y0.floor() as i32,
            y_max: y1.ceil() as i32,
        }
    }
}

/// A colour transform: `c' = c * mul + add`, per channel, with `mul` in 1.0 units and `add`
/// in 0..255 units.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColorTransform {
    pub mul: [f32; 4],
    pub add: [f32; 4],
}

impl Default for ColorTransform {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl ColorTransform {
    pub const IDENTITY: Self = Self {
        mul: [1.0; 4],
        add: [0.0; 4],
    };

    pub fn read(r: &mut Reader, alpha: bool) -> Result<Self> {
        r.align();
        let has_add = r.flag()?;
        let has_mul = r.flag()?;
        let n = r.ub(4)?;
        let k = if alpha { 4 } else { 3 };
        let mut cx = Self::IDENTITY;
        if has_mul {
            for i in 0..k {
                cx.mul[i] = r.sb(n)? as f32 / 256.0;
            }
        }
        if has_add {
            for i in 0..k {
                cx.add[i] = r.sb(n)? as f32;
            }
        }
        r.align();
        Ok(cx)
    }

    /// `self` applied after `inner` (a parent's transform over a child's).
    pub fn concat(&self, inner: &Self) -> Self {
        let mut o = Self::IDENTITY;
        for i in 0..4 {
            o.mul[i] = self.mul[i] * inner.mul[i];
            o.add[i] = self.mul[i] * inner.add[i] + self.add[i];
        }
        o
    }

    pub fn is_identity(&self) -> bool {
        *self == Self::IDENTITY
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matrix_inverse_round_trip() {
        let m = Matrix {
            a: 2.0,
            b: 0.5,
            c: -0.25,
            d: 1.5,
            tx: 100.0,
            ty: -40.0,
        };
        let i = m.invert().unwrap();
        let (x, y) = m.apply(3.0, 7.0);
        let (x2, y2) = i.apply(x, y);
        assert!((x2 - 3.0).abs() < 1e-4 && (y2 - 7.0).abs() < 1e-4);
    }

    #[test]
    fn identity_matrix_record() {
        // No scale, no rotate, 0-bit translate.
        let mut r = Reader::new(&[0b0000_0000]);
        assert_eq!(Matrix::read(&mut r).unwrap(), Matrix::IDENTITY);
    }
}
