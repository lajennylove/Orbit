//! The schema builder renders exact SQL for each driver, and the SQL runs on a real database.
//! The execution test uses the database that `ORBIT_DATABASE_URL` names and skips with a notice
//! when it is not set, like the other database tests.

mod common;

use orbit_orm::schema::{self, Action, Blueprint};
use orbit_orm::{Database, Driver, Error, Value, raw};

/// The table the brief names: an id, a string, a nullable text, a foreign key that restricts
/// deletes, and a unique index.
/// Any enabled driver, for the tests whose result does not depend on the dialect.
fn any() -> Driver {
    #[cfg(feature = "mysql")]
    return Driver::MySql;
    #[cfg(all(not(feature = "mysql"), feature = "postgres"))]
    return Driver::Postgres;
    #[cfg(all(not(feature = "mysql"), not(feature = "postgres")))]
    return Driver::Sqlite;
}

fn posts() -> schema::Statement {
    schema::create("posts", |t| {
        t.id();
        t.string("title");
        t.text("body").nullable();
        t.big_integer("user_id");
        t.foreign("user_id")
            .references("users", "id")
            .on_delete(Action::Restrict);
        t.unique(["title"]);
    })
}

#[cfg(feature = "mysql")]
#[test]
fn renders_mysql() {
    assert_eq!(
        posts().to_sql(Driver::MySql),
        vec![
            "CREATE TABLE `posts` (`id` BIGINT AUTO_INCREMENT PRIMARY KEY, \
             `title` VARCHAR(255) NOT NULL, `body` TEXT NULL, `user_id` BIGINT NOT NULL, \
             CONSTRAINT `posts_title_unique` UNIQUE (`title`), \
             CONSTRAINT `posts_user_id_foreign` FOREIGN KEY (`user_id`) \
             REFERENCES `users` (`id`) ON DELETE RESTRICT)"
        ]
    );
}

#[cfg(feature = "postgres")]
#[test]
fn renders_postgres() {
    assert_eq!(
        posts().to_sql(Driver::Postgres),
        vec![
            "CREATE TABLE \"posts\" (\"id\" BIGSERIAL PRIMARY KEY, \
             \"title\" VARCHAR(255) NOT NULL, \"body\" TEXT NULL, \"user_id\" BIGINT NOT NULL, \
             CONSTRAINT \"posts_title_unique\" UNIQUE (\"title\"), \
             CONSTRAINT \"posts_user_id_foreign\" FOREIGN KEY (\"user_id\") \
             REFERENCES \"users\" (\"id\") ON DELETE RESTRICT)"
        ]
    );
}

#[cfg(feature = "sqlite")]
#[test]
fn renders_sqlite() {
    assert_eq!(
        posts().to_sql(Driver::Sqlite),
        vec![
            "CREATE TABLE \"posts\" (\"id\" INTEGER PRIMARY KEY AUTOINCREMENT, \
             \"title\" TEXT NOT NULL, \"body\" TEXT NULL, \"user_id\" INTEGER NOT NULL, \
             CONSTRAINT \"posts_title_unique\" UNIQUE (\"title\"), \
             CONSTRAINT \"posts_user_id_foreign\" FOREIGN KEY (\"user_id\") \
             REFERENCES \"users\" (\"id\") ON DELETE RESTRICT)"
        ]
    );
}

fn extras() -> schema::Statement {
    schema::create("events", |t| {
        t.id();
        t.string("slug").length(64).default("none");
        t.integer("views").default(0);
        t.boolean("live").default(true);
        t.date_time("starts_at").nullable();
        t.timestamps();
        t.index(["slug", "views"]);
    })
}

#[cfg(feature = "mysql")]
#[test]
fn renders_extras_mysql() {
    assert_eq!(
        extras().to_sql(Driver::MySql),
        vec![
            "CREATE TABLE `events` (`id` BIGINT AUTO_INCREMENT PRIMARY KEY, \
             `slug` VARCHAR(64) NOT NULL DEFAULT 'none', `views` INTEGER NOT NULL DEFAULT 0, \
             `live` TINYINT(1) NOT NULL DEFAULT 1, `starts_at` DATETIME NULL, \
             `created_at` DATETIME NULL, `updated_at` DATETIME NULL, \
             INDEX `events_slug_views_index` (`slug`, `views`))"
        ]
    );
}

#[cfg(feature = "postgres")]
#[test]
fn renders_extras_postgres() {
    assert_eq!(
        extras().to_sql(Driver::Postgres),
        vec![
            "CREATE TABLE \"events\" (\"id\" BIGSERIAL PRIMARY KEY, \
             \"slug\" VARCHAR(64) NOT NULL DEFAULT 'none', \"views\" INTEGER NOT NULL DEFAULT 0, \
             \"live\" BOOLEAN NOT NULL DEFAULT true, \"starts_at\" TIMESTAMP(0) WITHOUT TIME ZONE NULL, \
             \"created_at\" TIMESTAMP(0) WITHOUT TIME ZONE NULL, \
             \"updated_at\" TIMESTAMP(0) WITHOUT TIME ZONE NULL)",
            "CREATE INDEX \"events_slug_views_index\" ON \"events\" (\"slug\", \"views\")"
        ]
    );
}

#[cfg(feature = "sqlite")]
#[test]
fn renders_extras_sqlite() {
    assert_eq!(
        extras().to_sql(Driver::Sqlite),
        vec![
            "CREATE TABLE \"events\" (\"id\" INTEGER PRIMARY KEY AUTOINCREMENT, \
             \"slug\" TEXT NOT NULL DEFAULT 'none', \"views\" INTEGER NOT NULL DEFAULT 0, \
             \"live\" TINYINT(1) NOT NULL DEFAULT 1, \"starts_at\" DATETIME NULL, \
             \"created_at\" DATETIME NULL, \"updated_at\" DATETIME NULL)",
            "CREATE INDEX \"events_slug_views_index\" ON \"events\" (\"slug\", \"views\")"
        ]
    );
}

#[cfg(feature = "mysql")]
#[test]
fn escapes_text_defaults() {
    let s = schema::create("t", |t| {
        t.string("a").default("it's a \\ test");
    });
    assert_eq!(
        s.to_sql(Driver::MySql),
        vec!["CREATE TABLE `t` (`a` VARCHAR(255) NOT NULL DEFAULT 'it''s a \\\\ test')"]
    );
}

#[test]
fn drops() {
    let q = if cfg!(feature = "mysql") {
        "`posts`"
    } else {
        "\"posts\""
    };
    assert_eq!(
        schema::drop("posts").to_sql(any()),
        vec![format!("DROP TABLE {q}")]
    );
    assert_eq!(
        schema::drop_if_exists("posts").to_sql(any()),
        vec![format!("DROP TABLE IF EXISTS {q}")]
    );
}

#[test]
fn rejects_invalid_names() {
    for bad in ["", "a`b", "a\"b"] {
        let table = schema::create(bad, |t| {
            t.id();
        });
        assert!(matches!(
            table.try_to_sql(any()),
            Err(Error::InvalidIdentifier(_))
        ));
        let column = schema::create("t", |t| {
            t.string(bad);
        });
        assert!(matches!(
            column.try_to_sql(any()),
            Err(Error::InvalidIdentifier(_))
        ));
        let index = schema::create("t", |t| {
            t.string("a");
            t.index([bad]);
        });
        assert!(matches!(
            index.try_to_sql(any()),
            Err(Error::InvalidIdentifier(_))
        ));
        let foreign = schema::create("t", |t| {
            t.foreign("a").references(bad, "id");
        });
        assert!(matches!(
            foreign.try_to_sql(any()),
            Err(Error::InvalidIdentifier(_))
        ));
        assert!(matches!(
            schema::drop(bad).try_to_sql(any()),
            Err(Error::InvalidIdentifier(_))
        ));
    }
}

#[test]
fn rejects_a_default_that_has_no_literal() {
    let s = schema::create("t", |t| {
        t.string("a").default(Value::Bytes(vec![1]));
    });
    assert!(matches!(
        s.try_to_sql(any()),
        Err(Error::Unsupported { .. })
    ));
}

#[test]
fn a_foreign_key_without_a_target_is_an_error() {
    let s = schema::create("t", |t| {
        t.big_integer("a");
        t.foreign("a");
    });
    assert!(s.try_to_sql(any()).is_err());
}

#[test]
fn foreign_actions() {
    let s = schema::create("t", |t| {
        t.big_integer("a").nullable();
        t.foreign("a")
            .references("u", "id")
            .on_delete(Action::SetNull)
            .on_update(Action::Cascade);
    });
    assert!(s.to_sql(any())[0].ends_with("ON DELETE SET NULL ON UPDATE CASCADE)"));
}

async fn run(db: &Database, statement: &schema::Statement) {
    for sql in statement.to_sql(db.driver()) {
        raw::execute(db, &sql, &[])
            .await
            .unwrap_or_else(|e| panic!("`{sql}` failed: {e}"));
    }
}

#[tokio::test]
async fn the_rendered_sql_runs_on_the_database() {
    let Some(url) = common::url() else {
        eprintln!("skipped: ORBIT_DATABASE_URL is not set");
        return;
    };
    let db = Database::connect(&url).await.expect("connects");
    for table in ["schema_events", "schema_posts", "schema_users"] {
        run(&db, &schema::drop_if_exists(table)).await;
    }

    run(
        &db,
        &schema::create("schema_users", |t: &mut Blueprint| {
            t.id();
            t.string("name");
        }),
    )
    .await;
    run(
        &db,
        &schema::create("schema_posts", |t| {
            t.id();
            t.string("title");
            t.text("body").nullable();
            t.big_integer("user_id");
            t.foreign("user_id")
                .references("schema_users", "id")
                .on_delete(Action::Restrict);
            t.unique(["title"]);
        }),
    )
    .await;
    run(
        &db,
        &schema::create("schema_events", |t| {
            t.id();
            t.string("slug").length(64).default("none");
            t.integer("views").default(0);
            t.boolean("live").default(true);
            t.date_time("starts_at").nullable();
            t.timestamps();
            t.index(["slug", "views"]);
        }),
    )
    .await;

    raw::execute(&db, "INSERT INTO schema_users (name) VALUES ('ada')", &[])
        .await
        .expect("inserts a user");
    let insert = "INSERT INTO schema_posts (title, user_id) VALUES ('hello', 1)";
    raw::execute(&db, insert, &[])
        .await
        .expect("inserts a post");

    // The unique index rejects a second post with the same title.
    assert!(
        raw::execute(&db, insert, &[]).await.is_err(),
        "unique index is enforced"
    );

    // The foreign key restricts deleting a user who has posts.
    let restricted = raw::execute(&db, "DELETE FROM schema_users WHERE id = 1", &[]).await;
    assert!(restricted.is_err(), "ON DELETE RESTRICT is enforced");

    raw::execute(
        &db,
        "INSERT INTO schema_events (starts_at) VALUES (NULL)",
        &[],
    )
    .await
    .expect("defaults apply");

    for table in ["schema_events", "schema_posts", "schema_users"] {
        run(&db, &schema::drop(table)).await;
    }
}
