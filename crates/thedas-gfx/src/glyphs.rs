//! Glyphs rasterised with anti-aliasing at the size they are drawn, packed into atlas pages,
//! as Scaleform's glyph cache did. Text then draws as textured quads: cheap to rebuild, and
//! smooth without multisampling.

use std::collections::HashMap;

use crate::swf::Record;

/// A glyph's coverage at one pixel size: `w` x `h` alpha, placed `left`/`top` pixels from the
/// pen (top is negative above the baseline).
#[derive(Clone, Debug, PartialEq)]
pub struct GlyphBitmap {
    pub w: u32,
    pub h: u32,
    pub left: i32,
    pub top: i32,
    pub alpha: Vec<u8>,
}

/// Flattens an outline (font units) into line segments in pixels (`scale` pixels per unit).
fn segments(records: &[Record], scale: f32) -> Vec<[(f32, f32); 2]> {
    let mut out = Vec::new();
    let (mut px, mut py) = (0.0f32, 0.0f32);
    let mut start = (0.0f32, 0.0f32);
    let mut open = false;
    let close = |out: &mut Vec<[(f32, f32); 2]>, from: (f32, f32), to: (f32, f32)| {
        if from != to {
            out.push([from, to]);
        }
    };
    for r in records {
        match r {
            Record::Style { move_to, .. } => {
                if let Some((x, y)) = move_to {
                    if open {
                        close(&mut out, (px, py), start);
                    }
                    px = *x as f32 * scale;
                    py = *y as f32 * scale;
                    start = (px, py);
                    open = true;
                }
            }
            Record::Line { to } => {
                let p = (to.0 as f32 * scale, to.1 as f32 * scale);
                out.push([(px, py), p]);
                (px, py) = p;
                open = true;
            }
            Record::Curve { control, to } => {
                let c = (control.0 as f32 * scale, control.1 as f32 * scale);
                let e = (to.0 as f32 * scale, to.1 as f32 * scale);
                // Enough steps that the chord stays within a tenth of a pixel.
                let dd = ((px - 2.0 * c.0 + e.0).powi(2) + (py - 2.0 * c.1 + e.1).powi(2)).sqrt();
                let n = ((dd / 0.4).sqrt().ceil() as usize).clamp(1, 64);
                let mut prev = (px, py);
                for i in 1..=n {
                    let t = i as f32 / n as f32;
                    let u = 1.0 - t;
                    let q = (
                        u * u * px + 2.0 * u * t * c.0 + t * t * e.0,
                        u * u * py + 2.0 * u * t * c.1 + t * t * e.1,
                    );
                    out.push([prev, q]);
                    prev = q;
                }
                (px, py) = e;
                open = true;
            }
        }
    }
    if open {
        close(&mut out, (px, py), start);
    }
    out
}

/// Signed-area coverage accumulation (one cell per pixel plus a spill column), the
/// approach of the font-rs rasteriser: each edge adds the area it covers to the cells it
/// crosses, and a running sum along each row gives coverage. Overlapping contours of the
/// same direction add up and clamp, as nonzero filling does.
struct Raster {
    w: usize,
    h: usize,
    acc: Vec<f32>,
}

impl Raster {
    fn add(&mut self, i: isize, v: f32) {
        if i >= 0 {
            if let Some(c) = self.acc.get_mut(i as usize) {
                *c += v;
            }
        }
    }

    fn line(&mut self, p0: (f32, f32), p1: (f32, f32)) {
        if (p0.1 - p1.1).abs() <= 1e-6 {
            return;
        }
        let (dir, p0, p1) = if p0.1 < p1.1 {
            (1.0, p0, p1)
        } else {
            (-1.0, p1, p0)
        };
        let dxdy = (p1.0 - p0.0) / (p1.1 - p0.1);
        let mut x = p0.0;
        if p0.1 < 0.0 {
            x -= p0.1 * dxdy;
        }
        let y_start = p0.1.max(0.0) as usize;
        let y_end = (p1.1.ceil().max(0.0) as usize).min(self.h);
        for y in y_start..y_end {
            let row = (y * self.w) as isize;
            let dy = ((y + 1) as f32).min(p1.1) - (y as f32).max(p0.1);
            let xnext = x + dxdy * dy;
            let d = dy * dir;
            let (x0, x1) = if x < xnext { (x, xnext) } else { (xnext, x) };
            let x0f = x0.floor();
            let x0i = x0f as isize;
            let x1c = x1.ceil();
            let x1i = x1c as isize;
            if x1i <= x0i + 1 {
                let xm = 0.5 * (x + xnext) - x0f;
                self.add(row + x0i, d - d * xm);
                self.add(row + x0i + 1, d * xm);
            } else {
                let s = 1.0 / (x1 - x0);
                let x0frac = x0 - x0f;
                let a0 = 0.5 * s * (1.0 - x0frac) * (1.0 - x0frac);
                let x1frac = x1 - x1c + 1.0;
                let am = 0.5 * s * x1frac * x1frac;
                self.add(row + x0i, d * a0);
                if x1i == x0i + 2 {
                    self.add(row + x0i + 1, d * (1.0 - a0 - am));
                } else {
                    let a1 = s * (1.5 - x0frac);
                    self.add(row + x0i + 1, d * (a1 - a0));
                    for xi in x0i + 2..x1i - 1 {
                        self.add(row + xi, d * s);
                    }
                    let a2 = a1 + (x1i - x0i - 3) as f32 * s;
                    self.add(row + x1i - 1, d * (1.0 - a2 - am));
                }
                self.add(row + x1i, d * am);
            }
            x = xnext;
        }
    }
}

/// Rasterises a glyph outline (font units) at `scale` pixels per unit. `None` for an
/// empty glyph (a space) or one too large to cache.
pub fn rasterize(records: &[Record], scale: f32) -> Option<GlyphBitmap> {
    if !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    let segs = segments(records, scale);
    if segs.is_empty() {
        return None;
    }
    let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for s in &segs {
        for p in s {
            x0 = x0.min(p.0);
            y0 = y0.min(p.1);
            x1 = x1.max(p.0);
            y1 = y1.max(p.1);
        }
    }
    // One pixel of padding all round, so bilinear sampling never reaches a neighbour.
    let left = x0.floor() as i32 - 1;
    let top = y0.floor() as i32 - 1;
    let w = (x1.ceil() as i32 + 1 - left).max(1) as usize;
    let h = (y1.ceil() as i32 + 1 - top).max(1) as usize;
    if w > 512 || h > 512 {
        return None;
    }
    let mut r = Raster {
        w,
        h,
        acc: vec![0.0; w * h + 1],
    };
    for s in &segs {
        let a = (s[0].0 - left as f32, s[0].1 - top as f32);
        let b = (s[1].0 - left as f32, s[1].1 - top as f32);
        r.line(a, b);
    }
    let mut alpha = Vec::with_capacity(w * h);
    let mut sum = 0.0f32;
    for c in &r.acc[..w * h] {
        sum += c;
        alpha.push((sum.abs().min(1.0) * 255.0 + 0.5) as u8);
    }
    Some(GlyphBitmap {
        w: w as u32,
        h: h as u32,
        left,
        top,
        alpha,
    })
}

/// Where a glyph lies in the atlas: its page, its texture rectangle (0..1) and its pixel
/// box relative to the pen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placed {
    pub page: usize,
    pub uv: [f32; 4],
    pub left: f32,
    pub top: f32,
    pub w: f32,
    pub h: f32,
}

/// Packs glyph bitmaps into square pages with shelves (rows of glyphs of similar height).
pub struct Packer {
    pub size: u32,
    pages: usize,
    x: u32,
    y: u32,
    row: u32,
    entries: HashMap<(usize, usize, u32), Option<Placed>>,
}

impl Packer {
    pub fn new(size: u32) -> Self {
        Self {
            size,
            pages: 0,
            x: 0,
            y: 0,
            row: 0,
            entries: HashMap::new(),
        }
    }

    pub fn pages(&self) -> usize {
        self.pages
    }

    /// The glyph already placed for (font, glyph, size key), if any (`Some(None)`: it has no
    /// pixels or did not fit).
    pub fn get(&self, key: (usize, usize, u32)) -> Option<Option<Placed>> {
        self.entries.get(&key).copied()
    }

    /// Finds room for a `w` x `h` bitmap; returns its page and top-left corner, opening a
    /// new page when the current one is full.
    pub fn place(&mut self, w: u32, h: u32) -> Option<(usize, u32, u32)> {
        if w > self.size || h > self.size {
            return None;
        }
        if self.pages == 0 {
            self.pages = 1;
        }
        if self.x + w > self.size {
            self.x = 0;
            self.y += self.row;
            self.row = 0;
        }
        if self.y + h > self.size {
            self.pages += 1;
            self.x = 0;
            self.y = 0;
            self.row = 0;
        }
        let at = (self.pages - 1, self.x, self.y);
        self.x += w;
        self.row = self.row.max(h);
        Some(at)
    }

    pub fn insert(&mut self, key: (usize, usize, u32), placed: Option<Placed>) {
        self.entries.insert(key, placed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(x0: i32, y0: i32, x1: i32, y1: i32, clockwise: bool) -> Vec<Record> {
        let pts = if clockwise {
            [(x0, y0), (x1, y0), (x1, y1), (x0, y1)]
        } else {
            [(x0, y0), (x0, y1), (x1, y1), (x1, y0)]
        };
        let mut r = vec![Record::Style {
            move_to: Some(pts[0]),
            fill0: None,
            fill1: Some(1),
            line: None,
            new_styles: None,
        }];
        r.extend(pts[1..].iter().map(|p| Record::Line { to: *p }));
        r.push(Record::Line { to: pts[0] });
        r
    }

    fn at(b: &GlyphBitmap, x: i32, y: i32) -> u8 {
        b.alpha[((y - b.top) * b.w as i32 + (x - b.left)) as usize]
    }

    #[test]
    fn a_square_covers_its_pixels_and_antialiases_half_pixels() {
        // 10 x 10 units at 1 px/unit, offset by half a pixel on the left.
        let mut r = square(0, 0, 10, 10, true);
        if let Record::Style { move_to, .. } = &mut r[0] {
            *move_to = Some((0, 0));
        }
        let b = rasterize(&r, 1.0).unwrap();
        assert_eq!(at(&b, 5, 5), 255);
        assert_eq!(at(&b, -1, 5), 0);
        assert_eq!(at(&b, 10, 5), 0);
        let half = rasterize(&square(1, 0, 21, 20, true), 0.5).unwrap();
        assert!(
            (120..=135).contains(&at(&half, 0, 5)),
            "{}",
            at(&half, 0, 5)
        );
    }

    #[test]
    fn overlaps_stay_solid_and_counters_stay_open() {
        let mut r = square(0, 0, 20, 20, true);
        r.extend(square(10, 10, 30, 30, true));
        let b = rasterize(&r, 1.0).unwrap();
        assert_eq!(at(&b, 15, 15), 255);
        let mut r = square(0, 0, 30, 30, true);
        r.extend(square(10, 10, 20, 20, false));
        let b = rasterize(&r, 1.0).unwrap();
        assert_eq!(at(&b, 15, 15), 0);
        assert_eq!(at(&b, 5, 15), 255);
        assert!(rasterize(&[], 1.0).is_none());
    }

    #[test]
    fn packer_fills_shelves_then_opens_pages() {
        let mut p = Packer::new(64);
        assert_eq!(p.place(30, 10), Some((0, 0, 0)));
        assert_eq!(p.place(30, 12), Some((0, 30, 0)));
        assert_eq!(p.place(30, 10), Some((0, 0, 12)));
        assert_eq!(p.place(64, 50), Some((1, 0, 0)));
        assert_eq!(p.place(65, 1), None);
    }
}
