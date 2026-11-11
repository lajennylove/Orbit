//! Runs against a real database: MySQL or MariaDB, PostgreSQL or SQLite. Set `ORBIT_DATABASE_URL`
//! to a test database, such as `mysql://user@127.0.0.1:3306/orbit_test`. The tests are skipped
//! with a notice when it is not set. Each test uses its own table and drops it at the end.

mod common;

use std::sync::Arc;

use orbit_orm::{Database, Error, Model, raw};
use sqlx::{FromRow, Row};
use tokio::sync::{Mutex, MutexGuard};

/// The tests share one database, so they run one at a time, on every driver, as the migration and
/// history tests do. On SQLite, a transaction that holds a read lock can wait on a writer in
/// another test while that writer waits for the read lock to clear. Both then fail after the busy
/// timeout, which made the transaction tests fail now and then when they ran in parallel.
static LOCK: Mutex<()> = Mutex::const_new(());

/// Connects and takes the lock. The lock is held until the test ends.
async fn connect() -> Option<(MutexGuard<'static, ()>, Database)> {
    let url = common::url()?;
    let guard = LOCK.lock().await;
    Some((guard, Database::connect(&url).await.expect("connects")))
}

/// Declares a row type and its model on the given table.
macro_rules! item {
    ($name:ident, $table:literal) => {
        #[derive(Debug, PartialEq)]
        struct $name {
            id: i64,
            title: String,
        }

        impl<'r, R> FromRow<'r, R> for $name
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
    common::drop_tables(db, &[table]).await;
    common::exec(
        db,
        &format!(
            "CREATE TABLE `{table}` (
                id BIGINT AUTO_INCREMENT PRIMARY KEY,
                title VARCHAR(255) NOT NULL
            )"
        ),
    )
    .await;
}

async fn teardown(db: &Database, table: &str) {
    common::drop_tables(db, &[table]).await;
}

#[tokio::test]
async fn commits_when_the_closure_returns_ok() {
    let Some((_lock, db)) = connect().await else {
        return;
    };
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
    let Some((_lock, db)) = connect().await else {
        return;
    };
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
    let Some((_lock, db)) = connect().await else {
        return;
    };
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
    let Some((_lock, db)) = connect().await else {
        return;
    };
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
    let Some((_lock, db)) = connect().await else {
        return;
    };
    setup(&db, Locked::TABLE).await;
    common::exec(
        &db,
        &format!(
            "INSERT INTO `{}` (id, title) VALUES (1, 'a')",
            Locked::TABLE
        ),
    )
    .await;
    let outside = &db;
    let driver = db.driver();

    let outer = db
        .transaction(async |tx| {
            let row = Locked::query()
                .where_eq("id", 1)
                .for_update()
                .first_or_fail(&mut *tx)
                .await?;
            assert_eq!(row.title, "a");
            // PostgreSQL refuses FOR UPDATE with an aggregate such as COUNT or EXISTS, so those
            // checks run only where the database accepts them. SQLite has no row locks, so the
            // clause is left out and the checks are plain reads.
            if !common::on_postgres(driver) {
                assert!(
                    Locked::query()
                        .where_eq("id", 1)
                        .for_update()
                        .exists(&mut *tx)
                        .await?
                );
                assert_eq!(Locked::query().for_update().count(&mut *tx).await?, 1);
            }

            if common::on_sqlite(driver) {
                // SQLite has no row locks: FOR UPDATE is a no-op, so a second transaction reads
                // the row at once instead of waiting for the lock.
                let read = outside
                    .transaction(async |inner| {
                        Locked::query()
                            .where_eq("id", 1)
                            .for_update()
                            .first_or_fail(&mut *inner)
                            .await
                    })
                    .await
                    .expect("SQLite reads without a row lock");
                assert_eq!(read.title, "a");
                return Ok::<(), Error>(());
            }

            // A second transaction on another connection waits for the lock, then gives up. The
            // wait is bounded by the database's lock timeout, set for this transaction only.
            let blocked = outside
                .transaction(async |inner| {
                    if common::on_mysql(driver) {
                        raw::execute(&mut *inner, "SET SESSION innodb_lock_wait_timeout = 1", &[])
                            .await?;
                    } else {
                        raw::execute(&mut *inner, "SET LOCAL lock_timeout = '1s'", &[]).await?;
                    }
                    let attempt = Locked::query()
                        .where_eq("id", 1)
                        .for_update()
                        .first(&mut *inner)
                        .await
                        .map(|_| ());
                    // On PostgreSQL a failed statement aborts the transaction, so the reset
                    // runs only on MySQL, where it is still allowed.
                    if common::on_mysql(driver) {
                        raw::execute(
                            &mut *inner,
                            "SET SESSION innodb_lock_wait_timeout = DEFAULT",
                            &[],
                        )
                        .await?;
                    }
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
async fn for_update_outside_a_transaction_runs_in_autocommit() {
    let Some((_lock, db)) = connect().await else {
        return;
    };
    setup(&db, LockOutside::TABLE).await;
    common::exec(
        &db,
        &format!(
            "INSERT INTO `{}` (id, title) VALUES (1, 'a')",
            LockOutside::TABLE
        ),
    )
    .await;

    // On the pool, FOR UPDATE runs in autocommit and succeeds, as Laravel's lockForUpdate does.
    let fetched = LockOutside::query()
        .where_eq("id", 1)
        .for_update()
        .first(&db)
        .await
        .expect("the locked read succeeds");
    assert_eq!(fetched.map(|row| row.title), Some("a".to_owned()));

    let all = LockOutside::query()
        .for_update()
        .get(&db)
        .await
        .expect("the locked read succeeds");
    assert_eq!(all.len(), 1);

    // PostgreSQL refuses FOR UPDATE with COUNT and EXISTS, so only the other databases check them.
    if !common::on_postgres(db.driver()) {
        assert_eq!(
            LockOutside::query()
                .for_update()
                .count(&db)
                .await
                .expect("the locked count succeeds"),
            1
        );
        assert!(
            LockOutside::query()
                .where_eq("id", 1)
                .for_update()
                .exists(&db)
                .await
                .expect("the locked exists succeeds")
        );
    }

    teardown(&db, LockOutside::TABLE).await;
    db.close().await;
}

#[tokio::test]
async fn the_transaction_connection_runs_raw_sql() {
    let Some((_lock, db)) = connect().await else {
        return;
    };
    setup(&db, Raw::TABLE).await;
    let db_driver = db.driver();

    let counted: i64 = db
        .transaction(async |tx| {
            let n: i64 = raw::fetch_scalar(
                &mut *tx,
                &common::sql(db_driver, &format!("SELECT COUNT(*) FROM `{}`", Raw::TABLE)),
                &[],
            )
            .await?;
            Ok::<_, Error>(n)
        })
        .await
        .expect("queries");
    assert_eq!(counted, 0);

    teardown(&db, Raw::TABLE).await;
    db.close().await;
}
