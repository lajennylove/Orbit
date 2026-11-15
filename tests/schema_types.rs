//! The `binary` and `timestamp` columns, the precision of date and time columns, and
//! `use_current`. The render tests check the exact SQL for each driver; the database test uses the
//! database that `ORBIT_DATABASE_URL` names and skips with a notice when it is not set.

mod common;

use orbit_orm::schema;
use orbit_orm::{Database, Value, raw};

/// Each new type, at precision 0 and 6, and `use_current` with and without precision.
fn files() -> schema::Statement {
    schema::create("files", |t| {
        t.binary("data");
        t.timestamp("seen_at");
        t.timestamp("seen_at_6").precision(6);
        t.date_time("at_6").precision(6).nullable();
        t.date_time("created_at").use_current();
        t.timestamp("updated_at").precision(6).use_current();
    })
}

/// `use_current` takes the place of a literal default.
fn current_over_literal() -> schema::Statement {
    schema::create("logs", |t| {
        t.date_time("at")
            .default("2020-01-01 00:00:00")
            .use_current();
    })
}

/// Precision and `use_current` have no effect on other types.
fn other_types() -> schema::Statement {
    schema::create("notes", |t| {
        t.string("title").precision(6).use_current();
        t.binary("body").precision(6);
    })
}

/// The new types and modifiers in `schema::table`, added and changed. SQLite cannot change a
/// column, so its test adds columns only.
#[cfg(any(feature = "mysql", feature = "postgres"))]
fn alter() -> schema::Statement {
    schema::table("files", |t| {
        t.binary("thumb").nullable();
        t.timestamp("checked_at").precision(6).use_current();
        t.date_time("at_6").precision(3).use_current().change();
        t.timestamp("seen_at").nullable().change();
    })
}

#[cfg(feature = "mysql")]
#[test]
fn renders_mysql() {
    use orbit_orm::Driver;
    assert_eq!(
        files().to_sql(Driver::MySql),
        vec![
            "CREATE TABLE `files` (`data` BLOB NOT NULL, `seen_at` TIMESTAMP NOT NULL, \
             `seen_at_6` TIMESTAMP(6) NOT NULL, `at_6` DATETIME(6) NULL, \
             `created_at` DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP, \
             `updated_at` TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6))"
        ]
    );
    assert_eq!(
        current_over_literal().to_sql(Driver::MySql),
        vec!["CREATE TABLE `logs` (`at` DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP)"]
    );
    assert_eq!(
        other_types().to_sql(Driver::MySql),
        vec!["CREATE TABLE `notes` (`title` VARCHAR(255) NOT NULL, `body` BLOB NOT NULL)"]
    );
    assert_eq!(
        alter().to_sql(Driver::MySql),
        vec![
            "ALTER TABLE `files` ADD COLUMN `thumb` BLOB NULL",
            "ALTER TABLE `files` ADD COLUMN `checked_at` TIMESTAMP(6) NOT NULL \
             DEFAULT CURRENT_TIMESTAMP(6)",
            "ALTER TABLE `files` MODIFY COLUMN `at_6` DATETIME(3) NOT NULL \
             DEFAULT CURRENT_TIMESTAMP(3)",
            "ALTER TABLE `files` MODIFY COLUMN `seen_at` TIMESTAMP NULL",
        ]
    );
}

#[cfg(feature = "postgres")]
#[test]
fn renders_postgres() {
    use orbit_orm::Driver;
    assert_eq!(
        files().to_sql(Driver::Postgres),
        vec![
            "CREATE TABLE \"files\" (\"data\" BYTEA NOT NULL, \
             \"seen_at\" TIMESTAMP(0) WITHOUT TIME ZONE NOT NULL, \
             \"seen_at_6\" TIMESTAMP(6) WITHOUT TIME ZONE NOT NULL, \
             \"at_6\" TIMESTAMP(6) WITHOUT TIME ZONE NULL, \
             \"created_at\" TIMESTAMP(0) WITHOUT TIME ZONE NOT NULL DEFAULT CURRENT_TIMESTAMP, \
             \"updated_at\" TIMESTAMP(6) WITHOUT TIME ZONE NOT NULL DEFAULT CURRENT_TIMESTAMP)"
        ]
    );
    assert_eq!(
        current_over_literal().to_sql(Driver::Postgres),
        vec![
            "CREATE TABLE \"logs\" (\"at\" TIMESTAMP(0) WITHOUT TIME ZONE NOT NULL \
             DEFAULT CURRENT_TIMESTAMP)"
        ]
    );
    assert_eq!(
        other_types().to_sql(Driver::Postgres),
        vec!["CREATE TABLE \"notes\" (\"title\" VARCHAR(255) NOT NULL, \"body\" BYTEA NOT NULL)"]
    );
    assert_eq!(
        alter().to_sql(Driver::Postgres),
        vec![
            "ALTER TABLE \"files\" ADD COLUMN \"thumb\" BYTEA NULL",
            "ALTER TABLE \"files\" ADD COLUMN \"checked_at\" TIMESTAMP(6) WITHOUT TIME ZONE \
             NOT NULL DEFAULT CURRENT_TIMESTAMP",
            "ALTER TABLE \"files\" ALTER COLUMN \"at_6\" TYPE TIMESTAMP(3) WITHOUT TIME ZONE, \
             ALTER COLUMN \"at_6\" SET NOT NULL, ALTER COLUMN \"at_6\" SET DEFAULT CURRENT_TIMESTAMP",
            "ALTER TABLE \"files\" ALTER COLUMN \"seen_at\" TYPE TIMESTAMP(0) WITHOUT TIME ZONE, \
             ALTER COLUMN \"seen_at\" DROP NOT NULL, ALTER COLUMN \"seen_at\" DROP DEFAULT",
        ]
    );
}

#[cfg(feature = "sqlite")]
#[test]
fn renders_sqlite() {
    use orbit_orm::Driver;
    assert_eq!(
        files().to_sql(Driver::Sqlite),
        vec![
            "CREATE TABLE \"files\" (\"data\" BLOB NOT NULL, \"seen_at\" DATETIME NOT NULL, \
             \"seen_at_6\" DATETIME NOT NULL, \"at_6\" DATETIME NULL, \
             \"created_at\" DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP, \
             \"updated_at\" DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP)"
        ]
    );
    assert_eq!(
        current_over_literal().to_sql(Driver::Sqlite),
        vec!["CREATE TABLE \"logs\" (\"at\" DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP)"]
    );
    assert_eq!(
        other_types().to_sql(Driver::Sqlite),
        vec!["CREATE TABLE \"notes\" (\"title\" TEXT NOT NULL, \"body\" BLOB NOT NULL)"]
    );
    assert_eq!(
        schema::table("files", |t| {
            t.binary("thumb").nullable();
            t.timestamp("checked_at").nullable().use_current();
        })
        .to_sql(Driver::Sqlite),
        vec![
            "ALTER TABLE \"files\" ADD COLUMN \"thumb\" BLOB NULL",
            "ALTER TABLE \"files\" ADD COLUMN \"checked_at\" DATETIME NULL \
             DEFAULT CURRENT_TIMESTAMP",
        ]
    );
}

async fn run(db: &Database, statement: &schema::Statement) {
    for sql in statement.to_sql(db.driver()) {
        raw::execute(db, &sql, &[])
            .await
            .unwrap_or_else(|e| panic!("`{sql}` failed: {e}"));
    }
}

#[tokio::test]
async fn the_new_types_work_on_the_database() {
    let Some(url) = common::url() else {
        eprintln!("skipped: ORBIT_DATABASE_URL is not set");
        return;
    };
    let db = Database::connect(&url).await.expect("connects");
    let driver = db.driver();
    run(&db, &schema::drop_if_exists("typed_items")).await;
    run(
        &db,
        &schema::create("typed_items", |t| {
            t.id();
            t.binary("data");
            t.date_time("created_at").precision(6).use_current();
            // Nullable: before MariaDB 10.10 a NOT NULL TIMESTAMP gets an implicit default.
            t.timestamp("seen_at").nullable();
        }),
    )
    .await;

    let bytes = vec![0u8, 1, 2, 254, 255, b'\'', b'\\'];
    raw::execute(
        &db,
        "INSERT INTO typed_items (data) VALUES (?)",
        &[Value::from(bytes.clone())],
    )
    .await
    .expect("inserts without the defaulted column");

    let data: Vec<u8> = raw::fetch_scalar(&db, "SELECT data FROM typed_items", &[])
        .await
        .expect("reads the bytes");
    assert_eq!(data, bytes, "the bytes read back equal");

    let set: i64 = raw::fetch_scalar(
        &db,
        &format!(
            "SELECT CAST(COUNT(*) AS {}) FROM typed_items \
             WHERE created_at IS NOT NULL AND seen_at IS NULL",
            common::bigint_type(driver)
        ),
        &[],
    )
    .await
    .expect("reads the defaulted column");
    assert_eq!(set, 1, "use_current fills the column");

    run(&db, &schema::drop("typed_items")).await;
}
