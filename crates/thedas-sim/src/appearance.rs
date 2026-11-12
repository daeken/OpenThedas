//! Character creation's appearance sliders: what the player chose for their face, kept on
//! the creature ([`crate::world::Object::face_choices`]) by slider id. The ids are the
//! exe's (its slider table, built at 0x80ada0 and grouped at 0x73a0c1); the menu's labels,
//! groups and ranges are the interface's (`thedas_ui`).
//!
//! What a choice does to the face: the preset puts that head on; hair and beard put their
//! models in the morph's slots; the colours put their tints in its tint slots. Provisional:
//! the makeup intensities, tattoos, complexion and the face-shape sliders are kept but not
//! yet put on the face (morph targets and material scalars, not traced yet).

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
