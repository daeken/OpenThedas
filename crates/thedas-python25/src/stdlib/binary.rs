use std::cell::Cell;

use super::{module, no_keywords};
use crate::{Error, NativeObject, Result, Runtime, Value, value::Kind};

pub(super) fn install(runtime: &mut Runtime) {
    module(
        runtime,
        "struct",
        [
            ("error", Value::exception_type("struct.error")),
            (
                "calcsize",
                Value::function(|_, args| {
                    let [format] = args.take()?;
                    Ok(Value::int(Format::parse(format.as_text()?)?.size))
                }),
            ),
            (
                "unpack_from",
                Value::function(|_, args| {
                    let args = no_keywords(args)?;
                    match args.as_slice() {
                        [format, buffer] => unpack(format, buffer, 0, false),
                        [format, buffer, offset] => unpack(format, buffer, offset.as_i64()?, false),
                        _ => Err(Error::type_error(
                            "unpack_from expects two or three arguments",
                        )),
                    }
                }),
            ),
            (
                "unpack",
                Value::function(|_, args| {
                    let [format, buffer] = args.take()?;
                    unpack(&format, &buffer, 0, true)
                }),
            ),
        ],
    );
    module(
        runtime,
        "ctypes",
        [(
            "c_long",
            Value::function(|_, args| {
                let [value] = args.take()?;
                Ok(Value::object(CLong(Cell::new(as_long(&value)?))))
            }),
        )],
    );
}

struct CLong(Cell<i32>);

fn as_long(value: &Value) -> Result<i32> {
    let bytes = value.as_int()?.to_signed_bytes_le();
    let mut word = [if value.as_int()? < num_bigint::BigInt::ZERO {
        255
    } else {
        0
    }; 4];
    let count = bytes.len().min(4);
    word[..count].copy_from_slice(&bytes[..count]);
    Ok(i32::from_le_bytes(word))
}

impl NativeObject for CLong {
    fn get(&self, name: &str) -> Result<Value> {
        if name == "value" {
            Ok(Value::int(self.0.get()))
        } else {
            Err(Error::new("AttributeError", name))
        }
    }
    fn set(&self, name: &str, value: Value) -> Result<()> {
        if name != "value" {
            return Err(Error::new("AttributeError", name));
        }
        self.0.set(as_long(&value)?);
        Ok(())
    }
    fn buffer(&self) -> Option<Vec<u8>> {
        Some(self.0.get().to_le_bytes().to_vec())
    }
}

struct Field {
    code: u8,
    offset: usize,
    width: usize,
    count: usize,
}
struct Format {
    big_endian: bool,
    size: usize,
    fields: Vec<Field>,
}

fn struct_error(message: impl Into<String>) -> Error {
    Error::new("struct.error", message)
}

impl Format {
    fn parse(format: &str) -> Result<Self> {
        let (prefix, source) = match format.as_bytes() {
            [prefix @ (b'@' | b'=' | b'<' | b'>' | b'!'), rest @ ..] => (*prefix, rest),
            source => (b'@', source),
        };
        let mut fields = Vec::new();
        let mut size = 0_usize;
        let mut repeat = None::<usize>;
        for &code in source {
            if code.is_ascii_whitespace() && repeat.is_none() {
                continue;
            }
            if code.is_ascii_digit() {
                repeat = Some(
                    repeat
                        .unwrap_or(0)
                        .checked_mul(10)
                        .and_then(|n| n.checked_add((code - b'0') as usize))
                        .ok_or_else(|| struct_error("repeat count overflow"))?,
                );
                continue;
            }
            let mut count = repeat.take().unwrap_or(1);
            let width = match code {
                b'x' | b'c' | b'b' | b'B' => 1,
                b'h' | b'H' => 2,
                b'i' | b'I' | b'l' | b'L' | b'f' => 4,
                b'q' | b'Q' | b'd' => 8,
                b'P' if prefix == b'@' => 4,
                b's' | b'p' => std::mem::replace(&mut count, 1),
                _ => {
                    return Err(struct_error(format!(
                        "bad format character {}",
                        code as char
                    )));
                }
            };
            if prefix == b'@' && width > 1 && !matches!(code, b's' | b'p') {
                size = size
                    .checked_next_multiple_of(width)
                    .ok_or_else(|| struct_error("size overflow"))?;
            }
            fields.push(Field {
                code,
                offset: size,
                width,
                count,
            });
            size = width
                .checked_mul(count)
                .and_then(|n| size.checked_add(n))
                .ok_or_else(|| struct_error("size overflow"))?;
        }
        if repeat.is_some() {
            return Err(struct_error("repeat count without format specifier"));
        }
        Ok(Self {
            big_endian: matches!(prefix, b'>' | b'!'),
            size,
            fields,
        })
    }
}

fn unpack(format: &Value, buffer: &Value, offset: i64, exact: bool) -> Result<Value> {
    let format = Format::parse(format.as_text()?)?;
    let owned;
    let bytes = match &buffer.0 {
        Kind::Bytes(data) | Kind::Buffer(data) => data.as_ref(),
        Kind::Host(object) => {
            owned = object
                .native
                .buffer()
                .ok_or_else(|| Error::type_error("object does not supply a readable buffer"))?;
            owned.as_slice()
        }
        _ => return Err(Error::type_error("expected a readable buffer")),
    };
    let offset = if offset < 0 {
        bytes.len() as i128 + i128::from(offset)
    } else {
        i128::from(offset)
    };
    let start = usize::try_from(offset).map_err(|_| struct_error("offset outside buffer"))?;
    let end = start
        .checked_add(format.size)
        .ok_or_else(|| struct_error("size overflow"))?;
    if exact && end != bytes.len() {
        return Err(struct_error("buffer has incorrect size"));
    }
    let bytes = bytes
        .get(start..end)
        .ok_or_else(|| struct_error("buffer is too short"))?;
    let mut output = Vec::new();
    for field in format.fields {
        if field.code == b'x' {
            continue;
        }
        for i in 0..field.count {
            let start = field.offset + i * field.width;
            let part = &bytes[start..start + field.width];
            if matches!(field.code, b'c' | b's' | b'p') {
                let bytes = if field.code == b'p' {
                    match part.split_first() {
                        Some((&length, rest)) => &rest[..usize::from(length).min(rest.len())],
                        None => return Err(struct_error("zero-sized Pascal string")),
                    }
                } else {
                    part
                };
                output.push(Value::bytes(bytes));
                continue;
            }
            let mut word = [0; 8];
            word[..part.len()].copy_from_slice(part);
            if format.big_endian {
                word[..part.len()].reverse();
            }
            let bits = u64::from_le_bytes(word);
            output.push(match field.code {
                b'b' => Value::int(bits as i8),
                b'h' => Value::int(bits as i16),
                b'i' | b'l' => Value::int(bits as i32),
                b'q' => Value::int(bits as i64),
                b'f' => Value::float(f64::from(f32::from_bits(bits as u32))),
                b'd' => Value::float(f64::from_bits(bits)),
                _ => Value::int(bits),
            });
        }
    }
    Ok(Value::tuple(output))
}
