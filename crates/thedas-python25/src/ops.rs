use std::{cmp::Ordering, rc::Rc};

use num_bigint::BigInt;
use num_integer::Integer;
use num_traits::{FromPrimitive, ToPrimitive, Zero};

use crate::{Error, Result, Value, value::Kind};

pub(crate) fn integer(source: &str) -> Result<Value> {
    let source = source.trim_end_matches(['l', 'L']);
    let (radix, digits) = match source.as_bytes() {
        [b'0', b'x' | b'X', ..] => (16, &source[2..]),
        [b'0', b'b' | b'B', ..] => (2, &source[2..]),
        [b'0', b'o' | b'O', ..] => (8, &source[2..]),
        [b'0', _, ..] => (8, source),
        _ => (10, source),
    };
    if !digits.is_empty() && digits.chars().all(|c| c.is_digit(radix)) {
        if let Some(value) = BigInt::parse_bytes(digits.as_bytes(), radix) {
            return Ok(Value::int(value));
        }
    }
    Err(Error::new(
        "SyntaxError",
        format!("invalid integer {source}"),
    ))
}

pub(crate) fn literal(source: &str) -> Result<Value> {
    let start = source
        .find(['\'', '"'])
        .ok_or_else(|| Error::new("SyntaxError", "missing quote"))?;
    let prefix = source[..start].to_ascii_lowercase();
    let quote = source.as_bytes()[start];
    let quote_len = if source.as_bytes()[start..].starts_with(&[quote; 3]) {
        3
    } else {
        1
    };
    let content = &source[start + quote_len..source.len() - quote_len];
    let unicode = prefix.contains('u');
    if !["", "u", "r", "ur", "ru", "b", "br", "rb"].contains(&prefix.as_str()) {
        return Err(Error::unsupported(format!("string prefix {prefix}")));
    }
    let raw = prefix.contains('r');
    let mut result = Vec::new();
    let mut chars = content.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            push_char(&mut result, ch);
            continue;
        }
        let escape = chars
            .next()
            .ok_or_else(|| Error::new("SyntaxError", "incomplete escape"))?;
        if raw && !(unicode && matches!(escape, 'u' | 'U')) {
            result.push(b'\\');
            push_char(&mut result, escape);
            continue;
        }
        match escape {
            '\n' => {}
            'n' => result.push(b'\n'),
            'r' => result.push(b'\r'),
            't' => result.push(b'\t'),
            'a' => result.push(7),
            'b' => result.push(8),
            'f' => result.push(12),
            'v' => result.push(11),
            '\\' | '\'' | '"' => result.push(escape as u8),
            '0'..='7' => {
                let mut number = escape.to_digit(8).unwrap_or_default();
                for _ in 0..2 {
                    let Some(digit) = chars.peek().and_then(|ch| ch.to_digit(8)) else {
                        break;
                    };
                    chars.next();
                    number = number * 8 + digit;
                }
                push_escape(&mut result, number, unicode)?;
            }
            'x' | 'u' | 'U' if escape == 'x' || unicode => {
                let count = match escape {
                    'x' => 2,
                    'u' => 4,
                    _ => 8,
                };
                let mut number = 0;
                for _ in 0..count {
                    let digit = chars.next().and_then(|ch| ch.to_digit(16)).ok_or_else(|| {
                        Error::new("SyntaxError", "incomplete hexadecimal escape")
                    })?;
                    number = number * 16 + digit;
                }
                push_escape(&mut result, number, unicode)?;
            }
            'N' if unicode => return Err(Error::unsupported("named Unicode escape")),
            _ => {
                result.push(b'\\');
                push_char(&mut result, escape);
            }
        }
    }
    if unicode {
        Ok(Value::unicode(String::from_utf8(result).map_err(|_| {
            Error::new("UnicodeDecodeError", "invalid Unicode string")
        })?))
    } else {
        Ok(Value::bytes(result))
    }
}

fn push_char(out: &mut Vec<u8>, ch: char) {
    out.extend_from_slice(ch.encode_utf8(&mut [0; 4]).as_bytes());
}

fn push_escape(out: &mut Vec<u8>, number: u32, unicode: bool) -> Result<()> {
    if unicode {
        push_char(
            out,
            char::from_u32(number)
                .ok_or_else(|| Error::unsupported("Unicode surrogate or invalid code point"))?,
        );
    } else {
        out.push(number as u8);
    }
    Ok(())
}

pub(crate) fn is(left: &Value, right: &Value) -> bool {
    match (&left.0, &right.0) {
        (Kind::None, Kind::None) => true,
        (Kind::Bool(a), Kind::Bool(b)) => a == b,
        (Kind::Bytes(a), Kind::Bytes(b)) | (Kind::Buffer(a), Kind::Buffer(b)) => Rc::ptr_eq(a, b),
        (Kind::Unicode(a), Kind::Unicode(b)) => Rc::ptr_eq(a, b),
        (Kind::Tuple(a), Kind::Tuple(b)) => Rc::ptr_eq(a, b),
        (Kind::List(a), Kind::List(b)) => Rc::ptr_eq(a, b),
        (Kind::Dict(a), Kind::Dict(b)) => Rc::ptr_eq(a, b),
        (Kind::Module(a), Kind::Module(b)) => Rc::ptr_eq(a, b),
        (Kind::Class { attrs: a, .. }, Kind::Class { attrs: b, .. })
        | (Kind::Instance { attrs: a, .. }, Kind::Instance { attrs: b, .. }) => Rc::ptr_eq(a, b),
        (Kind::Function(a), Kind::Function(b)) => Rc::ptr_eq(a, b),
        (Kind::Native(a), Kind::Native(b)) => Rc::ptr_eq(a, b),
        (Kind::Host(a), Kind::Host(b)) => Rc::ptr_eq(&a.native, &b.native),
        (Kind::ExceptionType(a), Kind::ExceptionType(b)) => a == b,
        (Kind::Exception(a), Kind::Exception(b)) => Rc::ptr_eq(a, b),
        _ => false,
    }
}

fn numeric_compare(left: &Value, right: &Value) -> Option<Option<Ordering>> {
    fn float_int(float: f64, int: &BigInt) -> Option<Ordering> {
        if float.is_nan() {
            return None;
        }
        if float.is_infinite() {
            return Some(if float > 0.0 {
                Ordering::Greater
            } else {
                Ordering::Less
            });
        }
        let whole = BigInt::from_f64(float)?;
        Some(
            whole
                .cmp(int)
                .then_with(|| float.fract().partial_cmp(&0.0).unwrap_or(Ordering::Equal)),
        )
    }
    match (&left.0, &right.0) {
        (Kind::Float(a), Kind::Float(b)) => Some(a.partial_cmp(b)),
        (Kind::Float(a), _) => Some(float_int(*a, &right.as_int().ok()?)),
        (_, Kind::Float(b)) => Some(float_int(*b, &left.as_int().ok()?).map(Ordering::reverse)),
        _ => Some(Some(left.as_int().ok()?.cmp(&right.as_int().ok()?))),
    }
}

pub(crate) fn eq(left: &Value, right: &Value) -> Result<bool> {
    equal(left, right, 0)
}

fn equal(left: &Value, right: &Value, depth: usize) -> Result<bool> {
    if depth > 128 {
        return Err(Error::new(
            "RuntimeError",
            "maximum comparison depth exceeded",
        ));
    }
    if let Some(order) = numeric_compare(left, right) {
        return Ok(order == Some(Ordering::Equal));
    }
    if let Kind::Host(object) = &left.0
        && let Some(order) = object.native.compare(right)
    {
        return Ok(order == Ordering::Equal);
    }
    if is(left, right) {
        return Ok(true);
    }
    fn sequence(a: &[Value], b: &[Value], depth: usize) -> Result<bool> {
        if a.len() != b.len() {
            return Ok(false);
        }
        for (x, y) in a.iter().zip(b) {
            if !equal(x, y, depth + 1)? {
                return Ok(false);
            }
        }
        Ok(true)
    }
    match (&left.0, &right.0) {
        (Kind::Bytes(a) | Kind::Buffer(a), Kind::Bytes(b) | Kind::Buffer(b)) => Ok(a == b),
        (Kind::Unicode(a), Kind::Unicode(b)) => Ok(a == b),
        (Kind::Bytes(a), Kind::Unicode(b)) | (Kind::Unicode(b), Kind::Bytes(a)) => {
            Ok(a.is_ascii() && a.as_ref() == b.as_bytes())
        }
        (Kind::Tuple(a), Kind::Tuple(b)) => sequence(a, b, depth),
        (Kind::List(a), Kind::List(b)) => sequence(&a.borrow(), &b.borrow(), depth),
        (Kind::Dict(a), Kind::Dict(b)) => {
            let a = a.borrow();
            let b = b.borrow();
            if a.len() != b.len() {
                return Ok(false);
            }
            for (key, value) in a.iter() {
                let Some(index) = dict_index(&b, key)? else {
                    return Ok(false);
                };
                if !equal(value, &b[index].1, depth + 1)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        _ => Ok(false),
    }
}

pub(crate) fn dict_index(values: &[(Value, Value)], key: &Value) -> Result<Option<usize>> {
    fn hashable(key: &Value) -> bool {
        match &key.0 {
            Kind::Dict(_) | Kind::List(_) => false,
            Kind::Tuple(items) => items.iter().all(hashable),
            _ => true,
        }
    }
    if !hashable(key) {
        return Err(Error::type_error("unhashable dictionary key"));
    }
    for (index, (candidate, _)) in values.iter().enumerate() {
        if is(candidate, key) || eq(candidate, key)? {
            return Ok(Some(index));
        }
    }
    Ok(None)
}

pub(crate) fn compare(op: &str, left: &Value, right: &Value) -> Result<bool> {
    match op {
        "==" => return eq(left, right),
        "!=" | "<>" => return Ok(!eq(left, right)?),
        "is" => return Ok(is(left, right)),
        "is not" => return Ok(!is(left, right)),
        "in" | "not in" => {
            let member = match &right.0 {
                Kind::Bytes(bytes) if matches!(left.0, Kind::Bytes(_)) => {
                    let needle = left.as_bytes()?;
                    needle.is_empty() || bytes.windows(needle.len()).any(|item| item == needle)
                }
                Kind::Bytes(_) | Kind::Unicode(_) => right.as_text()?.contains(left.as_text()?),
                Kind::Dict(items) => dict_index(&items.borrow(), left)?.is_some(),
                _ => {
                    let mut found = false;
                    for item in right.items()? {
                        if eq(&item, left)? {
                            found = true;
                            break;
                        }
                    }
                    found
                }
            };
            return Ok(member == (op == "in"));
        }
        _ => {}
    }
    let order = if let Some(order) = numeric_compare(left, right) {
        order
    } else if let (Kind::Host(object), Kind::Host(_)) = (&left.0, &right.0)
        && let Some(order) = object.native.compare(right)
    {
        Some(order)
    } else if let (Kind::Bytes(a), Kind::Bytes(b)) = (&left.0, &right.0) {
        Some(a.cmp(b))
    } else if let (Ok(a), Ok(b)) = (left.as_text(), right.as_text()) {
        Some(a.cmp(b))
    } else {
        return Err(Error::unsupported(format!(
            "ordering {} and {}",
            left.type_name(),
            right.type_name()
        )));
    };
    Ok(match op {
        "<" => order == Some(Ordering::Less),
        "<=" => matches!(order, Some(Ordering::Less | Ordering::Equal)),
        ">" => order == Some(Ordering::Greater),
        ">=" => matches!(order, Some(Ordering::Greater | Ordering::Equal)),
        _ => return Err(Error::unsupported(format!("comparison {op}"))),
    })
}

pub(crate) fn unary(op: &str, value: Value) -> Result<Value> {
    Ok(match op {
        "not" => Value::bool(!value.truth()),
        "+" if matches!(value.0, Kind::Float(_)) => value,
        "-" if matches!(value.0, Kind::Float(_)) => Value::float(-value.as_f64()?),
        "+" => Value::int(value.as_int()?),
        "-" => Value::int(-value.as_int()?),
        "~" => Value::int(!value.as_int()?),
        _ => return Err(Error::unsupported(format!("unary operator {op}"))),
    })
}

pub(crate) fn binary(op: &str, left: Value, right: Value, future_division: bool) -> Result<Value> {
    if let Kind::Host(object) = &left.0
        && let Some(result) = object.native.binary(op, &right)
    {
        return result;
    }
    match (op, &left.0, &right.0) {
        ("+", Kind::Bytes(a), Kind::Bytes(b)) => {
            return Ok(Value::bytes([a.as_ref(), b.as_ref()].concat()));
        }
        ("+", Kind::Bytes(_) | Kind::Unicode(_), Kind::Bytes(_) | Kind::Unicode(_)) => {
            return Ok(Value::unicode(format!(
                "{}{}",
                ascii_text(&left)?,
                ascii_text(&right)?
            )));
        }
        ("+", Kind::Tuple(a), Kind::Tuple(b)) => {
            return Ok(Value::tuple([a.as_ref(), b.as_ref()].concat()));
        }
        ("+", Kind::List(a), Kind::List(b)) => {
            return Ok(Value::list(
                [a.borrow().as_slice(), b.borrow().as_slice()].concat(),
            ));
        }
        ("%", Kind::Bytes(_) | Kind::Unicode(_), _) => return format_percent(&left, &right),
        ("*", Kind::Bytes(_) | Kind::Unicode(_) | Kind::List(_) | Kind::Tuple(_), _) => {
            return repeat(&left, &right);
        }
        ("*", _, Kind::Bytes(_) | Kind::Unicode(_) | Kind::List(_) | Kind::Tuple(_)) => {
            return repeat(&right, &left);
        }
        _ => {}
    }
    let use_float = matches!(left.0, Kind::Float(_))
        || matches!(right.0, Kind::Float(_))
        || (op == "/" && future_division)
        || (op == "**" && right.as_int().is_ok_and(|x| x < BigInt::ZERO));
    if use_float {
        let a = left.as_f64()?;
        let b = right.as_f64()?;
        if matches!(op, "/" | "//" | "%") && b == 0.0 {
            return Err(Error::new(
                "ZeroDivisionError",
                "float division or modulo by zero",
            ));
        }
        return Ok(Value::float(match op {
            "+" => a + b,
            "-" => a - b,
            "*" => a * b,
            "/" => a / b,
            "//" | "%" => {
                let mut rem = a % b;
                let mut div = (a - rem) / b;
                if rem != 0.0 && (b < 0.0) != (rem < 0.0) {
                    rem += b;
                    div -= 1.0;
                } else if rem == 0.0 {
                    rem = 0.0_f64.copysign(b);
                }
                if op == "%" {
                    rem
                } else if div == 0.0 {
                    0.0_f64.copysign(a / b)
                } else {
                    div.round()
                }
            }
            "**" if a == 0.0 && b < 0.0 => {
                return Err(Error::new("ZeroDivisionError", "zero to a negative power"));
            }
            "**" if a < 0.0 && b.is_finite() && b.fract() != 0.0 => {
                return Err(Error::new(
                    "ValueError",
                    "negative number to a fractional power",
                ));
            }
            "**" => a.powf(b),
            _ => return Err(Error::type_error(format!("invalid float operator {op}"))),
        }));
    }
    let a = left.as_int()?;
    let b = right.as_int()?;
    if matches!(op, "/" | "//" | "%") && b.is_zero() {
        return Err(Error::new(
            "ZeroDivisionError",
            "integer division or modulo by zero",
        ));
    }
    Ok(Value::int(match op {
        "+" => a + b,
        "-" => a - b,
        "*" => a * b,
        "/" | "//" => a.div_floor(&b),
        "%" => a.mod_floor(&b),
        "&" => a & b,
        "|" => a | b,
        "^" => a ^ b,
        "<<" | ">>" | "**" => {
            if b < BigInt::ZERO {
                return Err(Error::new("ValueError", "negative shift count"));
            }
            let shift = b
                .to_u32()
                .filter(|n| *n <= 1_000_000)
                .ok_or_else(|| Error::unsupported("shift or exponent exceeds interpreter limit"))?;
            match op {
                "<<" => a << shift,
                ">>" => a >> shift,
                _ if a.bits().saturating_mul(u64::from(shift)) <= 8_000_000 => a.pow(shift),
                _ => {
                    return Err(Error::unsupported(
                        "integer power exceeds interpreter limit",
                    ));
                }
            }
        }
        _ => return Err(Error::unsupported(format!("binary operator {op}"))),
    }))
}

fn ascii_text(value: &Value) -> Result<&str> {
    if matches!(&value.0, Kind::Bytes(bytes) if !bytes.is_ascii()) {
        return Err(Error::new("UnicodeDecodeError", "non-ASCII byte string"));
    }
    value.as_text()
}

fn repeat(value: &Value, count: &Value) -> Result<Value> {
    let count = count.as_i64()?.max(0) as usize;
    let size = len(value)?;
    if size == 0 {
        return Ok(value.clone());
    }
    if size.checked_mul(count).is_none_or(|len| len > 16_000_000) {
        return Err(Error::unsupported("repetition exceeds interpreter limit"));
    }
    Ok(match &value.0 {
        Kind::Bytes(bytes) => Value::bytes(bytes.repeat(count)),
        Kind::Unicode(text) => Value::unicode(text.repeat(count)),
        Kind::List(values) => Value::list(
            values
                .borrow()
                .iter()
                .cloned()
                .cycle()
                .take(size * count)
                .collect(),
        ),
        Kind::Tuple(values) => {
            Value::tuple(values.iter().cloned().cycle().take(size * count).collect())
        }
        _ => return Err(Error::type_error("expected repeatable sequence")),
    })
}

pub(crate) fn format_percent(template: &Value, value: &Value) -> Result<Value> {
    let unicode = matches!(template.0, Kind::Unicode(_));
    let source = match &template.0 {
        Kind::Bytes(bytes) => bytes.as_ref(),
        Kind::Unicode(text) => text.as_bytes(),
        _ => return Err(Error::type_error("expected string format")),
    };
    let args = match &value.0 {
        Kind::Tuple(items) => items.as_ref(),
        _ => std::slice::from_ref(value),
    };
    let mut result = Vec::new();
    let mut index = 0;
    let mut arg = 0;
    let mut unicode_result = unicode;
    while index < source.len() {
        if source[index] != b'%' {
            result.push(source[index]);
            index += 1;
            continue;
        }
        index += 1;
        if source.get(index) == Some(&b'%') {
            result.push(b'%');
            index += 1;
            continue;
        }
        let mut flags = Vec::new();
        while let Some(c @ (b'-' | b'+' | b' ' | b'0' | b'#')) = source.get(index) {
            flags.push(*c);
            index += 1;
        }
        let width = format_number(source, &mut index)?;
        let precision = if source.get(index) == Some(&b'.') {
            index += 1;
            Some(format_number(source, &mut index)?)
        } else {
            None
        };
        let code = *source
            .get(index)
            .ok_or_else(|| Error::new("ValueError", "incomplete format"))?;
        index += 1;
        let value = args
            .get(arg)
            .ok_or_else(|| Error::type_error("not enough arguments for format"))?;
        arg += 1;
        let mut part = match code {
            b's' => match &value.0 {
                Kind::Bytes(bytes) => bytes.to_vec(),
                Kind::Unicode(text) => {
                    unicode_result = true;
                    text.as_bytes().to_vec()
                }
                _ => value.string().into_bytes(),
            },
            b'r' => format!("{value:?}").into_bytes(),
            b'd' | b'i' | b'u' => {
                let integer = match value.0 {
                    Kind::Float(x) => BigInt::from_f64(x)
                        .ok_or_else(|| Error::new("ValueError", "invalid integer format value"))?,
                    _ => value.as_int()?,
                };
                integer.to_string().into_bytes()
            }
            b'x' | b'X' | b'o' => {
                let integer = value.as_int()?;
                let digits = integer
                    .magnitude()
                    .to_str_radix(if code == b'o' { 8 } else { 16 });
                let sign = if integer < BigInt::ZERO { "-" } else { "" };
                let prefix = if flags.contains(&b'#') {
                    match code {
                        b'o' if digits != "0" => "0",
                        b'x' => "0x",
                        b'X' => "0X",
                        _ => "",
                    }
                } else {
                    ""
                };
                let digits = if code == b'X' {
                    digits.to_uppercase()
                } else {
                    digits
                };
                format!("{sign}{prefix}{digits}").into_bytes()
            }
            b'f' | b'F' => format!("{:.*}", precision.unwrap_or(6), value.as_f64()?).into_bytes(),
            b'e' | b'E' => {
                let raw = format!("{:.*e}", precision.unwrap_or(6), value.as_f64()?);
                if let Some((mantissa, exponent)) = raw.split_once('e') {
                    let exponent = exponent
                        .parse::<i32>()
                        .map_err(|_| Error::new("ValueError", "invalid float exponent"))?;
                    format!("{mantissa}{}{exponent:+03}", code as char).into_bytes()
                } else {
                    raw.into_bytes()
                }
            }
            b'c' => match &value.0 {
                Kind::Bytes(bytes) if bytes.len() == 1 => bytes.to_vec(),
                Kind::Unicode(text) if text.chars().count() == 1 => {
                    unicode_result = true;
                    text.as_bytes().to_vec()
                }
                _ => {
                    let n = value.as_i64()?;
                    if unicode {
                        char::from_u32(u32::try_from(n).unwrap_or(u32::MAX))
                            .ok_or_else(|| Error::new("OverflowError", "character out of range"))?
                            .to_string()
                            .into_bytes()
                    } else {
                        vec![
                            u8::try_from(n).map_err(|_| {
                                Error::new("OverflowError", "character out of range")
                            })?,
                        ]
                    }
                }
            },
            _ => {
                return Err(Error::unsupported(format!(
                    "percent format %{}",
                    code as char
                )));
            }
        };
        let is_number = matches!(
            code,
            b'd' | b'i' | b'u' | b'x' | b'X' | b'o' | b'f' | b'F' | b'e' | b'E'
        );
        if is_number && !part.starts_with(b"-") {
            if flags.contains(&b'+') {
                part.insert(0, b'+');
            } else if flags.contains(&b' ') {
                part.insert(0, b' ');
            }
        }
        if matches!(code, b's' | b'r') {
            if let Some(precision) = precision {
                let end = if unicode_result {
                    let text = std::str::from_utf8(&part).map_err(|_| {
                        Error::new("UnicodeDecodeError", "invalid formatted Unicode string")
                    })?;
                    text.char_indices()
                        .nth(precision)
                        .map_or(part.len(), |(index, _)| index)
                } else {
                    precision
                };
                part.truncate(end);
            }
        }
        let len = if unicode_result {
            std::str::from_utf8(&part)
                .map_err(|_| Error::new("UnicodeDecodeError", "invalid formatted Unicode string"))?
                .chars()
                .count()
        } else {
            part.len()
        };
        let pad = width.saturating_sub(len);
        if flags.contains(&b'-') {
            result.extend(part);
            result.extend(std::iter::repeat_n(b' ', pad));
        } else if is_number && flags.contains(&b'0') {
            let sign = usize::from(matches!(part.first(), Some(b'+' | b'-' | b' ')));
            let prefix = if part[sign..].starts_with(b"0x") || part[sign..].starts_with(b"0X") {
                2
            } else {
                0
            };
            result.extend_from_slice(&part[..sign + prefix]);
            result.extend(std::iter::repeat_n(b'0', pad));
            result.extend_from_slice(&part[sign + prefix..]);
        } else {
            result.extend(std::iter::repeat_n(b' ', pad));
            result.extend(part);
        }
    }
    if arg != args.len() {
        return Err(Error::type_error(
            "not all arguments converted during string formatting",
        ));
    }
    if unicode_result {
        Ok(Value::unicode(String::from_utf8(result).map_err(|_| {
            Error::new("UnicodeDecodeError", "invalid formatted Unicode string")
        })?))
    } else {
        Ok(Value::bytes(result))
    }
}

fn format_number(source: &[u8], index: &mut usize) -> Result<usize> {
    let mut value = 0;
    while let Some(&digit @ b'0'..=b'9') = source.get(*index) {
        *index += 1;
        value = value * 10 + usize::from(digit - b'0');
        if value > 1_000_000 {
            return Err(Error::unsupported(
                "format width or precision exceeds interpreter limit",
            ));
        }
    }
    Ok(value)
}

pub(crate) fn len(value: &Value) -> Result<usize> {
    match &value.0 {
        Kind::Bytes(bytes) | Kind::Buffer(bytes) => Ok(bytes.len()),
        Kind::Unicode(text) => Ok(text.chars().count()),
        Kind::Tuple(vals) => Ok(vals.len()),
        Kind::List(vals) => Ok(vals.borrow().len()),
        Kind::XRange { len, .. } => Ok(*len),
        Kind::Dict(vals) => Ok(vals.borrow().len()),
        _ => Err(Error::type_error("object has no length")),
    }
}

pub(crate) fn index(key: &Value, len: usize) -> Result<usize> {
    let value = key.as_int()?.to_i128().ok_or_else(index_error)?;
    let normal = if value < 0 {
        value + len as i128
    } else {
        value
    };
    usize::try_from(normal)
        .ok()
        .filter(|i| *i < len)
        .ok_or_else(index_error)
}

fn index_error() -> Error {
    Error::new("IndexError", "sequence index out of range")
}

pub(crate) fn subscript(value: &Value, key: &Value) -> Result<Value> {
    if let Kind::Dict(items) = &value.0 {
        let items = items.borrow();
        let index =
            dict_index(&items, key)?.ok_or_else(|| Error::new("KeyError", format!("{key:?}")))?;
        return Ok(items[index].1.clone());
    }
    let at = index(key, len(value)?)?;
    match &value.0 {
        Kind::Bytes(bytes) | Kind::Buffer(bytes) => Ok(Value::bytes([bytes[at]])),
        Kind::Unicode(text) => Ok(Value::unicode(
            text.chars().nth(at).ok_or_else(index_error)?.to_string(),
        )),
        Kind::Tuple(items) => Ok(items[at].clone()),
        Kind::List(items) => Ok(items.borrow()[at].clone()),
        Kind::XRange { start, step, .. } => Ok(Value::int(
            i128::from(*start) + i128::from(*step) * at as i128,
        )),
        _ => Err(Error::type_error("object is not subscriptable")),
    }
}

pub(crate) fn slice(
    value: &Value,
    start: Option<i64>,
    end: Option<i64>,
    step: Option<i64>,
) -> Result<Value> {
    let size = len(value)? as i128;
    let step = i128::from(step.unwrap_or(1));
    if step == 0 {
        return Err(Error::new("ValueError", "slice step cannot be zero"));
    }
    let (min, max) = if step > 0 { (0, size) } else { (-1, size - 1) };
    let convert = |index: i64| {
        let index = i128::from(index);
        (if index < 0 { index + size } else { index }).clamp(min, max)
    };
    let mut at = start.map_or(if step > 0 { 0 } else { max }, convert);
    let end = end.map_or(if step > 0 { size } else { -1 }, convert);
    let indices = std::iter::from_fn(move || {
        if (step > 0 && at >= end) || (step < 0 && at <= end) {
            return None;
        }
        let index = at as usize;
        at += step;
        Some(index)
    });
    Ok(match &value.0 {
        Kind::Bytes(bytes) | Kind::Buffer(bytes) => {
            Value::bytes(indices.map(|i| bytes[i]).collect::<Vec<_>>())
        }
        Kind::Unicode(text) => {
            let chars: Vec<_> = text.chars().collect();
            Value::unicode(indices.map(|i| chars[i]).collect::<String>())
        }
        Kind::List(items) => {
            let items = items.borrow();
            Value::list(indices.map(|i| items[i].clone()).collect())
        }
        Kind::Tuple(items) => Value::tuple(indices.map(|i| items[i].clone()).collect()),
        _ => return Err(Error::type_error("object does not support slicing")),
    })
}
