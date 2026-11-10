# orbit-orm

Orbit is an ORM for Rocket.rs apps, built to replace Diesel in the RocketBoost family (Propel, Ignite). The crate is `orbit-orm`.

**Status:** early. The connection layer works against MariaDB 13.0.2. Models and queries work; relations, soft deletes and migrations do not yet.

## Scope

1. **MySQL and MariaDB first.** Connections, queries, and migrations for the same database Ignite uses today.
2. **PostgreSQL and SQLite next,** once the MySQL path passes the same tests.

Orbit is used by Ignite, which will move from Diesel to Orbit once Orbit covers what Ignite needs. Until then, Ignite stays on Diesel.

## Design

- The driver is [`sqlx`](https://crates.io/crates/sqlx), which provides the connection pool and the wire protocol. Orbit adds the ORM layer above it, the way Laravel's Eloquent sits on PDO.
- `sqlx` is async, so Orbit fits Rocket's async handlers.

## Usage

```rust
use orbit_orm::Database;

let db = Database::connect("mysql://user:password@127.0.0.1/app").await?;
db.ping().await?;
```

## Migrations

A migration is a folder named `<timestamp>_<name>`, such as `20261108000000_create_users`, with an `up.sql` and a `down.sql`. The timestamp is 14 digits, `YYYYMMDDHHMMSS`, the same format Ignite's migrations use. Migrations sort by name. A name without a timestamp is an error, and so are two migrations with the same name or the same timestamp. The `migrations` table records each applied migration with its batch number, as Laravel does.

```rust
use orbit_orm::{Database, Migrator};

let db = Database::connect(&url).await?;
let migrator = Migrator::from_dir("migrations")?;

migrator.run(&db).await?;                  // applies every pending migration as one batch
let status = migrator.status(&db).await?;  // each migration as Applied (with its batch) or Pending
migrator.rollback(&db).await?;             // reverts the last batch, in reverse order
```

`migrations!("migrations", "20261108000000_create_users")` embeds the files at build time instead. The names are listed by hand, and a missing file is a build error.

Each migration runs in its own transaction, and the run stops at the first failure, which the error names. MySQL does not roll back DDL, so a failing migration can leave the DDL it ran before the failure in place. Only its DML is rolled back.

`run` and `rollback` hold a MySQL named lock, `orbit_migrations`, so two processes cannot migrate at once. A second process waits for the lock up to 10 seconds (change it with `Migrator::with_lock_timeout`), then fails with `MigrationsLocked`.

## Models and queries

A model maps a table to a Rust type. Rows are read with sqlx's `FromRow`, and the model names its table and the columns it may write:

```rust
use orbit_orm::{Database, Direction, Model, Operator, Query};
use sqlx::mysql::MySqlRow;
use sqlx::{FromRow, Row};

struct Post {
    id: i64,
    title: String,
    views: i64,
}

impl<'r> FromRow<'r, MySqlRow> for Post {
    fn from_row(row: &'r MySqlRow) -> Result<Self, sqlx::Error> {
        Ok(Self {
            id: row.try_get("id")?,
            title: row.try_get("title")?,
            views: row.try_get("views")?,
        })
    }
}

impl Model for Post {
    const TABLE: &'static str = "posts";
    const FILLABLE: &'static [&'static str] = &["title", "views"];
}

let popular = Post::query()
    .where_op("views", Operator::Ge, 100)
    .order_by("views", Direction::Desc)
    .limit(10)
    .get(&db)
    .await?;

let post = Post::query().find_or_fail(&db, 1).await?;
let id = Query::<Post>::table().insert(&db, &[("title", "Hello".into())]).await?;
```

Values are always bound as parameters. Table and column names are quoted, and a name that is empty or contains a backtick is an error.

## Tests

The database tests need a MySQL or MariaDB test database:

```sh
ORBIT_DATABASE_URL='mysql://user:password@127.0.0.1:3306/orbit_test' cargo test
```

Without `ORBIT_DATABASE_URL`, the database tests are skipped.

## Documentation

The docs site lives in [RocketOrbit](https://github.com/lajennylove/RocketOrbit).

## License

MIT. Copyright (c) 2026 Jenny Martinez.
