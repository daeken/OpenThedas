//! The headless, deterministic simulation of a Dragon Age: Origins world.
//!
//! - [`world`]: objects, locals, engine values (events, locations, effects, commands), time and
//!   the event queue.
//! - [`sim`]: loading modules and areas, running scripts on the NWScript VM, delivering events
//!   and ticking.
//! - [`engine`]: engine functions scripts call, by name, in call-frequency order.
//! - [`m2da`]: merged 2DA tables. [`plot`]: plot flags and plot scripts. [`dialog`]:
//!   conversations.
//! - [`trace`]: what happened, as JSON lines.
//!
//! Nothing here reads the wall clock, the frame rate or a GPU: time advances only through
//! [`Sim::tick`], and randomness comes from a seeded generator.

pub mod ambient;
pub mod aoe;
pub mod appearance;
pub mod chanters;
pub mod chargen;
pub mod cnv;
pub mod collision;
pub mod combat;
pub mod commands;
pub mod contact;
pub mod container;
pub mod cutactors;
pub mod cutscene;
pub mod cuttracks;
mod da2_ai_level;
mod da2_animation;
mod da2_items;
pub mod da2_locomotion;
mod da2_projectile;
mod da2_properties;
pub mod decay;
pub mod derived;
pub mod dialog;
mod encounter;
pub mod engine;
mod engine_equality;
pub mod fmath;
mod heartbeat;
pub mod inventory;
pub mod journal;
pub mod levelup;
pub mod m2da;
mod native_queries;
pub mod nav;
pub mod orders;
mod placeable;
pub mod plot;
pub mod prcscr;
pub mod quickbar;
pub mod replay;
pub mod save;
pub mod session;
pub mod sim;
pub mod store;
pub mod tactics;
pub mod threat;
pub mod trace;
pub mod training;
pub mod vfx;
pub mod voice;
pub mod world;
pub mod worldmap;

pub use sim::Sim;
pub use world::World;
