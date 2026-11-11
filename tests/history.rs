//! Diesel's migration history against a real MySQL or MariaDB server. Set `ORBIT_DATABASE_URL` to
//! a test database, such as `mysql://user:password@127.0.0.1:3306/orbit_test`. The tests are
//! skipped with a notice when it is not set. They share the `__diesel_schema_migrations` table, so
//! they take a lock and reset the tables first.

use std::path::{Path, PathBuf};

use tokio::sync::{Mutex, MutexGuard};

use orbit_orm::{Database, Migrator, State};

const WIDGETS: &str = "20260101000000_create_widgets";
const GADGETS: &str = "20260102000000_create_gadgets";
const COLOR: &str = "20260103000000_add_widget_color";

static LOCK: Mutex<()> = Mutex::const_new(());

fn full_set() -> Migrator {
    let dir: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/migrations/full");
    Migrator::from_dir(dir).expect("reads the fixture migrations")
}

/// The 14-digit version of a migration name.
fn v(name: &str) -> String {
    name[..14].to_string()
}

/// Connects, drops the tables the tests use, and holds the lock until the test ends. Returns
/// `None` with a notice when `ORBIT_DATABASE_URL` is not set.
async fn setup() -> Option<(MutexGuard<'static, ()>, Database)> {
    let Ok(url) = std::env::var("ORBIT_DATABASE_URL") else {
        eprintln!("skipped: set ORBIT_DATABASE_URL to run the history tests");
        return None;
    };
    let guard = LOCK.lock().await;
    let db = Database::connect(&url).await.expect("connects");
    sqlx::raw_sql("DROP TABLE IF EXISTS migrations, widgets, gadgets, __diesel_schema_migrations")
        .execute(db.pool())
        .await
        .expect("drops the test tables");
    Some((guard, db))
}

/// Creates Diesel's table, in Diesel's layout (`version` and `run_on` only), with the given
/// versions. Each has a fixed `run_on`.
async fn diesel_table(db: &Database, versions: &[&str]) {
    sqlx::query(
        "CREATE TABLE __diesel_schema_migrations (\
         version VARCHAR(50) PRIMARY KEY, \
         run_on TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP)",
    )
    .execute(db.pool())
    .await
    .expect("creates Diesel's table");
    for version in versions {
        add_diesel_version(db, version).await;
    }
}

async fn add_diesel_version(db: &Database, version: &str) {
    sqlx::query("INSERT INTO __diesel_schema_migrations (version, run_on) VALUES (?, ?)")
        .bind(version)
        .bind("2026-01-05 10:00:00")
        .execute(db.pool())
        .await
        .expect("inserts a Diesel version");
}

/// Diesel's columns, as they are stored: the version and the run time. Compared before and after
/// an import, which must not change them.
async fn diesel_rows(db: &Database) -> Vec<(String, String)> {
    sqlx::query_as(
        "SELECT version, CAST(run_on AS CHAR) FROM __diesel_schema_migrations ORDER BY version",
    )
    .fetch_all(db.pool())
    .await
    .expect("reads Diesel's table")
}

/// The rows of `__diesel_schema_migrations`, as `(version, batch)`.
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
    let found: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM information_schema.tables \
         WHERE table_schema = DATABASE() AND table_name = ?",
    )
    .bind(table)
    .fetch_one(db.pool())
    .await
    .expect("checks for a table");
    found == 1
}

#[tokio::test]
async fn import_upgrades_diesel_table_and_records_nothing_new() {
    let Some((_guard, db)) = setup().await else {
        return;
    };
    diesel_table(&db, &[&v(WIDGETS), &v(GADGETS)]).await;
    let before = diesel_rows(&db).await;
    let migrator = full_set();

    let imported = migrator
        .import_diesel_history(&db)
        .await
        .expect("upgrades the table");
    assert!(imported.is_empty(), "Diesel's rows are already the history");

    assert_eq!(columns(&db).await, ["version", "run_on", "batch"]);
    assert_eq!(
        diesel_rows(&db).await,
        before,
        "Diesel's version and run_on are unchanged"
    );
    assert_eq!(recorded(&db).await, [(v(WIDGETS), 0), (v(GADGETS), 0)]);

    // Nothing ran: the tables the up.sql files create do not exist.
    assert!(!table_exists(&db, "widgets").await);
    assert!(!table_exists(&db, "gadgets").await);

    let status = migrator.status(&db).await.expect("reads the status");
    assert_eq!(status[0].name, WIDGETS);
    assert_eq!(status[0].state, State::Applied { batch: 0 });
    assert_eq!(status[1].name, GADGETS);
    assert_eq!(status[1].state, State::Applied { batch: 0 });
    assert_eq!(status[2].name, COLOR);
    assert_eq!(status[2].state, State::Pending);

    db.close().await;
}

#[tokio::test]
async fn a_second_import_changes_nothing() {
    let Some((_guard, db)) = setup().await else {
        return;
    };
    diesel_table(&db, &[&v(WIDGETS), &v(GADGETS)]).await;
    let migrator = full_set();
    migrator
        .import_diesel_history(&db)
        .await
        .expect("upgrades the table");
    let after_first = recorded(&db).await;

    let again = migrator
        .import_diesel_history(&db)
        .await
        .expect("imports again");
    assert!(again.is_empty());
    assert_eq!(columns(&db).await, ["version", "run_on", "batch"]);
    assert_eq!(recorded(&db).await, after_first);

    db.close().await;
}

#[tokio::test]
async fn a_version_that_diesel_writes_later_counts_as_applied() {
    let Some((_guard, db)) = setup().await else {
        return;
    };
    // Orbit creates the table, then applies WIDGETS in batch 1.
    let migrator = full_set();
    Migrator::from_dir(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/migrations/basic"),
    )
    .expect("reads the basic fixtures")
    .run(&db)
    .await
    .expect("runs batch 1");

    // Diesel then runs GADGETS and inserts its row with only the columns it knows about.
    sqlx::raw_sql("CREATE TABLE gadgets (id BIGINT NOT NULL AUTO_INCREMENT PRIMARY KEY)")
        .execute(db.pool())
        .await
        .expect("Diesel's up.sql");
    sqlx::query("INSERT INTO __diesel_schema_migrations (version) VALUES (?)")
        .bind(v(GADGETS))
        .execute(db.pool())
        .await
        .expect("Diesel's insert, without batch");

    // The column default makes the row batch 0, and Orbit does not run GADGETS again.
    let names = migrator.run(&db).await.expect("runs the missing migration");
    assert_eq!(names, [COLOR]);
    assert_eq!(
        recorded(&db).await,
        [(v(WIDGETS), 1), (v(GADGETS), 0), (v(COLOR), 2)]
    );

    db.close().await;
}

#[tokio::test]
async fn versions_without_a_folder_are_listed_and_not_imported() {
    let Some((_guard, db)) = setup().await else {
        return;
    };
    diesel_table(&db, &[&v(WIDGETS), "20269999999999"]).await;
    let migrator = full_set();

    let unmatched = migrator
        .unmatched_diesel_versions(&db)
        .await
        .expect("lists the unmatched versions");
    assert_eq!(unmatched, vec!["20269999999999".to_owned()]);

    let imported = migrator
        .import_diesel_history(&db)
        .await
        .expect("upgrades the table");
    assert!(imported.is_empty());
    assert_eq!(
        recorded(&db).await,
        [(v(WIDGETS), 0), ("20269999999999".to_owned(), 0)]
    );

    db.close().await;
}

#[tokio::test]
async fn without_a_table_the_import_creates_it_and_records_nothing() {
    let Some((_guard, db)) = setup().await else {
        return;
    };
    let migrator = full_set();
    assert!(
        migrator
            .unmatched_diesel_versions(&db)
            .await
            .expect("lists nothing")
            .is_empty()
    );
    assert!(!table_exists(&db, "__diesel_schema_migrations").await);

    let imported = migrator
        .import_diesel_history(&db)
        .await
        .expect("no Diesel table is not an error");
    assert!(imported.is_empty());
    assert!(table_exists(&db, "__diesel_schema_migrations").await);
    assert_eq!(columns(&db).await, ["version", "run_on", "batch"]);
    assert!(recorded(&db).await.is_empty());

    db.close().await;
}
