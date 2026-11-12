//! Character creation's appearance sliders: what the player chose for their face, kept on
//! the creature ([`crate::world::Object::face_choices`]) by slider id. The ids are the
//! exe's (its slider table, built at 0x80ada0 and grouped at 0x73a0c1); the menu's labels,
//! groups and ranges are the interface's (`thedas_ui`).
//!
//! What a choice does to the face: the preset puts that head on; hair and beard put their
//! models in the morph's slots. Provisional: the colour, tattoo, makeup and complexion
//! choices and the face-shape sliders are kept but not yet put on the face (the exe sets
//! them through the head materials' colour slots and morph targets, not traced yet).

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
    o
}
