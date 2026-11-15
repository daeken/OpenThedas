//! Deterministic offscreen shadow repro. No window; optional frozen save objects.
use std::{io::Write, path::PathBuf};

use anyhow::{Context, Result};
use clap::Parser;
use glam::Vec3;
use thedas_render::{AreaRenderer, AreaScene, Camera, GpuContext, ProgramRegistry, RenderCache};
use thedas_vfs::{DaoInstall, install::TextureQuality};

#[derive(Parser)]
struct Args {
    #[arg(long)]
    install: PathBuf,
    #[arg(long)]
    out: PathBuf,
    /// Freeze the creatures/placeables from a save in the requested area.
    #[arg(long)]
    save: Option<PathBuf>,
    #[arg(long, default_value = "bdc200ar_slums")]
    area: String,
    #[arg(long, default_value = "389,240,10,99,-14", allow_hyphen_values = true)]
    camera: String,
    /// World-space movement per frame, in metres (60 frames per second).
    #[arg(long, default_value = "0.01,0,0", allow_hyphen_values = true)]
    step: String,
    #[arg(long, default_value_t = 120)]
    frames: u32,
    #[arg(long, default_value_t = 30)]
    warmup: u32,
    /// Seconds of particle/shader time per captured frame; zero isolates camera fitting.
    #[arg(long, default_value_t = 0.0)]
    time_step: f64,
    #[arg(long, default_value = "faithful", value_parser = ["faithful", "deferred"])]
    renderer: String,
    /// A control sequence with the same lighting but no run-time shadow casters.
    #[arg(long)]
    unshadowed: bool,
}

fn main() -> Result<()> {
    env_logger::init();
    let args = Args::parse();
    let install = DaoInstall::open(&args.install)?;
    let mut vfs = install.play_vfs(TextureQuality::High)?;
    let level = match install.mount_level(&mut vfs, &args.area) {
        Ok(m) => m.level,
        Err(_) => {
            let (data, _) = vfs.read(&format!("{}.are", args.area))?;
            let layout = thedas_formats::Area::parse(&data)?
                .layout
                .context("no layout")?;
            install.mount_level(&mut vfs, &layout)?.level
        }
    };
    let mut scene = AreaScene::load(&vfs, &level)?;
    println!(
        "{level}: sun {:?}, {} lights, {} draws",
        scene.sun,
        scene.lights.len(),
        scene.draws.len()
    );
    if args.unshadowed {
        for draw in &mut scene.draws {
            draw.cast_shadow = Some(false);
        }
    }
    let mut cache = RenderCache::default();
    let filter = thedas_render::d3d::object_filter();
    let source = thedas_render::d3d::D3dSource::new_cached(&vfs, &scene, &filter, &mut cache);
    let mut programs = ProgramRegistry::default();
    programs.push_front(Box::new(source));
    let ctx = GpuContext::headless()?;
    let mut renderer = AreaRenderer::new_cached_with_path(
        &ctx,
        &vfs,
        &scene,
        &programs,
        thedas_render::gpu::CAPTURE_FORMAT,
        &mut cache,
        if args.renderer == "deferred" {
            thedas_render::RenderPath::Deferred
        } else {
            thedas_render::RenderPath::Faithful
        },
    );
    // Freeze animated effects and disable postprocessing so deltas measure the shadows.
    renderer.frame_effects = false;
    renderer.antialias = false;
    renderer.set_time(3.0);
    let mut objects = if let Some(save) = &args.save {
        let options = thedas_sim::session::SessionOptions {
            install: args.install.clone(),
            module: thedas_vfs::install::campaign_module(&args.install).to_string(),
            area: Some(args.area.clone()),
            waypoint: None,
            seed: 1,
            keep_trace: false,
            cutscenes: thedas_sim::sim::CutscenePolicy::Play,
            chargen: thedas_sim::sim::ChargenPolicy::QuickStart,
        };
        let mut sim = thedas_sim::session::open(&options)?;
        sim.load_json(&std::fs::read(save)?)?;
        let area = sim.world.area.context("save has no current area")?;
        anyhow::ensure!(
            sim.world.get(area).is_some_and(|a| a.template == args.area),
            "save is in a different area"
        );
        let models = thedas_ui::models::area_models(&sim, area);
        let props: Vec<_> = models
            .iter()
            .map(|m| thedas_render::Prop {
                label: format!("{} {}", m.id, m.tag),
                models: m.models.clone(),
                attached: m
                    .attached
                    .iter()
                    .map(|a| (a.model.clone(), a.node.clone()))
                    .collect(),
                morph: m.morph.clone(),
                hide_hair: m.hide_hair,
                tints: m.tints.clone(),
                effects: Vec::new(),
                transform: thedas_render::Prop::placement(m.position, m.facing),
            })
            .collect();
        let mut scene = AreaScene::load_objects(&vfs, &level, &props)?;
        println!(
            "{} frozen objects, {} object draws",
            models.len(),
            scene.draws.len()
        );
        if args.unshadowed {
            for draw in &mut scene.draws {
                draw.cast_shadow = Some(false);
            }
        }
        let source = thedas_render::d3d::D3dSource::new_cached(&vfs, &scene, &filter, &mut cache);
        let mut programs = ProgramRegistry::default();
        programs.push_front(Box::new(source));
        let mut objects = AreaRenderer::new_cached_with_path(
            &ctx,
            &vfs,
            &scene,
            &programs,
            thedas_render::gpu::CAPTURE_FORMAT,
            &mut cache,
            renderer.path,
        );
        objects.set_time(3.0);
        Some(objects)
    } else {
        None
    };
    let mut camera = Camera::parse(&args.camera).context("bad camera")?;
    let step: Vec<f32> = args
        .step
        .split(',')
        .map(str::parse)
        .collect::<Result<_, _>>()?;
    anyhow::ensure!(step.len() == 3, "step must be x,y,z");
    let step = Vec3::new(step[0], step[1], step[2]);
    let (width, height) = (640, 360);
    std::fs::create_dir_all(&args.out)?;
    std::fs::write(
        args.out.join("settings.txt"),
        std::env::args().skip(1).collect::<Vec<_>>().join(" "),
    )?;
    for _ in 0..args.warmup {
        let mut overlays: Vec<_> = objects.iter_mut().collect();
        renderer.capture_with(&ctx, &camera, width, height, &mut overlays, None)?;
    }
    let mut metadata = std::fs::File::create(args.out.join("cameras.csv"))?;
    writeln!(metadata, "frame,x,y,z,yaw,pitch")?;
    let mut shadow_data = std::fs::File::create(args.out.join("shadow-data.csv"))?;
    writeln!(shadow_data, "frame,size,partition,row,x,y,z,w")?;
    let mut emitter_data = std::fs::File::create(args.out.join("emitter-lights.csv"))?;
    writeln!(emitter_data, "frame,emitter,x,y,z,radius,r,g,b")?;
    for i in 0..args.frames {
        let time = 3.0 + f64::from(i) * args.time_step;
        renderer.set_time(time);
        if let Some(objects) = objects.as_mut() {
            objects.set_time(time);
        }
        for (emitter, state) in renderer.emitters.iter().enumerate() {
            if let Some(thedas_render::scene::SceneLight::Point {
                position,
                radius,
                color,
                ..
            }) = thedas_render::deferred::emitter_light(state)
            {
                writeln!(
                    emitter_data,
                    "{i},{emitter},{},{},{},{radius},{},{},{}",
                    position.x, position.y, position.z, color.x, color.y, color.z
                )?;
            }
        }
        let mut overlays: Vec<_> = objects.iter_mut().collect();
        let rgba = renderer.capture_with(&ctx, &camera, width, height, &mut overlays, None)?;
        if let Some((data, size)) = renderer.sun_shadow_data() {
            for partition in 0..2 {
                for row in 0..3 {
                    let [x, y, z, w] = data[partition * 4 + row];
                    writeln!(shadow_data, "{i},{size},{partition},{row},{x},{y},{z},{w}")?;
                }
            }
        }
        let file =
            std::io::BufWriter::new(std::fs::File::create(args.out.join(format!("{i:04}.png")))?);
        let mut encoder = png::Encoder::new(file, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.write_header()?.write_image_data(&rgba)?;
        writeln!(
            metadata,
            "{i},{},{},{},{},{}",
            camera.position.x, camera.position.y, camera.position.z, camera.yaw, camera.pitch
        )?;
        camera.position += step;
    }
    Ok(())
}
