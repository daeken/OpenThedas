//! Shapes: fill and line styles and the edge records of `DefineShape` 1-4, font glyphs and
//! morph shapes.

use super::SwfError;
use super::read::{Reader, Result};
use super::types::{Color, Matrix, Rect};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Spread {
    Pad,
    Reflect,
    Repeat,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Gradient {
    /// Maps gradient space (-16384..16384 twips square) to shape space.
    pub matrix: Matrix,
    pub spread: Spread,
    pub linear_rgb: bool,
    /// (ratio 0..255, colour)
    pub stops: Vec<(u8, Color)>,
    /// Focal point (-1..1) for focal radial gradients.
    pub focal: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Fill {
    Solid(Color),
    Linear(Gradient),
    Radial(Gradient),
    Focal(Gradient),
    Bitmap {
        id: u16,
        /// Maps bitmap pixels (in twips, 20 per pixel) to shape space.
        matrix: Matrix,
        repeat: bool,
        smooth: bool,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    /// Width in twips.
    pub width: u16,
    pub color: Color,
    /// `DefineShape4` line styles with a fill instead of a colour.
    pub fill: Option<Fill>,
    /// Hairline/no-scale flags of `LineStyle2`: (no horizontal scale, no vertical scale).
    pub no_scale: (bool, bool),
    pub start_cap: LineCap,
    pub end_cap: LineCap,
    pub join: LineJoin,
    /// Keep a stroke open even when its final point meets its first.
    pub no_close: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineCap {
    Round,
    None,
    Square,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LineJoin {
    Round,
    Bevel,
    Miter(f32),
}

/// One edge record, with coordinates already made absolute (twips).
#[derive(Clone, Debug, PartialEq)]
pub enum Record {
    /// Pen moves (a new subpath) and/or style changes; style indices are 1-based into the
    /// current style arrays (0 = none). `new_styles` replaces the arrays from here on.
    Style {
        move_to: Option<(i32, i32)>,
        fill0: Option<u32>,
        fill1: Option<u32>,
        line: Option<u32>,
        new_styles: Option<(Vec<Fill>, Vec<Line>)>,
    },
    Line {
        to: (i32, i32),
    },
    Curve {
        control: (i32, i32),
        to: (i32, i32),
    },
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Shape {
    pub bounds: Rect,
    pub fills: Vec<Fill>,
    pub lines: Vec<Line>,
    pub records: Vec<Record>,
}

fn read_gradient(r: &mut Reader, ver: u8, focal: bool) -> Result<Gradient> {
    let matrix = Matrix::read(r)?;
    let spread = match r.ub(2)? {
        1 => Spread::Reflect,
        2 => Spread::Repeat,
        _ => Spread::Pad,
    };
    let linear_rgb = r.ub(2)? == 1;
    let n = r.ub(4)?;
    let mut stops = Vec::with_capacity(n as usize);
    for _ in 0..n {
        let ratio = r.u8()?;
        let c = if ver >= 3 {
            Color::rgba(r)?
        } else {
            Color::rgb(r)?
        };
        stops.push((ratio, c));
    }
    let focal = if focal { r.fixed8()? } else { 0.0 };
    Ok(Gradient {
        matrix,
        spread,
        linear_rgb,
        stops,
        focal,
    })
}

pub(crate) fn read_fill(r: &mut Reader, ver: u8) -> Result<Fill> {
    let kind = r.u8()?;
    Ok(match kind {
        0x00 => Fill::Solid(if ver >= 3 {
            Color::rgba(r)?
        } else {
            Color::rgb(r)?
        }),
        0x10 => Fill::Linear(read_gradient(r, ver, false)?),
        0x12 => Fill::Radial(read_gradient(r, ver, false)?),
        0x13 => Fill::Focal(read_gradient(r, ver, true)?),
        0x40..=0x43 => {
            let id = r.u16()?;
            let matrix = Matrix::read(r)?;
            Fill::Bitmap {
                id,
                matrix,
                repeat: kind & 1 == 0,
                smooth: kind & 2 == 0,
            }
        }
        k => return Err(SwfError::Unsupported(format!("fill style {k:#x}"))),
    })
}

fn read_count(r: &mut Reader, ver: u8) -> Result<usize> {
    let n = r.u8()? as usize;
    if n == 0xff && ver >= 2 {
        Ok(r.u16()? as usize)
    } else {
        Ok(n)
    }
}

fn read_line(r: &mut Reader, ver: u8) -> Result<Line> {
    let width = r.u16()?;
    if ver < 4 {
        let color = if ver >= 3 {
            Color::rgba(r)?
        } else {
            Color::rgb(r)?
        };
        return Ok(Line {
            width,
            color,
            fill: None,
            no_scale: (false, false),
            start_cap: LineCap::Round,
            end_cap: LineCap::Round,
            join: LineJoin::Round,
            no_close: false,
        });
    }
    let cap = |bits| match bits {
        1 => LineCap::None,
        2 => LineCap::Square,
        _ => LineCap::Round,
    };
    let start_cap = cap(r.ub(2)?);
    let join = r.ub(2)?;
    let has_fill = r.flag()?;
    let no_h = r.flag()?;
    let no_v = r.flag()?;
    let _pixel_hinting = r.flag()?;
    let _reserved = r.ub(5)?;
    let no_close = r.flag()?;
    let end_cap = cap(r.ub(2)?);
    let join = match join {
        1 => LineJoin::Bevel,
        2 => LineJoin::Miter(r.u16()? as f32 / 256.0),
        _ => LineJoin::Round,
    };
    let (color, fill) = if has_fill {
        (Color::default(), Some(read_fill(r, ver)?))
    } else {
        (Color::rgba(r)?, None)
    };
    Ok(Line {
        width,
        color,
        fill,
        no_scale: (no_h, no_v),
        start_cap,
        end_cap,
        join,
        no_close,
    })
}

fn read_styles(r: &mut Reader, ver: u8) -> Result<(Vec<Fill>, Vec<Line>)> {
    let n = read_count(r, ver)?;
    let mut fills = Vec::with_capacity(n.min(256));
    for _ in 0..n {
        fills.push(read_fill(r, ver)?);
    }
    let n = read_count(r, ver)?;
    let mut lines = Vec::with_capacity(n.min(256));
    for _ in 0..n {
        lines.push(read_line(r, ver)?);
    }
    Ok((fills, lines))
}

/// Reads shape records up to the end record. `ver` is the DefineShape version (1-4; glyphs use
/// 1 with one fill and no lines).
pub(crate) fn read_records(
    r: &mut Reader,
    ver: u8,
    mut fill_bits: u32,
    mut line_bits: u32,
) -> Result<Vec<Record>> {
    let mut out = Vec::new();
    let (mut x, mut y) = (0i32, 0i32);
    loop {
        if r.flag()? {
            if r.flag()? {
                let n = r.ub(4)? + 2;
                let (dx, dy) = if r.flag()? {
                    (r.sb(n)?, r.sb(n)?)
                } else if r.flag()? {
                    (0, r.sb(n)?)
                } else {
                    (r.sb(n)?, 0)
                };
                x = x.wrapping_add(dx);
                y = y.wrapping_add(dy);
                out.push(Record::Line { to: (x, y) });
            } else {
                let n = r.ub(4)? + 2;
                let cx = x.wrapping_add(r.sb(n)?);
                let cy = y.wrapping_add(r.sb(n)?);
                x = cx.wrapping_add(r.sb(n)?);
                y = cy.wrapping_add(r.sb(n)?);
                out.push(Record::Curve {
                    control: (cx, cy),
                    to: (x, y),
                });
            }
        } else {
            let flags = r.ub(5)?;
            if flags == 0 {
                break;
            }
            let mut move_to = None;
            if flags & 1 != 0 {
                let n = r.ub(5)?;
                x = r.sb(n)?;
                y = r.sb(n)?;
                move_to = Some((x, y));
            }
            let fill0 = if flags & 2 != 0 {
                Some(r.ub(fill_bits)?)
            } else {
                None
            };
            let fill1 = if flags & 4 != 0 {
                Some(r.ub(fill_bits)?)
            } else {
                None
            };
            let line = if flags & 8 != 0 {
                Some(r.ub(line_bits)?)
            } else {
                None
            };
            let new_styles = if flags & 16 != 0 && ver >= 2 {
                let s = read_styles(r, ver)?;
                fill_bits = r.ub(4)?;
                line_bits = r.ub(4)?;
                Some(s)
            } else {
                None
            };
            out.push(Record::Style {
                move_to,
                fill0,
                fill1,
                line,
                new_styles,
            });
        }
    }
    r.align();
    Ok(out)
}

/// `DefineShape` 1-4 after the character id.
pub fn read_shape(r: &mut Reader, ver: u8) -> Result<Shape> {
    let bounds = Rect::read(r)?;
    if ver == 4 {
        let _edge_bounds = Rect::read(r)?;
        let _flags = r.u8()?;
    }
    let (fills, lines) = read_styles(r, ver)?;
    let fill_bits = r.ub(4)?;
    let line_bits = r.ub(4)?;
    let records = read_records(r, ver, fill_bits, line_bits)?;
    Ok(Shape {
        bounds,
        fills,
        lines,
        records,
    })
}

/// A glyph outline (a SHAPE: no styles, one implicit fill).
pub fn read_glyph(data: &[u8]) -> Result<Vec<Record>> {
    let mut r = Reader::new(data);
    let fill_bits = r.ub(4)?;
    let line_bits = r.ub(4)?;
    read_records(&mut r, 1, fill_bits, line_bits)
}

/// `DefineMorphShape`: start and end shapes sharing one record structure.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct MorphShape {
    pub start_bounds: Rect,
    pub end_bounds: Rect,
    /// Fills and lines as (start, end) pairs.
    pub fills: Vec<(Fill, Fill)>,
    pub lines: Vec<(Line, Line)>,
    pub start: Vec<Record>,
    pub end: Vec<Record>,
}

fn read_morph_gradient(r: &mut Reader) -> Result<(Gradient, Gradient)> {
    let m0 = Matrix::read(r)?;
    let m1 = Matrix::read(r)?;
    let n = r.u8()? & 15;
    let (mut s0, mut s1) = (Vec::new(), Vec::new());
    for _ in 0..n {
        s0.push((r.u8()?, Color::rgba(r)?));
        s1.push((r.u8()?, Color::rgba(r)?));
    }
    let g = |matrix, stops| Gradient {
        matrix,
        spread: Spread::Pad,
        linear_rgb: false,
        stops,
        focal: 0.0,
    };
    Ok((g(m0, s0), g(m1, s1)))
}

fn read_morph_fill(r: &mut Reader) -> Result<(Fill, Fill)> {
    let kind = r.u8()?;
    Ok(match kind {
        0x00 => (Fill::Solid(Color::rgba(r)?), Fill::Solid(Color::rgba(r)?)),
        0x10 => {
            let (a, b) = read_morph_gradient(r)?;
            (Fill::Linear(a), Fill::Linear(b))
        }
        0x12 => {
            let (a, b) = read_morph_gradient(r)?;
            (Fill::Radial(a), Fill::Radial(b))
        }
        0x40..=0x43 => {
            let id = r.u16()?;
            let (m0, m1) = (Matrix::read(r)?, Matrix::read(r)?);
            let f = |matrix| Fill::Bitmap {
                id,
                matrix,
                repeat: kind & 1 == 0,
                smooth: kind & 2 == 0,
            };
            (f(m0), f(m1))
        }
        k => return Err(SwfError::Unsupported(format!("morph fill {k:#x}"))),
    })
}

/// `DefineMorphShape` (version 1) after the character id.
pub fn read_morph(r: &mut Reader) -> Result<MorphShape> {
    let start_bounds = Rect::read(r)?;
    let end_bounds = Rect::read(r)?;
    let offset = r.u32()? as usize;
    let edges_at = r.pos() + offset;
    let n = read_count(r, 2)?;
    let mut fills = Vec::new();
    for _ in 0..n {
        fills.push(read_morph_fill(r)?);
    }
    let n = read_count(r, 2)?;
    let mut lines = Vec::new();
    for _ in 0..n {
        let (w0, w1) = (r.u16()?, r.u16()?);
        let (c0, c1) = (Color::rgba(r)?, Color::rgba(r)?);
        let l = |width, color| Line {
            width,
            color,
            fill: None,
            no_scale: (false, false),
            start_cap: LineCap::Round,
            end_cap: LineCap::Round,
            join: LineJoin::Round,
            no_close: false,
        };
        lines.push((l(w0, c0), l(w1, c1)));
    }
    let fb = r.ub(4)?;
    let lb = r.ub(4)?;
    let start = read_records(r, 3, fb, lb)?;
    let mut e = r.clone();
    e.align();
    let skip = edges_at.saturating_sub(e.pos());
    e.skip(skip)?;
    let fb = e.ub(4)?;
    let lb = e.ub(4)?;
    let end = read_records(&mut e, 3, fb, lb)?;
    Ok(MorphShape {
        start_bounds,
        end_bounds,
        fills,
        lines,
        start,
        end,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn square_glyph() {
        // fill bits 1, line bits 0; move to (0,0) with fill1=1; four straight edges; end.
        // Built with a tiny bit writer.
        let mut bits: Vec<bool> = Vec::new();
        let mut put = |v: u32, n: u32| {
            for i in (0..n).rev() {
                bits.push((v >> i) & 1 == 1);
            }
        };
        put(1, 4);
        put(0, 4);
        // style change: flags move(1) + fill1(4) = 0b00101
        put(0, 1);
        put(0b00101, 5);
        put(2, 5); // move bits
        put(0, 2);
        put(0, 2);
        put(1, 1); // fill1 = 1
        for (dx, dy) in [(100i32, 0i32), (0, 100), (-100, 0), (0, -100)] {
            put(1, 1);
            put(1, 1);
            put(6, 4); // n = 8
            if dx != 0 {
                put(0, 1);
                put(0, 1);
                put((dx as u32) & 0xff, 8);
            } else {
                put(0, 1);
                put(1, 1);
                put((dy as u32) & 0xff, 8);
            }
        }
        put(0, 6);
        let mut bytes = vec![0u8; bits.len().div_ceil(8)];
        for (i, b) in bits.iter().enumerate() {
            if *b {
                bytes[i / 8] |= 0x80 >> (i % 8);
            }
        }
        let recs = read_glyph(&bytes).unwrap();
        assert_eq!(recs.len(), 5);
        assert_eq!(recs[2], Record::Line { to: (100, 100) });
        assert_eq!(recs[4], Record::Line { to: (0, 0) });
    }

    #[test]
    fn truncated_shape_is_an_error() {
        assert!(read_shape(&mut Reader::new(&[0x50, 0, 0]), 1).is_err());
    }

    #[test]
    fn stroke_styles_keep_caps_join_limit_and_closure() {
        // Shape4: square start, miter join (2.5), no horizontal scale, no close,
        // flat end. The following colour also proves the flags/limit stay aligned.
        let bytes = [230, 0, 0xa4, 0x05, 0x80, 0x02, 0x12, 0x34, 0x56, 0x78];
        let line = read_line(&mut Reader::new(&bytes), 4).unwrap();
        assert_eq!(line.width, 230);
        assert_eq!(line.start_cap, LineCap::Square);
        assert_eq!(line.end_cap, LineCap::None);
        assert_eq!(line.join, LineJoin::Miter(2.5));
        assert!(line.no_close);
        assert_eq!(line.no_scale, (true, false));
        assert_eq!(
            line.color,
            Color {
                r: 0x12,
                g: 0x34,
                b: 0x56,
                a: 0x78
            }
        );

        let legacy = read_line(&mut Reader::new(&[100, 0, 10, 20, 30]), 1).unwrap();
        assert_eq!(legacy.start_cap, LineCap::Round);
        assert_eq!(legacy.end_cap, LineCap::Round);
        assert_eq!(legacy.join, LineJoin::Round);
        assert!(!legacy.no_close);
    }
}
