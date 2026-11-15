//! `thedas-bake`: bakes a level's lightmaps with our baker and compares them texel by texel with
//! the shipped atlases.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};
use clap::Parser;
use glam::Vec3;
use thedas_bake::bake::{BakeLight, LightKind, Settings, bake_region, placement};
use thedas_bake::compare::Stats;
use thedas_bake::gather::BakeScene;
use thedas_formats::dds::Dds;
use thedas_render::AreaScene;
use thedas_render::scene::SceneLight;
use thedas_vfs::DaoInstall;
use thedas_vfs::install::TextureQuality;

#[derive(Parser)]
#[command(about = "Bake a level's lightmaps and compare them with the shipped ones")]
struct Args {
    /// The game install.
    install: PathBuf,
    /// The level (layout) name, e.g. hrt201d.
    level: String,
    /// Where to write images and the report.
    #[arg(long, default_value = "bake-out")]
    out: PathBuf,
    /// Only atlases whose name contains this.
    #[arg(long)]
    atlas: Option<String>,
    /// Worker threads.
    #[arg(long, default_value_t = 4)]
    threads: usize,
    /// Use a converged occlusion estimate with this many rays per sample.
    #[arg(long)]
    ao_converged: Option<u32>,
    /// Experimental shadow pass min(1, k·Σ falloff·cos)^e, as "k,e".
    #[arg(long, value_parser = parse_ke)]
    shadow_intensity: Option<(f32, f32)>,
    /// Experimental fitted occlusion model "length,exponent,lo,power".
    #[arg(long)]
    ao_model: Option<String>,
    /// Experimental soft shadows for point lights: "radius,samples".
    #[arg(long, value_parser = parse_ke)]
    light_size: Option<(f32, f32)>,
    /// Grow each part's covered area by this many texels.
    #[arg(long, default_value_t = 0)]
    dilate: u32,
    /// Occlusion ray length.
    #[arg(long, default_value_t = 5.0)]
    ao_length: f32,
    /// Occlusion rays per sample.
    #[arg(long, default_value_t = 5)]
    ao_rays: u32,
    /// Scale applied to the runtime lights' colour to make the bake lights' brightness.
    #[arg(long, default_value_t = 1.0)]
    brightness: f32,
    /// Fit the lights' brightness and the room ambients to the shipped atlases instead of
    /// baking.
    #[arg(long)]
    fit: bool,
    /// Explain one atlas texel (x,y) instead of baking.
    #[arg(long, value_parser = parse_xy)]
    probe: Option<(i32, i32)>,
    /// Use a previous --fit's fit.txt for the RGB pass's light brightness and room ambients.
    #[arg(long)]
    use_fit: Option<PathBuf>,
    /// With --fit: look for this many missing (baked-only) lights in the residual.
    #[arg(long, default_value_t = 0)]
    discover: usize,
    /// Fit every n-th texel.
    #[arg(long, default_value_t = 2)]
    stride: u32,
    /// Leave the level's point lights out of the lightmap (RGB) pass.
    #[arg(long)]
    no_point_rgb: bool,
}

fn main() -> Result<()> {
    env_logger::init();
    let args = Args::parse();
    let install = DaoInstall::open(&args.install)?;
    let mut vfs = install.play_vfs(TextureQuality::High)?;
    let level = install.mount_level(&mut vfs, &args.level)?.level;
    let started = Instant::now();
    let scene = AreaScene::load(&vfs, &level)?;
    let bake = BakeScene::new(&scene, &vfs);
    if let Ok(near) = std::env::var("THEDAS_BAKE_NEAR") {
        let v: Vec<f32> = near.split(',').filter_map(|x| x.parse().ok()).collect();
        let p = Vec3::new(v[0], v[1], v[2]);
        for (i, d) in scene.draws.iter().enumerate() {
            let dist = (d.sphere.0 - p).length() - d.sphere.1;
            if dist < 0.5 {
                eprintln!(
                    "near: draw {i} {} {} sphere {:?} static {}",
                    d.model,
                    scene.meshes[d.mesh].key,
                    d.sphere,
                    i < scene.static_draws
                );
            }
        }
    }
    eprintln!(
        "{level}: {} triangles ({} non-casting draws left out), {} regions in {} atlases, {} lights (loaded in {:.1}s)",
        bake.bvh.tris.len(),
        bake.non_casters,
        bake.regions.len(),
        bake.atlases.len(),
        scene.lights.len(),
        started.elapsed().as_secs_f32()
    );
    std::fs::create_dir_all(&args.out)?;
    let settings = Settings {
        threads: args.threads,
        ao_rays: args.ao_rays,
        ao_length: args.ao_length,
        ao_converged: args.ao_converged,
        dilate: args.dilate,
        light_size: args.light_size.map(|(r, n)| (r, n as u32)),
        ao_model: args.ao_model.as_deref().map(|s| {
            let v: Vec<f32> = s.split(',').filter_map(|x| x.trim().parse().ok()).collect();
            thedas_bake::bake::AoModel {
                length: v.first().copied().unwrap_or(1.5),
                exponent: v.get(1).copied().unwrap_or(2.0),
                lo: v.get(2).copied().unwrap_or(-1.0),
                power: v.get(3).copied().unwrap_or(1.0),
            }
        }),
        shadow_model: match args.shadow_intensity {
            Some((k, e)) => thedas_bake::bake::ShadowModel::Intensity { k, e },
            None => thedas_bake::bake::ShadowModel::Monochrome,
        },
        ..Settings::default()
    };

    // The lights: every point light of the level (all are static ones: shadow pass, and the
    // lightmap pass unless --no-point-rgb), the sun, and each room's ambient light.
    let mut points = Vec::new();
    let mut names = Vec::new();
    let mut ambient: BTreeMap<String, Vec3> = BTreeMap::new();
    for l in &scene.lights {
        match l {
            SceneLight::Point {
                position,
                color,
                radius,
                intensity,
                name,
                domain,
                ..
            } if *domain != Some(thedas_render::scene::DOMAIN_CHARACTERS) => points.push({
                names.push(name.clone());
                BakeLight {
                    kind: LightKind::Point {
                        position: *position,
                        radius: *radius,
                    },
                    color: *color * *intensity,
                    brightness: args.brightness,
                    in_lightmap: !args.no_point_rgb,
                    cast_shadows: true,
                }
            }),
            SceneLight::Ambient { color, room, .. } => {
                ambient.insert(room.to_ascii_lowercase(), *color);
            }
            SceneLight::Point { .. } => {}
        }
    }
    if let Some((dir, color)) = scene.sun {
        names.push("sun".into());
        points.push(BakeLight {
            kind: LightKind::Directional { toward: -dir },
            color,
            brightness: 1.0,
            in_lightmap: !args.no_point_rgb,
            cast_shadows: true,
        });
    }

    // A fit's results replace the RGB pass's light brightness and the room ambients.
    if let Some(path) = &args.use_fit {
        let text = std::fs::read_to_string(path)?;
        for line in text.lines() {
            let f: Vec<&str> = line.split_whitespace().collect();
            match f.as_slice() {
                ["ambient", room, r, g, b, ..] => {
                    let c = Vec3::new(r.parse()?, g.parse()?, b.parse()?);
                    ambient.insert(room.to_string(), c);
                }
                ["found", x, y, z, r, cr, cg, cb] => {
                    points.push(BakeLight {
                        kind: LightKind::Point {
                            position: Vec3::new(x.parse()?, y.parse()?, z.parse()?),
                            radius: r.parse()?,
                        },
                        color: Vec3::new(cr.parse()?, cg.parse()?, cb.parse()?),
                        brightness: 1.0,
                        in_lightmap: true,
                        cast_shadows: false,
                    });
                    names.push("found".into());
                }
                ["light", i, k, ..] => {
                    let (i, k): (usize, f32) = (i.parse()?, k.parse()?);
                    if let Some(l) = points.get_mut(i) {
                        l.brightness = k;
                    }
                }
                _ => {}
            }
        }
    }
    if args.fit {
        return fit(
            &args, &vfs, &scene, &bake, &points, &names, &ambient, &settings,
        );
    }
    let mut report = String::new();
    let mut total = Stats::default();
    for (atlas, regions) in &bake.atlases {
        if let Some(f) = &args.atlas
            && !atlas.contains(f.as_str())
        {
            continue;
        }
        let Ok((data, _)) = vfs.read(atlas) else {
            eprintln!("{atlas}: not found");
            continue;
        };
        let dds = Dds::parse(&data).with_context(|| atlas.clone())?;
        let (aw, ah, _) = dds.mip_size(0);
        let shipped = dds.decode_rgba8(0, 0)?;
        let mut ours = vec![0u8; (aw * ah * 4) as usize];
        let mut mask = vec![false; (aw * ah) as usize];
        let mut placed = vec![false; (aw * ah) as usize];
        let t0 = Instant::now();
        let mut unplaced = 0;
        for &ri in regions {
            let r = &bake.regions[ri];
            let Some(pl) = placement(r.offset, r.scale, aw, ah) else {
                unplaced += 1;
                continue;
            };
            if std::env::var_os("THEDAS_BAKE_REGIONS").is_some() {
                let (mut lo, mut hi) = (glam::Vec2::splat(f32::MAX), glam::Vec2::splat(f32::MIN));
                for (_, uv) in &r.tris {
                    for u in uv {
                        lo = lo.min(*u);
                        hi = hi.max(*u);
                    }
                }
                let d = &scene.draws[r.draw];
                eprintln!(
                    "  region {ri} draw {} {} mesh {} tris {} at {:?} uv {lo:?}..{hi:?}",
                    r.draw,
                    d.model,
                    scene.meshes[d.mesh].key,
                    r.tris.len(),
                    pl
                );
            }
            let room = scene.draws[r.draw].room.to_ascii_lowercase();
            let mut lights = points.clone();
            if let Some(c) = ambient.get(&room) {
                lights.push(BakeLight {
                    kind: LightKind::Ambient,
                    color: *c,
                    brightness: 1.0,
                    in_lightmap: true,
                    cast_shadows: false,
                });
            }
            if let Some((px, py)) = args.probe {
                let (x, y) = (px - pl.x0, py - pl.y0);
                if x >= 0 && y >= 0 && (x as u32) < pl.w && (y as u32) < pl.h {
                    let uv = glam::Vec2::new(
                        (x as f32 + 0.5) / pl.w as f32,
                        (y as f32 + 0.5) / pl.h as f32,
                    );
                    eprintln!(
                        "probe {px},{py} region {ri} ({}):\n{}",
                        scene.draws[r.draw].model,
                        thedas_bake::bake::probe(&bake, r, uv, &lights, &settings, &|d| {
                            let d = &scene.draws[d as usize];
                            format!("{} [{}] {}", d.model, d.room, scene.meshes[d.mesh].key)
                        })
                    );
                }
                continue;
            }
            let films = bake_region(&bake, r, pl.w, pl.h, &lights, &settings);
            for y in 0..pl.h {
                for x in 0..pl.w {
                    let (ax, ay) = (pl.x0 + x as i32, pl.y0 + y as i32);
                    if ax < 0 || ay < 0 || ax >= aw as i32 || ay >= ah as i32 {
                        continue;
                    }
                    let i = (y * pl.w + x) as usize;
                    let at = (ay as u32 * aw + ax as u32) as usize;
                    placed[at] = true;
                    if !films.covered[i] {
                        continue;
                    }
                    ours[at * 4..at * 4 + 4].copy_from_slice(&films.rgba(i));
                    mask[at] = true;
                }
            }
        }
        let mut stats = Stats::default();
        let (mut extra, mut missing) = (0u64, 0u64);
        for (i, &m) in mask.iter().enumerate() {
            let zero = shipped[i * 4..i * 4 + 4] == [0, 0, 0, 0];
            if m && zero {
                extra += 1;
            }
            if placed[i] && !m && !zero {
                missing += 1;
            }
            if m {
                let o = [
                    ours[i * 4],
                    ours[i * 4 + 1],
                    ours[i * 4 + 2],
                    ours[i * 4 + 3],
                ];
                let s = [
                    shipped[i * 4],
                    shipped[i * 4 + 1],
                    shipped[i * 4 + 2],
                    shipped[i * 4 + 3],
                ];
                stats.add(o, s);
            }
        }
        let line = format!(
            "{atlas:28} {aw}x{ah} {} regions ({unplaced} unplaced) {:.1}s; {extra} of ours are black in the shipped atlas, {missing} shipped texels we leave uncovered\n  {}\n",
            regions.len(),
            t0.elapsed().as_secs_f32(),
            stats.line()
        );
        eprint!("{line}");
        report.push_str(&line);
        total.merge(&stats);
        let stem = atlas.trim_end_matches(".dds");
        if std::env::var_os("THEDAS_BAKE_COVERAGE").is_some() {
            // Red: ours covers; green: shipped is not black; blue: inside a placed region.
            let mut img = vec![0u8; (aw * ah * 3) as usize];
            for i in 0..(aw * ah) as usize {
                img[i * 3] = if mask[i] { 255 } else { 0 };
                img[i * 3 + 1] = if shipped[i * 4..i * 4 + 4] != [0, 0, 0, 0] {
                    255
                } else {
                    0
                };
                img[i * 3 + 2] = if placed[i] { 96 } else { 0 };
            }
            let file = std::fs::File::create(args.out.join(format!("{stem}-coverage.png")))?;
            let mut enc = png::Encoder::new(std::io::BufWriter::new(file), aw, ah);
            enc.set_color(png::ColorType::Rgb);
            enc.set_depth(png::BitDepth::Eight);
            enc.write_header()?.write_image_data(&img)?;
        }
        write_side_by_side(
            &args.out.join(format!("{stem}.png")),
            aw,
            ah,
            &ours,
            &shipped,
            &mask,
        )?;
    }
    let line = format!("TOTAL\n  {}\n", total.line());
    eprint!("{line}");
    report.push_str(&line);
    std::fs::write(args.out.join("report.txt"), report)?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn fit(
    args: &Args,
    vfs: &thedas_vfs::Vfs,
    scene: &AreaScene,
    bake: &BakeScene,
    lights: &[BakeLight],
    names: &[String],
    ambient: &BTreeMap<String, Vec3>,
    settings: &Settings,
) -> Result<()> {
    use thedas_bake::fit;
    // Every room with lightmapped pieces gets a fitted ambient, whether or not its light model
    // has a runtime one.
    let room_names: std::collections::BTreeSet<String> = ambient
        .keys()
        .cloned()
        .chain(
            bake.regions
                .iter()
                .map(|r| scene.draws[r.draw].room.to_ascii_lowercase()),
        )
        .collect();
    let rooms: Vec<&String> = room_names.iter().collect();
    let mut rows = Vec::new();
    for (atlas, regions) in &bake.atlases {
        if let Some(f) = &args.atlas
            && !atlas.contains(f.as_str())
        {
            continue;
        }
        let Ok((data, _)) = vfs.read(atlas) else {
            continue;
        };
        let dds = Dds::parse(&data)?;
        let (aw, ah, _) = dds.mip_size(0);
        let shipped = dds.decode_rgba8(0, 0)?;
        for &ri in regions {
            let r = &bake.regions[ri];
            let Some(pl) = placement(r.offset, r.scale, aw, ah) else {
                continue;
            };
            let room = scene.draws[r.draw].room.to_ascii_lowercase();
            let room = rooms.iter().position(|n| **n == room).map(|i| i as u32);
            rows.extend(fit::rows(
                bake,
                r,
                pl,
                &shipped,
                aw,
                lights,
                room,
                settings,
                args.stride,
            ));
        }
        eprintln!("{atlas}: {} rows so far", rows.len());
    }
    // The shadow pass against alpha.
    let (mut tp, mut tn, mut fp, mut fneg) = (0, 0, 0, 0);
    let mut ao = Stats::default();
    for r in &rows {
        let ours = r
            .terms
            .iter()
            .any(|&(i, _)| lights[i as usize].cast_shadows);
        let theirs = r.shipped[3] >= 8;
        match (ours, theirs) {
            (true, true) => tp += 1,
            (false, false) => tn += 1,
            (true, false) => fp += 1,
            (false, true) => fneg += 1,
        }
        if ours && theirs {
            let a = (255.0 * r.open.powf(1.0 / settings.gamma)) as u8;
            ao.add([0, 0, 0, a], [0, 0, 0, r.shipped[3]]);
        }
    }
    println!(
        "shadow pass vs alpha≥8: both lit {tp}, both dark {tn}, ours only {fp}, shipped only {fneg} of {}",
        rows.len()
    );
    let mut culprits: BTreeMap<usize, usize> = BTreeMap::new();
    for r in &rows {
        if r.shipped[3] < 8 {
            for &(i, _) in &r.terms {
                *culprits.entry(i as usize).or_default() += 1;
            }
        }
    }
    for r in rows
        .iter()
        .filter(|r| r.shipped[3] < 8 && r.terms.iter().map(|t| t.1).sum::<f32>() > 0.3)
        .step_by(41)
        .take(12)
    {
        println!(
            "  strongly lit by us, dark in alpha at {:?}: shipped {:?}, terms {:?}",
            r.atlas_xy, r.shipped, r.terms
        );
    }
    let mut culprits: Vec<_> = culprits.into_iter().collect();
    culprits.sort_by_key(|c| std::cmp::Reverse(c.1));
    for (i, n) in culprits.iter().take(10) {
        println!(
            "  lit by us, dark in alpha: {:28} {n}",
            names.get(*i).map(String::as_str).unwrap_or("?")
        );
    }
    for scale in [1.0f32, 0.95, 0.9, 0.85, 0.8, 0.75, 0.7, 0.6, 0.5] {
        let (mut ok, mut bad) = (0, 0);
        for r in &rows {
            let ours = r.reach.iter().any(|&(_, q)| q < scale);
            if ours == (r.shipped[3] >= 8) {
                ok += 1;
            } else {
                bad += 1;
            }
        }
        println!("  shadow pass with radius × {scale}: {ok} agree, {bad} differ");
    }
    // Shipped alpha against the summed unshadowed-by-us light (max channel), in buckets.
    let mut buckets = vec![(0u64, 0f64, 0f64); 24];
    for r in &rows {
        let mut sum = Vec3::ZERO;
        for &(i, t) in &r.terms {
            sum += lights[i as usize].color * t;
        }
        let m = sum.max_element();
        let b = ((m * 8.0) as usize).min(23);
        buckets[b].0 += 1;
        buckets[b].1 += r.shipped[3] as f64;
        buckets[b].2 += r.open as f64;
    }
    for r in rows
        .iter()
        .filter(|r| r.open < 0.3 && r.shipped[3] > 200)
        .step_by(97)
        .take(8)
    {
        println!(
            "  dark-AO example at {:?}: open {:.2} shipped {:?}",
            r.atlas_xy, r.open, r.shipped
        );
    }
    for (b, (n, a, o)) in buckets.iter().enumerate() {
        if *n > 0 {
            println!(
                "  light max-channel {:.3}..{:.3}: {n:7} texels, mean shipped alpha {:6.1}, mean our open {:.3}",
                b as f32 / 8.0,
                (b + 1) as f32 / 8.0,
                a / *n as f64,
                o / *n as f64
            );
        }
    }
    // Shipped alpha over our occlusion, against the summed falloff × cosine of the visible
    // lights (no colour), for open texels.
    let mut b2 = vec![(0u64, 0f64); 30];
    for r in &rows {
        if r.open < 0.85 {
            continue;
        }
        let sum: f32 = r.terms.iter().map(|t| t.1).sum();
        let b = ((sum * 20.0) as usize).min(29);
        b2[b].0 += 1;
        b2[b].1 += r.shipped[3] as f64 / 255.0 / (r.open as f64).powf(1.0 / 1.5);
    }
    for (b, (n, a)) in b2.iter().enumerate() {
        if *n > 0 {
            println!(
                "  falloff×cos sum {:.2}..{:.2}: {n:6} open texels, shipped alpha / ao {:.3}",
                b as f32 / 20.0,
                (b + 1) as f32 / 20.0,
                a / *n as f64
            );
        }
    }
    // Occlusion alone: where the lights are strong enough that the shadow pass saturates.
    {
        let sat: Vec<&fit::Row> = rows
            .iter()
            .filter(|r| r.terms.iter().map(|t| t.1).sum::<f32>() >= 0.45)
            .collect();
        for e in [0.5f32, 0.667, 1.0] {
            let (mut se, mut bias) = (0.0, 0.0);
            for r in &sat {
                let d = 255.0 * r.open.powf(e) as f64 - r.shipped[3] as f64;
                se += d * d;
                bias += d;
            }
            let n = sat.len().max(1) as f64;
            println!(
                "  saturated texels {}: alpha vs 255·open^{e}: RMSE {:.1} bias {:+.1}",
                sat.len(),
                (se / n).sqrt(),
                bias / n
            );
        }
    }
    // Occlusion models on the saturated texels: visibility v from the hit distances, then
    // colour = clamp(lo + (1 − lo)·v), then the film's power.
    {
        let sat: Vec<&fit::Row> = rows
            .iter()
            .filter(|r| r.terms.iter().map(|t| t.1).sum::<f32>() >= 0.45 && r.shipped[3] >= 24)
            .collect();
        println!(
            "  occlusion models over {} saturated, lit texels",
            sat.len()
        );
        {
            let n = sat.len().max(1) as f64;
            let mean = sat.iter().map(|r| r.shipped[3] as f64).sum::<f64>() / n;
            let rmse255 = (sat
                .iter()
                .map(|r| (255.0 - r.shipped[3] as f64).powi(2))
                .sum::<f64>()
                / n)
                .sqrt();
            let mut hist = [0usize; 8];
            for r in &sat {
                hist[(r.shipped[3] / 32) as usize] += 1;
            }
            println!(
                "  shipped alpha there: mean {mean:.1}, RMSE vs 255 {rmse255:.1}, histogram by 32s {hist:?}"
            );
        }
        let mut results = Vec::new();
        for (dist, cosine) in [
            ("cosine", 0),
            ("uniform", 1),
            ("cosine45", 2),
            ("uniform45", 3),
        ] {
            for (wname, wi) in [
                ("binary L", 0),
                ("(1-t/L)^1", 1),
                ("(1-t/L)^2", 2),
                ("(1-t/L)^0.5", 3),
            ] {
                for li in [0.5f32, 1.0, 1.5, 2.0, 3.0, 5.0] {
                    for lo in [0.0f32, -0.5, -1.0, -1.5, -2.0] {
                        for pw in [1.0f32, 1.0 / 1.5, 0.5] {
                            let (mut se, mut n) = (0.0f64, 0);
                            for r in &sat {
                                let hits = match cosine {
                                    0 => &r.ao_cos,
                                    1 => &r.ao_uni,
                                    2 => &r.ao_cos45,
                                    _ => &r.ao_uni45,
                                };
                                let occ: f32 = hits
                                    .iter()
                                    .map(|&t| {
                                        if t >= li {
                                            0.0
                                        } else {
                                            let x = 1.0 - t / li;
                                            match wi {
                                                0 => 1.0,
                                                1 => x,
                                                2 => x * x,
                                                _ => x.sqrt(),
                                            }
                                        }
                                    })
                                    .sum::<f32>()
                                    / hits.len() as f32;
                                let v = 1.0 - occ;
                                let c = (lo + (1.0 - lo) * v).clamp(0.0, 1.0).powf(pw);
                                let d = 255.0 * c as f64 - r.shipped[3] as f64;
                                se += d * d;
                                n += 1;
                            }
                            results.push(((se / n.max(1) as f64).sqrt(), dist, wname, li, lo, pw));
                        }
                    }
                }
            }
        }
        results.sort_by(|a, b| a.0.total_cmp(&b.0));
        for r in results.iter().take(12) {
            println!(
                "  occlusion model RMSE {:.2}: {} {} L {} lo {} power {:.3}",
                r.0, r.1, r.2, r.3, r.4, r.5
            );
        }
    }
    // Shadow pass falloff shapes, with the fitted occlusion model, over every texel.
    {
        let ao_of = |r: &fit::Row| -> f32 {
            let (len, ex, lo) = (1.5f32, 2.0f32, -1.0f32);
            let occ: f32 = r
                .ao_cos
                .iter()
                .map(|&t| {
                    if t >= len {
                        0.0
                    } else {
                        (1.0 - t / len).powf(ex)
                    }
                })
                .sum::<f32>()
                / r.ao_cos.len() as f32;
            (lo + (1.0 - lo) * (1.0 - occ)).clamp(0.0, 1.0)
        };
        let aos: Vec<f32> = rows.iter().map(ao_of).collect();
        let mut results = Vec::new();
        for (gname, g) in [
            ("linear", 0),
            ("quadratic", 1),
            ("cos²", 2),
            ("smoothstep", 3),
        ] {
            for rs in [1.0f32, 0.9, 0.8] {
                for ki in 1..=24 {
                    let k = ki as f32 * 0.5;
                    for ei in 3..=10 {
                        let e = ei as f32 * 0.1;
                        let mut se = 0.0f64;
                        for (r, ao) in rows.iter().zip(&aos) {
                            let mut sum = 0.0f32;
                            for (&(_, t), &(_, q)) in r.terms.iter().zip(&r.reach) {
                                let lin = 1.0 - q;
                                let cosv = if lin > 0.0 { t / lin } else { 0.0 };
                                let q = q / rs;
                                if q >= 1.0 {
                                    continue;
                                }
                                let f = match g {
                                    0 => 1.0 - q,
                                    1 => (1.0 - q) * (1.0 - q),
                                    2 => (std::f32::consts::FRAC_PI_2 * q).cos().powi(2),
                                    _ => {
                                        let x = 1.0 - q;
                                        x * x * (3.0 - 2.0 * x)
                                    }
                                };
                                sum += f * cosv;
                            }
                            let sm = (k * sum).min(1.0).powf(e);
                            let d = 255.0 * (ao * sm) as f64 - r.shipped[3] as f64;
                            se += d * d;
                        }
                        results.push(((se / rows.len().max(1) as f64).sqrt(), gname, rs, k, e));
                    }
                }
            }
        }
        results.sort_by(|a, b| a.0.total_cmp(&b.0));
        for r in results.iter().take(10) {
            println!(
                "  shadow falloff RMSE {:.2}: {} radius×{} k {} e {:.1}",
                r.0, r.1, r.2, r.3, r.4
            );
        }
    }
    // Grid search: sm = min(1, k · Σ w_l t_l)^e, alpha = 255 · ao^(1/γ) · sm.
    let weight = |mode: usize, c: Vec3| -> f32 {
        match mode {
            0 => 1.0,
            1 => c.max_element(),
            2 => 0.299 * c.x + 0.587 * c.y + 0.114 * c.z,
            _ => (c.x + c.y + c.z) / 3.0,
        }
    };
    let mut best = (f64::MAX, 0usize, 0f32, 0f32);
    for mode in 0..4 {
        for ki in 1..=40 {
            let k = ki as f32 * 0.25;
            for ei in 3..=12 {
                let e = ei as f32 * 0.1;
                let mut se = 0.0;
                let mut n = 0;
                for r in rows.iter().filter(|r| r.open >= 0.85) {
                    let sum: f32 = r
                        .terms
                        .iter()
                        .map(|&(i, t)| weight(mode, lights[i as usize].color) * t)
                        .sum();
                    let sm = (k * sum).min(1.0).powf(e);
                    let pred = 255.0 * r.open.powf(1.0 / settings.gamma) * sm;
                    let d = pred as f64 - r.shipped[3] as f64;
                    se += d * d;
                    n += 1;
                }
                let rmse = (se / n.max(1) as f64).sqrt();
                if rmse < best.0 {
                    best = (rmse, mode, k, e);
                }
            }
        }
    }
    println!(
        "alpha model sm = min(1, k Σ w t)^e: best RMSE {:.2} with weight mode {} (0 one, 1 max, 2 luma, 3 mean), k {}, e {}",
        best.0, best.1, best.2, best.3
    );
    let f = ao.finish();
    println!(
        "occlusion vs alpha where both lit: MAE {:.1} bias {:+.1} RMSE {:.1}",
        f.mae[3], f.bias[3], f.rmse[3]
    );
    let open_model = |r: &fit::Row| -> f32 {
        let (len, ex, lo) = (1.5f32, 2.0f32, -1.0f32);
        let occ: f32 = r
            .ao_cos
            .iter()
            .map(|&t| {
                if t >= len {
                    0.0
                } else {
                    (1.0 - t / len).powf(ex)
                }
            })
            .sum::<f32>()
            / r.ao_cos.len() as f32;
        (lo + (1.0 - lo) * (1.0 - occ)).clamp(0.0, 1.0)
    };
    type Picker<'a> = (&'a str, Box<dyn Fn(&fit::Row) -> bool + 'a>);
    let pickers: [Picker; 2] = [
        ("alpha ≥ 250", Box::new(|r: &fit::Row| r.shipped[3] >= 250)),
        (
            "modelled occlusion ≥ 0.98",
            Box::new(|r: &fit::Row| open_model(r) >= 0.98),
        ),
    ];
    let mut out = String::new();
    let mut last = None;
    for (pname, pick) in pickers.iter() {
        let lf = fit::fit_lights(&rows, lights, rooms.len(), settings.gamma, pick.as_ref());
        println!(
            "lightmap fit on {pname}: {} texels, RMSE {:.2} levels",
            lf.texels, lf.rmse_levels
        );
        out.clear();
        for (i, room) in rooms.iter().enumerate() {
            if lf.ambient[i] == Vec3::ZERO {
                continue;
            }
            out.push_str(&format!(
                "ambient {room} {} {} {}\n",
                lf.ambient[i].x, lf.ambient[i].y, lf.ambient[i].z
            ));
        }
        for (i, k) in lf.light_scale.iter().enumerate() {
            out.push_str(&format!(
                "light {i} {k} {}\n",
                names.get(i).map(String::as_str).unwrap_or("?")
            ));
        }
        for (i, room) in rooms.iter().enumerate() {
            println!(
                "  ambient {room}: fitted {:.4} {:.4} {:.4}  data {:?}",
                lf.ambient[i].x,
                lf.ambient[i].y,
                lf.ambient[i].z,
                ambient.get(*room)
            );
        }
        let mut seen: Vec<(usize, usize)> = (0..lights.len())
            .map(|i| {
                (
                    i,
                    rows.iter()
                        .filter(|r| r.terms.iter().any(|t| t.0 as usize == i))
                        .count(),
                )
            })
            .filter(|x| x.1 > 0)
            .collect();
        seen.sort_by_key(|x| std::cmp::Reverse(x.1));
        for (i, n) in seen.iter().take(40) {
            let l = &lights[*i];
            let extra = match l.kind {
                LightKind::Point { radius, position } => format!("r {radius:.1} at {position}"),
                _ => String::new(),
            };
            println!(
                "  {:28} texels {n:7} scale {:.3} colour {:.3} {:.3} {:.3} {extra}",
                names.get(*i).map(String::as_str).unwrap_or("?"),
                lf.light_scale[*i],
                l.color.x,
                l.color.y,
                l.color.z
            );
        }
        last = Some(lf);
    }
    // Missing lights: what the residual RGB of unoccluded texels asks for.
    if let (Some(lf), true) = (&last, args.discover > 0) {
        let picked: Vec<&fit::Row> = rows.iter().filter(|r| open_model(r) >= 0.98).collect();
        let step = (picked.len() / 6000).max(1);
        let picked: Vec<&fit::Row> = picked.into_iter().step_by(step).collect();
        let mut residual: Vec<Vec3> = picked
            .iter()
            .map(|r| {
                let mut v = r
                    .room
                    .map(|room| lf.ambient[room as usize])
                    .unwrap_or(Vec3::ZERO);
                for &(i, t) in &r.terms {
                    v += lights[i as usize].color * (lf.light_scale[i as usize] as f32 * t);
                }
                let y = Vec3::new(
                    fit::linear(r.shipped[0], settings.gamma) as f32,
                    fit::linear(r.shipped[1], settings.gamma) as f32,
                    fit::linear(r.shipped[2], settings.gamma) as f32,
                );
                y - v
            })
            .collect();
        let before: f64 = residual.iter().map(|r| r.length_squared() as f64).sum();
        let found = fit::discover(
            &bake.bvh,
            &picked,
            &mut residual,
            settings,
            args.discover,
            2.0,
            4.0,
        );
        let after: f64 = residual.iter().map(|r| r.length_squared() as f64).sum();
        println!(
            "discovered {} lights over {} texels: residual {:.3} -> {:.3}",
            found.len(),
            picked.len(),
            before,
            after
        );
        for f in &found {
            println!(
                "  found at {} r {} colour {:.3} {:.3} {:.3} gain {:.3}",
                f.position, f.radius, f.color.x, f.color.y, f.color.z, f.gain
            );
            out.push_str(&format!(
                "found {} {} {} {} {} {} {}\n",
                f.position.x, f.position.y, f.position.z, f.radius, f.color.x, f.color.y, f.color.z
            ));
        }
    }
    std::fs::create_dir_all(&args.out)?;
    std::fs::write(args.out.join("fit.txt"), out)?;
    Ok(())
}

fn parse_ke(s: &str) -> Result<(f32, f32), String> {
    let (k, e) = s.split_once(',').ok_or("expected k,e")?;
    Ok((
        k.trim().parse().map_err(|e| format!("{e}"))?,
        e.trim().parse().map_err(|e| format!("{e}"))?,
    ))
}

fn parse_xy(s: &str) -> Result<(i32, i32), String> {
    let (x, y) = s.split_once(',').ok_or("expected x,y")?;
    Ok((
        x.trim().parse().map_err(|e| format!("{e}"))?,
        y.trim().parse().map_err(|e| format!("{e}"))?,
    ))
}

/// Ours | shipped | |delta|×4, RGB above alpha.
fn write_side_by_side(
    path: &Path,
    w: u32,
    h: u32,
    ours: &[u8],
    shipped: &[u8],
    mask: &[bool],
) -> Result<()> {
    let (ow, oh) = (w * 3, h * 2);
    let mut img = vec![0u8; (ow * oh * 3) as usize];
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) as usize;
            let o = &ours[i * 4..i * 4 + 4];
            let s = &shipped[i * 4..i * 4 + 4];
            let d: Vec<u8> = (0..4)
                .map(|c| {
                    if mask[i] {
                        ((o[c] as i32 - s[c] as i32).unsigned_abs() * 4).min(255) as u8
                    } else {
                        0
                    }
                })
                .collect();
            let put = |img: &mut [u8], px: u32, py: u32, rgb: [u8; 3]| {
                let at = ((py * ow + px) * 3) as usize;
                img[at..at + 3].copy_from_slice(&rgb);
            };
            put(&mut img, x, y, [o[0], o[1], o[2]]);
            put(&mut img, x + w, y, [s[0], s[1], s[2]]);
            put(&mut img, x + 2 * w, y, [d[0], d[1], d[2]]);
            put(&mut img, x, y + h, [o[3]; 3]);
            put(&mut img, x + w, y + h, [s[3]; 3]);
            put(&mut img, x + 2 * w, y + h, [d[3]; 3]);
        }
    }
    let file = std::fs::File::create(path)?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), ow, oh);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(&img)?;
    Ok(())
}
