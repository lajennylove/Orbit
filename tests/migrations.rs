//! Runs migrations against a real database: MySQL or MariaDB, PostgreSQL or SQLite. Set
//! `ORBIT_DATABASE_URL` to a test database, such as `mysql://user@127.0.0.1:3306/orbit_test`. The
//! database tests are skipped with a notice when it is not set. They share the
//! `__diesel_schema_migrations` table, so they take a lock and reset the tables first.
//!
//! The fixture files are written in MySQL syntax. `load` translates each one with `common::sql`
//! before it builds the migrator, so the same files run on every database.

mod common;

use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::sync::{Mutex, MutexGuard};

use orbit_orm::{
    Database, Driver, Error, Migration, Migrator, State, Status, Value, migrations, raw,
};

const WIDGETS: &str = "20260101000000_create_widgets";
const GADGETS: &str = "20260102000000_create_gadgets";
const COLOR: &str = "20260103000000_add_widget_color";

static LOCK: Mutex<()> = Mutex::const_new(());

/// The tables the migration tests create. Each test drops them at the end, so none is left behind.
const MIGRATION_TABLES: &[&str] = &[
    "__diesel_schema_migrations",
    "migrations",
    "widgets",
    "gadgets",
    "gizmos",
];

fn fixture(set: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/migrations")
        .join(set)
}

/// Reads a fixture set like `Migrator::from_dir` does, but translates each `up.sql` and `down.sql`
/// from MySQL syntax into the syntax of `driver`.
fn load(driver: Driver, set: &str) -> Migrator {
    let dir = fixture(set);
    let mut folders: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("reads the fixture folder")
        .map(|entry| entry.expect("reads an entry").path())
        .filter(|path| path.is_dir())
        .collect();
    folders.sort();
    let migrations = folders
        .iter()
        .map(|folder| {
            let name = folder.file_name().unwrap().to_str().unwrap().to_owned();
            let up = std::fs::read_to_string(folder.join("up.sql")).expect("reads up.sql");
            let down = std::fs::read_to_string(folder.join("down.sql")).expect("reads down.sql");
            Migration::new(name, common::sql(driver, &up), common::sql(driver, &down))
        })
        .collect();
    Migrator::new(migrations).expect("the fixtures are valid")
}

/// The version of a migration name: its 14-digit timestamp prefix.
fn v(name: &str) -> String {
    name[..14].to_string()
}

/// Connects, drops the tables the tests use, and holds the lock until the test ends. Returns
/// `None` with a notice when `ORBIT_DATABASE_URL` is not set.
async fn setup() -> Option<(MutexGuard<'static, ()>, Database)> {
    let Some(url) = common::url() else {
        eprintln!("skipped: set ORBIT_DATABASE_URL to run the migration tests");
        return None;
    };
    let guard = LOCK.lock().await;
    let db = Database::connect(&url).await.expect("connects");
    common::drop_tables(
        &db,
        &[
            "__diesel_schema_migrations",
            "migrations",
            "widgets",
            "gadgets",
            "gizmos",
        ],
    )
    .await;
    Some((guard, db))
}

/// The rows of `__diesel_schema_migrations`, as `(version, batch)` in version order.
async fn recorded(db: &Database) -> Vec<(String, i64)> {
    raw::fetch_all(
        db,
        &format!(
            "SELECT version, CAST(batch AS {}) FROM __diesel_schema_migrations ORDER BY version",
            common::bigint_type(db.driver())
        ),
        &[],
    )
    .await
    .expect("reads the migrations table")
}

/// The columns of `__diesel_schema_migrations`, in table order.
async fn columns(db: &Database) -> Vec<String> {
    let rows: Vec<(String,)> = raw::fetch_all(
        db,
        &common::columns_sql(db.driver()),
        &[Value::from("__diesel_schema_migrations")],
    )
    .await
    .expect("reads the columns");
    rows.into_iter().map(|(name,)| name).collect()
}

async fn table_exists(db: &Database, table: &str) -> bool {
    let count: i64 = raw::fetch_scalar(
        db,
        common::table_count_sql(db.driver()),
        &[Value::from(table)],
    )
    .await
    .expect("reads the catalog");
    count > 0
}

fn applied(batch: i64) -> State {
    State::Applied { batch }
}

#[tokio::test]
async fn run_applies_all_in_order_and_records_one_batch() {
    let Some((_lock, db)) = setup().await else {
        return;
    };
    let migrator = load(db.driver(), "full");

    // COLOR alters widgets, so it can only run after WIDGETS: the order is checked by success.
    let names = migrator.run(&db).await.expect("runs every migration");
    assert_eq!(names, [WIDGETS, GADGETS, COLOR]);
    assert_eq!(
        recorded(&db).await,
        [(v(WIDGETS), 1), (v(GADGETS), 1), (v(COLOR), 1)]
    );
    assert!(table_exists(&db, "gadgets").await);
    common::drop_tables(&db, MIGRATION_TABLES).await;
}

#[tokio::test]
async fn a_second_run_applies_nothing() {
    let Some((_lock, db)) = setup().await else {
        return;
    };
    let migrator = load(db.driver(), "full");
    migrator.run(&db).await.expect("first run");

    let names = migrator.run(&db).await.expect("second run");
    assert!(names.is_empty());
    assert_eq!(recorded(&db).await.len(), 3);
    assert!(recorded(&db).await.iter().all(|(_, batch)| *batch == 1));
    common::drop_tables(&db, MIGRATION_TABLES).await;
}

#[tokio::test]
async fn status_lists_applied_and_pending() {
    let Some((_lock, db)) = setup().await else {
        return;
    };
    load(db.driver(), "basic")
        .run(&db)
        .await
        .expect("runs the first batch");

    let status = load(db.driver(), "full")
        .status(&db)
        .await
        .expect("reads the status");
    assert_eq!(
        status,
        [
            Status {
                name: WIDGETS.to_string(),
                state: applied(1),
            },
            Status {
                name: GADGETS.to_string(),
                state: State::Pending,
            },
            Status {
                name: COLOR.to_string(),
                state: State::Pending,
            },
        ]
    );
    common::drop_tables(&db, MIGRATION_TABLES).await;
}

#[tokio::test]
async fn rollback_reverts_the_last_batch_only() {
    let Some((_lock, db)) = setup().await else {
        return;
    };
    load(db.driver(), "basic")
        .run(&db)
        .await
        .expect("runs batch 1");
    let full = load(db.driver(), "full");
    full.run(&db).await.expect("runs batch 2");

    let reverted = full.rollback(&db).await.expect("rolls back batch 2");
    assert_eq!(reverted, [COLOR, GADGETS], "reverse order within the batch");
    assert_eq!(recorded(&db).await, [(v(WIDGETS), 1)]);
    assert!(
        !table_exists(&db, "gadgets").await,
        "down.sql dropped gadgets"
    );
    assert!(table_exists(&db, "widgets").await, "batch 1 stays applied");

    let reverted = full.rollback(&db).await.expect("rolls back batch 1");
    assert_eq!(reverted, [WIDGETS]);
    assert!(recorded(&db).await.is_empty());
    assert!(!table_exists(&db, "widgets").await);

    assert!(full.rollback(&db).await.expect("nothing left").is_empty());
    common::drop_tables(&db, MIGRATION_TABLES).await;
}

#[tokio::test]
async fn a_failing_migration_stops_the_run_and_names_it() {
    let Some((_lock, db)) = setup().await else {
        return;
    };
    let migrator = load(db.driver(), "broken");

    let error = migrator
        .run(&db)
        .await
        .expect_err("the second migration fails");
    match &error {
        Error::MigrationFailed { name, .. } => assert_eq!(name, "20260102000000_broken"),
        other => panic!("expected MigrationFailed, got {other:?}"),
    }
    assert!(error.to_string().contains("20260102000000_broken"));

    // The first migration stays applied in the batch. The third never runs.
    assert_eq!(recorded(&db).await, [(v(WIDGETS), 1)]);
    assert!(!table_exists(&db, "gadgets").await);
    if common::on_mysql(db.driver()) {
        // MySQL commits each CREATE TABLE as it runs, so the failed migration leaves gizmos behind.
        assert!(table_exists(&db, "gizmos").await);
    } else {
        // PostgreSQL and SQLite run each migration in a transaction, so the failure rolls back
        // the CREATE TABLE as well.
        assert!(!table_exists(&db, "gizmos").await);
    }
    common::drop_tables(&db, MIGRATION_TABLES).await;
}

/// The column types of `__diesel_schema_migrations`, as `(name, type, nullable, key)`. Each
/// database names the types its own way, so the query is written per database.
async fn column_types(db: &Database) -> Vec<(String, String, String, String)> {
    let sql = if common::on_mysql(db.driver()) {
        "SELECT CAST(column_name AS CHAR), CAST(column_type AS CHAR), is_nullable, column_key \
         FROM information_schema.columns \
         WHERE table_schema = DATABASE() AND table_name = '__diesel_schema_migrations' \
         ORDER BY ordinal_position"
    } else if common::on_postgres(db.driver()) {
        "SELECT CAST(c.column_name AS TEXT), \
         CAST(CASE WHEN c.character_maximum_length IS NULL THEN c.data_type \
             ELSE c.data_type || '(' || c.character_maximum_length || ')' END AS TEXT), \
         c.is_nullable, \
         CASE WHEN EXISTS (SELECT 1 FROM information_schema.table_constraints t \
             JOIN information_schema.key_column_usage k \
               ON k.constraint_name = t.constraint_name AND k.table_name = t.table_name \
             WHERE t.table_name = c.table_name AND t.constraint_type = 'PRIMARY KEY' \
               AND k.column_name = c.column_name) THEN 'PRI' ELSE '' END \
         FROM information_schema.columns c \
         WHERE c.table_schema = current_schema() AND c.table_name = '__diesel_schema_migrations' \
         ORDER BY c.ordinal_position"
    } else {
        "SELECT name, type, CASE WHEN \"notnull\" = 1 THEN 'NO' ELSE 'YES' END, \
         CASE WHEN pk > 0 THEN 'PRI' ELSE '' END \
         FROM pragma_table_info('__diesel_schema_migrations') ORDER BY cid"
    };
    raw::fetch_all(db, sql, &[])
        .await
        .expect("reads the column types")
}

/// (a) A fresh database gets Diesel's table, with `version`, `run_on` and `batch`, and no
/// `migrations` table.
#[tokio::test]
async fn a_fresh_database_gets_the_diesel_table_with_a_batch_column() {
    let Some((_lock, db)) = setup().await else {
        return;
    };
    assert!(!table_exists(&db, "migrations").await);

    let migrator = load(db.driver(), "basic");
    migrator
        .status(&db)
        .await
        .expect("status creates the table");

    assert!(table_exists(&db, "__diesel_schema_migrations").await);
    assert!(
        !table_exists(&db, "migrations").await,
        "no Laravel-named table"
    );
    assert_eq!(columns(&db).await, ["version", "run_on", "batch"]);

    let types = column_types(&db).await;
    // SQLite stores these as TEXT and INTEGER, which is how the migrations table is created there.
    let (version_type, run_on_type, batch_type) = if common::on_postgres(db.driver()) {
        ("character varying(50)", "timestamp", "integer")
    } else if common::on_sqlite(db.driver()) {
        ("text", "text", "integer")
    } else {
        ("varchar(50)", "timestamp", "int")
    };
    assert_eq!(types[0].0, "version");
    assert_eq!(types[0].1.to_ascii_lowercase(), version_type);
    assert_eq!(types[0].2, "NO");
    assert_eq!(types[0].3, "PRI");
    assert_eq!(types[1].0, "run_on");
    assert!(
        types[1].1.to_ascii_lowercase().starts_with(run_on_type),
        "run_on is {}",
        types[1].1
    );
    assert_eq!(types[1].2, "NO");
    assert_eq!(types[2].0, "batch");
    assert!(
        types[2].1.to_ascii_lowercase().starts_with(batch_type),
        "batch is {}",
        types[2].1
    );
    assert_eq!(types[2].2, "NO");

    // Running again must not fail on the existing table, or add the column twice.
    migrator
        .status(&db)
        .await
        .expect("the table already exists");
    assert_eq!(columns(&db).await, ["version", "run_on", "batch"]);
    common::drop_tables(&db, MIGRATION_TABLES).await;
}

/// Creates Diesel's layout (`version` and `run_on`, with the given rows) and the fixture schema.
async fn diesel_layout(db: &Database, rows: &str) {
    common::exec(
        db,
        &format!(
            "CREATE TABLE __diesel_schema_migrations (\
             version VARCHAR(50) NOT NULL PRIMARY KEY, \
             run_on TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP);\
             INSERT INTO __diesel_schema_migrations (version, run_on) VALUES {rows};\
             CREATE TABLE widgets (\
             id BIGINT NOT NULL AUTO_INCREMENT PRIMARY KEY, \
             name VARCHAR(255) NOT NULL);\
             CREATE TABLE gadgets (\
             id BIGINT NOT NULL AUTO_INCREMENT PRIMARY KEY)"
        ),
    )
    .await;
}

/// (b) A database with Diesel's layout (`version` and `run_on` only, two rows) is upgraded in place
/// by `ensure_table`: `batch` is added and the rows are batch 0. `run` then applies only the
/// missing migration, as batch 1.
#[tokio::test]
async fn a_diesel_table_is_upgraded_in_place_and_run_applies_only_the_missing() {
    let Some((_lock, db)) = setup().await else {
        return;
    };
    diesel_layout(
        &db,
        "('20260101000000', '2026-01-05 10:00:00'), ('20260102000000', '2026-01-05 10:00:00')",
    )
    .await;
    let migrator = load(db.driver(), "full");

    // Any call runs ensure_table. status is the one that only reads.
    let status = migrator.status(&db).await.expect("reads the status");
    assert_eq!(columns(&db).await, ["version", "run_on", "batch"]);
    assert_eq!(
        recorded(&db).await,
        [(v(WIDGETS), 0), (v(GADGETS), 0)],
        "Diesel's rows are batch 0"
    );
    assert_eq!(status[0].state, applied(0));
    assert_eq!(status[1].state, applied(0));
    assert_eq!(status[2].state, State::Pending);
    let run_on: Vec<(String,)> = raw::fetch_all(
        &db,
        &format!(
            "SELECT CAST(run_on AS {}) FROM __diesel_schema_migrations ORDER BY version",
            common::text_type(db.driver())
        ),
        &[],
    )
    .await
    .expect("reads run_on");
    assert_eq!(
        run_on,
        [
            ("2026-01-05 10:00:00".to_owned(),),
            ("2026-01-05 10:00:00".to_owned(),)
        ]
    );

    let names = migrator.run(&db).await.expect("runs the missing migration");
    assert_eq!(names, [COLOR], "only the migration Diesel did not record");
    assert_eq!(
        recorded(&db).await,
        [(v(WIDGETS), 0), (v(GADGETS), 0), (v(COLOR), 1)]
    );
    common::drop_tables(&db, MIGRATION_TABLES).await;
}

/// (c) After (b), a rollback reverts only the batch-1 migration and leaves Diesel's rows.
#[tokio::test]
async fn rollback_after_the_upgrade_reverts_only_the_new_batch() {
    let Some((_lock, db)) = setup().await else {
        return;
    };
    diesel_layout(
        &db,
        "('20260101000000', CURRENT_TIMESTAMP), ('20260102000000', CURRENT_TIMESTAMP)",
    )
    .await;
    let migrator = load(db.driver(), "full");
    migrator
        .run(&db)
        .await
        .expect("upgrades, then runs batch 1");

    let reverted = migrator.rollback(&db).await.expect("rolls back batch 1");
    assert_eq!(reverted, [COLOR]);
    assert_eq!(recorded(&db).await, [(v(WIDGETS), 0), (v(GADGETS), 0)]);
    assert!(
        table_exists(&db, "widgets").await,
        "Diesel's migrations stay applied"
    );
    assert!(table_exists(&db, "gadgets").await);
    // down.sql dropped the color column, so selecting it fails.
    let probe = raw::fetch_scalar::<i64>(&db, "SELECT COUNT(color) FROM widgets", &[]).await;
    assert!(probe.is_err(), "down.sql dropped the color column");
    common::drop_tables(&db, MIGRATION_TABLES).await;
}

#[test]
fn duplicate_names_are_rejected() {
    let widgets = Migration::new(WIDGETS, "SELECT 1;", "SELECT 1;");
    let error = Migrator::new(vec![widgets.clone(), widgets]).expect_err("duplicate");
    assert!(matches!(
        error,
        Error::DuplicateMigration { ref name } if name == WIDGETS
    ));
    assert_eq!(
        error.to_string(),
        format!("migration `{WIDGETS}` is defined more than once")
    );
}

#[test]
fn from_dir_sorts_by_name() {
    // The folders are read in whatever order the file system gives, so the sort is checked here.
    let migrator = Migrator::from_dir(fixture("full")).expect("reads the fixtures");
    let names: Vec<&str> = migrator
        .migrations()
        .iter()
        .map(|m| m.name.as_str())
        .collect();
    assert_eq!(names, [WIDGETS, GADGETS, COLOR]);
}

#[test]
fn migrations_macro_embeds_the_same_files() {
    let embedded = migrations!(
        "tests/fixtures/migrations/basic",
        "20260101000000_create_widgets"
    )
    .expect("embeds the fixture");
    let from_disk = Migrator::from_dir(fixture("basic")).expect("reads the fixture");
    let (embedded, from_disk) = (&embedded.migrations()[0], &from_disk.migrations()[0]);
    assert_eq!(embedded.name, from_disk.name);
    assert_eq!(embedded.up, from_disk.up);
    assert_eq!(embedded.down, from_disk.down);
}

#[test]
fn a_name_without_a_timestamp_is_rejected() {
    let migration = Migration::new("create_widgets", "SELECT 1;", "SELECT 1;");
    let err = Migrator::new(vec![migration]).expect_err("needs a timestamp");
    assert!(matches!(err, Error::InvalidMigrationName { ref name } if name == "create_widgets"));
}

#[test]
fn two_migrations_with_one_timestamp_are_rejected() {
    let a = Migration::new("20260101000000_create_widgets", "SELECT 1;", "SELECT 1;");
    let b = Migration::new("20260101000000_create_gadgets", "SELECT 1;", "SELECT 1;");
    let err = Migrator::new(vec![a, b]).expect_err("shared timestamp");
    assert!(matches!(err, Error::DuplicateTimestamp { .. }));
}

/// The migration that `a_run_waits_for_another_connection...` runs: slow enough that a second run
/// finds the lock held. It is a sleep on MySQL and PostgreSQL. SQLite has no sleep, so it counts
/// a recursive series instead.
fn slow_migrator(driver: Driver) -> Migrator {
    let up = if common::on_mysql(driver) {
        "SELECT SLEEP(1)"
    } else if common::on_postgres(driver) {
        "SELECT pg_sleep(1)"
    } else {
        "WITH RECURSIVE c(x) AS (SELECT 1 UNION ALL SELECT x + 1 FROM c LIMIT 30000000) \
         SELECT COUNT(*) FROM c"
    };
    Migrator::new(vec![Migration::new("20260101000000_slow", up, "SELECT 1")])
        .expect("one migration")
}

/// Another run holds the migrations lock while it works. A second run, on another connection to
/// the same database, waits for the lock timeout and then fails with `MigrationsLocked`. This is
/// the lock on every database: a named lock on MySQL, an advisory lock on PostgreSQL, and a
/// process-held lock on SQLite.
#[tokio::test(flavor = "multi_thread")]
async fn a_run_waits_for_another_connection_then_gives_up() {
    let Some((_lock, db)) = setup().await else {
        return;
    };
    let url = common::url().expect("the url is set");
    let other = Database::connect(&url)
        .await
        .expect("connects a second handle");
    let driver = db.driver();

    let holder = tokio::spawn(async move {
        let result = slow_migrator(driver).run(&other).await;
        other.close().await;
        result
    });
    // Give the holder time to take the lock before the second run tries.
    tokio::time::sleep(Duration::from_millis(500)).await;

    let waiting = load(driver, "basic").with_lock_timeout(Duration::from_millis(100));
    let err = waiting.run(&db).await.expect_err("the lock is held");
    assert!(
        matches!(err, Error::MigrationsLocked { .. }),
        "expected MigrationsLocked, got {err:?}"
    );

    holder
        .await
        .expect("the holder task finishes")
        .expect("the holder run succeeds");
    waiting.run(&db).await.expect("runs once the lock is free");
    common::drop_tables(&db, MIGRATION_TABLES).await;
}

/// MySQL only: a named lock taken on a transaction's connection, the way another process would
/// hold it. The second run waits for the lock timeout, then fails with `MigrationsLocked`.
#[tokio::test]
async fn a_run_waits_for_a_mysql_named_lock_then_gives_up() {
    let Some((_lock, db)) = setup().await else {
        return;
    };
    if !common::on_mysql(db.driver()) {
        // The named lock is MySQL's GET_LOCK. The test above covers the other databases.
        return;
    }
    let migrator = load(db.driver(), "basic").with_lock_timeout(Duration::from_secs(1));
    db.transaction(async |tx| {
        let got: Option<i64> = raw::fetch_scalar(
            &mut *tx,
            "SELECT GET_LOCK(CONCAT('orbit_migrations:', DATABASE()), 0)",
            &[],
        )
        .await?;
        assert_eq!(got, Some(1));

        let err = migrator.run(&db).await.expect_err("the lock is held");
        assert!(matches!(err, Error::MigrationsLocked { seconds: 1 }));

        raw::execute(
            &mut *tx,
            "SELECT RELEASE_LOCK(CONCAT('orbit_migrations:', DATABASE()))",
            &[],
        )
        .await?;
        Ok::<(), Error>(())
    })
    .await
    .expect("holds and releases the lock");
    migrator.run(&db).await.expect("runs once the lock is free");
    common::drop_tables(&db, MIGRATION_TABLES).await;
}
