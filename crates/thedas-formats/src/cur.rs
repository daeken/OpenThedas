//! Windows cursor files (`.cur`): the game's mouse cursors (`packages/core/data/cursors`).
//!
//! An icon directory (reserved 0, type 2, image count) with one 16-byte entry per image
//! (width, height, colours, reserved, hotspot x and y, byte size, offset), each image a
//! `BITMAPINFOHEADER` whose height counts the colour rows and the mask rows together, an
//! optional palette, the colour rows (bottom up, padded to four bytes) and a 1-bit mask
//! (set bits are transparent). The game's own are all one 32x32 8-bit image; 1, 4, 8, 24 and
//! 32-bit uncompressed images are read.

/// One image of a cursor file, decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cursor {
    pub width: u16,
    pub height: u16,
    /// The pixel that points, from the top left.
    pub hotspot: (u16, u16),
    /// `width * height` RGBA8 pixels, rows top down.
    pub rgba: Vec<u8>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CurError {
    #[error("truncated cursor file")]
    Truncated,
    #[error("not a cursor file")]
    NotCursor,
    #[error("unsupported cursor image: {0}")]
    Unsupported(String),
}

fn u16_at(d: &[u8], at: usize) -> Result<u16, CurError> {
    let b = d.get(at..at + 2).ok_or(CurError::Truncated)?;
    Ok(u16::from_le_bytes([b[0], b[1]]))
}

fn u32_at(d: &[u8], at: usize) -> Result<u32, CurError> {
    let b = d.get(at..at + 4).ok_or(CurError::Truncated)?;
    Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// The largest side read: cursors are small, and a hostile header must not ask for much.
const MAX_SIDE: u32 = 256;

impl Cursor {
    /// The first image of a cursor file.
    pub fn parse(data: &[u8]) -> Result<Cursor, CurError> {
        if u16_at(data, 0)? != 0 || u16_at(data, 2)? != 2 {
            return Err(CurError::NotCursor);
        }
        if u16_at(data, 4)? == 0 {
            return Err(CurError::Unsupported("no images".into()));
        }
        let hotspot = (u16_at(data, 10)?, u16_at(data, 12)?);
        let size = u32_at(data, 14)? as usize;
        let offset = u32_at(data, 18)? as usize;
        let image = data
            .get(offset..offset.checked_add(size).ok_or(CurError::Truncated)?)
            .ok_or(CurError::Truncated)?;
        let header = u32_at(image, 0)? as usize;
        if header < 40 || header > image.len() {
            return Err(CurError::Unsupported(format!("header of {header} bytes")));
        }
        let width = u32_at(image, 4)?;
        // The height counts the colour rows and the mask rows.
        let height = u32_at(image, 8)? / 2;
        let bits = u16_at(image, 14)?;
        let compression = u32_at(image, 16)?;
        if compression != 0 {
            return Err(CurError::Unsupported(format!("compression {compression}")));
        }
        if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
            return Err(CurError::Unsupported(format!("{width}x{height}")));
        }
        let (w, h) = (width as usize, height as usize);
        let colours = match bits {
            1 | 4 | 8 => match u32_at(image, 32)? as usize {
                0 => 1usize << bits,
                n if n <= 1 << bits => n,
                n => return Err(CurError::Unsupported(format!("{n} colours"))),
            },
            24 | 32 => 0,
            other => return Err(CurError::Unsupported(format!("{other} bits a pixel"))),
        };
        let palette = image
            .get(header..header + colours * 4)
            .ok_or(CurError::Truncated)?;
        let row = (w * bits as usize).div_ceil(32) * 4;
        let mask_row = w.div_ceil(32) * 4;
        let pixels_at = header + colours * 4;
        let pixels = image
            .get(pixels_at..pixels_at + row * h)
            .ok_or(CurError::Truncated)?;
        let mask = image
            .get(pixels_at + row * h..pixels_at + row * h + mask_row * h)
            .ok_or(CurError::Truncated)?;
        let mut rgba = vec![0u8; w * h * 4];
        // A 32-bit image with any alpha set carries its own transparency; the mask rules the
        // rest.
        let own_alpha = bits == 32 && pixels.chunks_exact(4).any(|p| p[3] != 0);
        for y in 0..h {
            // Rows are stored bottom up.
            let line = &pixels[(h - 1 - y) * row..][..row];
            let mask_line = &mask[(h - 1 - y) * mask_row..][..mask_row];
            for x in 0..w {
                let (b, g, r, a) = match bits {
                    24 => (line[x * 3], line[x * 3 + 1], line[x * 3 + 2], 255),
                    32 => (
                        line[x * 4],
                        line[x * 4 + 1],
                        line[x * 4 + 2],
                        line[x * 4 + 3],
                    ),
                    _ => {
                        let index = match bits {
                            1 => (line[x / 8] >> (7 - x % 8)) & 1,
                            4 => (line[x / 2] >> (4 - 4 * (x % 2))) & 15,
                            _ => line[x],
                        } as usize;
                        match palette.get(index * 4..index * 4 + 3) {
                            Some(c) => (c[0], c[1], c[2], 255),
                            None => (0, 0, 0, 255),
                        }
                    }
                };
                let masked = (mask_line[x / 8] >> (7 - x % 8)) & 1 != 0;
                let alpha = match (own_alpha, masked) {
                    (true, _) => a,
                    (false, true) => 0,
                    (false, false) => 255,
                };
                rgba[(y * w + x) * 4..][..4].copy_from_slice(&[r, g, b, alpha]);
            }
        }
        Ok(Cursor {
            width: width as u16,
            height: height as u16,
            hotspot,
            rgba,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 4x2 8-bit cursor: palette of black, red, green and blue; the top row is red, green,
    /// blue, black and the bottom row all red; the mask hides the top row's last pixel.
    fn sample() -> Vec<u8> {
        let mut image = Vec::new();
        image.extend(40u32.to_le_bytes());
        image.extend(4i32.to_le_bytes());
        image.extend(4i32.to_le_bytes()); // two colour rows and two mask rows
        image.extend(1u16.to_le_bytes());
        image.extend(8u16.to_le_bytes());
        image.extend([0u8; 16]); // compression, size, resolution
        image.extend(4u32.to_le_bytes()); // colours used
        image.extend(0u32.to_le_bytes());
        for bgr in [[0, 0, 0], [0, 0, 255], [0, 255, 0], [255, 0, 0]] {
            image.extend(bgr);
            image.push(0);
        }
        image.extend([1, 1, 1, 1]); // bottom row
        image.extend([1, 2, 3, 0]); // top row
        image.extend([0, 0, 0, 0]); // bottom mask row
        image.extend([0b0001_0000, 0, 0, 0]); // top mask row
        let mut file = Vec::new();
        file.extend([0, 0, 2, 0, 1, 0]);
        file.extend([4, 2, 0, 0]);
        file.extend(1u16.to_le_bytes());
        file.extend(2u16.to_le_bytes());
        file.extend((image.len() as u32).to_le_bytes());
        file.extend(22u32.to_le_bytes());
        file.extend(image);
        file
    }

    #[test]
    fn an_indexed_cursor_is_decoded_top_down_with_its_mask() {
        let c = Cursor::parse(&sample()).unwrap();
        assert_eq!((c.width, c.height, c.hotspot), (4, 2, (1, 2)));
        assert_eq!(
            c.rgba,
            [
                [255, 0, 0, 255],
                [0, 255, 0, 255],
                [0, 0, 255, 255],
                [0, 0, 0, 0],
                [255, 0, 0, 255],
                [255, 0, 0, 255],
                [255, 0, 0, 255],
                [255, 0, 0, 255],
            ]
            .concat()
        );
    }

    #[test]
    fn bad_files_are_errors_not_panics() {
        let good = sample();
        for n in 0..good.len() {
            assert!(Cursor::parse(&good[..n]).is_err(), "cut at {n}");
        }
        let mut icon = good.clone();
        icon[2] = 1;
        assert_eq!(Cursor::parse(&icon), Err(CurError::NotCursor));
        // A header that asks for a huge image, an offset past the end, odd depths.
        let mut huge = good.clone();
        huge[22 + 4..22 + 8].copy_from_slice(&100_000u32.to_le_bytes());
        assert!(matches!(
            Cursor::parse(&huge),
            Err(CurError::Unsupported(_))
        ));
        let mut far = good.clone();
        far[18..22].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(Cursor::parse(&far), Err(CurError::Truncated));
        let mut depth = good.clone();
        depth[22 + 14] = 16;
        assert!(matches!(
            Cursor::parse(&depth),
            Err(CurError::Unsupported(_))
        ));
        let mut colours = good;
        colours[22 + 32..22 + 36].copy_from_slice(&1000u32.to_le_bytes());
        assert!(matches!(
            Cursor::parse(&colours),
            Err(CurError::Unsupported(_))
        ));
    }
}
