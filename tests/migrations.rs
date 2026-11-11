//! Runs migrations against a real MySQL or MariaDB server. Set `ORBIT_DATABASE_URL` to a test
//! database, such as `mysql://user:password@127.0.0.1:3306/orbit_test`. The database tests are
//! skipped with a notice when it is not set. They share the `__diesel_schema_migrations` table,
//! so they take a lock and reset the tables first.

use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::sync::{Mutex, MutexGuard};

use orbit_orm::{Database, Error, Migration, Migrator, State, Status, migrations};

const WIDGETS: &str = "20260101000000_create_widgets";
const GADGETS: &str = "20260102000000_create_gadgets";
const COLOR: &str = "20260103000000_add_widget_color";

static LOCK: Mutex<()> = Mutex::const_new(());

fn fixture(set: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/migrations")
        .join(set)
}

/// The version of a migration name: its 14-digit timestamp prefix.
fn v(name: &str) -> String {
    name[..14].to_string()
}

/// Connects, drops the tables the tests use, and holds the lock until the test ends. Returns
/// `None` with a notice when `ORBIT_DATABASE_URL` is not set.
async fn setup() -> Option<(MutexGuard<'static, ()>, Database)> {
    let Ok(url) = std::env::var("ORBIT_DATABASE_URL") else {
        eprintln!("skipped: set ORBIT_DATABASE_URL to run the migration tests");
        return None;
    };
    let guard = LOCK.lock().await;
    let db = Database::connect(&url).await.expect("connects");
    sqlx::raw_sql(
        "DROP TABLE IF EXISTS __diesel_schema_migrations, migrations, widgets, gadgets, gizmos",
    )
    .execute(db.pool())
    .await
    .expect("drops the test tables");
    Some((guard, db))
}

/// The rows of `__diesel_schema_migrations`, as `(version, batch)` in version order.
async fn recorded(db: &Database) -> Vec<(String, i64)> {
    sqlx::query_as("SELECT version, batch FROM __diesel_schema_migrations ORDER BY version")
        .fetch_all(db.pool())
        .await
        .expect("reads the migrations table")
}

/// The columns of `__diesel_schema_migrations`, in table order.
async fn columns(db: &Database) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT column_name FROM information_schema.columns \
         WHERE table_schema = DATABASE() AND table_name = '__diesel_schema_migrations' \
         ORDER BY ordinal_position",
    )
    .fetch_all(db.pool())
    .await
    .expect("reads the columns")
}

async fn table_exists(db: &Database, table: &str) -> bool {
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM information_schema.tables \
         WHERE table_schema = DATABASE() AND table_name = ?",
    )
    .bind(table)
    .fetch_one(db.pool())
    .await
    .expect("reads information_schema");
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
    let migrator = Migrator::from_dir(fixture("full")).expect("reads the fixtures");

    // COLOR alters widgets, so it can only run after WIDGETS: the order is checked by success.
    let names = migrator.run(&db).await.expect("runs every migration");
    assert_eq!(names, [WIDGETS, GADGETS, COLOR]);
    assert_eq!(
        recorded(&db).await,
        [(v(WIDGETS), 1), (v(GADGETS), 1), (v(COLOR), 1)]
    );
    assert!(table_exists(&db, "gadgets").await);
}

#[tokio::test]
async fn a_second_run_applies_nothing() {
    let Some((_lock, db)) = setup().await else {
        return;
    };
    let migrator = Migrator::from_dir(fixture("full")).expect("reads the fixtures");
    migrator.run(&db).await.expect("first run");

    let names = migrator.run(&db).await.expect("second run");
    assert!(names.is_empty());
    assert_eq!(recorded(&db).await.len(), 3);
    assert!(recorded(&db).await.iter().all(|(_, batch)| *batch == 1));
}

#[tokio::test]
async fn status_lists_applied_and_pending() {
    let Some((_lock, db)) = setup().await else {
        return;
    };
    Migrator::from_dir(fixture("basic"))
        .expect("reads the basic fixtures")
        .run(&db)
        .await
        .expect("runs the first batch");

    let status = Migrator::from_dir(fixture("full"))
        .expect("reads the fixtures")
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
}

#[tokio::test]
async fn rollback_reverts_the_last_batch_only() {
    let Some((_lock, db)) = setup().await else {
        return;
    };
    Migrator::from_dir(fixture("basic"))
        .expect("reads the basic fixtures")
        .run(&db)
        .await
        .expect("runs batch 1");
    let full = Migrator::from_dir(fixture("full")).expect("reads the fixtures");
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
}

#[tokio::test]
async fn a_failing_migration_stops_the_run_and_names_it() {
    let Some((_lock, db)) = setup().await else {
        return;
    };
    let migrator = Migrator::from_dir(fixture("broken")).expect("reads the fixtures");

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
    // MySQL commits the CREATE TABLE before the failing statement, so gizmos is left behind.
    assert!(table_exists(&db, "gizmos").await);
}

/// (a) A fresh database gets Diesel's table, with `version`, `run_on` and `batch`, and no
/// `migrations` table.
#[tokio::test]
async fn a_fresh_database_gets_the_diesel_table_with_a_batch_column() {
    let Some((_lock, db)) = setup().await else {
        return;
    };
    assert!(!table_exists(&db, "migrations").await);

    let migrator = Migrator::from_dir(fixture("basic")).expect("reads the fixtures");
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

    let types: Vec<(String, String, String, String)> = sqlx::query_as(
        "SELECT column_name, column_type, is_nullable, column_key FROM information_schema.columns \
         WHERE table_schema = DATABASE() AND table_name = '__diesel_schema_migrations' \
         ORDER BY ordinal_position",
    )
    .fetch_all(db.pool())
    .await
    .expect("reads the column types");
    assert_eq!(types[0].0, "version");
    assert_eq!(types[0].1, "varchar(50)");
    assert_eq!(types[0].2, "NO");
    assert_eq!(types[0].3, "PRI");
    assert_eq!(types[1].0, "run_on");
    assert!(
        types[1].1.starts_with("timestamp"),
        "run_on is {}",
        types[1].1
    );
    assert_eq!(types[1].2, "NO");
    assert_eq!(types[2].0, "batch");
    assert!(types[2].1.starts_with("int"), "batch is {}", types[2].1);
    assert_eq!(types[2].2, "NO");

    // Running again must not fail on the existing table, or add the column twice.
    migrator
        .status(&db)
        .await
        .expect("the table already exists");
    assert_eq!(columns(&db).await, ["version", "run_on", "batch"]);
}

/// (b) A database with Diesel's layout (`version` and `run_on` only, two rows) is upgraded in place
/// by `ensure_table`: `batch` is added and the rows are batch 0. `run` then applies only the
/// missing migration, as batch 1.
#[tokio::test]
async fn a_diesel_table_is_upgraded_in_place_and_run_applies_only_the_missing() {
    let Some((_lock, db)) = setup().await else {
        return;
    };
    sqlx::raw_sql(
        "CREATE TABLE __diesel_schema_migrations (\
             version VARCHAR(50) NOT NULL PRIMARY KEY, \
             run_on TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP);\
         INSERT INTO __diesel_schema_migrations (version, run_on) VALUES \
             ('20260101000000', '2026-01-05 10:00:00'), \
             ('20260102000000', '2026-01-05 10:00:00');\
         CREATE TABLE widgets (\
             id BIGINT NOT NULL AUTO_INCREMENT PRIMARY KEY, \
             name VARCHAR(255) NOT NULL);\
         CREATE TABLE gadgets (\
             id BIGINT NOT NULL AUTO_INCREMENT PRIMARY KEY);",
    )
    .execute(db.pool())
    .await
    .expect("creates Diesel's table and the fixture schema");
    let migrator = Migrator::from_dir(fixture("full")).expect("reads the fixtures");

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
    let run_on: Vec<String> = sqlx::query_scalar(
        "SELECT CAST(run_on AS CHAR) FROM __diesel_schema_migrations ORDER BY version",
    )
    .fetch_all(db.pool())
    .await
    .expect("reads run_on");
    assert_eq!(run_on, ["2026-01-05 10:00:00", "2026-01-05 10:00:00"]);

    let names = migrator.run(&db).await.expect("runs the missing migration");
    assert_eq!(names, [COLOR], "only the migration Diesel did not record");
    assert_eq!(
        recorded(&db).await,
        [(v(WIDGETS), 0), (v(GADGETS), 0), (v(COLOR), 1)]
    );
}

/// (c) After (b), a rollback reverts only the batch-1 migration and leaves Diesel's rows.
#[tokio::test]
async fn rollback_after_the_upgrade_reverts_only_the_new_batch() {
    let Some((_lock, db)) = setup().await else {
        return;
    };
    sqlx::raw_sql(
        "CREATE TABLE __diesel_schema_migrations (\
             version VARCHAR(50) NOT NULL PRIMARY KEY, \
             run_on TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP);\
         INSERT INTO __diesel_schema_migrations (version) VALUES \
             ('20260101000000'), ('20260102000000');\
         CREATE TABLE widgets (\
             id BIGINT NOT NULL AUTO_INCREMENT PRIMARY KEY, \
             name VARCHAR(255) NOT NULL);\
         CREATE TABLE gadgets (\
             id BIGINT NOT NULL AUTO_INCREMENT PRIMARY KEY);",
    )
    .execute(db.pool())
    .await
    .expect("creates Diesel's table and the fixture schema");
    let migrator = Migrator::from_dir(fixture("full")).expect("reads the fixtures");
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
    let color_gone: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM information_schema.columns \
         WHERE table_schema = DATABASE() AND table_name = 'widgets' AND column_name = 'color'",
    )
    .fetch_one(db.pool())
    .await
    .expect("reads information_schema");
    assert_eq!(color_gone, 0, "down.sql dropped the color column");
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

#[tokio::test]
async fn a_run_waits_for_another_process_then_gives_up() {
    let Some((_lock, db)) = setup().await else {
        return;
    };
    // Another process holds the migrations lock on its own connection.
    let mut other = db.pool().acquire().await.expect("a connection");
    let got: Option<i64> =
        sqlx::query_scalar("SELECT GET_LOCK(CONCAT('orbit_migrations:', DATABASE()), 0)")
            .fetch_one(&mut *other)
            .await
            .expect("takes the lock");
    assert_eq!(got, Some(1));

    let migrator = Migrator::from_dir(fixture("basic"))
        .expect("reads the fixtures")
        .with_lock_timeout(Duration::from_secs(1));
    let err = migrator.run(&db).await.expect_err("the lock is held");
    assert!(matches!(err, Error::MigrationsLocked { seconds: 1 }));

    sqlx::query("SELECT RELEASE_LOCK(CONCAT('orbit_migrations:', DATABASE()))")
        .execute(&mut *other)
        .await
        .expect("releases the lock");
    drop(other);
    migrator.run(&db).await.expect("runs once the lock is free");
}
