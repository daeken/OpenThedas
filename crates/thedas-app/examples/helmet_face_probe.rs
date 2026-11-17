//! Offscreen diagnostic for a copied save's hero. No simulation steps are run.
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use serde_json::json;
use thedas_render::d3d::{D3dSource, LightClass, object_filter, pick_lights};
use thedas_render::{
    Animator, AreaRenderer, AreaScene, Camera, Clip, GpuContext, ProgramRegistry, Prop, RenderCache,
};
use thedas_vfs::{DaoInstall, install::TextureQuality};

#[derive(Parser)]
struct Args {
    #[arg(long)]
    install: PathBuf,
    #[arg(long)]
    save: PathBuf,
    /// Replay the hero's recorded placement and animation instead of a neutral idle.
    #[arg(long, conflicts_with = "rest")]
    presentation: Option<PathBuf>,
    #[arg(long)]
    out: PathBuf,
    #[arg(long, allow_hyphen_values = true)]
    camera: String,
    #[arg(long)]
    no_helmet: bool,
    #[arg(long)]
    no_head: bool,
    #[arg(long)]
    unshadowed: bool,
    #[arg(long)]
    rest: bool,
    #[arg(long)]
    metadata_only: bool,
    /// Include the level's post effects (off for the pixel-isolation controls).
    #[arg(long)]
    frame_effects: bool,
}

fn main() -> Result<()> {
    env_logger::init();
    run(Args::parse()).map(|_| ())
}

fn run(args: Args) -> Result<Option<Vec<u8>>> {
    std::fs::create_dir_all(&args.out)?;
    let options = thedas_sim::session::SessionOptions {
        install: args.install.clone(),
        module: "single player".into(),
        area: None,
        waypoint: None,
        seed: 1,
        keep_trace: false,
        cutscenes: thedas_sim::sim::CutscenePolicy::Play,
        chargen: thedas_sim::sim::ChargenPolicy::QuickStart,
    };
    let mut sim = thedas_sim::session::open(&options)?;
    sim.load_json(&std::fs::read(&args.save)?)?;
    let area_id = sim.world.area.context("no area")?;
    let area = sim
        .world
        .get(area_id)
        .context("missing area")?
        .template
        .clone();
    let hero = sim.world.controlled().context("no hero")?;
    let model = thedas_ui::models::area_models(&sim, area_id)
        .into_iter()
        .find(|m| m.id == hero)
        .context("no hero model")?;
    let install = DaoInstall::open(&args.install)?;
    let mut vfs = install.play_vfs(TextureQuality::High)?;
    let (data, _) = vfs.read(&format!("{area}.are"))?;
    let layout = thedas_formats::Area::parse(&data)?
        .layout
        .context("no layout")?;
    let level = install.mount_level(&mut vfs, &layout)?.level;
    let recorded = args
        .presentation
        .as_ref()
        .map(|path| -> Result<_> {
            let data: serde_json::Value = serde_json::from_slice(&std::fs::read(path)?)?;
            let object = data["world"]["objects"]
                .as_array()
                .context("no recorded objects")?
                .iter()
                .find(|o| o["id"].as_u64() == Some(u64::from(hero)))
                .context("no recorded hero")?;
            let world: [f32; 16] = serde_json::from_value(object["world_transform"].clone())?;
            let clip = (!object["animations"].is_null())
                .then(|| recorded_clip(&object["animations"]))
                .transpose()?;
            Ok((glam::Mat4::from_cols_array(&world), clip))
        })
        .transpose()?;
    let placement = recorded.as_ref().map_or_else(
        || Prop::placement(model.position, model.facing),
        |(world, _)| *world,
    );
    let mut scene = AreaScene::load(&vfs, &level)?;
    let prop = Prop {
        label: format!("{} {}", model.id, model.tag),
        models: model
            .models
            .iter()
            .filter(|m| !args.no_helmet || !m.contains("_hlf_"))
            .cloned()
            .collect(),
        attached: model
            .attached
            .iter()
            .map(|a| (a.model.clone(), a.node.clone()))
            .collect(),
        // Keep the morph while loading even in the no-head control: its skin tone also
        // colours exposed skin on the body. Suppress only its draws after loading.
        morph: model.morph.clone(),
        hide_hair: model.hide_hair,
        tints: model.tints.clone(),
        effects: vec![],
        transform: placement,
    };
    let mut objects_scene = AreaScene::load_objects(&vfs, &level, &[prop])?;
    anyhow::ensure!(
        !args.no_head || model.morph.is_some(),
        "--no-head requires a head morph"
    );
    let body_draws: std::collections::HashSet<usize> = objects_scene
        .draws
        .iter()
        .enumerate()
        .filter(|(_, d)| {
            model
                .models
                .iter()
                .any(|m| m.eq_ignore_ascii_case(&d.model))
                || model
                    .attached
                    .iter()
                    .any(|m| m.model.eq_ignore_ascii_case(&d.model))
        })
        .map(|(i, _)| i)
        .collect();
    let draws: Vec<_> = objects_scene.draws.iter().enumerate().map(|(i,d)| {
        let mat = &objects_scene.materials[d.material];
        let mesh = &objects_scene.meshes[d.mesh];
        let lights = pick_lights(&objects_scene.lights, &objects_scene.light_rooms,
            d.sphere, d.room_id, LightClass::of(mat.render_class.as_deref()));
        json!({ "draw":i, "suppressed":args.no_head && !body_draws.contains(&i), "model":d.model, "mesh":mesh.key, "vertices":mesh.vertices.len(),
            "indices":mesh.indices.len(), "material":mat.mao, "definition":mat.object.material,
            "semantic":mat.semantic, "class":mat.render_class, "textures":mat.object.textures.iter().map(|t| {
                json!({"parameter":t.parameter, "resource":t.resource(), "requires_id":t.requires_id, "exists":vfs.read(&t.resource()).is_ok()})
            }).collect::<Vec<_>>(), "vectors":mat.object.vectors, "floats":mat.object.floats,
            "vector_arrays":mat.object.vector_arrays,
            "technique":format!("{:?}",mat.technique), "probe":d.probe,
            "room":d.room, "room_id":d.room_id, "cast_shadow":d.cast_shadow,
            "skin":format!("{:?}",d.skin), "transform":d.transform.to_cols_array(),
            "lights":format!("{:?}",lights), "sphere":format!("{:?}",d.sphere) })
    }).collect();
    std::fs::write(
        args.out.join("draws.json"),
        serde_json::to_vec_pretty(&json!({
            "save":args.save, "presentation":args.presentation, "camera":args.camera,
            "pose":if args.presentation.is_some() { "recorded" } else if args.rest { "rest" } else { "idle at 0s" },
        "no_head":args.no_head, "no_helmet":args.no_helmet, "unshadowed":args.unshadowed,
        "frame_effects":args.frame_effects,
            "models":model.models, "morph":model.morph,
            "hide_hair":model.hide_hair, "position":model.position, "facing":model.facing,
            "sun":format!("{:?}",scene.sun), "probes":format!("{:?}",objects_scene.probes),
            "lights":format!("{:?}",objects_scene.lights), "warnings":objects_scene.warnings, "draws":draws,
        }))?,
    )?;
    if args.metadata_only {
        return Ok(None);
    }
    if args.unshadowed {
        for d in scene.draws.iter_mut().chain(objects_scene.draws.iter_mut()) {
            d.cast_shadow = Some(false);
        }
    }
    let ctx = GpuContext::headless()?;
    let mut cache = RenderCache::default();
    let filter = object_filter();
    let mut renderer = |scene: &AreaScene| {
        let source = D3dSource::new_cached(&vfs, scene, &filter, &mut cache);
        if scene.static_draws == 0 {
            let mut resolved: Vec<_> = source
                .programs()
                .map(|p| json!({"program":p.name,"state":format!("{:?}",p.pass)}))
                .collect();
            resolved.sort_by(|a, b| a["program"].as_str().cmp(&b["program"].as_str()));
            std::fs::write(
                args.out.join("programs.json"),
                serde_json::to_vec_pretty(
                    &json!({"failures":source.failures,"programs":resolved}),
                )?,
            )
            .context("write programs.json")?;
        }
        let mut programs = ProgramRegistry::default();
        programs.push_front(Box::new(source));
        Ok::<_, anyhow::Error>(AreaRenderer::new_cached_with_path(
            &ctx,
            &vfs,
            scene,
            &programs,
            thedas_render::gpu::CAPTURE_FORMAT,
            &mut cache,
            thedas_render::RenderPath::Faithful,
        ))
    };
    let mut level_renderer = renderer(&scene)?;
    let mut objects = renderer(&objects_scene)?;
    if args.no_head {
        objects.only_draws = Some(body_draws);
    }
    level_renderer.frame_effects = args.frame_effects;
    level_renderer.antialias = false;
    level_renderer.set_time(0.0);
    objects.set_time(0.0);
    let mut animator = Animator::default();
    let clip = if let Some((_, clip)) = recorded {
        clip
    } else {
        (!args.rest).then(|| Clip {
            trees: vec!["humanoid.anb".into()],
            actions: vec!["pause".into()],
            looping: true,
            time: 0.0,
            translation_scale: thedas_ui::models::translation_scale(&sim, hero),
            ..Default::default()
        })
    };
    animator.pose_props(
        &vfs,
        &objects_scene,
        &mut objects,
        &ctx,
        &[(placement, clip)],
        0.0,
    );
    let camera = Camera::parse(&args.camera).context("bad camera")?;
    let pixels =
        level_renderer.capture_with(&ctx, &camera, 1280, 720, &mut [&mut objects], None)?;
    let file = std::fs::File::create(args.out.join("frame.png"))?;
    let mut encoder = png::Encoder::new(file, 1280, 720);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(&pixels)?;
    Ok(Some(pixels))
}

fn recorded_clip(value: &serde_json::Value) -> Result<Clip> {
    anyhow::ensure!(
        value["inline"] != true,
        "inline animation is not stored in presentation.json"
    );
    Ok(Clip {
        trees: serde_json::from_value(value["trees"].clone())?,
        actions: serde_json::from_value(value["actions"].clone())?,
        time: serde_json::from_value(value["clock_s"].clone())?,
        looping: serde_json::from_value(value["looping"].clone())?,
        absolute: serde_json::from_value(value["absolute"].clone())?,
        weight: serde_json::from_value(value["blend_weight"].clone())?,
        translation_scale: serde_json::from_value(value["translation_scale"].clone())?,
        phase: serde_json::from_value(value["phase"].clone())?,
        layers: value["layers"]
            .as_array()
            .context("no clip layers")?
            .iter()
            .map(recorded_clip)
            .collect::<Result<_>>()?,
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot() -> serde_json::Value {
        json!({"trees":["humanoid.anb"],"actions":["pause"],"clock_s":12.25,
            "time_s":0.25,"looping":true,"absolute":false,"blend_weight":0.75,
            "translation_scale":[0.73,0.73,0.73],"phase":null,"inline":false,"layers":[]})
    }

    #[test]
    fn replay_keeps_the_clock_and_nested_pose_inputs() {
        let mut root = snapshot();
        let mut layer = snapshot();
        layer["absolute"] = json!(true);
        layer["phase"] = json!(0.6);
        layer["translation_scale"] = serde_json::Value::Null;
        root["layers"] = json!([layer]);
        let clip = recorded_clip(&root).unwrap();
        assert_eq!(clip.time, 12.25); // Re-resolve from the clock, not already wrapped time_s.
        assert_eq!(clip.translation_scale, Some([0.73; 3]));
        assert_eq!(clip.weight, 0.75);
        assert!(clip.looping);
        assert!(clip.layers[0].absolute);
        assert_eq!(clip.layers[0].phase, Some(0.6));
        assert_eq!(clip.layers[0].translation_scale, None);
    }

    #[test]
    fn replay_rejects_unrecoverable_or_incomplete_poses() {
        let mut inline = snapshot();
        inline["inline"] = json!(true);
        assert!(
            recorded_clip(&inline)
                .unwrap_err()
                .to_string()
                .contains("inline")
        );
        let mut incomplete = snapshot();
        incomplete.as_object_mut().unwrap().remove("clock_s");
        assert!(recorded_clip(&incomplete).is_err());
    }

    #[test]
    #[ignore = "requires THEDAS_INSTALL, THEDAS_HELMET_SAVE, THEDAS_HELMET_OUT and an offscreen GPU; use the copied 02-56 Proving save"]
    fn proving_visor_shows_the_head_without_runtime_shadows() {
        let install = std::env::var_os("THEDAS_INSTALL").expect("THEDAS_INSTALL");
        let save = std::env::var_os("THEDAS_HELMET_SAVE").expect("THEDAS_HELMET_SAVE");
        let out = PathBuf::from(std::env::var_os("THEDAS_HELMET_OUT").expect("THEDAS_HELMET_OUT"));
        let args = |label: &str, no_head, unshadowed| Args {
            install: install.clone().into(),
            save: save.clone().into(),
            out: out.join(label),
            presentation: None,
            camera: "63.805,-19.745,1.41,151.806,0".into(),
            no_helmet: false,
            no_head,
            unshadowed,
            rest: false,
            metadata_only: false,
            frame_effects: false,
        };
        let baseline = run(args("baseline", false, false)).unwrap().unwrap();
        let metadata: serde_json::Value =
            serde_json::from_slice(&std::fs::read(out.join("baseline/draws.json")).unwrap())
                .unwrap();
        assert_eq!(metadata["sun"], "None");
        let programs: serde_json::Value =
            serde_json::from_slice(&std::fs::read(out.join("baseline/programs.json")).unwrap())
                .unwrap();
        assert_eq!(programs["failures"], json!([]));
        for name in [
            "face1.vsh+face0.psh+face.sta",
            "char1.vsh+ch1notnt.psh+character.sta",
        ] {
            assert!(
                programs["programs"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|p| p["program"] == name)
            );
        }
        let draws = metadata["draws"].as_array().unwrap();
        for model in [
            "df_hlf_d01a_0",
            "df_uhm_basa_0",
            "df_uem_basa_0",
            "df_ulm_basa_0",
        ] {
            assert!(draws.iter().any(|d| d["model"] == model), "missing {model}");
        }
        for model in ["df_hlf_d01a_0", "df_uhm_basa_0"] {
            let draw = draws.iter().find(|d| d["model"] == model).unwrap();
            assert!(
                draw["textures"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|t| t["exists"] == true),
                "missing {model} texture"
            );
        }
        assert!(
            !draws
                .iter()
                .any(|d| d["model"].as_str().unwrap().contains("_har_"))
        );
        let no_head = run(args("no-head", true, false)).unwrap().unwrap();
        let unshadowed = run(args("unshadowed", false, true)).unwrap().unwrap();
        let changes = |other: &[u8]| {
            baseline
                .chunks_exact(4)
                .zip(other.chunks_exact(4))
                .filter(|(a, b)| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 2))
                .count()
        };
        let visor_changes = baseline
            .chunks_exact(4)
            .zip(no_head.chunks_exact(4))
            .enumerate()
            .filter(|(i, (a, b))| {
                let (x, y) = (i % 1280, i / 1280);
                (550..720).contains(&x)
                    && (290..395).contains(&y)
                    && a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 2)
            })
            .count();
        assert!(
            visor_changes > 500,
            "the face must be visible through the visor"
        );
        assert_eq!(
            changes(&unshadowed),
            0,
            "an indoor helmet cannot self-shadow through absent sun partitions"
        );
    }
}
