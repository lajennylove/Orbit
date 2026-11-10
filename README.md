# orbit-orm

Orbit is an ORM for Rocket.rs apps, built to replace Diesel in the RocketBoost family (Propel, Ignite). The crate is `orbit-orm`.

**Status:** early. The connection layer works against MariaDB 13.0.2. No models or queries yet.

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

A migration is a folder named `<timestamp>_<name>` with an `up.sql` and a `down.sql`, the same layout Ignite uses. The `migrations` table records each applied migration with its batch number, as Laravel does. Migrations sort by name, and a duplicate name is an error.

```rust
use orbit_orm::{Database, Migrator};

let db = Database::connect(&url).await?;
let migrator = Migrator::from_dir("migrations")?;

migrator.run(&db).await?;                  // applies every pending migration as one batch
let status = migrator.status(&db).await?;  // each migration as Applied (with its batch) or Pending
migrator.rollback(&db).await?;             // reverts the last batch, in reverse order
```

`migrations!("migrations", "20261108000000_create_users")` embeds the files at build time instead. The names are listed by hand, and a missing file is a build error.

Each migration runs in its own transaction, and the run stops at the first failure, which the error names. MySQL does not roll back DDL, so a failing migration can leave the DDL it ran before the failure in place. Only its DML is rolled back. Run migrations from one process at a time, as a deploy step.

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
