//! Process-wide rendering diagnostics, read once before use in frame/draw loops.

use std::sync::LazyLock;

pub(crate) struct Config {
    pub punch: bool,
    pub objects: bool,
    pub mesh_tangents: bool,
    pub static_tint: bool,
    pub terrain: bool,
    pub faces: bool,
    pub fronds: bool,
    pub branches: bool,
    pub mesh_leaves: bool,
    pub leaf_cards: bool,
    pub baked_lighting: bool,
    /// [`BakedLighting::Hybrid`]: lightmaps keep only their baked bounce; direct visibility
    /// is real-time (see `docs/deferred.md`).
    pub realtime_direct: bool,
    pub ssao: bool,
    pub point_shadows: bool,
    pub shadow_slots: usize,
    pub tree_shadows: bool,
    pub tiled_lights: bool,
    pub tight_tiles: bool,
    pub light_masks: bool,
    pub opaque_gbuffer: bool,
    pub specialized_gbuffer: bool,
    pub specialized_lighting: bool,
    pub forward_lighting: bool,
    pub early_light_reject: bool,
    pub depth_lights: bool,
    pub direct_terrain_layers: bool,
    pub sun_shadows: bool,
    pub sun_early_exit: bool,
    pub shadow_reuse: bool,
    pub shadow_cull: bool,
    pub ground_mask: bool,
    pub late_transparency: bool,
    pub cutout_shadows: bool,
    pub blended_cutouts: bool,
    pub character_cutouts: bool,
    pub cutaway: bool,
    pub black: bool,
    pub fused_reflections: bool,
    pub log_lights: bool,
    pub log_materials: bool,
    pub profile: bool,
    pub profile_serial: bool,
}

/// How the deferred path treats the baked lightmaps (see `docs/deferred.md`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BakedLighting {
    /// Lightmaps as the original uses them: baked bounce and baked direct visibility.
    Baked,
    /// Baked bounce; every light's direct visibility real-time where a shadow map holds it.
    Hybrid,
    /// No lightmaps: probe ambient, SSAO and real-time visibility (experimental).
    Realtime,
}

impl BakedLighting {
    pub fn parse(s: &str) -> Option<BakedLighting> {
        match s.trim().to_ascii_lowercase().as_str() {
            "baked" => Some(BakedLighting::Baked),
            "hybrid" | "bounce" => Some(BakedLighting::Hybrid),
            "realtime" | "real-time" => Some(BakedLighting::Realtime),
            _ => None,
        }
    }
}

static LIGHTING: std::sync::OnceLock<BakedLighting> = std::sync::OnceLock::new();

/// Chooses how the deferred path treats lightmaps, in place of
/// `THEDAS_DEFERRED_BAKED_LIGHTING`. Call it before the first renderer is made: the choice
/// is read once, and a call after that (or a second call) does nothing.
pub fn set_baked_lighting(mode: BakedLighting) {
    let _ = LIGHTING.set(mode);
}

fn baked_lighting() -> BakedLighting {
    *LIGHTING.get_or_init(|| match std::env::var("THEDAS_DEFERRED_BAKED_LIGHTING").as_deref() {
        Ok("0") => BakedLighting::Realtime,
        Ok("bounce") | Ok("hybrid") => BakedLighting::Hybrid,
        _ => BakedLighting::Baked,
    })
}

pub(crate) static CONFIG: LazyLock<Config> = LazyLock::new(|| {
    let enabled = |name| std::env::var(name).as_deref() != Ok("0");
    Config {
        punch: enabled("THEDAS_DEFERRED_PUNCH"),
        objects: enabled("THEDAS_DEFERRED_OBJECTS"),
        mesh_tangents: enabled("THEDAS_DEFERRED_MESH_TANGENTS"),
        static_tint: enabled("THEDAS_DEFERRED_STATIC_TINT"),
        terrain: enabled("THEDAS_DEFERRED_TERRAIN"),
        faces: enabled("THEDAS_DEFERRED_FACES"),
        fronds: enabled("THEDAS_DEFERRED_FRONDS"),
        branches: enabled("THEDAS_DEFERRED_BRANCHES"),
        mesh_leaves: enabled("THEDAS_DEFERRED_MESH_LEAVES"),
        leaf_cards: enabled("THEDAS_DEFERRED_LEAF_CARDS"),
        baked_lighting: baked_lighting() != BakedLighting::Realtime,
        realtime_direct: baked_lighting() == BakedLighting::Hybrid,
        ssao: enabled("THEDAS_DEFERRED_SSAO"),
        point_shadows: enabled("THEDAS_DEFERRED_POINT_SHADOWS"),
        shadow_slots: std::env::var("THEDAS_DEFERRED_SHADOW_SLOTS")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(crate::deferred_shadow::SLOTS)
            .clamp(1, crate::deferred_shadow::SLOTS),
        tree_shadows: enabled("THEDAS_DEFERRED_TREE_SHADOWS"),
        tiled_lights: enabled("THEDAS_DEFERRED_TILED_LIGHTS"),
        tight_tiles: enabled("THEDAS_DEFERRED_TIGHT_TILES"),
        light_masks: enabled("THEDAS_DEFERRED_LIGHT_MASKS"),
        opaque_gbuffer: enabled("THEDAS_DEFERRED_OPAQUE_GBUFFER"),
        specialized_gbuffer: enabled("THEDAS_DEFERRED_SPECIALIZED_GBUFFER"),
        specialized_lighting: enabled("THEDAS_DEFERRED_SPECIALIZED_LIGHTING"),
        forward_lighting: enabled("THEDAS_DEFERRED_FORWARD_LIGHTING"),
        early_light_reject: enabled("THEDAS_DEFERRED_EARLY_LIGHT_REJECT"),
        depth_lights: enabled("THEDAS_DEFERRED_DEPTH_LIGHTS"),
        direct_terrain_layers: enabled("THEDAS_DEFERRED_DIRECT_TERRAIN_LAYERS"),
        sun_shadows: enabled("THEDAS_DEFERRED_SUN_SHADOWS"),
        sun_early_exit: enabled("THEDAS_DEFERRED_SUN_EARLY_EXIT"),
        shadow_reuse: enabled("THEDAS_DEFERRED_SHADOW_REUSE"),
        shadow_cull: enabled("THEDAS_DEFERRED_SHADOW_CULL"),
        ground_mask: enabled("THEDAS_DEFERRED_GROUND_MASK"),
        late_transparency: enabled("THEDAS_DEFERRED_LATE_TRANSPARENCY"),
        cutout_shadows: enabled("THEDAS_DEFERRED_CUTOUT_SHADOWS"),
        blended_cutouts: enabled("THEDAS_DEFERRED_BLENDED_CUTOUTS"),
        character_cutouts: enabled("THEDAS_DEFERRED_CHARACTER_CUTOUTS"),
        cutaway: enabled("THEDAS_DEFERRED_CUTAWAY"),
        black: enabled("THEDAS_DEFERRED_BLACK"),
        fused_reflections: enabled("THEDAS_DEFERRED_FUSED_REFLECTIONS"),
        log_lights: std::env::var_os("THEDAS_DEFERRED_LOG_LIGHTS").is_some(),
        log_materials: std::env::var_os("THEDAS_DEFERRED_LOG_MATERIALS").is_some(),
        profile: std::env::var("THEDAS_RENDER_PROFILE").as_deref() == Ok("1"),
        profile_serial: std::env::var("THEDAS_RENDER_PROFILE_SERIAL").as_deref() == Ok("1"),
    }
});

#[cfg(test)]
mod lighting_tests {
    use super::BakedLighting;

    #[test]
    fn lighting_modes_parse() {
        assert_eq!(BakedLighting::parse("baked"), Some(BakedLighting::Baked));
        assert_eq!(BakedLighting::parse("Hybrid"), Some(BakedLighting::Hybrid));
        assert_eq!(BakedLighting::parse("bounce"), Some(BakedLighting::Hybrid));
        assert_eq!(BakedLighting::parse("realtime"), Some(BakedLighting::Realtime));
        assert_eq!(BakedLighting::parse("off"), None);
    }
}
