//! NWScript as Dragon Age: Origins runs it.
//!
//! - [`ldf`]: `script.ldf`, the engine-function table (signatures, ids) and constants.
//! - [`ncs`]: compiled script bytecode: parsing and disassembly.
//! - [`vm`]: the stack machine. Engine functions go to a [`vm::Host`], which the simulation
//!   implements; the VM itself is deterministic and headless.

pub mod ldf;
pub mod ncs;
pub mod vm;

pub use ldf::EngineTable;
pub use ncs::Ncs;
pub use vm::{Host, Value, Vm};
