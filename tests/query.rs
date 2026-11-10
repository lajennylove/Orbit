//! Runs against a real MySQL or MariaDB server. Set `ORBIT_DATABASE_URL` to a test database,
//! such as `mysql://user:password@127.0.0.1:3306/orbit_test`. The tests are skipped with a
//! notice when it is not set. Each test uses its own table and drops it at the end.

use orbit_orm::{Database, Direction, Error, Model, Operator, Query, Value};
use sqlx::mysql::MySqlRow;
use sqlx::{AssertSqlSafe, FromRow, Row};

async fn connect() -> Option<Database> {
    let Ok(url) = std::env::var("ORBIT_DATABASE_URL") else {
        eprintln!("skipped: set ORBIT_DATABASE_URL to run the database tests");
        return None;
    };
    Some(Database::connect(&url).await.expect("connects"))
}

/// Declares a row type and its model on the given table.
macro_rules! model {
    ($name:ident, $table:literal) => {
        #[derive(Debug, PartialEq)]
        struct $name {
            id: i64,
            title: String,
            views: Option<i64>,
        }

        impl<'r> FromRow<'r, MySqlRow> for $name {
            fn from_row(row: &'r MySqlRow) -> Result<Self, sqlx::Error> {
                Ok(Self {
                    id: row.try_get("id")?,
                    title: row.try_get("title")?,
                    views: row.try_get("views")?,
                })
            }
        }

        impl Model for $name {
            const TABLE: &'static str = $table;
            const FILLABLE: &'static [&'static str] = &["title", "views"];
        }
    };
}

model!(Find, "orbit_query_find");
model!(FindOrFail, "orbit_query_find_or_fail");
model!(First, "orbit_query_first");
model!(Get, "orbit_query_get");
model!(Where, "orbit_query_where");
model!(Count, "orbit_query_count");
model!(Insert, "orbit_query_insert");
model!(Update, "orbit_query_update");
model!(Delete, "orbit_query_delete");
model!(Inject, "orbit_query_inject");
model!(BadTable, "orbit`bad");

async fn setup(db: &Database, table: &str) {
    sqlx::query(AssertSqlSafe(format!("DROP TABLE IF EXISTS `{table}`")))
        .execute(db.pool())
        .await
        .expect("drops a leftover table");
    sqlx::query(AssertSqlSafe(format!(
        "CREATE TABLE `{table}` (
            id BIGINT AUTO_INCREMENT PRIMARY KEY,
            title VARCHAR(255) NOT NULL,
            views BIGINT NULL
        )"
    )))
    .execute(db.pool())
    .await
    .expect("creates the table");
}

async fn teardown(db: &Database, table: &str) {
    sqlx::query(AssertSqlSafe(format!("DROP TABLE IF EXISTS `{table}`")))
        .execute(db.pool())
        .await
        .expect("drops the table");
}

async fn seed(db: &Database, table: &str, rows: &[(i64, &str, Option<i64>)]) {
    for (id, title, views) in rows {
        sqlx::query(AssertSqlSafe(format!(
            "INSERT INTO `{table}` (id, title, views) VALUES (?, ?, ?)"
        )))
        .bind(*id)
        .bind(*title)
        .bind(*views)
        .execute(db.pool())
        .await
        .expect("seeds a row");
    }
}

fn row(id: i64, title: &'static str, views: Option<i64>) -> (i64, &'static str, Option<i64>) {
    (id, title, views)
}

#[tokio::test]
async fn find_returns_the_row_or_none() {
    let Some(db) = connect().await else { return };
    setup(&db, Find::TABLE).await;
    seed(&db, Find::TABLE, &[(1, "first", Some(10))]).await;

    let found = Find::query().find(&db, 1).await.expect("finds");
    assert_eq!(
        found,
        Some(Find {
            id: 1,
            title: "first".into(),
            views: Some(10),
        })
    );
    assert_eq!(Find::query().find(&db, 99).await.expect("finds"), None);

    teardown(&db, Find::TABLE).await;
    db.close().await;
}

#[tokio::test]
async fn find_or_fail_errors_with_the_table_name() {
    let Some(db) = connect().await else { return };
    setup(&db, FindOrFail::TABLE).await;
    seed(&db, FindOrFail::TABLE, &[(1, "first", None)]).await;

    let found = FindOrFail::query()
        .find_or_fail(&db, 1)
        .await
        .expect("finds");
    assert_eq!(found.title, "first");

    let err = FindOrFail::query()
        .find_or_fail(&db, 99)
        .await
        .expect_err("a missing row is an error");
    assert!(matches!(
        err,
        Error::NotFound {
            table: "orbit_query_find_or_fail"
        }
    ));

    teardown(&db, FindOrFail::TABLE).await;
    db.close().await;
}

#[tokio::test]
async fn first_returns_the_row_in_the_order_given() {
    let Some(db) = connect().await else { return };
    setup(&db, First::TABLE).await;
    assert_eq!(First::query().first(&db).await.expect("queries"), None);

    seed(
        &db,
        First::TABLE,
        &[row(3, "c", None), row(1, "a", None), row(2, "b", None)],
    )
    .await;

    // Without order_by, first() has no order, so the row is one of the three.
    let any = First::query().first(&db).await.expect("queries");
    assert!(any.is_some());

    let first = Query::<First>::table()
        .order_by("id", Direction::Asc)
        .first(&db)
        .await
        .expect("queries");
    assert_eq!(first.map(|r| r.id), Some(1));

    let last = Query::<First>::table()
        .order_by("id", Direction::Desc)
        .first(&db)
        .await
        .expect("queries");
    assert_eq!(last.map(|r| r.id), Some(3));

    let first_or_fail = Query::<First>::table()
        .order_by("id", Direction::Asc)
        .first_or_fail(&db)
        .await
        .expect("finds");
    assert_eq!(first_or_fail.id, 1);

    teardown(&db, First::TABLE).await;
    db.close().await;
}

#[tokio::test]
async fn get_applies_where_order_limit_and_offset() {
    let Some(db) = connect().await else { return };
    setup(&db, Get::TABLE).await;
    seed(
        &db,
        Get::TABLE,
        &[
            row(1, "one", Some(1)),
            row(2, "two", Some(2)),
            row(3, "three", Some(3)),
            row(4, "four", Some(4)),
            row(5, "five", Some(5)),
        ],
    )
    .await;

    let all = Get::query().get(&db).await.expect("queries");
    assert_eq!(all.len(), 5);

    let page = Query::<Get>::table()
        .where_op("views", Operator::Ge, 2)
        .order_by("views", Direction::Desc)
        .limit(2)
        .offset(1)
        .get(&db)
        .await
        .expect("queries");
    let ids: Vec<i64> = page.iter().map(|r| r.id).collect();
    assert_eq!(ids, vec![4, 3]);

    let selected = Query::<Get>::table()
        .select(&["id", "title", "views"])
        .where_eq("title", "one")
        .get(&db)
        .await
        .expect("queries");
    assert_eq!(selected.len(), 1);

    teardown(&db, Get::TABLE).await;
    db.close().await;
}

#[tokio::test]
async fn where_in_where_null_and_where_not_null() {
    let Some(db) = connect().await else { return };
    setup(&db, Where::TABLE).await;
    seed(
        &db,
        Where::TABLE,
        &[row(1, "a", None), row(2, "b", Some(2)), row(3, "c", None)],
    )
    .await;

    let listed = Query::<Where>::table()
        .where_in("id", [1, 3])
        .order_by("id", Direction::Asc)
        .get(&db)
        .await
        .expect("queries");
    assert_eq!(listed.iter().map(|r| r.id).collect::<Vec<_>>(), vec![1, 3]);

    let none = Query::<Where>::table()
        .where_in("id", Vec::<i64>::new())
        .get(&db)
        .await
        .expect("queries");
    assert!(none.is_empty());

    let nulls = Query::<Where>::table()
        .where_null("views")
        .order_by("id", Direction::Asc)
        .get(&db)
        .await
        .expect("queries");
    assert_eq!(nulls.iter().map(|r| r.id).collect::<Vec<_>>(), vec![1, 3]);

    let by_eq_null = Query::<Where>::table()
        .where_eq("views", None::<i64>)
        .count(&db)
        .await
        .expect("queries");
    assert_eq!(by_eq_null, 2);

    let set = Query::<Where>::table()
        .where_not_null("views")
        .get(&db)
        .await
        .expect("queries");
    assert_eq!(set.iter().map(|r| r.id).collect::<Vec<_>>(), vec![2]);

    teardown(&db, Where::TABLE).await;
    db.close().await;
}

#[tokio::test]
async fn count_and_exists() {
    let Some(db) = connect().await else { return };
    setup(&db, Count::TABLE).await;
    seed(
        &db,
        Count::TABLE,
        &[row(1, "a", None), row(2, "b", Some(2)), row(3, "c", None)],
    )
    .await;

    assert_eq!(Count::query().count(&db).await.expect("counts"), 3);
    assert_eq!(
        Query::<Count>::table()
            .where_null("views")
            .count(&db)
            .await
            .expect("counts"),
        2
    );
    assert!(
        Count::query()
            .where_eq("id", 2)
            .exists(&db)
            .await
            .expect("checks")
    );
    assert!(
        !Count::query()
            .where_eq("id", 9)
            .exists(&db)
            .await
            .expect("checks")
    );

    teardown(&db, Count::TABLE).await;
    db.close().await;
}

#[tokio::test]
async fn insert_returns_the_auto_increment_id() {
    let Some(db) = connect().await else { return };
    setup(&db, Insert::TABLE).await;

    let first = Insert::query()
        .insert(&db, &[("title", "hello".into()), ("views", Value::from(7))])
        .await
        .expect("inserts");
    let second = Insert::query()
        .insert(&db, &[("title", "it's \"quoted\"".into())])
        .await
        .expect("inserts");
    assert_eq!((first, second), (1, 2));

    let saved = Insert::query().find(&db, 2).await.expect("finds");
    assert_eq!(
        saved,
        Some(Insert {
            id: 2,
            title: "it's \"quoted\"".into(),
            views: None,
        })
    );

    let not_fillable = Insert::query()
        .insert(&db, &[("id", Value::from(50))])
        .await
        .expect_err("id is not fillable");
    assert!(matches!(not_fillable, Error::NotFillable { .. }));

    teardown(&db, Insert::TABLE).await;
    db.close().await;
}

#[tokio::test]
async fn update_changes_only_the_matching_rows() {
    let Some(db) = connect().await else { return };
    setup(&db, Update::TABLE).await;
    seed(
        &db,
        Update::TABLE,
        &[
            row(1, "a", Some(1)),
            row(2, "b", Some(5)),
            row(3, "c", Some(9)),
        ],
    )
    .await;

    let changed = Query::<Update>::table()
        .where_op("views", Operator::Lt, 5)
        .update(&db, &[("title", "small".into())])
        .await
        .expect("updates");
    assert_eq!(changed, 1);

    let rows = Update::query()
        .order_by("id", Direction::Asc)
        .get(&db)
        .await
        .expect("queries");
    let titles: Vec<&str> = rows.iter().map(|r| r.title.as_str()).collect();
    assert_eq!(titles, vec!["small", "b", "c"]);

    let none = Update::query()
        .update(&db, &[])
        .await
        .expect("an empty update is a no-op");
    assert_eq!(none, 0);

    teardown(&db, Update::TABLE).await;
    db.close().await;
}

#[tokio::test]
async fn delete_removes_only_the_matching_rows() {
    let Some(db) = connect().await else { return };
    setup(&db, Delete::TABLE).await;
    seed(
        &db,
        Delete::TABLE,
        &[row(1, "a", None), row(2, "b", Some(2)), row(3, "c", None)],
    )
    .await;

    let removed = Query::<Delete>::table()
        .where_null("views")
        .delete(&db)
        .await
        .expect("deletes");
    assert_eq!(removed, 2);
    assert_eq!(Delete::query().count(&db).await.expect("counts"), 1);

    teardown(&db, Delete::TABLE).await;
    db.close().await;
}

#[tokio::test]
async fn values_are_bound_not_spliced() {
    let Some(db) = connect().await else { return };
    setup(&db, Inject::TABLE).await;
    seed(&db, Inject::TABLE, &[row(1, "alice", Some(1))]).await;

    let attack = "x' OR '1'='1";
    let matched = Query::<Inject>::table()
        .where_eq("title", attack)
        .count(&db)
        .await
        .expect("queries");
    assert_eq!(matched, 0);

    let payload = "O'Brien \"x\"; DROP TABLE orbit_query_inject; --";
    Inject::query()
        .insert(&db, &[("title", payload.into())])
        .await
        .expect("inserts");
    let saved = Inject::query().find(&db, 2).await.expect("finds");
    assert_eq!(saved.map(|r| r.title), Some(payload.to_owned()));
    assert_eq!(Inject::query().count(&db).await.expect("counts"), 2);

    teardown(&db, Inject::TABLE).await;
    db.close().await;
}

#[tokio::test]
async fn an_invalid_identifier_is_rejected() {
    let Some(db) = connect().await else { return };

    let empty = Find::query()
        .where_eq("", 1)
        .get(&db)
        .await
        .expect_err("empty");
    assert!(matches!(empty, Error::InvalidIdentifier(ref name) if name.is_empty()));

    let with_tick = Find::query()
        .where_eq("bad`column", 1)
        .get(&db)
        .await
        .expect_err("backtick");
    assert!(matches!(with_tick, Error::InvalidIdentifier(ref name) if name == "bad`column"));

    let bad_order = Find::query()
        .order_by("a`b", Direction::Asc)
        .first(&db)
        .await
        .expect_err("backtick in order_by");
    assert!(matches!(bad_order, Error::InvalidIdentifier(_)));

    let bad_table = BadTable::query()
        .count(&db)
        .await
        .expect_err("backtick in table");
    assert!(matches!(bad_table, Error::InvalidIdentifier(ref name) if name == "orbit`bad"));

    db.close().await;
}
