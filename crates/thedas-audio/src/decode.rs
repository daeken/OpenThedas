//! Decoding FSB4 samples to PCM.
//!
//! DA:O ships two codecs (see [`thedas_formats::fsb`]): FMOD's IMA ADPCM and MPEG layer 3.
//! The MP3 frames go through Symphonia's decoder (MPL-2.0, used as a dependency); the ADPCM
//! decoder is written here.

use std::sync::Arc;

use symphonia_bundle_mp3::MpaDecoder;
use symphonia_core::audio::{AudioBufferRef, Signal};
use symphonia_core::codecs::{CODEC_TYPE_MP3, CodecParameters, Decoder, DecoderOptions};
use symphonia_core::formats::Packet;
use thedas_formats::fsb::{Codec, FsbSample};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DecodeError {
    #[error("sample {0}: codec {1:?} is not supported")]
    Codec(String, Codec),
    #[error("sample {0}: {1} channels are not supported")]
    Channels(String, u16),
    #[error("sample {0}: MP3 decoder: {1}")]
    Mp3(String, String),
}

/// Decoded audio: interleaved `f32` frames in -1..=1.
#[derive(Clone, Debug, PartialEq)]
pub struct Pcm {
    pub rate: u32,
    pub channels: u16,
    pub samples: Vec<f32>,
}

impl Pcm {
    pub fn frames(&self) -> usize {
        self.samples.len() / self.channels.max(1) as usize
    }

    pub fn shared(self) -> Arc<Pcm> {
        Arc::new(self)
    }
}

/// Decode one sample's data (`data` is exactly its encoded bytes). The result is cut to the
/// header's length in frames (encoders pad the last block or frame).
pub fn decode(sample: &FsbSample, data: &[u8]) -> Result<Pcm, DecodeError> {
    let ch = sample.channels;
    if !(1..=2).contains(&ch) {
        return Err(DecodeError::Channels(sample.name.clone(), ch));
    }
    let mut samples = match sample.codec() {
        Codec::ImaAdpcm => decode_ima(data, ch as usize),
        Codec::Mpeg => {
            decode_mp3(data, ch as usize).map_err(|e| DecodeError::Mp3(sample.name.clone(), e))?
        }
        Codec::Pcm16 => data
            .chunks_exact(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0)
            .collect(),
        Codec::Pcm8 => data.iter().map(|&b| (b as i8) as f32 / 128.0).collect(),
        c => return Err(DecodeError::Codec(sample.name.clone(), c)),
    };
    let want = sample.length as usize * ch as usize;
    if samples.len() > want {
        samples.truncate(want);
    } else {
        samples.resize(want, 0.0);
    }
    Ok(Pcm {
        rate: sample.frequency,
        channels: ch,
        samples,
    })
}

/// How many frames the sample's data decodes to, before cutting or padding to the header's
/// length (for checking banks).
pub fn decoded_len(sample: &FsbSample, data: &[u8]) -> Result<usize, DecodeError> {
    let ch = sample.channels.max(1) as usize;
    let n = match sample.codec() {
        Codec::ImaAdpcm => data.len() / (IMA_BLOCK * ch) * IMA_BLOCK_FRAMES,
        Codec::Mpeg => {
            decode_mp3(data, ch)
                .map_err(|e| DecodeError::Mp3(sample.name.clone(), e))?
                .len()
                / ch
        }
        Codec::Pcm16 => data.len() / 2 / ch,
        Codec::Pcm8 => data.len() / ch,
        c => return Err(DecodeError::Codec(sample.name.clone(), c)),
    };
    Ok(n)
}

const IMA_STEPS: [i32; 89] = [
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50, 55, 60, 66,
    73, 80, 88, 97, 107, 118, 130, 143, 157, 173, 190, 209, 230, 253, 279, 307, 337, 371, 408, 449,
    494, 544, 598, 658, 724, 796, 876, 963, 1060, 1166, 1282, 1411, 1552, 1707, 1878, 2066, 2272,
    2499, 2749, 3024, 3327, 3660, 4026, 4428, 4871, 5358, 5894, 6484, 7132, 7845, 8630, 9493,
    10442, 11487, 12635, 13899, 15289, 16818, 18500, 20350, 22385, 24623, 27086, 29794, 32767,
];
const IMA_INDEX: [i32; 16] = [-1, -1, -1, -1, 2, 4, 6, 8, -1, -1, -1, -1, 2, 4, 6, 8];

/// Bytes per channel per ADPCM block, and the frames each block holds.
pub const IMA_BLOCK: usize = 36;
pub const IMA_BLOCK_FRAMES: usize = 64;

struct ImaState {
    predictor: i32,
    index: i32,
}

impl ImaState {
    fn step(&mut self, nibble: u8) -> f32 {
        let step = IMA_STEPS[self.index as usize];
        let mut diff = step >> 3;
        if nibble & 1 != 0 {
            diff += step >> 2;
        }
        if nibble & 2 != 0 {
            diff += step >> 1;
        }
        if nibble & 4 != 0 {
            diff += step;
        }
        if nibble & 8 != 0 {
            diff = -diff;
        }
        self.predictor = (self.predictor + diff).clamp(-32768, 32767);
        self.index = (self.index + IMA_INDEX[nibble as usize & 15]).clamp(0, 88);
        self.predictor as f32 / 32768.0
    }
}

/// FMOD's IMA ADPCM (the Xbox layout): per block and channel a 4-byte header (initial
/// predictor, step index) and 32 bytes of nibbles, low nibble first. The header's predictor is
/// the block's first frame and 63 nibbles follow (the last nibble is unused), 64 frames in all.
/// Stereo blocks put both headers first, then alternate 4-byte words of each channel.
///
/// Provisional: the layout follows the community's description of FSB IMA ADPCM; the frame
/// count per block agrees with every bank's header lengths.
pub fn decode_ima(data: &[u8], channels: usize) -> Vec<f32> {
    let block = IMA_BLOCK * channels;
    let mut out = Vec::with_capacity(data.len() / block * IMA_BLOCK_FRAMES * channels);
    let mut nibbles = vec![[0u8; 64]; channels];
    for b in data.chunks_exact(block) {
        let mut states: Vec<ImaState> = (0..channels)
            .map(|c| {
                let h = &b[c * 4..c * 4 + 4];
                ImaState {
                    predictor: i16::from_le_bytes([h[0], h[1]]) as i32,
                    index: (h[2] as i32).clamp(0, 88),
                }
            })
            .collect();
        let body = &b[4 * channels..];
        for (c, n) in nibbles.iter_mut().enumerate() {
            for word in 0..8 {
                let at = (word * channels + c) * 4;
                for k in 0..4 {
                    let byte = body[at + k];
                    n[word * 8 + k * 2] = byte & 15;
                    n[word * 8 + k * 2 + 1] = byte >> 4;
                }
            }
        }
        for s in &states {
            out.push(s.predictor as f32 / 32768.0);
        }
        for i in 0..IMA_BLOCK_FRAMES - 1 {
            for (s, n) in states.iter_mut().zip(&nibbles) {
                out.push(s.step(n[i]));
            }
        }
    }
    out
}

/// The length of the MPEG audio frame whose header starts `h`, if it is one.
fn mpeg_frame_len(h: &[u8]) -> Option<usize> {
    if h.len() < 4 || h[0] != 0xff || h[1] & 0xe0 != 0xe0 {
        return None;
    }
    let version = (h[1] >> 3) & 3; // 0: 2.5, 2: 2, 3: 1
    let layer = (h[1] >> 1) & 3; // 1: III, 2: II, 3: I
    let br_index = (h[2] >> 4) as usize;
    let sr_index = ((h[2] >> 2) & 3) as usize;
    let padding = ((h[2] >> 1) & 1) as usize;
    if version == 1 || layer == 0 || br_index == 0 || br_index == 15 || sr_index == 3 {
        return None;
    }
    const BR_V1_L3: [usize; 15] = [
        0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320,
    ];
    const BR_V1_L2: [usize; 15] = [
        0, 32, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384,
    ];
    const BR_V1_L1: [usize; 15] = [
        0, 32, 64, 96, 128, 160, 192, 224, 256, 288, 320, 352, 384, 416, 448,
    ];
    const BR_V2_L23: [usize; 15] = [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160];
    const BR_V2_L1: [usize; 15] = [
        0, 32, 48, 56, 64, 80, 96, 112, 128, 144, 160, 176, 192, 224, 256,
    ];
    const SR: [[usize; 3]; 4] = [
        [11025, 12000, 8000],
        [0, 0, 0],
        [22050, 24000, 16000],
        [44100, 48000, 32000],
    ];
    let v1 = version == 3;
    let kbps = match (v1, layer) {
        (true, 1) => BR_V1_L3[br_index],
        (true, 2) => BR_V1_L2[br_index],
        (true, _) => BR_V1_L1[br_index],
        (false, 3) => BR_V2_L1[br_index],
        (false, _) => BR_V2_L23[br_index],
    };
    let rate = SR[version as usize][sr_index];
    let bps = kbps * 1000;
    Some(match layer {
        3 => (12 * bps / rate + padding) * 4,
        1 if !v1 => 72 * bps / rate + padding,
        _ => 144 * bps / rate + padding,
    })
}

/// Split back-to-back MPEG frames, skipping any padding between them (FMOD may align frames).
pub fn mpeg_frames(data: &[u8]) -> Vec<&[u8]> {
    let mut frames = Vec::new();
    let mut at = 0;
    while at + 4 <= data.len() {
        match mpeg_frame_len(&data[at..]) {
            Some(len) if len >= 4 && at + len <= data.len() => {
                frames.push(&data[at..at + len]);
                at += len;
            }
            Some(_) => break,
            None => at += 1,
        }
    }
    frames
}

fn decode_mp3(data: &[u8], channels: usize) -> Result<Vec<f32>, String> {
    let mut params = CodecParameters::new();
    params.for_codec(CODEC_TYPE_MP3);
    let mut dec =
        MpaDecoder::try_new(&params, &DecoderOptions::default()).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for (i, frame) in mpeg_frames(data).into_iter().enumerate() {
        let packet = Packet::new_from_slice(0, i as u64, 0, frame);
        let buf = match dec.decode(&packet) {
            Ok(b) => b,
            // A damaged frame is skipped, as players do; the length is padded at the end.
            Err(symphonia_core::errors::Error::DecodeError(e)) => {
                log::debug!("mp3 frame {i}: {e}");
                continue;
            }
            Err(e) => return Err(e.to_string()),
        };
        let AudioBufferRef::F32(b) = buf else {
            return Err("unexpected sample format".into());
        };
        let src = b.spec().channels.count();
        let n = b.frames();
        for f in 0..n {
            for c in 0..channels {
                out.push(b.chan(c.min(src - 1))[f]);
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An encoder for the same IMA step, to round-trip a signal.
    fn encode_block(pcm: &[i16]) -> Vec<u8> {
        let mut b = vec![0u8; IMA_BLOCK];
        b[..2].copy_from_slice(&pcm[0].to_le_bytes());
        b[2] = 0;
        let mut st = ImaState {
            predictor: pcm[0] as i32,
            index: 0,
        };
        for (i, &target) in pcm[1..64].iter().enumerate() {
            let step = IMA_STEPS[st.index as usize];
            let mut diff = target as i32 - st.predictor;
            let mut nib = 0u8;
            if diff < 0 {
                nib = 8;
                diff = -diff;
            }
            if diff >= step {
                nib |= 4;
                diff -= step;
            }
            if diff >= step >> 1 {
                nib |= 2;
                diff -= step >> 1;
            }
            if diff >= step >> 2 {
                nib |= 1;
            }
            st.step(nib);
            b[4 + i / 2] |= if i % 2 == 0 { nib } else { nib << 4 };
        }
        b
    }

    #[test]
    fn ima_mono_round_trips_a_sine() {
        let pcm: Vec<i16> = (0..64)
            .map(|i| ((i as f32 * 0.2).sin() * 8000.0) as i16)
            .collect();
        let out = decode_ima(&encode_block(&pcm), 1);
        assert_eq!(out.len(), 64);
        assert_eq!(out[0], pcm[0] as f32 / 32768.0);
        for (a, b) in out.iter().zip(&pcm).skip(16) {
            assert!((a * 32768.0 - *b as f32).abs() < 600.0, "{a} vs {b}");
        }
    }

    #[test]
    fn ima_stereo_interleaves_words() {
        let l: Vec<i16> = (0..64).map(|i| (i * 100) as i16).collect();
        let r: Vec<i16> = (0..64).map(|i| -(i * 100) as i16).collect();
        let (bl, br) = (encode_block(&l), encode_block(&r));
        let mut block = Vec::new();
        block.extend_from_slice(&bl[..4]);
        block.extend_from_slice(&br[..4]);
        for w in 0..8 {
            block.extend_from_slice(&bl[4 + w * 4..8 + w * 4]);
            block.extend_from_slice(&br[4 + w * 4..8 + w * 4]);
        }
        let out = decode_ima(&block, 2);
        assert_eq!(out.len(), 128);
        assert!(out[120] > 0.1 && out[121] < -0.1);
        // Truncated input yields nothing rather than panicking.
        assert!(decode_ima(&block[..50], 2).is_empty());
    }

    #[test]
    fn mpeg_frames_skip_padding_and_stop_at_truncation() {
        // MPEG-1 layer III, 128 kbps, 44.1 kHz, no padding: 417 bytes.
        let hdr = [0xff, 0xfb, 0x90, 0x00];
        assert_eq!(mpeg_frame_len(&hdr), Some(417));
        // MPEG-2 layer III, 64 kbps, 24 kHz: 72 * 64000 / 24000 = 192 bytes.
        assert_eq!(mpeg_frame_len(&[0xff, 0xf3, 0x84, 0x00]), Some(192));
        let mut data = Vec::new();
        for _ in 0..2 {
            data.extend_from_slice(&hdr);
            data.resize(data.len() + 413, 0);
            data.extend_from_slice(&[0, 0, 0]);
        }
        data.extend_from_slice(&hdr);
        data.resize(data.len() + 10, 0);
        let frames = mpeg_frames(&data);
        assert_eq!(frames.len(), 2);
        assert!(frames.iter().all(|f| f.len() == 417));
        assert!(mpeg_frame_len(&[0xff, 0xff, 0xff, 0xff]).is_none());
    }
}
