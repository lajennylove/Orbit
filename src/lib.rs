//! Orbit is an ORM for Rocket.rs apps. The first database is MySQL and MariaDB; PostgreSQL
//! and SQLite follow once the MySQL path passes the same tests.
//!
//! Orbit uses `sqlx` for the connection pool and the wire protocol, and adds the ORM layer
//! on top of it.

#[cfg(feature = "blocking")]
pub mod blocking;
mod database;
mod error;
mod migrate;
mod model;
mod query;
mod transaction;
mod value;

/// Re-exported for the `OrbitModel` derive, so a model does not need its own `sqlx` dependency.
#[doc(hidden)]
pub use sqlx as __sqlx;

pub use database::Database;
pub use error::Error;
pub use migrate::{Migration, Migrator, State, Status};
pub use model::Model;
#[cfg(feature = "derive")]
pub use orbit_macros::OrbitModel;
pub use query::{Direction, Operator, Query};
pub use transaction::{Executor, Transaction};
pub use value::Value;
