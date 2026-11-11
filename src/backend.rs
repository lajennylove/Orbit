//! Runs statements on whichever database the pool or transaction is on. The query builder
//! produces SQL and `Value`s; this module binds the values with the driver's types and runs the
//! statement. Each driver has its own concrete implementation of `Backend`, so the futures are
//! plain per-driver code; the public functions dispatch on the `Target`.

use std::future::Future;

use sqlx::{AssertSqlSafe, Database as SqlxDatabase, FromRow, IntoArguments};

use crate::Value;
use crate::database::Pool;
use crate::driver::rows::DriverRows;
use crate::transaction::{Target, TxInner};

/// What a statement that changes rows reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Outcome {
    pub rows_affected: u64,
    /// The auto-increment id of the inserted row. `None` on PostgreSQL, which returns the id
    /// from the statement instead (see `Driver::insert_returns_id`).
    pub insert_id: Option<u64>,
}

/// The per-driver operations. Each database implements them for its pool and for a connection.
pub(crate) trait Backend: SqlxDatabase
where
    Self::Arguments: IntoArguments<Self>,
{
    fn arguments(values: &[Value]) -> Result<Self::Arguments, sqlx::Error>;
    fn rows_affected(result: &Self::QueryResult) -> u64;
    fn insert_id(result: &Self::QueryResult) -> Option<u64>;

    fn execute_pool(
        pool: &sqlx::Pool<Self>,
        sql: &str,
        values: &[Value],
    ) -> impl Future<Output = Result<Outcome, sqlx::Error>> + Send;
    fn execute_conn(
        conn: &mut Self::Connection,
        sql: &str,
        values: &[Value],
    ) -> impl Future<Output = Result<Outcome, sqlx::Error>> + Send;
    fn fetch_all_pool<T>(
        pool: &sqlx::Pool<Self>,
        sql: &str,
        values: &[Value],
    ) -> impl Future<Output = Result<Vec<T>, sqlx::Error>> + Send
    where
        T: for<'r> FromRow<'r, Self::Row> + Send + Unpin;
    fn fetch_all_conn<T>(
        conn: &mut Self::Connection,
        sql: &str,
        values: &[Value],
    ) -> impl Future<Output = Result<Vec<T>, sqlx::Error>> + Send
    where
        T: for<'r> FromRow<'r, Self::Row> + Send + Unpin;
    fn fetch_optional_pool<T>(
        pool: &sqlx::Pool<Self>,
        sql: &str,
        values: &[Value],
    ) -> impl Future<Output = Result<Option<T>, sqlx::Error>> + Send
    where
        T: for<'r> FromRow<'r, Self::Row> + Send + Unpin;
    fn fetch_optional_conn<T>(
        conn: &mut Self::Connection,
        sql: &str,
        values: &[Value],
    ) -> impl Future<Output = Result<Option<T>, sqlx::Error>> + Send
    where
        T: for<'r> FromRow<'r, Self::Row> + Send + Unpin;
    fn script_pool(
        pool: &sqlx::Pool<Self>,
        sql: &str,
    ) -> impl Future<Output = Result<(), sqlx::Error>> + Send;
    fn script_conn(
        conn: &mut Self::Connection,
        sql: &str,
    ) -> impl Future<Output = Result<(), sqlx::Error>> + Send;
}

/// The error for an unsigned value that the database has no unsigned type for.
#[cfg(any(feature = "postgres", feature = "sqlite"))]
fn too_large(value: u64) -> sqlx::Error {
    sqlx::Error::Encode(
        format!("the value {value} is larger than i64::MAX, which this database cannot store")
            .into(),
    )
}

/// The methods that every driver implements the same way, written once per driver.
macro_rules! common_impl {
    ($db:ty) => {
        async fn execute_pool(
            pool: &sqlx::Pool<$db>,
            sql: &str,
            values: &[Value],
        ) -> Result<Outcome, sqlx::Error> {
            let args = <$db as Backend>::arguments(values)?;
            let result = sqlx::query_with::<$db, _>(AssertSqlSafe(sql.to_owned()), args)
                .execute(pool)
                .await?;
            Ok(outcome::<$db>(&result))
        }

        async fn execute_conn(
            conn: &mut <$db as SqlxDatabase>::Connection,
            sql: &str,
            values: &[Value],
        ) -> Result<Outcome, sqlx::Error> {
            let args = <$db as Backend>::arguments(values)?;
            let result = sqlx::query_with::<$db, _>(AssertSqlSafe(sql.to_owned()), args)
                .execute(conn)
                .await?;
            Ok(outcome::<$db>(&result))
        }

        async fn fetch_all_pool<T>(
            pool: &sqlx::Pool<$db>,
            sql: &str,
            values: &[Value],
        ) -> Result<Vec<T>, sqlx::Error>
        where
            T: for<'r> FromRow<'r, <$db as SqlxDatabase>::Row> + Send + Unpin,
        {
            let args = <$db as Backend>::arguments(values)?;
            sqlx::query_as_with::<$db, T, _>(AssertSqlSafe(sql.to_owned()), args)
                .fetch_all(pool)
                .await
        }

        async fn fetch_all_conn<T>(
            conn: &mut <$db as SqlxDatabase>::Connection,
            sql: &str,
            values: &[Value],
        ) -> Result<Vec<T>, sqlx::Error>
        where
            T: for<'r> FromRow<'r, <$db as SqlxDatabase>::Row> + Send + Unpin,
        {
            let args = <$db as Backend>::arguments(values)?;
            sqlx::query_as_with::<$db, T, _>(AssertSqlSafe(sql.to_owned()), args)
                .fetch_all(conn)
                .await
        }

        async fn fetch_optional_pool<T>(
            pool: &sqlx::Pool<$db>,
            sql: &str,
            values: &[Value],
        ) -> Result<Option<T>, sqlx::Error>
        where
            T: for<'r> FromRow<'r, <$db as SqlxDatabase>::Row> + Send + Unpin,
        {
            let args = <$db as Backend>::arguments(values)?;
            sqlx::query_as_with::<$db, T, _>(AssertSqlSafe(sql.to_owned()), args)
                .fetch_optional(pool)
                .await
        }

        async fn fetch_optional_conn<T>(
            conn: &mut <$db as SqlxDatabase>::Connection,
            sql: &str,
            values: &[Value],
        ) -> Result<Option<T>, sqlx::Error>
        where
            T: for<'r> FromRow<'r, <$db as SqlxDatabase>::Row> + Send + Unpin,
        {
            let args = <$db as Backend>::arguments(values)?;
            sqlx::query_as_with::<$db, T, _>(AssertSqlSafe(sql.to_owned()), args)
                .fetch_optional(conn)
                .await
        }

        async fn script_pool(pool: &sqlx::Pool<$db>, sql: &str) -> Result<(), sqlx::Error> {
            sqlx::raw_sql(AssertSqlSafe(sql.to_owned()))
                .execute(pool)
                .await?;
            Ok(())
        }

        async fn script_conn(
            conn: &mut <$db as SqlxDatabase>::Connection,
            sql: &str,
        ) -> Result<(), sqlx::Error> {
            sqlx::raw_sql(AssertSqlSafe(sql.to_owned()))
                .execute(conn)
                .await?;
            Ok(())
        }
    };
}

/// The outcome of a statement, from the driver's result type.
fn outcome<DB: Backend>(result: &DB::QueryResult) -> Outcome
where
    DB::Arguments: IntoArguments<DB>,
{
    Outcome {
        rows_affected: DB::rows_affected(result),
        insert_id: DB::insert_id(result),
    }
}

#[cfg(feature = "mysql")]
impl Backend for sqlx::MySql {
    fn arguments(values: &[Value]) -> Result<sqlx::mysql::MySqlArguments, sqlx::Error> {
        use sqlx::Arguments;
        let mut args = sqlx::mysql::MySqlArguments::default();
        for value in values {
            let added = match value {
                Value::Null => args.add(None::<i64>),
                Value::Bool(v) => args.add(*v),
                Value::Int(v) => args.add(*v),
                Value::UInt(v) => args.add(*v),
                Value::Float(v) => args.add(*v),
                Value::Text(v) => args.add(v.as_str()),
                Value::DateTime(v) => args.add(*v),
                Value::Date(v) => args.add(*v),
                Value::Bytes(v) => args.add(v.as_slice()),
            };
            added.map_err(sqlx::Error::Encode)?;
        }
        Ok(args)
    }

    fn rows_affected(result: &sqlx::mysql::MySqlQueryResult) -> u64 {
        result.rows_affected()
    }

    fn insert_id(result: &sqlx::mysql::MySqlQueryResult) -> Option<u64> {
        Some(result.last_insert_id())
    }

    common_impl!(sqlx::MySql);
}

#[cfg(feature = "postgres")]
impl Backend for sqlx::Postgres {
    fn arguments(values: &[Value]) -> Result<sqlx::postgres::PgArguments, sqlx::Error> {
        use sqlx::Arguments;
        let mut args = sqlx::postgres::PgArguments::default();
        for value in values {
            let added = match value {
                Value::Null => args.add(None::<i64>),
                Value::Bool(v) => args.add(*v),
                Value::Int(v) => args.add(*v),
                Value::UInt(v) => args.add(i64::try_from(*v).map_err(|_| too_large(*v))?),
                Value::Float(v) => args.add(*v),
                Value::Text(v) => args.add(v.as_str()),
                Value::DateTime(v) => args.add(*v),
                Value::Date(v) => args.add(*v),
                Value::Bytes(v) => args.add(v.as_slice()),
            };
            added.map_err(sqlx::Error::Encode)?;
        }
        Ok(args)
    }

    fn rows_affected(result: &sqlx::postgres::PgQueryResult) -> u64 {
        result.rows_affected()
    }

    fn insert_id(_: &sqlx::postgres::PgQueryResult) -> Option<u64> {
        None
    }

    common_impl!(sqlx::Postgres);
}

#[cfg(feature = "sqlite")]
impl Backend for sqlx::Sqlite {
    fn arguments(values: &[Value]) -> Result<sqlx::sqlite::SqliteArguments, sqlx::Error> {
        use sqlx::Arguments;
        let mut args = sqlx::sqlite::SqliteArguments::default();
        for value in values {
            let added = match value {
                Value::Null => args.add(None::<i64>),
                Value::Bool(v) => args.add(*v),
                Value::Int(v) => args.add(*v),
                Value::UInt(v) => args.add(i64::try_from(*v).map_err(|_| too_large(*v))?),
                Value::Float(v) => args.add(*v),
                Value::Text(v) => args.add(v.clone()),
                Value::DateTime(v) => args.add(*v),
                Value::Date(v) => args.add(*v),
                Value::Bytes(v) => args.add(v.clone()),
            };
            added.map_err(sqlx::Error::Encode)?;
        }
        Ok(args)
    }

    fn rows_affected(result: &sqlx::sqlite::SqliteQueryResult) -> u64 {
        result.rows_affected()
    }

    fn insert_id(result: &sqlx::sqlite::SqliteQueryResult) -> Option<u64> {
        u64::try_from(result.last_insert_rowid()).ok()
    }

    common_impl!(sqlx::Sqlite);
}

/// Calls the method of the driver that the target is on. `$pool` and `$conn` name the trait
/// methods for a pool and for a transaction's connection; `$args` are their arguments after the
/// pool or connection. Each driver's arm is a call on its concrete type.
macro_rules! on_target {
    ([$pool:ident, $conn:ident $(::<$gen:ty>)?] $target:expr, $($arg:expr),*) => {
        match $target {
            Target::Pool(pool) => match pool {
                #[cfg(feature = "mysql")]
                Pool::MySql(p) => <sqlx::MySql as Backend>::$pool$(::<$gen>)?(p, $($arg),*).await,
                #[cfg(feature = "postgres")]
                Pool::Postgres(p) => <sqlx::Postgres as Backend>::$pool$(::<$gen>)?(p, $($arg),*).await,
                #[cfg(feature = "sqlite")]
                Pool::Sqlite(p) => <sqlx::Sqlite as Backend>::$pool$(::<$gen>)?(p, $($arg),*).await,
            },
            Target::Tx(tx) => match tx {
                #[cfg(feature = "mysql")]
                TxInner::MySql(t) => <sqlx::MySql as Backend>::$conn$(::<$gen>)?(&mut **t, $($arg),*).await,
                #[cfg(feature = "postgres")]
                TxInner::Postgres(t) => <sqlx::Postgres as Backend>::$conn$(::<$gen>)?(&mut **t, $($arg),*).await,
                #[cfg(feature = "sqlite")]
                TxInner::Sqlite(t) => <sqlx::Sqlite as Backend>::$conn$(::<$gen>)?(&mut **t, $($arg),*).await,
            },
        }
    };
}

/// Runs a statement with bound values and reports the rows it changed and its insert id.
pub(crate) async fn execute(
    target: Target<'_>,
    sql: &str,
    values: &[Value],
) -> Result<Outcome, sqlx::Error> {
    on_target!([execute_pool, execute_conn] target, sql, values)
}

/// Runs a `SELECT` and decodes every row as `T`.
pub(crate) async fn fetch_all<T>(
    target: Target<'_>,
    sql: &str,
    values: &[Value],
) -> Result<Vec<T>, sqlx::Error>
where
    T: DriverRows + Send + Unpin,
{
    on_target!([fetch_all_pool, fetch_all_conn ::<T>] target, sql, values)
}

/// Runs a `SELECT` and decodes its first row as `T`, or `None` when no row comes back.
pub(crate) async fn fetch_optional<T>(
    target: Target<'_>,
    sql: &str,
    values: &[Value],
) -> Result<Option<T>, sqlx::Error>
where
    T: DriverRows + Send + Unpin,
{
    on_target!([fetch_optional_pool, fetch_optional_conn ::<T>] target, sql, values)
}

/// Runs a `SELECT` that returns one value in its first column and decodes it as `T`. It is an
/// error when no row comes back, as with sqlx's `fetch_one`.
pub(crate) async fn scalar<T>(
    target: Target<'_>,
    sql: &str,
    values: &[Value],
) -> Result<T, sqlx::Error>
where
    (T,): DriverRows + Send + Unpin,
{
    let row: Option<(T,)> = fetch_optional(target, sql, values).await?;
    row.map(|(value,)| value).ok_or(sqlx::Error::RowNotFound)
}

/// Runs a script of statements with no bound values, such as a migration's `up.sql`.
pub(crate) async fn run_script(target: Target<'_>, sql: &str) -> Result<(), sqlx::Error> {
    on_target!([script_pool, script_conn] target, sql)
}
