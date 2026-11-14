//! Reproducible isolated-movie performance and interaction audit, without a window.
//! `movieaudit INSTALL MOVIE_DIRECTORY bench|fuzz [iterations] [seed] [movie,...]`
//! Engine requests are recorded by the bridge and never dispatched to the app.
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Instant;
use thedas_ui::stage::Stage;
use thedas_ui::{Gpu, UiKey};

fn gpu() -> Gpu {
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter =
        pollster::block_on(instance.request_adapter(&Default::default())).expect("adapter");
    let (device, queue) =
        pollster::block_on(adapter.request_device(&Default::default())).expect("device");
    Gpu { device, queue }
}
fn random(seed: &mut u64) -> u64 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 7;
    *seed ^= *seed << 17;
    *seed
}
fn warnings(stage: &mut Stage, out: &mut BTreeSet<String>) {
    for line in stage.log.drain(..) {
        if line.contains("warning") || line.contains("Error") || line.contains("error:") {
            out.insert(line);
        }
    }
}
fn render(stage: &mut Stage, gpu: &Gpu) {
    for movie in &mut stage.movies {
        movie.render();
    }
    gpu.device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("GPU poll");
}
fn main() {
    env_logger::init();
    let args: Vec<String> = std::env::args().collect();
    assert!(
        args.len() >= 4,
        "movieaudit INSTALL MOVIE_DIRECTORY bench|fuzz [iterations] [seed] [movie,...]"
    );
    let bench = match args[3].as_str() {
        "bench" => true,
        "fuzz" => false,
        _ => panic!("mode"),
    };
    let count = args
        .get(4)
        .map(|s| s.parse().expect("iterations"))
        .unwrap_or(if bench { 300 } else { 1200 });
    assert!(count > 0, "iterations must be positive");
    let seed: u64 = args
        .get(5)
        .map(|s| s.parse().expect("seed"))
        .unwrap_or(815795);
    assert_ne!(seed, 0);
    let install = thedas_vfs::DaoInstall::open(&args[1]).expect("install");
    let vfs = Arc::new(
        install
            .play_vfs(thedas_vfs::install::TextureQuality::High)
            .expect("vfs"),
    );
    let mut names: Vec<String> = std::fs::read_dir(&args[2])
        .expect("movie directory")
        .map(|p| p.expect("entry").path())
        .filter(|p| p.extension().is_some_and(|s| s == "gfx"))
        .map(|p| p.file_stem().unwrap().to_string_lossy().into_owned())
        .filter(|n| !n.starts_with("fonts_") && n != "gfxfontlib")
        .filter(|n| {
            args.get(6)
                .is_none_or(|list| list.split(',').any(|m| m == n))
        })
        .collect();
    names.sort();
    assert!(!names.is_empty(), "no matching movies");
    let gpu = gpu();
    let mut execution_failed = false;
    for name in &names {
        println!("BEGIN\town\t{name}\tseed={seed}");
        let mut stage = Stage::new(gpu.clone(), &vfs, (1024, 768));
        stage.open(&format!("{name}.gfx")).expect("open movie");
        let mut startup = BTreeSet::new();
        for _ in 0..60 {
            stage.tick(1000.0 / 30.0);
            if bench {
                render(&mut stage, &gpu);
            }
            warnings(&mut stage, &mut startup);
        }
        if bench && stage.movies.is_empty() {
            println!("SKIP\town\t{name}\tclosed without engine context");
            continue;
        }
        let mut added = BTreeSet::new();
        let mut times = Vec::new();
        let mut state = seed;
        let mut targets = stage.focus_targets();
        let mut reopens = 0;
        let mut processed = 0;
        let mut closed = false;
        for i in 0..count {
            processed = i + 1;
            let start = Instant::now();
            if !bench {
                let r = random(&mut state);
                let (x, y) = if !targets.is_empty() && r & 1 == 0 {
                    targets[(r as usize / 2) % targets.len()].center()
                } else {
                    ((r % 1124) as f64 - 50.0, ((r >> 16) % 868) as f64 - 50.0)
                };
                stage.mouse_move(x, y);
                match r % 10 {
                    0..=2 => {
                        stage.mouse_button(x, y, true);
                        stage.mouse_button(x, y, false);
                    }
                    3 => {
                        stage.mouse_button(x, y, true);
                        stage.mouse_move(x + 20.0, y + 20.0);
                        stage.mouse_button(x + 20.0, y + 20.0, false);
                    }
                    4 => stage.mouse_wheel(x, y, if r & 16 == 0 { -3.0 } else { 3.0 }),
                    5 => {
                        stage.key(UiKey::Tab);
                    }
                    6 => {
                        stage.key(UiKey::Enter);
                    }
                    7 => {
                        stage.key(UiKey::Escape);
                    }
                    8 => {
                        stage.key(UiKey::SelectAll);
                        stage.type_text("Audit Ω 42");
                    }
                    _ => {
                        stage.key(
                            [
                                UiKey::Left,
                                UiKey::Right,
                                UiKey::Home,
                                UiKey::End,
                                UiKey::Backspace,
                                UiKey::Delete,
                            ][r as usize % 6],
                        );
                    }
                }
            }
            stage.tick(if bench {
                1000.0 / 30.0
            } else {
                [0.0, 10.0, 33.333333333, 100.0, 250.0][i as usize % 5]
            });
            if bench && stage.movies.is_empty() {
                closed = true;
                break;
            }
            if bench {
                render(&mut stage, &gpu);
                times.push(start.elapsed().as_secs_f64() * 1000.0);
            }
            warnings(&mut stage, &mut added);
            if !bench && stage.movies.is_empty() {
                if reopens == 3 {
                    break;
                }
                stage.open(&format!("{name}.gfx")).expect("reopen movie");
                reopens += 1;
            }
            if !bench && i % 25 == 0 {
                render(&mut stage, &gpu);
                targets = stage.focus_targets();
            }
        }
        let execution_errors = startup
            .union(&added)
            .filter(|line| {
                [
                    "call stack overflow",
                    "script ran too long; stopped",
                    "action queue runaway",
                    "uncaught throw:",
                ]
                .iter()
                .any(|error| line.contains(error))
            })
            .count();
        execution_failed |= execution_errors != 0;
        if bench {
            if closed {
                println!("SKIP\town\t{name}\tclosed during measurement");
                continue;
            }
            times.sort_by(f64::total_cmp);
            println!(
                "BENCH\town\t{name}\t{count}\tmean_ms={:.6}\tp50_ms={:.6}\tp95_ms={:.6}\tmax_ms={:.6}",
                times.iter().sum::<f64>() / f64::from(count),
                times[times.len() / 2],
                times[(times.len() * 95 / 100).min(times.len() - 1)],
                times[times.len() - 1]
            );
        } else {
            let novel: Vec<_> = added.difference(&startup).collect();
            println!(
                "FUZZ\t{name}\tseed={seed}\tevents={processed}\treopens={reopens}\tstartup_warnings={}\tnew_warnings={}\texecution_errors={execution_errors}",
                startup.len(),
                novel.len()
            );
            for warning in novel {
                println!("NEW\t{name}\t{warning}");
            }
        }
        for warning in startup {
            println!("STARTUP\t{name}\t{warning}");
        }
    }
    if execution_failed {
        std::process::exit(1);
    }
}
