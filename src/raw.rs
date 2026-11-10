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

use sqlx::mysql::MySqlRow;
use sqlx::{AssertSqlSafe, Decode, FromRow, MySql, Type};

use crate::query::{execute as run_execute, fetch_rows};
use crate::transaction::{Executor, Target};
use crate::value::bind;
use crate::{Error, Value};

/// Runs a statement that changes rows, such as `UPDATE` or `DELETE`, and returns the rows
/// affected. Use `insert` on `Query` for a plain insert, which also returns the id.
pub async fn execute<'a>(db: impl Executor<'a>, sql: &str, binds: &[Value]) -> Result<u64, Error> {
    let result = run_execute(db.target(), sql.to_owned(), bind(binds.to_vec())?).await?;
    Ok(result.rows_affected())
}

/// Runs a `SELECT` and returns every row, decoded as `T`. A tuple of column types or a struct
/// that implements `FromRow` both work.
pub async fn fetch_all<'a, T>(
    db: impl Executor<'a>,
    sql: &str,
    binds: &[Value],
) -> Result<Vec<T>, Error>
where
    T: for<'r> FromRow<'r, MySqlRow> + Send + Unpin,
{
    fetch_rows(db.target(), sql.to_owned(), bind(binds.to_vec())?).await
}

/// Runs a `SELECT` and returns its first row, decoded as `T`, or `None` when no row matches.
pub async fn fetch_optional<'a, T>(
    db: impl Executor<'a>,
    sql: &str,
    binds: &[Value],
) -> Result<Option<T>, Error>
where
    T: for<'r> FromRow<'r, MySqlRow> + Send + Unpin,
{
    let query =
        sqlx::query_as_with::<_, T, _>(AssertSqlSafe(sql.to_owned()), bind(binds.to_vec())?);
    Ok(match db.target() {
        Target::Pool(pool) => query.fetch_optional(pool).await?,
        Target::Transaction(conn) => query.fetch_optional(conn).await?,
    })
}

/// Runs a `SELECT` that returns one column, such as `SELECT COUNT(*)` or
/// `SELECT LAST_INSERT_ID()`, and returns its first value as `T`. It is an error when no row
/// comes back.
///
/// `LAST_INSERT_ID()` is per connection, so run it on the same connection as the insert: inside
/// the same `Transaction`, not on the pool.
pub async fn fetch_scalar<'a, T>(
    db: impl Executor<'a>,
    sql: &str,
    binds: &[Value],
) -> Result<T, Error>
where
    T: Send + Unpin + Type<MySql> + for<'r> Decode<'r, MySql>,
{
    let query =
        sqlx::query_scalar_with::<_, T, _>(AssertSqlSafe(sql.to_owned()), bind(binds.to_vec())?);
    Ok(match db.target() {
        Target::Pool(pool) => query.fetch_one(pool).await?,
        Target::Transaction(conn) => query.fetch_one(conn).await?,
    })
}
