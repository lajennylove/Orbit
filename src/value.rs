use chrono::{NaiveDate, NaiveDateTime};
use sqlx::Arguments;
use sqlx::mysql::MySqlArguments;

use crate::Error;

/// A value bound to a query. Orbit sends every value as a bind parameter, never as SQL text.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    UInt(u64),
    Float(f64),
    Text(String),
    /// A `DATETIME` or `TIMESTAMP`, with its microseconds.
    DateTime(NaiveDateTime),
    /// A `DATE`.
    Date(NaiveDate),
    /// Raw bytes, for `BLOB` columns. Use `Text` for strings.
    Bytes(Vec<u8>),
}

impl From<bool> for Value {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

impl From<i32> for Value {
    fn from(value: i32) -> Self {
        Self::Int(i64::from(value))
    }
}

impl From<i64> for Value {
    fn from(value: i64) -> Self {
        Self::Int(value)
    }
}

impl From<u32> for Value {
    fn from(value: u32) -> Self {
        Self::UInt(u64::from(value))
    }
}

impl From<u64> for Value {
    fn from(value: u64) -> Self {
        Self::UInt(value)
    }
}

impl From<f64> for Value {
    fn from(value: f64) -> Self {
        Self::Float(value)
    }
}

impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Self::Text(value.to_owned())
    }
}

impl From<String> for Value {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}

impl From<NaiveDateTime> for Value {
    fn from(value: NaiveDateTime) -> Self {
        Self::DateTime(value)
    }
}

impl From<NaiveDate> for Value {
    fn from(value: NaiveDate) -> Self {
        Self::Date(value)
    }
}

impl From<Vec<u8>> for Value {
    fn from(value: Vec<u8>) -> Self {
        Self::Bytes(value)
    }
}

impl<T: Into<Value>> From<Option<T>> for Value {
    fn from(value: Option<T>) -> Self {
        value.map_or(Self::Null, Into::into)
    }
}

/// Binds `values` in order, for the `?` placeholders of a statement.
pub(crate) fn bind(values: Vec<Value>) -> Result<MySqlArguments, Error> {
    let mut args = MySqlArguments::default();
    for value in values {
        let added = match value {
            Value::Null => args.add(None::<i64>),
            Value::Bool(v) => args.add(v),
            Value::Int(v) => args.add(v),
            Value::UInt(v) => args.add(v),
            Value::Float(v) => args.add(v),
            Value::Text(v) => args.add(v),
            Value::DateTime(v) => args.add(v),
            Value::Date(v) => args.add(v),
            Value::Bytes(v) => args.add(v),
        };
        added.map_err(|e| Error::Database(sqlx::Error::Encode(e)))?;
    }
    Ok(args)
}
