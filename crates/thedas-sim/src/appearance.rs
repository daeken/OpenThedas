//! Character creation's appearance sliders: what the player chose for their face, kept on
//! the creature ([`crate::world::Object::face_choices`]) by slider id. The ids are the
//! exe's (its slider table, built at 0x80ada0 and grouped at 0x73a0c1); the menu's labels,
//! groups and ranges are the interface's (`thedas_ui`).
//!
//! What a choice does to the face: the preset puts that head on; hair and beard put their
//! models in the morph's slots; the colours put their tints in its tint slots. Provisional:
//! the face-shape sliders weigh their morph targets (the exe's pairs, read back at 0x805f40,
//! and its per-race scales at 0x7fdd00). Provisional: the makeup intensities, tattoos,
//! complexion and the eye and nose shape lists are kept but not yet put on the face.

use thedas_formats::morph::{FaceOverlay, slot};
use thedas_script::vm::ObjectId;

use crate::sim::Sim;

/// The message chargen.gfx's sliders reach the game by (the movie sets
/// `GUICharGen.AppearanceSliders.N.Value`; the interface passes it on): args `[slider id,
/// value]`.
pub const MSG_SET_SLIDER: &str = "thedas.SetAppearanceSlider";

/// The head preset (`chargenmorphcfg.xml`'s `heads`).
pub const PRESET: u32 = 40;
/// The hair model (`hairs`).
pub const HAIR: u32 = 41;
/// The beard model (`beards`; the first is none).
pub const BEARD: u32 = 42;
/// The colour sliders (`chargenmorphcfg.xml`'s colour lists).
pub const HAIR_COLOR: u32 = 43;
pub const SKIN_TONE: u32 = 45;
pub const BLUSH_COLOR: u32 = 46;
pub const LIP_COLOR: u32 = 48;
pub const EYE_SHADOW_COLOR: u32 = 8;
pub const EYE_COLOR: u32 = 7;
pub const BROW_COLOR: u32 = 39;

/// The face-shape sliders' morph targets (slider id, the target its negative side
/// weighs, the one its positive side weighs; one target: the slider weighs it alone).
pub const FACE_TARGETS: &[(u32, Option<&str>, &str)] = &[
    (0, Some("UH_MTH_05a_0"), "UH_MTH_14a_0"),
    (1, Some("UH_MTH_07a_0"), "UH_MTH_08a_0"),
    (2, Some("UH_MTH_11a_0"), "UH_MTH_17a_0"),
    (3, Some("UH_MTH_12a_0"), "UH_MTH_13a_0"),
    (4, Some("UHM_EYE_03a_0"), "UHM_EYE_08a_0"),
    (5, Some("UHM_EYE_19a_0"), "UHM_EYE_20a_0"),
    (6, Some("UHM_EYE_09a_0"), "UHM_EYE_06a_0"),
    (11, Some("UH_CHK_01a_0"), "UH_CHK_06a_0"),
    (12, Some("UH_CHK_02a_0"), "UH_CHK_03a_0"),
    (13, None, "UH_CHK_07a_0"),
    (14, Some("UH_CHK_08a_0"), "UH_CHK_13a_0"),
    (15, None, "UH_CHK_04a_0"),
    (16, Some("UH_BRW_01a_0"), "UH_BRW_02a_0"),
    (17, Some("UH_BRW_04a_0"), "UH_BRW_05a_0"),
    (18, Some("UH_JAW_02a_0"), "UH_JAW_06a_0"),
    (19, Some("UH_JAW_03a_0"), "UH_JAW_04a_0"),
    (20, Some("UH_JAW_05a_0"), "UH_JAW_07a_0"),
    (21, Some("UH_JAW_09a_0"), "UH_JAW_10a_0"),
    (22, Some("UH_NOS_07a_0"), "UH_NOS_08a_0"),
    (23, Some("UH_NOS_09a_0"), "UH_NOS_10a_0"),
    (24, Some("UH_NOS_11a_0"), "UH_NOS_18a_0"),
    (25, Some("UH_NOS_35a_0"), "UH_NOS_36a_0"),
    (26, Some("UH_NOS_13a_0"), "UH_NOS_15a_0"),
    (27, Some("UH_NOS_33a_0"), "UH_NOS_34a_0"),
    (28, Some("UH_NOS_37a_0"), "UH_NOS_38a_0"),
    (29, Some("UH_NOS_01a_0"), "UH_NOS_02a_0"),
    (31, Some("UH_EAR_01a_0"), "UH_EAR_06a_0"),
    (32, Some("UH_EAR_03a_0"), "UH_EAR_05a_0"),
    (33, Some("UH_NEC_02a_0"), "UH_NEC_03a_0"),
];

/// Per race and gender (human male, human female, dwarf male, dwarf female, elf male,
/// elf female) and slider id (0 to 33): how far each side of a slider reaches (negative,
/// positive).
pub const FACE_SCALES: [[(f32, f32); 34]; 6] = [
    [
        (0.8, 0.8),
        (1.0, 1.0),
        (0.8, 1.0),
        (1.0, 1.0),
        (0.8, 1.0),
        (1.0, 1.0),
        (0.5, 0.3),
        (1.0, 1.0),
        (1.0, 1.0),
        (1.0, 1.0),
        (1.0, 1.0),
        (0.4, 1.0),
        (0.5, 0.4),
        (1.0, 1.0),
        (1.0, 0.5),
        (1.0, 0.4),
        (1.0, 1.0),
        (1.0, 0.8),
        (0.8, 0.8),
        (1.0, 0.6),
        (0.8, 1.0),
        (0.6, 0.4),
        (1.0, 0.6),
        (0.8, 1.0),
        (0.5, 0.6),
        (1.0, 0.8),
        (0.8, 1.0),
        (0.4, 1.0),
        (0.8, 0.8),
        (0.6, 0.6),
        (1.0, 1.0),
        (1.0, 0.6),
        (0.2, 1.0),
        (1.0, 0.4),
    ],
    [
        (0.6, 0.6),
        (1.0, 1.0),
        (0.6, 0.6),
        (1.0, 1.0),
        (0.6, 1.0),
        (1.0, 1.0),
        (0.5, 0.3),
        (1.0, 1.0),
        (1.0, 1.0),
        (1.0, 1.0),
        (1.0, 1.0),
        (1.0, 1.0),
        (0.5, 0.4),
        (1.0, 0.7),
        (1.0, 0.5),
        (1.0, 1.0),
        (0.8, 0.8),
        (0.8, 0.6),
        (0.8, 0.6),
        (0.6, 0.6),
        (0.6, 0.8),
        (0.6, 0.6),
        (0.6, 0.6),
        (0.2, 0.8),
        (0.5, 0.6),
        (0.6, 0.6),
        (0.2, 0.8),
        (0.3, 0.8),
        (0.6, 0.8),
        (0.3, 1.0),
        (1.0, 1.0),
        (0.6, 1.0),
        (0.2, 1.0),
        (0.6, 0.4),
    ],
    [
        (0.8, 0.6),
        (1.0, 1.0),
        (1.0, 1.0),
        (1.0, 1.0),
        (1.0, 1.0),
        (1.0, 1.0),
        (0.3, 0.3),
        (1.0, 1.0),
        (1.0, 1.0),
        (1.0, 1.0),
        (1.0, 1.0),
        (1.0, 1.0),
        (1.0, 0.4),
        (1.0, 1.0),
        (0.1, 0.5),
        (1.0, 1.0),
        (1.0, 1.0),
        (1.0, 1.0),
        (1.0, 1.0),
        (1.0, 1.0),
        (1.0, 1.0),
        (1.0, 0.5),
        (1.0, 1.0),
        (1.0, 1.0),
        (0.6, 0.8),
        (1.0, 1.0),
        (1.0, 1.0),
        (1.0, 1.0),
        (1.0, 1.0),
        (1.0, 1.0),
        (1.0, 1.0),
        (1.0, 1.0),
        (1.0, 1.0),
        (1.0, 0.4),
    ],
    [
        (0.8, 0.6),
        (1.0, 1.0),
        (0.8, 1.0),
        (1.0, 1.0),
        (1.0, 1.0),
        (1.0, 1.0),
        (0.6, 0.3),
        (1.0, 1.0),
        (1.0, 1.0),
        (1.0, 1.0),
        (1.0, 1.0),
        (1.0, 1.0),
        (1.0, 0.5),
        (1.0, 1.0),
        (1.0, 0.5),
        (1.0, 1.0),
        (1.0, 1.0),
        (1.0, 0.6),
        (0.6, 0.8),
        (1.0, 1.0),
        (1.0, 1.0),
        (0.8, 0.6),
        (0.6, 1.0),
        (0.3, 0.6),
        (0.5, 0.6),
        (0.6, 1.0),
        (0.3, 1.0),
        (0.3, 1.0),
        (0.6, 1.0),
        (0.3, 1.0),
        (1.0, 1.0),
        (1.0, 1.0),
        (1.0, 1.0),
        (0.6, 1.0),
    ],
    [
        (0.8, 0.8),
        (1.0, 1.0),
        (0.7, 0.6),
        (1.0, 1.0),
        (0.8, 0.8),
        (1.0, 1.0),
        (0.5, 0.3),
        (1.0, 1.0),
        (1.0, 1.0),
        (1.0, 1.0),
        (1.0, 1.0),
        (0.4, 1.0),
        (0.5, 0.4),
        (1.0, 1.0),
        (1.0, 0.5),
        (1.0, 1.0),
        (1.0, 0.8),
        (1.0, 0.6),
        (0.8, 0.8),
        (1.0, 0.6),
        (0.6, 1.0),
        (0.6, 0.8),
        (0.6, 1.0),
        (1.0, 1.0),
        (0.5, 0.6),
        (0.6, 0.6),
        (0.5, 0.8),
        (0.5, 0.8),
        (0.6, 0.6),
        (0.4, 0.6),
        (1.0, 1.0),
        (1.0, 0.6),
        (0.4, 1.0),
        (0.8, 0.2),
    ],
    [
        (0.6, 0.6),
        (1.0, 1.0),
        (0.5, 1.0),
        (1.0, 1.0),
        (0.6, 0.6),
        (1.0, 1.0),
        (0.5, 0.3),
        (1.0, 1.0),
        (1.0, 1.0),
        (1.0, 1.0),
        (1.0, 1.0),
        (0.4, 1.0),
        (0.5, 0.4),
        (1.0, 1.0),
        (1.0, 0.5),
        (1.0, 1.0),
        (0.6, 1.0),
        (1.0, 0.6),
        (0.6, 0.6),
        (0.8, 0.5),
        (0.6, 0.8),
        (0.6, 0.6),
        (0.6, 0.3),
        (0.2, 0.5),
        (0.5, 0.4),
        (0.6, 0.4),
        (0.2, 1.0),
        (0.3, 0.6),
        (0.4, 0.8),
        (0.3, 0.5),
        (1.0, 1.0),
        (1.0, 1.0),
        (0.4, 1.0),
        (0.4, 0.3),
    ],
];

/// [`FACE_SCALES`]' row for a race and gender as `chargenmorphcfg.xml` names it (the exe's
/// 0x7fdca0: human 0 and 1, dwarf 2 and 3, elf 4 and 5, men first).
pub fn race_gender_row(race_gender: &str) -> usize {
    let race = match race_gender.split('_').next() {
        Some("dwarf") => 2,
        Some("elf") => 4,
        _ => 0,
    };
    race + usize::from(race_gender.ends_with("female"))
}

/// The names a face target goes by on each head model: the head's own (`UHM_EYE_03a_0`),
/// and for the eye targets the eyes' and lashes' (`UEM_`, `ULM_`), which move with it.
fn target_names(t: &str) -> Vec<String> {
    match t.strip_prefix("UHM_") {
        Some(rest) => ["UHM_", "UEM_", "ULM_"]
            .iter()
            .map(|p| format!("{p}{rest}"))
            .collect(),
        None => vec![t.to_string()],
    }
}

/// The morph target weights a face-shape slider at `v` gives (both of its targets, the side
/// it is not on at 0), scaled for the race and gender. Provisional: which of the exe's two
/// scales goes with which side is read from the order 0x7fdd00 writes them.
pub fn feature_weights(slider: u32, v: f32, row: usize) -> Vec<(String, f32)> {
    let Some(&(_, neg, pos)) = FACE_TARGETS.iter().find(|(id, _, _)| *id == slider) else {
        return Vec::new();
    };
    let (lo, hi) = FACE_SCALES
        .get(row)
        .and_then(|r| r.get(slider as usize))
        .copied()
        .unwrap_or((1.0, 1.0));
    let mut out = Vec::new();
    let mut put = |t: &str, w: f32| {
        for n in target_names(t) {
            out.push((n, w));
        }
    };
    match neg {
        Some(n) => {
            put(n, (-v).max(0.0) * lo);
            put(pos, v.max(0.0) * hi);
        }
        None => put(pos, v.max(0.0) * hi),
    }
    out
}

/// Where a face-shape slider stands on a face whose targets weigh `weights`: the inverse of
/// [`feature_weights`] (the positive target's weight less the negative's, unscaled).
pub fn feature_value(slider: u32, weights: &[(String, f32)], row: usize) -> Option<f32> {
    let &(_, neg, pos) = FACE_TARGETS.iter().find(|(id, _, _)| *id == slider)?;
    let w = |t: &str| {
        weights
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(t))
            .map_or(0.0, |(_, w)| *w)
    };
    let (lo, hi) = FACE_SCALES
        .get(row)
        .and_then(|r| r.get(slider as usize))
        .copied()
        .unwrap_or((1.0, 1.0));
    let (p, n) = (w(pos), neg.map_or(0.0, w));
    Some(if p >= n {
        p / hi.max(f32::EPSILON)
    } else {
        -n / lo.max(f32::EPSILON)
    })
}

/// The choices made over a creature's head morph, as the renderer applies them.
pub fn overlay(sim: &Sim, id: ObjectId) -> FaceOverlay {
    let mut o = FaceOverlay::default();
    let Some(obj) = sim.world.get(id) else {
        return o;
    };
    for (slider, list, model_slot) in [(HAIR, "hairs", slot::HAIR), (BEARD, "beards", slot::BEARD)]
    {
        let Some(&v) = obj.face_choices.get(&slider) else {
            continue;
        };
        let names = sim.face_list(id, list);
        if let Some(name) = names.get(v.max(0.0).round() as usize) {
            o.models
                .push((model_slot, Some(name.clone()).filter(|n| !n.is_empty())));
        }
    }
    // Face shape: each slider's morph targets.
    let row = race_gender_row(&sim.face_race_gender(id).unwrap_or_default());
    for (&slider, &v) in &obj.face_choices {
        o.targets.extend(feature_weights(slider, v, row));
    }
    // Colours: the tint each colour slider picks, in the morph's tint slot the exe's
    // setter writes (lips 1, eyes 2, hair 3, eye shadow 4, blush 5, skin 0, brows and
    // stubble 10). A "none" entry (the makeup lists start with one) leaves the face's own.
    for (slider, list, tint_slot) in [
        (SKIN_TONE, "skin_colors", 0),
        (LIP_COLOR, "lip_makeup_colors", 1),
        (EYE_COLOR, "eyes_colors", 2),
        (HAIR_COLOR, "hair_colors", 3),
        (EYE_SHADOW_COLOR, "eyes_makeup_colors", 4),
        (BLUSH_COLOR, "blush_makeup_colors", 5),
        (BROW_COLOR, "brow_stubble_colors", 10),
    ] {
        let Some(&v) = obj.face_choices.get(&slider) else {
            continue;
        };
        let names = sim.face_list(id, list);
        if let Some(name) = names
            .get(v.max(0.0).round() as usize)
            .filter(|n| !n.is_empty())
        {
            o.tints.push((tint_slot, Some(name.clone())));
        }
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_face_slider_weighs_one_side_and_reads_back() {
        // Mouth height on a human man: -0.5 weighs its negative target by the exe's scale.
        let w = feature_weights(0, -0.5, 0);
        assert_eq!(
            w,
            vec![
                ("UH_MTH_05a_0".to_string(), 0.4),
                ("UH_MTH_14a_0".to_string(), 0.0)
            ]
        );
        assert!((feature_value(0, &w, 0).unwrap() + 0.5).abs() < 1e-6);
        // The eye targets move the eyes and lashes too.
        let e = feature_weights(4, 1.0, 0);
        assert!(e.iter().any(|(n, w)| n == "UEM_EYE_08a_0" && *w > 0.0));
        assert!(e.iter().any(|(n, _)| n == "ULM_EYE_03a_0"));
        // One-target sliders, and sliders with no targets.
        assert_eq!(
            feature_weights(13, 0.5, 0),
            vec![("UH_CHK_07a_0".to_string(), 0.5)]
        );
        assert!(feature_weights(PRESET, 1.0, 0).is_empty());
        assert_eq!(race_gender_row("elf_female"), 5);
        assert_eq!(race_gender_row("human_male"), 0);
    }
}
