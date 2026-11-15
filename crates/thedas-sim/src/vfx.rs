//! How long an instant visual effect plays (notes/instant-vfx-lifetime.md).
//!
//! DAOrigins.exe plays a visual effect as a `CCVisualEffect` of its own, apart from the
//! script's effect. Its animation tree (`VFX_base.gda`'s `BlendTreeName`, 0x79dc03) names
//! a cast, an impact, a duration and a cessation animation (`vfxcast`, `vfximpact`,
//! `vfxduration`, `vfxcessation`: animations 500, 502, 501 and 503, state table 0xb23d94),
//! played in turn; each animation's end (event 13, 0x815e4a) moves the visual on
//! (0x7eac30), skipping the stages it has none for, and after the last it is taken away
//! (0x79e3a0 → 0x79df10), particles and all. Nothing waits for the last particle.
//!
//! An instant effect is a flash when its duration animation has no length and its impact
//! one has (0x7e9ad0): it plays its impact and cessation and goes. Otherwise the engine
//! makes the script's effect temporary for the impact's and duration's lengths together
//! (0x79faf0-0x79fbd2); when that runs out the duration stops and the cessation plays
//! (0x79e3e0).
//!
//! Not mirrored (provisional): the cast stage (taken as not played for an effect applied
//! as instant), the script-visible change of such an effect to temporary (the simulation
//! keeps only the note), and model variants by size.

use thedas_formats::anim::{AnimTree, Animation};

use crate::Sim;

/// `VFX_base.gda`'s `BlendTreeName` and `ModelName` columns (the hashes the exe looks up,
/// 0x7e9ee0).
const COL_BLEND_TREE: u32 = 0x1109_e419;
const COL_MODEL: u32 = 0xfd84_03d5;

/// How an instant visual effect plays, by its animations' lengths.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VfxTiming {
    /// How long it is shown, in milliseconds: the impact and cessation for a flash, the
    /// impact, duration and cessation otherwise.
    pub ms: u64,
    /// It has a duration animation, which loops until its time is up.
    pub lasting: bool,
}

/// What [`instant_timing`] gives an effect whose animations cannot be read: a guess
/// (there is no such case in the exe, whose missing lengths are zero).
pub const UNREAD_MS: u64 = 2_000;

impl Sim {
    /// How long `VFX_base` row `row`, played as an instant effect, is shown (see the module
    /// docs). An effect whose tree or animations are missing gets [`UNREAD_MS`].
    pub fn instant_vfx_timing(&self, row: i32) -> VfxTiming {
        self.read_vfx_timing(row).unwrap_or(VfxTiming {
            ms: UNREAD_MS,
            lasting: false,
        })
    }

    fn read_vfx_timing(&self, row: i32) -> Option<VfxTiming> {
        let t = self.world.tables().get(0, "vfx_base")?;
        let name = |hash: u32| match t.cell_by_hash(hash, i64::from(row)) {
            Some(thedas_formats::gda::Cell::String(s)) => {
                let s = s.trim_end_matches('\0').trim().to_ascii_lowercase();
                (!s.is_empty()).then_some(s)
            }
            _ => None,
        };
        let tree = name(COL_BLEND_TREE).or_else(|| name(COL_MODEL))?;
        let (data, _) = self.vfs.read(&format!("{tree}.anb")).ok()?;
        let tree = AnimTree::parse(&data).ok()?;
        let length = |action: &str| -> f32 {
            tree.action(action)
                .and_then(|n| n.animation.as_deref())
                .and_then(|a| self.vfs.read(a).ok())
                .and_then(|(d, _)| Animation::parse(&d).ok())
                .map_or(0.0, |a| a.length.max(0.0))
        };
        let (impact, duration, cessation) = (
            length("vfximpact"),
            length("vfxduration"),
            length("vfxcessation"),
        );
        let lasting = !(duration <= 0.0 && impact > 0.0);
        let seconds = impact + if lasting { duration } else { 0.0 } + cessation;
        Some(VfxTiming {
            ms: (seconds * 1000.0).round() as u64,
            lasting,
        })
    }
}
