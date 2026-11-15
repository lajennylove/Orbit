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

pub use blueprint::{Action, Blueprint, ForeignKey, Index};
pub use column::Column;

/// A database family. A [`Driver`] maps to one of these. Unlike [`Driver`], all three variants
/// exist whatever Cargo features are on, so a migration can name any of them, for example in
/// [`Statement::only`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Dialect {
    /// MySQL and MariaDB.
    MySql,
    /// PostgreSQL.
    Postgres,
    /// SQLite.
    Sqlite,
}

impl Dialect {
    /// The family of `driver`.
    pub fn of(driver: Driver) -> Self {
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
    /// The only database family the statement renders on, set by [`Statement::only`].
    only: Option<Dialect>,
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
    Raw(String),
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
        only: None,
    }
}

/// Changes an existing table, as Laravel's `Schema::table`. A column described in the closure is
/// added, or modified when it ends with `.change()`; `rename_column` and `drop_column` rename and
/// drop columns; `index`, `unique`, `primary` and `foreign` add keys; `drop_index` and
/// `drop_foreign` drop them.
///
/// Each operation renders as its own statement on every database: the added columns first, then
/// the changed columns, then the other commands in the order they were called. PostgreSQL and
/// SQLite add a plain index with `CREATE INDEX` and drop one with `DROP INDEX`, and SQLite adds a
/// unique index with `CREATE UNIQUE INDEX`; the rest is `ALTER TABLE`. SQLite cannot add or drop
/// a constraint on an existing table: a foreign key and `drop_foreign` render nothing there, and
/// a primary key is [`Error::Unsupported`].
pub fn table(table: &str, f: impl FnOnce(&mut Blueprint)) -> Statement {
    let mut blueprint = Blueprint::new();
    f(&mut blueprint);
    Statement {
        kind: Kind::Alter {
            table: table.to_owned(),
            blueprint,
        },
        only: None,
    }
}

/// Drops a table. It is an error in the database if the table does not exist.
pub fn drop(table: &str) -> Statement {
    Statement {
        kind: Kind::Drop {
            table: table.to_owned(),
            if_exists: false,
        },
        only: None,
    }
}

/// Drops a table if it exists.
pub fn drop_if_exists(table: &str) -> Statement {
    Statement {
        kind: Kind::Drop {
            table: table.to_owned(),
            if_exists: true,
        },
        only: None,
    }
}

/// Runs `sql` as written, as Laravel's `DB::statement` inside a migration.
///
/// The SQL is database-specific: Orbit does not check, quote or translate it, and runs the same
/// string on every database. Use it for data rows (seeding a role, moving values between
/// columns) and for what the builder cannot express, such as a MySQL-only column type. Add
/// [`only`](Statement::only) when the SQL is for one database family. Orbit's own generated code
/// never uses it.
///
/// [`to_sql`](Statement::to_sql) and [`try_to_sql`](Statement::try_to_sql) return the string
/// unchanged, as one statement. An empty or wrong string is reported by the database when it
/// runs.
///
/// ```
/// use orbit_orm::schema::{self, Dialect};
///
/// let seed = schema::statement("INSERT INTO roles (name) VALUES ('admin')");
/// let avatar = schema::statement("ALTER TABLE users MODIFY avatar MEDIUMBLOB").only(Dialect::MySql);
/// # let _ = (seed, avatar);
/// ```
pub fn statement(sql: impl Into<String>) -> Statement {
    Statement {
        kind: Kind::Raw(sql.into()),
        only: None,
    }
}

impl Statement {
    /// Limits the statement to one database family, as a Laravel migration wraps a call in
    /// `if (DB::getDriverName() === 'mysql') { ... }`. On that family the statement renders as
    /// usual; on the others it renders to an empty list, and nothing runs. It works on every
    /// statement: [`create`], [`table`], [`drop`] and [`statement`].
    ///
    /// A statement for another family is not rendered at all, so an invalid name in it is not
    /// reported there either. Calling `only` twice keeps the last call.
    pub fn only(mut self, dialect: Dialect) -> Self {
        self.only = Some(dialect);
        self
    }

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
    /// `rename_column` inside `create`, is [`Error::Unsupported`]. A raw [`statement`] is never
    /// an error, and a statement limited by [`only`](Self::only) to another family renders as an
    /// empty list.
    pub fn try_to_sql(&self, driver: Driver) -> Result<Vec<String>, Error> {
        let dialect = Dialect::of(driver);
        if self.only.is_some_and(|only| only != dialect) {
            return Ok(Vec::new());
        }
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
            Kind::Raw(sql) => Ok(vec![sql.clone()]),
        }
    }
}
