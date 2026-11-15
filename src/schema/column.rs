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
    /// A date and time without a time zone, with its fractional-second precision.
    DateTime(u32),
    /// A `TIMESTAMP`, with its fractional-second precision.
    Timestamp(u32),
    Binary,
}

/// A column of a table being created or changed. Chain `nullable`, `default`, `use_current`,
/// `length`, `precision`, `primary` and `change` on it.
#[derive(Debug, Clone)]
pub struct Column {
    pub(crate) name: String,
    pub(crate) ty: ColumnType,
    nullable: bool,
    default: Option<Value>,
    use_current: bool,
    pub(crate) primary: bool,
    pub(crate) change: bool,
}

impl Column {
    pub(crate) fn new(name: &str, ty: ColumnType) -> Self {
        Self {
            name: name.to_owned(),
            ty,
            nullable: false,
            default: None,
            use_current: false,
            primary: false,
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

    /// Sets the fractional-second precision of a `date_time` or `timestamp` column, as the
    /// `$precision` argument of Laravel's `dateTime` and `timestamp` (default 0): `DATETIME(p)`
    /// or `TIMESTAMP(p)` on MySQL, `TIMESTAMP(p) WITHOUT TIME ZONE` on PostgreSQL. SQLite keeps
    /// `DATETIME`. It has no effect on other types.
    pub fn precision(&mut self, precision: u32) -> &mut Self {
        if let ColumnType::DateTime(p) | ColumnType::Timestamp(p) = &mut self.ty {
            *p = precision;
        }
        self
    }

    /// Makes the current time the default of a `date_time` or `timestamp` column, as Laravel's
    /// `->useCurrent()`: `DEFAULT CURRENT_TIMESTAMP`, or `CURRENT_TIMESTAMP(p)` on MySQL with a
    /// precision p above 0. It takes the place of any `default` value. As in Laravel, it has no
    /// effect on other types.
    pub fn use_current(&mut self) -> &mut Self {
        self.use_current = true;
        self
    }

    /// Makes this column the table's primary key, as Laravel's `->primary()`: `PRIMARY KEY` at
    /// the end of the column definition, for a key that is not an auto-incrementing `id`, such as
    /// `t.string("id").length(64).primary()`. For a key over several columns, use
    /// [`Blueprint::primary`](super::Blueprint::primary). In [`schema::table`](super::table)
    /// SQLite cannot add it, and no database can take it with `.change()`: both are
    /// [`Error::Unsupported`].
    pub fn primary(&mut self) -> &mut Self {
        self.primary = true;
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

    /// The column's SQL: its quoted name, type, nullability, default and `PRIMARY KEY`.
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
        if let Some(default) = self.default_sql(driver, dialect)? {
            sql.push_str(" DEFAULT ");
            sql.push_str(&default);
        }
        if self.primary {
            sql.push_str(" PRIMARY KEY");
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
        let default = match self.default_sql(driver, Dialect::Postgres)? {
            Some(default) => format!("SET DEFAULT {default}"),
            None => "DROP DEFAULT".to_owned(),
        };
        Ok(format!(
            "ALTER COLUMN {name} TYPE {ty}, ALTER COLUMN {name} {null}, \
             ALTER COLUMN {name} {default}"
        ))
    }

    /// The SQL of the column's default, if it has one: the current time for `use_current` on a
    /// date and time column, or else the literal `default` value.
    fn default_sql(&self, driver: Driver, dialect: Dialect) -> Result<Option<String>, Error> {
        if self.use_current {
            match (self.ty, dialect) {
                (ColumnType::DateTime(p) | ColumnType::Timestamp(p), Dialect::MySql) if p > 0 => {
                    return Ok(Some(format!("CURRENT_TIMESTAMP({p})")));
                }
                (ColumnType::DateTime(_) | ColumnType::Timestamp(_), _) => {
                    return Ok(Some("CURRENT_TIMESTAMP".to_owned()));
                }
                _ => {}
            }
        }
        self.default
            .as_ref()
            .map(|value| literal(value, driver, dialect))
            .transpose()
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
            (ColumnType::DateTime(p) | ColumnType::Timestamp(p), Dialect::Postgres) => {
                format!("TIMESTAMP({p}) WITHOUT TIME ZONE")
            }
            (ColumnType::DateTime(_) | ColumnType::Timestamp(_), Dialect::Sqlite) => {
                "DATETIME".to_owned()
            }
            (ColumnType::DateTime(0), _) => "DATETIME".to_owned(),
            (ColumnType::DateTime(p), _) => format!("DATETIME({p})"),
            (ColumnType::Timestamp(0), _) => "TIMESTAMP".to_owned(),
            (ColumnType::Timestamp(p), _) => format!("TIMESTAMP({p})"),
            (ColumnType::Binary, Dialect::Postgres) => "BYTEA".to_owned(),
            (ColumnType::Binary, _) => "BLOB".to_owned(),
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
