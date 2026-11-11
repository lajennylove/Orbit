//! Migrations are SQL folders that change the schema. Each folder is named `<timestamp>_<name>`
//! and holds `up.sql` and `down.sql`. The applied ones are recorded in Diesel's
//! `__diesel_schema_migrations` table with a batch number, and a rollback reverts the last batch.
//! The table has the same name and columns on every database.

#[cfg(feature = "sqlite")]
use std::collections::BTreeSet;
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::Path;
#[cfg(feature = "sqlite")]
use std::sync::{Mutex, PoisonError};
use std::time::Duration;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use std::time::Instant;

use crate::backend;
use crate::database::Pool;
use crate::transaction::{Target, Transaction};
use crate::{Database, Driver, Error, Value};

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

/// The lock that serialises `run` and `rollback` across processes is named after the database,
/// so two databases on one server do not wait for each other.
const LOCK_PREFIX: &str = "orbit_migrations";

/// How long `run` and `rollback` wait for another process's lock before giving up.
const DEFAULT_LOCK_TIMEOUT: Duration = Duration::from_secs(10);

/// How often a waiting lock is tried again, where the database has no blocking lock call.
#[cfg(any(feature = "postgres", feature = "sqlite"))]
const LOCK_POLL: Duration = Duration::from_millis(50);

/// The table that records the applied migrations, on every database.
const TABLE: &str = "__diesel_schema_migrations";

/// The migrations that are held in this process, by lock name. Used by SQLite, which has no
/// named lock. See `sqlite_lock`.
#[cfg(feature = "sqlite")]
static SQLITE_HELD: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

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
    /// MySQL and SQLite do not roll back DDL, and PostgreSQL does. On MySQL a failing migration can
    /// leave the DDL it ran before the failure in place; only its DML is rolled back. Returns the
    /// names that were applied.
    ///
    /// Only one process runs migrations at a time. A second process waits up to the lock timeout,
    /// then fails with [`Error::MigrationsLocked`]. The lock is held differently on each database;
    /// see `docs/laravel-differences.md`.
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

        let max: Option<i32> = backend::scalar(
            Target::Pool(&db.pool),
            &format!("SELECT MAX(batch) FROM {TABLE}"),
            &[],
        )
        .await?;
        let batch = i64::from(max.unwrap_or(0)) + 1;
        let mut names = Vec::with_capacity(pending.len());
        for migration in pending {
            let mut tx = Transaction::begin(&db.pool).await?;
            backend::run_script(Target::Tx(&mut tx.inner), &migration.up)
                .await
                .map_err(|source| Error::MigrationFailed {
                    name: migration.name.clone(),
                    source,
                })?;
            let (sql, values) = prepared(
                db.driver(),
                &format!("INSERT INTO {TABLE} (version, batch) VALUES (?, ?)"),
                vec![
                    Value::Text(timestamp(&migration.name).to_owned()),
                    Value::Int(batch),
                ],
            );
            backend::execute(Target::Tx(&mut tx.inner), &sql, &values)
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
    /// As with [`run`](Self::run), DDL in a `down.sql` is only rolled back on PostgreSQL.
    pub async fn rollback(&self, db: &Database) -> Result<Vec<String>, Error> {
        self.with_lock(db, self.rollback_unlocked(db)).await
    }

    async fn rollback_unlocked(&self, db: &Database) -> Result<Vec<String>, Error> {
        ensure_table(db).await?;
        let last: Option<i32> = backend::scalar(
            Target::Pool(&db.pool),
            &format!("SELECT MAX(batch) FROM {TABLE}"),
            &[],
        )
        .await?;
        let Some(batch) = last else {
            return Ok(Vec::new());
        };
        let (sql, values) = prepared(
            db.driver(),
            &format!("SELECT version FROM {TABLE} WHERE batch = ? ORDER BY version DESC"),
            vec![Value::Int(i64::from(batch))],
        );
        let versions: Vec<(String,)> =
            backend::fetch_all(Target::Pool(&db.pool), &sql, &values).await?;

        let mut to_revert = Vec::with_capacity(versions.len());
        for (version,) in &versions {
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
            let mut tx = Transaction::begin(&db.pool).await?;
            backend::run_script(Target::Tx(&mut tx.inner), &migration.down)
                .await
                .map_err(|source| Error::RollbackFailed {
                    name: migration.name.clone(),
                    source,
                })?;
            let (sql, values) = prepared(
                db.driver(),
                &format!("DELETE FROM {TABLE} WHERE version = ?"),
                vec![Value::Text(timestamp(&migration.name).to_owned())],
            );
            backend::execute(Target::Tx(&mut tx.inner), &sql, &values)
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
    /// Runs `body` while holding the migrations lock for the database. See each driver's lock
    /// function for how it is held.
    async fn with_lock<T>(
        &self,
        db: &Database,
        body: impl Future<Output = Result<T, Error>>,
    ) -> Result<T, Error> {
        match &db.pool {
            #[cfg(feature = "mysql")]
            Pool::MySql(pool) => mysql_lock(pool, self.lock_timeout, body).await,
            #[cfg(feature = "postgres")]
            Pool::Postgres(pool) => postgres_lock(pool, self.lock_timeout, body).await,
            #[cfg(feature = "sqlite")]
            Pool::Sqlite(pool) => sqlite_lock(pool, self.lock_timeout, body).await,
        }
    }
}

/// MySQL: a named lock from `GET_LOCK`, held by one pooled connection, so it lasts for the whole
/// run and is released when the run ends.
#[cfg(feature = "mysql")]
async fn mysql_lock<T>(
    pool: &sqlx::MySqlPool,
    timeout: Duration,
    body: impl Future<Output = Result<T, Error>>,
) -> Result<T, Error> {
    let name = format!(
        "{LOCK_PREFIX}:{}",
        pool.connect_options().get_database().unwrap_or("")
    );
    let mut conn = pool.acquire().await?;
    let got: Option<i64> = sqlx::query_scalar("SELECT GET_LOCK(?, ?)")
        .bind(&name)
        .bind(timeout.as_secs() as i64)
        .fetch_one(&mut *conn)
        .await?;
    if got != Some(1) {
        return Err(Error::MigrationsLocked {
            seconds: timeout.as_secs(),
        });
    }

    let result = body.await;
    let released = sqlx::query("SELECT RELEASE_LOCK(?)")
        .bind(&name)
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

/// PostgreSQL: a session advisory lock, keyed by a stable hash of the database name. It is held by
/// one pooled connection, and waiting is a `pg_try_advisory_lock` retried until the timeout.
#[cfg(feature = "postgres")]
async fn postgres_lock<T>(
    pool: &sqlx::PgPool,
    timeout: Duration,
    body: impl Future<Output = Result<T, Error>>,
) -> Result<T, Error> {
    let name = format!(
        "{LOCK_PREFIX}:{}",
        pool.connect_options().get_database().unwrap_or("")
    );
    let key = advisory_key(&name);
    let mut conn = pool.acquire().await?;
    let deadline = Instant::now() + timeout;
    loop {
        let got: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1)")
            .bind(key)
            .fetch_one(&mut *conn)
            .await?;
        if got {
            break;
        }
        if Instant::now() >= deadline {
            return Err(Error::MigrationsLocked {
                seconds: timeout.as_secs(),
            });
        }
        tokio::time::sleep(LOCK_POLL).await;
    }

    let result = body.await;
    let released = sqlx::query_scalar::<_, bool>("SELECT pg_advisory_unlock($1)")
        .bind(key)
        .fetch_one(&mut *conn)
        .await;
    if released.is_err() {
        // Closing the connection ends the session, which drops its advisory locks.
        drop(conn.detach());
    }
    let value = result?;
    released?;
    Ok(value)
}

/// SQLite has no named lock, so this lock is held by the process: the name is kept in a set
/// while the run lasts, and a second run in the same process waits for it to leave the set.
/// Two processes are not held apart by this lock. SQLite serialises their writes, so they can
/// both reach the migrations table, and one of them may fail on a duplicate version. See
/// `docs/laravel-differences.md`.
#[cfg(feature = "sqlite")]
async fn sqlite_lock<T>(
    pool: &sqlx::SqlitePool,
    timeout: Duration,
    body: impl Future<Output = Result<T, Error>>,
) -> Result<T, Error> {
    let name = format!(
        "{LOCK_PREFIX}:{}",
        pool.connect_options().get_filename().display()
    );
    let deadline = Instant::now() + timeout;
    loop {
        let taken = SQLITE_HELD
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(name.clone());
        if taken {
            break;
        }
        if Instant::now() >= deadline {
            return Err(Error::MigrationsLocked {
                seconds: timeout.as_secs(),
            });
        }
        tokio::time::sleep(LOCK_POLL).await;
    }

    let result = body.await;
    SQLITE_HELD
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(&name);
    result
}

/// A stable 64-bit key for a PostgreSQL advisory lock: FNV-1a of the lock name. It does not change
/// between releases, so two versions of the app agree on the lock.
#[cfg(feature = "postgres")]
fn advisory_key(name: &str) -> i64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in name.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash as i64
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

/// The statement and values for the driver: `?` placeholders are numbered on PostgreSQL.
fn prepared(driver: Driver, sql: &str, values: Vec<Value>) -> (String, Vec<Value>) {
    driver.prepare(sql, &values)
}

/// Picks the SQL for the driver. Each argument is the same statement written for one database.
#[cfg_attr(
    not(all(feature = "mysql", feature = "postgres", feature = "sqlite")),
    allow(unused_variables)
)]
fn pick<'a>(driver: Driver, mysql: &'a str, postgres: &'a str, sqlite: &'a str) -> &'a str {
    match driver {
        #[cfg(feature = "mysql")]
        Driver::MySql => mysql,
        #[cfg(feature = "postgres")]
        Driver::Postgres => postgres,
        #[cfg(feature = "sqlite")]
        Driver::Sqlite => sqlite,
    }
}

/// Creates the migrations table in Diesel's layout if it does not exist, with Orbit's `batch`
/// column. A table that Diesel created has `version` and `run_on` only. For that table the `batch`
/// column is added, and its existing rows get batch 0, so they count as one batch that was
/// imported from Diesel. Nothing is dropped or renamed. Safe to run on every call.
async fn ensure_table(db: &Database) -> Result<(), Error> {
    let driver = db.driver();
    let create = pick(
        driver,
        "CREATE TABLE IF NOT EXISTS __diesel_schema_migrations (\
         version VARCHAR(50) NOT NULL PRIMARY KEY, \
         run_on TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP, \
         batch INT NOT NULL DEFAULT 0)",
        "CREATE TABLE IF NOT EXISTS __diesel_schema_migrations (\
         version VARCHAR(50) NOT NULL PRIMARY KEY, \
         run_on TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP, \
         batch INT NOT NULL DEFAULT 0)",
        "CREATE TABLE IF NOT EXISTS __diesel_schema_migrations (\
         version TEXT NOT NULL PRIMARY KEY, \
         run_on TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP, \
         batch INTEGER NOT NULL DEFAULT 0)",
    );
    backend::execute(Target::Pool(&db.pool), create, &[]).await?;
    if has_batch_column(db).await? {
        return Ok(());
    }
    let added = backend::execute(
        Target::Pool(&db.pool),
        "ALTER TABLE __diesel_schema_migrations ADD COLUMN batch INT NOT NULL DEFAULT 0",
        &[],
    )
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
    let sql = pick(
        db.driver(),
        "SELECT COUNT(*) FROM information_schema.columns \
         WHERE table_schema = DATABASE() AND table_name = '__diesel_schema_migrations' \
         AND column_name = 'batch'",
        "SELECT COUNT(*) FROM information_schema.columns \
         WHERE table_schema = current_schema() AND table_name = '__diesel_schema_migrations' \
         AND column_name = 'batch'",
        "SELECT COUNT(*) FROM pragma_table_info('__diesel_schema_migrations') \
         WHERE name = 'batch'",
    );
    let found: i64 = backend::scalar(Target::Pool(&db.pool), sql, &[]).await?;
    Ok(found > 0)
}

/// Whether the error says that the column already exists: MySQL 1060, PostgreSQL 42701, or
/// SQLite's "duplicate column name".
fn is_duplicate_column(err: &sqlx::Error) -> bool {
    err.as_database_error().is_some_and(|db_err| {
        matches!(db_err.code().as_deref(), Some("1060" | "42701"))
            || db_err.message().contains("duplicate column name")
    })
}

/// The applied migrations, keyed by version, with their batch numbers.
async fn applied(db: &Database) -> Result<HashMap<String, i64>, Error> {
    let rows: Vec<(String, i32)> = backend::fetch_all(
        Target::Pool(&db.pool),
        &format!("SELECT version, batch FROM {TABLE}"),
        &[],
    )
    .await?;
    Ok(rows
        .into_iter()
        .map(|(version, batch)| (version, i64::from(batch)))
        .collect())
}

/// The versions in the migrations table, or `None` when the table does not exist. Only reads the
/// table.
async fn recorded_versions(db: &Database) -> Result<Option<HashSet<String>>, Error> {
    let sql = pick(
        db.driver(),
        "SELECT COUNT(*) FROM information_schema.tables \
         WHERE table_schema = DATABASE() AND table_name = '__diesel_schema_migrations'",
        "SELECT COUNT(*) FROM information_schema.tables \
         WHERE table_schema = current_schema() AND table_name = '__diesel_schema_migrations'",
        "SELECT COUNT(*) FROM sqlite_master \
         WHERE type = 'table' AND name = '__diesel_schema_migrations'",
    );
    let found: i64 = backend::scalar(Target::Pool(&db.pool), sql, &[]).await?;
    if found == 0 {
        return Ok(None);
    }
    let versions: Vec<(String,)> = backend::fetch_all(
        Target::Pool(&db.pool),
        &format!("SELECT version FROM {TABLE}"),
        &[],
    )
    .await?;
    Ok(Some(versions.into_iter().map(|(v,)| v).collect()))
}

/// Reads a migration file. The error names the file.
pub(crate) fn read_file(path: &Path) -> Result<String, Error> {
    std::fs::read_to_string(path).map_err(|source| Error::Read {
        path: path.to_path_buf(),
        source,
    })
}
