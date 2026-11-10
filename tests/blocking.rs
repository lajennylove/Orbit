//! Runs the blocking API against a real MySQL or MariaDB server. Set `ORBIT_DATABASE_URL` to a
//! test database, such as `mysql://user:password@127.0.0.1:3306/orbit_test`. The tests are
//! skipped with a notice when it is not set. Each test uses its own table and drops it at the end.

use orbit_orm::blocking::Database;
use orbit_orm::{Error, Model, OrbitModel};
use sqlx::AssertSqlSafe;

fn connect() -> Option<Database> {
    let Ok(url) = std::env::var("ORBIT_DATABASE_URL") else {
        eprintln!("skipped: set ORBIT_DATABASE_URL to run the database tests");
        return None;
    };
    Some(Database::connect(&url).expect("connects"))
}

#[derive(Debug, PartialEq, OrbitModel)]
#[orbit(table = "orbit_blocking_notes")]
struct Note {
    id: i64,
    #[orbit(fillable)]
    title: String,
}

#[derive(Debug, PartialEq, OrbitModel)]
#[orbit(table = "orbit_blocking_tx_commit")]
struct Committed {
    id: i64,
    #[orbit(fillable)]
    title: String,
}

#[derive(Debug, PartialEq, OrbitModel)]
#[orbit(table = "orbit_blocking_tx_rollback")]
struct RolledBack {
    id: i64,
    #[orbit(fillable)]
    title: String,
}

#[derive(Debug, PartialEq, OrbitModel)]
#[orbit(table = "orbit_blocking_spawn")]
struct Spawned {
    id: i64,
    #[orbit(fillable)]
    title: String,
}

#[derive(Debug, PartialEq, OrbitModel)]
#[orbit(table = "orbit_blocking_codes")]
struct Code {
    id: i64,
    #[orbit(fillable)]
    code: String,
    #[orbit(fillable)]
    title: String,
}

/// Runs a statement on the pool, through `run`, since sqlx is async.
fn run_sql(db: &Database, sql: &str) {
    db.run(sqlx::query(AssertSqlSafe(sql.to_owned())).execute(db.pool()))
        .expect("runs on the blocking database")
        .expect("runs sql");
}

/// Recreates `table` with an id and a title, as the models above expect.
fn setup(db: &Database, table: &str) {
    run_sql(db, &format!("DROP TABLE IF EXISTS `{table}`"));
    run_sql(
        db,
        &format!(
            "CREATE TABLE `{table}` (
                id BIGINT AUTO_INCREMENT PRIMARY KEY,
                title VARCHAR(255) NOT NULL
            )"
        ),
    );
}

fn teardown(db: &Database, table: &str) {
    run_sql(db, &format!("DROP TABLE IF EXISTS `{table}`"));
}

#[test]
fn connects_and_answers_a_ping() {
    let Some(db) = connect() else { return };
    db.ping().expect("answers SELECT 1");
    db.close().expect("closes");
}

#[test]
fn a_wrong_password_is_an_error() {
    let Ok(url) = std::env::var("ORBIT_DATABASE_URL") else {
        return;
    };
    let wrong = url.replacen("://", "://orbit-no-such-user:wrong@", 1);
    assert!(Database::connect(&wrong).is_err());
}

#[test]
fn finds_a_row_and_inserts_one() {
    let Some(db) = connect() else { return };
    setup(&db, Note::TABLE);

    let id = Note::query()
        .blocking()
        .insert(&db, &[("title", "hello".into())])
        .expect("inserts");
    let found = Note::query().blocking().find(&db, id).expect("finds");
    assert_eq!(
        found,
        Some(Note {
            id: id as i64,
            title: "hello".to_owned()
        })
    );
    assert_eq!(
        Note::query()
            .blocking()
            .find(&db, 999)
            .expect("finds nothing"),
        None
    );
    assert!(matches!(
        Note::query().blocking().find_or_fail(&db, 999),
        Err(Error::NotFound { .. })
    ));

    teardown(&db, Note::TABLE);
    db.close().expect("closes");
}

#[test]
fn transaction_commits_when_the_closure_returns_ok() {
    let Some(db) = connect() else { return };
    setup(&db, Committed::TABLE);

    let id = db
        .transaction(|tx| {
            let id = Committed::query()
                .blocking()
                .insert(tx, &[("title", "kept".into())])?;
            Ok::<_, Error>(id)
        })
        .expect("commits");

    let saved = Committed::query().blocking().find(&db, id).expect("finds");
    assert_eq!(saved.map(|row| row.title), Some("kept".to_owned()));

    teardown(&db, Committed::TABLE);
    db.close().expect("closes");
}

#[test]
fn transaction_rolls_back_when_the_closure_returns_err() {
    let Some(db) = connect() else { return };
    setup(&db, RolledBack::TABLE);

    let result = db.transaction(|tx| {
        RolledBack::query()
            .blocking()
            .insert(tx, &[("title", "dropped".into())])?;
        Err::<(), _>(Error::NotFound {
            table: RolledBack::TABLE,
        })
    });
    assert!(matches!(result, Err(Error::NotFound { .. })));
    assert_eq!(
        RolledBack::query().blocking().count(&db).expect("counts"),
        0
    );

    teardown(&db, RolledBack::TABLE);
    db.close().expect("closes");
}

#[test]
fn replace_overwrites_on_a_unique_key() {
    let Some(db) = connect() else { return };
    run_sql(&db, "DROP TABLE IF EXISTS orbit_blocking_codes");
    run_sql(
        &db,
        "CREATE TABLE orbit_blocking_codes (
            id BIGINT AUTO_INCREMENT PRIMARY KEY,
            code VARCHAR(32) NOT NULL,
            title VARCHAR(255) NOT NULL,
            UNIQUE KEY orbit_blocking_codes_code (code)
        )",
    );

    Code::query()
        .blocking()
        .insert(&db, &[("code", "a".into()), ("title", "first".into())])
        .expect("inserts");
    let affected = Code::query()
        .blocking()
        .replace(&db, &[("code", "a".into()), ("title", "second".into())])
        .expect("replaces");
    assert_eq!(affected, 2);

    let row = Code::query()
        .where_eq("code", "a")
        .blocking()
        .first_or_fail(&db)
        .expect("finds the replaced row");
    assert_eq!(row.title, "second");

    run_sql(&db, "DROP TABLE IF EXISTS orbit_blocking_codes");
    db.close().expect("closes");
}

#[test]
fn a_blocking_call_on_an_async_thread_is_an_error() {
    let Ok(url) = std::env::var("ORBIT_DATABASE_URL") else {
        eprintln!("skipped: set ORBIT_DATABASE_URL to run the database tests");
        return;
    };
    let Some(db) = connect() else { return };

    let runtime = tokio::runtime::Runtime::new().expect("builds a runtime");
    runtime.block_on(async {
        assert!(matches!(db.ping(), Err(Error::BlockingInRuntime)));
        assert!(matches!(
            Note::query().blocking().count(&db),
            Err(Error::BlockingInRuntime)
        ));
        assert!(matches!(
            db.transaction(|_| Ok::<(), Error>(())),
            Err(Error::BlockingInRuntime)
        ));
        assert!(matches!(
            Database::connect(&url),
            Err(Error::BlockingInRuntime)
        ));
    });

    db.close().expect("closes");
}

#[test]
fn spawn_blocking_runs_blocking_calls() {
    let Some(db) = connect() else { return };
    setup(&db, Spawned::TABLE);
    let runtime = tokio::runtime::Runtime::new().expect("builds a runtime");

    let (db, id) = runtime.block_on(async {
        tokio::task::spawn_blocking(move || {
            let id = Spawned::query()
                .blocking()
                .insert(&db, &[("title", "from a blocking thread".into())])
                .expect("inserts on a blocking thread");
            (db, id)
        })
        .await
        .expect("the blocking task finishes")
    });

    let (db, found) = runtime.block_on(async {
        tokio::task::spawn_blocking(move || {
            let found = Spawned::query().blocking().find(&db, id).expect("finds");
            (db, found)
        })
        .await
        .expect("the blocking task finishes")
    });
    assert_eq!(
        found.map(|row| row.title),
        Some("from a blocking thread".to_owned())
    );

    teardown(&db, Spawned::TABLE);
}
