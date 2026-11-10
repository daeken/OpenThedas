//! `script.ldf`: BioWare's list of engine functions ("actions") and predefined constants, in
//! NWScript declaration syntax:
//!
//! ```text
//! #define ENGINE_STRUCTURE_0  event
//! int OBJECT_TYPE_CREATURE = 32;
//! object GetObjectByTag( string sTag, int nNth = 0 ) = 211;
//! ```
//!
//! The file ships with the game (newest copy in `packages/core/patch/patch003.erf`), so the
//! engine-function table is built from it at run time instead of being generated into the
//! source tree. The text is Windows-1252; it is decoded as Latin-1, which only matters inside
//! comments.

use std::collections::HashMap;
use std::fmt;

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum LdfError {
    #[error("line {line}: {msg}")]
    Syntax { line: usize, msg: String },
}

/// An NWScript type as the engine table names it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Type {
    Void,
    Int,
    Float,
    String,
    Object,
    Resource,
    Vector,
    /// Any single value (a couple of debugging functions take this).
    Any,
    /// Engine structure `n` (`ENGINE_STRUCTURE_n`: event, location, command, effect,
    /// itemproperty, player).
    Engine(u8),
    Array(Box<Type>),
}

impl Type {
    /// Stack cells a value of this type occupies (each cell is 4 bytes in the bytecode).
    pub fn cells(&self) -> usize {
        match self {
            Type::Void => 0,
            Type::Vector => 3,
            _ => 1,
        }
    }
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Type::Void => write!(f, "void"),
            Type::Int => write!(f, "int"),
            Type::Float => write!(f, "float"),
            Type::String => write!(f, "string"),
            Type::Object => write!(f, "object"),
            Type::Resource => write!(f, "resource"),
            Type::Vector => write!(f, "vector"),
            Type::Any => write!(f, "any"),
            Type::Engine(n) => write!(f, "engine{n}"),
            Type::Array(t) => write!(f, "{t}[]"),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Param {
    pub ty: Type,
    pub name: String,
    /// The default as written in the declaration (`0`, `TRUE`, `"", [0.0, 0.0, 0.0]`).
    pub default: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Function {
    pub id: u16,
    pub name: String,
    pub ret: Type,
    pub params: Vec<Param>,
}

impl Function {
    pub fn required(&self) -> usize {
        self.params
            .iter()
            .take_while(|p| p.default.is_none())
            .count()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ConstValue {
    Int(i64),
    Float(f32),
    String(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Constant {
    pub ty: Type,
    pub name: String,
    pub value: ConstValue,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct EngineTable {
    /// Engine structure names by index.
    pub structures: Vec<String>,
    pub constants: Vec<Constant>,
    pub functions: Vec<Function>,
    by_id: HashMap<u16, usize>,
    by_name: HashMap<String, usize>,
}

impl EngineTable {
    pub fn function(&self, id: u16) -> Option<&Function> {
        self.by_id.get(&id).map(|&i| &self.functions[i])
    }

    pub fn function_by_name(&self, name: &str) -> Option<&Function> {
        self.by_name.get(name).map(|&i| &self.functions[i])
    }

    /// Adds or replaces a function.
    pub fn insert(&mut self, f: Function) {
        match self.by_id.get(&f.id) {
            Some(&i) => self.functions[i] = f.clone(),
            None => {
                self.by_id.insert(f.id, self.functions.len());
                self.functions.push(f.clone());
            }
        }
        self.by_name.insert(f.name.clone(), self.by_id[&f.id]);
    }

    pub fn constant(&self, name: &str) -> Option<&Constant> {
        self.constants.iter().find(|c| c.name == name)
    }

    pub fn type_name(&self, t: &Type) -> String {
        match t {
            Type::Engine(n) => self
                .structures
                .get(*n as usize)
                .cloned()
                .unwrap_or_else(|| format!("engine{n}")),
            Type::Array(e) => format!("{}[]", self.type_name(e)),
            other => other.to_string(),
        }
    }

    /// Parses `script.ldf`.
    pub fn parse(data: &[u8]) -> Result<Self, LdfError> {
        let text: String = data.iter().map(|&b| b as char).collect();
        let text = strip_comments(&text);
        let mut table = EngineTable::default();
        let mut structures: HashMap<usize, String> = HashMap::new();
        // Preprocessor lines first.
        let mut body = String::new();
        for line in text.lines() {
            let t = line.trim();
            if let Some(rest) = t.strip_prefix("#define") {
                let mut it = rest.split_whitespace();
                if let (Some(k), Some(v)) = (it.next(), it.next()) {
                    if let Some(n) = k.strip_prefix("ENGINE_STRUCTURE_") {
                        if let Ok(n) = n.parse::<usize>() {
                            structures.insert(n, v.to_string());
                        }
                    }
                }
                body.push('\n');
            } else {
                body.push_str(line);
                body.push('\n');
            }
        }
        let n = structures.keys().max().map_or(0, |m| m + 1);
        table.structures = (0..n)
            .map(|i| structures.get(&i).cloned().unwrap_or_default())
            .collect();

        let mut line = 1;
        for stmt in split_statements(&body) {
            let start_line = line;
            line += stmt.matches('\n').count();
            let s = stmt.trim();
            if s.is_empty() {
                continue;
            }
            let lead = stmt[..stmt.len() - stmt.trim_start().len()]
                .matches('\n')
                .count();
            let err = |msg: String| LdfError::Syntax {
                line: start_line + lead,
                msg,
            };
            if s.contains('(') {
                let f = parse_function(&table, s).map_err(err)?;
                if table.by_id.contains_key(&f.id) {
                    // Later declarations win, as they would in a compiler symbol table.
                    let i = table.by_id[&f.id];
                    table.functions[i] = f.clone();
                } else {
                    table.by_id.insert(f.id, table.functions.len());
                    table.functions.push(f.clone());
                }
                table.by_name.insert(f.name.clone(), table.by_id[&f.id]);
            } else {
                table
                    .constants
                    .push(parse_constant(&table, s).map_err(err)?);
            }
        }
        Ok(table)
    }
}

fn strip_comments(t: &str) -> String {
    let mut out = String::with_capacity(t.len());
    let b: Vec<char> = t.chars().collect();
    let mut i = 0;
    let mut in_str = false;
    while i < b.len() {
        let c = b[i];
        if in_str {
            out.push(c);
            if c == '\\' && i + 1 < b.len() {
                out.push(b[i + 1]);
                i += 2;
                continue;
            }
            if c == '"' {
                in_str = false;
            }
            i += 1;
        } else if c == '"' {
            in_str = true;
            out.push(c);
            i += 1;
        } else if c == '/' && b.get(i + 1) == Some(&'/') {
            while i < b.len() && b[i] != '\n' {
                i += 1;
            }
        } else if c == '/' && b.get(i + 1) == Some(&'*') {
            i += 2;
            while i < b.len() && !(b[i] == '*' && b.get(i + 1) == Some(&'/')) {
                if b[i] == '\n' {
                    out.push('\n');
                }
                i += 1;
            }
            i += 2;
        } else {
            out.push(if c == '\r' { '\n' } else { c });
            i += 1;
        }
    }
    out
}

/// Splits on `;` outside string literals.
fn split_statements(t: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_str = false;
    for c in t.chars() {
        match c {
            '"' => {
                in_str = !in_str;
                cur.push(c);
            }
            ';' if !in_str => out.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur);
    }
    out
}

fn parse_type(table: &EngineTable, s: &str) -> Result<Type, String> {
    let s = s.trim();
    if let Some(base) = s.strip_suffix("[]") {
        return Ok(Type::Array(Box::new(parse_type(table, base)?)));
    }
    Ok(match s {
        "void" => Type::Void,
        "int" => Type::Int,
        "float" => Type::Float,
        "string" => Type::String,
        "object" => Type::Object,
        "resource" => Type::Resource,
        "vector" => Type::Vector,
        "any" => Type::Any,
        other => match table.structures.iter().position(|n| n == other) {
            Some(i) => Type::Engine(i as u8),
            None => return Err(format!("unknown type {other:?}")),
        },
    })
}

/// Splits a declaration's leading type from the rest: `object [] Foo` and `int[] Foo` both
/// have array types.
fn split_type(s: &str) -> Option<(String, &str)> {
    let s = s.trim_start();
    let end = s.find(|c: char| c.is_whitespace() || c == '[')?;
    let mut ty = s[..end].to_string();
    let mut rest = &s[end..];
    let r = rest.trim_start();
    if let Some(after) = r.strip_prefix('[') {
        let after = after.trim_start();
        if let Some(after) = after.strip_prefix(']') {
            ty.push_str("[]");
            rest = after;
        }
    }
    Some((ty, rest))
}

fn parse_function(table: &EngineTable, s: &str) -> Result<Function, String> {
    let open = s.find('(').ok_or("no '('")?;
    let close = s.rfind(')').ok_or("no ')'")?;
    if close < open {
        return Err("mismatched parentheses".into());
    }
    let (ret, name) = split_type(&s[..open]).ok_or("no return type")?;
    let name = name.trim().to_string();
    let id = s[close + 1..]
        .trim()
        .strip_prefix('=')
        .ok_or("no '= id'")?
        .trim()
        .parse::<u16>()
        .map_err(|e| format!("bad id: {e}"))?;
    let mut params = Vec::new();
    for p in split_params(&s[open + 1..close]) {
        let p = p.trim();
        if p.is_empty() {
            continue;
        }
        let (decl, default) = match p.find('=') {
            Some(i) => (&p[..i], Some(p[i + 1..].trim().to_string())),
            None => (p, None),
        };
        let (ty, pname) = split_type(decl).ok_or_else(|| format!("bad parameter {p:?}"))?;
        params.push(Param {
            ty: parse_type(table, &ty)?,
            name: pname.trim().to_string(),
            default,
        });
    }
    Ok(Function {
        id,
        name,
        ret: parse_type(table, &ret)?,
        params,
    })
}

/// Splits parameters on commas outside brackets and strings (vector defaults are
/// `[0.0, 0.0, 0.0]`).
fn split_params(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let (mut depth, mut in_str) = (0, false);
    for c in s.chars() {
        match c {
            '"' => {
                in_str = !in_str;
                cur.push(c);
            }
            '[' if !in_str => {
                depth += 1;
                cur.push(c);
            }
            ']' if !in_str => {
                depth -= 1;
                cur.push(c);
            }
            ',' if !in_str && depth == 0 => out.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    out.push(cur);
    out
}

fn parse_constant(table: &EngineTable, s: &str) -> Result<Constant, String> {
    let eq = s.find('=').ok_or("constant without '='")?;
    let (ty, name) = split_type(&s[..eq]).ok_or("no type")?;
    let ty = parse_type(table, &ty)?;
    let v = s[eq + 1..].trim();
    // A constant may be defined as an earlier one (`int X = TRUE;`).
    if v.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && v.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
    {
        let c = table
            .constant(v)
            .ok_or_else(|| format!("unknown constant {v:?}"))?;
        return Ok(Constant {
            ty,
            name: name.trim().to_string(),
            value: c.value.clone(),
        });
    }
    let value = match ty {
        Type::Int => {
            let v = v.trim_end_matches(['u', 'U', 'l', 'L']);
            let n = if let Some(h) = v.strip_prefix("0x").or_else(|| v.strip_prefix("0X")) {
                i64::from_str_radix(h, 16)
            } else {
                v.parse::<i64>()
            }
            .map_err(|e| format!("bad int {v:?}: {e}"))?;
            ConstValue::Int(n)
        }
        Type::Float => ConstValue::Float(
            v.trim_end_matches(['f', 'F'])
                .parse()
                .map_err(|e| format!("bad float {v:?}: {e}"))?,
        ),
        Type::String => ConstValue::String(v.trim_matches('"').to_string()),
        other => return Err(format!("constant of type {other}")),
    };
    Ok(Constant {
        ty,
        name: name.trim().to_string(),
        value,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const LDF: &str = "// header\r\n#define ENGINE_NUM_STRUCTURES 2\r\n#define ENGINE_STRUCTURE_0  event\r\n#define ENGINE_STRUCTURE_1  location\r\n\r\nint TRUE = 1;\r\nint YES = TRUE;\r\nfloat HALF = 0.5f;\r\nint OBJECT_TYPE_ALL = 4294967295;\r\nstring EMPTY = \"a;b\";\r\n/** doc with ; and ( */\r\nvoid PrintInteger( int nInteger, int bPrepend = FALSE ) = 0;\r\nobject [] GetThings( object oAOE ) = 337;\r\nint[] GetInts(vector v = [0.0, 0.0, 0.0], location l) = 5;\r\nevent Event(int n) = 6;\r\n";

    #[test]
    fn parses_constants_and_functions() {
        let t = EngineTable::parse(LDF.as_bytes()).unwrap();
        assert_eq!(t.structures, vec!["event", "location"]);
        assert_eq!(t.constant("TRUE").unwrap().value, ConstValue::Int(1));
        assert_eq!(t.constant("YES").unwrap().value, ConstValue::Int(1));
        assert_eq!(t.constant("HALF").unwrap().value, ConstValue::Float(0.5));
        assert_eq!(
            t.constant("OBJECT_TYPE_ALL").unwrap().value,
            ConstValue::Int(4294967295)
        );
        assert_eq!(
            t.constant("EMPTY").unwrap().value,
            ConstValue::String("a;b".into())
        );
        let f = t.function(0).unwrap();
        assert_eq!(f.name, "PrintInteger");
        assert_eq!(f.required(), 1);
        assert_eq!(f.params[1].default.as_deref(), Some("FALSE"));
        assert_eq!(
            t.function(337).unwrap().ret,
            Type::Array(Box::new(Type::Object))
        );
        let g = t.function_by_name("GetInts").unwrap();
        assert_eq!(g.ret, Type::Array(Box::new(Type::Int)));
        assert_eq!(g.params[0].ty, Type::Vector);
        assert_eq!(g.params[0].default.as_deref(), Some("[0.0, 0.0, 0.0]"));
        assert_eq!(g.params[1].ty, Type::Engine(1));
        assert_eq!(t.type_name(&t.function(6).unwrap().ret), "event");
    }

    #[test]
    fn rejects_bad_declarations() {
        assert!(EngineTable::parse(b"void F( wibble x ) = 1;").is_err());
        assert!(EngineTable::parse(b"void F( int x ) = nope;").is_err());
        assert!(EngineTable::parse(b"int X = abc;").is_err());
        assert!(EngineTable::parse(b"int X = 1 + 2;").is_err());
    }
}
