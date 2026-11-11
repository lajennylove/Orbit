//! Runs against a real database: MySQL or MariaDB, PostgreSQL or SQLite. Set `ORBIT_DATABASE_URL`
//! to a test database, such as `mysql://user@127.0.0.1:3306/orbit_test`. The tests are skipped
//! with a notice when it is not set.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use orbit_orm::{Database, Error, PoolOptions};

#[tokio::test]
async fn connects_and_answers_a_ping() {
    let Some(url) = common::url() else {
        eprintln!("skipped: set ORBIT_DATABASE_URL to run the database tests");
        return;
    };
    let db = Database::connect(&url).await.expect("connects");
    db.ping().await.expect("answers SELECT 1");
    db.close().await;
}

#[tokio::test]
async fn a_wrong_password_is_an_error() {
    let Some(url) = common::url() else {
        return;
    };
    // SQLite has no users or passwords, so there is no login to get wrong.
    if url.starts_with("sqlite:") {
        return;
    }
    let wrong = url.replacen("://", "://orbit-no-such-user:wrong@", 1);
    assert!(Database::connect(&wrong).await.is_err());
}

#[test]
fn the_default_pool_options_match_connect() {
    assert_eq!(
        PoolOptions::default(),
        PoolOptions {
            max_connections: 5,
            acquire_timeout: Duration::from_secs(10),
            min_connections: 0,
        }
    );
}

#[tokio::test]
async fn max_connections_and_acquire_timeout_are_applied() {
    let Some(url) = common::url() else {
        return;
    };
    let options = PoolOptions {
        max_connections: 2,
        acquire_timeout: Duration::from_secs(1),
        ..PoolOptions::default()
    };
    let db = Arc::new(
        Database::connect_with(&url, options)
            .await
            .expect("connects"),
    );

    // Two open transactions hold both connections the pool may open. Each one waits on a release
    // signal, so it keeps its connection until the test lets it go. A third transaction then
    // waits for the timeout, which is driver-neutral: it does not depend on the pool's internals.
    let (release, _) = tokio::sync::watch::channel(false);
    let mut ready = Vec::new();
    let mut holders = Vec::new();
    for _ in 0..2 {
        let db = Arc::clone(&db);
        let mut released = release.subscribe();
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        ready.push(ready_rx);
        holders.push(tokio::spawn(async move {
            db.transaction(async |_tx| {
                let _ = ready_tx.send(());
                while !*released.borrow_and_update() {
                    released.changed().await.expect("the test is still running");
                }
                Ok::<(), Error>(())
            })
            .await
        }));
    }
    for signal in ready {
        signal.await.expect("a transaction holds a connection");
    }

    let started = Instant::now();
    let err = db
        .transaction(async |_tx| Ok::<(), Error>(()))
        .await
        .expect_err("the third request times out");
    let waited = started.elapsed();
    assert!(
        matches!(err, Error::Database(sqlx::Error::PoolTimedOut)),
        "got {err:?}"
    );
    assert!(waited >= Duration::from_secs(1), "waited {waited:?}");
    assert!(waited < Duration::from_secs(5), "waited {waited:?}");

    // Once a connection is released, the next request gets it.
    release.send(true).expect("the holders are listening");
    for holder in holders {
        holder
            .await
            .expect("the holder task finishes")
            .expect("the holder commits");
    }
    db.transaction(async |_tx| Ok::<(), Error>(()))
        .await
        .expect("a released connection is reused");
    db.close().await;
}
