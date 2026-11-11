//! Diesel's migration history against a real database: MySQL or MariaDB, PostgreSQL or SQLite. Set
//! `ORBIT_DATABASE_URL` to a test database, such as `mysql://user@127.0.0.1:3306/orbit_test`. The
//! tests are skipped with a notice when it is not set. They share the `__diesel_schema_migrations`
//! table, so they take a lock and reset the tables first. The fixture files are MySQL syntax, and
//! `load` translates them for the database under test.

mod common;

use std::path::{Path, PathBuf};

use tokio::sync::{Mutex, MutexGuard};

use orbit_orm::{Database, Driver, Migration, Migrator, State, Value, raw};

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

/// Reads the full fixture set, translating each file from MySQL syntax into `driver`'s syntax.
fn full_set(driver: Driver) -> Migrator {
    let dir: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/migrations/full");
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

/// Reads the basic fixture set, translated like `full_set`.
fn basic_set(driver: Driver) -> Migrator {
    let folder = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/migrations/basic/20260101000000_create_widgets");
    let up = std::fs::read_to_string(folder.join("up.sql")).expect("reads up.sql");
    let down = std::fs::read_to_string(folder.join("down.sql")).expect("reads down.sql");
    Migrator::new(vec![Migration::new(
        "20260101000000_create_widgets",
        common::sql(driver, &up),
        common::sql(driver, &down),
    )])
    .expect("the fixture is valid")
}

/// The 14-digit version of a migration name.
fn v(name: &str) -> String {
    name[..14].to_string()
}

/// Connects, drops the tables the tests use, and holds the lock until the test ends. Returns
/// `None` with a notice when `ORBIT_DATABASE_URL` is not set.
async fn setup() -> Option<(MutexGuard<'static, ()>, Database)> {
    let Some(url) = common::url() else {
        eprintln!("skipped: set ORBIT_DATABASE_URL to run the history tests");
        return None;
    };
    let guard = LOCK.lock().await;
    let db = Database::connect(&url).await.expect("connects");
    common::drop_tables(
        &db,
        &[
            "migrations",
            "widgets",
            "gadgets",
            "__diesel_schema_migrations",
        ],
    )
    .await;
    Some((guard, db))
}

/// Creates Diesel's table, in Diesel's layout (`version` and `run_on` only), with the given
/// versions. Each has a fixed `run_on`.
async fn diesel_table(db: &Database, versions: &[&str]) {
    common::exec(
        db,
        "CREATE TABLE __diesel_schema_migrations (\
         version VARCHAR(50) PRIMARY KEY, \
         run_on TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP)",
    )
    .await;
    for version in versions {
        add_diesel_version(db, version).await;
    }
}

async fn add_diesel_version(db: &Database, version: &str) {
    raw::execute(
        db,
        "INSERT INTO __diesel_schema_migrations (version, run_on) VALUES (?, ?)",
        &[Value::from(version), Value::from(datetime())],
    )
    .await
    .expect("inserts a Diesel version");
}

/// The fixed run time of Diesel's rows.
fn datetime() -> chrono::NaiveDateTime {
    chrono::NaiveDate::from_ymd_opt(2026, 1, 5)
        .unwrap()
        .and_hms_opt(10, 0, 0)
        .unwrap()
}

/// Diesel's columns, as they are stored: the version and the run time. Compared before and after
/// an import, which must not change them.
async fn diesel_rows(db: &Database) -> Vec<(String, String)> {
    raw::fetch_all(
        db,
        &format!(
            "SELECT version, CAST(run_on AS {}) FROM __diesel_schema_migrations ORDER BY version",
            common::text_type(db.driver())
        ),
        &[],
    )
    .await
    .expect("reads Diesel's table")
}

/// The rows of `__diesel_schema_migrations`, as `(version, batch)`.
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
    let found: i64 = raw::fetch_scalar(
        db,
        common::table_count_sql(db.driver()),
        &[Value::from(table)],
    )
    .await
    .expect("checks for a table");
    found == 1
}

#[tokio::test]
#[allow(deprecated)] // Exercises the deprecated import_diesel_history on purpose.
async fn import_upgrades_diesel_table_and_records_nothing_new() {
    let Some((_guard, db)) = setup().await else {
        return;
    };
    diesel_table(&db, &[&v(WIDGETS), &v(GADGETS)]).await;
    let before = diesel_rows(&db).await;
    let migrator = full_set(db.driver());

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

    common::drop_tables(&db, MIGRATION_TABLES).await;
    db.close().await;
}

#[tokio::test]
#[allow(deprecated)] // Exercises the deprecated import_diesel_history on purpose.
async fn a_second_import_changes_nothing() {
    let Some((_guard, db)) = setup().await else {
        return;
    };
    diesel_table(&db, &[&v(WIDGETS), &v(GADGETS)]).await;
    let migrator = full_set(db.driver());
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

    common::drop_tables(&db, MIGRATION_TABLES).await;
    db.close().await;
}

#[tokio::test]
async fn a_version_that_diesel_writes_later_counts_as_applied() {
    let Some((_guard, db)) = setup().await else {
        return;
    };
    // Orbit creates the table, then applies WIDGETS in batch 1.
    let migrator = full_set(db.driver());
    basic_set(db.driver()).run(&db).await.expect("runs batch 1");

    // Diesel then runs GADGETS and inserts its row with only the columns it knows about.
    common::exec(
        &db,
        "CREATE TABLE gadgets (id BIGINT NOT NULL AUTO_INCREMENT PRIMARY KEY)",
    )
    .await;
    raw::execute(
        &db,
        "INSERT INTO __diesel_schema_migrations (version) VALUES (?)",
        &[Value::from(v(GADGETS))],
    )
    .await
    .expect("Diesel's insert, without batch");

    // The column default makes the row batch 0, and Orbit does not run GADGETS again.
    let names = migrator.run(&db).await.expect("runs the missing migration");
    assert_eq!(names, [COLOR]);
    assert_eq!(
        recorded(&db).await,
        [(v(WIDGETS), 1), (v(GADGETS), 0), (v(COLOR), 2)]
    );

    common::drop_tables(&db, MIGRATION_TABLES).await;
    db.close().await;
}

#[tokio::test]
#[allow(deprecated)] // Exercises the deprecated import_diesel_history on purpose.
async fn versions_without_a_folder_are_listed_and_not_imported() {
    let Some((_guard, db)) = setup().await else {
        return;
    };
    diesel_table(&db, &[&v(WIDGETS), "20269999999999"]).await;
    let migrator = full_set(db.driver());

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

    common::drop_tables(&db, MIGRATION_TABLES).await;
    db.close().await;
}

#[tokio::test]
#[allow(deprecated)] // Exercises the deprecated import_diesel_history on purpose.
async fn without_a_table_the_import_creates_it_and_records_nothing() {
    let Some((_guard, db)) = setup().await else {
        return;
    };
    let migrator = full_set(db.driver());
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

    common::drop_tables(&db, MIGRATION_TABLES).await;
    db.close().await;
}
