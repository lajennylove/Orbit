//! `schema::statement` runs raw SQL inside a migration, and `Statement::only` limits any schema
//! statement to one database family, after Laravel's `DB::statement` and its driver check. The
//! render tests run per enabled driver; the database tests use `ORBIT_DATABASE_URL` and skip with
//! a notice when it is not set.

mod common;

use tokio::sync::{Mutex, MutexGuard};

use orbit_orm::schema::{self, Dialect};
use orbit_orm::{Database, Driver, Migration, Migrator, State, Value, raw};

/// Every driver built with the current features.
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

/// A dialect that is not `driver`'s.
fn other(driver: Driver) -> Dialect {
    match Dialect::of(driver) {
        Dialect::MySql => Dialect::Postgres,
        Dialect::Postgres | Dialect::Sqlite => Dialect::MySql,
    }
}

#[cfg(feature = "mysql")]
#[test]
fn dialect_of_mysql() {
    assert_eq!(Dialect::of(Driver::MySql), Dialect::MySql);
}

#[cfg(feature = "postgres")]
#[test]
fn dialect_of_postgres() {
    assert_eq!(Dialect::of(Driver::Postgres), Dialect::Postgres);
}

#[cfg(feature = "sqlite")]
#[test]
fn dialect_of_sqlite() {
    assert_eq!(Dialect::of(Driver::Sqlite), Dialect::Sqlite);
}

#[cfg(feature = "mysql")]
#[test]
fn statement_renders_as_written_on_mysql() {
    let sql = "ALTER TABLE users MODIFY avatar MEDIUMBLOB";
    assert_eq!(schema::statement(sql).to_sql(Driver::MySql), vec![sql]);
}

#[cfg(feature = "postgres")]
#[test]
fn statement_renders_as_written_on_postgres() {
    let sql = "INSERT INTO roles (name) VALUES ('admin')";
    assert_eq!(schema::statement(sql).to_sql(Driver::Postgres), vec![sql]);
}

#[cfg(feature = "sqlite")]
#[test]
fn statement_renders_as_written_on_sqlite() {
    let sql = "INSERT INTO roles (name) VALUES ('admin')";
    assert_eq!(schema::statement(sql).to_sql(Driver::Sqlite), vec![sql]);
}

#[test]
fn statement_is_not_checked() {
    // Orbit does not parse, quote or validate the SQL: the database reports a wrong string.
    for driver in drivers() {
        assert_eq!(
            schema::statement("not `valid\" sql")
                .try_to_sql(driver)
                .unwrap(),
            vec!["not `valid\" sql"]
        );
        assert_eq!(
            schema::statement(String::new()).try_to_sql(driver).unwrap(),
            vec![""]
        );
    }
}

#[test]
fn only_renders_on_the_matching_dialect() {
    for driver in drivers() {
        let statement = schema::statement("SELECT 1").only(Dialect::of(driver));
        assert_eq!(statement.to_sql(driver), vec!["SELECT 1"]);
    }
}

#[test]
fn only_renders_nothing_on_another_dialect() {
    for driver in drivers() {
        let statement = schema::statement("SELECT 1").only(other(driver));
        assert_eq!(statement.try_to_sql(driver).unwrap(), Vec::<String>::new());
    }
}

#[test]
fn only_works_on_create() {
    for driver in drivers() {
        let create = || {
            schema::create("posts", |t| {
                t.id();
                t.string("title");
            })
        };
        assert_eq!(
            create().only(Dialect::of(driver)).to_sql(driver),
            create().to_sql(driver)
        );
        assert!(create().only(other(driver)).to_sql(driver).is_empty());
    }
}

#[test]
fn only_works_on_table_and_drop() {
    for driver in drivers() {
        let alter = || schema::table("posts", |t| t.rename_column("a", "b"));
        assert_eq!(
            alter().only(Dialect::of(driver)).to_sql(driver),
            alter().to_sql(driver)
        );
        assert!(alter().only(other(driver)).to_sql(driver).is_empty());
        assert_eq!(
            schema::drop("posts")
                .only(Dialect::of(driver))
                .to_sql(driver),
            schema::drop("posts").to_sql(driver)
        );
        assert!(
            schema::drop("posts")
                .only(other(driver))
                .to_sql(driver)
                .is_empty()
        );
    }
}

#[test]
fn only_skips_validation_on_another_dialect() {
    // A statement for another database is not rendered, so its names are not checked either,
    // like the body of Laravel's `if` around the call.
    for driver in drivers() {
        let bad = schema::drop("bad`name");
        assert!(bad.clone().try_to_sql(driver).is_err());
        assert!(
            bad.only(other(driver))
                .try_to_sql(driver)
                .unwrap()
                .is_empty()
        );
    }
}

#[test]
fn only_twice_keeps_the_last_call() {
    for driver in drivers() {
        let here = Dialect::of(driver);
        let there = other(driver);
        assert_eq!(
            schema::statement("SELECT 1")
                .only(there)
                .only(here)
                .to_sql(driver),
            vec!["SELECT 1"]
        );
        assert!(
            schema::statement("SELECT 1")
                .only(here)
                .only(there)
                .to_sql(driver)
                .is_empty()
        );
    }
}

static LOCK: Mutex<()> = Mutex::const_new(());

async fn setup() -> Option<(MutexGuard<'static, ()>, Database)> {
    let Some(url) = common::url() else {
        eprintln!("skipped: set ORBIT_DATABASE_URL to run the raw statement tests");
        return None;
    };
    let guard = LOCK.lock().await;
    let db = Database::connect(&url).await.expect("connects");
    common::drop_tables(&db, &["ss_roles", "ss_tags", "__diesel_schema_migrations"]).await;
    Some((guard, db))
}

async fn names(db: &Database, table: &str) -> Vec<String> {
    raw::fetch_all::<(String,)>(db, &format!("SELECT name FROM {table} ORDER BY name"), &[])
        .await
        .expect("selects")
        .into_iter()
        .map(|(name,)| name)
        .collect()
}

async fn run(db: &Database, statement: &schema::Statement) {
    for sql in statement.to_sql(db.driver()) {
        raw::execute(db, &sql, &[]).await.expect("runs");
    }
}

#[tokio::test]
async fn statement_runs_on_the_database() {
    let Some((_lock, db)) = setup().await else {
        return;
    };
    let driver = db.driver();
    run(
        &db,
        &schema::create("ss_roles", |t| {
            t.id();
            t.string("name");
        }),
    )
    .await;
    run(
        &db,
        &schema::statement("INSERT INTO ss_roles (name) VALUES ('admin')"),
    )
    .await;
    // A statement for another database does nothing here; it would fail if it ran.
    let elsewhere =
        schema::statement("INSERT INTO ss_missing (name) VALUES ('x')").only(other(driver));
    assert!(elsewhere.to_sql(driver).is_empty());
    run(&db, &elsewhere).await;
    run(
        &db,
        &schema::statement("INSERT INTO ss_roles (name) VALUES ('editor')")
            .only(Dialect::of(driver)),
    )
    .await;

    assert_eq!(names(&db, "ss_roles").await, vec!["admin", "editor"]);
    run(&db, &schema::drop("ss_roles")).await;
    let count: i64 = raw::fetch_scalar(
        &db,
        common::table_count_sql(driver),
        &[Value::from("ss_roles")],
    )
    .await
    .expect("reads the catalog");
    assert_eq!(count, 0);
}

#[tokio::test]
async fn migration_mixes_builder_and_raw_statements() {
    let Some((_lock, db)) = setup().await else {
        return;
    };
    let driver = db.driver();
    let migration = Migration::schema(
        "20270201000000_create_ss_tags",
        vec![
            schema::create("ss_tags", |t| {
                t.id();
                t.string("name");
            }),
            schema::statement("INSERT INTO ss_tags (name) VALUES ('news')"),
            // Renders nothing on this database, so no empty SQL is sent.
            schema::statement("INSERT INTO ss_missing (name) VALUES ('x')").only(other(driver)),
            schema::statement("INSERT INTO ss_tags (name) VALUES ('sport')")
                .only(Dialect::of(driver)),
        ],
        vec![
            schema::statement("DELETE FROM ss_tags"),
            schema::statement("DROP TABLE ss_missing").only(other(driver)),
            schema::drop("ss_tags"),
        ],
    );
    let migrator = Migrator::new(vec![migration]).expect("valid");

    let ran = migrator.run(&db).await.expect("runs");
    assert_eq!(ran, vec!["20270201000000_create_ss_tags"]);
    assert_eq!(names(&db, "ss_tags").await, vec!["news", "sport"]);
    let status = migrator.status(&db).await.expect("status");
    assert_eq!(status[0].state, State::Applied { batch: 1 });

    let reverted = migrator.rollback(&db).await.expect("rolls back");
    assert_eq!(reverted, vec!["20270201000000_create_ss_tags"]);
    let count: i64 = raw::fetch_scalar(
        &db,
        common::table_count_sql(driver),
        &[Value::from("ss_tags")],
    )
    .await
    .expect("reads the catalog");
    assert_eq!(count, 0);
    let status = migrator.status(&db).await.expect("status");
    assert_eq!(status[0].state, State::Pending);

    common::drop_tables(&db, &["__diesel_schema_migrations"]).await;
}
