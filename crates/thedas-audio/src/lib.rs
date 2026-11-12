//! OpenThedas audio: decoding the game's FMOD sound banks, mixing and output.
//!
//! - [`decode`]: FSB4 samples (IMA ADPCM, MP3) to PCM.
//! - [`mixer`]: voices with gain, pan, pitch, loops, fades and volume buses.
//! - [`wav`]: 16-bit WAV writing, for offline renders.
//! - `device` (feature `device`): the system output through cpal.
//!
//! Audio is presentation only: it reads simulation state and never writes it.

pub mod decode;
#[cfg(feature = "device")]
pub mod device;
pub mod mixer;
pub mod wav;

pub use decode::{Pcm, decode};
pub use mixer::{Bus, Mixer, VoiceId, VoiceParams};
