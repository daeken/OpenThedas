//! Bink audio, DCT form: blocks of transform coefficients coded per critical band with a
//! band quantiser, run-length grouped widths and a sign per coefficient, overlapped with
//! the block before by a sixteenth of a block.
//!
//! Following the description at <https://wiki.multimedia.cx/index.php/Bink_Audio>. A
//! packet (after its u32 count of decoded bytes) is a run of blocks, each 32-bit aligned.
//! Provisional where it is not checked against the original decoder: the output was
//! compared with ffmpeg's for every movie both games ship.

use super::bits::Bits;
use super::{AudioTrack, BinkError, Result};

/// The critical band edges, in Hz.
const CRITICAL_FREQS: [u32; 25] = [
    100, 200, 300, 400, 510, 630, 770, 920, 1080, 1270, 1480, 1720, 2000, 2320, 2700, 3150, 3700,
    4400, 5300, 6400, 7700, 9500, 12000, 15500, 24500,
];

/// Run lengths (in groups of eight coefficients) a run code stands for.
const RLE: [usize; 16] = [2, 3, 4, 5, 6, 8, 9, 10, 11, 12, 13, 14, 15, 16, 32, 64];

/// Decodes one audio track, packet by packet.
pub struct AudioDecoder {
    channels: usize,
    /// Coefficients per block and channel.
    len: usize,
    overlap: usize,
    /// Band edges, in coefficients.
    bands: Vec<usize>,
    /// The band quantisers by their 0..96 index.
    quant: [f32; 96],
    /// Scales the coded floats to the transform's input.
    root: f32,
    /// Each channel's tail of the last block, which the next block's start fades from.
    previous: Vec<Vec<f32>>,
    first: bool,
    dct: Dct,
    /// Scratch: one block's coefficients per channel.
    coefs: Vec<Vec<f32>>,
}

impl AudioDecoder {
    pub fn new(track: &AudioTrack) -> Result<Self> {
        if !track.dct() {
            return Err(BinkError::Unsupported("Bink audio in its FFT form"));
        }
        if track.rate == 0 {
            return Err(BinkError::BadHeader("audio rate"));
        }
        let channels = track.channels() as usize;
        let rate = track.rate;
        let bits = if rate < 22050 {
            9
        } else if rate < 44100 {
            10
        } else {
            11
        };
        let len = 1usize << bits;
        let half = rate.div_ceil(2) as u64;
        let nbands = (1..25)
            .find(|&n| half <= CRITICAL_FREQS[n - 1] as u64)
            .unwrap_or(25);
        let mut bands = vec![2usize];
        for f in &CRITICAL_FREQS[..nbands - 1] {
            bands.push(((*f as u64 * len as u64 / half) as usize) & !1);
        }
        bands.push(len);
        let root = 2.0 / ((len as f32).sqrt() * 32768.0);
        Ok(Self {
            channels,
            len,
            overlap: len / 16,
            bands,
            quant: std::array::from_fn(|i| (i as f32 * 0.152_891_65).exp() * root),
            root,
            previous: vec![vec![0.0; len / 16]; channels],
            first: true,
            dct: Dct::new(len),
            coefs: vec![vec![0.0; len]; channels],
        })
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Decodes a packet, appending its samples (interleaved, nominally -1..1) to `out`.
    pub fn decode(&mut self, packet: &[u8], out: &mut Vec<f32>) -> Result<()> {
        if packet.len() <= 4 {
            return Ok(());
        }
        let mut bits = Bits::new(packet);
        bits.skip(32);
        while bits.left() > 0 {
            self.block(&mut bits)?;
            let n = self.len - self.overlap;
            out.reserve(n * self.channels);
            for i in 0..n {
                for c in &self.coefs {
                    out.push(c[i]);
                }
            }
            bits.align32();
        }
        Ok(())
    }

    /// Decodes one block into `self.coefs` (as samples), fading its start from the last.
    fn block(&mut self, bits: &mut Bits) -> Result<()> {
        let len = self.len;
        bits.skip(2);
        for ch in 0..self.channels {
            let c = &mut self.coefs[ch];
            c[0] = float(bits) * self.root;
            c[1] = float(bits) * self.root;
            let mut q = [0f32; 25];
            for v in q.iter_mut().take(self.bands.len() - 1) {
                *v = self.quant[(bits.read(8) as usize).min(95)];
            }
            let mut k = 0usize;
            let mut qv = q[0];
            let mut i = 2usize;
            while i < len {
                let j = if bits.bit() {
                    i + RLE[bits.read(4) as usize] * 8
                } else {
                    i + 8
                }
                .min(len);
                let width = bits.read(4);
                if width == 0 {
                    c[i..j].fill(0.0);
                    i = j;
                    while k < self.bands.len() && self.bands[k] < i {
                        qv = q[k.min(24)];
                        k += 1;
                    }
                } else {
                    while i < j {
                        if k < self.bands.len() && self.bands[k] == i {
                            qv = q[k.min(24)];
                            k += 1;
                        }
                        let v = bits.read(width);
                        c[i] = if v == 0 {
                            0.0
                        } else if bits.bit() {
                            -qv * v as f32
                        } else {
                            qv * v as f32
                        };
                        i += 1;
                    }
                }
                if bits.overrun() {
                    return Err(BinkError::BadFrame("audio past the end of the packet"));
                }
            }
            c[0] *= 2.0;
            self.dct.inverse(c);
        }
        let count = (self.overlap * self.channels) as f32;
        for ch in 0..self.channels {
            let c = &mut self.coefs[ch];
            let prev = &mut self.previous[ch];
            if !self.first {
                for (i, (s, &p)) in c.iter_mut().zip(prev.iter()).enumerate() {
                    let j = (ch + i * self.channels) as f32;
                    *s = (p * (count - j) + *s * j) / count;
                }
            }
            prev.copy_from_slice(&c[len - self.overlap..]);
        }
        self.first = false;
        Ok(())
    }
}

/// A coded float: a 5-bit exponent, a 23-bit mantissa and a sign.
fn float(bits: &mut Bits) -> f32 {
    let power = bits.read(5) as i32;
    let f = bits.read(23) as f32 * (2f32).powi(power - 23);
    if bits.bit() { -f } else { f }
}

/// A DCT-III of a power-of-two size: `x[n] = X[0] / 2 + sum X[k] cos(pi k (2n + 1) / 2N)`,
/// through a complex FFT of the same size.
struct Dct {
    n: usize,
    /// `e^(i pi k / 2N)` for each `k`.
    twiddle: Vec<(f32, f32)>,
    /// `e^(2 pi i k / N)` for the FFT.
    roots: Vec<(f32, f32)>,
    buf: Vec<(f32, f32)>,
}

impl Dct {
    fn new(n: usize) -> Self {
        let twiddle = (0..n)
            .map(|k| {
                let a = std::f64::consts::PI * k as f64 / (2 * n) as f64;
                (a.cos() as f32, a.sin() as f32)
            })
            .collect();
        let roots = (0..n / 2)
            .map(|k| {
                let a = 2.0 * std::f64::consts::PI * k as f64 / n as f64;
                (a.cos() as f32, a.sin() as f32)
            })
            .collect();
        Self {
            n,
            twiddle,
            roots,
            buf: vec![(0.0, 0.0); n],
        }
    }

    fn inverse(&mut self, x: &mut [f32]) {
        let n = self.n;
        // V[k] = e^(i pi k / 2N) (X[k] - i X[N - k]) (X[N] = 0); its unnormalised inverse
        // FFT, reordered, is twice the DCT-III.
        for k in 0..n {
            let re = x[k];
            let im = if k == 0 { 0.0 } else { -x[n - k] };
            let (c, s) = self.twiddle[k];
            self.buf[k] = (re * c - im * s, re * s + im * c);
        }
        fft(&mut self.buf, &self.roots);
        // v = sum V[k] e^(2 pi i k m / N); x[2m] = Re v[m], x[2m + 1] = Re v[N - 1 - m].
        for m in 0..n / 2 {
            x[2 * m] = self.buf[m].0 * 0.5;
            x[2 * m + 1] = self.buf[n - 1 - m].0 * 0.5;
        }
    }
}

/// An in-place radix-2 FFT with positive exponents (an unnormalised inverse transform).
fn fft(a: &mut [(f32, f32)], roots: &[(f32, f32)]) {
    let n = a.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            a.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let step = n / len;
        for start in (0..n).step_by(len) {
            for k in 0..len / 2 {
                let (wr, wi) = roots[k * step];
                let (ur, ui) = a[start + k];
                let (vr, vi) = a[start + k + len / 2];
                let (tr, ti) = (vr * wr - vi * wi, vr * wi + vi * wr);
                a[start + k] = (ur + tr, ui + ti);
                a[start + k + len / 2] = (ur - tr, ui - ti);
            }
        }
        len <<= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dct_matches_its_definition() {
        let n = 64;
        let x: Vec<f32> = (0..n).map(|k| ((k * 37 % 11) as f32 - 5.0) / 3.0).collect();
        let mut got = x.clone();
        Dct::new(n).inverse(&mut got);
        for (m, g) in got.iter().enumerate() {
            let want: f64 = x[0] as f64 / 2.0
                + (1..n)
                    .map(|k| {
                        x[k] as f64
                            * (std::f64::consts::PI * k as f64 * (2 * m + 1) as f64
                                / (2 * n) as f64)
                                .cos()
                    })
                    .sum::<f64>();
            assert!((*g as f64 - want).abs() < 1e-3, "{m}: {g} vs {want}");
        }
    }

    #[test]
    fn bands_follow_the_critical_frequencies() {
        let d = AudioDecoder::new(&AudioTrack {
            rate: 48000,
            flags: 0x3000,
            id: 0,
        })
        .unwrap();
        assert_eq!(d.len, 2048);
        assert_eq!(d.overlap, 128);
        assert_eq!(d.bands.first(), Some(&2));
        assert_eq!(d.bands.last(), Some(&2048));
        assert_eq!(d.bands.len(), 26);
        assert!(d.bands.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn damaged_audio_is_an_error_or_silence_not_a_panic() {
        let mut d = AudioDecoder::new(&AudioTrack {
            rate: 32000,
            flags: 0x3000,
            id: 0,
        })
        .unwrap();
        let mut x = 0x9e37_79b9u32;
        let mut out = Vec::new();
        for len in [0, 3, 4, 5, 9, 64, 700, 5000] {
            let pkt: Vec<u8> = (0..len)
                .map(|_| {
                    x ^= x << 13;
                    x ^= x >> 17;
                    x ^= x << 5;
                    x as u8
                })
                .collect();
            let _ = d.decode(&pkt, &mut out);
        }
    }
}
