//! Orbit is an ORM for Rocket.rs apps. The first database is MySQL and MariaDB; PostgreSQL
//! and SQLite follow once the MySQL path passes the same tests.
//!
//! Orbit uses `sqlx` for the connection pool and the wire protocol, and adds the ORM layer
//! on top of it.

mod database;
mod error;

pub use database::Database;
pub use error::Error;
