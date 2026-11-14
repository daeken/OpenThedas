//! Development check, not shipped: decodes movies with our decoder and with an `ffmpeg` on
//! the PATH (as an oracle only), and compares them frame by frame and sample by sample.
//!
//! `cargo run --release -p thedas-video --example bink_oracle -- <movie.bik>...`
//!
//! One line per movie: `OK`/`DIFF`, frames, frames that differ (and the first), the largest
//! pixel difference, and the audio's sample counts, largest difference and SNR in dB.

// An error ends the check of that movie; its ffmpeg is reaped when the process exits.
#![allow(clippy::zombie_processes)]

use std::io::Read;
use std::process::{Command, Stdio};
use thedas_video::bink::{AudioDecoder, Movie, VideoDecoder};

fn main() {
    let movies: Vec<String> = std::env::args().skip(1).collect();
    if movies.is_empty() {
        eprintln!("usage: bink_oracle <movie.bik>...");
        std::process::exit(2);
    }
    let mut bad = 0;
    for m in &movies {
        match check(m) {
            Ok(line) => {
                if !line.starts_with("OK") {
                    bad += 1;
                }
                println!("{line}");
            }
            Err(e) => {
                bad += 1;
                println!("ERR {m}: {e}");
            }
        }
    }
    eprintln!("{} movies, {bad} not identical", movies.len());
    std::process::exit((bad > 0) as i32);
}

/// Runs ffmpeg over `path` writing raw output to its stdout; the caller reads it and waits.
fn ffmpeg(path: &str, args: &[&str]) -> std::process::Child {
    Command::new("ffmpeg")
        .args(["-nostdin", "-v", "error", "-i", path])
        .args(args)
        .arg("-")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .expect("ffmpeg")
}

fn check(path: &str) -> Result<String, Box<dyn std::error::Error>> {
    let mut movie = Movie::open(std::path::Path::new(path))?;
    let header = movie.header.clone();
    let mut video = VideoDecoder::new(&header)?;
    let mut audio = match header.tracks.first() {
        Some(t) => Some(AudioDecoder::new(t)?),
        None => None,
    };
    let mut child = ffmpeg(path, &["-an", "-f", "rawvideo", "-pix_fmt", "yuv420p"]);
    let mut theirs = child.stdout.take().ok_or("no stdout")?;
    let (mut differ, mut first, mut maxd, mut errors) = (0u32, None, 0u8, 0u32);
    let mut ours = Vec::new();
    let mut pcm = Vec::new();
    let mut frames = 0;
    for f in 0..movie.frames() {
        let p = movie.packets(f)?;
        if let (Some(a), Some(pkt)) = (audio.as_mut(), p.audio.first()) {
            a.decode(pkt, &mut pcm)?;
        }
        ours.clear();
        match video.decode(&p.video) {
            Ok(fr) => fr.write_yuv420(&mut ours),
            Err(e) => {
                if errors == 0 {
                    eprintln!("{path}: frame {f}: {e}");
                }
                errors += 1;
                video.last().write_yuv420(&mut ours);
            }
        }
        let mut t = vec![0u8; ours.len()];
        if theirs.read_exact(&mut t).is_err() {
            break;
        }
        frames += 1;
        if t != ours {
            if differ == 0
                && let Ok(dir) = std::env::var("BINK_DUMP")
            {
                let _ = std::fs::write(format!("{dir}/ours.yuv"), &ours);
                let _ = std::fs::write(format!("{dir}/theirs.yuv"), &t);
            }
            differ += 1;
            first.get_or_insert(f);
            let d = t
                .iter()
                .zip(&ours)
                .map(|(a, b)| a.abs_diff(*b))
                .max()
                .unwrap_or(0);
            maxd = maxd.max(d);
        }
    }
    let mut rest = Vec::new();
    theirs.read_to_end(&mut rest)?;
    child.wait()?;
    let mut line = format!(
        "{} {path}: {frames}/{} frames, {differ} differ (first {:?}, max {maxd}), {errors} errors{}",
        if differ == 0 && errors == 0 && rest.is_empty() && frames == movie.frames() {
            "OK"
        } else {
            "DIFF"
        },
        movie.frames(),
        first,
        if rest.is_empty() {
            ""
        } else {
            ", ffmpeg has more frames"
        },
    );
    if let Some(t) = header.tracks.first() {
        let mut raw = Vec::new();
        let mut child = ffmpeg(
            path,
            &["-vn", "-f", "f32le", "-ac", &t.channels().to_string()],
        );
        child
            .stdout
            .take()
            .ok_or("no stdout")?
            .read_to_end(&mut raw)?;
        child.wait()?;
        let theirs: Vec<f32> = raw
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .collect();
        let n = theirs.len().min(pcm.len());
        let (mut sig, mut err, mut maxa, mut dot, mut oo) = (0f64, 0f64, 0f32, 0f64, 0f64);
        for (a, b) in theirs[..n].iter().zip(&pcm[..n]) {
            sig += (*a as f64).powi(2);
            err += (*a as f64 - *b as f64).powi(2);
            maxa = maxa.max((a - b).abs());
            dot += *a as f64 * *b as f64;
            oo += (*b as f64).powi(2);
        }
        let snr = 10.0 * (sig / err.max(1e-30)).log10();
        line += &format!(
            "; audio {} vs ffmpeg {} samples, max diff {:.2e} ({:.1} in 16-bit), SNR {snr:.1} dB, scale {:.4}",
            pcm.len(),
            theirs.len(),
            maxa,
            maxa * 32768.0,
            dot / oo.max(1e-30),
        );
        if (err > 0.0 && snr < 90.0) || pcm.len() != theirs.len() {
            line = line.replacen("OK ", "DIFF ", 1);
        }
    }
    Ok(line)
}
