//! Runs against a real MySQL or MariaDB server. Set `ORBIT_DATABASE_URL` to a test database,
//! such as `mysql://user:password@127.0.0.1:3306/orbit_test`. The tests are skipped with a
//! notice when it is not set. Each test uses its own table and drops it at the end.

use orbit_orm::{Database, Error, Value, raw};
use sqlx::mysql::MySqlRow;
use sqlx::{AssertSqlSafe, FromRow, Row};

async fn connect() -> Option<Database> {
    let Ok(url) = std::env::var("ORBIT_DATABASE_URL") else {
        eprintln!("skipped: set ORBIT_DATABASE_URL to run the database tests");
        return None;
    };
    Some(Database::connect(&url).await.expect("connects"))
}

#[derive(Debug, PartialEq)]
struct Post {
    id: i64,
    title: String,
}

impl<'r> FromRow<'r, MySqlRow> for Post {
    fn from_row(row: &'r MySqlRow) -> Result<Self, sqlx::Error> {
        Ok(Self {
            id: row.try_get("id")?,
            title: row.try_get("title")?,
        })
    }
}

async fn setup(db: &Database, table: &str) {
    sqlx::query(AssertSqlSafe(format!("DROP TABLE IF EXISTS {table}")))
        .execute(db.pool())
        .await
        .expect("drops a leftover table");
    sqlx::query(AssertSqlSafe(format!(
        "CREATE TABLE {table} (
            id BIGINT AUTO_INCREMENT PRIMARY KEY,
            title VARCHAR(255) NOT NULL,
            views BIGINT NOT NULL
        )"
    )))
    .execute(db.pool())
    .await
    .expect("creates the table");
}

async fn teardown(db: &Database, table: &str) {
    sqlx::query(AssertSqlSafe(format!("DROP TABLE IF EXISTS {table}")))
        .execute(db.pool())
        .await
        .expect("drops the table");
}

#[tokio::test]
async fn a_bound_value_with_a_quote_is_stored_as_it_is() {
    const TABLE: &str = "orbit_raw_quote";
    let Some(db) = connect().await else { return };
    setup(&db, TABLE).await;

    let title = "O'Brien'); DROP TABLE orbit_raw_quote; --";
    let affected = raw::execute(
        &db,
        &format!("INSERT INTO {TABLE} (title, views) VALUES (?, ?)"),
        &[title.into(), Value::from(1)],
    )
    .await
    .expect("inserts");
    assert_eq!(affected, 1);

    let stored: Vec<Post> = raw::fetch_all(
        &db,
        &format!("SELECT id, title FROM {TABLE} WHERE title = ?"),
        &[title.into()],
    )
    .await
    .expect("finds");
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].title, title);

    teardown(&db, TABLE).await;
    db.close().await;
}

#[tokio::test]
async fn fetch_all_fetch_optional_and_fetch_scalar_read_rows() {
    const TABLE: &str = "orbit_raw_fetch";
    let Some(db) = connect().await else { return };
    setup(&db, TABLE).await;
    for (title, views) in [("first", 5), ("second", 50), ("third", 500)] {
        raw::execute(
            &db,
            &format!("INSERT INTO {TABLE} (title, views) VALUES (?, ?)"),
            &[title.into(), Value::from(views)],
        )
        .await
        .expect("seeds");
    }

    let popular: Vec<Post> = raw::fetch_all(
        &db,
        &format!("SELECT id, title FROM {TABLE} WHERE views >= ? ORDER BY id"),
        &[Value::from(50)],
    )
    .await
    .expect("fetches");
    let titles: Vec<&str> = popular.iter().map(|p| p.title.as_str()).collect();
    assert_eq!(titles, ["second", "third"]);

    let one: Option<Post> = raw::fetch_optional(
        &db,
        &format!("SELECT id, title FROM {TABLE} WHERE title = ?"),
        &["first".into()],
    )
    .await
    .expect("fetches");
    assert_eq!(one.map(|p| p.title), Some("first".to_owned()));

    let none: Option<Post> = raw::fetch_optional(
        &db,
        &format!("SELECT id, title FROM {TABLE} WHERE title = ?"),
        &["missing".into()],
    )
    .await
    .expect("fetches");
    assert_eq!(none, None);

    let count: i64 = raw::fetch_scalar(&db, &format!("SELECT COUNT(*) FROM {TABLE}"), &[])
        .await
        .expect("counts");
    assert_eq!(count, 3);

    teardown(&db, TABLE).await;
    db.close().await;
}

#[tokio::test]
async fn raw_calls_work_inside_a_transaction() {
    const TABLE: &str = "orbit_raw_transaction";
    let Some(db) = connect().await else { return };
    setup(&db, TABLE).await;

    let id: u64 = db
        .transaction(async |tx| {
            raw::execute(
                &mut *tx,
                &format!("INSERT INTO {TABLE} (title, views) VALUES (?, ?)"),
                &["in tx".into(), Value::from(7)],
            )
            .await?;
            // LAST_INSERT_ID is per connection, so it reads the insert made just above.
            let id: u64 = raw::fetch_scalar(&mut *tx, "SELECT LAST_INSERT_ID()", &[]).await?;
            let seen: Option<Post> = raw::fetch_optional(
                &mut *tx,
                &format!("SELECT id, title FROM {TABLE} WHERE id = ?"),
                &[Value::UInt(id)],
            )
            .await?;
            assert_eq!(seen.map(|p| p.title), Some("in tx".to_owned()));
            Ok::<_, Error>(id)
        })
        .await
        .expect("commits");

    let committed: Vec<Post> = raw::fetch_all(
        &db,
        &format!("SELECT id, title FROM {TABLE} WHERE id = ?"),
        &[Value::UInt(id)],
    )
    .await
    .expect("fetches");
    assert_eq!(committed.len(), 1);

    // An error from the closure rolls back the raw write as well.
    let result: Result<(), Error> = db
        .transaction(async |tx| {
            raw::execute(
                &mut *tx,
                &format!("INSERT INTO {TABLE} (title, views) VALUES (?, ?)"),
                &["rolled back".into(), Value::from(0)],
            )
            .await?;
            Err(Error::NotFound { table: TABLE })
        })
        .await;
    assert!(matches!(result, Err(Error::NotFound { .. })));
    let count: i64 = raw::fetch_scalar(&db, &format!("SELECT COUNT(*) FROM {TABLE}"), &[])
        .await
        .expect("counts");
    assert_eq!(count, 1);

    teardown(&db, TABLE).await;
    db.close().await;
}
