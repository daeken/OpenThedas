//! Bink 1 video, revision `i`.
//!
//! A frame is three planes (luma, then the red and blue chroma at half size), each a 32-bit
//! aligned run of bits read lowest first. A plane is 8x8 blocks, each one of ten kinds:
//! copied from the last frame (in place or moved), filled, two-colour patterns, runs in
//! one of sixteen fill orders, raw pixels, DCT-coded (intra, or inter on top of a moved
//! copy) or a moved copy plus a sparse residue. A scaled block is any of the still kinds
//! coded at 8x8 and doubled to 16x16.
//!
//! The values blocks use come from nine "bundles", streams decoded a row of blocks at a time
//! with Huffman codebooks the plane picks from sixteen.
//!
//! Provisional where it is not checked against the original decoder: decoded pictures were
//! compared with ffmpeg's for every movie both games ship.

use std::num::Wrapping;

use super::bits::Bits;
use super::tables::{Tables, tables};
use super::{BinkError, Header, Result};

/// One plane of a decoded picture.
#[derive(Clone, Debug, Default)]
pub struct Plane {
    pub width: usize,
    pub height: usize,
    /// Bytes from one row to the next (the plane is padded to whole 16x16 blocks).
    pub stride: usize,
    pub data: Vec<u8>,
}

impl Plane {
    fn new(width: usize, height: usize) -> Self {
        let stride = width.div_ceil(16) * 16;
        let rows = height.div_ceil(16) * 16;
        Self {
            width,
            height,
            stride,
            data: vec![0; stride * rows],
        }
    }

    /// Row `y`, cropped to the picture.
    pub fn row(&self, y: usize) -> &[u8] {
        &self.data[y * self.stride..y * self.stride + self.width]
    }
}

/// A decoded picture: luma, then the blue and red chroma at half size (rounded up).
#[derive(Clone, Debug, Default)]
pub struct Frame {
    pub planes: [Plane; 3],
}

impl Frame {
    fn new(width: usize, height: usize) -> Self {
        let (cw, ch) = (width.div_ceil(2), height.div_ceil(2));
        Self {
            planes: [
                Plane::new(width, height),
                Plane::new(cw, ch),
                Plane::new(cw, ch),
            ],
        }
    }

    pub fn width(&self) -> usize {
        self.planes[0].width
    }

    pub fn height(&self) -> usize {
        self.planes[0].height
    }

    /// The picture as planar 4:2:0 (luma rows, then blue chroma rows, then red), cropped.
    pub fn write_yuv420(&self, out: &mut Vec<u8>) {
        for p in &self.planes {
            for y in 0..p.height {
                out.extend_from_slice(p.row(y));
            }
        }
    }

    /// The picture as RGBA (opaque), BT.601 limited range, each chroma sample covering 2x2
    /// pixels. Provisional: the original player's colour conversion is not traced; this is
    /// the conversion ffmpeg applied when it decoded the movies.
    pub fn write_rgba(&self, out: &mut Vec<u8>) {
        let [yp, up, vp] = &self.planes;
        out.clear();
        out.reserve(yp.width * yp.height * 4);
        for y in 0..yp.height {
            let (ly, cu, cv) = (yp.row(y), up.row(y / 2), vp.row(y / 2));
            for (x, &l) in ly.iter().enumerate() {
                let c = 1192 * (l as i32 - 16);
                let u = cu.get(x / 2).copied().unwrap_or(128) as i32 - 128;
                let v = cv.get(x / 2).copied().unwrap_or(128) as i32 - 128;
                let px = |n: i32| ((n + 512) >> 10).clamp(0, 255) as u8;
                out.extend_from_slice(&[
                    px(c + 1634 * v),
                    px(c - 401 * u - 832 * v),
                    px(c + 2066 * u),
                    255,
                ]);
            }
        }
    }
}

/// The block kinds.
const SKIP: i32 = 0;
const SCALED: i32 = 1;
const MOTION: i32 = 2;
const RUN: i32 = 3;
const RESIDUE: i32 = 4;
const INTRA: i32 = 5;
const FILL: i32 = 6;
const INTER: i32 = 7;
const PATTERN: i32 = 8;
const RAW: i32 = 9;

/// The bundles, in the order a plane reads their codebooks and a row refills them.
const BLOCK_TYPES: usize = 0;
const SUB_BLOCK_TYPES: usize = 1;
const COLORS: usize = 2;
const PATTERNS: usize = 3;
const X_OFF: usize = 4;
const Y_OFF: usize = 5;
const INTRA_DC: usize = 6;
const INTER_DC: usize = 7;
const RUNS: usize = 8;

/// A codebook choice and the order its symbols come in.
#[derive(Clone, Copy, Debug)]
struct Tree {
    book: u8,
    syms: [u8; 16],
}

impl Default for Tree {
    fn default() -> Self {
        Self {
            book: 0,
            syms: std::array::from_fn(|i| i as u8),
        }
    }
}

impl Tree {
    /// Reads which codebook and in what order its symbols come: either some symbols named
    /// first and the rest in order, or the identity order shuffled by up to four rounds of
    /// merging neighbouring runs of 1, 2, 4 and 8.
    fn read(bits: &mut Bits) -> Self {
        let book = bits.read(4) as u8;
        let mut t = Self {
            book,
            ..Self::default()
        };
        if book == 0 {
            return t;
        }
        if bits.bit() {
            let named = bits.read(3) as usize + 1;
            let mut used = [false; 16];
            let mut n = 0;
            for _ in 0..named {
                let s = bits.read(4) as u8;
                t.syms[n] = s;
                n += 1;
                used[s as usize] = true;
            }
            for s in 0..16u8 {
                if n < 16 && !used[s as usize] {
                    t.syms[n] = s;
                    n += 1;
                }
            }
        } else {
            let rounds = bits.read(2) + 1;
            let mut cur = t.syms;
            for r in 0..rounds {
                let size = 1usize << r;
                let mut out = [0u8; 16];
                for start in (0..16).step_by(size * 2) {
                    let (a, b) = cur[start..start + size * 2].split_at(size);
                    let (mut i, mut j) = (0, 0);
                    let dst = &mut out[start..start + size * 2];
                    for d in dst.iter_mut() {
                        let take_b = if i < size && j < size {
                            bits.bit()
                        } else {
                            i >= size
                        };
                        if take_b {
                            *d = b[j];
                            j += 1;
                        } else {
                            *d = a[i];
                            i += 1;
                        }
                    }
                }
                cur = out;
            }
            t.syms = cur;
        }
        t
    }
}

/// A stream of values decoded ahead of the blocks that use them.
#[derive(Default)]
struct Bundle {
    tree: Tree,
    /// Bits of each chunk's length.
    len_bits: u32,
    vals: Vec<i32>,
    /// The next value to hand out.
    pos: usize,
    /// A zero-length chunk ends the bundle for the rest of the plane.
    ended: bool,
}

impl Bundle {
    fn start(&mut self, len_bits: u32) {
        self.len_bits = len_bits;
        self.vals.clear();
        self.pos = 0;
        self.ended = false;
    }

    /// The length of the next chunk to decode, if the bundle wants one now (every value
    /// decoded so far has been used).
    fn chunk(&mut self, bits: &mut Bits) -> Option<usize> {
        if self.ended || self.pos < self.vals.len() {
            return None;
        }
        let n = bits.read(self.len_bits) as usize;
        if n == 0 {
            self.ended = true;
            return None;
        }
        Some(n)
    }

    #[inline]
    fn sym(&self, books: &Tables, bits: &mut Bits) -> u8 {
        let book = &books.books[self.tree.book as usize & 15];
        self.tree.syms[book.read(bits) as usize & 15]
    }

    #[inline]
    fn next(&mut self) -> Result<i32> {
        let v = *self
            .vals
            .get(self.pos)
            .ok_or(BinkError::BadFrame("a bundle ran dry"))?;
        self.pos += 1;
        Ok(v)
    }
}

/// `floor(log2(n)) + 1`: the bits a count up to `n` needs.
fn len_bits(n: usize) -> u32 {
    usize::BITS - n.leading_zeros()
}

/// Decodes a movie's pictures in order.
pub struct VideoDecoder {
    tables: &'static Tables,
    /// The picture being decoded and the last one (which moved blocks copy from).
    frames: [Frame; 2],
    cur: usize,
    bundles: [Bundle; 9],
    /// The colour bundle's high nibbles each have a codebook per last high nibble.
    col_high: [Tree; 16],
    col_last: u8,
}

impl VideoDecoder {
    pub fn new(header: &Header) -> Result<Self> {
        if header.revision != b'i' {
            return Err(BinkError::Unsupported("only Bink revision i"));
        }
        if header.flags & 0x0010_0000 != 0 {
            return Err(BinkError::Unsupported("alpha planes"));
        }
        if header.flags & 0xf000_0000 != 0 {
            return Err(BinkError::Unsupported("scaled or interlaced video"));
        }
        let (w, h) = (header.width as usize, header.height as usize);
        Ok(Self {
            tables: tables(),
            frames: [Frame::new(w, h), Frame::new(w, h)],
            cur: 0,
            bundles: Default::default(),
            col_high: [Tree::default(); 16],
            col_last: 0,
        })
    }

    /// Decodes the next picture from its packet. On an error the picture may be partly
    /// decoded; the next frame still decodes against it.
    pub fn decode(&mut self, packet: &[u8]) -> Result<&Frame> {
        let mut bits = Bits::new(packet);
        // Revision i leads with the size of the luma plane's data.
        bits.skip(32);
        let (a, b) = self.frames.split_at_mut(1);
        let (cur, prev) = if self.cur == 0 {
            (&mut a[0], &b[0])
        } else {
            (&mut b[0], &a[0])
        };
        let mut result = Ok(());
        // The chroma planes come red first.
        for p in [0, 2, 1] {
            let plane = &mut cur.planes[p];
            let r = decode_plane(
                self.tables,
                &mut self.bundles,
                &mut self.col_high,
                &mut self.col_last,
                &mut bits,
                plane,
                &prev.planes[p],
            );
            if r.is_err() {
                result = r;
                break;
            }
            if bits.left() <= 0 {
                break;
            }
        }
        if result.is_ok() && bits.overrun() {
            result = Err(BinkError::BadFrame("packet too short"));
        }
        let done = self.cur;
        self.cur ^= 1;
        result.map(|()| &self.frames[done])
    }

    /// The last picture decoded.
    pub fn last(&self) -> &Frame {
        &self.frames[self.cur ^ 1]
    }
}

/// Fills one bundle's next chunk, if it wants one.
fn refill(
    which: usize,
    b: &mut Bundle,
    t: &Tables,
    col_high: &[Tree; 16],
    col_last: &mut u8,
    bits: &mut Bits,
) -> Result<()> {
    let Some(n) = b.chunk(bits) else {
        return Ok(());
    };
    match which {
        BLOCK_TYPES | SUB_BLOCK_TYPES => {
            if bits.bit() {
                let v = bits.read(4) as i32;
                b.vals.extend(std::iter::repeat_n(v, n));
            } else {
                const RLE: [usize; 4] = [4, 8, 12, 32];
                let end = b.vals.len() + n;
                let mut last = 0;
                while b.vals.len() < end {
                    let v = b.sym(t, bits) as i32;
                    if v < 12 {
                        last = v;
                        b.vals.push(v);
                    } else {
                        let run = RLE[(v - 12) as usize];
                        if b.vals.len() + run > end {
                            return Err(BinkError::BadFrame("block type run too long"));
                        }
                        b.vals.extend(std::iter::repeat_n(last, run));
                    }
                }
            }
        }
        COLORS => {
            let mut one = |b: &Bundle, bits: &mut Bits| {
                let hi = &col_high[*col_last as usize & 15];
                let book = &t.books[hi.book as usize & 15];
                *col_last = hi.syms[book.read(bits) as usize & 15];
                let lo = b.sym(t, bits);
                (*col_last as i32) << 4 | lo as i32
            };
            if bits.bit() {
                let v = one(b, bits);
                b.vals.extend(std::iter::repeat_n(v, n));
            } else {
                for _ in 0..n {
                    let v = one(b, bits);
                    b.vals.push(v);
                }
            }
        }
        PATTERNS => {
            for _ in 0..n {
                let lo = b.sym(t, bits) as i32;
                let hi = b.sym(t, bits) as i32;
                b.vals.push(lo | hi << 4);
            }
        }
        X_OFF | Y_OFF => {
            let signed = |v: i32, bits: &mut Bits| if v != 0 && bits.bit() { -v } else { v };
            if bits.bit() {
                let v = bits.read(4) as i32;
                let v = signed(v, bits);
                b.vals.extend(std::iter::repeat_n(v, n));
            } else {
                for _ in 0..n {
                    let v = b.sym(t, bits) as i32;
                    let v = signed(v, bits);
                    b.vals.push(v);
                }
            }
        }
        INTRA_DC | INTER_DC => {
            let has_sign = which == INTER_DC;
            let mut v = bits.read(11 - has_sign as u32) as i32;
            if has_sign && v != 0 && bits.bit() {
                v = -v;
            }
            b.vals.push(v);
            let mut left = n - 1;
            while left > 0 {
                let group = left.min(8);
                let size = bits.read(4);
                for _ in 0..group {
                    if size != 0 {
                        let mut d = bits.read(size) as i32;
                        if d != 0 && bits.bit() {
                            d = -d;
                        }
                        v += d;
                        if !(-32768..=32767).contains(&v) {
                            return Err(BinkError::BadFrame("DC out of range"));
                        }
                    }
                    b.vals.push(v);
                }
                left -= group;
            }
        }
        _ => {
            // Runs.
            if bits.bit() {
                let v = bits.read(4) as i32;
                b.vals.extend(std::iter::repeat_n(v, n));
            } else {
                for _ in 0..n {
                    let v = b.sym(t, bits) as i32;
                    b.vals.push(v);
                }
            }
        }
    }
    if bits.overrun() {
        return Err(BinkError::BadFrame("bundle past the end of the packet"));
    }
    Ok(())
}

/// A block of coefficients or pixels being built.
type Block = [i32; 64];

#[allow(clippy::too_many_arguments)]
fn decode_plane(
    t: &Tables,
    bundles: &mut [Bundle; 9],
    col_high: &mut [Tree; 16],
    col_last: &mut u8,
    bits: &mut Bits,
    dst: &mut Plane,
    prev: &Plane,
) -> Result<()> {
    let width = dst.width;
    let bw = width.div_ceil(8);
    let bh = dst.height.div_ceil(8);
    let w = width.max(8);
    let lens = [
        len_bits((w >> 3) + 511),
        len_bits((w >> 4) + 511),
        len_bits(bw * 64 + 511),
        len_bits((bw << 3) + 511),
        len_bits((w >> 3) + 511),
        len_bits((w >> 3) + 511),
        len_bits((w >> 3) + 511),
        len_bits((w >> 3) + 511),
        len_bits(bw * 48 + 511),
    ];
    for (i, b) in bundles.iter_mut().enumerate() {
        if i == COLORS {
            for h in col_high.iter_mut() {
                *h = Tree::read(bits);
            }
            *col_last = 0;
        }
        if i != INTRA_DC && i != INTER_DC {
            b.tree = Tree::read(bits);
        }
        b.start(lens[i]);
    }
    let stride = dst.stride;
    let mut coefs: Block = [0; 64];
    let mut small: Block = [0; 64];
    for by in 0..bh {
        for (i, b) in bundles.iter_mut().enumerate() {
            refill(i, b, t, col_high, col_last, bits)?;
        }
        let mut bx = 0;
        while bx < bw {
            let at = by * 8 * stride + bx * 8;
            let kind = bundles[BLOCK_TYPES].next()?;
            if by & 1 == 1 && kind == SCALED {
                // The lower half of a scaled block from the row above.
                bx += 2;
                continue;
            }
            match kind {
                SKIP => copy8(&mut dst.data, at, &prev.data, at as isize, stride)?,
                SCALED => {
                    let sub = bundles[SUB_BLOCK_TYPES].next()?;
                    if sub == FILL {
                        let v = bundles[COLORS].next()? as u8;
                        for y in 0..16 {
                            let row = dst
                                .data
                                .get_mut(at + y * stride..at + y * stride + 16)
                                .ok_or(BinkError::BadFrame("scaled block off the plane"))?;
                            row.fill(v);
                        }
                    } else {
                        match sub {
                            RUN => run_block(t, bundles, bits, &mut small)?,
                            INTRA => {
                                coefs.fill(0);
                                coefs[0] = bundles[INTRA_DC].next()?;
                                let q = read_dct(t, bits, &mut coefs)?;
                                dequant(&mut coefs, &t.intra[q]);
                                idct(&mut coefs);
                                for (s, c) in small.iter_mut().zip(coefs) {
                                    *s = c.clamp(0, 255);
                                }
                            }
                            PATTERN => pattern_block(bundles, &mut small)?,
                            RAW => {
                                for s in small.iter_mut() {
                                    *s = bundles[COLORS].next()?;
                                }
                            }
                            _ => return Err(BinkError::BadFrame("bad scaled block kind")),
                        }
                        for y in 0..16 {
                            let row = dst
                                .data
                                .get_mut(at + y * stride..at + y * stride + 16)
                                .ok_or(BinkError::BadFrame("scaled block off the plane"))?;
                            for (x, p) in row.iter_mut().enumerate() {
                                *p = small[(y / 2) * 8 + x / 2] as u8;
                            }
                        }
                    }
                    bx += 2;
                    continue;
                }
                MOTION => {
                    let from = moved(bundles, at, stride)?;
                    copy8(&mut dst.data, at, &prev.data, from, stride)?;
                }
                RUN => {
                    run_block(t, bundles, bits, &mut small)?;
                    put8(&mut dst.data, at, stride, &small)?;
                }
                RESIDUE => {
                    let from = moved(bundles, at, stride)?;
                    copy8(&mut dst.data, at, &prev.data, from, stride)?;
                    coefs.fill(0);
                    let masks = bits.read(7) as i32;
                    read_residue(t, bits, &mut coefs, masks);
                    for y in 0..8 {
                        let row = &mut dst.data[at + y * stride..at + y * stride + 8];
                        for (x, p) in row.iter_mut().enumerate() {
                            *p = (*p as i32 + coefs[y * 8 + x]) as u8;
                        }
                    }
                }
                INTRA => {
                    coefs.fill(0);
                    coefs[0] = bundles[INTRA_DC].next()?;
                    let q = read_dct(t, bits, &mut coefs)?;
                    dequant(&mut coefs, &t.intra[q]);
                    idct(&mut coefs);
                    for c in coefs.iter_mut() {
                        *c = (*c).clamp(0, 255);
                    }
                    put8(&mut dst.data, at, stride, &coefs)?;
                }
                FILL => {
                    let v = bundles[COLORS].next()?;
                    put8(&mut dst.data, at, stride, &[v; 64])?;
                }
                INTER => {
                    let from = moved(bundles, at, stride)?;
                    copy8(&mut dst.data, at, &prev.data, from, stride)?;
                    coefs.fill(0);
                    coefs[0] = bundles[INTER_DC].next()?;
                    let q = read_dct(t, bits, &mut coefs)?;
                    dequant(&mut coefs, &t.inter[q]);
                    idct(&mut coefs);
                    for y in 0..8 {
                        let row = &mut dst.data[at + y * stride..at + y * stride + 8];
                        for (x, p) in row.iter_mut().enumerate() {
                            // Wrapping, as the encoder expects (a bright block plus a
                            // large residue comes out dark).
                            *p = (*p as i32 + coefs[y * 8 + x]) as u8;
                        }
                    }
                }
                PATTERN => {
                    pattern_block(bundles, &mut small)?;
                    put8(&mut dst.data, at, stride, &small)?;
                }
                RAW => {
                    for s in small.iter_mut() {
                        *s = bundles[COLORS].next()?;
                    }
                    put8(&mut dst.data, at, stride, &small)?;
                }
                _ => return Err(BinkError::BadFrame("bad block kind")),
            }
            bx += 1;
        }
        if bits.overrun() {
            return Err(BinkError::BadFrame("plane past the end of the packet"));
        }
    }
    bits.align32();
    Ok(())
}

/// Where a moved block comes from in the last picture: the motion bundles' offsets from
/// `at`, as an index (the copy checks it is inside the plane).
fn moved(bundles: &mut [Bundle; 9], at: usize, stride: usize) -> Result<isize> {
    let dx = bundles[X_OFF].next()? as isize;
    let dy = bundles[Y_OFF].next()? as isize;
    Ok(at as isize + dx + dy * stride as isize)
}

/// Copies the 8x8 block at `from` in `src` to `at` in `dst`.
fn copy8(dst: &mut [u8], at: usize, src: &[u8], from: isize, stride: usize) -> Result<()> {
    let bad = || BinkError::BadFrame("block copied from off the plane");
    let from = usize::try_from(from).map_err(|_| bad())?;
    if from + 7 * stride + 8 > src.len() || at + 7 * stride + 8 > dst.len() {
        return Err(bad());
    }
    for y in 0..8 {
        dst[at + y * stride..at + y * stride + 8]
            .copy_from_slice(&src[from + y * stride..from + y * stride + 8]);
    }
    Ok(())
}

/// Writes an 8x8 block of pixel values.
fn put8(dst: &mut [u8], at: usize, stride: usize, b: &Block) -> Result<()> {
    if at + 7 * stride + 8 > dst.len() {
        return Err(BinkError::BadFrame("block off the plane"));
    }
    for y in 0..8 {
        for (p, &v) in dst[at + y * stride..at + y * stride + 8]
            .iter_mut()
            .zip(&b[y * 8..y * 8 + 8])
        {
            *p = v as u8;
        }
    }
    Ok(())
}

/// A run block: runs of one colour or of separate colours, in one of sixteen orders.
fn run_block(
    t: &Tables,
    bundles: &mut [Bundle; 9],
    bits: &mut Bits,
    out: &mut Block,
) -> Result<()> {
    let order = &t.patterns[bits.read(4) as usize & 15];
    let mut i = 0usize;
    while i < 63 {
        let run = bundles[RUNS].next()? as usize + 1;
        if i + run > 64 {
            return Err(BinkError::BadFrame("run past the end of the block"));
        }
        if bits.bit() {
            let v = bundles[COLORS].next()?;
            for &p in &order[i..i + run] {
                out[p as usize & 63] = v;
            }
        } else {
            for &p in &order[i..i + run] {
                out[p as usize & 63] = bundles[COLORS].next()?;
            }
        }
        i += run;
    }
    if i == 63 {
        out[order[63] as usize & 63] = bundles[COLORS].next()?;
    }
    Ok(())
}

/// A pattern block: two colours, and a row of eight bits per pixel row choosing between them.
fn pattern_block(bundles: &mut [Bundle; 9], out: &mut Block) -> Result<()> {
    let c = [bundles[COLORS].next()?, bundles[COLORS].next()?];
    for y in 0..8 {
        let bits = bundles[PATTERNS].next()?;
        for x in 0..8 {
            out[y * 8 + x] = c[(bits >> x) as usize & 1];
        }
    }
    Ok(())
}

/// The coefficient groups both codings start from: coded positions and how each is split.
/// Mode 0: a group of 16 not yet split; 1: its remaining 12, split into three groups of
/// four; 2: a group of four; 3: a single coefficient.
#[derive(Clone, Copy)]
struct Entry {
    pos: u8,
    mode: u8,
}

/// The work list of groups still to visit: new singles go in front of the pass under way,
/// new groups at the back.
struct WorkList {
    e: [Entry; 128],
    start: usize,
    end: usize,
}

impl WorkList {
    fn new(init: &[(u8, u8)]) -> Self {
        let mut w = Self {
            e: [Entry { pos: 0, mode: 0 }; 128],
            start: 64,
            end: 64,
        };
        for &(pos, mode) in init {
            w.e[w.end] = Entry { pos, mode };
            w.end += 1;
        }
        w
    }

    fn push_front(&mut self, pos: u8) {
        if self.start > 0 {
            self.start -= 1;
            self.e[self.start] = Entry { pos, mode: 3 };
        }
    }

    fn push_back(&mut self, pos: u8, mode: u8) {
        if self.end < self.e.len() {
            self.e[self.end] = Entry { pos, mode };
            self.end += 1;
        }
    }
}

/// Reads a coefficient magnitude of `bits` bits whose top bit is implied, then its sign.
fn coef(bits: &mut Bits, n: u32) -> i32 {
    if n == 0 {
        if bits.bit() { -1 } else { 1 }
    } else {
        let v = (bits.read(n) | 1 << n) as i32;
        if bits.bit() { -v } else { v }
    }
}

/// Reads a DCT block's AC coefficients (the DC is already in `b[0]`) in bit planes, top
/// first, and its quantiser.
fn read_dct(t: &Tables, bits: &mut Bits, b: &mut Block) -> Result<usize> {
    let mut list = WorkList::new(&[(4, 0), (24, 0), (44, 0), (1, 3), (2, 3), (3, 3)]);
    let planes = bits.read(4) as i32;
    let put = |b: &mut Block, pos: u8, v: i32| b[t.scan[pos as usize & 63] as usize & 63] = v;
    for n in (0..planes).rev() {
        let n = n as u32;
        let mut i = list.start;
        while i < list.end {
            let Entry { pos, mode } = list.e[i];
            if (pos == 0 && mode == 0) || !bits.bit() {
                i += 1;
                continue;
            }
            match mode {
                0 | 2 => {
                    if mode == 0 {
                        list.e[i] = Entry {
                            pos: pos + 4,
                            mode: 1,
                        };
                    } else {
                        list.e[i] = Entry { pos: 0, mode: 0 };
                        i += 1;
                    }
                    for p in pos..pos + 4 {
                        if bits.bit() {
                            list.push_front(p);
                        } else {
                            let v = coef(bits, n);
                            put(b, p, v);
                        }
                    }
                }
                1 => {
                    list.e[i].mode = 2;
                    for k in 1..=3 {
                        list.push_back(pos + 4 * k, 2);
                    }
                }
                _ => {
                    let v = coef(bits, n);
                    put(b, pos, v);
                    list.e[i] = Entry { pos: 0, mode: 0 };
                    i += 1;
                }
            }
            if bits.overrun() {
                return Err(BinkError::BadFrame("coefficients past the end"));
            }
        }
    }
    Ok(bits.read(4) as usize)
}

/// Reads a residue: coefficients added in bit planes of `mask`, refining the signs already
/// known first, until `masks` values have been coded.
fn read_residue(t: &Tables, bits: &mut Bits, b: &mut Block, mut masks: i32) {
    let mut list = WorkList::new(&[(4, 0), (24, 0), (44, 0), (0, 2)]);
    let mut nonzero = [0u8; 64];
    let mut nnz = 0usize;
    let mut mask = 1i32 << bits.read(3);
    while mask != 0 {
        for &p in &nonzero[..nnz] {
            if !bits.bit() {
                continue;
            }
            let c = &mut b[p as usize & 63];
            if *c < 0 {
                *c -= mask;
            } else {
                *c += mask;
            }
            masks -= 1;
            if masks < 0 {
                return;
            }
        }
        let mut i = list.start;
        while i < list.end {
            let Entry { pos, mode } = list.e[i];
            if (pos == 0 && mode == 0) || !bits.bit() {
                i += 1;
                continue;
            }
            let mut set = |p: u8, bits: &mut Bits, masks: &mut i32| {
                let at = t.scan[p as usize & 63];
                if nnz < 64 {
                    nonzero[nnz] = at;
                    nnz += 1;
                }
                b[at as usize & 63] = if bits.bit() { -mask } else { mask };
                *masks -= 1;
                *masks >= 0
            };
            match mode {
                0 | 2 => {
                    if mode == 0 {
                        list.e[i] = Entry {
                            pos: pos + 4,
                            mode: 1,
                        };
                    } else {
                        list.e[i] = Entry { pos: 0, mode: 0 };
                        i += 1;
                    }
                    for p in pos..pos + 4 {
                        if bits.bit() {
                            list.push_front(p);
                        } else if !set(p, bits, &mut masks) {
                            return;
                        }
                    }
                }
                1 => {
                    list.e[i].mode = 2;
                    for k in 1..=3 {
                        list.push_back(pos + 4 * k, 2);
                    }
                }
                _ => {
                    let ok = set(pos, bits, &mut masks);
                    list.e[i] = Entry { pos: 0, mode: 0 };
                    i += 1;
                    if !ok {
                        return;
                    }
                }
            }
            if bits.overrun() {
                return;
            }
        }
        mask >>= 1;
    }
}

/// Scales coefficients by a dequantisation table (16.16, then down to the IDCT's scale).
fn dequant(b: &mut Block, q: &[u32; 64]) {
    for (c, &m) in b.iter_mut().zip(q) {
        if *c != 0 {
            *c = ((*c as u32).wrapping_mul(m) as i32) >> 11;
        }
    }
}

/// Bink's fixed-point IDCT (the AAN factorisation; constants in 1/2048ths).
fn idct(b: &mut Block) {
    const A1: Wrapping<i32> = Wrapping(2896);
    const A2: Wrapping<i32> = Wrapping(2217);
    const A3: Wrapping<i32> = Wrapping(3784);
    const A4: Wrapping<i32> = Wrapping(-5352);
    fn pass(s: [Wrapping<i32>; 8]) -> [Wrapping<i32>; 8] {
        let a0 = s[0] + s[4];
        let a1 = s[0] - s[4];
        let a2 = s[2] + s[6];
        let a3 = (A1 * (s[2] - s[6])) >> 11;
        let a4 = s[5] + s[3];
        let a5 = s[5] - s[3];
        let a6 = s[1] + s[7];
        let a7 = s[1] - s[7];
        let b0 = a4 + a6;
        let b1 = (A3 * (a5 + a7)) >> 11;
        let b2 = ((A4 * a5) >> 11) - b0 + b1;
        let b3 = ((A1 * (a6 - a4)) >> 11) - b2;
        let b4 = ((A2 * a7) >> 11) + b3 - b1;
        [
            a0 + a2 + b0,
            a1 + a3 - a2 + b2,
            a1 - a3 + a2 + b3,
            a0 - a2 - b4,
            a0 - a2 + b4,
            a1 - a3 + a2 - b3,
            a1 + a3 - a2 - b2,
            a0 + a2 - b0,
        ]
    }
    let mut tmp = [Wrapping(0i32); 64];
    for x in 0..8 {
        let col = pass(std::array::from_fn(|y| Wrapping(b[y * 8 + x])));
        for y in 0..8 {
            tmp[y * 8 + x] = col[y];
        }
    }
    for y in 0..8 {
        let row = pass(std::array::from_fn(|x| tmp[y * 8 + x]));
        for x in 0..8 {
            b[y * 8 + x] = ((row[x] + Wrapping(0x7f)) >> 8).0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes bits lowest first.
    #[derive(Default)]
    struct Writer {
        bytes: Vec<u8>,
        n: usize,
    }

    impl Writer {
        fn put(&mut self, v: u32, bits: u32) {
            for i in 0..bits {
                if self.n % 8 == 0 {
                    self.bytes.push(0);
                }
                if v >> i & 1 != 0 {
                    *self.bytes.last_mut().unwrap() |= 1 << (self.n % 8);
                }
                self.n += 1;
            }
        }

        fn align32(&mut self) {
            while self.n % 32 != 0 {
                self.put(0, 1);
            }
        }
    }

    fn header(w: u32, h: u32) -> Header {
        Header {
            revision: b'i',
            frames: 1,
            largest_frame: 0,
            width: w,
            height: h,
            fps_num: 30,
            fps_den: 1,
            flags: 0,
            tracks: Vec::new(),
        }
    }

    /// One plane whose every block is filled with `color`, with raw codebooks: the
    /// block-type bundle holds `blocks` fills (one chunk, its value given once), the colour
    /// bundle `blocks` copies of `color`, every other bundle empty.
    fn fill_plane(wr: &mut Writer, width: usize, blocks: usize, color: u8) {
        let bw = width.div_ceil(8);
        let w = width.max(8);
        // Codebooks: raw for all but the DC bundles (16 high colour trees first).
        for i in 0..9 {
            if i == COLORS {
                for _ in 0..16 {
                    wr.put(0, 4);
                }
            }
            if i != INTRA_DC && i != INTER_DC {
                wr.put(0, 4);
            }
        }
        let lens = [
            len_bits((w >> 3) + 511),
            len_bits((w >> 4) + 511),
            len_bits(bw * 64 + 511),
            len_bits((bw << 3) + 511),
            len_bits((w >> 3) + 511),
            len_bits((w >> 3) + 511),
            len_bits((w >> 3) + 511),
            len_bits((w >> 3) + 511),
            len_bits(bw * 48 + 511),
        ];
        // First row: block types (all fills), no sub types, the colours, the rest end.
        wr.put(blocks as u32, lens[0]);
        wr.put(1, 1);
        wr.put(FILL as u32, 4);
        wr.put(0, lens[1]);
        wr.put(blocks as u32, lens[2]);
        wr.put(1, 1);
        wr.put(color as u32 >> 4, 4);
        wr.put(color as u32 & 15, 4);
        for &l in &lens[3..] {
            wr.put(0, l);
        }
        wr.align32();
    }

    #[test]
    fn fill_blocks_decode_from_a_made_up_frame() {
        let (w, h) = (16usize, 8usize);
        let mut wr = Writer::default();
        wr.put(0, 32);
        // Luma: 2x1 blocks; chroma: 8x4, one block each.
        fill_plane(&mut wr, w, 2, 0x51);
        fill_plane(&mut wr, w / 2, 1, 0x80);
        fill_plane(&mut wr, w / 2, 1, 0x7f);
        let mut d = VideoDecoder::new(&header(w as u32, h as u32)).unwrap();
        let f = d.decode(&wr.bytes).expect("decodes");
        assert!(f.planes[0].row(7).iter().all(|&p| p == 0x51));
        // Red chroma is coded first.
        assert_eq!(f.planes[2].row(3), &[0x80; 8]);
        assert_eq!(f.planes[1].row(0), &[0x7f; 8]);
        let mut yuv = Vec::new();
        f.write_yuv420(&mut yuv);
        assert_eq!(yuv.len(), 16 * 8 + 2 * 8 * 4);
    }

    #[test]
    fn damaged_packets_are_errors_not_panics() {
        let mut wr = Writer::default();
        wr.put(0, 32);
        fill_plane(&mut wr, 16, 2, 0x51);
        fill_plane(&mut wr, 8, 1, 0x80);
        fill_plane(&mut wr, 8, 1, 0x7f);
        let good = wr.bytes;
        let mut d = VideoDecoder::new(&header(16, 8)).unwrap();
        for cut in 0..good.len() {
            let _ = d.decode(&good[..cut]);
        }
        // Noise: every byte pattern decodes to something or an error.
        let mut x = 0x1234_5678u32;
        for _ in 0..300 {
            let pkt: Vec<u8> = (0..200)
                .map(|_| {
                    x ^= x << 13;
                    x ^= x >> 17;
                    x ^= x << 5;
                    x as u8
                })
                .collect();
            let _ = d.decode(&pkt);
        }
    }

    #[test]
    fn idct_of_a_dc_alone_is_flat() {
        let mut b = [0i32; 64];
        // A DC of 8 times the level, dequantised by 1.0 in 16.16 (>> 11 leaves 32x).
        b[0] = 100 * 8;
        dequant(&mut b, &[0x10000; 64]);
        idct(&mut b);
        assert!(b.iter().all(|&v| v == 100), "{b:?}");
    }

    #[test]
    fn trees_shuffle_by_merging() {
        // Named: book 5, three symbols 9, 1, 7 first.
        let mut wr = Writer::default();
        wr.put(5, 4);
        wr.put(1, 1);
        wr.put(2, 3);
        for s in [9, 1, 7] {
            wr.put(s, 4);
        }
        let t = Tree::read(&mut Bits::new(&wr.bytes));
        assert_eq!(t.book, 5);
        assert_eq!(
            &t.syms,
            &[9, 1, 7, 0, 2, 3, 4, 5, 6, 8, 10, 11, 12, 13, 14, 15]
        );
        // Merged: one round swapping pairs 1 and 2.
        let mut wr = Writer::default();
        wr.put(3, 4);
        wr.put(0, 1);
        wr.put(0, 2);
        for i in 0..8 {
            wr.put((i == 1 || i == 2) as u32, 1);
        }
        let t = Tree::read(&mut Bits::new(&wr.bytes));
        assert_eq!(
            &t.syms,
            &[0, 1, 3, 2, 5, 4, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]
        );
    }
}
