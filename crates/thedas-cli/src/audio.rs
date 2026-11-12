//! `thedas fsb`: list, extract and check FMOD sound banks.

use std::fs::File;
use std::io::{BufWriter, Read};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use thedas_audio::{decode, wav};
use thedas_formats::fsb::Fsb;

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
