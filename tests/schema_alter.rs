//! `schema::table` changes an existing table and `schema::create_if_not_exists` creates one only
//! when it is missing. The render tests check the exact SQL for each driver; the database test
//! uses the database that `ORBIT_DATABASE_URL` names and skips with a notice when it is not set.

mod common;

use orbit_orm::schema;
use orbit_orm::{Database, Driver, Error, Value, raw};

/// Any enabled driver, for the tests whose result does not depend on the dialect.
fn any() -> Driver {
    #[cfg(feature = "mysql")]
    return Driver::MySql;
    #[cfg(all(not(feature = "mysql"), feature = "postgres"))]
    return Driver::Postgres;
    #[cfg(all(not(feature = "mysql"), not(feature = "postgres")))]
    return Driver::Sqlite;
}

fn add_columns() -> schema::Statement {
    schema::table("users", |t| {
        t.string("nick").nullable();
        t.integer("score").default(0);
    })
}

fn rename_and_drop() -> schema::Statement {
    schema::table("users", |t| {
        t.rename_column("name", "full_name");
        t.drop_column("nick");
    })
}

fn change() -> schema::Statement {
    schema::table("users", |t| {
        t.string("name").length(100).nullable().change();
        t.integer("score").default(5).change();
    })
}

/// Calls in a mixed order: Laravel renders added columns, then changed columns, then the other
/// commands in call order.
fn mixed() -> schema::Statement {
    schema::table("users", |t| {
        t.drop_column("a");
        t.string("b").change();
        t.rename_column("c", "d");
        t.integer("e");
    })
}

fn tags_if_not_exists() -> schema::Statement {
    schema::create_if_not_exists("tags", |t| {
        t.id();
        t.string("name");
        t.index(["name"]);
    })
}

fn add_id() -> schema::Statement {
    schema::table("users", |t| {
        t.id();
    })
}

#[cfg(feature = "mysql")]
#[test]
fn renders_mysql() {
    assert_eq!(
        add_columns().to_sql(Driver::MySql),
        vec![
            "ALTER TABLE `users` ADD COLUMN `nick` VARCHAR(255) NULL",
            "ALTER TABLE `users` ADD COLUMN `score` INTEGER NOT NULL DEFAULT 0",
        ]
    );
    assert_eq!(
        rename_and_drop().to_sql(Driver::MySql),
        vec![
            "ALTER TABLE `users` RENAME COLUMN `name` TO `full_name`",
            "ALTER TABLE `users` DROP COLUMN `nick`",
        ]
    );
    assert_eq!(
        change().to_sql(Driver::MySql),
        vec![
            "ALTER TABLE `users` MODIFY COLUMN `name` VARCHAR(100) NULL",
            "ALTER TABLE `users` MODIFY COLUMN `score` INTEGER NOT NULL DEFAULT 5",
        ]
    );
    assert_eq!(
        mixed().to_sql(Driver::MySql),
        vec![
            "ALTER TABLE `users` ADD COLUMN `e` INTEGER NOT NULL",
            "ALTER TABLE `users` MODIFY COLUMN `b` VARCHAR(255) NOT NULL",
            "ALTER TABLE `users` DROP COLUMN `a`",
            "ALTER TABLE `users` RENAME COLUMN `c` TO `d`",
        ]
    );
    assert_eq!(
        tags_if_not_exists().to_sql(Driver::MySql),
        vec![
            "CREATE TABLE IF NOT EXISTS `tags` (`id` BIGINT AUTO_INCREMENT PRIMARY KEY, \
             `name` VARCHAR(255) NOT NULL, INDEX `tags_name_index` (`name`))"
        ]
    );
    assert_eq!(
        add_id().to_sql(Driver::MySql),
        vec!["ALTER TABLE `users` ADD COLUMN `id` BIGINT AUTO_INCREMENT PRIMARY KEY"]
    );
}

#[cfg(feature = "postgres")]
#[test]
fn renders_postgres() {
    assert_eq!(
        add_columns().to_sql(Driver::Postgres),
        vec![
            "ALTER TABLE \"users\" ADD COLUMN \"nick\" VARCHAR(255) NULL",
            "ALTER TABLE \"users\" ADD COLUMN \"score\" INTEGER NOT NULL DEFAULT 0",
        ]
    );
    assert_eq!(
        rename_and_drop().to_sql(Driver::Postgres),
        vec![
            "ALTER TABLE \"users\" RENAME COLUMN \"name\" TO \"full_name\"",
            "ALTER TABLE \"users\" DROP COLUMN \"nick\"",
        ]
    );
    assert_eq!(
        change().to_sql(Driver::Postgres),
        vec![
            "ALTER TABLE \"users\" ALTER COLUMN \"name\" TYPE VARCHAR(100), \
             ALTER COLUMN \"name\" DROP NOT NULL, ALTER COLUMN \"name\" DROP DEFAULT",
            "ALTER TABLE \"users\" ALTER COLUMN \"score\" TYPE INTEGER, \
             ALTER COLUMN \"score\" SET NOT NULL, ALTER COLUMN \"score\" SET DEFAULT 5",
        ]
    );
    assert_eq!(
        mixed().to_sql(Driver::Postgres),
        vec![
            "ALTER TABLE \"users\" ADD COLUMN \"e\" INTEGER NOT NULL",
            "ALTER TABLE \"users\" ALTER COLUMN \"b\" TYPE VARCHAR(255), \
             ALTER COLUMN \"b\" SET NOT NULL, ALTER COLUMN \"b\" DROP DEFAULT",
            "ALTER TABLE \"users\" DROP COLUMN \"a\"",
            "ALTER TABLE \"users\" RENAME COLUMN \"c\" TO \"d\"",
        ]
    );
    assert_eq!(
        tags_if_not_exists().to_sql(Driver::Postgres),
        vec![
            "CREATE TABLE IF NOT EXISTS \"tags\" (\"id\" BIGSERIAL PRIMARY KEY, \
             \"name\" VARCHAR(255) NOT NULL)",
            "CREATE INDEX IF NOT EXISTS \"tags_name_index\" ON \"tags\" (\"name\")",
        ]
    );
    assert_eq!(
        add_id().to_sql(Driver::Postgres),
        vec!["ALTER TABLE \"users\" ADD COLUMN \"id\" BIGSERIAL PRIMARY KEY"]
    );
}

#[cfg(feature = "sqlite")]
#[test]
fn renders_sqlite() {
    assert_eq!(
        add_columns().to_sql(Driver::Sqlite),
        vec![
            "ALTER TABLE \"users\" ADD COLUMN \"nick\" TEXT NULL",
            "ALTER TABLE \"users\" ADD COLUMN \"score\" INTEGER NOT NULL DEFAULT 0",
        ]
    );
    assert_eq!(
        rename_and_drop().to_sql(Driver::Sqlite),
        vec![
            "ALTER TABLE \"users\" RENAME COLUMN \"name\" TO \"full_name\"",
            "ALTER TABLE \"users\" DROP COLUMN \"nick\"",
        ]
    );
    assert_eq!(
        tags_if_not_exists().to_sql(Driver::Sqlite),
        vec![
            "CREATE TABLE IF NOT EXISTS \"tags\" (\"id\" INTEGER PRIMARY KEY AUTOINCREMENT, \
             \"name\" TEXT NOT NULL)",
            "CREATE INDEX IF NOT EXISTS \"tags_name_index\" ON \"tags\" (\"name\")",
        ]
    );
}

/// SQLite cannot change a column in place; Laravel 11 rebuilds the table (Ruling R3).
#[cfg(feature = "sqlite")]
#[test]
fn changing_a_column_is_unsupported_on_sqlite() {
    for statement in [change(), mixed()] {
        assert!(matches!(
            statement.try_to_sql(Driver::Sqlite),
            Err(Error::Unsupported {
                operation: "changing a column",
                ..
            })
        ));
    }
}

/// SQLite cannot add a PRIMARY KEY column to an existing table.
#[cfg(feature = "sqlite")]
#[test]
fn adding_an_id_is_unsupported_on_sqlite() {
    assert!(matches!(
        add_id().try_to_sql(Driver::Sqlite),
        Err(Error::Unsupported { .. })
    ));
}

#[test]
fn create_keeps_its_sql_without_if_not_exists() {
    let sql = schema::create("tags", |t| {
        t.id();
    })
    .to_sql(any());
    assert!(sql[0].starts_with("CREATE TABLE ") && !sql[0].contains("IF NOT EXISTS"));
}

#[test]
fn alter_only_commands_are_errors_in_create() {
    let cases: [(&str, schema::Statement); 3] = [
        (
            "renaming a column in create",
            schema::create("t", |t| {
                t.id();
                t.rename_column("a", "b");
            }),
        ),
        (
            "dropping a column in create",
            schema::create_if_not_exists("t", |t| {
                t.id();
                t.drop_column("a");
            }),
        ),
        (
            "changing a column in create",
            schema::create("t", |t| {
                t.string("a").change();
            }),
        ),
    ];
    for (expected, statement) in cases {
        match statement.try_to_sql(any()) {
            Err(Error::Unsupported { operation, .. }) => assert_eq!(operation, expected),
            other => panic!("expected Unsupported({expected}), got {other:?}"),
        }
    }
}

/// Indexes and foreign keys inside `schema::table` come with the index commands; until then they
/// are an error, never dropped in silence.
#[test]
fn indexes_and_foreign_keys_in_table_are_not_dropped_silently() {
    let index = schema::table("t", |t| {
        t.index(["a"]);
    });
    assert!(matches!(
        index.try_to_sql(any()),
        Err(Error::Unsupported { .. })
    ));
    let foreign = schema::table("t", |t| {
        t.foreign("a").references("u", "id");
    });
    assert!(matches!(
        foreign.try_to_sql(any()),
        Err(Error::Unsupported { .. })
    ));
}

#[test]
fn rejects_invalid_names() {
    for bad in ["", "a`b", "a\"b"] {
        let statements = [
            schema::table(bad, |t| {
                t.drop_column("a");
            }),
            schema::table("t", |t| {
                t.rename_column(bad, "b");
            }),
            schema::table("t", |t| {
                t.rename_column("a", bad);
            }),
            schema::table("t", |t| {
                t.drop_column(bad);
            }),
            schema::table("t", |t| {
                t.string(bad).nullable();
            }),
            schema::create_if_not_exists(bad, |t| {
                t.id();
            }),
        ];
        for statement in statements {
            assert!(matches!(
                statement.try_to_sql(any()),
                Err(Error::InvalidIdentifier(_))
            ));
        }
    }
}

async fn run(db: &Database, statement: &schema::Statement) {
    for sql in statement.to_sql(db.driver()) {
        raw::execute(db, &sql, &[])
            .await
            .unwrap_or_else(|e| panic!("`{sql}` failed: {e}"));
    }
}

async fn columns(db: &Database, table: &str) -> Vec<String> {
    let rows: Vec<(String,)> =
        raw::fetch_all(db, &common::columns_sql(db.driver()), &[Value::from(table)])
            .await
            .expect("reads the columns");
    rows.into_iter().map(|(name,)| name).collect()
}

#[tokio::test]
async fn the_altered_table_works_on_the_database() {
    let Some(url) = common::url() else {
        eprintln!("skipped: ORBIT_DATABASE_URL is not set");
        return;
    };
    let db = Database::connect(&url).await.expect("connects");
    let driver = db.driver();
    run(&db, &schema::drop_if_exists("alter_items")).await;

    // Twice: the second run finds the table and its index and does nothing.
    let create = schema::create_if_not_exists("alter_items", |t| {
        t.id();
        t.string("name");
        t.index(["name"]);
    });
    run(&db, &create).await;
    run(&db, &create).await;

    run(
        &db,
        &schema::table("alter_items", |t| {
            t.string("nick").nullable();
            t.integer("score").default(7);
        }),
    )
    .await;
    run(
        &db,
        &schema::table("alter_items", |t| {
            t.rename_column("name", "title");
            t.drop_column("nick");
        }),
    )
    .await;
    if !common::on_sqlite(driver) {
        run(
            &db,
            &schema::table("alter_items", |t| {
                t.string("title").length(100).nullable().change();
            }),
        )
        .await;
    }
    assert_eq!(columns(&db, "alter_items").await, ["id", "title", "score"]);

    let title = if common::on_sqlite(driver) {
        "'first'"
    } else {
        // The changed column takes NULL now.
        "NULL"
    };
    raw::execute(
        &db,
        &format!("INSERT INTO alter_items (title) VALUES ({title})"),
        &[],
    )
    .await
    .expect("inserts into the altered table");
    let score: i64 = raw::fetch_scalar(
        &db,
        &format!(
            "SELECT CAST(score AS {}) FROM alter_items",
            common::bigint_type(driver)
        ),
        &[],
    )
    .await
    .expect("reads the added column");
    assert_eq!(score, 7, "the added column's default applies");

    run(&db, &schema::drop("alter_items")).await;
}
