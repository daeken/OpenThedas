//! Source-level Python 2.5 interpreter for EclipseRay-style scripts.
//!
//! The Rust evaluator walks an owned AST lowered from Tree-sitter's Python
//! grammar. The implementation targets lightmapper drivers and the Python 2.5
//! features they exercise. It accepts UTF-8 source and reports unsupported
//! constructs as host errors with source line information.
//!
//! Register host modules such as `aergia` before running a driver. The caller
//! supplies argv, controls subprocess execution, and may resolve additional
//! imports from Python source. Built-in filesystem operations use host paths.
//! An embedding can enable case-insensitive reads for legacy ASCII asset names.
//!
//! ```
//! use thedas_python25::{Runtime, Value};
//!
//! let mut vm = Runtime::new([Value::bytes("--cpus=2")]);
//! vm.register_module("example", Value::module([(
//!     "double",
//!     Value::function(|_, args| {
//!         let [value] = args.take()?;
//!         Ok(Value::int(value.as_int()? << 1))
//!     }),
//! )]))?;
//! vm.run("import example\nresult = example.double(7 / 2)\n")?;
//! assert_eq!(vm.global("result").unwrap().as_i64()?, 6);
//! # Ok::<(), thedas_python25::Error>(())
//! ```
//!
//! This is an interpreter for local tooling. Registered callbacks and built-in
//! filesystem methods execute with the embedding process's privileges.

mod ast;
mod ops;
mod runtime;
mod stdlib;
mod value;

pub use runtime::{ModuleSource, ProcessOutput, Runtime};
pub use value::{Args, Error, NativeObject, Result, Value};
