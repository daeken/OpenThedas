use std::cmp::Ordering;

use num_bigint::BigInt;
use num_traits::{FromPrimitive, ToPrimitive};

use crate::{Args, Error, NativeObject, Result, Runtime, Value, value::Kind};

const SECOND: i64 = 1_000_000;
const DAY: i64 = 86_400 * SECOND;

pub(super) fn install(runtime: &mut Runtime) {
    let now = Value::function(|_, args| {
        let [] = args.take()?;
        let local = chrono::Local::now().naive_local();
        Ok(Value::object(DateTime(local.and_utc().timestamp_micros())))
    });
    super::module(
        runtime,
        "datetime",
        [
            ("datetime", Value::module([("now", now)])),
            ("timedelta", Value::function(|_, args| timedelta(args))),
        ],
    );
}

struct DateTime(i64);

impl NativeObject for DateTime {
    fn binary(&self, op: &str, other: &Value) -> Option<Result<Value>> {
        let other = other.downcast_object::<Self>()?;
        (op == "-").then(|| TimeDelta::value(i128::from(self.0) - i128::from(other.0)))
    }

    fn compare(&self, other: &Value) -> Option<Ordering> {
        Some(self.0.cmp(&other.downcast_object::<Self>()?.0))
    }
}

struct TimeDelta(i128);

impl TimeDelta {
    fn value(microseconds: i128) -> Result<Value> {
        const MAX_DAYS: i128 = 999_999_999;
        let days = microseconds.div_euclid(i128::from(DAY));
        if !(-MAX_DAYS..=MAX_DAYS).contains(&days) {
            return Err(Error::new("OverflowError", "timedelta days out of range"));
        }
        Ok(Value::object(Self(microseconds)))
    }
}

impl NativeObject for TimeDelta {
    fn get(&self, name: &str) -> Result<Value> {
        let second = i128::from(SECOND);
        let day = i128::from(DAY);
        let number = match name {
            "days" => self.0.div_euclid(day),
            "seconds" => self.0.rem_euclid(day) / second,
            "microseconds" => self.0.rem_euclid(second),
            _ => return Err(Error::new("AttributeError", name)),
        };
        Ok(Value::int(number))
    }

    fn compare(&self, other: &Value) -> Option<Ordering> {
        Some(self.0.cmp(&other.downcast_object::<Self>()?.0))
    }

    fn truth(&self) -> bool {
        self.0 != 0
    }
}

fn timedelta(mut args: Args) -> Result<Value> {
    const UNITS: [(&str, i64); 7] = [
        ("days", DAY),
        ("seconds", SECOND),
        ("microseconds", 1),
        ("milliseconds", 1000),
        ("minutes", 60 * SECOND),
        ("hours", 3600 * SECOND),
        ("weeks", 7 * DAY),
    ];
    if args.positional.len() > UNITS.len() {
        return Err(Error::type_error(
            "timedelta accepts at most seven arguments",
        ));
    }
    let count = args.positional.len();
    let mut values: [Option<Value>; UNITS.len()] = std::array::from_fn(|_| None);
    for (index, (name, _)) in UNITS.iter().enumerate() {
        let named = args.named.remove(*name);
        if index < count && named.is_some() {
            return Err(Error::type_error(format!("multiple values for {name}")));
        }
        values[index] = args.positional.get(index).cloned().or(named);
    }
    if let Some(name) = args.named.keys().next() {
        return Err(Error::type_error(format!(
            "unexpected timedelta argument {name}"
        )));
    }

    let mut whole = BigInt::ZERO;
    let mut remainder = 0.0;
    // CPython 2.5 adds fractional microseconds from the smallest unit first.
    for index in [2, 3, 1, 4, 5, 0, 6] {
        let Some(value) = &values[index] else {
            continue;
        };
        let factor = UNITS[index].1;
        if let Kind::Float(number) = value.0 {
            let integer = BigInt::from_f64(number.trunc())
                .ok_or_else(|| Error::new("OverflowError", "non-finite timedelta component"))?;
            let fractional = number.fract() * factor as f64;
            whole += integer * factor + fractional.trunc() as i64;
            remainder += fractional.fract();
        } else {
            whole += value.as_int()? * factor;
        }
    }
    whole += remainder.round() as i64;
    TimeDelta::value(
        whole
            .to_i128()
            .ok_or_else(|| Error::new("OverflowError", "timedelta days out of range"))?,
    )
}
