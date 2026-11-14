//! Migrations written with the schema builder run on a real database: the statements are rendered
//! for the connected driver when the migrator runs. Set `ORBIT_DATABASE_URL` to a test database;
//! the tests skip with a notice when it is not set.

mod common;

use tokio::sync::{Mutex, MutexGuard};

use orbit_orm::schema;
use orbit_orm::{Database, Migration, Migrator, State, Value, raw};

static LOCK: Mutex<()> = Mutex::const_new(());

async fn setup() -> Option<(MutexGuard<'static, ()>, Database)> {
    let Some(url) = common::url() else {
        eprintln!("skipped: set ORBIT_DATABASE_URL to run the schema migration tests");
        return None;
    };
    let guard = LOCK.lock().await;
    let db = Database::connect(&url).await.expect("connects");
    // The books table references authors, so it goes first.
    common::drop_tables(
        &db,
        &[
            "sm_books",
            "sm_authors",
            "sm_plain",
            "__diesel_schema_migrations",
        ],
    )
    .await;
    Some((guard, db))
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

fn authors() -> Migration {
    Migration::schema(
        "20270101000000_create_sm_authors",
        vec![schema::create("sm_authors", |t| {
            t.id();
            t.string("name");
        })],
        vec![schema::drop_if_exists("sm_authors")],
    )
}

fn books() -> Migration {
    Migration::schema(
        "20270102000000_create_sm_books",
        vec![schema::create("sm_books", |t| {
            t.id();
            t.string("title");
            t.big_integer("author_id");
            t.foreign("author_id").references("sm_authors", "id");
            t.unique(["title"]);
        })],
        vec![schema::drop("sm_books")],
    )
}

#[tokio::test]
async fn schema_migration_runs_and_rolls_back() {
    let Some((_lock, db)) = setup().await else {
        return;
    };
    let migrator = Migrator::new(vec![books(), authors()]).expect("valid");

    let names = migrator.run(&db).await.expect("runs");
    assert_eq!(
        names,
        vec![
            "20270101000000_create_sm_authors",
            "20270102000000_create_sm_books"
        ]
    );
    assert!(table_exists(&db, "sm_authors").await);
    assert!(table_exists(&db, "sm_books").await);
    let status = migrator.status(&db).await.expect("status");
    assert!(
        status
            .iter()
            .all(|s| s.state == State::Applied { batch: 1 })
    );

    // The tables are usable, so the rendered DDL is right for this database.
    raw::execute(&db, "INSERT INTO sm_authors (name) VALUES ('Ann')", &[])
        .await
        .expect("inserts");
    raw::execute(&db, "DELETE FROM sm_authors", &[])
        .await
        .expect("clears");

    let reverted = migrator.rollback(&db).await.expect("rolls back");
    assert_eq!(
        reverted,
        vec![
            "20270102000000_create_sm_books",
            "20270101000000_create_sm_authors"
        ]
    );
    assert!(!table_exists(&db, "sm_authors").await);
    assert!(!table_exists(&db, "sm_books").await);
    let status = migrator.status(&db).await.expect("status");
    assert!(status.iter().all(|s| s.state == State::Pending));

    common::drop_tables(&db, &["__diesel_schema_migrations"]).await;
}

#[tokio::test]
async fn sql_string_migration_still_runs() {
    let Some((_lock, db)) = setup().await else {
        return;
    };
    let plain = Migration::new(
        "20270103000000_create_sm_plain",
        "CREATE TABLE sm_plain (id INTEGER PRIMARY KEY)",
        "DROP TABLE sm_plain",
    );
    assert_eq!(plain.up, "CREATE TABLE sm_plain (id INTEGER PRIMARY KEY)");
    let migrator = Migrator::new(vec![authors(), plain]).expect("valid");
    migrator.run(&db).await.expect("runs mixed migrations");
    assert!(table_exists(&db, "sm_plain").await);
    assert!(table_exists(&db, "sm_authors").await);
    migrator.rollback(&db).await.expect("rolls back");
    assert!(!table_exists(&db, "sm_plain").await);
    assert!(!table_exists(&db, "sm_authors").await);
    common::drop_tables(&db, &["__diesel_schema_migrations"]).await;
}

#[tokio::test]
async fn statement_that_cannot_render_fails_the_migration() {
    let Some((_lock, db)) = setup().await else {
        return;
    };
    let bad = Migration::schema(
        "20270104000000_create_bad",
        vec![schema::create("sm_bad`name", |t| {
            t.id();
        })],
        vec![],
    );
    let migrator = Migrator::new(vec![authors(), bad]).expect("valid");
    let err = migrator
        .run(&db)
        .await
        .expect_err("the bad statement fails");
    assert!(
        !matches!(err, orbit_orm::Error::MigrationFailed { .. }),
        "{err}"
    );
    // The first migration stays applied; the bad one is not recorded.
    assert!(table_exists(&db, "sm_authors").await);
    let status = migrator.status(&db).await.expect("status");
    assert_eq!(status[0].state, State::Applied { batch: 1 });
    assert_eq!(status[1].state, State::Pending);

    common::drop_tables(&db, &["sm_authors", "__diesel_schema_migrations"]).await;
}
