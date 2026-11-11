//! Runs against a real database: MySQL or MariaDB, PostgreSQL or SQLite. Set `ORBIT_DATABASE_URL`
//! to a test database, such as `mysql://user@127.0.0.1:3306/orbit_test`. The tests are skipped
//! with a notice when it is not set. Each test uses its own table and drops it at the end.

mod common;

use orbit_orm::{Database, Error, Value, raw};
use sqlx::{FromRow, Row};

async fn connect() -> Option<Database> {
    let url = common::url()?;
    Some(Database::connect(&url).await.expect("connects"))
}

#[derive(Debug, PartialEq)]
struct Post {
    id: i64,
    title: String,
}

impl<'r, R> FromRow<'r, R> for Post
where
    R: Row,
    &'r str: sqlx::ColumnIndex<R>,
    i64: sqlx::Decode<'r, R::Database> + sqlx::Type<R::Database>,
    String: sqlx::Decode<'r, R::Database> + sqlx::Type<R::Database>,
{
    fn from_row(row: &'r R) -> Result<Self, sqlx::Error> {
        Ok(Self {
            id: row.try_get("id")?,
            title: row.try_get("title")?,
        })
    }
}

async fn setup(db: &Database, table: &str) {
    common::drop_tables(db, &[table]).await;
    common::exec(
        db,
        &format!(
            "CREATE TABLE {table} (
                id BIGINT AUTO_INCREMENT PRIMARY KEY,
                title VARCHAR(255) NOT NULL,
                views BIGINT NOT NULL
            )"
        ),
    )
    .await;
}

async fn teardown(db: &Database, table: &str) {
    common::drop_tables(db, &[table]).await;
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
            // The last insert id is per connection, so it reads the insert made just above. Each
            // database names the function its own way; the id is read as a signed integer.
            let sql = if common::on_mysql(tx.driver()) {
                "SELECT CAST(LAST_INSERT_ID() AS SIGNED)"
            } else if common::on_postgres(tx.driver()) {
                "SELECT lastval()"
            } else {
                "SELECT last_insert_rowid()"
            };
            let id: i64 = raw::fetch_scalar(&mut *tx, sql, &[]).await?;
            let id = id as u64;
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
