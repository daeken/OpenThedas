mod binary;
mod datetime;
mod files;
mod xml;

use std::{cell::RefCell, rc::Rc};

use num_bigint::BigInt;
use num_traits::FromPrimitive;

use crate::{Args, Error, Result, Runtime, Value, ops, value::Kind};

pub(crate) fn install(runtime: &mut Runtime, argv: Vec<Value>) {
    for name in [
        "BaseException",
        "Exception",
        "StandardError",
        "ArithmeticError",
        "AssertionError",
        "AttributeError",
        "EnvironmentError",
        "IOError",
        "ImportError",
        "IndexError",
        "KeyError",
        "KeyboardInterrupt",
        "MemoryError",
        "LookupError",
        "NameError",
        "OSError",
        "OverflowError",
        "RuntimeError",
        "StopIteration",
        "SyntaxError",
        "SystemExit",
        "TypeError",
        "UnboundLocalError",
        "UnicodeError",
        "ValueError",
        "WindowsError",
        "ZeroDivisionError",
    ] {
        runtime.register_builtin(name, Value::exception_type(name));
    }
    for (name, builtin) in [
        (
            "len",
            Value::function(|_, args| {
                let [value] = args.take()?;
                Ok(Value::int(ops::len(&value)?))
            }),
        ),
        (
            "ord",
            Value::function(|_, args| {
                let [value] = args.take()?;
                if let Kind::Bytes(bytes) = value.0 {
                    if let [byte] = bytes.as_ref() {
                        return Ok(Value::int(*byte));
                    }
                } else if let Ok(text) = value.as_text() {
                    let mut chars = text.chars();
                    if let (Some(ch), None) = (chars.next(), chars.next()) {
                        return Ok(Value::int(ch as u32));
                    }
                }
                Err(Error::type_error("ord requires one character"))
            }),
        ),
        (
            "chr",
            Value::function(|_, args| {
                let [value] = args.take()?;
                let byte = u8::try_from(value.as_i64()?)
                    .map_err(|_| Error::new("ValueError", "chr argument out of range"))?;
                Ok(Value::bytes([byte]))
            }),
        ),
        (
            "range",
            Value::function(|_, args| {
                let range = make_range(args)?;
                Ok(Value::list(range.items()?))
            }),
        ),
        ("xrange", Value::function(|_, args| make_range(args))),
        ("map", Value::function(map)),
        ("filter", Value::function(filter)),
        (
            "zip",
            Value::function(|_, args| {
                let iterables = no_keywords(args)?
                    .into_iter()
                    .map(|item| item.items())
                    .collect::<Result<Vec<_>>>()?;
                let len = iterables.iter().map(Vec::len).min().unwrap_or(0);
                Ok(Value::list(
                    (0..len)
                        .map(|index| {
                            Value::tuple(
                                iterables.iter().map(|items| items[index].clone()).collect(),
                            )
                        })
                        .collect(),
                ))
            }),
        ),
        (
            "enumerate",
            Value::function(|_, args| {
                let [value] = args.take()?;
                Ok(Value::list(
                    value
                        .items()?
                        .into_iter()
                        .enumerate()
                        .map(|(index, item)| Value::tuple(vec![Value::int(index), item]))
                        .collect(),
                ))
            }),
        ),
        ("int", Value::function(|_, args| to_int(args))),
        ("long", Value::function(|_, args| to_int(args))),
        (
            "float",
            Value::function(|_, args| {
                let Some(value) = optional(args)? else {
                    return Ok(Value::float(0.0));
                };
                match value.0 {
                    Kind::Bytes(_) | Kind::Unicode(_) => value
                        .as_text()?
                        .trim()
                        .parse::<f64>()
                        .map(Value::float)
                        .map_err(|_| Error::new("ValueError", "invalid floating point number")),
                    _ => Ok(Value::float(value.as_f64()?)),
                }
            }),
        ),
        (
            "bool",
            Value::function(|_, args| {
                Ok(Value::bool(
                    optional(args)?.is_some_and(|value| value.truth()),
                ))
            }),
        ),
        (
            "str",
            Value::function(|_, args| {
                let Some(value) = optional(args)? else {
                    return Ok(Value::bytes([]));
                };
                match &value.0 {
                    Kind::Bytes(_) => Ok(value),
                    Kind::Unicode(text) if !text.is_ascii() => {
                        Err(Error::new("UnicodeEncodeError", "non-ASCII Unicode string"))
                    }
                    _ => Ok(Value::bytes(value.string())),
                }
            }),
        ),
        (
            "unicode",
            Value::function(|_, args| {
                let Some(value) = optional(args)? else {
                    return Ok(Value::unicode(""));
                };
                match &value.0 {
                    Kind::Unicode(_) => Ok(value),
                    Kind::Bytes(bytes) if !bytes.is_ascii() => {
                        Err(Error::new("UnicodeDecodeError", "non-ASCII byte string"))
                    }
                    _ => Ok(Value::unicode(value.string())),
                }
            }),
        ),
        (
            "repr",
            Value::function(|_, args| {
                let [value] = args.take()?;
                Ok(Value::bytes(format!("{value:?}")))
            }),
        ),
        (
            "list",
            Value::function(|_, args| Ok(Value::list(optional_items(args)?))),
        ),
        (
            "tuple",
            Value::function(|_, args| Ok(Value::tuple(optional_items(args)?))),
        ),
        (
            "dict",
            Value::function(|_, args| {
                let mut values = Vec::new();
                for item in optional_items(Args {
                    positional: args.positional,
                    ..Args::default()
                })? {
                    let [key, value] = item.items()?.try_into().map_err(|_| {
                        Error::new("ValueError", "dictionary item must have length 2")
                    })?;
                    update(&mut values, key, value)?;
                }
                for (key, value) in args.named {
                    update(&mut values, Value::bytes(key), value)?;
                }
                Ok(Value(Kind::Dict(Rc::new(RefCell::new(values)))))
            }),
        ),
        ("min", Value::function(|_, args| minimum(args, "<"))),
        ("max", Value::function(|_, args| minimum(args, ">"))),
        (
            "sum",
            Value::function(|_, args| {
                let args = no_keywords(args)?;
                let (items, mut result) = match args.as_slice() {
                    [items] => (items, Value::int(0)),
                    [items, start] => (items, start.clone()),
                    _ => return Err(Error::type_error("sum expects one or two arguments")),
                };
                for item in items.items()? {
                    result = ops::binary("+", result, item, false)?;
                }
                Ok(result)
            }),
        ),
        (
            "abs",
            Value::function(|_, args| {
                let [value] = args.take()?;
                if let Kind::Float(real) = value.0 {
                    return Ok(Value::float(real.abs()));
                }
                Ok(Value::int(num_traits::Signed::abs(&value.as_int()?)))
            }),
        ),
        (
            "getattr",
            Value::function(|runtime, args| {
                let args = no_keywords(args)?;
                match args.as_slice() {
                    [object, name] => runtime.attribute(object, name.as_text()?),
                    [object, name, default] => match runtime.attribute(object, name.as_text()?) {
                        Err(err) if err.class == "AttributeError" => Ok(default.clone()),
                        result => result,
                    },
                    _ => Err(Error::type_error("getattr expects two or three arguments")),
                }
            }),
        ),
        (
            "hasattr",
            Value::function(|runtime, args| {
                let [object, name] = args.take()?;
                match runtime.attribute(&object, name.as_text()?) {
                    Ok(_) => Ok(Value::bool(true)),
                    Err(err) if !err.fatal => Ok(Value::bool(false)),
                    Err(err) => Err(err),
                }
            }),
        ),
        ("open", Value::function(files::open)),
        ("file", Value::function(files::open)),
    ] {
        runtime.register_builtin(name, builtin);
    }

    let stdout = Value::module([
        (
            "write",
            Value::function(|runtime, args| {
                let [value] = args.take()?;
                let bytes = match &value.0 {
                    Kind::Bytes(bytes) => bytes.as_ref(),
                    _ => value.as_text()?.as_bytes(),
                };
                runtime.write_stdout(bytes)?;
                Ok(Value::NONE)
            }),
        ),
        (
            "flush",
            Value::function(|runtime, args| {
                let [] = args.take()?;
                runtime.flush_stdout()?;
                Ok(Value::NONE)
            }),
        ),
    ]);
    module(
        runtime,
        "sys",
        [
            ("argv", Value::list(argv)),
            ("stdout", stdout),
            (
                "version_info",
                Value::tuple(vec![
                    Value::int(2),
                    Value::int(5),
                    Value::int(4),
                    Value::bytes("final"),
                    Value::int(0),
                ]),
            ),
            ("platform", Value::bytes("win32")),
            (
                "exit",
                Value::function(|_, args| {
                    let status = optional(args)?.unwrap_or(Value::NONE);
                    let status = if matches!(status.0, Kind::None) {
                        "0".to_owned()
                    } else {
                        status.string()
                    };
                    Err(Error::new("SystemExit", status))
                }),
            ),
        ],
    );
    module(
        runtime,
        "getopt",
        [
            ("getopt", Value::function(|_, args| getopt(args))),
            ("GetoptError", Value::exception_type("GetoptError")),
        ],
    );
    let unary_float = |function: fn(f64) -> f64| {
        Value::function(move |_, args| {
            let [value] = args.take()?;
            let input = value.as_f64()?;
            let output = function(input);
            if output.is_nan() && !input.is_nan() {
                return Err(Error::new("ValueError", "math domain error"));
            }
            Ok(Value::float(output))
        })
    };
    module(
        runtime,
        "math",
        [
            ("pi", Value::float(std::f64::consts::PI)),
            ("e", Value::float(std::f64::consts::E)),
            ("sqrt", unary_float(f64::sqrt)),
            ("ceil", unary_float(f64::ceil)),
            ("floor", unary_float(f64::floor)),
            ("sin", unary_float(f64::sin)),
            ("cos", unary_float(f64::cos)),
            ("tan", unary_float(f64::tan)),
            ("fabs", unary_float(f64::abs)),
            ("exp", unary_float(f64::exp)),
            ("log", unary_float(f64::ln)),
        ],
    );
    binary::install(runtime);
    datetime::install(runtime);
    files::install(runtime);
    xml::install(runtime);
}

fn module<const N: usize>(runtime: &mut Runtime, name: &str, attrs: [(&str, Value); N]) {
    runtime
        .register_module(name, Value::module(attrs))
        .expect("valid built-in module registration");
}

fn make_range(args: Args) -> Result<Value> {
    let args = no_keywords(args)?;
    let (start, end, step) = match args.as_slice() {
        [end] => (0, end.as_i64()?, 1),
        [start, end] => (start.as_i64()?, end.as_i64()?, 1),
        [start, end, step] => (start.as_i64()?, end.as_i64()?, step.as_i64()?),
        _ => return Err(Error::type_error("range expects one to three arguments")),
    };
    if step == 0 {
        return Err(Error::new("ValueError", "range step is zero"));
    }
    let distance = i128::from(end) - i128::from(start);
    let magnitude = i128::from(step).abs();
    let len = if distance.signum() == i128::from(step).signum() {
        (distance.abs() + magnitude - 1) / magnitude
    } else {
        0
    };
    let len = usize::try_from(len).map_err(|_| Error::new("OverflowError", "range too large"))?;
    Ok(Value(Kind::XRange { start, step, len }))
}

fn map(runtime: &mut Runtime, args: Args) -> Result<Value> {
    let args = no_keywords(args)?;
    let (function, lists) = args
        .split_first()
        .ok_or_else(|| Error::type_error("map needs arguments"))?;
    if lists.is_empty() {
        return Err(Error::type_error("map requires an iterable"));
    }
    let items = lists.iter().map(Value::items).collect::<Result<Vec<_>>>()?;
    let count = items.iter().map(Vec::len).max().unwrap_or(0);
    let mut output = Vec::with_capacity(count);
    for index in 0..count {
        let arguments = items
            .iter()
            .map(|items| items.get(index).cloned().unwrap_or(Value::NONE))
            .collect::<Vec<_>>();
        let item = if matches!(function.0, Kind::None) {
            if let [only] = arguments.as_slice() {
                only.clone()
            } else {
                Value::tuple(arguments)
            }
        } else {
            runtime.call(
                function,
                Args {
                    positional: arguments,
                    ..Args::default()
                },
            )?
        };
        output.push(item);
    }
    Ok(Value::list(output))
}

fn filter(runtime: &mut Runtime, args: Args) -> Result<Value> {
    let [predicate, iterable] = args.take()?;
    let mut items = Vec::new();
    for value in iterable.items()? {
        let include = if matches!(predicate.0, Kind::None) {
            value.truth()
        } else {
            runtime
                .call(
                    &predicate,
                    Args {
                        positional: vec![value.clone()],
                        ..Args::default()
                    },
                )?
                .truth()
        };
        if include {
            items.push(value);
        }
    }
    match &iterable.0 {
        Kind::Tuple(_) => Ok(Value::tuple(items)),
        Kind::Bytes(_) => Ok(Value::bytes(
            items
                .iter()
                .map(|item| item.as_bytes().map(|bytes| bytes[0]))
                .collect::<Result<Vec<_>>>()?,
        )),
        Kind::Unicode(_) => Ok(Value::unicode(
            items
                .iter()
                .map(Value::as_text)
                .collect::<Result<Vec<_>>>()?
                .concat(),
        )),
        _ => Ok(Value::list(items)),
    }
}

fn to_int(args: Args) -> Result<Value> {
    let args = no_keywords(args)?;
    match args.as_slice() {
        [] => Ok(Value::int(0)),
        [value] => match value.0 {
            Kind::Float(real) => BigInt::from_f64(real)
                .map(Value::int)
                .ok_or_else(|| Error::new("ValueError", "invalid integer")),
            Kind::Bytes(_) | Kind::Unicode(_) => parse_int(value.as_text()?, 10),
            _ => Ok(Value::int(value.as_int()?)),
        },
        [value, base] => parse_int(value.as_text()?, base.as_i64()?),
        _ => Err(Error::type_error("int expects up to two arguments")),
    }
}

fn parse_int(text: &str, base: i64) -> Result<Value> {
    if base != 0 && !(2..=36).contains(&base) {
        return Err(Error::new("ValueError", "invalid integer base"));
    }
    let text = text.trim();
    let (sign, text) = if let Some(text) = text.strip_prefix('-') {
        (-1, text)
    } else {
        (1, text.strip_prefix('+').unwrap_or(text))
    };
    let lower = text.to_ascii_lowercase();
    let mut radix = base as u32;
    let mut digits = lower.as_str();
    for (prefix, possible) in [("0x", 16), ("0b", 2), ("0o", 8)] {
        if (radix == 0 || radix == possible) && digits.starts_with(prefix) {
            radix = possible;
            digits = &digits[2..];
            break;
        }
    }
    if radix == 0 {
        radix = if digits.starts_with('0') { 8 } else { 10 };
    }
    if !digits.is_empty() && digits.chars().all(|ch| ch.is_digit(radix)) {
        if let Some(value) = BigInt::parse_bytes(digits.as_bytes(), radix) {
            return Ok(Value::int(sign * value));
        }
    }
    Err(Error::new("ValueError", "invalid integer literal"))
}

fn minimum(args: Args, op: &str) -> Result<Value> {
    let args = no_keywords(args)?;
    let items = if let [iterable] = args.as_slice() {
        iterable.items()?
    } else {
        args
    };
    let mut items = items.into_iter();
    let mut best = items
        .next()
        .ok_or_else(|| Error::new("ValueError", "empty sequence"))?;
    for item in items {
        if ops::compare(op, &item, &best)? {
            best = item;
        }
    }
    Ok(best)
}

pub(super) fn no_keywords(args: Args) -> Result<Vec<Value>> {
    if !args.named.is_empty() {
        return Err(Error::type_error("unexpected keyword arguments"));
    }
    Ok(args.positional)
}

pub(super) fn optional(args: Args) -> Result<Option<Value>> {
    let mut args = no_keywords(args)?;
    if args.len() > 1 {
        return Err(Error::type_error("expected at most one argument"));
    }
    Ok(args.pop())
}

fn optional_items(args: Args) -> Result<Vec<Value>> {
    optional(args)?.map_or_else(|| Ok(Vec::new()), |value| value.items())
}

fn update(items: &mut Vec<(Value, Value)>, key: Value, value: Value) -> Result<()> {
    if let Some(index) = ops::dict_index(items, &key)? {
        items[index].1 = value;
    } else {
        items.push((key, value));
    }
    Ok(())
}

pub(crate) fn method(value: &Value, name: &str) -> Result<Value> {
    match (&value.0, name) {
        (Kind::List(items), "append") => {
            let items = items.clone();
            Ok(Value::function(move |_, args| {
                let [value] = args.take()?;
                items.borrow_mut().push(value);
                Ok(Value::NONE)
            }))
        }
        (Kind::List(items), "extend") => {
            let items = items.clone();
            Ok(Value::function(move |_, args| {
                let [value] = args.take()?;
                let addition = value.items()?;
                items.borrow_mut().extend(addition);
                Ok(Value::NONE)
            }))
        }
        (Kind::List(items), "reverse") => {
            let items = items.clone();
            Ok(Value::function(move |_, args| {
                let [] = args.take()?;
                items.borrow_mut().reverse();
                Ok(Value::NONE)
            }))
        }
        (
            Kind::Dict(items),
            "keys" | "values" | "items" | "iterkeys" | "itervalues" | "iteritems",
        ) => {
            let items = items.clone();
            let name = name.trim_start_matches("iter").to_owned();
            Ok(Value::function(move |_, args| {
                let [] = args.take()?;
                Ok(Value::list(
                    items
                        .borrow()
                        .iter()
                        .map(|(key, value)| match name.as_str() {
                            "keys" => key.clone(),
                            "values" => value.clone(),
                            _ => Value::tuple(vec![key.clone(), value.clone()]),
                        })
                        .collect(),
                ))
            }))
        }
        (Kind::Dict(items), "get" | "has_key") => {
            let items = items.clone();
            let has_key = name == "has_key";
            Ok(Value::function(move |_, args| {
                let args = no_keywords(args)?;
                let (key, default) = match args.as_slice() {
                    [key] => (key, Value::NONE),
                    [key, default] if !has_key => (key, default.clone()),
                    _ => return Err(Error::type_error("invalid dictionary method arguments")),
                };
                let items = items.borrow();
                let found = ops::dict_index(&items, key)?;
                Ok(if has_key {
                    Value::bool(found.is_some())
                } else {
                    found.map_or(default, |index| items[index].1.clone())
                })
            }))
        }
        (Kind::Bytes(_) | Kind::Unicode(_), _) => string_method(value, name),
        _ => Err(Error::new("AttributeError", name)),
    }
}

fn string_method(value: &Value, name: &str) -> Result<Value> {
    let name = name.to_owned();
    if !matches!(
        name.as_str(),
        "lower"
            | "upper"
            | "strip"
            | "lstrip"
            | "rstrip"
            | "startswith"
            | "endswith"
            | "split"
            | "join"
            | "replace"
    ) {
        return Err(Error::new("AttributeError", name));
    }
    let value = value.clone();
    Ok(Value::function(move |_, args| {
        let unicode = matches!(value.0, Kind::Unicode(_));
        let convert = |text: &str| {
            if unicode {
                Value::unicode(text)
            } else {
                Value::bytes(text)
            }
        };
        if let Kind::Bytes(bytes) = &value.0 {
            match name.as_str() {
                "lower" => {
                    let [] = args.take()?;
                    return Ok(Value::bytes(bytes.to_ascii_lowercase()));
                }
                "upper" => {
                    let [] = args.take()?;
                    return Ok(Value::bytes(bytes.to_ascii_uppercase()));
                }
                _ => {}
            }
        }
        let source = value.as_text()?;
        match name.as_str() {
            "lower" => {
                let [] = args.take()?;
                Ok(convert(&source.to_lowercase()))
            }
            "upper" => {
                let [] = args.take()?;
                Ok(convert(&source.to_uppercase()))
            }
            "strip" | "lstrip" | "rstrip" => {
                let chars = optional(args)?;
                let chars = chars.as_ref().map(Value::as_text).transpose()?;
                let trim = |ch: char| {
                    chars.map_or_else(
                        || {
                            if unicode {
                                ch.is_whitespace()
                            } else {
                                ch.is_ascii_whitespace()
                            }
                        },
                        |chars| chars.contains(ch),
                    )
                };
                Ok(convert(match name.as_str() {
                    "lstrip" => source.trim_start_matches(trim),
                    "rstrip" => source.trim_end_matches(trim),
                    _ => source.trim_matches(trim),
                }))
            }
            "startswith" | "endswith" => {
                let [affix] = args.take()?;
                let affix = affix.as_text()?;
                Ok(Value::bool(if name == "startswith" {
                    source.starts_with(affix)
                } else {
                    source.ends_with(affix)
                }))
            }
            "join" => {
                let [values] = args.take()?;
                let items = values.items()?;
                let text = items
                    .iter()
                    .map(Value::as_text)
                    .collect::<Result<Vec<_>>>()?
                    .join(source);
                Ok(
                    if items.iter().any(|item| matches!(item.0, Kind::Unicode(_))) {
                        Value::unicode(text)
                    } else {
                        convert(&text)
                    },
                )
            }
            "replace" => {
                let [old, new] = args.take()?;
                Ok(convert(&source.replace(old.as_text()?, new.as_text()?)))
            }
            "split" => {
                let separator = optional(args)?;
                let parts = if let Some(separator) =
                    separator.filter(|value| !matches!(value.0, Kind::None))
                {
                    let separator = separator.as_text()?;
                    if separator.is_empty() {
                        return Err(Error::new("ValueError", "empty separator"));
                    }
                    source.split(separator).map(convert).collect()
                } else if unicode {
                    source.split_whitespace().map(convert).collect()
                } else {
                    source.split_ascii_whitespace().map(convert).collect()
                };
                Ok(Value::list(parts))
            }
            _ => Err(Error::new("AttributeError", name.clone())),
        }
    }))
}

fn getopt(args: Args) -> Result<Value> {
    let args = no_keywords(args)?;
    let (argv, short, long) = match args.as_slice() {
        [argv, short] => (argv, short.as_text()?, Vec::new()),
        [argv, short, long] => {
            let long = if matches!(long.0, Kind::Bytes(_) | Kind::Unicode(_)) {
                vec![long.clone()]
            } else {
                long.items()?
            };
            (argv, short.as_text()?, long)
        }
        _ => return Err(Error::type_error("getopt expects two or three arguments")),
    };
    let long = long
        .iter()
        .map(Value::as_text)
        .collect::<Result<Vec<_>>>()?;
    let argv = argv.items()?;
    let mut input = argv.iter().peekable();
    let mut output = Vec::new();
    let error = |text| Error::new("GetoptError", text);
    while let Some(next) = input.peek() {
        let arg = next.as_text()?;
        if arg == "-" || !arg.starts_with('-') {
            break;
        }
        input.next();
        if arg == "--" {
            break;
        }
        if let Some(option) = arg.strip_prefix("--") {
            let (name, explicit) = option
                .split_once('=')
                .map_or((option, None), |(a, b)| (a, Some(b)));
            let exact = long
                .iter()
                .copied()
                .find(|opt| opt.trim_end_matches('=') == name);
            let mut prefixes = long.iter().copied().filter(|opt| opt.starts_with(name));
            let option = if let Some(exact) = exact {
                exact
            } else {
                let eligible = prefixes
                    .next()
                    .ok_or_else(|| error(format!("option --{name} not recognized")))?;
                if prefixes.next().is_some() {
                    return Err(error(format!("option --{name} not a unique prefix")));
                }
                eligible
            };
            let (option, argument) = if let Some(required) = option.strip_suffix('=') {
                let argument = match explicit {
                    Some(text) => text,
                    None => input
                        .next()
                        .ok_or_else(|| error(format!("option --{required} requires argument")))?
                        .as_text()?,
                };
                (required, argument)
            } else if explicit.is_some() {
                return Err(error(format!(
                    "option --{option} must not have an argument"
                )));
            } else {
                (option, "")
            };
            output.push(Value::tuple(vec![
                Value::bytes(format!("--{option}")),
                Value::bytes(argument),
            ]));
        } else {
            let mut chars = arg[1..].chars();
            while let Some(ch) = chars.next() {
                let text = format!("-{ch}");
                let option = short
                    .find(ch)
                    .filter(|_| ch != ':')
                    .ok_or_else(|| error(format!("option {text} not recognized")))?;
                let required = short[option + ch.len_utf8()..].starts_with(':');
                let argument = if !required {
                    ""
                } else if chars.as_str().is_empty() {
                    input
                        .next()
                        .ok_or_else(|| error(format!("option {text} requires argument")))?
                        .as_text()?
                } else {
                    chars.as_str()
                };
                output.push(Value::tuple(vec![
                    Value::bytes(text),
                    Value::bytes(argument),
                ]));
                if required {
                    break;
                }
            }
        }
    }
    Ok(Value::tuple(vec![
        Value::list(output),
        Value::list(input.cloned().collect()),
    ]))
}
