//! Runs `replace` against a real database. Set `ORBIT_DATABASE_URL` to a test database, such as
//! `mysql://user@127.0.0.1:3306/orbit_test`. The tests are skipped with a notice when it is not
//! set. Each test uses its own table and drops it at the end.
//!
//! `replace` is `REPLACE INTO` on MySQL and SQLite. PostgreSQL has no such statement, so `replace`
//! is refused there with `Error::Unsupported`, and each test asserts that instead.

mod common;

use orbit_orm::{Database, Error, Model, OrbitModel, Query};

async fn connect() -> Option<Database> {
    let url = common::url()?;
    Some(Database::connect(&url).await.expect("connects"))
}

/// The error PostgreSQL gives for `replace`.
fn assert_postgres_refuses(result: Result<u64, Error>) {
    assert!(
        matches!(
            result,
            Err(Error::Unsupported {
                operation: "replace",
                driver: "PostgreSQL"
            })
        ),
        "expected Unsupported on PostgreSQL, got {result:?}"
    );
}

/// Each test has its own table, since the tests run in parallel.
macro_rules! item {
    ($name:ident, $table:literal) => {
        #[derive(Debug, PartialEq, OrbitModel)]
        #[orbit(table = $table)]
        struct $name {
            id: i64,
            #[orbit(fillable)]
            code: String,
            #[orbit(fillable)]
            title: String,
            #[orbit(fillable)]
            note: Option<String>,
        }
    };
}

item!(Overwritten, "orbit_replace_overwrite");
item!(Inserted, "orbit_replace_insert");
item!(Checked, "orbit_replace_fillable");

/// Recreates `table` with a unique `code`, a `title`, and a `note` that defaults to 'none'.
async fn setup(db: &Database, table: &str) {
    common::drop_tables(db, &[table]).await;
    common::exec(
        db,
        &format!(
            "CREATE TABLE `{table}` (
                id BIGINT AUTO_INCREMENT PRIMARY KEY,
                code VARCHAR(32) NOT NULL,
                title VARCHAR(255) NOT NULL,
                note VARCHAR(255) NULL DEFAULT 'none',
                UNIQUE KEY `{table}_code` (code)
            )"
        ),
    )
    .await;
}

#[tokio::test]
async fn replace_overwrites_the_row_on_a_unique_key_and_changes_its_id() {
    let Some(db) = connect().await else { return };
    setup(&db, Overwritten::TABLE).await;

    let first = Query::<Overwritten>::table()
        .insert(
            &db,
            &[
                ("code", "a".into()),
                ("title", "first".into()),
                ("note", "kept?".into()),
            ],
        )
        .await
        .expect("inserts");

    let result = Query::<Overwritten>::table()
        .replace(&db, &[("code", "a".into()), ("title", "second".into())])
        .await;
    if common::on_postgres(db.driver()) {
        assert_postgres_refuses(result);
        common::drop_tables(&db, &[Overwritten::TABLE]).await;
        db.close().await;
        return;
    }
    let affected = result.expect("replaces");
    // MySQL deletes the old row and inserts the new one, so two rows are affected. SQLite's
    // REPLACE does the same delete and insert but reports one row.
    if common::on_mysql(db.driver()) {
        assert_eq!(affected, 2);
    } else {
        assert_eq!(affected, 1);
    }

    let row = Overwritten::query()
        .where_eq("code", "a")
        .first_or_fail(&db)
        .await
        .expect("finds the replaced row");
    assert_ne!(row.id, first as i64, "the id changes");
    assert_eq!(row.title, "second");
    assert_eq!(
        row.note.as_deref(),
        Some("none"),
        "a column not supplied takes its default"
    );
    assert_eq!(Overwritten::query().count(&db).await.expect("counts"), 1);

    common::drop_tables(&db, &[Overwritten::TABLE]).await;
    db.close().await;
}

#[tokio::test]
async fn replace_inserts_when_no_row_conflicts() {
    let Some(db) = connect().await else { return };
    setup(&db, Inserted::TABLE).await;

    Query::<Inserted>::table()
        .insert(&db, &[("code", "a".into()), ("title", "first".into())])
        .await
        .expect("inserts");

    let result = Query::<Inserted>::table()
        .replace(&db, &[("code", "b".into()), ("title", "other".into())])
        .await;
    if common::on_postgres(db.driver()) {
        assert_postgres_refuses(result);
        common::drop_tables(&db, &[Inserted::TABLE]).await;
        db.close().await;
        return;
    }
    let affected = result.expect("replaces");
    assert_eq!(affected, 1, "a plain insert affects one row");

    assert_eq!(Inserted::query().count(&db).await.expect("counts"), 2);
    let kept = Inserted::query()
        .where_eq("code", "a")
        .first_or_fail(&db)
        .await
        .expect("the other row is untouched");
    assert_eq!(kept.title, "first");

    common::drop_tables(&db, &[Inserted::TABLE]).await;
    db.close().await;
}

#[tokio::test]
async fn replace_checks_fillable_like_insert() {
    let Some(db) = connect().await else { return };
    setup(&db, Checked::TABLE).await;

    let result = Query::<Checked>::table()
        .replace(&db, &[("id", 5.into())])
        .await;
    if common::on_postgres(db.driver()) {
        // PostgreSQL refuses replace before it reads the columns, so fillable is not reached.
        assert_postgres_refuses(result);
    } else {
        assert!(matches!(result, Err(Error::NotFillable { .. })));
    }

    common::drop_tables(&db, &[Checked::TABLE]).await;
    db.close().await;
}
