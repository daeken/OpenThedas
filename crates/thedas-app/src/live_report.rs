//! Read-only diagnostics from the last scene/pose. No resource loads or GPU readbacks.
use super::*;
use serde_json::{Value, json};

impl Live {
    pub fn report_presentation(&self, eye: glam::Vec3) -> Value {
        let sim = self.sim();
        let objects = self.objects.as_ref().map(|objects| {
            let seen = objects.renderer.seen_draws();
            self.posed.iter().enumerate().filter_map(|(i, (id, world, clip))| {
                let prop = objects.scene.props.get(i)?;
                let opacity = prop.draws.clone().filter_map(|d| objects.renderer.draw_opacity(d))
                    .reduce(f32::max).unwrap_or(1.0);
                if prop.draws.is_empty() || opacity <= 0.0 {
                    return None;
                }
                let object = sim.world.get(*id);
                let position = object.map(|o| glam::Vec3::from_array(o.position));
                let roots: Vec<_> = self.animator.last_pose(i).into_iter().flat_map(|poses| {
                    let mut skeletons: Vec<_> = poses.iter().collect();
                    skeletons.sort_by_key(|(s, _)| **s);
                    skeletons.into_iter().filter_map(|(s, (rest, pose))| {
                        let skeleton = objects.scene.skeletons.get(*s)?;
                        let node = skeleton.node("root").or_else(|| skeleton.bones.iter().position(|b| b.parent.is_none()))?;
                        let rest = rest.get(node)?.w_axis.truncate();
                        let posed = pose.get(node)?.w_axis.truncate();
                        let mut root = root_snapshot(*world, position, rest, posed);
                        root["model"] = json!(objects.scene.skeleton_models.get(*s));
                        root["node"] = json!(skeleton.bones[node].name);
                        Some(root)
                    })
                }).collect();
                let forward = world.transform_vector3(glam::Vec3::Y);
                Some(json!({
                    "id": id, "tag": object.map(|o| o.tag.as_str()).unwrap_or(&prop.label),
                    "models": objects.key.get(i).map(|(_, models)| models),
                    "kind": object.map(|o| format!("{:?}", o.kind)),
                    "seen_in_view_or_shadow": prop.draws.clone().any(|d| seen.get(d).copied().unwrap_or(true)),
                    "sim_position": position.map(|p| p.to_array()), "sim_facing_degrees": object.map(|o| o.facing),
                    "world_position": world.w_axis.truncate().to_array(), "world_transform": world.to_cols_array(),
                    "offset_from_sim": position.map(|p| (world.w_axis.truncate() - p).to_array()),
                    "facing_radians": (-forward.x).atan2(forward.y), "forward": forward.to_array(),
                    "roots": roots,
                    "first_draw_world": objects.renderer.draw_world(prop.draws.start).map(|m| m.to_cols_array()),
                    "animations": clip.as_ref().map(|c| crate::report::clip_snapshot(&self.animator, c)),
                    "look_at": self.looks.report(*id), "opacity": opacity,
                    "camera_fade": self.opacity.get(id),
                    "attached_vfx": self.effects.as_ref().map(|effects| effects.key.iter()
                        .filter(|(owner, _, _)| owner == id)
                        .flat_map(|(_, _, keys)| keys).collect::<Vec<_>>()).unwrap_or_default(),
                }))
            }).collect::<Vec<_>>()
        }).unwrap_or_default();
        let mut emitters = Vec::new();
        let mut total_near = 0;
        let mut add_emitters = |source: &str,
                                renderer: &AreaRenderer,
                                scene: Option<&AreaScene>| {
            for (i, e) in renderer.emitters.iter().enumerate() {
                let at = e.world.w_axis.truncate();
                if at.distance_squared(eye) > 50.0 * 50.0 {
                    continue;
                }
                total_near += 1;
                if emitters.len() >= 512 {
                    continue;
                }
                let resource = scene.and_then(|s| s.emitters.get(i));
                emitters.push(json!({
                    "source": source, "index": i, "model": resource.map(|r| &r.model),
                    "node": resource.map(|r| &r.node), "owner": resource.and_then(|r| r.owner.as_ref()),
                    "material": e.blueprint.material, "library": e.blueprint.library,
                    "position": at.to_array(), "particles": e.particles.len(),
                    "opacity": e.opacity, "rate": e.rate, "color": e.color, "scale": e.scale,
                }));
            }
        };
        if let Some(level) = &self.level {
            add_emitters("level", &level.renderer, None);
        }
        if let Some(objects) = &self.objects {
            add_emitters("objects", &objects.renderer, Some(&objects.scene));
        }
        if let Some(effects) = &self.effects {
            add_emitters("vfx", &effects.renderer, Some(&effects.scene));
        }
        json!({
            "time_ms": sim.world.time_ms, "shader_time_s": self.shader_time,
            "layout": self.level.as_ref().map(|l| &l.layout), "objects": objects,
            "visibility": "scene candidates with nonzero opacity; seen flag includes shadow casters",
            "emitters": { "radius_m": 50, "limit": 512, "total_near": total_near, "items": emitters },
            "active_vfx": self.effects.as_ref().map(|e| &e.key),
            "location_vfx_near_camera": sim.world.location_vfx.iter()
                .filter(|v| Some(v.area) == sim.world.area && glam::Vec3::from_array(v.position).distance_squared(eye) <= 50.0 * 50.0)
                .take(512).map(|v| json!({ "handle": v.handle, "vfx": v.vfx,
                    "position": v.position, "facing_degrees": v.facing, "started_ms": v.started_ms,
                    "duration_ms": v.duration_ms })).collect::<Vec<_>>(),
            "instant_location_vfx_near_camera": sim.world.instant_location_vfx.iter()
                .filter(|v| Some(v.area) == sim.world.area && glam::Vec3::from_array(v.position).distance_squared(eye) <= 50.0 * 50.0)
                .take(512).map(|v| json!({ "handle": v.handle, "vfx": v.vfx,
                    "position": v.position, "facing_degrees": v.facing, "started_ms": v.started_ms,
                    "duration_ms": v.ms, "lasting": v.lasting })).collect::<Vec<_>>(),
            "cutaway_opacity": self.cutaway.0.shown(),
        })
    }

    /// Counts of submitted scene draws (loading statistics, not GPU visibility queries).
    pub fn report_draw_counts(&self) -> [usize; 3] {
        let mut counts = [0; 3];
        for r in [
            self.level.as_ref().map(|l| &l.renderer),
            self.objects.as_ref().map(|o| &o.renderer),
            self.effects.as_ref().map(|e| &e.renderer),
            self.rings.as_ref().map(|r| &r.renderer),
        ]
        .into_iter()
        .flatten()
        {
            counts[0] += r.stats.draws;
            counts[1] += r.stats.draws_skipped;
            counts[2] += r.stats.particle_draws;
        }
        counts
    }
}

fn root_snapshot(
    world: Mat4,
    sim: Option<glam::Vec3>,
    rest: glam::Vec3,
    posed: glam::Vec3,
) -> Value {
    let at = world.transform_point3(posed);
    json!({
        "rest_model": rest.to_array(), "posed_model": posed.to_array(), "world": at.to_array(),
        "animation_offset_world": world.transform_vector3(posed - rest).to_array(),
        "offset_from_sim": sim.map(|p| (at - p).to_array()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_diagnostics_distinguish_placement_from_two_centimeters_of_animation_sink() {
        let position = glam::Vec3::new(10.0, 20.0, 3.0);
        let world = Prop::placement(position.to_array(), 90.0);
        let root = root_snapshot(
            world,
            Some(position),
            glam::Vec3::ZERO,
            glam::Vec3::new(0.0, 0.0, -0.02),
        );
        assert!((root["animation_offset_world"][2].as_f64().unwrap() + 0.02).abs() < 1e-6);
        assert!((root["offset_from_sim"][2].as_f64().unwrap() + 0.02).abs() < 1e-6);
        assert!((root["world"][2].as_f64().unwrap() - 2.98).abs() < 1e-6);
    }
}
