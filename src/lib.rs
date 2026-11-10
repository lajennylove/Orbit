//! Orbit is an ORM for Rocket.rs apps. The first database is MySQL and MariaDB; PostgreSQL
//! and SQLite follow once the MySQL path passes the same tests.
//!
//! Orbit uses `sqlx` for the connection pool and the wire protocol, and adds the ORM layer
//! on top of it.

mod database;
mod error;
mod model;
mod query;
mod value;

pub use database::Database;
pub use error::Error;
pub use model::Model;
pub use query::{Direction, Operator, Query};
pub use value::Value;
