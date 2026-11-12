//! `thedas fsb`: list, extract and check FMOD sound banks.

use std::fs::File;
use std::io::{BufWriter, Read};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use thedas_audio::{decode, wav};
use thedas_formats::fev::{EventBody, Fev, WaveformKind};
use thedas_formats::fsb::Fsb;

pub fn play(
    install: &Path,
    events: &[String],
    seconds: f32,
    out: &Path,
    stop_at: Option<f32>,
    language: &str,
    rate: u32,
) -> Result<()> {
    use std::sync::{Arc, Mutex};
    use thedas_audio::{AudioEngine, Library, Mixer, PlayOptions};
    let lib = Arc::new(Library::new(Library::install_dirs(install, language)));
    let mixer = Arc::new(Mutex::new(Mixer::new(rate)));
    let mut engine = AudioEngine::new(lib, mixer.clone());
    engine.synchronous = true;
    // (start time, path, programmer sound)
    let mut queue: Vec<(f32, String, Option<String>)> = events
        .iter()
        .map(|e| {
            let (e, prog) = match e.split_once(':') {
                Some((a, b)) => (a, Some(b.to_string())),
                None => (e.as_str(), None),
            };
            let (path, at) = match e.split_once('@') {
                Some((p, t)) => (p, t.parse().unwrap_or(0.0)),
                None => (e, 0.0),
            };
            (at, path.to_string(), prog)
        })
        .collect();
    queue.sort_by(|a, b| a.0.total_cmp(&b.0));
    let tick = rate as usize / 100;
    let total = (seconds * rate as f32) as usize;
    let mut samples = Vec::with_capacity(total * 2);
    let mut buf = vec![0.0f32; tick * 2];
    let mut handles = Vec::new();
    let mut t = 0.0f32;
    let mut stopped = false;
    while samples.len() < total * 2 {
        while queue.first().is_some_and(|q| q.0 <= t) {
            let (_, path, prog) = queue.remove(0);
            match engine.play(
                &path,
                PlayOptions {
                    programmer_sound: prog,
                    ..Default::default()
                },
            ) {
                Some(h) => handles.push(h),
                None => eprintln!("{path}: no such event"),
            }
        }
        if !stopped && stop_at.is_some_and(|s| t >= s) {
            for h in &handles {
                engine.stop(*h, false);
            }
            stopped = true;
        }
        engine.update(tick as f32 / rate as f32);
        mixer
            .lock()
            .map_err(|_| anyhow::anyhow!("mixer poisoned"))?
            .render(&mut buf);
        samples.extend_from_slice(&buf);
        t += tick as f32 / rate as f32;
    }
    samples.truncate(total * 2);
    let peak = samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    let rms = (samples.iter().map(|s| s * s).sum::<f32>() / samples.len().max(1) as f32).sqrt();
    println!(
        "{}: {seconds}s, peak {:.1} dBFS, rms {:.1} dBFS, {} events still playing",
        out.display(),
        20.0 * peak.max(1e-9).log10(),
        20.0 * rms.max(1e-9).log10(),
        engine.active_events()
    );
    wav::write_wav(BufWriter::new(File::create(out)?), rate, 2, &samples)?;
    Ok(())
}

fn files_under(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            files_under(&p, ext, out);
        } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case(ext)) {
            out.push(p);
        }
    }
}

pub fn fev(path: &Path, filter: Option<&str>, check: bool) -> Result<()> {
    if check {
        let mut files = Vec::new();
        if path.is_dir() {
            files_under(path, "fev", &mut files);
        } else {
            files.push(path.to_path_buf());
        }
        files.sort();
        let (mut ok, mut events, mut bad) = (0usize, 0usize, 0usize);
        for f in &files {
            match Fev::parse(&std::fs::read(f)?) {
                Ok(fev) => {
                    ok += 1;
                    events += fev.events().len();
                }
                Err(e) => {
                    bad += 1;
                    println!("{}: {e}", f.display());
                }
            }
        }
        println!(
            "{} event files: {ok} parse ({events} events), {bad} fail",
            files.len()
        );
        return Ok(());
    }
    let fev = Fev::parse(&std::fs::read(path)?).with_context(|| path.display().to_string())?;
    println!(
        "project {} banks {:?}",
        fev.name,
        fev.wave_banks.iter().map(|b| &b.name).collect::<Vec<_>>()
    );
    for (p, e) in fev.events() {
        if filter.is_some_and(|f| !p.to_lowercase().contains(&f.to_lowercase())) {
            continue;
        }
        println!(
            "{}/{p}: {} vol {:.3} pitch {:.2} {} dist {}..{} fade {}/{} ms cat {:?}",
            fev.name,
            if e.kind_flags == 0x10 {
                "simple"
            } else {
                "complex"
            },
            e.volume,
            e.pitch,
            if e.is_3d() { "3D" } else { "2D" },
            e.min_distance,
            e.max_distance,
            e.fade_in_ms,
            e.fade_out_ms,
            e.categories
        );
        if let EventBody::Complex { parameters, .. } = &e.body {
            for prm in parameters {
                println!("    param {} {}..{}", prm.name, prm.min, prm.max);
            }
        }
        for s in e.sounds() {
            let Some(def) = fev.sound_def(s) else {
                continue;
            };
            let t = fev.template(def);
            println!(
                "    sound {} @{}+{} loop {} play {:?} vol {:?}: {}",
                def.name,
                s.start,
                s.length,
                s.loop_mode,
                t.map(|t| t.play_mode),
                t.map(|t| t.volume),
                def.waveforms
                    .iter()
                    .map(|w| match &w.kind {
                        WaveformKind::Wave {
                            bank, index, file, ..
                        } => {
                            format!("{bank}#{index} ({file})")
                        }
                        k => format!("{k:?}"),
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
    }
    Ok(())
}

fn banks_under(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            banks_under(&p, out);
        } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("fsb")) {
            out.push(p);
        }
    }
}

pub fn fsb(path: &Path, wav_dir: Option<&Path>, only: &[String], check: bool) -> Result<()> {
    if check {
        let mut banks = Vec::new();
        if path.is_dir() {
            banks_under(path, &mut banks);
        } else {
            banks.push(path.to_path_buf());
        }
        banks.sort();
        let (mut ok, mut bad, mut short) = (0usize, 0usize, 0usize);
        for b in &banks {
            let data = std::fs::read(b).with_context(|| b.display().to_string())?;
            let fsb = match Fsb::parse(&data) {
                Ok(f) => f,
                Err(e) => {
                    println!("{}: {e}", b.display());
                    bad += 1;
                    continue;
                }
            };
            for (i, s) in fsb.samples.iter().enumerate() {
                let raw = match fsb.sample_data(&data, i) {
                    Ok(r) => r,
                    Err(e) => {
                        println!("{} {}: {e}", b.display(), s.name);
                        bad += 1;
                        continue;
                    }
                };
                match decode::decoded_len(s, raw) {
                    Ok(n) if n + 1152 < s.length as usize => {
                        println!(
                            "{} {}: decodes {n} of {} frames",
                            b.display(),
                            s.name,
                            s.length
                        );
                        short += 1;
                    }
                    Ok(_) => ok += 1,
                    Err(e) => {
                        println!("{} {}: {e}", b.display(), s.name);
                        bad += 1;
                    }
                }
            }
        }
        println!(
            "{} banks: {ok} samples decode, {short} short, {bad} failures",
            banks.len()
        );
        return Ok(());
    }

    let mut f = File::open(path).with_context(|| path.display().to_string())?;
    let mut head = vec![0u8; 48];
    f.read_exact(&mut head)?;
    let data = std::fs::read(path)?;
    let fsb = Fsb::parse(&data).with_context(|| path.display().to_string())?;
    for (i, s) in fsb.samples.iter().enumerate() {
        if !only.is_empty()
            && !only
                .iter()
                .any(|n| fsb.find(n).is_some_and(|(j, _)| j == i))
        {
            continue;
        }
        println!(
            "{i:4} {:30} {:?} {}ch {:6}Hz {:9} frames {:.2}s vol {} pan {} dist {}..{} mode {:#x}{}",
            s.name,
            s.codec(),
            s.channels,
            s.frequency,
            s.length,
            s.length as f64 / s.frequency.max(1) as f64,
            s.volume,
            s.pan,
            s.min_distance,
            s.max_distance,
            s.mode,
            if s.sync_points.is_empty() {
                String::new()
            } else {
                format!(" sync {}", s.sync_points.len())
            }
        );
        if let Some(dir) = wav_dir {
            std::fs::create_dir_all(dir)?;
            let pcm = decode::decode(s, fsb.sample_data(&data, i)?)?;
            let out = dir.join(format!("{}.wav", s.stem()));
            wav::write_wav(
                BufWriter::new(File::create(&out)?),
                pcm.rate,
                pcm.channels,
                &pcm.samples,
            )?;
        }
    }
    Ok(())
}
