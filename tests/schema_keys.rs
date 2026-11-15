//! Named indexes and foreign keys, primary keys, and the index and foreign key commands of
//! `schema::table`. The render tests check the exact SQL for each driver; the database test uses
//! the database that `ORBIT_DATABASE_URL` names and skips with a notice when it is not set.

mod common;

use orbit_orm::schema::{self, Action};
use orbit_orm::{Database, Driver, Error, raw};

/// Any enabled driver, for the tests whose result does not depend on the dialect.
fn any() -> Driver {
    #[cfg(feature = "mysql")]
    return Driver::MySql;
    #[cfg(all(not(feature = "mysql"), feature = "postgres"))]
    return Driver::Postgres;
    #[cfg(all(not(feature = "mysql"), not(feature = "postgres")))]
    return Driver::Sqlite;
}

/// Every enabled driver.
fn drivers() -> Vec<Driver> {
    vec![
        #[cfg(feature = "mysql")]
        Driver::MySql,
        #[cfg(feature = "postgres")]
        Driver::Postgres,
        #[cfg(feature = "sqlite")]
        Driver::Sqlite,
    ]
}

/// A string primary key, and an index, a unique and a foreign key with custom names.
fn sessions() -> schema::Statement {
    schema::create("sessions", |t| {
        t.string("id").length(64).primary();
        t.big_integer("user_id").nullable();
        t.integer("last_activity");
        t.index(["user_id"]).name("sessions_user_index");
        t.unique(["last_activity"]).name("sessions_activity_unique");
        t.foreign("user_id")
            .references("users", "id")
            .on_delete(Action::Cascade)
            .name("sessions_user_fk");
    })
}

/// `create_if_not_exists` keeps `IF NOT EXISTS` on a named index created separately.
fn sessions_if_not_exists() -> schema::Statement {
    schema::create_if_not_exists("sessions", |t| {
        t.string("id").length(64).primary();
        t.big_integer("user_id").nullable();
        t.index(["user_id"]).name("sessions_user_index");
    })
}

/// `PRIMARY KEY` comes after the nullability and the default.
fn coded() -> schema::Statement {
    schema::create("codes", |t| {
        t.string("code").length(8).default("x").primary();
    })
}

/// A composite primary key.
fn members() -> schema::Statement {
    schema::create("members", |t| {
        t.big_integer("team_id");
        t.big_integer("user_id");
        t.primary(["team_id", "user_id"]);
    })
}

/// Index commands on an existing table. The added column is declared last and still renders
/// first.
fn keys_in_table() -> schema::Statement {
    schema::table("posts", |t| {
        t.index(["user_id"]);
        t.unique(["slug"]).name("posts_slug_key");
        t.foreign("user_id")
            .references("users", "id")
            .on_delete(Action::Cascade);
        t.drop_index("posts_old_index");
        t.drop_foreign("posts_old_foreign");
        t.string("slug");
    })
}

fn primary_in_table() -> schema::Statement {
    schema::table("members", |t| {
        t.primary(["team_id", "user_id"]);
    })
}

fn primary_column_in_table() -> schema::Statement {
    schema::table("codes", |t| {
        t.string("code").length(8).primary();
    })
}

#[cfg(feature = "mysql")]
#[test]
fn renders_mysql() {
    assert_eq!(
        sessions().to_sql(Driver::MySql),
        vec![
            "CREATE TABLE `sessions` (`id` VARCHAR(64) NOT NULL PRIMARY KEY, \
             `user_id` BIGINT NULL, `last_activity` INTEGER NOT NULL, \
             INDEX `sessions_user_index` (`user_id`), \
             CONSTRAINT `sessions_activity_unique` UNIQUE (`last_activity`), \
             CONSTRAINT `sessions_user_fk` FOREIGN KEY (`user_id`) \
             REFERENCES `users` (`id`) ON DELETE CASCADE)"
        ]
    );
    assert_eq!(
        sessions_if_not_exists().to_sql(Driver::MySql),
        vec![
            "CREATE TABLE IF NOT EXISTS `sessions` (`id` VARCHAR(64) NOT NULL PRIMARY KEY, \
             `user_id` BIGINT NULL, INDEX `sessions_user_index` (`user_id`))"
        ]
    );
    assert_eq!(
        coded().to_sql(Driver::MySql),
        vec!["CREATE TABLE `codes` (`code` VARCHAR(8) NOT NULL DEFAULT 'x' PRIMARY KEY)"]
    );
    assert_eq!(
        members().to_sql(Driver::MySql),
        vec![
            "CREATE TABLE `members` (`team_id` BIGINT NOT NULL, `user_id` BIGINT NOT NULL, \
             PRIMARY KEY (`team_id`, `user_id`))"
        ]
    );
    assert_eq!(
        keys_in_table().to_sql(Driver::MySql),
        vec![
            "ALTER TABLE `posts` ADD COLUMN `slug` VARCHAR(255) NOT NULL",
            "ALTER TABLE `posts` ADD INDEX `posts_user_id_index` (`user_id`)",
            "ALTER TABLE `posts` ADD UNIQUE `posts_slug_key` (`slug`)",
            "ALTER TABLE `posts` ADD CONSTRAINT `posts_user_id_foreign` FOREIGN KEY (`user_id`) \
             REFERENCES `users` (`id`) ON DELETE CASCADE",
            "ALTER TABLE `posts` DROP INDEX `posts_old_index`",
            "ALTER TABLE `posts` DROP FOREIGN KEY `posts_old_foreign`",
        ]
    );
    assert_eq!(
        primary_in_table().to_sql(Driver::MySql),
        vec!["ALTER TABLE `members` ADD PRIMARY KEY (`team_id`, `user_id`)"]
    );
    assert_eq!(
        primary_column_in_table().to_sql(Driver::MySql),
        vec!["ALTER TABLE `codes` ADD COLUMN `code` VARCHAR(8) NOT NULL PRIMARY KEY"]
    );
}

#[cfg(feature = "postgres")]
#[test]
fn renders_postgres() {
    assert_eq!(
        sessions().to_sql(Driver::Postgres),
        vec![
            "CREATE TABLE \"sessions\" (\"id\" VARCHAR(64) NOT NULL PRIMARY KEY, \
             \"user_id\" BIGINT NULL, \"last_activity\" INTEGER NOT NULL, \
             CONSTRAINT \"sessions_activity_unique\" UNIQUE (\"last_activity\"), \
             CONSTRAINT \"sessions_user_fk\" FOREIGN KEY (\"user_id\") \
             REFERENCES \"users\" (\"id\") ON DELETE CASCADE)",
            "CREATE INDEX \"sessions_user_index\" ON \"sessions\" (\"user_id\")",
        ]
    );
    assert_eq!(
        sessions_if_not_exists().to_sql(Driver::Postgres),
        vec![
            "CREATE TABLE IF NOT EXISTS \"sessions\" (\"id\" VARCHAR(64) NOT NULL PRIMARY KEY, \
             \"user_id\" BIGINT NULL)",
            "CREATE INDEX IF NOT EXISTS \"sessions_user_index\" ON \"sessions\" (\"user_id\")",
        ]
    );
    assert_eq!(
        coded().to_sql(Driver::Postgres),
        vec!["CREATE TABLE \"codes\" (\"code\" VARCHAR(8) NOT NULL DEFAULT 'x' PRIMARY KEY)"]
    );
    assert_eq!(
        members().to_sql(Driver::Postgres),
        vec![
            "CREATE TABLE \"members\" (\"team_id\" BIGINT NOT NULL, \"user_id\" BIGINT NOT NULL, \
             PRIMARY KEY (\"team_id\", \"user_id\"))"
        ]
    );
    assert_eq!(
        keys_in_table().to_sql(Driver::Postgres),
        vec![
            "ALTER TABLE \"posts\" ADD COLUMN \"slug\" VARCHAR(255) NOT NULL",
            "CREATE INDEX \"posts_user_id_index\" ON \"posts\" (\"user_id\")",
            "ALTER TABLE \"posts\" ADD CONSTRAINT \"posts_slug_key\" UNIQUE (\"slug\")",
            "ALTER TABLE \"posts\" ADD CONSTRAINT \"posts_user_id_foreign\" \
             FOREIGN KEY (\"user_id\") REFERENCES \"users\" (\"id\") ON DELETE CASCADE",
            "DROP INDEX \"posts_old_index\"",
            "ALTER TABLE \"posts\" DROP CONSTRAINT \"posts_old_foreign\"",
        ]
    );
    assert_eq!(
        primary_in_table().to_sql(Driver::Postgres),
        vec!["ALTER TABLE \"members\" ADD PRIMARY KEY (\"team_id\", \"user_id\")"]
    );
    assert_eq!(
        primary_column_in_table().to_sql(Driver::Postgres),
        vec!["ALTER TABLE \"codes\" ADD COLUMN \"code\" VARCHAR(8) NOT NULL PRIMARY KEY"]
    );
}

#[cfg(feature = "sqlite")]
#[test]
fn renders_sqlite() {
    assert_eq!(
        sessions().to_sql(Driver::Sqlite),
        vec![
            "CREATE TABLE \"sessions\" (\"id\" TEXT NOT NULL PRIMARY KEY, \
             \"user_id\" INTEGER NULL, \"last_activity\" INTEGER NOT NULL, \
             CONSTRAINT \"sessions_activity_unique\" UNIQUE (\"last_activity\"), \
             CONSTRAINT \"sessions_user_fk\" FOREIGN KEY (\"user_id\") \
             REFERENCES \"users\" (\"id\") ON DELETE CASCADE)",
            "CREATE INDEX \"sessions_user_index\" ON \"sessions\" (\"user_id\")",
        ]
    );
    assert_eq!(
        sessions_if_not_exists().to_sql(Driver::Sqlite),
        vec![
            "CREATE TABLE IF NOT EXISTS \"sessions\" (\"id\" TEXT NOT NULL PRIMARY KEY, \
             \"user_id\" INTEGER NULL)",
            "CREATE INDEX IF NOT EXISTS \"sessions_user_index\" ON \"sessions\" (\"user_id\")",
        ]
    );
    assert_eq!(
        coded().to_sql(Driver::Sqlite),
        vec!["CREATE TABLE \"codes\" (\"code\" TEXT NOT NULL DEFAULT 'x' PRIMARY KEY)"]
    );
    assert_eq!(
        members().to_sql(Driver::Sqlite),
        vec![
            "CREATE TABLE \"members\" (\"team_id\" INTEGER NOT NULL, \"user_id\" INTEGER NOT NULL, \
             PRIMARY KEY (\"team_id\", \"user_id\"))"
        ]
    );
    // The foreign key and drop_foreign render nothing on SQLite (Ruling R4).
    assert_eq!(
        keys_in_table().to_sql(Driver::Sqlite),
        vec![
            "ALTER TABLE \"posts\" ADD COLUMN \"slug\" TEXT NOT NULL",
            "CREATE INDEX \"posts_user_id_index\" ON \"posts\" (\"user_id\")",
            "CREATE UNIQUE INDEX \"posts_slug_key\" ON \"posts\" (\"slug\")",
            "DROP INDEX \"posts_old_index\"",
        ]
    );
    let foreign_only = schema::table("posts", |t| {
        t.foreign("user_id").references("users", "id");
        t.drop_foreign("posts_old_foreign");
    });
    assert_eq!(foreign_only.to_sql(Driver::Sqlite), Vec::<String>::new());
}

/// SQLite cannot add a primary key to an existing table.
#[cfg(feature = "sqlite")]
#[test]
fn primary_keys_in_table_are_unsupported_on_sqlite() {
    for (expected, statement) in [
        ("adding a primary key", primary_in_table()),
        ("adding a primary key column", primary_column_in_table()),
    ] {
        match statement.try_to_sql(Driver::Sqlite) {
            Err(Error::Unsupported { operation, .. }) => assert_eq!(operation, expected),
            other => panic!("expected Unsupported({expected}), got {other:?}"),
        }
    }
}

/// `.primary().change()` would lose the key on PostgreSQL, so it is an error, never dropped.
#[cfg(any(feature = "mysql", feature = "postgres"))]
#[test]
fn changing_a_column_into_a_primary_key_is_unsupported() {
    let statement = schema::table("codes", |t| {
        t.string("code").primary().change();
    });
    for driver in drivers() {
        if common::on_sqlite(driver) {
            continue;
        }
        match statement.try_to_sql(driver) {
            Err(Error::Unsupported { operation, .. }) => {
                assert_eq!(operation, "changing a column into a primary key")
            }
            other => panic!("expected Unsupported, got {other:?}"),
        }
    }
}

#[test]
fn drops_are_errors_in_create() {
    let cases: [(&str, schema::Statement); 2] = [
        (
            "dropping an index in create",
            schema::create("t", |t| {
                t.id();
                t.drop_index("t_a_index");
            }),
        ),
        (
            "dropping a foreign key in create",
            schema::create("t", |t| {
                t.id();
                t.drop_foreign("t_a_foreign");
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

/// A foreign key without `references` is an error in `table` too, SQLite included.
#[test]
fn a_foreign_key_without_a_target_is_an_error_in_table() {
    let statement = schema::table("t", |t| {
        t.foreign("a");
    });
    for driver in drivers() {
        match statement.try_to_sql(driver) {
            Err(Error::Unsupported { operation, .. }) => {
                assert_eq!(operation, "a foreign key without references()")
            }
            other => panic!("expected Unsupported, got {other:?}"),
        }
    }
}

#[test]
fn rejects_invalid_names() {
    for bad in ["", "a`b", "a\"b"] {
        let statements = [
            schema::create("t", |t| {
                t.string("a");
                t.index(["a"]).name(bad);
            }),
            schema::create("t", |t| {
                t.string("a");
                t.unique(["a"]).name(bad);
            }),
            schema::create("t", |t| {
                t.big_integer("a");
                t.foreign("a").references("u", "id").name(bad);
            }),
            schema::create("t", |t| {
                t.string("a");
                t.primary([bad]);
            }),
            schema::table("t", |t| {
                t.index(["a"]).name(bad);
            }),
            schema::table("t", |t| {
                t.unique([bad]);
            }),
            schema::table("t", |t| {
                t.foreign("a").references("u", "id").name(bad);
            }),
            schema::table("t", |t| {
                t.drop_index(bad);
            }),
            schema::table("t", |t| {
                t.drop_foreign(bad);
            }),
        ];
        for driver in drivers() {
            for statement in &statements {
                assert!(
                    matches!(
                        statement.try_to_sql(driver),
                        Err(Error::InvalidIdentifier(_))
                    ),
                    "{statement:?} on {driver:?}"
                );
            }
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

#[tokio::test]
async fn keys_work_on_the_database() {
    let Some(url) = common::url() else {
        eprintln!("skipped: ORBIT_DATABASE_URL is not set");
        return;
    };
    let db = Database::connect(&url).await.expect("connects");
    let driver = db.driver();
    for table in ["keys_children", "keys_parents"] {
        run(&db, &schema::drop_if_exists(table)).await;
    }

    run(
        &db,
        &schema::create("keys_parents", |t| {
            t.string("code").length(64).primary();
            t.string("label");
        }),
    )
    .await;
    run(
        &db,
        &schema::create("keys_children", |t| {
            t.string("parent_code").length(64);
            t.integer("position");
            t.string("slug").length(64);
            t.string("label").length(64);
            t.primary(["parent_code", "position"]);
        }),
    )
    .await;

    run(
        &db,
        &schema::table("keys_children", |t| {
            t.index(["label"]).name("keys_children_label_idx");
            t.unique(["slug"]);
            t.foreign("parent_code")
                .references("keys_parents", "code")
                .on_delete(Action::Cascade)
                .name("keys_children_parent_fk");
        }),
    )
    .await;

    let parent = "INSERT INTO keys_parents (code, label) VALUES ('p', 'Parent')";
    raw::execute(&db, parent, &[])
        .await
        .expect("inserts a parent");
    assert!(
        raw::execute(&db, parent, &[]).await.is_err(),
        "the string primary key is enforced"
    );
    let child = "INSERT INTO keys_children (parent_code, position, slug, label) \
                 VALUES ('p', 1, 'one', 'One')";
    raw::execute(&db, child, &[])
        .await
        .expect("inserts a child");
    assert!(
        raw::execute(
            &db,
            "INSERT INTO keys_children (parent_code, position, slug, label) \
             VALUES ('p', 1, 'other', 'Other')",
            &[]
        )
        .await
        .is_err(),
        "the composite primary key is enforced"
    );
    assert!(
        raw::execute(
            &db,
            "INSERT INTO keys_children (parent_code, position, slug, label) \
             VALUES ('p', 2, 'one', 'Again')",
            &[]
        )
        .await
        .is_err(),
        "the unique index added to the existing table is enforced"
    );

    if !common::on_sqlite(driver) {
        // The foreign key added to the existing table cascades the delete.
        raw::execute(&db, "DELETE FROM keys_parents WHERE code = 'p'", &[])
            .await
            .expect("deletes the parent");
        let left: i64 = raw::fetch_scalar(
            &db,
            &format!(
                "SELECT CAST(COUNT(*) AS {}) FROM keys_children",
                common::bigint_type(driver)
            ),
            &[],
        )
        .await
        .expect("counts the children");
        assert_eq!(left, 0, "ON DELETE CASCADE is enforced");
    }

    // The foreign key first: MySQL keeps an index that a foreign key needs.
    run(
        &db,
        &schema::table("keys_children", |t| {
            t.drop_foreign("keys_children_parent_fk");
            t.drop_index("keys_children_label_idx");
        }),
    )
    .await;

    for table in ["keys_children", "keys_parents"] {
        run(&db, &schema::drop(table)).await;
    }
}
