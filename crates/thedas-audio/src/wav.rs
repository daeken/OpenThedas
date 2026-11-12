//! Writing PCM as 16-bit WAV, for offline renders and extraction.

use std::io::{self, Write};

/// Write interleaved `f32` samples (clamped to -1..=1) as a 16-bit PCM WAV.
pub fn write_wav<W: Write>(mut w: W, rate: u32, channels: u16, samples: &[f32]) -> io::Result<()> {
    let data_len = (samples.len() * 2) as u32;
    let block = channels as u32 * 2;
    w.write_all(b"RIFF")?;
    w.write_all(&(36 + data_len).to_le_bytes())?;
    w.write_all(b"WAVEfmt ")?;
    w.write_all(&16u32.to_le_bytes())?;
    w.write_all(&1u16.to_le_bytes())?;
    w.write_all(&channels.to_le_bytes())?;
    w.write_all(&rate.to_le_bytes())?;
    w.write_all(&(rate * block).to_le_bytes())?;
    w.write_all(&(block as u16).to_le_bytes())?;
    w.write_all(&16u16.to_le_bytes())?;
    w.write_all(b"data")?;
    w.write_all(&data_len.to_le_bytes())?;
    let mut buf = Vec::with_capacity(samples.len() * 2);
    for s in samples {
        buf.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0).round() as i16).to_le_bytes());
    }
    w.write_all(&buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_a_canonical_header() {
        let mut v = Vec::new();
        write_wav(&mut v, 48000, 2, &[0.0, 1.0, -1.0, 2.0]).unwrap();
        assert_eq!(v.len(), 44 + 8);
        assert_eq!(&v[..4], b"RIFF");
        assert_eq!(&v[44..], &[0, 0, 0xff, 0x7f, 0x01, 0x80, 0xff, 0x7f]);
    }
}
