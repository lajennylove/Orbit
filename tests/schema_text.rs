//! The `medium_text` and `long_text` columns, and `text` unchanged beside them. The render tests
//! check the exact SQL for each driver; the database test uses the database that
//! `ORBIT_DATABASE_URL` names and skips with a notice when it is not set.

mod common;

use orbit_orm::schema;
use orbit_orm::{Database, Value, raw};

/// Each text type, with one of them nullable.
fn posts() -> schema::Statement {
    schema::create("posts", |t| {
        t.text("excerpt");
        t.medium_text("summary");
        t.long_text("content").nullable();
    })
}

/// A plain `text` column on its own.
fn plain_text() -> schema::Statement {
    schema::create("notes", |t| {
        t.text("body");
    })
}

/// `long_text` in `schema::table`, added and changed. SQLite cannot change a column, so its test
/// adds the column only.
#[cfg(any(feature = "mysql", feature = "postgres"))]
fn alter() -> schema::Statement {
    schema::table("posts", |t| {
        t.long_text("meta_value").nullable();
        t.long_text("excerpt").change();
    })
}

#[cfg(feature = "mysql")]
#[test]
fn renders_mysql() {
    use orbit_orm::Driver;
    assert_eq!(
        posts().to_sql(Driver::MySql),
        vec![
            "CREATE TABLE `posts` (`excerpt` TEXT NOT NULL, `summary` MEDIUMTEXT NOT NULL, \
             `content` LONGTEXT NULL)"
        ]
    );
    assert_eq!(
        alter().to_sql(Driver::MySql),
        vec![
            "ALTER TABLE `posts` ADD COLUMN `meta_value` LONGTEXT NULL",
            "ALTER TABLE `posts` MODIFY COLUMN `excerpt` LONGTEXT NOT NULL",
        ]
    );
}

#[cfg(feature = "postgres")]
#[test]
fn renders_postgres() {
    use orbit_orm::Driver;
    assert_eq!(
        posts().to_sql(Driver::Postgres),
        vec![
            "CREATE TABLE \"posts\" (\"excerpt\" TEXT NOT NULL, \"summary\" TEXT NOT NULL, \
             \"content\" TEXT NULL)"
        ]
    );
    assert_eq!(
        alter().to_sql(Driver::Postgres),
        vec![
            "ALTER TABLE \"posts\" ADD COLUMN \"meta_value\" TEXT NULL",
            "ALTER TABLE \"posts\" ALTER COLUMN \"excerpt\" TYPE TEXT, \
             ALTER COLUMN \"excerpt\" SET NOT NULL, ALTER COLUMN \"excerpt\" DROP DEFAULT",
        ]
    );
}

#[cfg(feature = "sqlite")]
#[test]
fn renders_sqlite() {
    use orbit_orm::Driver;
    assert_eq!(
        posts().to_sql(Driver::Sqlite),
        vec![
            "CREATE TABLE \"posts\" (\"excerpt\" TEXT NOT NULL, \"summary\" TEXT NOT NULL, \
             \"content\" TEXT NULL)"
        ]
    );
    assert_eq!(
        schema::table("posts", |t| {
            t.long_text("meta_value").nullable();
        })
        .to_sql(Driver::Sqlite),
        vec!["ALTER TABLE \"posts\" ADD COLUMN \"meta_value\" TEXT NULL"]
    );
}

/// `text` stays `TEXT` on every database this build supports.
#[test]
fn text_still_renders_text() {
    use orbit_orm::Driver;
    let drivers = [
        #[cfg(feature = "mysql")]
        (Driver::MySql, "CREATE TABLE `notes` (`body` TEXT NOT NULL)"),
        #[cfg(feature = "postgres")]
        (
            Driver::Postgres,
            "CREATE TABLE \"notes\" (\"body\" TEXT NOT NULL)",
        ),
        #[cfg(feature = "sqlite")]
        (
            Driver::Sqlite,
            "CREATE TABLE \"notes\" (\"body\" TEXT NOT NULL)",
        ),
    ];
    for (driver, sql) in drivers {
        assert_eq!(plain_text().to_sql(driver), vec![sql]);
    }
}

async fn run(db: &Database, statement: &schema::Statement) {
    for sql in statement.to_sql(db.driver()) {
        raw::execute(db, &sql, &[])
            .await
            .unwrap_or_else(|e| panic!("`{sql}` failed: {e}"));
    }
}

/// A string of exactly `len` bytes: mixed ASCII with a few multi-byte UTF-8 characters.
fn mixed_text(len: usize) -> String {
    let pattern = "Lorem ipsum <p>dolor</p> sit amet, 'quoted' \\ \"double\" é 🚀 ✓\n";
    let mut s = String::with_capacity(len);
    while s.len() < len {
        for c in pattern.chars() {
            if s.len() + c.len_utf8() > len {
                break;
            }
            s.push(c);
        }
        if s.len() + 4 > len {
            // Fill the last few bytes with ASCII so the length is exact.
            while s.len() < len {
                s.push('x');
            }
        }
    }
    s
}

#[tokio::test]
async fn long_and_medium_text_hold_large_values_on_the_database() {
    let Some(url) = common::url() else {
        eprintln!("skipped: ORBIT_DATABASE_URL is not set");
        return;
    };
    let db = Database::connect(&url).await.expect("connects");
    run(&db, &schema::drop_if_exists("text_sized_items")).await;
    run(
        &db,
        &schema::create("text_sized_items", |t| {
            t.id();
            t.long_text("content");
            t.medium_text("summary");
        }),
    )
    .await;

    let content = mixed_text(1_048_576);
    let summary = mixed_text(100_000);
    assert_eq!(content.len(), 1_048_576);
    assert_eq!(summary.len(), 100_000);
    assert!(content.chars().any(|c| c.len_utf8() > 1), "has multi-byte");

    raw::execute(
        &db,
        "INSERT INTO text_sized_items (content, summary) VALUES (?, ?)",
        &[Value::from(content.clone()), Value::from(summary.clone())],
    )
    .await
    .expect("inserts the large values");

    let read: String = raw::fetch_scalar(&db, "SELECT content FROM text_sized_items", &[])
        .await
        .expect("reads the long text");
    assert_eq!(read.len(), content.len(), "the long text keeps its length");
    assert!(read == content, "the long text reads back equal");

    let read: String = raw::fetch_scalar(&db, "SELECT summary FROM text_sized_items", &[])
        .await
        .expect("reads the medium text");
    assert_eq!(
        read.len(),
        summary.len(),
        "the medium text keeps its length"
    );
    assert!(read == summary, "the medium text reads back equal");

    run(&db, &schema::drop("text_sized_items")).await;
}
