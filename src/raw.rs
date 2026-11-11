//! Raw SQL, for statements that the query builder does not cover. Each function takes a
//! `&Database` or a `&mut Transaction`, as the query builder's terminal methods do.
//!
//! The SQL string is not escaped and is not checked. It must not contain user input. Put every
//! value in `binds`, in the order of the `?` placeholders, and Orbit sends each one as a bind
//! parameter:
//!
//! ```ignore
//! let title: String = orbit_orm::raw::fetch_scalar(
//!     &db,
//!     "SELECT title FROM posts WHERE slug = ?",
//!     &[slug.into()],
//! )
//! .await?;
//! ```
//!
//! Write `?` on every database. On PostgreSQL Orbit numbers the placeholders (`$1`, `$2`, ...)
//! itself, and a `NULL` bind is written into the SQL as `NULL`, so it needs no type. A `?` inside
//! a quoted string is not a placeholder.

use crate::backend;
use crate::driver::rows::DriverRows;
use crate::transaction::Executor;
use crate::{Error, Value};

/// Runs a statement that changes rows, such as `UPDATE` or `DELETE`, and returns the rows
/// affected. Use `insert` on `Query` for a plain insert, which also returns the id.
pub async fn execute<'a>(db: impl Executor<'a>, sql: &str, binds: &[Value]) -> Result<u64, Error> {
    let target = db.target();
    let (sql, values) = target.driver().prepare(sql, binds);
    Ok(backend::execute(target, &sql, &values).await?.rows_affected)
}

/// Runs a `SELECT` and returns every row, decoded as `T`. A tuple of column types or a struct
/// that implements `FromRow` both work.
pub async fn fetch_all<'a, T>(
    db: impl Executor<'a>,
    sql: &str,
    binds: &[Value],
) -> Result<Vec<T>, Error>
where
    T: DriverRows + Send + Unpin,
{
    let target = db.target();
    let (sql, values) = target.driver().prepare(sql, binds);
    Ok(backend::fetch_all::<T>(target, &sql, &values).await?)
}

/// Runs a `SELECT` and returns its first row, decoded as `T`, or `None` when no row matches.
pub async fn fetch_optional<'a, T>(
    db: impl Executor<'a>,
    sql: &str,
    binds: &[Value],
) -> Result<Option<T>, Error>
where
    T: DriverRows + Send + Unpin,
{
    let target = db.target();
    let (sql, values) = target.driver().prepare(sql, binds);
    Ok(backend::fetch_optional::<T>(target, &sql, &values).await?)
}

/// Runs a `SELECT` that returns one column, such as `SELECT COUNT(*)`, and returns its first
/// value as `T`. It is an error when no row comes back.
///
/// On MySQL, `LAST_INSERT_ID()` is per connection, so run it on the same connection as the
/// insert: inside the same `Transaction`, not on the pool.
pub async fn fetch_scalar<'a, T>(
    db: impl Executor<'a>,
    sql: &str,
    binds: &[Value],
) -> Result<T, Error>
where
    (T,): DriverRows + Send + Unpin,
{
    let target = db.target();
    let (sql, values) = target.driver().prepare(sql, binds);
    Ok(backend::scalar::<T>(target, &sql, &values).await?)
}
