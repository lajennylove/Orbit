use std::path::PathBuf;

/// Any error from Orbit. Database errors come from the driver unchanged.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The database rejected a connection, a query, or a statement.
    #[error(transparent)]
    Database(#[from] sqlx::Error),

    /// A file could not be read, such as a migration's `up.sql`.
    #[error("could not read {}: {source}", path.display())]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },

    /// Two migrations have the same name.
    #[error("migration `{name}` is defined more than once")]
    DuplicateMigration { name: String },

    /// A migration name does not start with a 14-digit timestamp and an underscore.
    #[error("migration `{name}` must start with a 14-digit timestamp, such as 20261108000000_name")]
    InvalidMigrationName { name: String },

    /// Two migrations share a timestamp, so their order is not defined.
    #[error("migrations `{first}` and `{second}` have the same timestamp")]
    DuplicateTimestamp { first: String, second: String },

    /// Another process is running migrations and did not finish within the lock timeout.
    #[error("another process is running migrations; gave up after {seconds} seconds")]
    MigrationsLocked { seconds: u64 },

    /// A migration's `up.sql` failed. The run stopped there; earlier migrations stay applied.
    #[error("migration `{name}` failed, so the run stopped: {source}")]
    MigrationFailed { name: String, source: sqlx::Error },

    /// A migration's `down.sql` failed. The rollback stopped there.
    #[error("migration `{name}` failed to roll back: {source}")]
    RollbackFailed { name: String, source: sqlx::Error },

    /// A migration is recorded as applied, but this migrator has no files for it, so it cannot
    /// be rolled back.
    #[error("migration `{name}` is applied but has no files here, so it cannot be rolled back")]
    UnknownMigration { name: String },

    /// A table or column name is empty or contains a backtick.
    #[error("invalid identifier `{0}`: names must not be empty or contain a backtick")]
    InvalidIdentifier(String),

    /// A column is not in the model's `FILLABLE` list.
    #[error("column `{column}` is not fillable on table `{table}`")]
    NotFillable { table: &'static str, column: String },

    /// A query that must find a row found none.
    #[error("no query results for table `{table}`")]
    NotFound { table: &'static str },
}
