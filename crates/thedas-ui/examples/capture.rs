//! Captures movies on the game's Scaleform player and reports load/render timings.
//!
//! `capture INSTALL OUT.png MOVIES [FRAMES] [WIDTHxHEIGHT]`; movies may be `menu`,
//! `hud` or a comma-separated list. `SF_CAPTURE_SETUP` accepts `call`/`set` steps.

use std::sync::Arc;
use std::time::Instant;

use thedas_ui::stage::{Stage, movie_names};
use thedas_ui::{Gpu, Rgba, UiValue};

/// Optional reproducible setup, e.g. `call portraits.gfx _root.attachMovie
/// PendingCommandText probe 500; set portraits.gfx _root.probe.TextField.text Attack`.
fn setup(stage: &mut Stage) {
    let Ok(steps) = std::env::var("SF_CAPTURE_SETUP") else {
        return;
    };
    let value = |s: &str| {
        s.parse::<f64>()
            .map(UiValue::Number)
            .unwrap_or_else(|_| match s {
                "true" => UiValue::Bool(true),
                "false" => UiValue::Bool(false),
                "null" => UiValue::Null,
                _ => UiValue::String(s.into()),
            })
    };
    for step in steps.split(';').map(str::trim).filter(|s| !s.is_empty()) {
        let words: Vec<&str> = step.split_whitespace().collect();
        assert!(words.len() >= 4, "setup: {step}");
        let movie = stage
            .movies
            .iter_mut()
            .find(|m| m.name == words[1])
            .expect("setup movie");
        match words[0] {
            "call" => {
                movie.call(
                    words[2],
                    &words[3..].iter().map(|s| value(s)).collect::<Vec<_>>(),
                );
            }
            "set" => movie.set(words[2], value(&words[3..].join(" "))),
            _ => panic!("setup: {step}"),
        }
    }
}

fn gpu() -> Gpu {
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        ..Default::default()
    }))
    .expect("adapter");
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("capture"),
        ..Default::default()
    }))
    .expect("device");
    Gpu { device, queue }
}

fn run(
    gpu: &Gpu,
    vfs: &Arc<thedas_vfs::Vfs>,
    names: &[String],
    frames: u32,
    size: (u32, u32),
) -> (Rgba, String) {
    let mut stage = Stage::new(gpu.clone(), vfs, size);
    let t0 = Instant::now();
    stage.open_all(names);
    let open = t0.elapsed();
    let t1 = Instant::now();
    for _ in 0..frames {
        stage.tick(1000.0 / 30.0);
    }
    setup(&mut stage);
    let ticks = t1.elapsed();
    let mut base = Rgba {
        width: size.0,
        height: size.1,
        pixels: [40u8, 44, 52, 255].repeat((size.0 * size.1) as usize),
    };
    let t2 = Instant::now();
    if let Err(e) = stage.capture_over(&mut base) {
        eprintln!("capture: {e}");
    }
    let cap = t2.elapsed();
    // Steady state: ticks with nothing happening.
    let t3 = Instant::now();
    for _ in 0..30 {
        stage.tick(1000.0 / 30.0);
        for m in &mut stage.movies {
            m.render();
        }
    }
    let steady = t3.elapsed() / 30;
    let log: Vec<String> = stage
        .log
        .iter()
        .filter(|l| l.contains("warning"))
        .take(15)
        .cloned()
        .collect();
    (
        base,
        format!(
            "open {open:?}, {frames} ticks {ticks:?}, capture {cap:?}, steady tick+render {steady:?}\n  {}",
            log.join("\n  ")
        ),
    )
}

fn main() {
    env_logger::init();
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("usage: capture INSTALL OUT.png MOVIES [FRAMES] [WxH]");
        std::process::exit(2);
    }
    let install = thedas_vfs::DaoInstall::open(&args[1]).expect("install");
    let vfs = Arc::new(
        install
            .play_vfs(thedas_vfs::install::TextureQuality::High)
            .expect("vfs"),
    );
    let names = movie_names(&args[3]);
    let frames: u32 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(30);
    let size = args
        .get(5)
        .and_then(|s| s.split_once('x'))
        .and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?)))
        .unwrap_or((1280u32, 720u32));
    let gpu = gpu();
    let (frame, timing) = run(&gpu, &vfs, &names, frames, size);
    println!("{timing}");
    let png = thedas_ui::stage::encode_png(&frame).expect("png");
    std::fs::write(&args[2], png).expect("write");
}
