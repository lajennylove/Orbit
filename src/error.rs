use std::path::PathBuf;

use sqlx::error::ErrorKind;
use sqlx::mysql::MySqlDatabaseError;

/// MySQL error 1205: a lock was not released within `innodb_lock_wait_timeout`.
const ER_LOCK_WAIT_TIMEOUT: u16 = 1205;
/// MySQL error 1213: the database rolled back the transaction to break a deadlock.
const ER_LOCK_DEADLOCK: u16 = 1213;

/// Any error from Orbit. Driver errors that have a variant here are classified; the rest come
/// through unchanged as `Database`.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The database rejected a connection, a query, or a statement, and no other variant fits.
    #[error(transparent)]
    Database(sqlx::Error),

    /// An insert or update broke a unique index or primary key. `index` is the index name when
    /// the server reports it.
    #[error("unique index `{}` was violated", .index.as_deref().unwrap_or("unknown"))]
    UniqueViolation { index: Option<String> },

    /// A row refers to a missing parent, or a parent is still referenced. `constraint` is the
    /// foreign key name when the server reports it.
    #[error("foreign key `{}` was violated", .constraint.as_deref().unwrap_or("unknown"))]
    ForeignKeyViolation { constraint: Option<String> },

    /// The database rolled back the whole transaction to break a deadlock. Run it again.
    #[error("deadlock found; the transaction was rolled back")]
    Deadlock,

    /// A lock was not released in time. Only the statement failed; the transaction is still open.
    #[error("lock wait timeout; the statement was rolled back")]
    LockWaitTimeout,

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

impl From<sqlx::Error> for Error {
    /// Classifies the driver error by its kind and MySQL error number. Anything else is kept as
    /// `Database`.
    fn from(error: sqlx::Error) -> Self {
        if let sqlx::Error::Database(db) = &error {
            match db.kind() {
                ErrorKind::UniqueViolation => {
                    return Self::UniqueViolation {
                        index: duplicate_key(db.message()),
                    };
                }
                ErrorKind::ForeignKeyViolation => {
                    return Self::ForeignKeyViolation {
                        constraint: foreign_key(db.message()),
                    };
                }
                _ => {}
            }
            // The driver maps lock errors to `Other`, so they are matched by number.
            if let Some(mysql) = db.try_downcast_ref::<MySqlDatabaseError>() {
                match mysql.number() {
                    ER_LOCK_DEADLOCK => return Self::Deadlock,
                    ER_LOCK_WAIT_TIMEOUT => return Self::LockWaitTimeout,
                    _ => {}
                }
            }
        }
        Self::Database(error)
    }
}

/// The index in a "Duplicate entry 'value' for key 'index'" message. MySQL 8.0.19 and later
/// prefix the table name, as in `table.index`, which is removed here.
fn duplicate_key(message: &str) -> Option<String> {
    const MARK: &str = "for key '";
    let start = message.rfind(MARK)? + MARK.len();
    let key = message[start..].strip_suffix('\'')?;
    Some(key.rsplit('.').next().unwrap_or(key).to_owned())
}

/// The constraint in a "... CONSTRAINT `name` FOREIGN KEY ..." message.
fn foreign_key(message: &str) -> Option<String> {
    const MARK: &str = "CONSTRAINT `";
    let start = message.find(MARK)? + MARK.len();
    let rest = &message[start..];
    Some(rest[..rest.find('`')?].to_owned())
}

#[cfg(test)]
mod tests {
    use super::{duplicate_key, foreign_key};

    #[test]
    fn duplicate_key_reads_the_index_name() {
        assert_eq!(
            duplicate_key("Duplicate entry 'a@example.com' for key 'users_email_unique'"),
            Some("users_email_unique".to_owned())
        );
    }

    #[test]
    fn duplicate_key_drops_the_table_prefix_of_mysql_8() {
        assert_eq!(
            duplicate_key("Duplicate entry '1' for key 'users.PRIMARY'"),
            Some("PRIMARY".to_owned())
        );
    }

    #[test]
    fn duplicate_key_ignores_a_value_that_looks_like_the_marker() {
        assert_eq!(
            duplicate_key("Duplicate entry 'x' for key 'y' for key 'real_index'"),
            Some("real_index".to_owned())
        );
    }

    #[test]
    fn duplicate_key_is_none_without_a_key() {
        assert_eq!(
            duplicate_key("Can't write, because of unique constraint 'x'"),
            None
        );
    }

    #[test]
    fn foreign_key_reads_the_constraint_name() {
        assert_eq!(
            foreign_key(
                "Cannot add or update a child row: a foreign key constraint fails (`db`.`child`, \
                 CONSTRAINT `fk_child_parent` FOREIGN KEY (`parent_id`) REFERENCES `parent` (`id`))"
            ),
            Some("fk_child_parent".to_owned())
        );
        assert_eq!(foreign_key("something else"), None);
    }
}
