//! Character creation's appearance sliders: what the player chose for their face, kept on
//! the creature ([`crate::world::Object::face_choices`]) by slider id. The ids are the
//! exe's (its slider table, built at 0x80ada0 and grouped at 0x73a0c1); the menu's labels,
//! groups and ranges are the interface's (`thedas_ui`).
//!
//! What a choice does to the face: the preset puts that head on; hair and beard put their
//! models in the morph's slots; the colours put their tints in its tint slots; the
//! face-shape sliders weigh their morph targets (the exe's pairs, read back at 0x805f40,
//! and its per-race scales at 0x7fdd00); the makeup intensities set the face's tint
//! opacities and the complexion its diffuse texture. Provisional: tattoos are kept but not
//! yet put on the face.

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
/// The makeup intensity sliders and the complexion.
pub const LIP_INTENSITY: u32 = 49;
pub const EYE_SHADOW_INTENSITY: u32 = 9;
pub const BLUSH_INTENSITY: u32 = 47;
pub const COMPLEXION: u32 = 44;

/// The face screen's Portrait group (labels 374751-374757): how the creature's portrait is
/// framed. Their setters (0x7ff310, 0x7ff350, 0x7ff390, 0x7ff3d0, 0x7ff410, 0x7ff450 from the
/// slider table 0x80ada0) write the creature's portrait settings (its appearance block
/// +0x598: +0x58 look up/down, +0x5c turn, +0x60 camera distance, +0x64 and +0x68 the
/// moves, +0x6c the background), the values `portraits.gda` holds for a follower.
pub const PORTRAIT_PITCH: u32 = 50;
pub const PORTRAIT_TURN: u32 = 51;
pub const PORTRAIT_DISTANCE: u32 = 52;
pub const PORTRAIT_MOVE_X: u32 = 53;
pub const PORTRAIT_MOVE_Y: u32 = 54;
pub const PORTRAIT_BACKGROUND: u32 = 56;

/// How a creature's portrait is framed: `portraits.gda`'s row for its tag, else its
/// `default` row, with the face screen's Portrait choices over it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Portrait {
    /// Degrees the camera looks up (+) or down at the face (`CameraPitch`).
    pub pitch: f32,
    /// Degrees the camera turns round the face from its front (`CameraRotation`).
    pub turn: f32,
    /// Metres from the face (`CameraDistance`).
    pub distance: f32,
    /// Metres the camera moves along its right and up (the columns hashed 0xdb253899 and
    /// 0x3a4bfc34; names not known).
    pub shift: (f32, f32),
    /// The background picture (1 to 20; column 0xc9210008).
    pub background: i32,
}

/// `portraits.gda`'s columns without a known name.
const PORTRAIT_SHIFT_X: u32 = 0xdb25_3899;
const PORTRAIT_SHIFT_Y: u32 = 0x3a4b_fc34;
const PORTRAIT_BACKGROUND_COLUMN: u32 = 0xc921_0008;

impl Sim {
    /// Creature `id`'s portrait framing (see [`Portrait`]). Provisional: the face screen's
    /// starting values are taken to be the `default` row's (the exe's are not traced), and
    /// the moves' directions are not traced.
    pub fn portrait(&self, id: thedas_script::vm::ObjectId) -> Portrait {
        use thedas_formats::gda::Cell;
        let o = self.world.get(id);
        let tables = self.world.tables();
        let table = tables.get(0, "portraits");
        let row = table.as_ref().and_then(|t| {
            o.and_then(|o| t.find("Tag", &o.tag))
                .or_else(|| t.find("Tag", "default"))
        });
        let num = |c: Option<&Cell>, d: f32| match c {
            Some(Cell::Float(v)) => *v as f32,
            Some(Cell::Int(v)) => *v as f32,
            _ => d,
        };
        let cell = |name: &str| table.as_ref().zip(row).and_then(|(t, r)| t.cell(name, r));
        let hashed = |h: u32| {
            table
                .as_ref()
                .zip(row)
                .and_then(|(t, r)| t.cell_by_hash(h, r))
        };
        let mut p = Portrait {
            pitch: num(cell("CameraPitch"), 0.0),
            turn: num(cell("CameraRotation"), 0.0),
            distance: num(cell("CameraDistance"), 0.7),
            shift: (
                num(hashed(PORTRAIT_SHIFT_X), 0.0),
                num(hashed(PORTRAIT_SHIFT_Y), 0.0),
            ),
            background: num(hashed(PORTRAIT_BACKGROUND_COLUMN), 1.0) as i32,
        };
        if let Some(o) = o {
            let chosen = |s: u32| o.face_choices.get(&s).copied();
            p.pitch = chosen(PORTRAIT_PITCH).unwrap_or(p.pitch);
            p.turn = chosen(PORTRAIT_TURN).unwrap_or(p.turn);
            p.distance = chosen(PORTRAIT_DISTANCE).unwrap_or(p.distance);
            p.shift.0 = chosen(PORTRAIT_MOVE_X).unwrap_or(p.shift.0);
            p.shift.1 = chosen(PORTRAIT_MOVE_Y).unwrap_or(p.shift.1);
            p.background = chosen(PORTRAIT_BACKGROUND).map_or(p.background, |v| v as i32);
        }
        p
    }
}

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

/// Eye Shape's choices per race and gender (rows as [`FACE_SCALES`]): the head's and the
/// lashes' target and its weight (the exe's table at 0x7fe330). The slider's 0 is none, 1 to
/// 4 these (its apply at 0x808cc7 clears all four, then weighs the one chosen).
pub const EYE_SHAPES: [[(&str, &str, f32); 4]; 6] = [
    [
        ("UHM_CHM_09a_0", "ULM_CHM_09a_0", 1.0),
        ("UHM_CHM_10a_0", "ULM_CHM_10a_0", 1.0),
        ("UHM_CHM_11a_0", "ULM_CHM_11a_0", 1.0),
        ("UHM_CHM_12a_0", "ULM_CHM_12a_0", 0.8),
    ],
    [
        ("UHM_CHF_07a_0", "ULM_CHF_07a_0", 0.7),
        ("UHM_CHF_08a_0", "ULM_CHF_08a_0", 1.0),
        ("UHM_CHF_09a_0", "ULM_CHF_09a_0", 0.7),
        ("UHM_CHF_10a_0", "ULM_CHF_10a_0", 0.8),
    ],
    [
        ("UHM_CDM_02a_0", "ULM_CDM_02a_0", 0.6),
        ("UHM_CDM_03a_0", "ULM_CDM_03a_0", 0.8),
        ("UHM_CDM_04a_0", "ULM_CDM_04a_0", 0.8),
        ("UHM_CDM_05a_0", "ULM_CDM_05a_0", 0.6),
    ],
    [
        ("UHM_CDF_02a_0", "ULM_CDF_02a_0", 0.6),
        ("UHM_CDF_03a_0", "ULM_CDF_03a_0", 0.7),
        ("UHM_CDF_04a_0", "ULM_CDF_04a_0", 0.6),
        ("UHM_CDF_05a_0", "ULM_CDF_05a_0", 0.7),
    ],
    [
        ("UHM_CEM_03a_0", "ULM_CEM_03a_0", 0.7),
        ("UHM_CEM_04a_0", "ULM_CEM_04a_0", 0.7),
        ("UHM_CEM_05a_0", "ULM_CEM_05a_0", 0.8),
        ("UHM_CEM_06a_0", "ULM_CEM_06a_0", 0.7),
    ],
    [
        ("UHM_CEF_02a_0", "ULM_CEF_02a_0", 0.7),
        ("UHM_CEF_03a_0", "ULM_CEF_03a_0", 0.8),
        ("UHM_CEF_04a_0", "ULM_CEF_04a_0", 0.8),
        ("UHM_CEF_05a_0", "ULM_CEF_05a_0", 0.6),
    ],
];

/// Nose Shape's choices per race and gender (the exe's table at 0x7fe650, its counts at
/// 0x7fe610): the target and its weight. The slider's 0 is none, 1 on these (its apply at
/// 0x809734). Provisional: some weights were read through an emulated x87 stack.
pub const NOSE_SHAPES: [&[(&str, f32)]; 6] = [
    &[
        ("UH_CHM_04a_0", 0.8),
        ("UH_CHM_02a_0", 0.8),
        ("UH_CHM_01a_0", 0.8),
        ("UH_CHM_03a_0", 0.8),
        ("UH_CHM_05a_0", 1.0),
        ("UH_CHM_06a_0", 1.0),
    ],
    &[
        ("UH_CHF_02a_0", 0.9),
        ("UH_CHF_04a_0", 0.7),
        ("UH_CHF_01a_0", 0.8),
        ("UH_CHF_03a_0", 0.7),
    ],
    &[
        ("UH_CDM_04a_0", 0.6),
        ("UH_CDM_03a_0", 0.8),
        ("UH_CDM_08a_0", 0.6),
        ("UH_CDM_09a_0", 0.7),
        ("UH_CDM_01a_0", 1.0),
        ("UH_CDM_02a_0", 0.7),
    ],
    &[
        ("UH_CDF_06a_0", 0.1),
        ("UH_CDF_03a_0", 0.6),
        ("UH_CDF_05a_0", 0.5),
        ("UH_CDF_01a_0", 0.7),
        ("UH_CDF_01a_0", 0.5),
        ("UH_CDF_04a_0", 0.6),
    ],
    &[
        ("UH_CEM_04a_0", 0.7),
        ("UH_CEM_03a_0", 0.8),
        ("UH_CEM_05a_0", 0.8),
        ("UH_CEM_02a_0", 0.8),
        ("UH_CEM_01a_0", 0.6),
    ],
    &[
        ("UH_CEF_03a_0", 0.6),
        ("UH_CEF_02a_0", 0.7),
        ("UH_CEF_06a_0", 0.7),
        ("UH_CEF_01a_0", 0.8),
        ("UH_CEF_04a_0", 0.8),
        ("UH_CEF_05a_0", 0.8),
    ],
];

/// Eye Shape's and Nose Shape's slider ids.
pub const EYE_SHAPE: u32 = 10;
pub const NOSE_SHAPE: u32 = 30;

/// A shape list's choices for a race and gender row: each choice's targets and weight.
fn shape_choices(slider: u32, row: usize) -> Vec<(Vec<&'static str>, f32)> {
    match slider {
        EYE_SHAPE => EYE_SHAPES
            .get(row)
            .map(|r| r.iter().map(|(h, l, w)| (vec![*h, *l], *w)).collect())
            .unwrap_or_default(),
        NOSE_SHAPE => NOSE_SHAPES
            .get(row)
            .map(|r| r.iter().map(|(t, w)| (vec![*t], *w)).collect())
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// How many choices a shape slider has past its 0 (none) for a race and gender row.
pub fn shape_count(slider: u32, row: usize) -> usize {
    shape_choices(slider, row).len()
}

/// The weights a shape slider at `v` gives: every choice's targets at 0, then the chosen
/// one's (`v` from 1) at its weight.
pub fn shape_weights(slider: u32, v: f32, row: usize) -> Vec<(String, f32)> {
    let choices = shape_choices(slider, row);
    let chosen = (v.round() as i64 - 1).max(-1);
    let mut out = Vec::new();
    for (i, (names, w)) in choices.iter().enumerate() {
        for n in names {
            let weight = if i as i64 == chosen { *w } else { 0.0 };
            match out.iter_mut().find(|(t, _): &&mut (String, f32)| t == n) {
                Some(e) => e.1 = e.1.max(weight),
                None => out.push((n.to_string(), weight)),
            }
        }
    }
    out
}

/// Where a shape slider stands on a face weighing `weights`: the first choice whose
/// target is weighed (from 1), else 0.
pub fn shape_value(slider: u32, weights: &[(String, f32)], row: usize) -> Option<f32> {
    let choices = shape_choices(slider, row);
    if choices.is_empty() {
        return None;
    }
    let weighed = |t: &str| {
        weights
            .iter()
            .any(|(n, w)| n.eq_ignore_ascii_case(t) && *w > 0.01)
    };
    Some(
        choices
            .iter()
            .position(|(names, _)| weighed(names[0]))
            .map_or(0.0, |i| (i + 1) as f32),
    )
}

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
        o.targets.extend(shape_weights(slider, v, row));
    }
    // Makeup intensities: the face's tint opacity for that channel (the exe's setters
    // 0x8041d0 lips, 0x803450 eye shadow, 0x8040d0 blush write FaceM1's
    // mml_vTintMaskColours components 0, 1 and 2).
    for (slider, index) in [
        (LIP_INTENSITY, 0),
        (EYE_SHADOW_INTENSITY, 1),
        (BLUSH_INTENSITY, 2),
    ] {
        if let Some(&v) = obj.face_choices.get(&slider) {
            o.scalars.push((
                "FaceM1".into(),
                "mml_vTintMaskColours".into(),
                index,
                v.clamp(0.0, 1.0),
            ));
        }
    }
    // Complexion: the face's diffuse texture (0x805800 writes FaceM1's mml_tDiffuse).
    if let Some(&v) = obj.face_choices.get(&COMPLEXION) {
        if let Some(t) = sim
            .face_list(id, "skins")
            .get(v.max(0.0).round() as usize)
            .filter(|t| !t.is_empty())
        {
            o.textures
                .push(("FaceM1".into(), "mml_tDiffuse".into(), t.clone()));
            // Its normal map goes with it, as the presets pair them (`uh_hed_fema_0d`,
            // `uh_hed_fema_0n.dds`). Provisional: the exe's setter's second write is not
            // traced.
            if let Some(stem) = t.strip_suffix("_0d") {
                o.textures.push((
                    "FaceM1".into(),
                    "mml_tNormalMap".into(),
                    format!("{stem}_0n.dds"),
                ));
            }
        }
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
        // Shapes: 0 is none; a choice weighs its targets and zeroes the rest.
        let n = shape_weights(NOSE_SHAPE, 2.0, 0);
        assert_eq!(n.len(), 6);
        assert!(n.iter().any(|(t, w)| t == "UH_CHM_02a_0" && *w == 0.8));
        assert!(n.iter().filter(|(_, w)| *w > 0.0).count() == 1);
        assert_eq!(shape_value(NOSE_SHAPE, &n, 0), Some(2.0));
        assert!(
            shape_weights(EYE_SHAPE, 0.0, 1)
                .iter()
                .all(|(_, w)| *w == 0.0)
        );
        assert_eq!(shape_weights(EYE_SHAPE, 1.0, 1).len(), 8);
        assert_eq!(shape_count(NOSE_SHAPE, 4), 5);
    }
}
