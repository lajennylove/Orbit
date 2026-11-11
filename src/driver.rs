//! The databases Orbit supports, and what differs between them in SQL.
//!
//! Each database is a Cargo feature: `mysql` (the default), `postgres` and `sqlite`. Only the
//! enabled ones exist in the build, so a crate built with `sqlite` alone has no MySQL types.

use sqlx::FromRow;

use crate::Value;

/// A database that Orbit can talk to. The variants are the enabled Cargo features.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Driver {
    /// MySQL and MariaDB.
    #[cfg(feature = "mysql")]
    MySql,
    /// PostgreSQL.
    #[cfg(feature = "postgres")]
    Postgres,
    /// SQLite, in a file or in memory.
    #[cfg(feature = "sqlite")]
    Sqlite,
}

impl Driver {
    /// The name of the database, for messages.
    pub fn name(self) -> &'static str {
        match self {
            #[cfg(feature = "mysql")]
            Self::MySql => "MySQL",
            #[cfg(feature = "postgres")]
            Self::Postgres => "PostgreSQL",
            #[cfg(feature = "sqlite")]
            Self::Sqlite => "SQLite",
        }
    }

    /// Quotes a table or column name. The name is checked by the caller: it is never empty and
    /// has no quote character.
    pub(crate) fn quote(self, name: &str) -> String {
        match self {
            #[cfg(feature = "mysql")]
            Self::MySql => format!("`{name}`"),
            #[cfg(any(feature = "postgres", feature = "sqlite"))]
            _ => format!("\"{name}\""),
        }
    }

    /// The placeholder for the `n`th bound value, counted from 1. PostgreSQL numbers them;
    /// MySQL and SQLite take `?`.
    #[cfg_attr(not(feature = "postgres"), allow(unused_variables))]
    pub(crate) fn placeholder(self, n: usize) -> String {
        match self {
            #[cfg(feature = "mysql")]
            Self::MySql => "?".to_owned(),
            #[cfg(feature = "postgres")]
            Self::Postgres => format!("${n}"),
            #[cfg(feature = "sqlite")]
            Self::Sqlite => "?".to_owned(),
        }
    }

    /// The clause that locks the selected rows, or nothing. SQLite has no row locks: its writers
    /// are serialised by the database, so the clause is left out, as Laravel's SQLite grammar
    /// does.
    pub(crate) fn lock_clause(self) -> &'static str {
        match self {
            #[cfg(feature = "mysql")]
            Self::MySql => " FOR UPDATE",
            #[cfg(feature = "postgres")]
            Self::Postgres => " FOR UPDATE",
            #[cfg(feature = "sqlite")]
            Self::Sqlite => "",
        }
    }

    /// The `LIMIT` that an `OFFSET` without a `LIMIT` needs. MySQL and SQLite require one;
    /// PostgreSQL does not.
    pub(crate) fn offset_only_limit(self) -> Option<&'static str> {
        match self {
            #[cfg(feature = "mysql")]
            Self::MySql => Some("18446744073709551615"),
            #[cfg(feature = "postgres")]
            Self::Postgres => None,
            #[cfg(feature = "sqlite")]
            Self::Sqlite => Some("-1"),
        }
    }

    /// The part of an `INSERT` that has no columns.
    pub(crate) fn empty_insert(self) -> &'static str {
        match self {
            #[cfg(feature = "mysql")]
            Self::MySql => "() VALUES ()",
            #[cfg(any(feature = "postgres", feature = "sqlite"))]
            _ => "DEFAULT VALUES",
        }
    }

    /// The integer type for a `CAST`, wide enough for a count.
    pub(crate) fn bigint(self) -> &'static str {
        match self {
            #[cfg(feature = "mysql")]
            Self::MySql => "SIGNED",
            #[cfg(feature = "postgres")]
            Self::Postgres => "BIGINT",
            #[cfg(feature = "sqlite")]
            Self::Sqlite => "INTEGER",
        }
    }

    /// Whether `replace` can run. MySQL and SQLite have `REPLACE INTO`; PostgreSQL has no such
    /// statement, and `INSERT ... ON CONFLICT` would not delete the old row, so it is refused.
    pub(crate) fn supports_replace(self) -> bool {
        match self {
            #[cfg(feature = "mysql")]
            Self::MySql => true,
            #[cfg(feature = "postgres")]
            Self::Postgres => false,
            #[cfg(feature = "sqlite")]
            Self::Sqlite => true,
        }
    }

    /// Whether a `NULL` value is written into the SQL, not bound. PostgreSQL types a bound
    /// parameter, so a bound `NULL` would not fit every column; a literal `NULL` fits any.
    pub(crate) fn inlines_null(self) -> bool {
        match self {
            #[cfg(feature = "mysql")]
            Self::MySql => false,
            #[cfg(feature = "postgres")]
            Self::Postgres => true,
            #[cfg(feature = "sqlite")]
            Self::Sqlite => false,
        }
    }

    /// Whether the insert id comes from `RETURNING` (PostgreSQL), not from the statement result.
    pub(crate) fn insert_returns_id(self) -> bool {
        match self {
            #[cfg(feature = "mysql")]
            Self::MySql => false,
            #[cfg(feature = "postgres")]
            Self::Postgres => true,
            #[cfg(feature = "sqlite")]
            Self::Sqlite => false,
        }
    }

    /// Prepares a raw statement: `?` placeholders become this driver's placeholders, and the
    /// values are the ones that stay bound. On PostgreSQL a `NULL` is written into the SQL, as
    /// for the query builder, so it is not bound. Question marks inside quotes are left alone.
    pub(crate) fn prepare(self, sql: &str, binds: &[Value]) -> (String, Vec<Value>) {
        match self {
            #[cfg(feature = "mysql")]
            Self::MySql => (sql.to_owned(), binds.to_vec()),
            #[cfg(feature = "postgres")]
            Self::Postgres => prepare_postgres(sql, binds),
            #[cfg(feature = "sqlite")]
            Self::Sqlite => (sql.to_owned(), binds.to_vec()),
        }
    }
}

/// Numbers the `?` placeholders of `sql` as `$1`, `$2`, ..., skipping any inside a string,
/// identifier or comment. A placeholder whose value is `NULL` becomes the literal `NULL`.
#[cfg(feature = "postgres")]
fn prepare_postgres(sql: &str, binds: &[Value]) -> (String, Vec<Value>) {
    let mut out = String::with_capacity(sql.len() + 8);
    let mut kept = Vec::new();
    let mut seen = 0usize;
    let mut chars = sql.chars().peekable();
    let mut quote: Option<char> = None;
    while let Some(c) = chars.next() {
        match quote {
            Some(q) => {
                out.push(c);
                if c == q {
                    quote = None;
                }
            }
            None => match c {
                '\'' | '"' => {
                    quote = Some(c);
                    out.push(c);
                }
                '-' if chars.peek() == Some(&'-') => {
                    out.push(c);
                    for next in chars.by_ref() {
                        out.push(next);
                        if next == '\n' {
                            break;
                        }
                    }
                }
                '?' => {
                    let value = binds.get(seen).cloned().unwrap_or(Value::Null);
                    seen += 1;
                    if matches!(value, Value::Null) {
                        out.push_str("NULL");
                    } else {
                        kept.push(value);
                        out.push_str(&format!("${}", kept.len()));
                    }
                }
                _ => out.push(c),
            },
        }
    }
    (out, kept)
}

/// Row decoding for each enabled driver. A model must decode from every enabled database's row
/// type, so `Model` bounds on these. Each trait is implemented for every type when its driver is
/// not enabled, so it adds nothing to the bound then.
#[doc(hidden)]
pub mod rows {
    #[cfg(feature = "mysql")]
    pub trait RowMySql: for<'r> super::FromRow<'r, sqlx::mysql::MySqlRow> {}
    #[cfg(feature = "mysql")]
    impl<T: for<'r> super::FromRow<'r, sqlx::mysql::MySqlRow>> RowMySql for T {}
    #[cfg(not(feature = "mysql"))]
    pub trait RowMySql {}
    #[cfg(not(feature = "mysql"))]
    impl<T> RowMySql for T {}

    #[cfg(feature = "postgres")]
    pub trait RowPostgres: for<'r> super::FromRow<'r, sqlx::postgres::PgRow> {}
    #[cfg(feature = "postgres")]
    impl<T: for<'r> super::FromRow<'r, sqlx::postgres::PgRow>> RowPostgres for T {}
    #[cfg(not(feature = "postgres"))]
    pub trait RowPostgres {}
    #[cfg(not(feature = "postgres"))]
    impl<T> RowPostgres for T {}

    #[cfg(feature = "sqlite")]
    pub trait RowSqlite: for<'r> super::FromRow<'r, sqlx::sqlite::SqliteRow> {}
    #[cfg(feature = "sqlite")]
    impl<T: for<'r> super::FromRow<'r, sqlx::sqlite::SqliteRow>> RowSqlite for T {}
    #[cfg(not(feature = "sqlite"))]
    pub trait RowSqlite {}
    #[cfg(not(feature = "sqlite"))]
    impl<T> RowSqlite for T {}

    /// Decodes from every enabled driver's row type.
    pub trait DriverRows: RowMySql + RowPostgres + RowSqlite {}
    impl<T: RowMySql + RowPostgres + RowSqlite> DriverRows for T {}
}

#[cfg(not(any(feature = "mysql", feature = "postgres", feature = "sqlite")))]
compile_error!("orbit-orm needs at least one database feature: mysql, postgres or sqlite");
