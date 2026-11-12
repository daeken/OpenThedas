//! OpenThedas audio: decoding the game's FMOD sound banks, mixing and output.
//!
//! - [`decode`]: FSB4 samples (IMA ADPCM, MP3) to PCM.
//! - [`library`]: finding the install's FMOD projects and banks; decoded-sample cache.
//! - [`engine`]: playing FMOD events by path on the mixer.
//! - [`mixer`]: voices with gain, pan, pitch, loops, fades and volume buses.
//! - [`wav`]: 16-bit WAV writing, for offline renders.
//! - `device` (feature `device`): the system output through cpal.
//!
//! Audio is presentation only: it reads simulation state and never writes it.

pub mod area;
pub mod decode;
#[cfg(feature = "device")]
pub mod device;
pub mod engine;
pub mod library;
pub mod mixer;
pub mod wav;

pub use decode::{Pcm, decode};
pub use engine::{AudioEngine, EventHandle, PlayOptions, Volumes};
pub use library::Library;
pub use mixer::{Bus, Mixer, VoiceId, VoiceParams};
