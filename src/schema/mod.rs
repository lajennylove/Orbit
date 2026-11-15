//! A database-neutral schema builder, after Laravel's `Schema::create`, `Schema::table` and
//! `Blueprint`.
//!
//! A migration describes a table once and the same code runs on SQLite, MySQL and MariaDB, and
//! PostgreSQL:
//!
//! ```
//! use orbit_orm::schema::{self, Action};
//!
//! let posts = schema::create("posts", |t| {
//!     t.id();
//!     t.string("title");
//!     t.text("body").nullable();
//!     t.big_integer("user_id");
//!     t.foreign("user_id").references("users", "id").on_delete(Action::Restrict);
//!     t.unique(["title"]);
//! });
//! # let _ = posts;
//!
//! let rename = schema::table("posts", |t| {
//!     t.string("subtitle").nullable();
//!     t.rename_column("body", "content");
//! });
//! # let _ = rename;
//! ```
//!
//! A [`Statement`] is only a description. [`Statement::to_sql`] renders it for a [`Driver`] as a
//! list of statements to run in order: PostgreSQL and SQLite create a plain index with a separate
//! `CREATE INDEX`, and sqlx does not run several statements in one string on every database.

mod blueprint;
mod column;

use crate::{Driver, Error};

pub use blueprint::{Action, Blueprint, ForeignKey};
pub use column::Column;

/// The database families that differ in DDL. A [`Driver`] maps to one of these, so the SQL below
/// matches on a type that has all three variants whatever Cargo features are on.
#[derive(Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum Dialect {
    MySql,
    Postgres,
    Sqlite,
}

impl Dialect {
    pub(crate) fn of(driver: Driver) -> Self {
        match driver {
            #[cfg(feature = "mysql")]
            Driver::MySql => Self::MySql,
            #[cfg(feature = "postgres")]
            Driver::Postgres => Self::Postgres,
            #[cfg(feature = "sqlite")]
            Driver::Sqlite => Self::Sqlite,
        }
    }
}

/// A schema change, ready to render for any database.
#[derive(Debug, Clone)]
pub struct Statement {
    kind: Kind,
}

#[derive(Debug, Clone)]
enum Kind {
    Create {
        table: String,
        blueprint: Blueprint,
        if_not_exists: bool,
    },
    Alter {
        table: String,
        blueprint: Blueprint,
    },
    Drop {
        table: String,
        if_exists: bool,
    },
}

/// Creates a table. The closure describes its columns, indexes and foreign keys.
pub fn create(table: &str, f: impl FnOnce(&mut Blueprint)) -> Statement {
    creating(table, f, false)
}

/// Creates a table unless it exists: `CREATE TABLE IF NOT EXISTS`. A plain index that PostgreSQL
/// and SQLite create with a separate statement takes `CREATE INDEX IF NOT EXISTS`.
///
/// Laravel has no such method; a Laravel migration writes `if (! Schema::hasTable(...))`. That
/// needs a live connection, and a schema migration here is rendered without one, so the check is
/// in the SQL instead. An existing table is left as it is, even when its columns differ.
pub fn create_if_not_exists(table: &str, f: impl FnOnce(&mut Blueprint)) -> Statement {
    creating(table, f, true)
}

fn creating(table: &str, f: impl FnOnce(&mut Blueprint), if_not_exists: bool) -> Statement {
    let mut blueprint = Blueprint::new();
    f(&mut blueprint);
    Statement {
        kind: Kind::Create {
            table: table.to_owned(),
            blueprint,
            if_not_exists,
        },
    }
}

/// Changes an existing table, as Laravel's `Schema::table`. A column described in the closure is
/// added, or modified when it ends with `.change()`; `rename_column` and `drop_column` rename and
/// drop columns.
///
/// Each operation renders as its own `ALTER TABLE` statement on every database: the added columns
/// first, then the changed columns, then the other commands in the order they were called. An
/// index or foreign key declared here is not rendered yet: it is [`Error::Unsupported`].
pub fn table(table: &str, f: impl FnOnce(&mut Blueprint)) -> Statement {
    let mut blueprint = Blueprint::new();
    f(&mut blueprint);
    Statement {
        kind: Kind::Alter {
            table: table.to_owned(),
            blueprint,
        },
    }
}

/// Drops a table. It is an error in the database if the table does not exist.
pub fn drop(table: &str) -> Statement {
    Statement {
        kind: Kind::Drop {
            table: table.to_owned(),
            if_exists: false,
        },
    }
}

/// Drops a table if it exists.
pub fn drop_if_exists(table: &str) -> Statement {
    Statement {
        kind: Kind::Drop {
            table: table.to_owned(),
            if_exists: true,
        },
    }
}

impl Statement {
    /// Renders the statement for `driver`: one SQL statement per element, to run in order.
    ///
    /// # Panics
    ///
    /// Panics when a name is invalid or a default has no SQL literal. Use
    /// [`try_to_sql`](Self::try_to_sql) to get the error instead.
    pub fn to_sql(&self, driver: Driver) -> Vec<String> {
        self.try_to_sql(driver)
            .unwrap_or_else(|e| panic!("cannot render the schema statement: {e}"))
    }

    /// Renders the statement for `driver`, or the error for an invalid name. A table, column or
    /// index column name must not be empty or contain a backtick or a double quote, the rule of
    /// the query builder. An operation the database cannot do, or an alter-only command such as
    /// `rename_column` inside `create`, is [`Error::Unsupported`].
    pub fn try_to_sql(&self, driver: Driver) -> Result<Vec<String>, Error> {
        let dialect = Dialect::of(driver);
        match &self.kind {
            Kind::Create {
                table,
                blueprint,
                if_not_exists,
            } => blueprint.render_create(driver, dialect, table, *if_not_exists),
            Kind::Alter { table, blueprint } => blueprint.render_alter(driver, dialect, table),
            Kind::Drop { table, if_exists } => {
                crate::query::validate(table)?;
                let exists = if *if_exists { "IF EXISTS " } else { "" };
                Ok(vec![format!("DROP TABLE {exists}{}", driver.quote(table))])
            }
        }
    }
}
