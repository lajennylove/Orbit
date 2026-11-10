//! Runs against a real MySQL or MariaDB server. Set `ORBIT_DATABASE_URL` to a test database,
//! such as `mysql://user:password@127.0.0.1:3306/orbit_test`. The tests are skipped with a
//! notice when it is not set.

use std::time::{Duration, Instant};

use orbit_orm::{Database, PoolOptions};

fn test_url() -> Option<String> {
    std::env::var("ORBIT_DATABASE_URL").ok()
}

#[tokio::test]
async fn connects_and_answers_a_ping() {
    let Some(url) = test_url() else {
        eprintln!("skipped: set ORBIT_DATABASE_URL to run the database tests");
        return;
    };
    let db = Database::connect(&url).await.expect("connects");
    db.ping().await.expect("answers SELECT 1");
    db.close().await;
}

#[tokio::test]
async fn a_wrong_password_is_an_error() {
    let Some(url) = test_url() else {
        return;
    };
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
    let Some(url) = test_url() else {
        return;
    };
    let options = PoolOptions {
        max_connections: 2,
        acquire_timeout: Duration::from_secs(1),
        ..PoolOptions::default()
    };
    let db = Database::connect_with(&url, options)
        .await
        .expect("connects");

    // Two connections are the most the pool opens, so a third request waits for the timeout.
    let first = db.pool().acquire().await.expect("first connection");
    let second = db.pool().acquire().await.expect("second connection");
    let started = Instant::now();
    let err = db
        .pool()
        .acquire()
        .await
        .expect_err("the third request times out");
    let waited = started.elapsed();
    assert!(matches!(err, sqlx::Error::PoolTimedOut));
    assert!(waited >= Duration::from_secs(1), "waited {waited:?}");
    assert!(waited < Duration::from_secs(5), "waited {waited:?}");

    // A released connection goes to the next request.
    drop(first);
    db.pool()
        .acquire()
        .await
        .expect("a released connection is reused");
    drop(second);
    db.close().await;
}
