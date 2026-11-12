//! Character creation's appearance menu (chargen.gfx's Appearance and Voice page): the
//! slider groups (`GUICharGen.AppearanceSliderGroups.N`: `Name`, `Enabled`) and the sliders
//! (`GUICharGen.AppearanceSliders.N`: `Group`, `Name`, `Value`, `Min`, `Max`, `Integral`),
//! as the exe builds them: its slider table (0x80ada0: id, label, range, whether whole
//! numbers) and the groups it shows them in (0x73a0c1). A slider the movie moves comes back
//! as a `SetValue` of its `Value`, which goes to the game ([`thedas_sim::appearance`]).
//!
//! Provisional: the ranges the exe takes from a list's length use `chargenmorphcfg.xml`'s
//! lists; Nose Shape's and Expression's lists are not traced (fixed ranges here); a slider
//! with no list for the race and gender (Beard, for women and elves) is left out.

use std::collections::BTreeMap;

use thedas_script::vm::ObjectId;
use thedas_sim::appearance::{BEARD, HAIR, PRESET};
use thedas_sim::sim::Sim;

use crate::UiValue;

/// The groups, in order: their labels (talk table strrefs).
pub const GROUPS: [u32; 9] = [
    374691, // Preset
    374692, // Skin
    374704, // Hair
    374712, // Eyes
    374721, // Nose
    374731, // Mouth
    374736, // Jaw / Cheeks
    374746, // Neck / Ears
    374750, // Portrait
];

/// A slider's range: fixed, or a face-screen list's indices.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Range {
    Fixed(f32, f32),
    List(&'static str),
}

/// One slider of the exe's table.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Slider {
    pub id: u32,
    pub group: u32,
    /// Its label (talk table strref); for women, the second where it differs.
    pub label: (u32, u32),
    pub range: Range,
    pub integral: bool,
}

const fn s(id: u32, group: u32, label: u32, range: Range, integral: bool) -> Slider {
    Slider {
        id,
        group,
        label: (label, label),
        range,
        integral,
    }
}

const fn f(id: u32, group: u32, label: u32) -> Slider {
    s(id, group, label, Range::Fixed(-1.0, 1.0), false)
}

const fn list(id: u32, group: u32, label: u32, l: &'static str) -> Slider {
    s(id, group, label, Range::List(l), true)
}

/// The sliders, in the menu's order (the exe adds them group by group).
pub const SLIDERS: &[Slider] = &[
    list(PRESET, 0, 374691, "heads"),
    // Skin.
    list(44, 1, 374693, "skins"),
    list(45, 1, 374694, "skin_colors"),
    list(46, 1, 374695, "blush_makeup_colors"),
    s(47, 1, 374696, Range::Fixed(0.0, 1.0), false),
    list(48, 1, 374697, "lip_makeup_colors"),
    s(49, 1, 374698, Range::Fixed(0.0, 1.0), false),
    list(8, 1, 374699, "eyes_makeup_colors"),
    s(9, 1, 374700, Range::Fixed(0.0, 1.0), false),
    list(34, 1, 374701, "tattoos"),
    list(35, 1, 374702, "tattoo_colors"),
    s(36, 1, 374703, Range::Fixed(0.4, 1.0), false),
    // Hair.
    list(HAIR, 2, 374704, "hairs"),
    list(BEARD, 2, 374705, "beards"),
    Slider {
        label: (374706, 374707),
        ..list(43, 2, 0, "hair_colors")
    },
    s(37, 2, 374708, Range::Fixed(-1.0, 3.0), true),
    s(38, 2, 374709, Range::Fixed(0.0, 15.0), true),
    Slider {
        label: (374710, 374711),
        ..list(39, 2, 0, "brow_stubble_colors")
    },
    // Eyes.
    s(10, 3, 374713, Range::Fixed(0.0, 4.0), true),
    f(6, 3, 374714),
    f(4, 3, 374716),
    f(5, 3, 374717),
    list(7, 3, 374718, "eyes_colors"),
    f(16, 3, 374719),
    f(17, 3, 374720),
    // Nose.
    s(30, 4, 374722, Range::Fixed(0.0, 4.0), true),
    f(27, 4, 374723),
    f(25, 4, 374724),
    f(24, 4, 374725),
    f(29, 4, 374726),
    f(22, 4, 374727),
    f(23, 4, 374728),
    f(26, 4, 374729),
    f(28, 4, 374730),
    // Mouth.
    f(0, 5, 374732),
    f(1, 5, 374733),
    f(2, 5, 374734),
    f(3, 5, 374735),
    // Jaw / Cheeks.
    f(11, 6, 374737),
    s(15, 6, 374738, Range::Fixed(0.0, 1.0), false),
    f(12, 6, 374739),
    s(13, 6, 374740, Range::Fixed(0.0, 1.0), false),
    f(14, 6, 374741),
    f(18, 6, 374742),
    f(19, 6, 374743),
    f(20, 6, 374744),
    f(21, 6, 374745),
    // Neck / Ears.
    f(31, 7, 374747),
    f(32, 7, 374748),
    f(33, 7, 374749),
    // Portrait.
    s(50, 8, 374751, Range::Fixed(-30.0, 30.0), false),
    s(51, 8, 374752, Range::Fixed(-45.0, 45.0), false),
    s(52, 8, 374753, Range::Fixed(0.5, 0.72), false),
    s(53, 8, 374754, Range::Fixed(-0.036, 0.036), false),
    s(54, 8, 374755, Range::Fixed(-0.02, 0.1), false),
    s(56, 8, 374756, Range::Fixed(1.0, 20.0), true),
    s(57, 8, 374757, Range::Fixed(0.0, 0.0), true),
];

/// The sliders shown for creature `id` (its race and gender's lists), in order.
pub fn shown(sim: &Sim, id: ObjectId) -> Vec<(Slider, f32, f32)> {
    SLIDERS
        .iter()
        .filter_map(|s| {
            let (min, max) = match s.range {
                Range::Fixed(a, b) => (a, b),
                Range::List(l) => {
                    let n = sim.face_list(id, l).len();
                    if n == 0 {
                        return None;
                    }
                    (0.0, (n - 1) as f32)
                }
            };
            Some((*s, min, max))
        })
        .collect()
}

/// A slider's value as the face shows it: the player's choice, else where the face's
/// preset stands (its index among the presets, its hair and beard models among theirs),
/// else the bottom of its range (0 within it).
fn value(sim: &Sim, id: ObjectId, s: &Slider, min: f32, max: f32) -> f32 {
    let Some(o) = sim.world.get(id) else {
        return 0.0;
    };
    if let Some(&v) = o.face_choices.get(&s.id) {
        return v;
    }
    let head = o.head_morph.clone().or_else(|| sim.face_screen_default(id));
    let index_of = |list: &str, name: Option<String>| {
        let name = name?.to_ascii_lowercase();
        sim.face_list(id, list)
            .iter()
            .position(|n| *n == name)
            .map(|i| i as f32)
    };
    let preset_model = |slot: usize| {
        let h = head.as_ref()?;
        let (d, _) = sim.vfs.read(&format!("{h}.mop")).ok()?;
        let p = thedas_formats::morph::MorphPreset::parse(&d).ok()?;
        Some(p.models.get(slot).cloned().flatten().unwrap_or_default())
    };
    use thedas_formats::morph::slot;
    let known = match s.id {
        PRESET => index_of("heads", head.clone()),
        HAIR => index_of("hairs", preset_model(slot::HAIR)),
        BEARD => index_of("beards", preset_model(slot::BEARD)),
        _ => None,
    };
    known.unwrap_or_else(|| 0.0f32.clamp(min, max))
}

/// The menu's values for the creature being created.
pub fn values(sim: &Sim, v: &mut BTreeMap<String, UiValue>) {
    let Some(id) = sim.world.chargen else {
        return;
    };
    let text = |strref: u32| UiValue::String(sim.string(strref).unwrap_or_default());
    for (i, g) in GROUPS.iter().enumerate() {
        let k = format!("GUICharGen.AppearanceSliderGroups.{i}");
        v.insert(k.clone(), UiValue::Number(i as f64));
        v.insert(format!("{k}.Name"), text(*g));
        v.insert(format!("{k}.Enabled"), UiValue::Number(1.0));
    }
    // Hair / Beard Color for those with beards, else Hair Color; Eyebrow / Stubble Color
    // for men, Eyebrow Color for women. Provisional: the exe's own rule is not traced.
    let female = sim
        .world
        .get(id)
        .is_some_and(|o| o.fields.get("Gender").copied() == Some(2));
    let beards = !sim.face_list(id, "beards").is_empty();
    for (i, (s, min, max)) in shown(sim, id).into_iter().enumerate() {
        let k = format!("GUICharGen.AppearanceSliders.{i}");
        let second = match s.id {
            43 => !beards,
            _ => female,
        };
        let label = if second { s.label.1 } else { s.label.0 };
        v.insert(k.clone(), UiValue::Number(s.id as f64));
        v.insert(format!("{k}.Group"), UiValue::Number(s.group as f64));
        v.insert(format!("{k}.Name"), text(label));
        v.insert(
            format!("{k}.Value"),
            UiValue::Number(value(sim, id, &s, min, max) as f64),
        );
        v.insert(format!("{k}.Min"), UiValue::Number(min as f64));
        v.insert(format!("{k}.Max"), UiValue::Number(max as f64));
        v.insert(
            format!("{k}.Integral"),
            UiValue::Number(f64::from(u8::from(s.integral))),
        );
    }
}

/// The slider a movie's `GUICharGen.AppearanceSliders.N.Value` is (by its place in the
/// menu), as the game's id.
pub fn slider_at(sim: &Sim, index: usize) -> Option<u32> {
    let id = sim.world.chargen?;
    shown(sim, id).get(index).map(|(s, _, _)| s.id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_has_each_slider_once_in_a_known_group() {
        let mut ids: Vec<u32> = SLIDERS.iter().map(|s| s.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), SLIDERS.len());
        assert!(SLIDERS.iter().all(|s| (s.group as usize) < GROUPS.len()));
        // Groups come in order, as the exe adds them.
        assert!(SLIDERS.windows(2).all(|w| w[0].group <= w[1].group));
        assert_eq!(SLIDERS[0].id, PRESET);
    }
}
