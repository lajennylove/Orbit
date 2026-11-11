//! Migrations are SQL folders that change the schema. Each folder is named `<timestamp>_<name>`
//! and holds `up.sql` and `down.sql`. The applied ones are recorded in Diesel's
//! `__diesel_schema_migrations` table with a batch number, and a rollback reverts the last batch.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::Path;
use std::time::Duration;

use sqlx::AssertSqlSafe;

use crate::{Database, Error};

/// One migration: its name, and the SQL that applies it (`up`) and reverts it (`down`).
#[derive(Debug, Clone)]
pub struct Migration {
    /// The folder name, such as `20261108000000_create_users`.
    pub name: String,
    /// The contents of `up.sql`.
    pub up: String,
    /// The contents of `down.sql`.
    pub down: String,
}

impl Migration {
    /// A migration from its name and SQL, for code that builds migrations itself.
    pub fn new(name: impl Into<String>, up: impl Into<String>, down: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            up: up.into(),
            down: down.into(),
        }
    }
}

/// Whether a migration has run. `Applied` carries the batch it ran in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Applied { batch: i64 },
    Pending,
}

/// One line of [`Migrator::status`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub name: String,
    pub state: State,
}

/// The migrations of one app, sorted by name. Build it with [`Migrator::from_dir`] or
/// [`migrations!`](crate::migrations).
#[derive(Debug, Clone)]
pub struct Migrator {
    migrations: Vec<Migration>,
    lock_timeout: Duration,
}

/// The MySQL lock that serialises `run` and `rollback` across processes is named after the
/// database, so two databases on one server do not wait for each other.
const LOCK_PREFIX: &str = "orbit_migrations";

/// How long `run` and `rollback` wait for another process's lock before giving up.
const DEFAULT_LOCK_TIMEOUT: Duration = Duration::from_secs(10);

impl Migrator {
    /// Sorts the migrations by name. A name must start with a 14-digit timestamp and an
    /// underscore, as `20261108000000_create_users` does. Two migrations with the same name, or
    /// the same timestamp, are an error.
    pub fn new(mut migrations: Vec<Migration>) -> Result<Self, Error> {
        for migration in &migrations {
            if !has_timestamp(&migration.name) {
                return Err(Error::InvalidMigrationName {
                    name: migration.name.clone(),
                });
            }
        }
        migrations.sort_by(|a, b| a.name.cmp(&b.name));
        for pair in migrations.windows(2) {
            if pair[0].name == pair[1].name {
                return Err(Error::DuplicateMigration {
                    name: pair[0].name.clone(),
                });
            }
            if timestamp(&pair[0].name) == timestamp(&pair[1].name) {
                return Err(Error::DuplicateTimestamp {
                    first: pair[0].name.clone(),
                    second: pair[1].name.clone(),
                });
            }
        }
        Ok(Self {
            migrations,
            lock_timeout: DEFAULT_LOCK_TIMEOUT,
        })
    }

    /// Sets how long `run` and `rollback` wait for another process to finish its migrations.
    pub fn with_lock_timeout(mut self, timeout: Duration) -> Self {
        self.lock_timeout = timeout;
        self
    }

    /// The migrations in name order.
    pub fn migrations(&self) -> &[Migration] {
        &self.migrations
    }

    /// Reads every folder in `dir` as a migration, such as `migrations/20261108000000_create_users`.
    /// Each folder needs `up.sql` and `down.sql`. Hidden entries are skipped.
    pub fn from_dir(dir: impl AsRef<Path>) -> Result<Self, Error> {
        let dir = dir.as_ref();
        let entries = std::fs::read_dir(dir).map_err(|source| Error::Read {
            path: dir.to_path_buf(),
            source,
        })?;
        let mut migrations = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|source| Error::Read {
                path: dir.to_path_buf(),
                source,
            })?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = entry.path();
            if name.starts_with('.') || !path.is_dir() {
                continue;
            }
            let up = read_file(&path.join("up.sql"))?;
            let down = read_file(&path.join("down.sql"))?;
            migrations.push(Migration { name, up, down });
        }
        Self::new(migrations)
    }

    /// Applies every pending migration, in name order, as one new batch. Each migration runs in
    /// its own transaction. The run stops at the first failure, and the error names it. The
    /// migrations before it stay applied in this batch.
    ///
    /// MySQL does not roll back DDL. A failing migration can leave the DDL it ran before the
    /// failure in place; only its DML is rolled back. Returns the names that were applied.
    ///
    /// Only one process runs migrations at a time. A second process waits up to the lock timeout,
    /// then fails with [`Error::MigrationsLocked`].
    pub async fn run(&self, db: &Database) -> Result<Vec<String>, Error> {
        self.with_lock(db, self.run_unlocked(db)).await
    }

    async fn run_unlocked(&self, db: &Database) -> Result<Vec<String>, Error> {
        ensure_table(db).await?;
        let applied = applied(db).await?;
        let pending: Vec<&Migration> = self
            .migrations
            .iter()
            .filter(|m| !applied.contains_key(timestamp(&m.name)))
            .collect();
        if pending.is_empty() {
            return Ok(Vec::new());
        }

        let batch: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(batch), 0) + 1 FROM __diesel_schema_migrations",
        )
        .fetch_one(db.pool())
        .await?;
        let mut names = Vec::with_capacity(pending.len());
        for migration in pending {
            let mut tx = db.pool().begin().await?;
            sqlx::raw_sql(AssertSqlSafe(migration.up.as_str()))
                .execute(&mut *tx)
                .await
                .map_err(|source| Error::MigrationFailed {
                    name: migration.name.clone(),
                    source,
                })?;
            sqlx::query("INSERT INTO __diesel_schema_migrations (version, batch) VALUES (?, ?)")
                .bind(timestamp(&migration.name))
                .bind(batch)
                .execute(&mut *tx)
                .await
                .map_err(|source| Error::MigrationFailed {
                    name: migration.name.clone(),
                    source,
                })?;
            tx.commit().await?;
            names.push(migration.name.clone());
        }
        Ok(names)
    }

    /// Lists every migration in this migrator as applied (with its batch) or pending.
    /// Applied migrations without files here are not listed.
    pub async fn status(&self, db: &Database) -> Result<Vec<Status>, Error> {
        ensure_table(db).await?;
        let applied = applied(db).await?;
        Ok(self
            .migrations
            .iter()
            .map(|m| Status {
                name: m.name.clone(),
                state: match applied.get(timestamp(&m.name)) {
                    Some(&batch) => State::Applied { batch },
                    None => State::Pending,
                },
            })
            .collect())
    }

    /// Reverts the last batch, running each `down.sql` in reverse name order, each in its own
    /// transaction. It checks that every migration in the batch has files here before it reverts
    /// anything. Returns the names that were reverted, or nothing when no migration has run.
    ///
    /// Batch 0 holds the history that Diesel recorded, so a rollback that reaches batch 0 reverts
    /// those migrations and removes their rows from Diesel's table too.
    ///
    /// As with [`run`](Self::run), DDL in a `down.sql` is not rolled back by MySQL.
    pub async fn rollback(&self, db: &Database) -> Result<Vec<String>, Error> {
        self.with_lock(db, self.rollback_unlocked(db)).await
    }

    async fn rollback_unlocked(&self, db: &Database) -> Result<Vec<String>, Error> {
        ensure_table(db).await?;
        let last: Option<i64> =
            sqlx::query_scalar("SELECT MAX(batch) FROM __diesel_schema_migrations")
                .fetch_one(db.pool())
                .await?;
        let Some(batch) = last else {
            return Ok(Vec::new());
        };
        let versions: Vec<String> = sqlx::query_scalar(
            "SELECT version FROM __diesel_schema_migrations WHERE batch = ? ORDER BY version DESC",
        )
        .bind(batch)
        .fetch_all(db.pool())
        .await?;

        let mut to_revert = Vec::with_capacity(versions.len());
        for version in &versions {
            let migration = self
                .migrations
                .iter()
                .find(|m| timestamp(&m.name) == version.as_str())
                .ok_or_else(|| Error::UnknownMigration {
                    name: version.clone(),
                })?;
            to_revert.push(migration);
        }
        let names = to_revert.iter().map(|m| m.name.clone()).collect();
        for migration in to_revert {
            let mut tx = db.pool().begin().await?;
            sqlx::raw_sql(AssertSqlSafe(migration.down.as_str()))
                .execute(&mut *tx)
                .await
                .map_err(|source| Error::RollbackFailed {
                    name: migration.name.clone(),
                    source,
                })?;
            sqlx::query("DELETE FROM __diesel_schema_migrations WHERE version = ?")
                .bind(timestamp(&migration.name))
                .execute(&mut *tx)
                .await
                .map_err(|source| Error::RollbackFailed {
                    name: migration.name.clone(),
                    source,
                })?;
            tx.commit().await?;
        }
        Ok(names)
    }
}

impl Migrator {
    /// Brings Diesel's history into this migrator's table. Orbit and Diesel share
    /// `__diesel_schema_migrations`, so the rows Diesel wrote are already the applied history, in
    /// batch 0. This call does what is still needed for them: it adds the `batch` column to a
    /// table that has only Diesel's columns. No `up.sql` runs, and no row is added or changed.
    /// It returns nothing, because there is nothing new to record. It is safe to call again.
    #[deprecated(
        since = "0.1.1",
        note = "not needed any more: run, status and rollback upgrade the migrations table \
                themselves. This returns an empty list and will be removed in a later release"
    )]
    pub async fn import_diesel_history(&self, db: &Database) -> Result<Vec<String>, Error> {
        self.with_lock(db, import_unlocked(db)).await
    }

    /// The versions in the migrations table that have no migration in this migrator, in order.
    /// They include versions that Orbit recorded when a folder has since been removed. Empty when
    /// the table does not exist. Only reads the table.
    pub async fn unmatched_diesel_versions(&self, db: &Database) -> Result<Vec<String>, Error> {
        let Some(versions) = recorded_versions(db).await? else {
            return Ok(Vec::new());
        };
        let mut unmatched: Vec<String> = versions
            .into_iter()
            .filter(|version| {
                !self
                    .migrations
                    .iter()
                    .any(|m| timestamp(&m.name) == version.as_str())
            })
            .collect();
        unmatched.sort();
        Ok(unmatched)
    }
}

/// Embeds migrations in the binary at build time. `dir` is relative to the crate root, and each
/// name is a folder in it with `up.sql` and `down.sql`. The names are listed by hand, and a
/// missing file is a build error. Returns a `Result<Migrator, Error>`.
///
/// ```ignore
/// let migrator = orbit_orm::migrations!("migrations", "20261108000000_create_users")?;
/// ```
#[macro_export]
macro_rules! migrations {
    ($dir:literal, $($name:literal),* $(,)?) => {
        $crate::Migrator::new(vec![
            $(
                $crate::Migration::new(
                    $name,
                    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/", $dir, "/", $name, "/up.sql")),
                    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/", $dir, "/", $name, "/down.sql")),
                ),
            )*
        ])
    };
}

impl Migrator {
    /// Runs `body` while holding the migrations lock. The lock is a MySQL named lock, held by one
    /// pooled connection, so it lasts for the whole run and is released when the run ends.
    async fn with_lock<T>(
        &self,
        db: &Database,
        body: impl Future<Output = Result<T, Error>>,
    ) -> Result<T, Error> {
        let mut conn = db.pool().acquire().await?;
        let got: Option<i64> = sqlx::query_scalar("SELECT GET_LOCK(?, ?)")
            .bind(lock_name(db))
            .bind(self.lock_timeout.as_secs() as i64)
            .fetch_one(&mut *conn)
            .await?;
        if got != Some(1) {
            return Err(Error::MigrationsLocked {
                seconds: self.lock_timeout.as_secs(),
            });
        }

        let result = body.await;
        let released = sqlx::query("SELECT RELEASE_LOCK(?)")
            .bind(lock_name(db))
            .execute(&mut *conn)
            .await;
        if released.is_err() {
            // Closing the connection drops the lock on the server, so it cannot stay in the pool.
            drop(conn.detach());
        }
        let value = result?;
        released?;
        Ok(value)
    }
}

/// The body of [`Migrator::import_diesel_history`], run under the lock.
async fn import_unlocked(db: &Database) -> Result<Vec<String>, Error> {
    ensure_table(db).await?;
    Ok(Vec::new())
}

/// Whether the name starts with a 14-digit timestamp and an underscore, followed by a name.
fn has_timestamp(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes.len() > 15 && bytes[..14].iter().all(u8::is_ascii_digit) && bytes[14] == b'_'
}

/// The timestamp of a name that passed `has_timestamp`. It is the `version` in the table.
fn timestamp(name: &str) -> &str {
    &name[..14]
}

/// Creates `__diesel_schema_migrations` in Diesel's layout if it does not exist, with Orbit's
/// `batch` column. A table that Diesel created has `version` and `run_on` only. For that table
/// the `batch` column is added, and its existing rows get batch 0, so they count as one batch
/// that was imported from Diesel. Nothing is dropped or renamed. Safe to run on every call.
async fn ensure_table(db: &Database) -> Result<(), Error> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS __diesel_schema_migrations (\
         version VARCHAR(50) NOT NULL PRIMARY KEY, \
         run_on TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP, \
         batch INT NOT NULL DEFAULT 0)",
    )
    .execute(db.pool())
    .await?;
    if has_batch_column(db).await? {
        return Ok(());
    }
    let added = sqlx::query(
        "ALTER TABLE __diesel_schema_migrations ADD COLUMN batch INT NOT NULL DEFAULT 0",
    )
    .execute(db.pool())
    .await;
    match added {
        Ok(_) => Ok(()),
        // Another process added the column after the check above.
        Err(err) if is_duplicate_column(&err) => Ok(()),
        Err(err) => Err(err.into()),
    }
}

/// Whether the migrations table has a `batch` column.
async fn has_batch_column(db: &Database) -> Result<bool, Error> {
    let found: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM information_schema.columns \
         WHERE table_schema = DATABASE() AND table_name = '__diesel_schema_migrations' \
         AND column_name = 'batch'",
    )
    .fetch_one(db.pool())
    .await?;
    Ok(found > 0)
}

/// MySQL error 1060, `ER_DUP_FIELDNAME`: the column already exists.
fn is_duplicate_column(err: &sqlx::Error) -> bool {
    err.as_database_error()
        .and_then(|db_err| db_err.code())
        .is_some_and(|code| code == "1060")
}

/// The applied migrations, keyed by version, with their batch numbers.
async fn applied(db: &Database) -> Result<HashMap<String, i64>, Error> {
    let rows: Vec<(String, i64)> =
        sqlx::query_as("SELECT version, batch FROM __diesel_schema_migrations")
            .fetch_all(db.pool())
            .await?;
    Ok(rows.into_iter().collect())
}

/// The versions in `__diesel_schema_migrations`, or `None` when the table does not exist. Only
/// reads the table.
async fn recorded_versions(db: &Database) -> Result<Option<HashSet<String>>, Error> {
    let found: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM information_schema.tables \
         WHERE table_schema = DATABASE() AND table_name = '__diesel_schema_migrations'",
    )
    .fetch_one(db.pool())
    .await?;
    if found == 0 {
        return Ok(None);
    }
    let versions: Vec<String> =
        sqlx::query_scalar("SELECT version FROM __diesel_schema_migrations")
            .fetch_all(db.pool())
            .await?;
    Ok(Some(versions.into_iter().collect()))
}

/// Reads a migration file. The error names the file.
pub(crate) fn read_file(path: &Path) -> Result<String, Error> {
    std::fs::read_to_string(path).map_err(|source| Error::Read {
        path: path.to_path_buf(),
        source,
    })
}

/// The lock name for `db`: the prefix and the database name.
fn lock_name(db: &Database) -> String {
    format!(
        "{LOCK_PREFIX}:{}",
        db.pool().connect_options().get_database().unwrap_or("")
    )
}
