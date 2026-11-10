use std::path::Path;
use std::time::Duration;

use sqlx::mysql::MySqlPoolOptions;
use sqlx::{AssertSqlSafe, MySqlPool};

use crate::Error;
use crate::migrate::read_file;

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

    /// Runs every statement in the file at `path` in one transaction. Nothing is recorded, so use
    /// it for one-off SQL, not for migrations. MySQL commits DDL on its own, so a failing file can
    /// leave the DDL it ran before the failure.
    pub async fn run_file(&self, path: impl AsRef<Path>) -> Result<(), Error> {
        let sql = read_file(path.as_ref())?;
        let mut tx = self.pool.begin().await?;
        sqlx::raw_sql(AssertSqlSafe(sql.as_str()))
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Closes every connection in the pool.
    pub async fn close(&self) {
        self.pool.close().await;
    }
}
