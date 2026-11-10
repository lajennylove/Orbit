//! Runs migrations against a real MySQL or MariaDB server. Set `ORBIT_DATABASE_URL` to a test
//! database, such as `mysql://user:password@127.0.0.1:3306/orbit_test`. The database tests are
//! skipped with a notice when it is not set. They share the `migrations` table, so they take a
//! lock and reset the tables first.

use std::path::{Path, PathBuf};
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

/// Connects, drops the tables the tests use, and holds the lock until the test ends. Returns
/// `None` with a notice when `ORBIT_DATABASE_URL` is not set.
async fn setup() -> Option<(MutexGuard<'static, ()>, Database)> {
    let Ok(url) = std::env::var("ORBIT_DATABASE_URL") else {
        eprintln!("skipped: set ORBIT_DATABASE_URL to run the migration tests");
        return None;
    };
    let guard = LOCK.lock().await;
    let db = Database::connect(&url).await.expect("connects");
    sqlx::raw_sql("DROP TABLE IF EXISTS migrations, widgets, gadgets, gizmos")
        .execute(db.pool())
        .await
        .expect("drops the test tables");
    Some((guard, db))
}

/// The rows of the `migrations` table, as `(name, batch)` in name order.
async fn recorded(db: &Database) -> Vec<(String, i64)> {
    sqlx::query_as("SELECT migration, batch FROM migrations ORDER BY migration")
        .fetch_all(db.pool())
        .await
        .expect("reads the migrations table")
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
        [
            (WIDGETS.to_string(), 1),
            (GADGETS.to_string(), 1),
            (COLOR.to_string(), 1),
        ]
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
    assert_eq!(recorded(&db).await, [(WIDGETS.to_string(), 1)]);
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
    assert_eq!(recorded(&db).await, [(WIDGETS.to_string(), 1)]);
    assert!(!table_exists(&db, "gadgets").await);
    // MySQL commits the CREATE TABLE before the failing statement, so gizmos is left behind.
    assert!(table_exists(&db, "gizmos").await);
}

#[tokio::test]
async fn the_migrations_table_is_created_on_first_use() {
    let Some((_lock, db)) = setup().await else {
        return;
    };
    assert!(!table_exists(&db, "migrations").await);

    let migrator = Migrator::from_dir(fixture("basic")).expect("reads the fixtures");
    migrator
        .status(&db)
        .await
        .expect("status creates the table");
    assert!(table_exists(&db, "migrations").await);

    // Running again must not fail on the existing table.
    migrator
        .status(&db)
        .await
        .expect("the table already exists");
}

#[tokio::test]
async fn run_file_runs_one_statement_file_without_recording_it() {
    let Some((_lock, db)) = setup().await else {
        return;
    };
    db.run_file(fixture("basic").join(WIDGETS).join("up.sql"))
        .await
        .expect("runs the file");

    assert!(table_exists(&db, "widgets").await);
    assert!(
        !table_exists(&db, "migrations").await,
        "nothing is recorded"
    );
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
