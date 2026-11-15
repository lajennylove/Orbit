use chrono::{NaiveDate, NaiveDateTime};

use super::Dialect;
use crate::{Driver, Error, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ColumnType {
    Id,
    String(u32),
    Text,
    Integer,
    BigInteger,
    Boolean,
    DateTime,
}

/// A column of a table being created or changed. Chain `nullable`, `default`, `length` and
/// `change` on it.
#[derive(Debug, Clone)]
pub struct Column {
    pub(crate) name: String,
    pub(crate) ty: ColumnType,
    nullable: bool,
    default: Option<Value>,
    pub(crate) change: bool,
}

impl Column {
    pub(crate) fn new(name: &str, ty: ColumnType) -> Self {
        Self {
            name: name.to_owned(),
            ty,
            nullable: false,
            default: None,
            change: false,
        }
    }

    /// Lets the column hold `NULL`. Columns are `NOT NULL` otherwise, as in Laravel.
    pub fn nullable(&mut self) -> &mut Self {
        self.nullable = true;
        self
    }

    /// Sets the default value, written into the SQL as a literal.
    pub fn default(&mut self, value: impl Into<Value>) -> &mut Self {
        self.default = Some(value.into());
        self
    }

    /// Sets the length of a `string` column. It has no effect on other types.
    pub fn length(&mut self, length: u32) -> &mut Self {
        if let ColumnType::String(n) = &mut self.ty {
            *n = length;
        }
        self
    }

    /// Modifies this existing column instead of adding it, as Laravel's `->change()`. It only
    /// works inside [`schema::table`](super::table): the column takes the type, nullability and
    /// default described here. SQLite cannot change a column in place, so rendering it there is
    /// [`Error::Unsupported`]; so is changing an `id` column.
    pub fn change(&mut self) -> &mut Self {
        self.change = true;
        self
    }

    /// The column's SQL: its quoted name, type, nullability and default.
    pub(crate) fn render(&self, driver: Driver, dialect: Dialect) -> Result<String, Error> {
        crate::query::validate(&self.name)?;
        let name = driver.quote(&self.name);
        if self.ty == ColumnType::Id {
            let id = match dialect {
                Dialect::MySql => "BIGINT AUTO_INCREMENT PRIMARY KEY",
                // BIGSERIAL, as Laravel's PostgreSQL grammar writes `bigIncrements`.
                Dialect::Postgres => "BIGSERIAL PRIMARY KEY",
                Dialect::Sqlite => "INTEGER PRIMARY KEY AUTOINCREMENT",
            };
            return Ok(format!("{name} {id}"));
        }
        let mut sql = format!("{name} {}", self.sql_type(dialect));
        sql.push_str(if self.nullable { " NULL" } else { " NOT NULL" });
        if let Some(value) = &self.default {
            sql.push_str(" DEFAULT ");
            sql.push_str(&literal(value, driver, dialect)?);
        }
        Ok(sql)
    }

    /// The PostgreSQL clauses that change this column to its description: its type, then its
    /// nullability, then its default, comma-separated for one `ALTER TABLE`.
    pub(crate) fn render_change_postgres(&self, driver: Driver) -> Result<String, Error> {
        crate::query::validate(&self.name)?;
        let name = driver.quote(&self.name);
        let ty = self.sql_type(Dialect::Postgres);
        let null = if self.nullable {
            "DROP NOT NULL"
        } else {
            "SET NOT NULL"
        };
        let default = match &self.default {
            Some(value) => format!("SET DEFAULT {}", literal(value, driver, Dialect::Postgres)?),
            None => "DROP DEFAULT".to_owned(),
        };
        Ok(format!(
            "ALTER COLUMN {name} TYPE {ty}, ALTER COLUMN {name} {null}, \
             ALTER COLUMN {name} {default}"
        ))
    }

    fn sql_type(&self, dialect: Dialect) -> String {
        match (self.ty, dialect) {
            (ColumnType::Id, _) => unreachable!("an id column is rendered whole"),
            (ColumnType::String(_), Dialect::Sqlite) => "TEXT".to_owned(),
            (ColumnType::String(n), _) => format!("VARCHAR({n})"),
            (ColumnType::Text, _) => "TEXT".to_owned(),
            (ColumnType::Integer, _) => "INTEGER".to_owned(),
            (ColumnType::BigInteger, Dialect::Sqlite) => "INTEGER".to_owned(),
            (ColumnType::BigInteger, _) => "BIGINT".to_owned(),
            (ColumnType::Boolean, Dialect::Postgres) => "BOOLEAN".to_owned(),
            (ColumnType::Boolean, _) => "TINYINT(1)".to_owned(),
            (ColumnType::DateTime, Dialect::Postgres) => {
                "TIMESTAMP(0) WITHOUT TIME ZONE".to_owned()
            }
            (ColumnType::DateTime, _) => "DATETIME".to_owned(),
        }
    }
}

/// A default value as a SQL literal. DDL cannot take bound values.
fn literal(value: &Value, driver: Driver, dialect: Dialect) -> Result<String, Error> {
    let unsupported = || Error::Unsupported {
        operation: "a column default of this type",
        driver: driver.name(),
    };
    Ok(match value {
        Value::Null => "NULL".to_owned(),
        Value::Bool(b) => match (dialect, b) {
            (Dialect::Postgres, true) => "true".to_owned(),
            (Dialect::Postgres, false) => "false".to_owned(),
            (_, true) => "1".to_owned(),
            (_, false) => "0".to_owned(),
        },
        Value::Int(n) => n.to_string(),
        Value::UInt(n) => n.to_string(),
        Value::Float(f) if f.is_finite() => format!("{f:?}"),
        Value::Float(_) => return Err(unsupported()),
        Value::Text(s) => quote_text(s, dialect),
        Value::DateTime(d) => quote_text(&format_datetime(d), dialect),
        Value::Date(d) => quote_text(&format_date(d), dialect),
        Value::Bytes(_) => return Err(unsupported()),
    })
}

fn format_datetime(d: &NaiveDateTime) -> String {
    d.format("%Y-%m-%d %H:%M:%S").to_string()
}

fn format_date(d: &NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}

/// A quoted string literal. A quote is doubled everywhere; MySQL also reads a backslash as an
/// escape, so it is doubled there.
fn quote_text(s: &str, dialect: Dialect) -> String {
    let mut out = s.replace('\'', "''");
    if dialect == Dialect::MySql {
        out = out.replace('\\', "\\\\");
    }
    format!("'{out}'")
}
