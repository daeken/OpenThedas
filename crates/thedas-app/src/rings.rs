//! The selection rings under creatures: the party's, and the red ones under enemies.
//!
//! Traced (by the interface stream; notes beside the test data, `selection-rings-trace.md`):
//! DAOrigins.exe 0x6aac50 picks a `VFX_base.gda` row for a creature and 0x6a5f90 puts it on
//! the creature as a visual effect, replacing the ring it had. Party members get row 30005,
//! or 30006 when they are among the selected; a hostile creature 30011, or 30012 while the
//! mouse is over it; a creature with neither gets 30002 under a condition that is not
//! traced. The rows' models (`SR_PTY`, `SR_EMY`, … with `_M` and `_L` variants) are flat
//! planes drawn with `GeneralGui.mat` (`Reticle.vsh`, `Reticle.psh`, `Reticle.sta`: blended,
//! without depth testing).
//!
//! Provisional: the conditions the trace could not name (creature +0xb80, +0x42c, +0x421:
//! here a living, active hostile creature has its ring while it fights or the mouse is
//! over it, and none is shown while a conversation, a cutscene or character creation is
//! up); "selected" is the controlled creature; the neutral
//! ring (30002) and the backstab rows (30025 on) are not shown; which size variant a
//! creature takes (the row's medium and large models are taken to be for bodies wider than
//! the number beside the model before them); and where the pass falls in the frame (the
//! materials' render class is `3dgui`; the rings are drawn after the level and before the
//! creatures, so that feet stand over them).

use thedas_script::vm::ObjectId;
use thedas_sim::Sim;
use thedas_sim::world::{Object, ObjectKind};

pub const ROW_PARTY: i32 = 30005;
pub const ROW_PARTY_SELECTED: i32 = 30006;
pub const ROW_ENEMY: i32 = 30011;
pub const ROW_ENEMY_TARGETED: i32 = 30012;

/// `VFX_base.gda`'s columns for a row's three sizes of model (names not known): the small
/// model, how wide a body it serves, the medium model, how wide, the large model.
const COL_SMALL: u32 = 0xe2c2_8829;
const COL_SMALL_WIDTH: u32 = 0x514c_c359;
const COL_MEDIUM: u32 = 0xc9ef_dbea;
const COL_MEDIUM_WIDTH: u32 = 0x7a61_909a;
const COL_LARGE: u32 = 0xd0f4_eaab;

/// The ring row creature `o` shows, if any; `hovered` is the creature under the mouse.
pub fn row(sim: &Sim, o: &Object, hovered: Option<ObjectId>) -> Option<i32> {
    if o.kind != ObjectKind::Creature || o.dead || !o.active {
        return None;
    }
    // The executable asks the game mode first (0x6aac50: no ring when its check fails).
    // Provisional: taken to fail in a cutscene, a conversation and character creation.
    if sim.world.cutscene.is_some()
        || sim.conversation_line().is_some()
        || sim.world.chargen.is_some()
    {
        return None;
    }
    let party = thedas_sim::engine::party_of(sim);
    let leader = sim.world.controlled().or(sim.world.hero)?;
    if party.contains(&o.id) {
        return Some(if o.id == leader {
            ROW_PARTY_SELECTED
        } else {
            ROW_PARTY
        });
    }
    if sim.hostile(leader, o.id) {
        if hovered == Some(o.id) {
            return Some(ROW_ENEMY_TARGETED);
        }
        // Provisional: the two flags the executable asks for (+0xb80, +0x42c) are taken
        // to be met by an enemy that is fighting.
        return o.combat.in_combat.then_some(ROW_ENEMY);
    }
    None
}

/// The model of ring `row` for a body `radius` metres wide (see the module docs).
pub fn model(sim: &Sim, row: i32, radius: f32) -> Option<String> {
    use thedas_formats::gda::Cell;
    let t = sim.world.tables().get(0, "vfx_base")?;
    let text = |hash: u32| match t.cell_by_hash(hash, row as i64)? {
        Cell::String(s) => {
            let s = s.trim_end_matches('\0').trim().to_ascii_lowercase();
            (!s.is_empty()).then_some(s)
        }
        _ => None,
    };
    let number = |hash: u32| match t.cell_by_hash(hash, row as i64)? {
        Cell::Float(v) => Some(*v as f32),
        Cell::Int(v) => Some(*v as f32),
        _ => None,
    };
    let small = text(COL_SMALL)?;
    let wider = |than: Option<f32>| than.is_some_and(|w| w > 0.0 && radius > w);
    if wider(number(COL_MEDIUM_WIDTH)) {
        if let Some(large) = text(COL_LARGE) {
            return Some(large);
        }
    }
    if wider(number(COL_SMALL_WIDTH)) {
        if let Some(medium) = text(COL_MEDIUM) {
            return Some(medium);
        }
    }
    Some(small)
}

/// Where the ids the rings' models are given start (see [`thedas_ui::models::ObjectModel`]):
/// above the visual effects at places.
pub const RING_IDS: ObjectId = 0xf800_0000;
