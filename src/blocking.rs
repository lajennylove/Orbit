//! A synchronous API over the same queries. Each call runs the async query on a current-thread
//! Tokio runtime that the `Database` owns.
//!
//! Call it from threads that may block, such as inside `tokio::task::spawn_blocking`. On a thread
//! that runs async code, a blocking call returns `Error::BlockingInRuntime`. Tokio prints its own
//! panic message to stderr before that, since it refuses to block there. Dropping a `Database` on
//! such a thread is safe.
//!
//! The builder methods are the async ones. Call `blocking()` before a terminal method:
//!
//! ```ignore
//! let post = Post::query().where_eq("id", 1).blocking().find_or_fail(&db, 1)?;
//! ```

use std::cell::Cell;
use std::future::Future;
use std::panic::{self, AssertUnwindSafe};
use std::sync::Arc;

use sqlx::MySqlPool;
use tokio::runtime::{Builder, Runtime};

use crate::{Error, Model, Value};

/// A connection pool with its own runtime, for synchronous code.
#[derive(Debug)]
pub struct Database {
    inner: crate::Database,
    /// Only taken in `drop`, which shuts the runtime down in the background.
    runtime: Option<Arc<Runtime>>,
}

impl Database {
    /// Connects to the database at `url`, such as `mysql://user:password@host/database`.
    pub fn connect(url: &str) -> Result<Self, Error> {
        let runtime = Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| Error::Database(sqlx::Error::Io(e)))?;
        match block_on(&runtime, crate::Database::connect(url)).and_then(|connected| connected) {
            Ok(inner) => Ok(Self {
                inner,
                runtime: Some(Arc::new(runtime)),
            }),
            Err(error) => {
                // Dropping a runtime on an async thread panics, so shut it down in the background.
                runtime.shutdown_background();
                Err(error)
            }
        }
    }

    /// The underlying pool, for queries that Orbit does not wrap yet. Run them with `run`.
    pub fn pool(&self) -> &MySqlPool {
        self.inner.pool()
    }

    /// Runs an async future on this database's runtime and returns its output. It is for sqlx
    /// queries that Orbit does not wrap, such as `db.run(sqlx::query("...").execute(db.pool()))`.
    pub fn run<F: Future>(&self, future: F) -> Result<F::Output, Error> {
        block_on(self.runtime(), future)
    }

    /// Checks that the database answers.
    pub fn ping(&self) -> Result<(), Error> {
        self.run(self.inner.ping())?
    }

    /// Runs `f` in a transaction and returns what it returns. The transaction commits when `f`
    /// returns `Ok` and rolls back when it returns `Err`. A panic in `f` rolls it back too.
    pub fn transaction<T, E, F>(&self, f: F) -> Result<T, E>
    where
        E: From<Error>,
        F: FnOnce(&mut Transaction) -> Result<T, E>,
    {
        let inner = self.run(crate::Transaction::begin(self.inner.pool()))??;
        let mut tx = Transaction {
            inner,
            runtime: Arc::clone(self.runtime_arc()),
        };
        match f(&mut tx) {
            Ok(value) => {
                self.run(tx.inner.commit())??;
                Ok(value)
            }
            Err(error) => {
                // The error from `f` is the one to report, so a failed rollback is ignored.
                let _ = self.run(tx.inner.rollback());
                Err(error)
            }
        }
    }

    /// Closes every connection in the pool.
    pub fn close(&self) -> Result<(), Error> {
        self.run(self.inner.close())
    }

    fn runtime(&self) -> &Runtime {
        self.runtime_arc()
    }

    fn runtime_arc(&self) -> &Arc<Runtime> {
        self.runtime
            .as_ref()
            .expect("the runtime is only taken in drop")
    }
}

impl Drop for Database {
    fn drop(&mut self) {
        // A transaction borrows the database, so none is left and the runtime has one owner.
        // Dropping a runtime blocks, which panics on an async thread. Shutting it down in the
        // background does not.
        if let Some(runtime) = self.runtime.take().and_then(|rt| Arc::try_unwrap(rt).ok()) {
            runtime.shutdown_background();
        }
    }
}

/// A transaction given to the closure of `Database::transaction`. Pass it to queries as `tx`.
/// It shares the database's runtime, so its queries block the same way.
pub struct Transaction {
    inner: crate::Transaction,
    runtime: Arc<Runtime>,
}

/// Where a blocking query runs: a `&Database`, or a `&mut Transaction`. The terminal methods
/// take either.
pub trait Executor<'a> {
    #[doc(hidden)]
    type Target: crate::Executor<'a>;

    #[doc(hidden)]
    fn parts(self) -> (&'a Runtime, Self::Target);
}

impl<'a> Executor<'a> for &'a Database {
    type Target = &'a crate::Database;

    fn parts(self) -> (&'a Runtime, &'a crate::Database) {
        (self.runtime(), &self.inner)
    }
}

impl<'a> Executor<'a> for &'a mut Transaction {
    type Target = &'a mut crate::Transaction;

    fn parts(self) -> (&'a Runtime, &'a mut crate::Transaction) {
        let Transaction { inner, runtime } = self;
        (runtime, inner)
    }
}

/// A query for the blocking terminal methods. Made by `blocking()` on the async builder.
pub struct Query<M: Model> {
    inner: crate::Query<M>,
}

impl<M: Model> crate::Query<M> {
    /// Turns the query into one whose terminal methods block the calling thread.
    pub fn blocking(self) -> Query<M> {
        Query { inner: self }
    }
}

impl<M: Model> Query<M> {
    /// Returns every row that matches. See `crate::Query::get`.
    pub fn get<'a>(self, db: impl Executor<'a>) -> Result<Vec<M>, Error> {
        let (runtime, target) = db.parts();
        block_on(runtime, self.inner.get(target))?
    }

    /// Returns the first row. See `crate::Query::first`.
    pub fn first<'a>(self, db: impl Executor<'a>) -> Result<Option<M>, Error> {
        let (runtime, target) = db.parts();
        block_on(runtime, self.inner.first(target))?
    }

    /// Returns the first row, or an error when none matches. See `crate::Query::first_or_fail`.
    pub fn first_or_fail<'a>(self, db: impl Executor<'a>) -> Result<M, Error> {
        let (runtime, target) = db.parts();
        block_on(runtime, self.inner.first_or_fail(target))?
    }

    /// Returns the row with the given primary key, or `None`. See `crate::Query::find`.
    pub fn find<'a>(self, db: impl Executor<'a>, id: impl Into<Value>) -> Result<Option<M>, Error> {
        let (runtime, target) = db.parts();
        block_on(runtime, self.inner.find(target, id))?
    }

    /// Returns the row with the given primary key, or an error. See `crate::Query::find_or_fail`.
    pub fn find_or_fail<'a>(self, db: impl Executor<'a>, id: impl Into<Value>) -> Result<M, Error> {
        let (runtime, target) = db.parts();
        block_on(runtime, self.inner.find_or_fail(target, id))?
    }

    /// Counts the rows that match. See `crate::Query::count`.
    pub fn count<'a>(self, db: impl Executor<'a>) -> Result<i64, Error> {
        let (runtime, target) = db.parts();
        block_on(runtime, self.inner.count(target))?
    }

    /// Returns whether any row matches. See `crate::Query::exists`.
    pub fn exists<'a>(self, db: impl Executor<'a>) -> Result<bool, Error> {
        let (runtime, target) = db.parts();
        block_on(runtime, self.inner.exists(target))?
    }

    /// Inserts one row and returns its auto-increment id. See `crate::Query::insert`.
    pub fn insert<'a>(self, db: impl Executor<'a>, values: &[(&str, Value)]) -> Result<u64, Error> {
        let (runtime, target) = db.parts();
        block_on(runtime, self.inner.insert(target, values))?
    }

    /// Replaces the row with the same unique or primary key, or inserts it. See
    /// `crate::Query::replace`, which describes Diesel's `replace_into` semantics.
    pub fn replace<'a>(
        self,
        db: impl Executor<'a>,
        values: &[(&str, Value)],
    ) -> Result<u64, Error> {
        let (runtime, target) = db.parts();
        block_on(runtime, self.inner.replace(target, values))?
    }

    /// Sets the columns on every matching row. See `crate::Query::update`.
    pub fn update<'a>(self, db: impl Executor<'a>, values: &[(&str, Value)]) -> Result<u64, Error> {
        let (runtime, target) = db.parts();
        block_on(runtime, self.inner.update(target, values))?
    }

    /// Deletes every matching row. See `crate::Query::delete`.
    pub fn delete<'a>(self, db: impl Executor<'a>) -> Result<u64, Error> {
        let (runtime, target) = db.parts();
        block_on(runtime, self.inner.delete(target))?
    }
}

/// Runs `future` on `runtime`. Tokio refuses to start a runtime on a thread that already runs
/// one, such as an async task, and panics before it polls the future. That panic is turned into
/// `Error::BlockingInRuntime`. A thread inside `spawn_blocking` may block, so it is allowed.
fn block_on<F: Future>(runtime: &Runtime, future: F) -> Result<F::Output, Error> {
    let polled = Cell::new(false);
    let future = async {
        polled.set(true);
        future.await
    };
    match panic::catch_unwind(AssertUnwindSafe(|| runtime.block_on(future))) {
        Ok(output) => Ok(output),
        Err(_) if !polled.get() => Err(Error::BlockingInRuntime),
        Err(payload) => panic::resume_unwind(payload),
    }
}
