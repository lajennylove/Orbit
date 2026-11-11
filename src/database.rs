use std::time::Duration;

use sqlx::Database as SqlxDatabase;
#[cfg(feature = "mysql")]
use sqlx::MySql;
#[cfg(feature = "mysql")]
use sqlx::MySqlPool;
#[cfg(feature = "postgres")]
use sqlx::PgPool;
#[cfg(feature = "postgres")]
use sqlx::Postgres;
#[cfg(feature = "sqlite")]
use sqlx::Sqlite;
#[cfg(feature = "sqlite")]
use sqlx::SqlitePool;

use crate::transaction::Target;
use crate::{Driver, Error, Transaction};

/// The settings for the connection pool, given to `Database::connect_with`. `Default` gives the
/// settings that `Database::connect` uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolOptions {
    /// The most connections the pool opens at once. Further requests wait for one to be released.
    pub max_connections: u32,
    /// How long a request waits for a connection before it fails.
    pub acquire_timeout: Duration,
    /// The connections the pool keeps open while idle.
    pub min_connections: u32,
}

impl Default for PoolOptions {
    fn default() -> Self {
        Self {
            max_connections: 5,
            acquire_timeout: Duration::from_secs(10),
            min_connections: 0,
        }
    }
}

/// The pool of the database that the URL named. One variant per enabled driver.
#[derive(Debug)]
#[doc(hidden)]
pub enum Pool {
    #[cfg(feature = "mysql")]
    MySql(MySqlPool),
    #[cfg(feature = "postgres")]
    Postgres(PgPool),
    #[cfg(feature = "sqlite")]
    Sqlite(SqlitePool),
}

impl Pool {
    pub(crate) fn driver(&self) -> Driver {
        match self {
            #[cfg(feature = "mysql")]
            Self::MySql(_) => Driver::MySql,
            #[cfg(feature = "postgres")]
            Self::Postgres(_) => Driver::Postgres,
            #[cfg(feature = "sqlite")]
            Self::Sqlite(_) => Driver::Sqlite,
        }
    }
}

/// The connection pool for one database. Clone-free: share it behind an `Arc` or in Rocket's state.
///
/// The driver comes from the URL's scheme: `mysql://` (or `mariadb://`), `postgres://` (or
/// `postgresql://`) and `sqlite:`. A scheme whose feature is not enabled is an error.
#[derive(Debug)]
pub struct Database {
    pub(crate) pool: Pool,
}

/// The driver that a URL's scheme names, if its feature is enabled.
fn driver_for(url: &str) -> Result<Driver, Error> {
    let scheme = url.split_once(':').map_or(url, |(scheme, _)| scheme);
    #[cfg(feature = "mysql")]
    if <MySql as SqlxDatabase>::URL_SCHEMES.contains(&scheme) {
        return Ok(Driver::MySql);
    }
    #[cfg(feature = "postgres")]
    if <Postgres as SqlxDatabase>::URL_SCHEMES.contains(&scheme) {
        return Ok(Driver::Postgres);
    }
    #[cfg(feature = "sqlite")]
    if <Sqlite as SqlxDatabase>::URL_SCHEMES.contains(&scheme) {
        return Ok(Driver::Sqlite);
    }
    Err(Error::UnsupportedDriver {
        scheme: scheme.to_owned(),
    })
}

impl Database {
    /// Connects to the database at `url`, such as `mysql://user:password@host/database`,
    /// `postgres://user:password@host/database` or `sqlite:path/to/file.db`.
    pub async fn connect(url: &str) -> Result<Self, Error> {
        Self::connect_with(url, PoolOptions::default()).await
    }

    /// Connects to the database at `url` with the pool settings in `options`. `connect` uses
    /// `PoolOptions::default()`.
    pub async fn connect_with(url: &str, options: PoolOptions) -> Result<Self, Error> {
        let pool = match driver_for(url)? {
            #[cfg(feature = "mysql")]
            Driver::MySql => Pool::MySql(options.apply::<MySql>().connect(url).await?),
            #[cfg(feature = "postgres")]
            Driver::Postgres => Pool::Postgres(options.apply::<Postgres>().connect(url).await?),
            #[cfg(feature = "sqlite")]
            Driver::Sqlite => Pool::Sqlite(options.apply::<Sqlite>().connect(url).await?),
        };
        Ok(Self { pool })
    }

    /// The database this pool talks to.
    pub fn driver(&self) -> Driver {
        self.pool.driver()
    }

    /// The MySQL pool, for queries that Orbit does not wrap yet.
    ///
    /// # Panics
    ///
    /// Panics when the database is not MySQL. Check `driver()` first, or use `postgres_pool` or
    /// `sqlite_pool`.
    #[cfg(feature = "mysql")]
    pub fn pool(&self) -> &MySqlPool {
        match &self.pool {
            Pool::MySql(pool) => pool,
            #[cfg(any(feature = "postgres", feature = "sqlite"))]
            _ => panic!(
                "`pool` is for MySQL; this database is {}",
                self.driver().name()
            ),
        }
    }

    /// The PostgreSQL pool, for queries that Orbit does not wrap yet. `None` for other databases.
    #[cfg(feature = "postgres")]
    pub fn postgres_pool(&self) -> Option<&PgPool> {
        match &self.pool {
            Pool::Postgres(pool) => Some(pool),
            #[cfg(any(feature = "mysql", feature = "sqlite"))]
            _ => None,
        }
    }

    /// The SQLite pool, for queries that Orbit does not wrap yet. `None` for other databases.
    #[cfg(feature = "sqlite")]
    pub fn sqlite_pool(&self) -> Option<&SqlitePool> {
        match &self.pool {
            Pool::Sqlite(pool) => Some(pool),
            #[cfg(any(feature = "mysql", feature = "postgres"))]
            _ => None,
        }
    }

    /// Checks that the database answers.
    pub async fn ping(&self) -> Result<(), Error> {
        crate::backend::execute(Target::Pool(&self.pool), "SELECT 1", &[]).await?;
        Ok(())
    }

    /// Runs `f` in a transaction and returns what it returns. The transaction commits when `f`
    /// returns `Ok` and rolls back when it returns `Err`. A panic in `f` rolls it back too, since
    /// nothing commits before `f` returns. Queries that must run inside the transaction take it
    /// as `&mut *tx`.
    ///
    /// The closure is an async closure: `db.transaction(async |tx| { ... Ok(value) }).await`.
    pub async fn transaction<T, E, F>(&self, f: F) -> Result<T, E>
    where
        E: From<Error>,
        F: AsyncFnOnce(&mut Transaction) -> Result<T, E>,
    {
        let mut tx = Transaction::begin(&self.pool).await?;
        match f(&mut tx).await {
            Ok(value) => {
                tx.commit().await?;
                Ok(value)
            }
            Err(error) => {
                // The error from `f` is the one to report, so a failed rollback is ignored.
                let _ = tx.rollback().await;
                Err(error)
            }
        }
    }

    /// Closes every connection in the pool.
    pub async fn close(&self) {
        match &self.pool {
            #[cfg(feature = "mysql")]
            Pool::MySql(pool) => pool.close().await,
            #[cfg(feature = "postgres")]
            Pool::Postgres(pool) => pool.close().await,
            #[cfg(feature = "sqlite")]
            Pool::Sqlite(pool) => pool.close().await,
        }
    }
}

impl PoolOptions {
    /// The sqlx pool options with these settings, for driver `DB`.
    fn apply<DB: SqlxDatabase>(self) -> sqlx::pool::PoolOptions<DB> {
        sqlx::pool::PoolOptions::<DB>::new()
            .max_connections(self.max_connections)
            .min_connections(self.min_connections)
            .acquire_timeout(self.acquire_timeout)
    }
}
