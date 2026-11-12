//! Nine-slice scaling (`DefineScalingGrid`) for players that ignore it.
//!
//! A sprite with a scaling grid keeps its corners at their size when it is scaled: only the
//! middle stretches. Ruffle stores the grid but draws the sprite scaled as a whole, so a
//! panel built from a small framed image (optionsmenu.gfx's background, 87 by 103 pixels
//! stretched over the screen) came out as a smear of its border. [`plan`] finds the grid
//! sprites whose only content is one placed shape and [`Plan::rewrite_sprite`] splits each
//! into nine pieces: each piece shows the shape through a rectangular mask over one cell of
//! the grid. The host lays the pieces out as the sprite's scale changes ([`layout`]): the
//! corner cells keep their size, the edges stretch one way and the middle both. The sprite
//! registers itself for that in a frame action (`_global.thedas_s9`, its pieces named
//! `thedas_s9_0` to `thedas_s9_8`, row by row).
//!
//! Provisional: Flash's own rule when a sprite is scaled smaller than its corners is not
//! modelled (the pieces then scale as one), nor a grid sprite scaled by its parent rather than
//! itself.

use std::collections::BTreeMap;

use super::{TAG_END, TAG_SHOW_FRAME, Tag, read_tags, tag_header};

const TAG_DEFINE_SHAPE: u16 = 2;
const TAG_PLACE_OBJECT: u16 = 4;
const TAG_DO_ACTION: u16 = 12;
const TAG_DEFINE_SHAPE2: u16 = 22;
const TAG_PLACE_OBJECT2: u16 = 26;
const TAG_DEFINE_SHAPE3: u16 = 32;
const TAG_DEFINE_SPRITE: u16 = 39;
const TAG_PLACE_OBJECT3: u16 = 70;
const TAG_DEFINE_SCALING_GRID: u16 = 78;
const TAG_DEFINE_SHAPE4: u16 = 83;

/// A rectangle in twips: x0, y0, x1, y1.
pub(super) type Rect = [i32; 4];

/// A grid sprite's content bounds and grid, in pixels (x0, y0, x1, y1), for [`layout`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scale9 {
    pub bounds: [f64; 4],
    pub grid: [f64; 4],
}

/// Where each of a grid sprite's nine pieces goes for the sprite's scale (`sx`, `sy`, as
/// fractions): (x, y, x scale, y scale) in the sprite's pixels and per cent, row by row.
pub fn layout(s: &Scale9, sx: f64, sy: f64) -> [[f64; 4]; 9] {
    // One axis: (offset, scale) for the low, middle and high cells.
    let axis = |b0: f64, b1: f64, g0: f64, g1: f64, s: f64| -> [(f64, f64); 3] {
        let (low, high, mid) = (g0 - b0, b1 - g1, g1 - g0);
        if !(s.is_finite() && s > 0.0) || mid <= 0.0 || s * (b1 - b0) < low + high {
            return [(0.0, 1.0); 3];
        }
        let a_mid = ((b1 - b0) - (low + high) / s) / mid;
        [
            (b0 - b0 / s, 1.0 / s),
            ((b0 + low / s) - a_mid * g0, a_mid),
            (b1 - b1 / s, 1.0 / s),
        ]
    };
    let [bx0, by0, bx1, by1] = s.bounds;
    let [gx0, gy0, gx1, gy1] = s.grid;
    let xs = axis(bx0, bx1, gx0, gx1, sx);
    let ys = axis(by0, by1, gy0, gy1, sy);
    let mut out = [[0.0; 4]; 9];
    for (r, (y, ay)) in ys.iter().enumerate() {
        for (c, (x, ax)) in xs.iter().enumerate() {
            out[r * 3 + c] = [*x, *y, ax * 100.0, ay * 100.0];
        }
    }
    out
}

/// Bit reader over a byte slice (SWF bit fields, most significant first).
struct Bits<'a> {
    d: &'a [u8],
    pos: usize,
}

impl Bits<'_> {
    fn u(&mut self, n: usize) -> Option<u32> {
        let mut v = 0u32;
        for _ in 0..n {
            let byte = *self.d.get(self.pos / 8)?;
            v = (v << 1) | ((byte >> (7 - self.pos % 8)) & 1) as u32;
            self.pos += 1;
        }
        Some(v)
    }

    fn s(&mut self, n: usize) -> Option<i32> {
        let v = self.u(n)?;
        Some(if n > 0 && v & (1 << (n - 1)) != 0 {
            (v as i64 - (1i64 << n)) as i32
        } else {
            v as i32
        })
    }

    fn byte_end(&self) -> usize {
        self.pos.div_ceil(8)
    }
}

pub(super) fn read_rect(d: &[u8]) -> Option<(Rect, usize)> {
    let mut b = Bits { d, pos: 0 };
    let n = b.u(5)? as usize;
    let (x0, x1, y0, y1) = (b.s(n)?, b.s(n)?, b.s(n)?, b.s(n)?);
    Some(([x0, y0, x1, y1], b.byte_end()))
}

/// A placement matrix without rotation: scale x, scale y, translate x, y (twips). `None` for a
/// rotated or skewed one.
pub(super) fn read_matrix(d: &[u8]) -> Option<(Option<[f64; 4]>, usize)> {
    let mut b = Bits { d, pos: 0 };
    let (mut sx, mut sy) = (1.0, 1.0);
    if b.u(1)? == 1 {
        let n = b.u(5)? as usize;
        sx = b.s(n)? as f64 / 65536.0;
        sy = b.s(n)? as f64 / 65536.0;
    }
    let mut rotated = false;
    if b.u(1)? == 1 {
        let n = b.u(5)? as usize;
        let (r0, r1) = (b.s(n)?, b.s(n)?);
        rotated = r0 != 0 || r1 != 0;
    }
    let n = b.u(5)? as usize;
    let (tx, ty) = (b.s(n)? as f64, b.s(n)? as f64);
    Some(((!rotated).then_some([sx, sy, tx, ty]), b.byte_end()))
}

/// Bit writer for shape records and rectangles.
#[derive(Default)]
struct BitOut {
    out: Vec<u8>,
    bits: usize,
}

impl BitOut {
    fn put(&mut self, v: u32, n: usize) {
        for i in (0..n).rev() {
            if self.bits % 8 == 0 {
                self.out.push(0);
            }
            if (v >> i) & 1 == 1 {
                let last = self.out.len() - 1;
                self.out[last] |= 1 << (7 - self.bits % 8);
            }
            self.bits += 1;
        }
    }

    fn signed(&mut self, v: i32, n: usize) {
        self.put((v as u32) & ((1u64 << n) - 1) as u32, n);
    }
}

/// Bits a signed value needs.
fn sbits(vs: &[i32]) -> usize {
    vs.iter()
        .map(|&v| {
            let m = if v < 0 { !v } else { v };
            33 - m.leading_zeros() as usize
        })
        .max()
        .unwrap_or(1)
        .max(2)
}

fn rect_bytes(r: Rect, out: &mut BitOut) {
    let n = sbits(&[r[0], r[2], r[1], r[3]]);
    out.put(n as u32, 5);
    for v in [r[0], r[2], r[1], r[3]] {
        out.signed(v, n);
    }
}

/// `DefineShape` of a solid rectangle (a mask: its colour does not show).
fn rect_shape(id: u16, r: Rect) -> Vec<u8> {
    let mut b = BitOut::default();
    rect_bytes(r, &mut b);
    let mut body = id.to_le_bytes().to_vec();
    body.extend(b.out);
    // One solid fill, no line styles; one fill bit, no line bits.
    body.extend_from_slice(&[1, 0x00, 0, 0, 0, 0, 0x10]);
    let mut s = BitOut::default();
    // Style change: move to (x0, y0), fill style 1 on the right.
    s.put(0, 1);
    s.put(0b00101, 5);
    let n = sbits(&[r[0], r[1]]);
    s.put(n as u32, 5);
    s.signed(r[0], n);
    s.signed(r[1], n);
    s.put(1, 1);
    // Four straight edges round the rectangle.
    let (w, h) = (r[2] - r[0], r[3] - r[1]);
    for (vertical, d) in [(false, w), (true, h), (false, -w), (true, -h)] {
        let n = sbits(&[d]);
        s.put(1, 1);
        s.put(1, 1);
        s.put(n as u32 - 2, 4);
        s.put(0, 1);
        s.put(vertical as u32, 1);
        s.signed(d, n);
    }
    s.put(0, 6);
    body.extend(s.out);
    let mut out = Vec::new();
    tag_header(TAG_DEFINE_SHAPE, body.len(), &mut out);
    out.extend(body);
    out
}

/// The fill style of a shape filled with exactly one bitmap and drawn with no line styles
/// (the GUI's framed panels): its bytes (type 0x40-0x43, bitmap id, matrix).
fn single_bitmap_fill(code: u16, body: &[u8]) -> Option<Vec<u8>> {
    let (_, n) = read_rect(body.get(2..)?)?;
    let mut o = 2 + n;
    if code == TAG_DEFINE_SHAPE4 {
        // Edge bounds, then a flags byte.
        let (_, n) = read_rect(body.get(o..)?)?;
        o += n + 1;
    }
    if *body.get(o)? != 1 {
        return None;
    }
    o += 1;
    let start = o;
    let kind = *body.get(o)?;
    if !(0x40..=0x43).contains(&kind) {
        return None;
    }
    o += 3;
    let (_, n) = read_matrix_bytes(body.get(o..)?)?;
    o += n;
    // No line styles.
    (*body.get(o)? == 0).then(|| body[start..o].to_vec())
}

/// The length of a MATRIX record (any matrix, rotated or not).
fn read_matrix_bytes(d: &[u8]) -> Option<((), usize)> {
    let mut b = Bits { d, pos: 0 };
    for _ in 0..2 {
        if b.u(1)? == 1 {
            let n = b.u(5)? as usize;
            b.s(n)?;
            b.s(n)?;
        }
    }
    let n = b.u(5)? as usize;
    b.s(n)?;
    b.s(n)?;
    Some(((), b.byte_end()))
}

/// `DefineShape3` of a rectangle filled with `fill` (a fill style's bytes, see
/// [`single_bitmap_fill`]): one cell of a bitmap-filled shape, in the shape's own coordinates.
fn filled_rect_shape(id: u16, r: Rect, fill: &[u8]) -> Vec<u8> {
    let mut b = BitOut::default();
    rect_bytes(r, &mut b);
    let mut body = id.to_le_bytes().to_vec();
    body.extend(b.out);
    // One fill style, no line styles; one fill bit, no line bits.
    body.push(1);
    body.extend_from_slice(fill);
    body.extend_from_slice(&[0, 0x10]);
    let mut s = BitOut::default();
    s.put(0, 1);
    s.put(0b00101, 5);
    let n = sbits(&[r[0], r[1]]);
    s.put(n as u32, 5);
    s.signed(r[0], n);
    s.signed(r[1], n);
    s.put(1, 1);
    let (w, h) = (r[2] - r[0], r[3] - r[1]);
    for (vertical, d) in [(false, w), (true, h), (false, -w), (true, -h)] {
        let n = sbits(&[d]).max(2);
        s.put(1, 1);
        s.put(1, 1);
        s.put(n as u32 - 2, 4);
        s.put(0, 1);
        s.put(vertical as u32, 1);
        s.signed(d, n);
    }
    s.put(0, 6);
    body.extend(s.out);
    let mut out = Vec::new();
    tag_header(TAG_DEFINE_SHAPE3, body.len(), &mut out);
    out.extend(body);
    out
}

/// `PlaceObject2` of a character at a depth, with a name or a clip depth.
fn place(depth: u16, ch: u16, name: Option<&str>, clip: Option<u16>) -> Vec<u8> {
    let flags =
        0x02 | if name.is_some() { 0x20 } else { 0 } | if clip.is_some() { 0x40 } else { 0 };
    let mut body = vec![flags];
    body.extend_from_slice(&depth.to_le_bytes());
    body.extend_from_slice(&ch.to_le_bytes());
    if let Some(n) = name {
        body.extend_from_slice(n.as_bytes());
        body.push(0);
    }
    if let Some(c) = clip {
        body.extend_from_slice(&c.to_le_bytes());
    }
    let mut out = Vec::new();
    tag_header(TAG_PLACE_OBJECT2, body.len(), &mut out);
    out.extend(body);
    out
}

pub(super) fn tag(code: u16, body: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    tag_header(code, body.len(), &mut out);
    out.extend_from_slice(body);
    out
}

/// A value a registering frame action stores on its sprite.
pub(super) enum Member<'a> {
    Int(i32),
    Str(&'a str),
}

/// The frame action registering a grid sprite (`this`) with the host (see
/// [`register_action_in`]): `_global.thedas_s9`, its id as `thedas_s9id`.
fn register_action(id: u16) -> Vec<u8> {
    register_action_in("thedas_s9", "thedas_s9id", Member::Int(id as i32))
}

/// A frame action registering its sprite (`this`) with the host:
/// `if (_global[list] == undefined) _global[list] = new Array();
/// this[member] = value; _global[list].push(this);`
pub(super) fn register_action_in(list: &str, member: &str, value: Member) -> Vec<u8> {
    fn push_str(out: &mut Vec<u8>, ss: &[&str]) {
        let mut b = Vec::new();
        for s in ss {
            b.push(0);
            b.extend_from_slice(s.as_bytes());
            b.push(0);
        }
        out.push(0x96);
        out.extend_from_slice(&(b.len() as u16).to_le_bytes());
        out.extend(b);
    }
    fn push_int(out: &mut Vec<u8>, v: i32) {
        out.push(0x96);
        out.extend_from_slice(&5u16.to_le_bytes());
        out.push(7);
        out.extend_from_slice(&v.to_le_bytes());
    }
    const GET_VARIABLE: u8 = 0x1c;
    const GET_MEMBER: u8 = 0x4e;
    const SET_MEMBER: u8 = 0x4f;
    let mut init = Vec::new();
    push_str(&mut init, &["_global"]);
    init.push(GET_VARIABLE);
    push_str(&mut init, &[list]);
    push_int(&mut init, 0);
    push_str(&mut init, &["Array"]);
    init.extend_from_slice(&[0x40, SET_MEMBER]);
    let mut a = Vec::new();
    push_str(&mut a, &["_global"]);
    a.push(GET_VARIABLE);
    push_str(&mut a, &[list]);
    a.push(GET_MEMBER);
    // undefined
    a.extend_from_slice(&[0x96, 1, 0, 3]);
    a.extend_from_slice(&[0x49, 0x12]);
    // If (not equal) skip the creation.
    a.push(0x9d);
    a.extend_from_slice(&2u16.to_le_bytes());
    a.extend_from_slice(&(init.len() as i16).to_le_bytes());
    a.extend(init);
    push_str(&mut a, &["this"]);
    a.push(GET_VARIABLE);
    push_str(&mut a, &[member]);
    match value {
        Member::Int(v) => push_int(&mut a, v),
        Member::Str(v) => push_str(&mut a, &[v]),
    }
    a.push(SET_MEMBER);
    push_str(&mut a, &["this"]);
    a.push(GET_VARIABLE);
    push_int(&mut a, 1);
    push_str(&mut a, &["_global"]);
    a.push(GET_VARIABLE);
    push_str(&mut a, &[list]);
    a.push(GET_MEMBER);
    push_str(&mut a, &["push"]);
    a.extend_from_slice(&[0x52, 0x17, 0]);
    tag(TAG_DO_ACTION, &a)
}

/// The grids and shapes of a movie's top-level tags, and the character ids still free.
pub(super) struct Plan {
    grids: BTreeMap<u16, Rect>,
    shapes: BTreeMap<u16, Rect>,
    /// The shapes filled with one bitmap and nothing else (no line styles): their fill
    /// style's bytes (type, bitmap id, matrix), which a cell of the shape can reuse.
    bitmap_fills: BTreeMap<u16, Vec<u8>>,
    next_id: u32,
}

/// Collects a movie's scaling grids and shape bounds.
pub(super) fn plan(tags: &[Tag]) -> Plan {
    let mut p = Plan {
        grids: BTreeMap::new(),
        shapes: BTreeMap::new(),
        bitmap_fills: BTreeMap::new(),
        next_id: 0,
    };
    for t in tags {
        let id = t.body.get(..2).map(|b| u16::from_le_bytes([b[0], b[1]]));
        match (t.code, id) {
            (TAG_DEFINE_SCALING_GRID, Some(id)) => {
                if let Some((r, _)) = read_rect(&t.body[2..]) {
                    p.grids.insert(id, r);
                }
            }
            (
                TAG_DEFINE_SHAPE | TAG_DEFINE_SHAPE2 | TAG_DEFINE_SHAPE3 | TAG_DEFINE_SHAPE4,
                Some(id),
            ) => {
                if let Some((r, _)) = read_rect(&t.body[2..]) {
                    p.shapes.insert(id, r);
                }
                if let Some(fill) = single_bitmap_fill(t.code, t.body) {
                    p.bitmap_fills.insert(id, fill);
                }
            }
            _ => {}
        }
        // Any defining tag's id (the tags with an id first are the definitions; others'
        // first bytes may read as large ids, which only waste room).
        if let Some(id) = id
            && is_definition(t.code)
        {
            p.next_id = p.next_id.max(id as u32 + 1);
        }
    }
    p
}

fn is_definition(code: u16) -> bool {
    matches!(
        code,
        2 | 6
            | 7
            | 10
            | 11
            | 13
            | 14
            | 20
            | 21
            | 22
            | 32
            | 33
            | 34
            | 35
            | 36
            | 37
            | 39
            | 46
            | 48
            | 60
            | 62
            | 75
            | 83
            | 84
            | 90
            | 1001
            | 1009
    )
}

impl Plan {
    fn alloc(&mut self) -> Option<u16> {
        let id = u16::try_from(self.next_id).ok().filter(|&i| i < u16::MAX)?;
        self.next_id += 1;
        Some(id)
    }

    /// A grid sprite (`DefineSprite` body) as nine masked pieces: the piece definitions and
    /// the sprite, ready to write, with its bounds and grid. `None` leaves the sprite as it
    /// is (no grid, or content other than one unrotated shape).
    pub(super) fn rewrite_sprite(&mut self, body: &[u8]) -> Option<(Vec<u8>, Scale9)> {
        let id = u16::from_le_bytes([*body.first()?, *body.get(1)?]);
        let grid = *self.grids.get(&id)?;
        let inner = read_tags(body.get(4..)?).ok()?;
        let mut placed = None;
        let mut frames = 0;
        for t in &inner {
            match t.code {
                TAG_PLACE_OBJECT | TAG_PLACE_OBJECT2 | TAG_PLACE_OBJECT3 if placed.is_none() => {
                    placed = Some(*t)
                }
                TAG_SHOW_FRAME => frames += 1,
                TAG_END => {}
                _ => return None,
            }
        }
        let place_tag = placed?;
        if frames != 1 {
            return None;
        }
        let (ch, matrix) = placement(place_tag)?;
        let [x0, y0, x1, y1] = *self.shapes.get(&ch)?;
        let [sx, sy, tx, ty] = matrix?;
        let fx = |x: i32| x as f64 * sx + tx;
        let fy = |y: i32| y as f64 * sy + ty;
        let (bx0, bx1) = (fx(x0).min(fx(x1)), fx(x0).max(fx(x1)));
        let (by0, by1) = (fy(y0).min(fy(y1)), fy(y0).max(fy(y1)));
        let gx0 = (grid[0] as f64).clamp(bx0, bx1);
        let gx1 = (grid[2] as f64).clamp(gx0, bx1);
        let gy0 = (grid[1] as f64).clamp(by0, by1);
        let gy1 = (grid[3] as f64).clamp(gy0, by1);
        let xs = [bx0, gx0, gx1, bx1].map(|v| v.round() as i32);
        let ys = [by0, gy0, gy1, by1].map(|v| v.round() as i32);
        // The shape at depth 2, as placed, under a mask at depth 1.
        let mut content = place_tag.raw.to_vec();
        // The depth follows the character id (PlaceObject) or the flags (PlaceObject3: two
        // bytes of them; PlaceObject2: one).
        let depth_at = if place_tag.code == TAG_PLACE_OBJECT2 {
            1
        } else {
            2
        };
        let start = place_tag.raw.len() - place_tag.body.len() + depth_at;
        content
            .get_mut(start..start + 2)?
            .copy_from_slice(&2u16.to_le_bytes());
        // For a bitmap-filled shape placed with PlaceObject2: its placement moved to depth 1,
        // and where its character id sits there, to place each cell in its stead.
        let bitmap_fill = self.bitmap_fills.get(&ch).cloned();
        let cell_content = (place_tag.code == TAG_PLACE_OBJECT2
            && place_tag.body.first().is_some_and(|f| f & 0x02 != 0))
        .then(|| {
            let mut raw = place_tag.raw.to_vec();
            let head = place_tag.raw.len() - place_tag.body.len();
            raw.get_mut(head + 1..head + 3)?
                .copy_from_slice(&1u16.to_le_bytes());
            Some((head + 3, raw))
        })
        .flatten();
        let mut defs = Vec::new();
        let mut pieces = Vec::new();
        for r in 0..3 {
            for c in 0..3 {
                let cell = [xs[c], ys[r], xs[c + 1], ys[r + 1]];
                if cell[2] <= cell[0] || cell[3] <= cell[1] {
                    pieces.push(None);
                    continue;
                }
                let mut sprite = Vec::new();
                match (&bitmap_fill, cell_content.as_ref()) {
                    // A bitmap-filled shape: the piece is the cell itself, a rectangle with
                    // the same fill, so the piece's bounds are its cell's. Masked pieces
                    // measure as the whole shape, and a movie sizing the grid sprite by
                    // `_width`/`_height` then got the wrong scale once the pieces had
                    // stretched (tooltips.gfx's box came out half its height).
                    (Some(fill), Some((at, raw))) => {
                        let (shape, piece) = (self.alloc()?, self.alloc()?);
                        let to_shape = |v: i32, s: f64, t: f64| ((v as f64 - t) / s).round() as i32;
                        let mut r = [
                            to_shape(cell[0], sx, tx),
                            to_shape(cell[1], sy, ty),
                            to_shape(cell[2], sx, tx),
                            to_shape(cell[3], sy, ty),
                        ];
                        if r[0] > r[2] {
                            r.swap(0, 2);
                        }
                        if r[1] > r[3] {
                            r.swap(1, 3);
                        }
                        defs.extend(filled_rect_shape(shape, r, fill));
                        let mut placed = raw.clone();
                        placed
                            .get_mut(*at..*at + 2)?
                            .copy_from_slice(&shape.to_le_bytes());
                        sprite.extend(piece.to_le_bytes());
                        sprite.extend_from_slice(&1u16.to_le_bytes());
                        sprite.extend(placed);
                        pieces.push(Some(piece));
                    }
                    _ => {
                        let (mask, piece) = (self.alloc()?, self.alloc()?);
                        defs.extend(rect_shape(mask, cell));
                        sprite.extend(piece.to_le_bytes());
                        sprite.extend_from_slice(&1u16.to_le_bytes());
                        sprite.extend(place(1, mask, None, Some(2)));
                        sprite.extend(&content);
                        pieces.push(Some(piece));
                    }
                }
                sprite.extend(tag(TAG_SHOW_FRAME, &[]));
                sprite.extend(tag(TAG_END, &[]));
                defs.extend(tag(TAG_DEFINE_SPRITE, &sprite));
            }
        }
        let mut sprite = id.to_le_bytes().to_vec();
        sprite.extend_from_slice(&1u16.to_le_bytes());
        sprite.extend(register_action(id));
        for (k, p) in pieces.iter().enumerate() {
            if let Some(p) = p {
                sprite.extend(place(
                    k as u16 + 1,
                    *p,
                    Some(&format!("thedas_s9_{k}")),
                    None,
                ));
            }
        }
        sprite.extend(tag(TAG_SHOW_FRAME, &[]));
        sprite.extend(tag(TAG_END, &[]));
        defs.extend(tag(TAG_DEFINE_SPRITE, &sprite));
        let px = |v: f64| v / 20.0;
        Some((
            defs,
            Scale9 {
                bounds: [px(bx0), px(by0), px(bx1), px(by1)],
                grid: [px(gx0), px(gy0), px(gx1), px(gy1)],
            },
        ))
    }
}

/// A placement's character and matrix (identity when it has none; `None` inside when
/// rotated).
fn placement(t: Tag) -> Option<(u16, Option<[f64; 4]>)> {
    let identity = Some([1.0, 1.0, 0.0, 0.0]);
    match t.code {
        TAG_PLACE_OBJECT => {
            let b = t.body;
            let ch = u16::from_le_bytes([*b.first()?, *b.get(1)?]);
            let (m, _) = read_matrix(b.get(4..)?)?;
            Some((ch, m))
        }
        TAG_PLACE_OBJECT2 | TAG_PLACE_OBJECT3 => {
            let b = t.body;
            let flags = *b.first()?;
            let mut i = if t.code == TAG_PLACE_OBJECT3 { 4 } else { 3 };
            if t.code == TAG_PLACE_OBJECT3 {
                let f2 = *b.get(1)?;
                // A class name: not a plain shape.
                if f2 & 0x08 != 0 {
                    return None;
                }
            }
            if flags & 0x02 == 0 || flags & 0x01 != 0 {
                return None;
            }
            let ch = u16::from_le_bytes([*b.get(i)?, *b.get(i + 1)?]);
            i += 2;
            let m = if flags & 0x04 != 0 {
                read_matrix(b.get(i..)?)?.0
            } else {
                identity
            };
            Some((ch, m))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rectangles_and_shapes_read_back() {
        let mut b = BitOut::default();
        rect_bytes([-40, 0, 1740, 2060], &mut b);
        assert_eq!(read_rect(&b.out).unwrap().0, [-40, 0, 1740, 2060]);
        let shape = rect_shape(7, [0, 0, 200, 100]);
        let t = read_tags(&shape).unwrap();
        assert_eq!(t[0].code, TAG_DEFINE_SHAPE);
        assert_eq!(read_rect(&t[0].body[2..]).unwrap().0, [0, 0, 200, 100]);
    }

    #[test]
    fn the_corners_keep_their_size_and_the_middle_stretches() {
        let s = Scale9 {
            bounds: [0.0, 0.0, 100.0, 50.0],
            grid: [10.0, 5.0, 90.0, 45.0],
        };
        // Twice as wide, as high as authored.
        let l = layout(&s, 2.0, 1.0);
        // Left column: half scale (drawn at 1 after the sprite's 2), at the left edge.
        assert_eq!(l[0], [0.0, 0.0, 50.0, 100.0]);
        // Right column ends at the right edge: x 100 maps to 100 (200 after the sprite's 2).
        let [x, _, xs, _] = l[2];
        assert!((x + 100.0 * xs / 100.0 - 100.0).abs() < 1e-9);
        // The middle fills the rest: 10/2 .. 100-10/2 from 10 .. 90.
        let [x, _, xs, _] = l[1];
        let (a, b) = (xs / 100.0, x);
        assert!((a * 10.0 + b - 5.0).abs() < 1e-9 && (a * 90.0 + b - 95.0).abs() < 1e-9);
        // Smaller than its corners: no slicing.
        assert_eq!(layout(&s, 0.1, 1.0)[0], [0.0, 0.0, 100.0, 100.0]);
    }

    #[test]
    fn a_grid_sprite_with_one_shape_becomes_nine_masked_pieces() {
        let mut movie = Vec::new();
        movie.extend(rect_shape(1, [0, 0, 2000, 1000]));
        let mut sprite = 2u16.to_le_bytes().to_vec();
        sprite.extend_from_slice(&1u16.to_le_bytes());
        sprite.extend(place(1, 1, None, None));
        sprite.extend(tag(TAG_SHOW_FRAME, &[]));
        sprite.extend(tag(TAG_END, &[]));
        let sprite_tag = tag(TAG_DEFINE_SPRITE, &sprite);
        movie.extend(&sprite_tag);
        let mut grid = 2u16.to_le_bytes().to_vec();
        let mut b = BitOut::default();
        rect_bytes([200, 100, 1800, 900], &mut b);
        grid.extend(b.out);
        movie.extend(tag(TAG_DEFINE_SCALING_GRID, &grid));
        let tags = read_tags(&movie).unwrap();
        let mut p = plan(&tags);
        assert_eq!(p.next_id, 3);
        let (defs, s9) = p.rewrite_sprite(tags[1].body).unwrap();
        assert_eq!(s9.bounds, [0.0, 0.0, 100.0, 50.0]);
        assert_eq!(s9.grid, [10.0, 5.0, 90.0, 45.0]);
        let out = read_tags(&defs).unwrap();
        // Nine masks and nine pieces, then the sprite.
        assert_eq!(out.len(), 19);
        let last = out.last().unwrap();
        assert_eq!(
            (last.code, &last.body[..2]),
            (TAG_DEFINE_SPRITE, &[2u8, 0][..])
        );
        let inner = read_tags(&last.body[4..]).unwrap();
        assert_eq!(inner[0].code, TAG_DO_ACTION);
        assert_eq!(
            inner.iter().filter(|t| t.code == TAG_PLACE_OBJECT2).count(),
            9
        );
        // A piece: the mask clips the shape, now at depth 2.
        let piece = read_tags(&out[1].body[4..]).unwrap();
        assert_eq!(piece[0].body[0] & 0x40, 0x40);
        assert_eq!(&piece[1].body[1..3], &2u16.to_le_bytes());
        // A sprite without a grid is left alone.
        assert!(p.rewrite_sprite(&[9, 0, 1, 0]).is_none());
    }
}
