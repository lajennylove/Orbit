//! Runs against a real database: MySQL or MariaDB, PostgreSQL or SQLite. Set `ORBIT_DATABASE_URL`
//! to a test database, such as `mysql://user@127.0.0.1:3306/orbit_test`. The tests are skipped
//! with a notice when it is not set. Each test uses its own tables and drops them at the end.

mod common;

use orbit_orm::{Database, Error, Model, Value};
use sqlx::{FromRow, Row};

async fn connect() -> Option<Database> {
    let url = common::url()?;
    Some(Database::connect(&url).await.expect("connects"))
}

/// A model with an `id` and one fillable column, which the tests insert into.
macro_rules! model {
    ($name:ident, $table:literal, $column:literal) => {
        #[derive(Debug)]
        #[allow(dead_code)]
        struct $name {
            id: i64,
        }

        impl<'r, R> FromRow<'r, R> for $name
        where
            R: Row,
            &'r str: sqlx::ColumnIndex<R>,
            i64: sqlx::Decode<'r, R::Database> + sqlx::Type<R::Database>,
        {
            fn from_row(row: &'r R) -> Result<Self, sqlx::Error> {
                Ok(Self {
                    id: row.try_get("id")?,
                })
            }
        }

        impl Model for $name {
            const TABLE: &'static str = $table;
            const FILLABLE: &'static [&'static str] = &[$column];
        }
    };
}

model!(Email, "orbit_err_users", "email");
model!(Parent, "orbit_err_parent", "name");
model!(Child, "orbit_err_child", "parent_id");
model!(Missing, "orbit_err_missing", "name");

#[tokio::test]
async fn a_duplicate_key_is_a_unique_violation_with_the_index_name() {
    let Some(db) = connect().await else { return };
    common::drop_tables(&db, &["orbit_err_users"]).await;
    common::exec(
        &db,
        "CREATE TABLE orbit_err_users (
            id BIGINT AUTO_INCREMENT PRIMARY KEY,
            email VARCHAR(255) NOT NULL,
            UNIQUE KEY `orbit_err_email_unique` (email)
        )",
    )
    .await;

    Email::query()
        .insert(&db, &[("email", "a@example.com".into())])
        .await
        .expect("the first insert succeeds");
    let err = Email::query()
        .insert(&db, &[("email", "a@example.com".into())])
        .await
        .expect_err("the second insert breaks the index");
    match err {
        Error::UniqueViolation { index } => {
            if common::on_sqlite(db.driver()) {
                // SQLite does not say which index a violation came from.
                assert_eq!(index, None);
            } else {
                assert_eq!(index.as_deref(), Some("orbit_err_email_unique"));
            }
        }
        other => panic!("expected a unique violation, got {other:?}"),
    }

    common::drop_tables(&db, &["orbit_err_users"]).await;
    db.close().await;
}

#[tokio::test]
async fn a_missing_parent_is_a_foreign_key_violation() {
    let Some(db) = connect().await else { return };
    common::drop_tables(&db, &["orbit_err_child", "orbit_err_parent"]).await;
    common::exec(
        &db,
        "CREATE TABLE orbit_err_parent (id BIGINT AUTO_INCREMENT PRIMARY KEY, name VARCHAR(255) NOT NULL);
         CREATE TABLE orbit_err_child (
            id BIGINT AUTO_INCREMENT PRIMARY KEY,
            parent_id BIGINT NOT NULL,
            CONSTRAINT orbit_err_child_parent_fk FOREIGN KEY (parent_id) REFERENCES orbit_err_parent (id)
         )",
    )
    .await;

    let err = Child::query()
        .insert(&db, &[("parent_id", Value::Int(99))])
        .await
        .expect_err("no parent has id 99");
    match err {
        Error::ForeignKeyViolation { constraint } => {
            if common::on_sqlite(db.driver()) {
                // SQLite does not say which constraint a violation came from.
                assert_eq!(constraint, None);
            } else {
                assert_eq!(constraint.as_deref(), Some("orbit_err_child_parent_fk"));
            }
        }
        other => panic!("expected a foreign key violation, got {other:?}"),
    }

    // Deleting a parent that a child still refers to is also a foreign key violation.
    Parent::query()
        .insert(&db, &[("name", "p".into())])
        .await
        .expect("inserts a parent");
    Child::query()
        .insert(&db, &[("parent_id", Value::Int(1))])
        .await
        .expect("inserts a child of parent 1");
    let delete = Parent::query()
        .where_eq("id", 1)
        .delete(&db)
        .await
        .expect_err("the child still refers to the parent");
    assert!(matches!(delete, Error::ForeignKeyViolation { .. }));

    common::drop_tables(&db, &["orbit_err_child", "orbit_err_parent"]).await;
    db.close().await;
}

#[tokio::test]
async fn other_errors_stay_database_errors() {
    let Some(db) = connect().await else { return };

    let err = Missing::query()
        .count(&db)
        .await
        .expect_err("the table does not exist");
    assert!(matches!(err, Error::Database(_)));

    db.close().await;
}
