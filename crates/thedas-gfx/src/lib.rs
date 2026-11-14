//! OpenThedas's own Scaleform GFx player.
//!
//! Plays the game's movies (SWF version 8, ActionScript 2, with Scaleform's extra tags) without
//! Ruffle. Only what Dragon Age: Origins' movies use is implemented; see
//! `docs/scaleform-player.md` for the inventory and the design.

pub mod avm1;
pub mod display;
mod filters;
pub mod player;
pub mod render;
pub mod swf;
pub mod tess;
pub mod text;

pub use avm1::{Host, HostValue};
pub use player::Player;
