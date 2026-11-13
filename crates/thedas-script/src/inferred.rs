//! Engine functions the shipped `script.ldf` does not declare, with signatures inferred from
//! how compiled scripts call them. The GOG 1.05 engine has functions newer than the table it
//! ships: even core `creature_core.ncs` calls #862. Everything here is provisional; names are
//! descriptive guesses, not BioWare's.
//!
//! - #859 `(object, int, int) -> int[]`: the result is stored in an `int[]` local
//!   (`gxa_module_chargen`).
//! - #860 `(int, float, float, int, object, string) -> void` (`vgk400ar_siege_ext_a`).
//! - #862 `(string) -> int`: called with `"core_ep1"` and tested directly; reads like "is this
//!   package loaded" (`creature_core`, `placeable_core`).
//! - `CommandFly` takes a second `int` in DLC scripts.

use crate::ldf::{EngineTable, Function, Param, Type};

fn p(ty: Type, name: &str, default: Option<&str>) -> Param {
    Param {
        ty,
        name: name.into(),
        default: default.map(str::to_string),
        by_ref: false,
    }
}

/// The inferred functions, for ids the table lacks.
pub fn inferred() -> Vec<Function> {
    vec![
        Function {
            id: 859,
            name: "Inferred859_GetIntArray".into(),
            ret: Type::Array(Box::new(Type::Int)),
            params: vec![
                p(Type::Object, "oObject", None),
                p(Type::Int, "n1", None),
                p(Type::Int, "n2", None),
            ],
        },
        Function {
            id: 860,
            name: "Inferred860".into(),
            ret: Type::Void,
            params: vec![
                p(Type::Int, "n1", None),
                p(Type::Float, "f1", None),
                p(Type::Float, "f2", None),
                p(Type::Int, "n2", None),
                p(Type::Object, "oObject", None),
                p(Type::String, "s1", None),
            ],
        },
        Function {
            id: 862,
            name: "IsPackageLoaded".into(),
            ret: Type::Int,
            params: vec![p(Type::String, "sPackage", None)],
        },
    ]
}

impl EngineTable {
    /// Adds the inferred functions the table does not declare, and the extra `CommandFly`
    /// parameter. Opt-in: corpus checks use the table as shipped.
    pub fn with_inferred(mut self) -> Self {
        for f in inferred() {
            if self.function(f.id).is_none() {
                self.insert(f);
            }
        }
        if let Some(f) = self.function_by_name("CommandFly").cloned() {
            if f.params.len() == 1 {
                let mut f = f;
                f.params.push(p(Type::Int, "nInferred", Some("0")));
                self.insert(f);
            }
        }
        self
    }
}
