//! Migrations are SQL folders that change the schema. Each folder is named `<timestamp>_<name>`
//! and holds `up.sql` and `down.sql`. Like Laravel's migrator, the applied ones are recorded in a
//! `migrations` table with a batch number, and a rollback reverts the last batch.

use std::collections::HashMap;
use std::path::Path;

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
}

impl Migrator {
    /// Sorts the migrations by name. Two migrations with the same name are an error.
    pub fn new(mut migrations: Vec<Migration>) -> Result<Self, Error> {
        migrations.sort_by(|a, b| a.name.cmp(&b.name));
        for pair in migrations.windows(2) {
            if pair[0].name == pair[1].name {
                return Err(Error::DuplicateMigration {
                    name: pair[0].name.clone(),
                });
            }
        }
        Ok(Self { migrations })
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
    pub async fn run(&self, db: &Database) -> Result<Vec<String>, Error> {
        ensure_table(db).await?;
        let applied = applied(db).await?;
        let pending: Vec<&Migration> = self
            .migrations
            .iter()
            .filter(|m| !applied.contains_key(&m.name))
            .collect();
        if pending.is_empty() {
            return Ok(Vec::new());
        }

        let batch: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(batch), 0) + 1 FROM migrations")
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
            sqlx::query("INSERT INTO migrations (migration, batch) VALUES (?, ?)")
                .bind(&migration.name)
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
                state: match applied.get(&m.name) {
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
    /// As with [`run`](Self::run), DDL in a `down.sql` is not rolled back by MySQL.
    pub async fn rollback(&self, db: &Database) -> Result<Vec<String>, Error> {
        ensure_table(db).await?;
        let last: Option<i64> = sqlx::query_scalar("SELECT MAX(batch) FROM migrations")
            .fetch_one(db.pool())
            .await?;
        let Some(batch) = last else {
            return Ok(Vec::new());
        };
        let names: Vec<String> = sqlx::query_scalar(
            "SELECT migration FROM migrations WHERE batch = ? ORDER BY migration DESC",
        )
        .bind(batch)
        .fetch_all(db.pool())
        .await?;

        let mut to_revert = Vec::with_capacity(names.len());
        for name in &names {
            let migration = self
                .migrations
                .iter()
                .find(|m| &m.name == name)
                .ok_or_else(|| Error::UnknownMigration { name: name.clone() })?;
            to_revert.push(migration);
        }
        for migration in to_revert {
            let mut tx = db.pool().begin().await?;
            sqlx::raw_sql(AssertSqlSafe(migration.down.as_str()))
                .execute(&mut *tx)
                .await
                .map_err(|source| Error::RollbackFailed {
                    name: migration.name.clone(),
                    source,
                })?;
            sqlx::query("DELETE FROM migrations WHERE migration = ?")
                .bind(&migration.name)
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

/// Creates the `migrations` table if it does not exist yet.
async fn ensure_table(db: &Database) -> Result<(), Error> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS migrations (\
         migration VARCHAR(255) NOT NULL PRIMARY KEY, \
         batch INT NOT NULL)",
    )
    .execute(db.pool())
    .await?;
    Ok(())
}

/// The recorded migrations, with their batch numbers.
async fn applied(db: &Database) -> Result<HashMap<String, i64>, Error> {
    let rows: Vec<(String, i64)> = sqlx::query_as("SELECT migration, batch FROM migrations")
        .fetch_all(db.pool())
        .await?;
    Ok(rows.into_iter().collect())
}

/// Reads a migration file. The error names the file.
pub(crate) fn read_file(path: &Path) -> Result<String, Error> {
    std::fs::read_to_string(path).map_err(|source| Error::Read {
        path: path.to_path_buf(),
        source,
    })
}
