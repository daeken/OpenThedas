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

pub mod aoe;
pub mod appearance;
pub mod chanters;
pub mod chargen;
pub mod collision;
pub mod combat;
pub mod cutscene;
pub mod cuttracks;
pub mod derived;
pub mod dialog;
pub mod engine;
pub mod inventory;
pub mod journal;
pub mod levelup;
pub mod m2da;
pub mod nav;
pub mod orders;
pub mod plot;
pub mod quickbar;
pub mod save;
pub mod session;
pub mod sim;
pub mod store;
pub mod tactics;
pub mod threat;
pub mod trace;
pub mod world;
pub mod worldmap;

pub use sim::Sim;
pub use world::World;
