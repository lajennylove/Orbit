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
}
