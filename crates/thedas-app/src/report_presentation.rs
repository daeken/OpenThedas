//! Small, read-only snapshots taken before the report prompt changes presentation.
use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value as J, json};
use thedas_render::{Animator, Clip};
use thedas_ui::UiValue;

use super::FrameContext;

pub fn frame_context(app: &crate::App) -> FrameContext {
    let mut context = FrameContext {
        loading: app.load_screen_up || app.gfx.as_ref().is_some_and(|g| g.ui.loading()),
        screens: app
            .screens
            .as_ref()
            .map(|s| s.shown().iter().copied().collect())
            .unwrap_or_default(),
        ..Default::default()
    };
    if let Some(live) = &app.live {
        let sim = live.sim();
        context.area = sim
            .world
            .area
            .and_then(|a| sim.world.get(a))
            .map(|a| a.tag.clone());
        context.time_ms = Some(sim.world.time_ms);
        context.cutscene = sim.world.cutscene.is_some();
        context.conversation = sim.dialog_state().is_some_and(|d| !d.ended);
        context.loading |= live.loading();
        context.draw_counts = Some(live.report_draw_counts());
    }
    context
}

pub fn snapshot(app: &crate::App) -> J {
    let context = frame_context(app);
    let staged = app
        .live
        .as_ref()
        .is_some_and(|l| l.sim().world.chargen.is_some());
    let mode = if app.follow.is_none() {
        "free"
    } else if staged {
        "chargen"
    } else if context.cutscene {
        "cutscene"
    } else if context.conversation {
        "conversation"
    } else {
        "explore"
    };
    let pivot = app
        .follow
        .as_ref()
        .filter(|_| mode == "explore")
        .and_then(|f| f.target());
    let camera = &app.camera;
    let interface = app.gfx.as_ref().map(|g| {
        let tokens: BTreeSet<_> =
            g.ui.stage
                .movies
                .iter()
                .flat_map(|m| m.observed_tokens())
                .collect();
        let values =
            g.ui.stage
                .bridge
                .inspect_values(|values| ui_values(values, &tokens));
        json!({
            "movies": g.ui.stage.movies.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(),
            "contexts": g.ui.stage.bridge.contexts(), "dialog_up": g.ui.stage.dialog_up(),
            "screens": context.screens, "values": values,
        })
    });
    json!({
        "schema": 1, "captured": "at report press, before prompts/pause", "frame_context": context,
        "camera": {
            "mode": mode, "position": camera.position.to_array(),
            "target": pivot.unwrap_or(camera.position + camera.forward()).to_array(),
            "target_kind": if pivot.is_some() { "explore_pivot" } else { "view_direction_point_1m" },
            "yaw_radians": camera.yaw, "pitch_radians": camera.pitch, "fov_y_radians": camera.fov_y,
            "zoom": app.follow.as_ref().map(|f| f.zoom), "near": camera.near, "far": camera.far,
        },
        "world": app.live.as_ref().map(|l| l.report_presentation(camera.position)),
        "sounds": app.audio.as_ref().map(|a| a.report_sounds()), "interface": interface,
    })
}

pub fn clip_snapshot(animator: &Animator, clip: &Clip) -> J {
    let resolved = animator.cached_animation_resource(clip);
    let animation = resolved.as_ref().map(|(_, a)| a);
    let time = animation.map(|a| {
        if let Some(phase) = clip.phase {
            phase * (a.length - 1.0 / 30.0) + 1.0 / 30.0
        } else if clip.looping && a.length > 0.0 {
            clip.time.rem_euclid(a.length)
        } else {
            clip.time.min(a.length)
        }
    });
    json!({
        "trees": clip.trees, "actions": clip.actions, "resource": resolved.as_ref().map(|(name, _)| name),
        "inline": clip.inline.is_some(), "clock_s": clip.time, "time_s": time,
        "frame_30fps": time.map(|t| t * 30.0), "length_s": animation.map(|a| a.length),
        "length_frames_30fps": animation.map(|a| a.length * 30.0),
        "looping": clip.looping, "blend_weight": clip.weight, "absolute": clip.absolute,
        "additive": animation.map(|a| a.additive), "phase": clip.phase,
        "layers": clip.layers.iter().map(|c| clip_snapshot(animator, c)).collect::<Vec<_>>(),
    })
}

fn ui_values(values: &BTreeMap<String, UiValue>, observed: &BTreeSet<&str>) -> J {
    const LIMIT: usize = 1024;
    let mut budget = 8192;
    let mut truncated = false;
    let names: BTreeSet<&str> = values
        .keys()
        .map(String::as_str)
        .filter(|k| {
            observed.contains(k)
                || k.starts_with("GUI.")
                || k.starts_with("Party.")
                || k.starts_with("ClientOptions.")
        })
        .filter(|k| !super::config::sensitive_key(k))
        .collect();
    let mut result = serde_json::Map::new();
    // Observed tokens first: a large inventory must not crowd out the HUD's values.
    for name in observed.iter().copied().chain(names.iter().copied()) {
        if result.len() >= LIMIT || budget == 0 {
            break;
        }
        if result.contains_key(name) || super::config::sensitive_key(name) {
            continue;
        }
        if let Some(value) = values.get(name) {
            result.insert(
                name.chars().take(256).collect(),
                ui_value(value, &mut budget, 0, &mut truncated),
            );
        }
    }
    json!({ "limit": LIMIT, "node_limit": 8192, "string_limit": 512,
        "candidate_count": names.len(), "captured_count": result.len(),
        "truncated": truncated || result.len() < names.len() || budget == 0, "items": result })
}

fn ui_value(value: &UiValue, budget: &mut usize, depth: usize, truncated: &mut bool) -> J {
    if *budget == 0 || depth >= 8 {
        *truncated = true;
        return json!({ "truncated": true });
    }
    *budget -= 1;
    match value {
        UiValue::Undefined | UiValue::Null => J::Null,
        UiValue::Bool(v) => json!(v),
        UiValue::Number(v) => json!(v),
        UiValue::String(v) => {
            *truncated |= v.chars().nth(512).is_some();
            json!(v.chars().take(512).collect::<String>())
        }
        UiValue::List(list) => {
            *truncated |= list.len() > 64;
            let mut out = Vec::new();
            for item in list.iter().take(64) {
                if *budget == 0 {
                    break;
                }
                out.push(ui_value(item, budget, depth + 1, truncated));
            }
            json!({ "items": out, "total": list.len() })
        }
        UiValue::Object(object) => {
            *truncated |= object.len() > 64;
            let mut out = serde_json::Map::new();
            for (key, value) in object
                .iter()
                .filter(|(k, _)| !super::config::sensitive_key(k))
                .take(64)
            {
                if *budget == 0 {
                    break;
                }
                out.insert(
                    key.chars().take(256).collect(),
                    ui_value(value, budget, depth + 1, truncated),
                );
            }
            json!({ "items": out, "total": object.len() })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn animation_snapshot_wraps_clamps_and_keeps_blend_layers() {
        let animator = Animator::default();
        let animation = std::sync::Arc::new(thedas_formats::anim::Animation {
            name: "fixture.ani".into(),
            length: 2.0,
            additive: true,
            tracks: vec![],
            scalars: vec![],
            colors: vec![],
        });
        let mut clip = Clip {
            time: 4.25,
            looping: true,
            inline: Some(animation.clone()),
            layers: vec![Clip {
                time: 10.0,
                weight: 0.25,
                inline: Some(animation),
                ..Default::default()
            }],
            ..Default::default()
        };
        let snapshot = clip_snapshot(&animator, &clip);
        assert_eq!(snapshot["time_s"], 0.25);
        assert_eq!(snapshot["frame_30fps"], 7.5);
        assert_eq!(snapshot["layers"][0]["time_s"], 2.0);
        assert_eq!(snapshot["layers"][0]["blend_weight"], 0.25);
        clip.phase = Some(0.5);
        assert!(
            (clip_snapshot(&animator, &clip)["time_s"].as_f64().unwrap() - 1.016667).abs()
                < 0.00001
        );
        assert!(clip_snapshot(&animator, &Clip::default())["length_s"].is_null());
    }

    #[test]
    fn hud_values_are_bounded_and_credentials_are_dropped() {
        let values = BTreeMap::from([
            ("Party.Health".into(), UiValue::Number(80.0)),
            ("GUI.Token".into(), UiValue::String("secret".into())),
            (
                "Inventory.Items".into(),
                UiValue::List(vec![UiValue::String("é".repeat(2000)); 500]),
            ),
        ]);
        let observed = BTreeSet::from(["Party.Health", "Inventory.Items", "GUI.Token"]);
        let result = ui_values(&values, &observed);
        assert_eq!(result["items"]["Party.Health"], 80.0);
        assert_eq!(result["truncated"], true);
        assert!(result["items"].get("GUI.Token").is_none());
        assert_eq!(result["items"]["Inventory.Items"]["total"], 500);
        assert_eq!(
            result["items"]["Inventory.Items"]["items"]
                .as_array()
                .unwrap()
                .len(),
            64
        );
        assert_eq!(
            result["items"]["Inventory.Items"]["items"][0]
                .as_str()
                .unwrap()
                .chars()
                .count(),
            512
        );
    }
}
