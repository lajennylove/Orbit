//! Runs against a real MySQL or MariaDB server. Set `ORBIT_DATABASE_URL` to a test database,
//! such as `mysql://user:password@127.0.0.1:3306/orbit_test`. The tests are skipped with a
//! notice when it is not set. Each test uses its own table and drops it at the end.

use std::sync::Arc;

use orbit_orm::{Database, Error, Model};
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
macro_rules! item {
    ($name:ident, $table:literal) => {
        #[derive(Debug, PartialEq)]
        struct $name {
            id: i64,
            title: String,
        }

        impl<'r> FromRow<'r, MySqlRow> for $name {
            fn from_row(row: &'r MySqlRow) -> Result<Self, sqlx::Error> {
                Ok(Self {
                    id: row.try_get("id")?,
                    title: row.try_get("title")?,
                })
            }
        }

        impl Model for $name {
            const TABLE: &'static str = $table;
            const FILLABLE: &'static [&'static str] = &["title"];
        }
    };
}

item!(Commit, "orbit_tx_commit");
item!(Rollback, "orbit_tx_rollback");
item!(Panicking, "orbit_tx_panic");
item!(Own, "orbit_tx_own_writes");
item!(Raw, "orbit_tx_raw");
item!(Locked, "orbit_tx_lock");
item!(LockOutside, "orbit_tx_lock_outside");

async fn setup(db: &Database, table: &str) {
    sqlx::query(AssertSqlSafe(format!("DROP TABLE IF EXISTS `{table}`")))
        .execute(db.pool())
        .await
        .expect("drops a leftover table");
    sqlx::query(AssertSqlSafe(format!(
        "CREATE TABLE `{table}` (
            id BIGINT AUTO_INCREMENT PRIMARY KEY,
            title VARCHAR(255) NOT NULL
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

#[tokio::test]
async fn commits_when_the_closure_returns_ok() {
    let Some(db) = connect().await else { return };
    setup(&db, Commit::TABLE).await;

    let id = db
        .transaction(async |tx| {
            let id = Commit::query()
                .insert(&mut *tx, &[("title", "kept".into())])
                .await?;
            Ok::<_, Error>(id)
        })
        .await
        .expect("commits");

    let saved = Commit::query().find(&db, id).await.expect("finds");
    assert_eq!(saved.map(|r| r.title), Some("kept".to_owned()));

    teardown(&db, Commit::TABLE).await;
    db.close().await;
}

#[tokio::test]
async fn rolls_back_when_the_closure_returns_err() {
    let Some(db) = connect().await else { return };
    setup(&db, Rollback::TABLE).await;

    let result = db
        .transaction(async |tx| {
            Rollback::query()
                .insert(&mut *tx, &[("title", "dropped".into())])
                .await?;
            Err::<(), _>(Error::NotFound {
                table: Rollback::TABLE,
            })
        })
        .await;
    assert!(matches!(result, Err(Error::NotFound { .. })));

    assert_eq!(Rollback::query().count(&db).await.expect("counts"), 0);

    teardown(&db, Rollback::TABLE).await;
    db.close().await;
}

#[tokio::test]
async fn a_panic_rolls_back_and_does_not_commit() {
    let Some(db) = connect().await else { return };
    setup(&db, Panicking::TABLE).await;
    let db = Arc::new(db);

    // The panic runs in a spawned task, so the test sees it as a join error instead of a crash.
    let shared = Arc::clone(&db);
    let task = tokio::spawn(async move {
        shared
            .transaction(async |tx| {
                Panicking::query()
                    .insert(&mut *tx, &[("title", "never".into())])
                    .await?;
                panic!("panic inside the transaction");
                #[allow(unreachable_code)]
                Ok::<(), Error>(())
            })
            .await
    });
    let joined = task.await;
    assert!(joined.expect_err("the task panicked").is_panic());

    assert_eq!(Panicking::query().count(&*db).await.expect("counts"), 0);

    teardown(&db, Panicking::TABLE).await;
}

#[tokio::test]
async fn queries_in_the_transaction_see_their_own_writes() {
    let Some(db) = connect().await else { return };
    setup(&db, Own::TABLE).await;
    let outside = &db;

    db.transaction(async |tx| {
        Own::query()
            .insert(&mut *tx, &[("title", "inside".into())])
            .await?;

        let inside = Own::query().count(&mut *tx).await?;
        assert_eq!(inside, 1, "the transaction sees its own insert");

        let from_pool = Own::query().count(outside).await?;
        assert_eq!(from_pool, 0, "the pool does not see the uncommitted insert");
        Ok::<(), Error>(())
    })
    .await
    .expect("commits");

    assert_eq!(Own::query().count(&db).await.expect("counts"), 1);

    teardown(&db, Own::TABLE).await;
    db.close().await;
}

#[tokio::test]
async fn for_update_locks_the_row_for_other_transactions() {
    let Some(db) = connect().await else { return };
    setup(&db, Locked::TABLE).await;
    sqlx::query(AssertSqlSafe(format!(
        "INSERT INTO `{}` (id, title) VALUES (1, 'a')",
        Locked::TABLE
    )))
    .execute(db.pool())
    .await
    .expect("seeds a row");
    let outside = &db;

    let outer = db
        .transaction(async |tx| {
            let row = Locked::query()
                .where_eq("id", 1)
                .for_update()
                .first_or_fail(&mut *tx)
                .await?;
            assert_eq!(row.title, "a");
            assert!(
                Locked::query()
                    .where_eq("id", 1)
                    .for_update()
                    .exists(&mut *tx)
                    .await?
            );
            assert_eq!(Locked::query().for_update().count(&mut *tx).await?, 1);

            // A second transaction on another connection waits for the lock, then gives up.
            let blocked = outside
                .transaction(async |inner| {
                    sqlx::query("SET SESSION innodb_lock_wait_timeout = 1")
                        .execute(inner.connection())
                        .await?;
                    let attempt = Locked::query()
                        .where_eq("id", 1)
                        .for_update()
                        .first(&mut *inner)
                        .await
                        .map(|_| ());
                    sqlx::query("SET SESSION innodb_lock_wait_timeout = DEFAULT")
                        .execute(inner.connection())
                        .await?;
                    attempt
                })
                .await;
            assert!(matches!(blocked, Err(Error::LockWaitTimeout)));
            Ok::<(), Error>(())
        })
        .await;
    outer.expect("the outer transaction commits");

    teardown(&db, Locked::TABLE).await;
    db.close().await;
}

#[tokio::test]
async fn for_update_outside_a_transaction_is_an_error() {
    let Some(db) = connect().await else { return };
    setup(&db, LockOutside::TABLE).await;

    let fetched = LockOutside::query()
        .where_eq("id", 1)
        .for_update()
        .first(&db)
        .await
        .expect_err("for_update needs a transaction");
    assert!(matches!(fetched, Error::LockOutsideTransaction));

    let counted = LockOutside::query()
        .for_update()
        .count(&db)
        .await
        .expect_err("for_update needs a transaction");
    assert!(matches!(counted, Error::LockOutsideTransaction));

    // Nothing ran, so the table is still empty.
    assert_eq!(LockOutside::query().count(&db).await.expect("counts"), 0);

    teardown(&db, LockOutside::TABLE).await;
    db.close().await;
}

#[tokio::test]
async fn the_transaction_connection_runs_raw_sql() {
    let Some(db) = connect().await else { return };
    setup(&db, Raw::TABLE).await;

    let counted: i64 = db
        .transaction(async |tx| {
            let n: i64 = sqlx::query_scalar(AssertSqlSafe(format!(
                "SELECT COUNT(*) FROM `{}`",
                Raw::TABLE
            )))
            .fetch_one(tx.connection())
            .await?;
            Ok::<_, Error>(n)
        })
        .await
        .expect("queries");
    assert_eq!(counted, 0);

    teardown(&db, Raw::TABLE).await;
    db.close().await;
}
