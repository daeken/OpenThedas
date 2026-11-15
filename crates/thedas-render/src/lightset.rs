//! Fitted light sets (experimental): what the shipped lightmaps say the original bake's lights
//! were, as `thedas-bake --lightset` writes them (`docs/lightmaps/baker-spec.md` §7). The
//! deferred path's lightmaps-off mode can use a level's set in place of probe ambient and to
//! add the baked-only fill lights the game files no longer hold.
//!
//! Off unless `THEDAS_DEFERRED_LIGHTSET` names a light set file or a directory of
//! `<level>.lightset.json` files. The sets are derived from game data and are not in this
//! repository.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use glam::Vec3;

/// One baked-only fill light: linear colour × brightness, falling off as max(0, 1 − d/R).
#[derive(Clone, Debug, PartialEq)]
pub struct FillLight {
    pub position: Vec3,
    pub radius: f32,
    pub color: Vec3,
    /// The atlas whose fit found it (`room1400`, `lak100d_5_2_000`): its pieces' room is the
    /// one the light belongs to.
    pub atlas: Option<String>,
}

/// What the fit says one of the level's own lights was in the bake.
#[derive(Clone, Debug, PartialEq)]
pub struct SetLight {
    pub position: Vec3,
    /// Its baked twin's brightness in the RGB pass (times the light's colour), when seen.
    pub baked_brightness: Option<f32>,
    /// Whether the shadow (alpha) pass took the light.
    pub in_alpha: bool,
    /// The share of its radius the shadow pass reached (the bake radius).
    pub shadow_reach: f32,
}

impl SetLight {
    /// How far (as a share of its radius) the light's baked shadow reached: none at all when
    /// the shadow pass left it out.
    pub fn reach(&self) -> f32 {
        if self.in_alpha {
            self.shadow_reach.clamp(0.0, 1.0)
        } else {
            0.0
        }
    }
}

/// A level's fitted light set.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LightSet {
    /// The level's own point lights, as the fit saw them in the bake.
    pub lights: Vec<SetLight>,
    /// Each room's baked ambient (linear), by lower-case room name.
    pub ambient: HashMap<String, Vec3>,
    /// The baked-only fill lights, with the copies neighbouring atlases' fits found merged.
    pub fills: Vec<FillLight>,
}

impl LightSet {
    /// Parses a light set's JSON. `None` when it is not one.
    pub fn parse(text: &str) -> Option<LightSet> {
        let doc: serde_json::Value = serde_json::from_str(text).ok()?;
        let vec3 = |v: &serde_json::Value| -> Option<Vec3> {
            let a = v.as_array()?;
            Some(Vec3::new(
                a.first()?.as_f64()? as f32,
                a.get(1)?.as_f64()? as f32,
                a.get(2)?.as_f64()? as f32,
            ))
        };
        let mut ambient = HashMap::new();
        for (room, v) in doc.get("rooms")?.as_object()? {
            if let Some(c) = v.get("baked_ambient").and_then(vec3) {
                ambient.insert(room.to_ascii_lowercase(), c);
            }
        }
        let mut found = Vec::new();
        for f in doc
            .get("found")
            .and_then(|f| f.as_array())
            .into_iter()
            .flatten()
        {
            let (Some(position), Some(radius), Some(color)) = (
                f.get("position").and_then(vec3),
                f.get("radius").and_then(|r| r.as_f64()),
                f.get("color").and_then(vec3),
            ) else {
                continue;
            };
            if radius > 0.0 && color.max_element() > 0.0 {
                found.push(FillLight {
                    position,
                    radius: radius as f32,
                    color,
                    atlas: f
                        .get("atlas")
                        .and_then(|a| a.as_str())
                        .map(str::to_ascii_lowercase),
                });
            }
        }
        let lights = doc
            .get("lights")
            .and_then(|l| l.as_array())
            .into_iter()
            .flatten()
            .filter(|l| l.get("kind").and_then(|k| k.as_str()) == Some("point"))
            .filter_map(|l| {
                Some(SetLight {
                    position: l.get("position").and_then(vec3)?,
                    baked_brightness: l
                        .get("baked_brightness")
                        .and_then(|b| b.as_f64())
                        .map(|b| b as f32),
                    in_alpha: l.get("in_alpha").and_then(|a| a.as_bool()).unwrap_or(true),
                    shadow_reach: l
                        .get("shadow_reach")
                        .and_then(|r| r.as_f64())
                        .map_or(1.0, |r| r as f32),
                })
            })
            .collect();
        Some(LightSet {
            lights,
            ambient,
            fills: merge(&found),
        })
    }

    /// The set for `level` named by `THEDAS_DEFERRED_LIGHTSET` (a file, or a directory
    /// holding `<level>.lightset.json`); `None` when the variable is unset or nothing loads.
    pub fn for_level(level: &str) -> Option<LightSet> {
        let path = PathBuf::from(std::env::var_os("THEDAS_DEFERRED_LIGHTSET")?);
        let file = if path.is_dir() {
            path.join(format!("{}.lightset.json", level.to_ascii_lowercase()))
        } else {
            path
        };
        let set = Self::load(&file);
        match &set {
            Some(s) => log::info!(
                "light set {}: {} room ambients, {} fill lights",
                file.display(),
                s.ambient.len(),
                s.fills.len()
            ),
            None => log::warn!("light set {}: not loaded", file.display()),
        }
        set
    }

    fn load(path: &Path) -> Option<LightSet> {
        Self::parse(&std::fs::read_to_string(path).ok()?)
    }
}

/// Merges fill lights found twice in one atlas's fit: same radius, within a metre (their
/// colours averaged). Fills of different atlases stay apart: each belongs to its atlas's room.
fn merge(found: &[FillLight]) -> Vec<FillLight> {
    let mut groups: Vec<(FillLight, u32)> = Vec::new();
    for f in found {
        match groups.iter_mut().find(|(g, _)| {
            g.atlas == f.atlas
                && (g.radius - f.radius).abs() < 0.01
                && g.position.distance(f.position) < 1.0
        }) {
            Some((g, n)) => {
                g.color = (g.color * *n as f32 + f.color) / (*n + 1) as f32;
                *n += 1;
            }
            None => groups.push((f.clone(), 1)),
        }
    }
    groups.into_iter().map(|(g, _)| g).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_merges_repeated_fills() {
        let text = r#"{
            "level": "test",
            "rooms": {"Room_1": {"baked_ambient": [0.1, 0.2, 0.3], "runtime_ambient": null}},
            "lights": [
                {"name": "lamp", "kind": "point", "position": [4, 5, 6], "radius": 8,
                 "color": [1, 1, 1], "baked_brightness": 0.8, "in_alpha": true, "shadow_reach": 0.85},
                {"name": "fill", "kind": "point", "position": [7, 5, 6], "radius": 8,
                 "color": [1, 1, 1], "baked_brightness": null, "in_alpha": false, "shadow_reach": 1}
            ],
            "found": [
                {"position": [1, 2, 3], "radius": 6, "color": [0.2, 0.0, 0.0], "atlas": "a"},
                {"position": [1, 2, 3], "radius": 6, "color": [0.4, 0.0, 0.0], "atlas": "a"},
                {"position": [9, 2, 3], "radius": 6, "color": [0.1, 0.1, 0.1], "atlas": "b"},
                {"position": [9, 2, 3], "radius": 3, "color": [0.0, 0.0, 0.0], "atlas": "b"}
            ]
        }"#;
        let set = LightSet::parse(text).unwrap();
        assert_eq!(set.ambient["room_1"], Vec3::new(0.1, 0.2, 0.3));
        assert_eq!(set.fills.len(), 2);
        assert!((set.fills[0].color.x - 0.3).abs() < 1e-6);
        assert_eq!(set.fills[1].position, Vec3::new(9.0, 2.0, 3.0));
        assert_eq!(set.fills[0].atlas.as_deref(), Some("a"));
        assert_eq!(set.lights.len(), 2);
        assert_eq!(set.lights[0].baked_brightness, Some(0.8));
        assert!((set.lights[0].reach() - 0.85).abs() < 1e-6);
        assert_eq!(set.lights[1].baked_brightness, None);
        assert_eq!(set.lights[1].reach(), 0.0);
        assert!(LightSet::parse("[]").is_none());
    }
}
