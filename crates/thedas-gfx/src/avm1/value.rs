//! AVM1 values and the primitive conversions (SWF 7+ rules: case-sensitive, `""` to NaN).

use std::rc::Rc;

pub type Str = Rc<str>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ObjId(pub u32);

#[derive(Clone, Debug, Default)]
pub enum Value {
    #[default]
    Undefined,
    Null,
    Bool(bool),
    Number(f64),
    String(Str),
    Object(ObjId),
}

impl PartialEq for Value {
    /// Strict equality (`===`), with NaN unequal to itself.
    fn eq(&self, o: &Self) -> bool {
        match (self, o) {
            (Value::Undefined, Value::Undefined) | (Value::Null, Value::Null) => true,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Number(a), Value::Number(b)) => a == b,
            (Value::String(a), Value::String(b)) => a == b,
            (Value::Object(a), Value::Object(b)) => a == b,
            _ => false,
        }
    }
}

impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Value::String(Rc::from(s))
    }
}

impl From<String> for Value {
    fn from(s: String) -> Self {
        Value::String(Rc::from(s))
    }
}

impl From<f64> for Value {
    fn from(n: f64) -> Self {
        Value::Number(n)
    }
}

impl From<bool> for Value {
    fn from(b: bool) -> Self {
        Value::Bool(b)
    }
}

impl From<ObjId> for Value {
    fn from(o: ObjId) -> Self {
        Value::Object(o)
    }
}

impl Value {
    pub fn str(s: &str) -> Self {
        s.into()
    }

    pub fn is_undefined(&self) -> bool {
        matches!(self, Value::Undefined)
    }

    pub fn is_nullish(&self) -> bool {
        matches!(self, Value::Undefined | Value::Null)
    }

    pub fn as_object(&self) -> Option<ObjId> {
        match self {
            Value::Object(o) => Some(*o),
            _ => None,
        }
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Undefined => "undefined",
            Value::Null => "null",
            Value::Bool(_) => "boolean",
            Value::Number(_) => "number",
            Value::String(_) => "string",
            Value::Object(_) => "object",
        }
    }
}

/// `ToNumber` for a string (SWF 7+: empty or blank is NaN; `0x` hex; leading/trailing
/// whitespace allowed).
pub fn string_to_number(s: &str) -> f64 {
    let t = s.trim_matches(|c: char| c.is_whitespace());
    if t.is_empty() {
        return f64::NAN;
    }
    if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        return i64::from_str_radix(h, 16)
            .map(|v| v as i32 as f64)
            .unwrap_or(f64::NAN);
    }
    let (neg, body) = match t.as_bytes()[0] {
        b'-' => (true, &t[1..]),
        b'+' => (false, &t[1..]),
        _ => (false, t),
    };
    if body == "Infinity" {
        return if neg {
            f64::NEG_INFINITY
        } else {
            f64::INFINITY
        };
    }
    // Only plain decimal forms (Rust also accepts "inf"/"nan", which Flash does not).
    if !body
        .bytes()
        .all(|b| b.is_ascii_digit() || matches!(b, b'.' | b'e' | b'E' | b'+' | b'-'))
        || !body
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_digit() || b == b'.')
    {
        return f64::NAN;
    }
    match body.parse::<f64>() {
        Ok(v) => {
            if neg {
                -v
            } else {
                v
            }
        }
        Err(_) => f64::NAN,
    }
}

/// `ToString` for a number, as Flash formats it (15 significant digits, exponent outside
/// 1e-5..1e15).
pub fn number_to_string(n: f64) -> String {
    if n.is_nan() {
        return "NaN".into();
    }
    if n.is_infinite() {
        return if n > 0.0 { "Infinity" } else { "-Infinity" }.into();
    }
    if n == 0.0 {
        return "0".into();
    }
    if n.fract() == 0.0 && n.abs() < 1e15 {
        return format!("{}", n as i64);
    }
    // Round to 15 significant digits.
    let s = format!("{:.14e}", n);
    let (mant, exp) = s.split_once('e').unwrap_or((&s, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let neg = mant.starts_with('-');
    let digits: String = mant.chars().filter(|c| c.is_ascii_digit()).collect();
    let digits = digits.trim_end_matches('0');
    let digits = if digits.is_empty() { "0" } else { digits };
    let sign = if neg { "-" } else { "" };
    if (-5..15).contains(&exp) {
        if exp >= 0 {
            let e = exp as usize + 1;
            if digits.len() <= e {
                format!("{sign}{}{}", digits, "0".repeat(e - digits.len()))
            } else {
                format!("{sign}{}.{}", &digits[..e], &digits[e..])
            }
        } else {
            format!("{sign}0.{}{}", "0".repeat((-exp - 1) as usize), digits)
        }
    } else {
        let (a, b) = digits.split_at(1);
        let frac = if b.is_empty() {
            String::new()
        } else {
            format!(".{b}")
        };
        let es = if exp > 0 {
            format!("+{exp}")
        } else {
            exp.to_string()
        };
        format!("{sign}{a}{frac}e{es}")
    }
}

/// ECMA `ToInt32`.
pub fn to_int32(n: f64) -> i32 {
    if !n.is_finite() {
        return 0;
    }
    let m = n.trunc().rem_euclid(4294967296.0);
    m as u32 as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_format_like_flash() {
        assert_eq!(number_to_string(1.0), "1");
        assert_eq!(number_to_string(-2.5), "-2.5");
        assert_eq!(number_to_string(0.1 + 0.2), "0.3");
        assert_eq!(number_to_string(1e21), "1e+21");
        assert_eq!(number_to_string(0.000001), "1e-6");
        assert_eq!(number_to_string(0.0001), "0.0001");
        assert_eq!(number_to_string(123456.789), "123456.789");
    }

    #[test]
    fn strings_convert_like_flash() {
        assert!(string_to_number("").is_nan());
        assert!(string_to_number("abc").is_nan());
        assert!(string_to_number("inf").is_nan());
        assert_eq!(string_to_number(" 42 "), 42.0);
        assert_eq!(string_to_number("0x10"), 16.0);
        assert_eq!(string_to_number("-1.5e2"), -150.0);
        assert_eq!(to_int32(4294967295.0), -1);
    }
}
