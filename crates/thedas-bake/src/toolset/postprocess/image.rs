use std::{fs, path::Path};

use anyhow::{Context, Result, ensure};
use thedas_formats::dds::{Dds, DdsKind};

#[derive(Clone)]
pub(super) struct Image {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<[u8; 4]>,
}

impl Image {
    pub fn new(width: u32, height: u32, pixels: Vec<[u8; 4]>) -> Result<Self> {
        ensure!(
            pixels.len() == pixel_count(width, height)?,
            "image has the wrong pixel count"
        );
        Ok(Self {
            width,
            height,
            pixels,
        })
    }

    pub fn read_tga(path: &Path) -> Result<Self> {
        let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        let header = bytes.get(..18).context("TGA header is truncated")?;
        let width = u32::from(u16::from_le_bytes([header[12], header[13]]));
        let height = u32::from(u16::from_le_bytes([header[14], header[15]]));
        let count = pixel_count(width, height)?;
        let bpp = match header[16] {
            24 => 3,
            32 => 4,
            value => anyhow::bail!("unsupported TGA pixel depth {value}"),
        };
        ensure!(
            header[1] == 0 && header[2] == 2 && header[17] & 0xc0 == 0,
            "expected an uncompressed true-color TGA"
        );
        let start = 18 + usize::from(header[0]);
        let pixels = bytes
            .get(start..start + count * bpp)
            .context("TGA pixel data is truncated")?;
        let mut rgba: Vec<[u8; 4]> = pixels
            .chunks_exact(bpp)
            .map(|bgr| [bgr[2], bgr[1], bgr[0], bgr.get(3).copied().unwrap_or(255)])
            .collect();
        if header[17] & 0x20 == 0 {
            rgba.reverse();
            for row in rgba.chunks_exact_mut(width as usize) {
                row.reverse();
            }
        }
        if header[17] & 0x10 != 0 {
            for row in rgba.chunks_exact_mut(width as usize) {
                row.reverse();
            }
        }
        Self::new(width, height, rgba)
    }

    pub fn read_dds(path: &Path) -> Result<Self> {
        let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        let dds = Dds::parse(&bytes).with_context(|| format!("decoding {}", path.display()))?;
        ensure!(dds.kind == DdsKind::Texture2D, "expected a 2D DDS");
        let rgba = dds.decode_rgba8(0, 0)?;
        let pixels = rgba
            .chunks_exact(4)
            .map(|pixel| [pixel[0], pixel[1], pixel[2], pixel[3]])
            .collect();
        Self::new(dds.width, dds.height, pixels)
    }

    pub fn write_bgra_dds(&self, path: &Path, atlas: bool) -> Result<()> {
        let data = self.pixels.iter().flat_map(|[r, g, b, a]| [*b, *g, *r, *a]);
        // BakedMapProcessor writes DDSD_LINEARSIZE and the whole surface size
        // even for an uncompressed image.
        let mut header = dds_header(self.width, self.height, (self.pixels.len() * 4) as u32);
        if atlas {
            // D3DX's atlas writer omits pitch/linear size and adds DDSCAPS_1.
            header[2] = 0x1007;
            header[5] = 0;
            header[27] = 0x1002;
        }
        for (index, value) in [
            (20, 0x41),
            (22, 32),
            (23, 0x00ff_0000),
            (24, 0x0000_ff00),
            (25, 0x0000_00ff),
            (26, 0xff00_0000),
        ] {
            header[index] = value;
        }
        write_dds(path, header, data)
    }

    pub fn write_bc3_dds(&self, path: &Path) -> Result<()> {
        // TextureProcessor rounds each dimension up to a full DXT block at
        // RVA 0x8f70, then samples the source at the destination texel centers.
        let width = self.width.next_multiple_of(4);
        let height = self.height.next_multiple_of(4);
        let row_blocks = width as usize / 4;
        let column_blocks = height as usize / 4;
        let sample = |coordinate, source: u32, output: u32| {
            (2 * coordinate + 1) * source as usize / (2 * output as usize)
        };
        let mut data = Vec::with_capacity(row_blocks * column_blocks * 16);
        for block_y in 0..column_blocks {
            for block_x in 0..row_blocks {
                let texels = std::array::from_fn(|texel| {
                    let x = sample(4 * block_x + texel % 4, self.width, width);
                    let y = sample(4 * block_y + texel / 4, self.height, height);
                    self.pixels[y * self.width as usize + x]
                });
                data.extend(super::bc3_alpha::encode(texels.map(|p| Some(p[3]))));
                data.extend(super::bc3_rgb::encode(
                    texels.map(|[r, g, b, _]| Some([r, g, b])),
                ));
            }
        }
        let mut header = dds_header(width, height, data.len() as u32);
        header[20] = 4;
        header[21] = u32::from_le_bytes(*b"DXT5");
        write_dds(path, header, data)
    }

    /// Downsample with separable area weights. Preserve thin alpha/RGB
    /// details when the source is an odd size or the cap differs per axis.
    pub fn resize_box(&self, width: u32, height: u32) -> Result<Self> {
        ensure!(
            width <= self.width && height <= self.height,
            "box upscale is unsupported"
        );
        let count = pixel_count(width, height)?;
        if (width, height) == (self.width, self.height) {
            return Ok(self.clone());
        }
        let mut pixels = Vec::with_capacity(count);
        let total = u64::from(self.width) * u64::from(self.height);
        for y in 0..height {
            let y0 = y * self.height;
            let y1 = y0 + self.height;
            for x in 0..width {
                let x0 = x * self.width;
                let x1 = x0 + self.width;
                let mut channels = [0_u64; 4];
                for sy in y0 / height..y1.div_ceil(height) {
                    let wy = y1.min((sy + 1) * height) - y0.max(sy * height);
                    for sx in x0 / width..x1.div_ceil(width) {
                        let wx = x1.min((sx + 1) * width) - x0.max(sx * width);
                        let pixel = self.pixels[(sy * self.width + sx) as usize];
                        let weight = u64::from(wx) * u64::from(wy);
                        for (sum, value) in channels.iter_mut().zip(pixel) {
                            *sum += u64::from(value) * weight;
                        }
                    }
                }
                pixels.push(channels.map(|sum| ((sum + total / 2) / total) as u8));
            }
        }
        Self::new(width, height, pixels)
    }
}

pub(super) fn pixel_count(width: u32, height: u32) -> Result<usize> {
    ensure!(
        (1..=16384).contains(&width) && (1..=16384).contains(&height),
        "unsupported image dimensions {width}x{height}"
    );
    Ok((width * height) as usize)
}

fn dds_header(width: u32, height: u32, size: u32) -> [u32; 32] {
    let mut header = [0; 32];
    header[..6].copy_from_slice(&[
        u32::from_le_bytes(*b"DDS "),
        124,
        0x0008_1007,
        height,
        width,
        size,
    ]);
    header[19] = 32;
    header[27] = 0x1000; // DDSCAPS_TEXTURE
    header
}

fn write_dds(path: &Path, header: [u32; 32], data: impl IntoIterator<Item = u8>) -> Result<()> {
    let data = data.into_iter();
    let mut bytes = Vec::with_capacity(128 + data.size_hint().0);
    bytes.extend(header.into_iter().flat_map(u32::to_le_bytes));
    bytes.extend(data);
    fs::write(path, bytes).with_context(|| format!("writing {}", path.display()))
}
