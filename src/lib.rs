//! Orbit is an ORM for Rocket.rs apps. It supports MySQL and MariaDB, PostgreSQL and SQLite,
//! each behind a Cargo feature of the same name. The database is chosen by the URL.
//!
//! Orbit uses `sqlx` for the connection pool and the wire protocol, and adds the ORM layer
//! on top of it.

mod backend;
#[cfg(feature = "blocking")]
pub mod blocking;
mod database;
mod driver;
mod error;
mod migrate;
mod model;
mod query;
pub mod raw;
mod transaction;
mod value;

/// Re-exported for the `OrbitModel` derive, so a model does not need its own `sqlx` dependency.
#[doc(hidden)]
pub use sqlx as __sqlx;

pub use database::{Database, PoolOptions};
pub use driver::Driver;
#[doc(hidden)]
pub use driver::rows::DriverRows;
pub use error::Error;
pub use migrate::{Migration, Migrator, State, Status};
pub use model::Model;
#[cfg(feature = "derive")]
pub use orbit_macros::OrbitModel;
pub use query::{Direction, Operator, Query};
pub use transaction::{Executor, Transaction};
pub use value::Value;
