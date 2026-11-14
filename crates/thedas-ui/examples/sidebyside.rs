//! Renders movies through Ruffle and through our own player, the same way the game opens
//! them, and writes both pictures side by side (Ruffle left, ours right) with timings.
//!
//! `sidebyside INSTALL OUT.png MOVIES [FRAMES] [WIDTHxHEIGHT]`, MOVIES as for the stage
//! (`mainmenu`, `hud`, `portraits,quickbar`).

use std::sync::Arc;
use std::time::Instant;

use thedas_ui::stage::{Stage, movie_names};
use thedas_ui::{Gpu, Rgba};

fn gpu() -> Gpu {
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        ..Default::default()
    }))
    .expect("adapter");
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("sidebyside"),
        ..Default::default()
    }))
    .expect("device");
    Gpu {
        instance,
        adapter,
        device,
        queue,
    }
}

fn run(
    gpu: &Gpu,
    vfs: &Arc<thedas_vfs::Vfs>,
    names: &[String],
    frames: u32,
    size: (u32, u32),
    own: bool,
) -> (Rgba, String) {
    // SAFETY: single-threaded tool; the variable is read when each movie opens.
    unsafe {
        std::env::set_var("THEDAS_UI_PLAYER", if own { "own" } else { "ruffle" });
    }
    let mut stage = Stage::new(gpu.clone(), vfs, size);
    let t0 = Instant::now();
    stage.open_all(names);
    let open = t0.elapsed();
    let t1 = Instant::now();
    for _ in 0..frames {
        stage.tick(1000.0 / 30.0);
    }
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
            "{}: open {open:?}, {frames} ticks {ticks:?}, capture {cap:?}, steady tick+render {steady:?}\n  {}",
            if own { "own" } else { "ruffle" },
            log.join("\n  ")
        ),
    )
}

fn main() {
    env_logger::init();
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("usage: sidebyside INSTALL OUT.png MOVIES [FRAMES] [WxH]");
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
    let (a, ta) = run(&gpu, &vfs, &names, frames, size, false);
    let (b, tb) = run(&gpu, &vfs, &names, frames, size, true);
    println!("{ta}\n{tb}");
    let gap = 8;
    let w = size.0 * 2 + gap;
    let mut out = vec![0u8; (w * size.1 * 4) as usize];
    for y in 0..size.1 as usize {
        for (img, x0) in [(&a, 0usize), (&b, (size.0 + gap) as usize)] {
            let src = &img.pixels[y * size.0 as usize * 4..(y + 1) * size.0 as usize * 4];
            let at = (y * w as usize + x0) * 4;
            out[at..at + src.len()].copy_from_slice(src);
        }
    }
    let png = thedas_ui::stage::encode_png(&Rgba {
        width: w,
        height: size.1,
        pixels: out,
    })
    .expect("png");
    std::fs::write(&args[2], png).expect("write");
}
