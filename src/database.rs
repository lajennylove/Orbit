use std::time::Duration;

use sqlx::MySqlPool;
use sqlx::mysql::MySqlPoolOptions;

use crate::{Error, Transaction};

/// The connection pool for one database. Clone-free: share it behind an `Arc` or in Rocket's state.
#[derive(Debug)]
pub struct Database {
    pool: MySqlPool,
}

impl Database {
    /// Connects to the database at `url`, such as `mysql://user:password@host/database`.
    pub async fn connect(url: &str) -> Result<Self, Error> {
        let pool = MySqlPoolOptions::new()
            .max_connections(5)
            .acquire_timeout(Duration::from_secs(10))
            .connect(url)
            .await?;
        Ok(Self { pool })
    }

    /// The underlying pool, for queries that Orbit does not wrap yet.
    pub fn pool(&self) -> &MySqlPool {
        &self.pool
    }

    /// Checks that the database answers.
    pub async fn ping(&self) -> Result<(), Error> {
        sqlx::query("SELECT 1").execute(&self.pool).await?;
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
        self.pool.close().await;
    }
}
