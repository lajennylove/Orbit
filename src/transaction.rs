#[cfg(feature = "mysql")]
use sqlx::MySql;
#[cfg(feature = "mysql")]
use sqlx::MySqlConnection;
#[cfg(feature = "postgres")]
use sqlx::Postgres;
#[cfg(feature = "sqlite")]
use sqlx::Sqlite;

use crate::database::Pool;
use crate::{Database, Driver, Error};

/// An open transaction, given to the closure of `Database::transaction`. Pass it to queries as
/// `&mut *tx`. It commits or rolls back when that closure returns.
pub struct Transaction {
    pub(crate) inner: TxInner,
}

/// The open transaction on the connection of one driver.
#[doc(hidden)]
pub enum TxInner {
    #[cfg(feature = "mysql")]
    MySql(sqlx::Transaction<'static, MySql>),
    #[cfg(feature = "postgres")]
    Postgres(sqlx::Transaction<'static, Postgres>),
    #[cfg(feature = "sqlite")]
    Sqlite(sqlx::Transaction<'static, Sqlite>),
}

impl TxInner {
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

impl Transaction {
    pub(crate) async fn begin(pool: &Pool) -> Result<Self, Error> {
        let inner = match pool {
            #[cfg(feature = "mysql")]
            Pool::MySql(pool) => TxInner::MySql(pool.begin().await?),
            #[cfg(feature = "postgres")]
            Pool::Postgres(pool) => TxInner::Postgres(pool.begin().await?),
            #[cfg(feature = "sqlite")]
            Pool::Sqlite(pool) => TxInner::Sqlite(pool.begin().await?),
        };
        Ok(Self { inner })
    }

    pub(crate) async fn commit(self) -> Result<(), Error> {
        match self.inner {
            #[cfg(feature = "mysql")]
            TxInner::MySql(tx) => Ok(tx.commit().await?),
            #[cfg(feature = "postgres")]
            TxInner::Postgres(tx) => Ok(tx.commit().await?),
            #[cfg(feature = "sqlite")]
            TxInner::Sqlite(tx) => Ok(tx.commit().await?),
        }
    }

    pub(crate) async fn rollback(self) -> Result<(), Error> {
        match self.inner {
            #[cfg(feature = "mysql")]
            TxInner::MySql(tx) => Ok(tx.rollback().await?),
            #[cfg(feature = "postgres")]
            TxInner::Postgres(tx) => Ok(tx.rollback().await?),
            #[cfg(feature = "sqlite")]
            TxInner::Sqlite(tx) => Ok(tx.rollback().await?),
        }
    }

    /// The database this transaction runs on.
    pub fn driver(&self) -> Driver {
        self.inner.driver()
    }

    /// The MySQL connection of this transaction, for queries that Orbit does not wrap yet.
    ///
    /// # Panics
    ///
    /// Panics when the database is not MySQL. Check `driver()` first.
    #[cfg(feature = "mysql")]
    pub fn connection(&mut self) -> &mut MySqlConnection {
        #[cfg(any(feature = "postgres", feature = "sqlite"))]
        let driver = self.inner.driver();
        match &mut self.inner {
            TxInner::MySql(tx) => tx,
            #[cfg(any(feature = "postgres", feature = "sqlite"))]
            _ => panic!(
                "`connection` is for MySQL; this transaction is on {}",
                driver.name()
            ),
        }
    }
}

/// Where a query runs: a `&Database`, on a pooled connection, or a `&mut Transaction`, on the
/// transaction's connection. Terminal methods such as `get` take either.
pub trait Executor<'a> {
    #[doc(hidden)]
    fn target(self) -> Target<'a>;
}

/// The connection a query runs on.
#[doc(hidden)]
pub enum Target<'a> {
    Pool(&'a Pool),
    Tx(&'a mut TxInner),
}

impl Target<'_> {
    /// The database the query runs on.
    pub(crate) fn driver(&self) -> Driver {
        match self {
            Self::Pool(pool) => pool.driver(),
            Self::Tx(tx) => tx.driver(),
        }
    }
}

impl<'a> Executor<'a> for &'a Database {
    fn target(self) -> Target<'a> {
        Target::Pool(&self.pool)
    }
}

impl<'a> Executor<'a> for &'a mut Transaction {
    fn target(self) -> Target<'a> {
        Target::Tx(&mut self.inner)
    }
}
