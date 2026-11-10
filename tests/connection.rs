//! Runs against a real MySQL or MariaDB server. Set `ORBIT_DATABASE_URL` to a test database,
//! such as `mysql://user:password@127.0.0.1:3306/orbit_test`. The tests are skipped with a
//! notice when it is not set.

use orbit_orm::Database;

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
