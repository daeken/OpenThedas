//! FMOD sound banks (`.fsb`, version 4): the waveforms the game's FMOD event files (`.fev`)
//! play. Every bank under `audio/sound` and `audio/vo` is FSB4 (header version `0x40000`).
//!
//! Layout: a 48-byte header (`FSB4`, sample count, sample-header bytes, data bytes, version,
//! flags, 8 reserved bytes, a 16-byte hash), the sample headers, then each sample's data back
//! to back in header order. A sample header is 80 bytes plus optional extra data; DA:O's
//! streamed music carries `SYNC` points there (a count and that many sample offsets: the
//! places FMOD's music system may cut to another cue).
//!
//! Only two codecs occur in the shipped data (every bank of the install was scanned): FMOD's
//! IMA ADPCM (Xbox layout, `MODE_IMAADPCM`) and MPEG layer 3 (`MODE_MPEG`). Decoding lives
//! in `thedas-audio`.
//!
//! Provisional: field names follow FMOD Ex 4's public `fsbank` header layout as documented by
//! the community (vgmstream, fsbext); they were checked against the data (sizes add up to the
//! data length, IMA sample counts match 64 samples per 36-byte block) but not against
//! `fmodex.dll`.

use std::ops::Range;

use thiserror::Error;

/// Header flag: sample headers after the first are 8 bytes (lengths only).
pub const FLAG_BASIC_HEADERS: u32 = 0x2;

pub const MODE_LOOP_OFF: u32 = 0x1;
pub const MODE_LOOP_NORMAL: u32 = 0x2;
pub const MODE_LOOP_BIDI: u32 = 0x4;
pub const MODE_8BITS: u32 = 0x8;
pub const MODE_16BITS: u32 = 0x10;
pub const MODE_MONO: u32 = 0x20;
pub const MODE_STEREO: u32 = 0x40;
pub const MODE_MPEG: u32 = 0x200;
pub const MODE_2D: u32 = 0x2000;
pub const MODE_STREAMABLE: u32 = 0x4000;
pub const MODE_IMAADPCM: u32 = 0x40_0000;
pub const MODE_VAG: u32 = 0x80_0000;
pub const MODE_XMA: u32 = 0x100_0000;
pub const MODE_GCADPCM: u32 = 0x200_0000;

const HEADER_LEN: usize = 48;
const SAMPLE_HEADER_LEN: usize = 80;

#[derive(Debug, Error)]
pub enum FsbError {
    #[error("not an FSB4 bank (magic {0:?})")]
    Magic([u8; 4]),
    #[error("FSB truncated: {what} needs bytes {need:?}, have {have}")]
    Truncated {
        what: &'static str,
        need: Range<usize>,
        have: usize,
    },
    #[error("FSB sample {index}: header size {size} is smaller than its fields")]
    SampleHeader { index: usize, size: usize },
}

pub type FsbResult<T> = Result<T, FsbError>;

/// The sample's encoding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    /// FMOD IMA ADPCM: 36-byte blocks of 64 samples per channel (stereo interleaves the two
    /// channels' blocks in 4-byte words after both 4-byte block headers).
    ImaAdpcm,
    /// MPEG audio (layer 3) frames, back to back.
    Mpeg,
    /// Little-endian signed 16-bit PCM.
    Pcm16,
    /// Signed 8-bit PCM.
    Pcm8,
    /// Something DA:O does not ship (VAG, XMA, GameCube ADPCM).
    Other,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FsbSample {
    /// The source file name (usually ending `.wav`; FMOD keeps at most 29 characters).
    pub name: String,
    /// Length in sample frames.
    pub length: u32,
    /// Bytes of encoded data.
    pub compressed_size: u32,
    pub loop_start: u32,
    pub loop_end: u32,
    pub mode: u32,
    /// Default playback rate in Hz.
    pub frequency: u32,
    /// Default volume, 0..=255.
    pub volume: u16,
    /// Default pan, 0 (left) ..= 255 (right); 128 is centre.
    pub pan: i16,
    pub priority: u16,
    pub channels: u16,
    pub min_distance: f32,
    pub max_distance: f32,
    /// `SYNC` points: sample offsets in the extra header data.
    pub sync_points: Vec<u32>,
    /// Where the data lies, from the start of the file.
    pub data: Range<usize>,
}

impl FsbSample {
    pub fn codec(&self) -> Codec {
        if self.mode & MODE_IMAADPCM != 0 {
            Codec::ImaAdpcm
        } else if self.mode & MODE_MPEG != 0 {
            Codec::Mpeg
        } else if self.mode & (MODE_VAG | MODE_XMA | MODE_GCADPCM) != 0 {
            Codec::Other
        } else if self.mode & MODE_8BITS != 0 {
            Codec::Pcm8
        } else {
            Codec::Pcm16
        }
    }

    /// Whether the sample's own mode asks for a loop.
    pub fn loops(&self) -> bool {
        self.mode & (MODE_LOOP_NORMAL | MODE_LOOP_BIDI) != 0
    }

    /// The name without its `.wav` (or other) extension, which is how `.fev` files refer to it.
    pub fn stem(&self) -> &str {
        match self.name.rfind('.') {
            Some(i) => &self.name[..i],
            None => &self.name,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Fsb {
    pub flags: u32,
    pub version: u32,
    pub samples: Vec<FsbSample>,
}

fn slice<'a>(data: &'a [u8], at: usize, len: usize, what: &'static str) -> FsbResult<&'a [u8]> {
    data.get(at..at.saturating_add(len))
        .filter(|s| s.len() == len)
        .ok_or(FsbError::Truncated {
            what,
            need: at..at.saturating_add(len),
            have: data.len(),
        })
}

fn u16_at(d: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([d[at], d[at + 1]])
}
fn u32_at(d: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([d[at], d[at + 1], d[at + 2], d[at + 3]])
}

impl Fsb {
    /// How many leading bytes [`Fsb::parse`] needs: the header and sample headers. Large banks
    /// (the streamed music is 300 MB) can be read this far and their sample data fetched later.
    pub fn header_len(head: &[u8]) -> FsbResult<usize> {
        let h = slice(head, 0, HEADER_LEN, "header")?;
        if &h[..4] != b"FSB4" {
            return Err(FsbError::Magic([h[0], h[1], h[2], h[3]]));
        }
        Ok(HEADER_LEN + u32_at(h, 8) as usize)
    }

    /// Parse the headers. `data` needs at least [`Fsb::header_len`] bytes; the sample data
    /// need not be present (each [`FsbSample::data`] says where it is).
    pub fn parse(data: &[u8]) -> FsbResult<Fsb> {
        let h = slice(data, 0, HEADER_LEN, "header")?;
        if &h[..4] != b"FSB4" {
            return Err(FsbError::Magic([h[0], h[1], h[2], h[3]]));
        }
        let count = u32_at(h, 4) as usize;
        let headers_len = u32_at(h, 8) as usize;
        let version = u32_at(h, 16);
        let flags = u32_at(h, 20);
        let headers = slice(data, HEADER_LEN, headers_len, "sample headers")?;
        let mut samples: Vec<FsbSample> = Vec::with_capacity(count.min(1 << 16));
        let mut at = 0usize;
        let mut data_at = HEADER_LEN + headers_len;
        for index in 0..count {
            let s = if flags & FLAG_BASIC_HEADERS != 0 && index > 0 {
                let b = slice(headers, at, 8, "basic sample header")?;
                at += 8;
                let mut s = samples[0].clone();
                s.length = u32_at(b, 0);
                s.compressed_size = u32_at(b, 4);
                s.sync_points.clear();
                s
            } else {
                let size = u16_at(slice(headers, at, 2, "sample header")?, 0) as usize;
                if size < SAMPLE_HEADER_LEN {
                    return Err(FsbError::SampleHeader { index, size });
                }
                let b = slice(headers, at, size, "sample header")?;
                at += size;
                let name_bytes = &b[2..32];
                let end = name_bytes.iter().position(|&c| c == 0).unwrap_or(30);
                let name = String::from_utf8_lossy(&name_bytes[..end]).into_owned();
                let extra = &b[SAMPLE_HEADER_LEN..];
                let mut sync_points = Vec::new();
                if extra.len() >= 8 && &extra[..4] == b"SYNC" {
                    let n = u32_at(extra, 4) as usize;
                    for i in 0..n {
                        let o = 8 + i * 4;
                        if o + 4 > extra.len() {
                            break;
                        }
                        sync_points.push(u32_at(extra, o));
                    }
                }
                FsbSample {
                    name,
                    length: u32_at(b, 32),
                    compressed_size: u32_at(b, 36),
                    loop_start: u32_at(b, 40),
                    loop_end: u32_at(b, 44),
                    mode: u32_at(b, 48),
                    frequency: u32_at(b, 52),
                    volume: u16_at(b, 56),
                    pan: u16_at(b, 58) as i16,
                    priority: u16_at(b, 60),
                    channels: u16_at(b, 62),
                    min_distance: f32::from_bits(u32_at(b, 64)),
                    max_distance: f32::from_bits(u32_at(b, 68)),
                    sync_points,
                    data: 0..0,
                }
            };
            let len = s.compressed_size as usize;
            let s = FsbSample {
                data: data_at..data_at.saturating_add(len),
                ..s
            };
            data_at = s.data.end;
            samples.push(s);
        }
        Ok(Fsb {
            flags,
            version,
            samples,
        })
    }

    /// The sample whose name (or name without extension) is `name`, ignoring case.
    pub fn find(&self, name: &str) -> Option<(usize, &FsbSample)> {
        self.samples
            .iter()
            .enumerate()
            .find(|(_, s)| s.name.eq_ignore_ascii_case(name) || s.stem().eq_ignore_ascii_case(name))
    }

    /// A sample's encoded bytes out of the whole file.
    pub fn sample_data<'a>(&self, file: &'a [u8], index: usize) -> FsbResult<&'a [u8]> {
        let s = &self.samples[index];
        slice(file, s.data.start, s.data.len(), "sample data")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_header(name: &str, len: u32, comp: u32, mode: u32, extra: &[u8]) -> Vec<u8> {
        let mut b = vec![0u8; SAMPLE_HEADER_LEN];
        b[..2].copy_from_slice(&((SAMPLE_HEADER_LEN + extra.len()) as u16).to_le_bytes());
        b[2..2 + name.len()].copy_from_slice(name.as_bytes());
        b[32..36].copy_from_slice(&len.to_le_bytes());
        b[36..40].copy_from_slice(&comp.to_le_bytes());
        b[44..48].copy_from_slice(&(len - 1).to_le_bytes());
        b[48..52].copy_from_slice(&mode.to_le_bytes());
        b[52..56].copy_from_slice(&22050u32.to_le_bytes());
        b[56..58].copy_from_slice(&255u16.to_le_bytes());
        b[58..60].copy_from_slice(&128u16.to_le_bytes());
        b[62..64].copy_from_slice(&1u16.to_le_bytes());
        b[64..68].copy_from_slice(&1.0f32.to_le_bytes());
        b[68..72].copy_from_slice(&10000.0f32.to_le_bytes());
        b.extend_from_slice(extra);
        b
    }

    fn bank() -> Vec<u8> {
        let mut sync = b"SYNC".to_vec();
        sync.extend_from_slice(&2u32.to_le_bytes());
        sync.extend_from_slice(&100u32.to_le_bytes());
        sync.extend_from_slice(&200u32.to_le_bytes());
        let mut headers = sample_header("a.wav", 64, 36, MODE_IMAADPCM | MODE_MONO, &[]);
        headers.extend(sample_header("music_b.wav", 1152, 64, MODE_MPEG, &sync));
        let mut f = b"FSB4".to_vec();
        f.extend_from_slice(&2u32.to_le_bytes());
        f.extend_from_slice(&(headers.len() as u32).to_le_bytes());
        f.extend_from_slice(&100u32.to_le_bytes());
        f.extend_from_slice(&0x40000u32.to_le_bytes());
        f.extend_from_slice(&0x20u32.to_le_bytes());
        f.extend_from_slice(&[0; 24]);
        f.extend(headers);
        f.extend(std::iter::repeat_n(7u8, 100));
        f
    }

    #[test]
    fn parses_headers_and_data_ranges() {
        let f = bank();
        let fsb = Fsb::parse(&f).unwrap();
        assert_eq!(fsb.samples.len(), 2);
        let a = &fsb.samples[0];
        assert_eq!(a.name, "a.wav");
        assert_eq!(a.stem(), "a");
        assert_eq!(a.codec(), Codec::ImaAdpcm);
        assert_eq!(a.frequency, 22050);
        assert_eq!(a.max_distance, 10000.0);
        let b = &fsb.samples[1];
        assert_eq!(b.codec(), Codec::Mpeg);
        assert_eq!(b.sync_points, vec![100, 200]);
        assert_eq!(b.data.start, a.data.end);
        assert_eq!(fsb.sample_data(&f, 1).unwrap().len(), 64);
        assert_eq!(fsb.find("MUSIC_B").unwrap().0, 1);
        assert_eq!(Fsb::header_len(&f).unwrap(), b.data.start - 36);
    }

    #[test]
    fn rejects_bad_input_without_panicking() {
        let f = bank();
        for cut in [0, 10, 47, 60, 200] {
            assert!(Fsb::parse(&f[..cut]).is_err());
        }
        assert!(matches!(
            Fsb::parse(b"RIFF0000"),
            Err(FsbError::Truncated { .. })
        ));
        let mut bad = f.clone();
        bad[..4].copy_from_slice(b"FSB3");
        assert!(matches!(Fsb::parse(&bad), Err(FsbError::Magic(_))));
        // Headers present but data cut: parse succeeds, data access fails.
        let fsb = Fsb::parse(&f[..f.len() - 50]).unwrap();
        assert!(fsb.sample_data(&f[..f.len() - 50], 1).is_err());
    }
}
