use sqlx::{MySql, MySqlConnection, MySqlPool};

use crate::{Database, Error};

/// An open transaction, given to the closure of `Database::transaction`. Pass it to queries as
/// `&mut *tx`. It commits or rolls back when that closure returns.
pub struct Transaction {
    inner: sqlx::Transaction<'static, MySql>,
}

impl Transaction {
    pub(crate) async fn begin(pool: &MySqlPool) -> Result<Self, Error> {
        Ok(Self {
            inner: pool.begin().await?,
        })
    }

    pub(crate) async fn commit(self) -> Result<(), Error> {
        Ok(self.inner.commit().await?)
    }

    pub(crate) async fn rollback(self) -> Result<(), Error> {
        Ok(self.inner.rollback().await?)
    }

    /// The connection of this transaction, for queries that Orbit does not wrap yet.
    pub fn connection(&mut self) -> &mut MySqlConnection {
        &mut self.inner
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
    Pool(&'a MySqlPool),
    Transaction(&'a mut MySqlConnection),
}

impl<'a> Executor<'a> for &'a Database {
    fn target(self) -> Target<'a> {
        Target::Pool(self.pool())
    }
}

impl<'a> Executor<'a> for &'a mut Transaction {
    fn target(self) -> Target<'a> {
        Target::Transaction(self.connection())
    }
}
