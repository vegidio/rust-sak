use std::borrow::Cow;
use std::collections::HashMap;

use opentelemetry::Key;
use opentelemetry::logs::AnyValue;

/// A value attached to a log record field.
///
/// This exists because [`AnyValue`] — OpenTelemetry's own value type — only converts from `&'static str` and from
/// integers up to `u32`. Telemetry fields are routinely built from borrowed strings and from `u64`/`usize` byte
/// counts, so `Value` widens the set of accepted types and converts to [`AnyValue`] at emit time.
///
/// You rarely name this type: [`Event::field`](super::Event::field) takes `impl Into<Value>`.
///
/// ```
/// use rust_sak::o11y::Value;
///
/// let owned = String::from("avif");
/// assert_eq!(Value::from(owned.as_str()), Value::String("avif".to_string()));
/// assert_eq!(Value::from(1_048_576u64), Value::Int(1_048_576));
/// assert_eq!(Value::from(vec![1i64, 2]), Value::List(vec![Value::Int(1), Value::Int(2)]));
/// ```
///
/// Unsigned values above [`i64::MAX`] saturate rather than wrap, since OTLP has no unsigned integer type:
///
/// ```
/// use rust_sak::o11y::Value;
///
/// assert_eq!(Value::from(u64::MAX), Value::Int(i64::MAX));
/// ```
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// A boolean.
    Bool(bool),
    /// A signed 64-bit integer. All integer types convert into this.
    Int(i64),
    /// A double-precision float.
    Double(f64),
    /// A UTF-8 string.
    String(String),
    /// A raw byte array.
    Bytes(Vec<u8>),
    /// An ordered list of values.
    List(Vec<Value>),
    /// A set of named values, arbitrarily nested.
    Map(Vec<(String, Value)>),
}

impl Value {
    /// Converts into OpenTelemetry's own value type. The one place this module depends on the `AnyValue` shape.
    pub(super) fn into_any_value(self) -> AnyValue {
        match self {
            Value::Bool(value) => AnyValue::Boolean(value),
            Value::Int(value) => AnyValue::Int(value),
            Value::Double(value) => AnyValue::Double(value),
            Value::String(value) => AnyValue::String(value.into()),
            Value::Bytes(value) => AnyValue::Bytes(Box::new(value)),
            Value::List(values) => AnyValue::ListAny(Box::new(values.into_iter().map(Value::into_any_value).collect())),
            Value::Map(entries) => AnyValue::Map(Box::new(
                entries
                    .into_iter()
                    .map(|(key, value)| (Key::from(key), value.into_any_value()))
                    .collect::<HashMap<_, _>>(),
            )),
        }
    }
}

impl From<bool> for Value {
    fn from(value: bool) -> Self {
        Value::Bool(value)
    }
}

impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Value::String(value.to_string())
    }
}

impl From<String> for Value {
    fn from(value: String) -> Self {
        Value::String(value)
    }
}

impl From<Cow<'_, str>> for Value {
    fn from(value: Cow<'_, str>) -> Self {
        Value::String(value.into_owned())
    }
}

impl From<&[u8]> for Value {
    fn from(value: &[u8]) -> Self {
        Value::Bytes(value.to_vec())
    }
}

impl<T: Into<Value>> From<Vec<T>> for Value {
    fn from(values: Vec<T>) -> Self {
        Value::List(values.into_iter().map(Into::into).collect())
    }
}

impl<K: Into<String>, V: Into<Value>> From<HashMap<K, V>> for Value {
    fn from(entries: HashMap<K, V>) -> Self {
        Value::Map(entries.into_iter().map(|(k, v)| (k.into(), v.into())).collect())
    }
}

/// Implements `From<$ty>` for [`Value`] by widening into `Value::Int`, for the integer types that always fit.
macro_rules! from_int {
    ($($ty:ty),*) => {
        $(
            impl From<$ty> for Value {
                fn from(value: $ty) -> Self {
                    Value::Int(i64::from(value))
                }
            }
        )*
    };
}

from_int!(i8, i16, i32, i64, u8, u16, u32);

/// Implements `From<$ty>` for [`Value`] by saturating into `Value::Int`. OTLP has no unsigned or 128-bit integer
/// type, so values above [`i64::MAX`] clamp rather than wrap — a wrong-but-bounded number beats a negative one.
macro_rules! from_int_saturating {
    ($($ty:ty),*) => {
        $(
            impl From<$ty> for Value {
                fn from(value: $ty) -> Self {
                    Value::Int(i64::try_from(value).unwrap_or(i64::MAX))
                }
            }
        )*
    };
}

from_int_saturating!(u64, usize, isize);

impl From<f32> for Value {
    fn from(value: f32) -> Self {
        Value::Double(f64::from(value))
    }
}

impl From<f64> for Value {
    fn from(value: f64) -> Self {
        Value::Double(value)
    }
}
