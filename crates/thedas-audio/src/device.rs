//! The system audio output (cpal), pulling stereo frames from a shared [`Mixer`].
//!
//! The stream lives as long as the returned [`Output`]; dropping it stops playback.

use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use crate::mixer::Mixer;

pub struct Output {
    _stream: cpal::Stream,
    pub mixer: Arc<Mutex<Mixer>>,
    pub rate: u32,
}

impl Output {
    /// Open the default output device at its preferred rate. `None` (logged) when there is no
    /// device or it refuses the stream; the game then runs silent.
    pub fn open() -> Option<Output> {
        let host = cpal::default_host();
        let device = host.default_output_device().or_else(|| {
            log::warn!("audio: no output device");
            None
        })?;
        let supported = device
            .default_output_config()
            .map_err(|e| log::warn!("audio: no output config: {e}"))
            .ok()?;
        if supported.sample_format() != cpal::SampleFormat::F32 {
            log::warn!(
                "audio: output format {:?} is not f32; running silent",
                supported.sample_format()
            );
            return None;
        }
        let config: cpal::StreamConfig = supported.into();
        let rate = config.sample_rate.0;
        let channels = config.channels as usize;
        let mixer = Arc::new(Mutex::new(Mixer::new(rate)));
        let m = mixer.clone();
        let mut stereo = Vec::new();
        let stream = device
            .build_output_stream(
                &config,
                move |data: &mut [f32], _| {
                    let frames = data.len() / channels.max(1);
                    stereo.resize(frames * 2, 0.0);
                    match m.lock() {
                        Ok(mut mixer) => mixer.render(&mut stereo),
                        Err(_) => stereo.iter_mut().for_each(|s| *s = 0.0),
                    }
                    for (f, out) in data.chunks_exact_mut(channels.max(1)).enumerate() {
                        for (c, o) in out.iter_mut().enumerate() {
                            *o = if c < 2 { stereo[f * 2 + c] } else { 0.0 };
                        }
                        if channels == 1 {
                            out[0] = (stereo[f * 2] + stereo[f * 2 + 1]) * 0.5;
                        }
                    }
                },
                |e| log::warn!("audio: stream error: {e}"),
                None,
            )
            .map_err(|e| log::warn!("audio: cannot open the output stream: {e}"))
            .ok()?;
        stream
            .play()
            .map_err(|e| log::warn!("audio: cannot start the output stream: {e}"))
            .ok()?;
        log::info!("audio: output at {rate} Hz, {channels} channels");
        Some(Output {
            _stream: stream,
            mixer,
            rate,
        })
    }
}
