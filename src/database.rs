use std::time::Duration;

use sqlx::MySqlPool;
use sqlx::mysql::MySqlPoolOptions;

use crate::Error;

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

    /// Closes every connection in the pool.
    pub async fn close(&self) {
        self.pool.close().await;
    }
}
