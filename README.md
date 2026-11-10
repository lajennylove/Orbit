# orbit-orm

Orbit is an ORM for Rocket.rs apps. It works with MySQL and MariaDB today.

## Install

```sh
cargo add orbit-orm --git https://github.com/lajennylove/Orbit
```

For the synchronous API, enable the `blocking` feature:

```sh
cargo add orbit-orm --features blocking --git https://github.com/lajennylove/Orbit
```

## Connect

```rust
use orbit_orm::Database;

let db = Database::connect("mysql://user:password@127.0.0.1/app").await?;
db.ping().await?;
```

Call `db.close().await` to shut the connection pool down.

## Models and queries

A model maps a table to a Rust type and names the columns it may write. The `OrbitModel` derive writes the row mapping and the model from attributes:

```rust
use orbit_orm::OrbitModel;

#[derive(OrbitModel)]
#[orbit(table = "posts")]
struct Post {
    id: i64,
    #[orbit(fillable)]
    title: String,
    #[orbit(column = "view_count", fillable)]
    views: Option<i64>,
}
```

`#[orbit(primary_key = "...")]` sets the primary key, and `#[orbit(skip)]` leaves a field out of the row.

Without the derive, write the two impls by hand. Rows are read with sqlx's `FromRow`:

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

Methods: `get`, `first`, `first_or_fail`, `find`, `find_or_fail`, `count`, `exists`, `insert`, `update`, `delete`, `replace`. Filters: `where_eq`, `where_op`, `where_in`, `where_null`, `where_not_null`, and the `or_where_*` variants, which join with `OR`. Sorting and paging: `order_by`, `select`, `limit`, `offset`. Row locks: `for_update`, inside a transaction only.

Conditions are grouped in parentheses with `where_group` and `or_where_group`:

```rust
let posts = Post::query()
    .where_op("views", Operator::Ge, 100)
    .where_group(|q| q.where_eq("title", "Hello").or_where_eq("title", "Hi"))
    .get(&db)
    .await?;
```

Values are always bound as parameters. Table and column names are quoted, and a name that is empty or contains a backtick is an error.

`insert` and `update` accept only columns listed in `FILLABLE`. Any other column returns `Error::NotFillable`.

`replace` writes a row the way MySQL's `REPLACE INTO` does: it overwrites the row that has the same unique key, and returns the rows affected.

```rust
let affected = Query::<Post>::table()
    .replace(&db, &[("title", "Hello".into())])
    .await?;
```

## Blocking API

With the `blocking` feature, `orbit_orm::blocking` has the same terminal methods without `async`. Build the query as usual, then call `blocking()`:

```rust
use orbit_orm::blocking::Database;

let db = Database::connect("mysql://user:password@127.0.0.1/app")?;
let post = Post::query().blocking().find_or_fail(&db, 1)?;

db.transaction(|tx| {
    Post::query().blocking().insert(tx, &[("title", "Hello".into())])?;
    Ok::<_, orbit_orm::Error>(())
})?;
```

In async code, call it from `tokio::task::spawn_blocking`.

## Transactions

`transaction` runs a closure in a transaction. It commits when the closure returns `Ok` and rolls back when it returns `Err` or panics. Pass the transaction to queries as `&mut *tx`; a query given `&db` runs outside it.

```rust
use orbit_orm::{Database, Error, Model};

let id = db
    .transaction(async |tx| {
        let id = Post::query()
            .insert(&mut *tx, &[("title", "Hello".into())])
            .await?;
        Post::query().where_eq("id", id).for_update().first_or_fail(&mut *tx).await?;
        Ok::<_, Error>(id)
    })
    .await?;
```

`for_update` locks the matching rows until the transaction ends. Outside a transaction it returns `Error::LockOutsideTransaction`. `tx.connection()` gives the raw connection for SQL that Orbit does not wrap.

## Errors

Some database errors have their own variants, so they can be matched:

```rust
match Post::query().insert(&db, &[("title", "Hello".into())]).await {
    Ok(id) => println!("inserted {id}"),
    Err(Error::UniqueViolation { index }) => println!("unique index {index:?} was violated"),
    Err(Error::ForeignKeyViolation { constraint }) => println!("foreign key {constraint:?} was violated"),
    Err(Error::Deadlock) | Err(Error::LockWaitTimeout) => println!("retry the transaction"),
    Err(other) => return Err(other),
}
```

`UniqueViolation` and `ForeignKeyViolation` carry the index or constraint name when the server reports it. Other errors are `Error::Database`.

## Migrations

A migration is a folder named `<timestamp>_<name>`, such as `20261108000000_create_users`, with an `up.sql` and a `down.sql`. The timestamp is 14 digits (`YYYYMMDDHHMMSS`). Migrations run in name order.

```rust
use orbit_orm::{Database, Migrator};

let db = Database::connect(&url).await?;
let migrator = Migrator::from_dir("migrations")?;

migrator.run(&db).await?;                  // applies every pending migration as one batch
let status = migrator.status(&db).await?;  // each migration as Applied (with its batch) or Pending
migrator.rollback(&db).await?;             // reverts the last batch, in reverse order
```

`migrations!("migrations", "20261108000000_create_users")` embeds the files in the binary at build time. List the names by hand; a missing file is a build error.

Migrations are recorded in the `migrations` table with their batch number. Each migration runs in its own transaction. MySQL does not roll back DDL, so a failing migration can leave the DDL it ran before the failure in place; only its DML is rolled back.

`run` and `rollback` hold the MySQL lock `orbit_migrations`. A second process waits up to 10 seconds, then fails with `MigrationsLocked`. Change the wait with `Migrator::with_lock_timeout`.

## Tests

The database tests need a MySQL or MariaDB test database:

```sh
ORBIT_DATABASE_URL='mysql://user:password@127.0.0.1:3306/orbit_test' cargo test
```

Without `ORBIT_DATABASE_URL`, the database tests are skipped.

## Differences from Laravel

See [docs/laravel-differences.md](docs/laravel-differences.md).

## License

MIT. Copyright (c) 2026 Jenny Martinez.
