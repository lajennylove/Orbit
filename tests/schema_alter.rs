//! `schema::table` changes an existing table and `schema::create_if_not_exists` creates one only
//! when it is missing. The render tests check the exact SQL for each driver; the database test
//! uses the database that `ORBIT_DATABASE_URL` names and skips with a notice when it is not set.

mod common;

use orbit_orm::schema;
use orbit_orm::{Database, Driver, Error, PoolOptions, Value, raw};

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

/// Calls in a mixed order: Laravel 11+ renders every operation in call order (Ruling R16).
fn mixed() -> schema::Statement {
    schema::table("users", |t| {
        t.drop_column("a");
        t.string("b").change();
        t.rename_column("c", "d");
        t.integer("e");
    })
}

/// Drops a column and adds it again: only call order makes this work.
fn drop_then_readd() -> schema::Statement {
    schema::table("users", |t| {
        t.drop_column("score");
        t.integer("score").default(0);
    })
}

/// Renames a column and adds a new one under the old name.
fn rename_then_readd() -> schema::Statement {
    schema::table("users", |t| {
        t.rename_column("name", "full_name");
        t.string("name").nullable();
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
            "ALTER TABLE `users` DROP COLUMN `a`",
            "ALTER TABLE `users` MODIFY COLUMN `b` VARCHAR(255) NOT NULL",
            "ALTER TABLE `users` RENAME COLUMN `c` TO `d`",
            "ALTER TABLE `users` ADD COLUMN `e` INTEGER NOT NULL",
        ]
    );
    assert_eq!(
        drop_then_readd().to_sql(Driver::MySql),
        vec![
            "ALTER TABLE `users` DROP COLUMN `score`",
            "ALTER TABLE `users` ADD COLUMN `score` INTEGER NOT NULL DEFAULT 0",
        ]
    );
    assert_eq!(
        rename_then_readd().to_sql(Driver::MySql),
        vec![
            "ALTER TABLE `users` RENAME COLUMN `name` TO `full_name`",
            "ALTER TABLE `users` ADD COLUMN `name` VARCHAR(255) NULL",
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
            "ALTER TABLE \"users\" DROP COLUMN \"a\"",
            "ALTER TABLE \"users\" ALTER COLUMN \"b\" TYPE VARCHAR(255), \
             ALTER COLUMN \"b\" SET NOT NULL, ALTER COLUMN \"b\" DROP DEFAULT",
            "ALTER TABLE \"users\" RENAME COLUMN \"c\" TO \"d\"",
            "ALTER TABLE \"users\" ADD COLUMN \"e\" INTEGER NOT NULL",
        ]
    );
    assert_eq!(
        drop_then_readd().to_sql(Driver::Postgres),
        vec![
            "ALTER TABLE \"users\" DROP COLUMN \"score\"",
            "ALTER TABLE \"users\" ADD COLUMN \"score\" INTEGER NOT NULL DEFAULT 0",
        ]
    );
    assert_eq!(
        rename_then_readd().to_sql(Driver::Postgres),
        vec![
            "ALTER TABLE \"users\" RENAME COLUMN \"name\" TO \"full_name\"",
            "ALTER TABLE \"users\" ADD COLUMN \"name\" VARCHAR(255) NULL",
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
        drop_then_readd().to_sql(Driver::Sqlite),
        vec![
            "ALTER TABLE \"users\" DROP COLUMN \"score\"",
            "ALTER TABLE \"users\" ADD COLUMN \"score\" INTEGER NOT NULL DEFAULT 0",
        ]
    );
    assert_eq!(
        rename_then_readd().to_sql(Driver::Sqlite),
        vec![
            "ALTER TABLE \"users\" RENAME COLUMN \"name\" TO \"full_name\"",
            "ALTER TABLE \"users\" ADD COLUMN \"name\" TEXT NULL",
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
    match add_id().try_to_sql(Driver::Sqlite) {
        Err(Error::Unsupported { operation, .. }) => {
            assert_eq!(operation, "adding a primary key column")
        }
        other => panic!("expected Unsupported, got {other:?}"),
    }
}

/// Every operation in call order, on SQLite too (Ruling R16).
#[cfg(feature = "sqlite")]
#[test]
fn renders_in_call_order_on_sqlite() {
    let statement = schema::table("users", |t| {
        t.drop_column("a");
        t.rename_column("c", "d");
        t.integer("e");
    });
    assert_eq!(
        statement.to_sql(Driver::Sqlite),
        vec![
            "ALTER TABLE \"users\" DROP COLUMN \"a\"",
            "ALTER TABLE \"users\" RENAME COLUMN \"c\" TO \"d\"",
            "ALTER TABLE \"users\" ADD COLUMN \"e\" INTEGER NOT NULL",
        ]
    );
}

/// Changing a column into an auto-incrementing id is not a column change (Ruling R10).
#[cfg(any(feature = "mysql", feature = "postgres"))]
#[test]
fn changing_an_id_column_is_unsupported() {
    let statement = schema::table("users", |t| {
        t.id().change();
    });
    let drivers = [
        #[cfg(feature = "mysql")]
        Driver::MySql,
        #[cfg(feature = "postgres")]
        Driver::Postgres,
    ];
    for driver in drivers {
        match statement.try_to_sql(driver) {
            Err(Error::Unsupported { operation, .. }) => {
                assert_eq!(operation, "changing an id column")
            }
            other => panic!("expected Unsupported, got {other:?}"),
        }
    }
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

/// Indexes and foreign keys inside `schema::table` render where they were called, among the
/// columns (Ruling R16); the exact SQL per driver is in `tests/schema_keys.rs`.
#[test]
fn indexes_and_foreign_keys_in_table_render_in_call_order() {
    let statement = schema::table("t", |t| {
        t.integer("a");
        t.index(["a"]);
        t.integer("b");
    });
    let sql = statement.to_sql(any());
    assert_eq!(sql.len(), 3, "{sql:?}");
    assert!(
        sql[0].contains("ADD COLUMN") && sql[0].contains('a'),
        "{sql:?}"
    );
    assert!(sql[1].contains("t_a_index"), "{sql:?}");
    assert!(
        sql[2].contains("ADD COLUMN") && sql[2].contains('b'),
        "{sql:?}"
    );
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
    // One connection, as the migrator runs each migration in one transaction. A second SQLite
    // connection keeps the schema it read before another connection changed it, and SQLite checks
    // `ADD COLUMN` and `DROP COLUMN` against that stale schema ("duplicate column name").
    let options = PoolOptions {
        max_connections: 1,
        ..PoolOptions::default()
    };
    let db = Database::connect_with(&url, options)
        .await
        .expect("connects");
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

    // Call order (Ruling R16): drop a column and add it again, then rename one and add a new
    // column under the old name.
    run(
        &db,
        &schema::table("alter_items", |t| {
            t.drop_column("score");
            t.integer("score").default(7);
        }),
    )
    .await;
    run(
        &db,
        &schema::table("alter_items", |t| {
            t.rename_column("title", "heading");
            t.string("title").nullable();
        }),
    )
    .await;
    assert_eq!(
        columns(&db, "alter_items").await,
        ["id", "heading", "score", "title"]
    );

    let heading = if common::on_sqlite(driver) {
        "'first'"
    } else {
        // The changed column takes NULL now.
        "NULL"
    };
    raw::execute(
        &db,
        &format!("INSERT INTO alter_items (heading) VALUES ({heading})"),
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
