//! Bink 1 movies (`.bik`): the container, the video codec and the audio codec.
//!
//! Every movie of DA:O and DA2 is revision `i` with no alpha plane, and its sound (when it
//! has any) is one stereo track of Bink audio in its DCT form; those are what this decodes.
//!
//! The container: a 44-byte header (`BIK`, the revision, then little-endian u32s: the size
//! after the first 8 bytes, the frame count, the largest frame, the frame count again, the
//! width, the height, the frame rate as a fraction, the video flags and the audio track
//! count), per audio track a u32 (largest decoded packet), then per track its sample rate
//! (u16) and flags (u16), then per track an id (u32), then one u32 offset per frame plus
//! one for the end (bit 0 marks a keyframe). A frame holds, per audio track, a u32 length
//! and that many bytes of audio, then the video packet in the rest.

mod audio;
mod bits;
mod tables;
mod video;

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

pub use audio::AudioDecoder;
pub use video::{Frame, Plane, VideoDecoder};

/// Why a movie could not be read or decoded.
#[derive(Debug, thiserror::Error)]
pub enum BinkError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("not a Bink movie")]
    NotBink,
    #[error("unsupported: {0}")]
    Unsupported(&'static str),
    #[error("bad header: {0}")]
    BadHeader(&'static str),
    #[error("bad frame: {0}")]
    BadFrame(&'static str),
    #[error("frame {0} is past the end")]
    NoSuchFrame(u32),
}

pub type Result<T> = std::result::Result<T, BinkError>;

/// An audio track's description in the header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioTrack {
    pub rate: u32,
    pub flags: u16,
    pub id: u32,
}

impl AudioTrack {
    pub fn channels(&self) -> u16 {
        if self.flags & 0x2000 != 0 { 2 } else { 1 }
    }

    /// Whether the track uses the DCT form of Bink audio (the other uses a real FFT).
    pub fn dct(&self) -> bool {
        self.flags & 0x1000 != 0
    }
}

/// What a movie's header says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Header {
    /// The revision letter (`b'i'`).
    pub revision: u8,
    pub frames: u32,
    pub largest_frame: u32,
    pub width: u32,
    pub height: u32,
    pub fps_num: u32,
    pub fps_den: u32,
    pub flags: u32,
    pub tracks: Vec<AudioTrack>,
}

impl Header {
    /// Bytes the header and its audio track descriptions take (not the frame index).
    fn len(tracks: usize) -> usize {
        44 + tracks * 12
    }

    /// Reads the header at the start of a `.bik`.
    pub fn parse(b: &[u8]) -> Result<Self> {
        if b.len() < 44 || &b[..3] != b"BIK" {
            return Err(BinkError::NotBink);
        }
        let u = |o: usize| -> Result<u32> {
            b.get(o..o + 4)
                .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
                .ok_or(BinkError::BadHeader("short"))
        };
        let ntracks = u(40)? as usize;
        if ntracks > 256 {
            return Err(BinkError::BadHeader("too many audio tracks"));
        }
        let tracks = (0..ntracks)
            .map(|t| {
                let o = 44 + ntracks * 4 + t * 4;
                let r = u(o)?;
                Ok(AudioTrack {
                    rate: r & 0xffff,
                    flags: (r >> 16) as u16,
                    id: u(44 + ntracks * 8 + t * 4)?,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let h = Self {
            revision: b[3],
            frames: u(8)?,
            largest_frame: u(12)?,
            width: u(20)?,
            height: u(24)?,
            fps_num: u(28)?,
            fps_den: u(32)?.max(1),
            flags: u(36)?,
            tracks,
        };
        if h.width == 0 || h.height == 0 || h.width > 8192 || h.height > 8192 {
            return Err(BinkError::BadHeader("size"));
        }
        if h.fps_num == 0 {
            return Err(BinkError::BadHeader("frame rate"));
        }
        if h.frames > 1 << 24 {
            return Err(BinkError::BadHeader("frame count"));
        }
        Ok(h)
    }

    pub fn fps(&self) -> f64 {
        self.fps_num as f64 / self.fps_den as f64
    }
}

/// A frame's packets, as stored.
#[derive(Clone, Debug, Default)]
pub struct Packets {
    /// Per audio track: its packet (empty when the frame has no audio for it).
    pub audio: Vec<Vec<u8>>,
    pub video: Vec<u8>,
    pub keyframe: bool,
}

/// A movie file, read a frame at a time.
pub struct Movie<R> {
    reader: R,
    pub header: Header,
    /// Each frame's start and whether it is a keyframe, then the end.
    index: Vec<(u64, bool)>,
}

impl Movie<std::io::BufReader<std::fs::File>> {
    pub fn open(path: &Path) -> Result<Self> {
        Self::new(std::io::BufReader::new(std::fs::File::open(path)?))
    }
}

impl<R: Read + Seek> Movie<R> {
    pub fn new(mut reader: R) -> Result<Self> {
        let file_len = reader.seek(SeekFrom::End(0))?;
        reader.seek(SeekFrom::Start(0))?;
        let mut head = vec![0u8; 44];
        reader.read_exact(&mut head)?;
        let ntracks = u32::from_le_bytes([head[40], head[41], head[42], head[43]]) as usize;
        if &head[..3] != b"BIK" {
            return Err(BinkError::NotBink);
        }
        if ntracks > 256 {
            return Err(BinkError::BadHeader("too many audio tracks"));
        }
        head.resize(Header::len(ntracks), 0);
        reader.read_exact(&mut head[44..])?;
        let header = Header::parse(&head)?;
        let n = header.frames as usize;
        let index_bytes = (n + 1) * 4;
        if (head.len() + index_bytes) as u64 > file_len {
            return Err(BinkError::BadHeader("frame index past the end"));
        }
        let mut raw = vec![0u8; index_bytes];
        reader.read_exact(&mut raw)?;
        let mut index: Vec<(u64, bool)> = raw
            .chunks_exact(4)
            .map(|c| {
                let v = u32::from_le_bytes([c[0], c[1], c[2], c[3]]);
                ((v & !1) as u64, v & 1 != 0)
            })
            .collect();
        // The last entry is the end; some writers leave it out (or zero), so the file's
        // length stands in.
        if let Some(last) = index.last_mut()
            && (last.0 == 0 || last.0 > file_len)
        {
            last.0 = file_len;
        }
        let data_start = (head.len() + index_bytes) as u64;
        if index
            .windows(2)
            .any(|w| w[1].0 < w[0].0 || w[0].0 < data_start)
        {
            return Err(BinkError::BadHeader("frame index out of order"));
        }
        Ok(Self {
            reader,
            header,
            index,
        })
    }

    pub fn frames(&self) -> u32 {
        self.header.frames
    }

    /// Reads frame `i`'s packets.
    pub fn packets(&mut self, i: u32) -> Result<Packets> {
        let (Some(&(start, keyframe)), Some(&(end, _))) =
            (self.index.get(i as usize), self.index.get(i as usize + 1))
        else {
            return Err(BinkError::NoSuchFrame(i));
        };
        let len = (end - start) as usize;
        if len > 64 << 20 {
            return Err(BinkError::BadFrame("larger than 64 MB"));
        }
        let mut buf = vec![0u8; len];
        self.reader.seek(SeekFrom::Start(start))?;
        self.reader.read_exact(&mut buf)?;
        let mut at = 0usize;
        let mut audio = Vec::with_capacity(self.header.tracks.len());
        for _ in &self.header.tracks {
            let size = buf
                .get(at..at + 4)
                .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]) as usize)
                .ok_or(BinkError::BadFrame("audio length past the end"))?;
            at += 4;
            let pkt = buf
                .get(at..at + size)
                .ok_or(BinkError::BadFrame("audio packet past the end"))?;
            audio.push(pkt.to_vec());
            at += size;
        }
        buf.drain(..at);
        Ok(Packets {
            audio,
            video: buf,
            keyframe,
        })
    }

    /// Reads frame `i`'s packet for audio track `track` alone (empty when the frame has no
    /// audio for it), without reading the video.
    pub fn audio_packet(&mut self, i: u32, track: usize) -> Result<Vec<u8>> {
        let (Some(&(start, _)), Some(&(end, _))) =
            (self.index.get(i as usize), self.index.get(i as usize + 1))
        else {
            return Err(BinkError::NoSuchFrame(i));
        };
        if track >= self.header.tracks.len() {
            return Err(BinkError::BadFrame("no such audio track"));
        }
        self.reader.seek(SeekFrom::Start(start))?;
        let mut at = start;
        for t in 0..=track {
            let mut size = [0u8; 4];
            if at + 4 > end {
                return Err(BinkError::BadFrame("audio length past the end"));
            }
            self.reader.read_exact(&mut size)?;
            let size = u32::from_le_bytes(size) as u64;
            at += 4;
            if at + size > end {
                return Err(BinkError::BadFrame("audio packet past the end"));
            }
            if t == track {
                let mut buf = vec![0u8; size as usize];
                self.reader.read_exact(&mut buf)?;
                return Ok(buf);
            }
            self.reader.seek(SeekFrom::Current(size as i64))?;
            at += size;
        }
        Ok(Vec::new())
    }

    /// Decodes audio track `track` whole: its samples, interleaved.
    pub fn decode_audio(&mut self, track: usize) -> Result<Vec<f32>> {
        let t = *self
            .header
            .tracks
            .get(track)
            .ok_or(BinkError::BadFrame("no such audio track"))?;
        let mut dec = AudioDecoder::new(&t)?;
        let mut out = Vec::new();
        for i in 0..self.frames() {
            let p = self.audio_packet(i, track)?;
            dec.decode(&p, &mut out)?;
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A made-up movie: `frames` frames of one stereo DCT track, packets of `a` audio bytes
    /// and `v` video bytes.
    pub(crate) fn synthetic_movie(frames: u32, a: usize, v: usize) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend(b"BIKi");
        for x in [0, frames, 100, frames, 64, 32, 30, 1, 0, 1] {
            b.extend(x.to_le_bytes());
        }
        b.extend(0u32.to_le_bytes());
        b.extend((48000u32 | 0x3000 << 16).to_le_bytes());
        b.extend(7u32.to_le_bytes());
        let start = b.len() + (frames as usize + 1) * 4;
        let frame_len = 4 + a + v;
        for f in 0..=frames as usize {
            let key = (f == 0) as u32;
            b.extend(((start + f * frame_len) as u32 | key).to_le_bytes());
        }
        for f in 0..frames as usize {
            b.extend((a as u32).to_le_bytes());
            b.extend(std::iter::repeat_n(f as u8, a));
            b.extend(std::iter::repeat_n(0xa0 | f as u8, v));
        }
        let size = (b.len() - 8) as u32;
        b[4..8].copy_from_slice(&size.to_le_bytes());
        b
    }

    #[test]
    fn container_splits_frames_into_audio_and_video() {
        let bytes = synthetic_movie(3, 5, 9);
        let mut m = Movie::new(std::io::Cursor::new(bytes.clone())).expect("parses");
        assert_eq!(m.header.width, 64);
        assert_eq!(m.header.tracks.len(), 1);
        assert_eq!(m.header.tracks[0].rate, 48000);
        assert_eq!(m.header.tracks[0].channels(), 2);
        assert!(m.header.tracks[0].dct());
        assert_eq!(m.header.tracks[0].id, 7);
        let p = m.packets(2).expect("frame 2");
        assert_eq!(p.audio, vec![vec![2u8; 5]]);
        assert_eq!(p.video, vec![0xa2; 9]);
        assert!(!p.keyframe);
        assert!(m.packets(0).expect("frame 0").keyframe);
        assert!(matches!(m.packets(3), Err(BinkError::NoSuchFrame(3))));
        assert_eq!(m.audio_packet(1, 0).expect("audio alone"), vec![1u8; 5]);
        assert!(m.audio_packet(1, 1).is_err());
        // Truncated anywhere: an error, never a panic.
        for cut in 0..bytes.len() {
            if let Ok(mut m) = Movie::new(std::io::Cursor::new(bytes[..cut].to_vec())) {
                for f in 0..m.frames() {
                    let _ = m.packets(f);
                }
            }
        }
    }
}
