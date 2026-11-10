//! Scaleform GFx movies: an SWF container with a `GFX` (uncompressed) or `CFX` (zlib) signature
//! and Scaleform-only tags numbered from 1000.
//!
//! Ruffle reads the container directly but ignores the Scaleform tags, so images the exporter
//! moved out of the movie (`DefineExternalImage`) would be missing. [`rewrite`] turns a GFx movie
//! into a plain SWF, inlining each external image as `DefineBitsLossless2` and dropping the other
//! Scaleform tags; [`scan`] inventories a movie for reports.
//!
//! Provisional: the Scaleform tag numbers and the `DefineExternalImage` layout are inferred from
//! the shipped movies (id, bitmap format, target width, target height, an empty export name and
//! a length-prefixed file name), not from Scaleform documentation or the executable.

use std::collections::BTreeMap;
use std::io::{Read, Write};

use flate2::Compression;
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;

#[derive(Debug, thiserror::Error)]
pub enum GfxError {
    #[error("not an SWF/GFx movie")]
    BadSignature,
    #[error("truncated movie")]
    Truncated,
    #[error("decompressing: {0}")]
    Zlib(#[from] std::io::Error),
    #[error("LZMA-compressed SWF is not supported")]
    Lzma,
}

/// Scaleform tag codes seen in the game's movies, with provisional names.
pub fn scaleform_tag_name(code: u16) -> Option<&'static str> {
    Some(match code {
        1000 => "ExporterInfo",
        1001 => "DefineExternalImage",
        1002 => "FontTextureInfo",
        1003 => "DefineExternalGradient",
        1004 => "DefineGradientMap",
        1005 => "DefineCompactedFont",
        1006 => "DefineExternalSound",
        1007 => "DefineExternalStreamSound",
        1008 => "DefineSubImage",
        1009 => "DefineExternalImage2",
        _ => return None,
    })
}

const TAG_END: u16 = 0;
const TAG_SHOW_FRAME: u16 = 1;
const TAG_DO_ACTION: u16 = 12;
const TAG_DEFINE_SPRITE: u16 = 39;
const TAG_IMPORT_ASSETS: u16 = 57;
const TAG_IMPORT_ASSETS2: u16 = 71;
const TAG_DEFINE_BITS_LOSSLESS2: u16 = 36;
const TAG_EXTERNAL_IMAGE: u16 = 1001;

#[derive(Clone, Copy, Debug)]
struct Tag<'a> {
    code: u16,
    /// The whole tag including its header.
    raw: &'a [u8],
    body: &'a [u8],
}

/// The decompressed movie: signature kind, version and the bytes after the 8-byte header.
pub struct Movie {
    pub signature: [u8; 3],
    pub version: u8,
    pub body: Vec<u8>,
}

impl Movie {
    pub fn parse(data: &[u8]) -> Result<Self, GfxError> {
        if data.len() < 8 {
            return Err(GfxError::Truncated);
        }
        let signature = [data[0], data[1], data[2]];
        let version = data[3];
        let body = match &signature {
            b"FWS" | b"GFX" => data[8..].to_vec(),
            b"CWS" | b"CFX" => {
                let mut out = Vec::new();
                ZlibDecoder::new(&data[8..]).read_to_end(&mut out)?;
                out
            }
            b"ZWS" => return Err(GfxError::Lzma),
            _ => return Err(GfxError::BadSignature),
        };
        Ok(Self {
            signature,
            version,
            body,
        })
    }

    pub fn is_scaleform(&self) -> bool {
        matches!(&self.signature, b"GFX" | b"CFX")
    }

    /// The authored stage size in pixels, from the header's RECT.
    pub fn stage_size(&self) -> Option<(f32, f32)> {
        let first = *self.body.first()?;
        let bits = (first >> 3) as usize;
        let total = 5 + 4 * bits;
        if total.div_ceil(8) > self.body.len() || bits == 0 {
            return None;
        }
        let read = |start: usize| -> i64 {
            let mut v: i64 = 0;
            for i in start..start + bits {
                let bit = (self.body[i / 8] >> (7 - i % 8)) & 1;
                v = (v << 1) | bit as i64;
            }
            // Sign-extend.
            if v & (1 << (bits - 1)) != 0 {
                v -= 1 << bits;
            }
            v
        };
        let (x0, x1, y0, y1) = (
            read(5),
            read(5 + bits),
            read(5 + 2 * bits),
            read(5 + 3 * bits),
        );
        Some(((x1 - x0) as f32 / 20.0, (y1 - y0) as f32 / 20.0))
    }

    /// Length of the RECT, frame rate and frame count that precede the tags.
    fn header_len(&self) -> Result<usize, GfxError> {
        let first = *self.body.first().ok_or(GfxError::Truncated)?;
        let bits = 5 + 4 * (first >> 3) as usize;
        let len = bits.div_ceil(8) + 4;
        if len > self.body.len() {
            return Err(GfxError::Truncated);
        }
        Ok(len)
    }

    fn tags(&self) -> Result<Vec<Tag<'_>>, GfxError> {
        let start = self.header_len()?;
        read_tags(&self.body[start..])
    }
}

/// Tags up to and including `End`. Scaleform appends data after the root `End`; it is ignored.
fn read_tags(mut d: &[u8]) -> Result<Vec<Tag<'_>>, GfxError> {
    let mut out = Vec::new();
    while !d.is_empty() {
        let h = u16::from_le_bytes([d[0], *d.get(1).ok_or(GfxError::Truncated)?]);
        let code = h >> 6;
        let (hl, len): (usize, usize) = if h & 0x3f == 0x3f {
            let l = d.get(2..6).ok_or(GfxError::Truncated)?;
            (6, u32::from_le_bytes([l[0], l[1], l[2], l[3]]) as usize)
        } else {
            (2, (h & 0x3f) as usize)
        };
        let end = hl.checked_add(len).ok_or(GfxError::Truncated)?;
        if end > d.len() {
            return Err(GfxError::Truncated);
        }
        out.push(Tag {
            code,
            raw: &d[..end],
            body: &d[hl..end],
        });
        d = &d[end..];
        if code == TAG_END {
            break;
        }
    }
    Ok(out)
}

fn tag_header(code: u16, len: usize, out: &mut Vec<u8>) {
    if len < 0x3f {
        out.extend_from_slice(&((code << 6) | len as u16).to_le_bytes());
    } else {
        out.extend_from_slice(&((code << 6) | 0x3f).to_le_bytes());
        out.extend_from_slice(&(len as u32).to_le_bytes());
    }
}

/// An image the exporter left outside the movie.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalImage {
    pub id: u16,
    pub format: u16,
    pub width: u16,
    pub height: u16,
    pub file: String,
}

fn pstr(d: &[u8]) -> Option<(String, &[u8])> {
    let (&n, rest) = d.split_first()?;
    let s = rest.get(..n as usize)?;
    Some((String::from_utf8_lossy(s).into_owned(), &rest[n as usize..]))
}

impl ExternalImage {
    fn parse(d: &[u8]) -> Option<Self> {
        let u = |i: usize| Some(u16::from_le_bytes([*d.get(i)?, *d.get(i + 1)?]));
        let (id, format, width, height) = (u(0)?, u(2)?, u(4)?, u(6)?);
        let (_export, rest) = pstr(d.get(8..)?)?;
        let (file, _) = pstr(rest)?;
        Some(Self {
            id,
            format,
            width,
            height,
            file,
        })
    }
}

/// What a movie contains, for reports.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Inventory {
    pub signature: String,
    pub version: u8,
    /// Tag code to count, including tags inside sprites.
    pub tags: BTreeMap<u16, u32>,
    pub external_images: Vec<ExternalImage>,
    /// `ImportAssets` URLs with the imported names.
    pub imports: Vec<(String, Vec<String>)>,
}

impl Inventory {
    pub fn scaleform_tags(&self) -> impl Iterator<Item = (u16, u32)> + '_ {
        self.tags
            .iter()
            .filter(|(c, _)| **c >= 1000)
            .map(|(c, n)| (*c, *n))
    }
}

fn parse_import(code: u16, d: &[u8]) -> Option<(String, Vec<String>)> {
    let nul = d.iter().position(|&b| b == 0)?;
    let url = String::from_utf8_lossy(&d[..nul]).into_owned();
    let mut rest = &d[nul + 1..];
    if code == TAG_IMPORT_ASSETS2 {
        rest = rest.get(2..)?;
    }
    let count = u16::from_le_bytes([*rest.first()?, *rest.get(1)?]);
    rest = &rest[2..];
    let mut names = Vec::new();
    for _ in 0..count {
        rest = rest.get(2..)?;
        let nul = rest.iter().position(|&b| b == 0)?;
        names.push(String::from_utf8_lossy(&rest[..nul]).into_owned());
        rest = &rest[nul + 1..];
    }
    Some((url, names))
}

fn inventory_tags(tags: &[Tag], inv: &mut Inventory) {
    for t in tags {
        *inv.tags.entry(t.code).or_default() += 1;
        match t.code {
            TAG_EXTERNAL_IMAGE => inv.external_images.extend(ExternalImage::parse(t.body)),
            TAG_IMPORT_ASSETS | TAG_IMPORT_ASSETS2 => {
                inv.imports.extend(parse_import(t.code, t.body))
            }
            TAG_DEFINE_SPRITE => {
                if let Some(Ok(inner)) = t.body.get(4..).map(read_tags) {
                    inventory_tags(&inner, inv);
                }
            }
            _ => {}
        }
    }
}

pub fn scan(data: &[u8]) -> Result<Inventory, GfxError> {
    let m = Movie::parse(data)?;
    let mut inv = Inventory {
        signature: String::from_utf8_lossy(&m.signature).into_owned(),
        version: m.version,
        ..Default::default()
    };
    inventory_tags(&m.tags()?, &mut inv);
    Ok(inv)
}

/// Straight RGBA8 pixels.
pub struct Rgba {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

/// What [`rewrite`] did.
#[derive(Debug, Default)]
pub struct RewriteReport {
    pub images_inlined: Vec<String>,
    pub images_missing: Vec<String>,
    /// Scaleform tags dropped, by code.
    pub dropped: BTreeMap<u16, u32>,
}

fn lossless2(id: u16, img: &Rgba) -> Option<Vec<u8>> {
    let (w, h) = (
        u16::try_from(img.width).ok()?,
        u16::try_from(img.height).ok()?,
    );
    if img.pixels.len() != img.width as usize * img.height as usize * 4 {
        return None;
    }
    // Premultiplied ARGB.
    let mut argb = Vec::with_capacity(img.pixels.len());
    for p in img.pixels.chunks_exact(4) {
        let a = p[3] as u32;
        let pm = |c: u8| ((c as u32 * a + 127) / 255) as u8;
        argb.extend_from_slice(&[p[3], pm(p[0]), pm(p[1]), pm(p[2])]);
    }
    let mut z = ZlibEncoder::new(Vec::new(), Compression::fast());
    z.write_all(&argb).ok()?;
    let data = z.finish().ok()?;
    let mut body = Vec::with_capacity(data.len() + 7);
    body.extend_from_slice(&id.to_le_bytes());
    body.push(5);
    body.extend_from_slice(&w.to_le_bytes());
    body.extend_from_slice(&h.to_le_bytes());
    body.extend_from_slice(&data);
    let mut out = Vec::new();
    tag_header(TAG_DEFINE_BITS_LOSSLESS2, body.len(), &mut out);
    out.extend_from_slice(&body);
    Some(out)
}

/// Nearest-neighbour resize, used when an image's decoded size differs from the size the
/// movie expects (bitmap fill matrices are authored against the target size).
pub fn resize(img: &Rgba, w: u32, h: u32) -> Rgba {
    if (img.width, img.height) == (w, h) || img.width == 0 || img.height == 0 {
        return Rgba {
            width: img.width,
            height: img.height,
            pixels: img.pixels.clone(),
        };
    }
    let mut pixels = Vec::with_capacity(w as usize * h as usize * 4);
    for y in 0..h {
        let sy = (y as u64 * img.height as u64 / h.max(1) as u64) as usize;
        for x in 0..w {
            let sx = (x as u64 * img.width as u64 / w.max(1) as u64) as usize;
            let i = (sy * img.width as usize + sx) * 4;
            pixels.extend_from_slice(img.pixels.get(i..i + 4).unwrap_or(&[0; 4]));
        }
    }
    Rgba {
        width: w,
        height: h,
        pixels,
    }
}

fn rewrite_tags(
    tags: &[Tag],
    images: &mut dyn FnMut(&ExternalImage) -> Option<Rgba>,
    report: &mut RewriteReport,
    out: &mut Vec<u8>,
) {
    for t in tags {
        match t.code {
            TAG_EXTERNAL_IMAGE => {
                let Some(ext) = ExternalImage::parse(t.body) else {
                    *report.dropped.entry(t.code).or_default() += 1;
                    continue;
                };
                let tag = images(&ext)
                    .map(|img| resize(&img, ext.width as u32, ext.height as u32))
                    .and_then(|img| lossless2(ext.id, &img));
                match tag {
                    Some(tag) => {
                        out.extend_from_slice(&tag);
                        report.images_inlined.push(ext.file);
                    }
                    None => report.images_missing.push(ext.file),
                }
            }
            TAG_DEFINE_SPRITE if t.body.len() >= 4 => match read_tags(&t.body[4..]) {
                Ok(inner) => {
                    let mut body = t.body[..4].to_vec();
                    rewrite_tags(&inner, images, report, &mut body);
                    tag_header(t.code, body.len(), out);
                    out.extend_from_slice(&body);
                }
                Err(_) => out.extend_from_slice(t.raw),
            },
            c if c >= 1000 => *report.dropped.entry(c).or_default() += 1,
            _ => out.extend_from_slice(t.raw),
        }
    }
}

/// Rewrites a GFx movie as an uncompressed SWF, inlining external images that `images` can
/// supply. `inject` is a `DoAction` body placed just before the first frame ends.
pub fn rewrite(
    data: &[u8],
    images: &mut dyn FnMut(&ExternalImage) -> Option<Rgba>,
    inject: Option<&[u8]>,
) -> Result<(Vec<u8>, RewriteReport), GfxError> {
    let m = Movie::parse(data)?;
    let hl = m.header_len()?;
    let tags = m.tags()?;
    let mut report = RewriteReport::default();
    let mut body = m.body[..hl].to_vec();
    let split = tags
        .iter()
        .position(|t| t.code == TAG_SHOW_FRAME || t.code == TAG_END)
        .unwrap_or(tags.len());
    rewrite_tags(&tags[..split], images, &mut report, &mut body);
    if let Some(actions) = inject {
        tag_header(TAG_DO_ACTION, actions.len(), &mut body);
        body.extend_from_slice(actions);
    }
    rewrite_tags(&tags[split..], images, &mut report, &mut body);
    if tags.last().map(|t| t.code) != Some(TAG_END) {
        body.extend_from_slice(&[0, 0]);
    }
    let mut out = Vec::with_capacity(body.len() + 8);
    out.extend_from_slice(b"FWS");
    out.push(m.version);
    out.extend_from_slice(&((body.len() + 8) as u32).to_le_bytes());
    out.extend_from_slice(&body);
    Ok((out, report))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag(code: u16, body: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        tag_header(code, body.len(), &mut out);
        out.extend_from_slice(body);
        out
    }

    /// A tiny CFX movie: ExporterInfo, one external image, a sprite holding a Scaleform tag,
    /// an ImportAssets2, ShowFrame and End, followed by trailing junk.
    fn synthetic_cfx() -> Vec<u8> {
        // RECT with nbits=0 (one byte), frame rate, frame count.
        let mut body = vec![0u8, 0, 24, 1, 0];
        body.extend(tag(1000, b"\x14\x02\x00\x00"));
        let mut ext = Vec::new();
        for v in [7u16, 14, 2, 1] {
            ext.extend_from_slice(&v.to_le_bytes());
        }
        ext.push(0);
        ext.push(5);
        ext.extend_from_slice(b"a.dds");
        body.extend(tag(1001, &ext));
        let mut sprite = vec![9, 0, 1, 0];
        sprite.extend(tag(1003, b"xy"));
        sprite.extend(tag(1, b""));
        sprite.extend(tag(0, b""));
        body.extend(tag(39, &sprite));
        body.extend(tag(71, b"lib.swf\0\x01\x00\x01\x00\x03\x00$Font\0"));
        body.extend(tag(1, b""));
        body.extend(tag(0, b""));
        body.extend_from_slice(b"junk after end");
        let mut z = ZlibEncoder::new(Vec::new(), Compression::default());
        z.write_all(&body).unwrap();
        let z = z.finish().unwrap();
        let mut out = b"CFX\x08".to_vec();
        out.extend_from_slice(&((body.len() + 8) as u32).to_le_bytes());
        out.extend(z);
        out
    }

    #[test]
    fn scans_scaleform_tags() {
        let inv = scan(&synthetic_cfx()).unwrap();
        assert_eq!(inv.signature, "CFX");
        assert_eq!(
            inv.scaleform_tags().collect::<Vec<_>>(),
            vec![(1000, 1), (1001, 1), (1003, 1)]
        );
        assert_eq!(
            inv.external_images,
            vec![ExternalImage {
                id: 7,
                format: 14,
                width: 2,
                height: 1,
                file: "a.dds".into()
            }]
        );
        assert_eq!(
            inv.imports,
            vec![("lib.swf".to_string(), vec!["$Font".to_string()])]
        );
    }

    #[test]
    fn rewrites_to_plain_swf() {
        let (swf, rep) = rewrite(
            &synthetic_cfx(),
            &mut |e| {
                (e.file == "a.dds").then(|| Rgba {
                    width: 2,
                    height: 1,
                    pixels: vec![255, 0, 0, 255, 0, 255, 0, 128],
                })
            },
            Some(&[0]),
        )
        .unwrap();
        assert_eq!(&swf[..4], b"FWS\x08");
        assert_eq!(
            u32::from_le_bytes([swf[4], swf[5], swf[6], swf[7]]) as usize,
            swf.len()
        );
        assert_eq!(rep.images_inlined, vec!["a.dds".to_string()]);
        assert_eq!(rep.dropped.get(&1000), Some(&1));
        assert_eq!(rep.dropped.get(&1003), Some(&1));
        let inv = scan(&swf).unwrap();
        assert_eq!(inv.scaleform_tags().count(), 0);
        assert_eq!(inv.tags.get(&36), Some(&1));
        assert_eq!(inv.tags.get(&39), Some(&1));
        // The sprite kept its ShowFrame and End.
        assert_eq!(inv.tags.get(&0), Some(&2));
        assert_eq!(inv.tags.get(&12), Some(&1));
    }

    #[test]
    fn reads_the_stage_size() {
        // RECT nbits=16: 0, 20480 (1024 px), 0, 15360 (768 px).
        let mut bits = String::from("10000");
        for v in [0u32, 20480, 0, 15360] {
            bits.push_str(&format!("{v:016b}"));
        }
        while bits.len() % 8 != 0 {
            bits.push('0');
        }
        let mut body: Vec<u8> = (0..bits.len() / 8)
            .map(|i| u8::from_str_radix(&bits[i * 8..i * 8 + 8], 2).unwrap())
            .collect();
        body.extend([0, 24, 1, 0, 0, 0]);
        let m = Movie {
            signature: *b"FWS",
            version: 8,
            body,
        };
        assert_eq!(m.stage_size(), Some((1024.0, 768.0)));
    }

    #[test]
    fn rejects_bad_input() {
        assert!(matches!(scan(b"abc"), Err(GfxError::Truncated)));
        assert!(matches!(
            scan(b"XYZ\x08\0\0\0\0"),
            Err(GfxError::BadSignature)
        ));
        assert!(scan(b"GFX\x08\0\0\0\0\xff").is_err());
        // A tag whose length runs past the end.
        assert!(scan(b"GFX\x08\0\0\0\0\0\0\0\0\0\x3f\x00").is_err());
    }
}
