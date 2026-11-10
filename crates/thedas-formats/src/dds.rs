//! DirectDraw Surface textures (`.dds`), as DA:O ships them: DXT1/3/5 (BC1/2/3) plus a few
//! uncompressed layouts (A8R8G8B8, X8R8G8B8, R8G8B8, A4R4G4B4, L8), as 2D textures, cube maps
//! and a handful of volume textures.
//!
//! Compressed data can be handed to the GPU as is. [`Dds::decode_rgba8`] expands any
//! supported surface to RGBA8 on the CPU for devices without BC support and for tools.

#[derive(Debug, thiserror::Error)]
pub enum DdsError {
    #[error("not a DDS file")]
    NotDds,
    #[error("unsupported pixel format: {0}")]
    Unsupported(String),
    #[error("bad dimensions {0}x{1}x{2}")]
    BadSize(u32, u32, u32),
    #[error("data too short: need {need} bytes, have {have}")]
    Truncated { need: u64, have: u64 },
    #[error("no surface for face {face} mip {mip}")]
    NoSurface { face: u32, mip: u32 },
}

pub type Result<T> = std::result::Result<T, DdsError>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DdsFormat {
    Bc1,
    Bc2,
    Bc3,
    /// A8R8G8B8: bytes B, G, R, A.
    Bgra8,
    /// X8R8G8B8: bytes B, G, R, unused.
    Bgrx8,
    /// R8G8B8: bytes B, G, R.
    Bgr8,
    /// A4R4G4B4, little-endian 16-bit.
    Bgra4,
    /// 8-bit luminance.
    L8,
}

impl DdsFormat {
    pub fn is_compressed(self) -> bool {
        matches!(self, DdsFormat::Bc1 | DdsFormat::Bc2 | DdsFormat::Bc3)
    }

    /// Bytes per 4x4 block for compressed formats, per pixel otherwise.
    pub fn unit_size(self) -> u64 {
        match self {
            DdsFormat::Bc1 => 8,
            DdsFormat::Bc2 | DdsFormat::Bc3 => 16,
            DdsFormat::Bgra8 | DdsFormat::Bgrx8 => 4,
            DdsFormat::Bgr8 => 3,
            DdsFormat::Bgra4 => 2,
            DdsFormat::L8 => 1,
        }
    }

    /// Byte size of one surface.
    pub fn surface_size(self, w: u32, h: u32) -> u64 {
        if self.is_compressed() {
            w.div_ceil(4).max(1) as u64 * h.div_ceil(4).max(1) as u64 * self.unit_size()
        } else {
            w as u64 * h as u64 * self.unit_size()
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DdsKind {
    Texture2D,
    Cube,
    Volume,
}

/// One mip level of one face.
#[derive(Clone, Copy, Debug)]
pub struct Surface<'a> {
    pub width: u32,
    pub height: u32,
    pub depth: u32,
    pub data: &'a [u8],
}

#[derive(Clone, Debug)]
pub struct Dds {
    pub width: u32,
    pub height: u32,
    pub depth: u32,
    pub mip_count: u32,
    pub format: DdsFormat,
    pub kind: DdsKind,
    /// Everything after the 128-byte header.
    pub data: Vec<u8>,
}

const DDPF_ALPHAPIXELS: u32 = 0x1;
const DDPF_FOURCC: u32 = 0x4;
const DDPF_RGB: u32 = 0x40;
const DDPF_LUMINANCE: u32 = 0x20000;
const DDSCAPS2_CUBEMAP: u32 = 0x200;
const DDSCAPS2_VOLUME: u32 = 0x20_0000;

fn u32_at(d: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([d[at], d[at + 1], d[at + 2], d[at + 3]])
}

impl Dds {
    pub fn parse(d: &[u8]) -> Result<Self> {
        if d.len() < 128 || &d[..4] != b"DDS " || u32_at(d, 4) != 124 {
            return Err(DdsError::NotDds);
        }
        let height = u32_at(d, 12);
        let width = u32_at(d, 16);
        let depth = u32_at(d, 24).max(1);
        let mip_count = u32_at(d, 28).max(1);
        let pf_flags = u32_at(d, 80);
        let fourcc = &d[84..88];
        let bits = u32_at(d, 88);
        let masks = [u32_at(d, 92), u32_at(d, 96), u32_at(d, 100), u32_at(d, 104)];
        let caps2 = u32_at(d, 112);

        let format = if pf_flags & DDPF_FOURCC != 0 {
            match fourcc {
                b"DXT1" => DdsFormat::Bc1,
                b"DXT2" | b"DXT3" => DdsFormat::Bc2,
                b"DXT4" | b"DXT5" => DdsFormat::Bc3,
                _ => {
                    return Err(DdsError::Unsupported(format!(
                        "FourCC {:?}",
                        String::from_utf8_lossy(fourcc)
                    )));
                }
            }
        } else if pf_flags & DDPF_RGB != 0 {
            let alpha = pf_flags & DDPF_ALPHAPIXELS != 0;
            match (bits, masks, alpha) {
                (32, [0xff_0000, 0xff00, 0xff, 0xff00_0000], true) => DdsFormat::Bgra8,
                (32, [0xff_0000, 0xff00, 0xff, _], false) => DdsFormat::Bgrx8,
                (24, [0xff_0000, 0xff00, 0xff, _], _) => DdsFormat::Bgr8,
                (16, [0xf00, 0xf0, 0xf, 0xf000], _) => DdsFormat::Bgra4,
                _ => {
                    return Err(DdsError::Unsupported(format!(
                        "RGB {bits} bits, masks {masks:x?}"
                    )));
                }
            }
        } else if pf_flags & DDPF_LUMINANCE != 0 && bits == 8 {
            DdsFormat::L8
        } else {
            return Err(DdsError::Unsupported(format!(
                "pixel format flags {pf_flags:#x}, {bits} bits"
            )));
        };
        let kind = if caps2 & DDSCAPS2_CUBEMAP != 0 {
            DdsKind::Cube
        } else if caps2 & DDSCAPS2_VOLUME != 0 {
            DdsKind::Volume
        } else {
            DdsKind::Texture2D
        };
        let depth = if kind == DdsKind::Volume { depth } else { 1 };
        if width == 0 || height == 0 || width > 16384 || height > 16384 || depth > 2048 {
            return Err(DdsError::BadSize(width, height, depth));
        }
        // A full chain never has more levels than log2(max dimension) + 1.
        let max_mips = 32 - width.max(height).max(depth).leading_zeros();
        let dds = Dds {
            width,
            height,
            depth,
            mip_count: mip_count.min(max_mips),
            format,
            kind,
            data: d[128..].to_vec(),
        };
        let need = dds.total_size();
        if need > dds.data.len() as u64 {
            return Err(DdsError::Truncated {
                need,
                have: dds.data.len() as u64,
            });
        }
        Ok(dds)
    }

    pub fn faces(&self) -> u32 {
        if self.kind == DdsKind::Cube { 6 } else { 1 }
    }

    pub fn mip_size(&self, mip: u32) -> (u32, u32, u32) {
        (
            (self.width >> mip).max(1),
            (self.height >> mip).max(1),
            (self.depth >> mip).max(1),
        )
    }

    fn face_size(&self) -> u64 {
        (0..self.mip_count)
            .map(|m| {
                let (w, h, d) = self.mip_size(m);
                self.format.surface_size(w, h) * d as u64
            })
            .sum()
    }

    fn total_size(&self) -> u64 {
        self.face_size() * self.faces() as u64
    }

    /// One mip level of one face (faces are stored one after another, each with its mips).
    pub fn surface(&self, face: u32, mip: u32) -> Result<Surface<'_>> {
        if face >= self.faces() || mip >= self.mip_count {
            return Err(DdsError::NoSurface { face, mip });
        }
        let mut at = self.face_size() * face as u64;
        for m in 0..mip {
            let (w, h, d) = self.mip_size(m);
            at += self.format.surface_size(w, h) * d as u64;
        }
        let (w, h, d) = self.mip_size(mip);
        let len = self.format.surface_size(w, h) * d as u64;
        let end = at + len;
        if end > self.data.len() as u64 {
            return Err(DdsError::Truncated {
                need: end,
                have: self.data.len() as u64,
            });
        }
        Ok(Surface {
            width: w,
            height: h,
            depth: d,
            data: &self.data[at as usize..end as usize],
        })
    }

    /// Expands one surface to tightly packed RGBA8 (`width * height * depth * 4` bytes).
    pub fn decode_rgba8(&self, face: u32, mip: u32) -> Result<Vec<u8>> {
        let s = self.surface(face, mip)?;
        let (w, h) = (s.width as usize, s.height as usize);
        let mut out = vec![0u8; w * h * s.depth as usize * 4];
        let slice = self.format.surface_size(s.width, s.height) as usize;
        for z in 0..s.depth as usize {
            let src = &s.data[z * slice..(z + 1) * slice];
            let dst = &mut out[z * w * h * 4..(z + 1) * w * h * 4];
            match self.format {
                DdsFormat::Bc1 | DdsFormat::Bc2 | DdsFormat::Bc3 => {
                    decode_bc(self.format, src, w, h, dst)
                }
                DdsFormat::Bgra8 | DdsFormat::Bgrx8 => {
                    for (o, i) in dst.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
                        let a = if self.format == DdsFormat::Bgra8 {
                            i[3]
                        } else {
                            255
                        };
                        o.copy_from_slice(&[i[2], i[1], i[0], a]);
                    }
                }
                DdsFormat::Bgr8 => {
                    for (o, i) in dst.chunks_exact_mut(4).zip(src.chunks_exact(3)) {
                        o.copy_from_slice(&[i[2], i[1], i[0], 255]);
                    }
                }
                DdsFormat::Bgra4 => {
                    for (o, i) in dst.chunks_exact_mut(4).zip(src.chunks_exact(2)) {
                        let v = u16::from_le_bytes([i[0], i[1]]);
                        let c = |s: u16| (((v >> s) & 0xf) as u8) * 17;
                        o.copy_from_slice(&[c(8), c(4), c(0), c(12)]);
                    }
                }
                DdsFormat::L8 => {
                    for (o, &l) in dst.chunks_exact_mut(4).zip(src) {
                        o.copy_from_slice(&[l, l, l, 255]);
                    }
                }
            }
        }
        Ok(out)
    }
}

fn rgb565(c: u16) -> [u8; 3] {
    let r = ((c >> 11) & 0x1f) as u32;
    let g = ((c >> 5) & 0x3f) as u32;
    let b = (c & 0x1f) as u32;
    [
        ((r * 255 + 15) / 31) as u8,
        ((g * 255 + 31) / 63) as u8,
        ((b * 255 + 15) / 31) as u8,
    ]
}

/// Decodes a BC1 colour block into 16 RGBA texels. `four_colour` forces the four-colour mode
/// (BC2/BC3 colour blocks always use it).
fn bc1_block(b: &[u8], four_colour: bool) -> [[u8; 4]; 16] {
    let c0 = u16::from_le_bytes([b[0], b[1]]);
    let c1 = u16::from_le_bytes([b[2], b[3]]);
    let (p0, p1) = (rgb565(c0), rgb565(c1));
    let mix = |a: u8, b: u8, wa: u32, wb: u32| ((a as u32 * wa + b as u32 * wb) / (wa + wb)) as u8;
    let mut pal = [[0u8; 4]; 4];
    pal[0] = [p0[0], p0[1], p0[2], 255];
    pal[1] = [p1[0], p1[1], p1[2], 255];
    if four_colour || c0 > c1 {
        for i in 0..3 {
            pal[2][i] = mix(p0[i], p1[i], 2, 1);
            pal[3][i] = mix(p0[i], p1[i], 1, 2);
        }
        pal[2][3] = 255;
        pal[3][3] = 255;
    } else {
        for i in 0..3 {
            pal[2][i] = mix(p0[i], p1[i], 1, 1);
        }
        pal[2][3] = 255;
        pal[3] = [0, 0, 0, 0];
    }
    let bits = u32::from_le_bytes([b[4], b[5], b[6], b[7]]);
    let mut out = [[0u8; 4]; 16];
    for (i, o) in out.iter_mut().enumerate() {
        *o = pal[((bits >> (2 * i)) & 3) as usize];
    }
    out
}

fn bc3_alpha(b: &[u8]) -> [u8; 16] {
    let (a0, a1) = (b[0] as u32, b[1] as u32);
    let mut pal = [0u8; 8];
    pal[0] = a0 as u8;
    pal[1] = a1 as u8;
    if a0 > a1 {
        for i in 1..7u32 {
            pal[i as usize + 1] = (((7 - i) * a0 + i * a1) / 7) as u8;
        }
    } else {
        for i in 1..5u32 {
            pal[i as usize + 1] = (((5 - i) * a0 + i * a1) / 5) as u8;
        }
        pal[6] = 0;
        pal[7] = 255;
    }
    let mut bits = 0u64;
    for (i, &x) in b[2..8].iter().enumerate() {
        bits |= (x as u64) << (8 * i);
    }
    let mut out = [0u8; 16];
    for (i, o) in out.iter_mut().enumerate() {
        *o = pal[((bits >> (3 * i)) & 7) as usize];
    }
    out
}

fn decode_bc(format: DdsFormat, src: &[u8], w: usize, h: usize, dst: &mut [u8]) {
    let bw = w.div_ceil(4).max(1);
    let bh = h.div_ceil(4).max(1);
    let size = format.unit_size() as usize;
    for by in 0..bh {
        for bx in 0..bw {
            let blk = &src[(by * bw + bx) * size..(by * bw + bx + 1) * size];
            let texels = match format {
                DdsFormat::Bc1 => bc1_block(blk, false),
                DdsFormat::Bc2 => {
                    let mut t = bc1_block(&blk[8..], true);
                    for (i, px) in t.iter_mut().enumerate() {
                        let nib = (blk[i / 2] >> (4 * (i % 2))) & 0xf;
                        px[3] = nib * 17;
                    }
                    t
                }
                _ => {
                    let mut t = bc1_block(&blk[8..], true);
                    let a = bc3_alpha(&blk[..8]);
                    for (px, a) in t.iter_mut().zip(a) {
                        px[3] = a;
                    }
                    t
                }
            };
            for (i, px) in texels.iter().enumerate() {
                let (x, y) = (bx * 4 + i % 4, by * 4 + i / 4);
                if x < w && y < h {
                    let o = (y * w + x) * 4;
                    dst[o..o + 4].copy_from_slice(px);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::too_many_arguments)]
    fn header(
        w: u32,
        h: u32,
        mips: u32,
        pf_flags: u32,
        fourcc: &[u8; 4],
        bits: u32,
        masks: [u32; 4],
        caps2: u32,
    ) -> Vec<u8> {
        let mut d = vec![0u8; 128];
        d[..4].copy_from_slice(b"DDS ");
        let put =
            |d: &mut Vec<u8>, at: usize, v: u32| d[at..at + 4].copy_from_slice(&v.to_le_bytes());
        put(&mut d, 4, 124);
        put(&mut d, 12, h);
        put(&mut d, 16, w);
        put(&mut d, 28, mips);
        put(&mut d, 76, 32);
        put(&mut d, 80, pf_flags);
        d[84..88].copy_from_slice(fourcc);
        put(&mut d, 88, bits);
        for (i, m) in masks.iter().enumerate() {
            put(&mut d, 92 + 4 * i, *m);
        }
        put(&mut d, 112, caps2);
        d
    }

    #[test]
    fn decodes_bc1_and_mips() {
        // 8x8 DXT1 with 4 mips (8, 4, 2, 1): 4 + 1 + 1 + 1 blocks.
        let mut d = header(8, 8, 4, DDPF_FOURCC, b"DXT1", 0, [0; 4], 0);
        // Block: c0 = pure red, c1 = pure blue, all texels index 0 except the first (index 1).
        let block = [0x00, 0xf8, 0x1f, 0x00, 0b01, 0, 0, 0];
        for _ in 0..7 {
            d.extend_from_slice(&block);
        }
        let dds = Dds::parse(&d).unwrap();
        assert_eq!(dds.format, DdsFormat::Bc1);
        assert_eq!(dds.mip_count, 4);
        assert_eq!(dds.surface(0, 3).unwrap().width, 1);
        let px = dds.decode_rgba8(0, 0).unwrap();
        assert_eq!(&px[..4], &[0, 0, 255, 255]);
        assert_eq!(&px[4..8], &[255, 0, 0, 255]);
        assert_eq!(dds.decode_rgba8(0, 3).unwrap().len(), 4);
        // One byte short.
        assert!(Dds::parse(&d[..d.len() - 1]).is_err());
    }

    #[test]
    fn decodes_bc3_alpha_and_uncompressed() {
        let mut d = header(4, 4, 1, DDPF_FOURCC, b"DXT5", 0, [0; 4], 0);
        d.extend_from_slice(&[255, 0, 0, 0, 0, 0, 0, 0]); // all alpha index 0 = 255
        d.extend_from_slice(&[0xff, 0xff, 0, 0, 0, 0, 0, 0]); // white
        let px = Dds::parse(&d).unwrap().decode_rgba8(0, 0).unwrap();
        assert!(px.chunks(4).all(|p| p == [255, 255, 255, 255]));

        let mut d = header(
            2,
            1,
            1,
            DDPF_RGB | DDPF_ALPHAPIXELS,
            b"\0\0\0\0",
            16,
            [0xf00, 0xf0, 0xf, 0xf000],
            0,
        );
        d.extend_from_slice(&0xf800u16.to_le_bytes()); // A=15, R=8
        d.extend_from_slice(&0x000fu16.to_le_bytes()); // B=15, A=0
        let px = Dds::parse(&d).unwrap().decode_rgba8(0, 0).unwrap();
        assert_eq!(px, vec![136, 0, 0, 255, 0, 0, 255, 0]);

        let mut d = header(
            1,
            1,
            1,
            DDPF_RGB,
            b"\0\0\0\0",
            24,
            [0xff0000, 0xff00, 0xff, 0],
            0,
        );
        d.extend_from_slice(&[1, 2, 3]);
        assert_eq!(
            Dds::parse(&d).unwrap().decode_rgba8(0, 0).unwrap(),
            vec![3, 2, 1, 255]
        );
    }

    #[test]
    fn cube_faces_and_errors() {
        let mut d = header(4, 4, 1, DDPF_FOURCC, b"DXT1", 0, [0; 4], 0xfe00);
        for face in 0..6u8 {
            d.extend_from_slice(&[face, 0, 0, 0, 0, 0, 0, 0]);
        }
        let dds = Dds::parse(&d).unwrap();
        assert_eq!(dds.kind, DdsKind::Cube);
        assert!(dds.surface(0, 5).is_err());
        assert_eq!(dds.surface(5, 0).unwrap().data[0], 5);
        assert!(Dds::parse(b"DDS ").is_err());
        let d = header(4, 4, 1, DDPF_FOURCC, b"ATI2", 0, [0; 4], 0);
        assert!(matches!(Dds::parse(&d), Err(DdsError::Unsupported(_))));
        let d = header(0, 4, 1, DDPF_FOURCC, b"DXT1", 0, [0; 4], 0);
        assert!(matches!(Dds::parse(&d), Err(DdsError::BadSize(..))));
    }
}
