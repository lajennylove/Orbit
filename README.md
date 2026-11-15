# orbit-orm

Orbit is an ORM for Rocket.rs apps. It works with MySQL and MariaDB, PostgreSQL and SQLite.

## Install

```sh
cargo add orbit-orm
```

Each database is a Cargo feature. `mysql` is on by default; enable the others, or turn `mysql` off:

| Feature    | Database                     | Default |
|------------|------------------------------|---------|
| `mysql`    | MySQL and MariaDB            | yes     |
| `postgres` | PostgreSQL                   | no      |
| `sqlite`   | SQLite (file or memory)      | no      |

```sh
cargo add orbit-orm --features postgres
cargo add orbit-orm --no-default-features --features sqlite,derive
```

For the synchronous API, also enable the `blocking` feature:

```sh
cargo add orbit-orm --features blocking
```

## Connect

The database is chosen by the URL's scheme. A scheme whose feature is not enabled is an error.

| Database   | URL                                              |
|------------|--------------------------------------------------|
| MySQL      | `mysql://user:password@host:3306/database`       |
| MariaDB    | `mariadb://user:password@host:3306/database`     |
| PostgreSQL | `postgres://user:password@host:5432/database`    |
| SQLite     | `sqlite:///absolute/path/app.db?mode=rwc`, or `sqlite://relative/app.db?mode=rwc` |

`mode=rwc` creates the SQLite file when it does not exist. A PostgreSQL URL must name the user: without one, sqlx connects as the role `anonymous`, which usually does not exist. `sqlite::memory:` is an in-memory database, one per connection.

MySQL:

```rust
use orbit_orm::Database;

let db = Database::connect("mysql://user:password@127.0.0.1/app").await?;
db.ping().await?;
```

PostgreSQL:

```rust
use orbit_orm::Database;

let db = Database::connect("postgres://user:password@127.0.0.1/app").await?;
db.ping().await?;
```

SQLite:

```rust
use orbit_orm::Database;

let db = Database::connect("sqlite:///var/lib/app/app.db?mode=rwc").await?;
db.ping().await?;
```

Call `db.close().await` to shut the connection pool down.

`connect` opens at most 5 connections and waits up to 10 seconds for one. To change that, pass `PoolOptions` to `connect_with`. Fields you leave out keep their defaults:

```rust
use std::time::Duration;
use orbit_orm::{Database, PoolOptions};

let db = Database::connect_with(
    "mysql://user:password@127.0.0.1/app",
    PoolOptions {
        max_connections: 10,
        acquire_timeout: Duration::from_secs(5),
        min_connections: 2,
    },
)
.await?;
```

## Models and queries

A model maps a table to a Rust type and names the columns it may write. The `OrbitModel` derive writes the row mapping and the model from attributes. Without `table`, the table is the plural of the struct name, as in Laravel: `Flight` uses `flights`, `UserProfile` uses `user_profiles`.

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

Methods: `get`, `first`, `first_or_fail`, `find`, `find_or_fail`, `count`, `exists`, `insert`, `update`, `delete`, `replace`, `get_as` and `first_as`.

`get_as` and `first_as` take the same clauses as `get` and `first`, but decode each row as any type that implements sqlx's `FromRow`. With `select`, that reads only some columns, into a tuple or a struct with only those fields:

```rust
let titles = Post::query()
    .select(&["id", "title"])
    .order_by("id", Direction::Asc)
    .get_as::<(i64, String)>(&db)
    .await?;
```
 Filters: `where_eq`, `where_op`, `where_in`, `where_null`, `where_not_null`, and the `or_where_*` variants, which join with `OR`. Sorting and paging: `order_by`, `select`, `limit`, `offset`. Row locks: `for_update`.

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

## Timestamps and bytes

`Value` has `DateTime(NaiveDateTime)`, `Date(NaiveDate)` and `Bytes(Vec<u8>)`, with `From` impls. They bind to `DATETIME` (with microseconds), `DATE` and `BLOB` columns, and the same types read back. A model field can use `NaiveDateTime`, `NaiveDate` or `Vec<u8>`:

```rust
use chrono::{NaiveDate, NaiveDateTime};

let published = NaiveDateTime::parse_from_str("2026-11-10 18:35:07.123456", "%Y-%m-%d %H:%M:%S%.f")?;
let id = Query::<Post>::table()
    .insert(
        &db,
        &[
            ("published_at", Value::from(published)),
            ("day", Value::from(NaiveDate::from_ymd_opt(2026, 11, 10).unwrap())),
            ("thumbnail", Value::from(vec![0x89, 0x50, 0x00])),
        ],
    )
    .await?;
```

A `NaiveDateTime` has no time zone, and Orbit does not convert it.

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

In async code, call it from `tokio::task::spawn_blocking`. See [docs/blocking.md](docs/blocking.md) for how it runs and how its transactions roll back.

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

`for_update` locks the matching rows. In a transaction the lock lasts until it ends. Outside one, the statement runs in autocommit and the lock lasts only for that statement, as in Laravel. `tx.connection()` gives the raw connection for SQL that Orbit does not wrap.

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

## Raw SQL

`orbit_orm::raw` runs SQL that the query builder does not cover. Each function takes `&db` or `&mut *tx`, as the terminal methods do:

```rust
use orbit_orm::{Value, raw};

let changed = raw::execute(&db, "UPDATE posts SET views = views + 1 WHERE id = ?", &[Value::from(1)]).await?;
let popular: Vec<(i64, String)> = raw::fetch_all(&db, "SELECT id, title FROM posts WHERE views >= ?", &[Value::from(100)]).await?;
let post: Option<(i64, String)> = raw::fetch_optional(&db, "SELECT id, title FROM posts WHERE id = ?", &[Value::from(1)]).await?;
let count: i64 = raw::fetch_scalar(&db, "SELECT COUNT(*) FROM posts", &[]).await?;
```

- `execute` returns the rows affected. `fetch_all` and `fetch_optional` decode each row as `T: FromRow`, and `fetch_scalar` decodes the first column of the first row. It is an error when no row comes back.
- The SQL string is not escaped and is not checked. It must not contain user input. Put every value in the binds, in the order of the `?` placeholders.
- `LAST_INSERT_ID()` belongs to one connection, so read it in the transaction that made the insert:

```rust
let id: u64 = db
    .transaction(async |tx| {
        raw::execute(&mut *tx, "INSERT INTO posts (title) VALUES (?)", &[Value::from("Hello")]).await?;
        raw::fetch_scalar(&mut *tx, "SELECT LAST_INSERT_ID()", &[]).await
    })
    .await?;
```

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

A migration can also be written in code with the schema builder, so one migration serves every database. `Migration::schema(name, up, down)` takes the `up` and `down` statements, and the migrator renders them for the connected database when it runs or rolls back the migration:

```rust
use orbit_orm::{schema, Migration, Migrator};

let users = Migration::schema(
    "20261108000000_create_users",
    vec![schema::create("users", |t| {
        t.id();
        t.string("email");
        t.unique(["email"]);
    })],
    vec![schema::drop_if_exists("users")],
);
let migrator = Migrator::new(vec![users])?;
```

A statement that cannot be rendered (an invalid table name, for example) fails the migration with that error before any of its SQL runs. Batches, the per-migration transaction and the DDL caveat below are the same as for SQL migrations. The public `up` and `down` fields of a schema migration are empty.

`migrations!("migrations", "20261108000000_create_users")` embeds the files in the binary at build time. List the names by hand; a missing file is a build error.

Migrations are recorded in Diesel's `__diesel_schema_migrations` table, so a database that Diesel migrated keeps its history. `run`, `status` and `rollback` add the `batch` column to a table that has only Diesel's columns, so nothing needs to be imported first. `unmatched_diesel_versions` lists the versions in the table that have no folder in the migrator.

Each recorded migration has its batch number. Each migration runs in its own transaction. MySQL does not roll back DDL, so a failing migration can leave the DDL it ran before the failure in place; only its DML is rolled back.

`run` and `rollback` hold a lock named after the database: a named lock on MySQL, an advisory lock on PostgreSQL. A second process waits up to 10 seconds, then fails with `MigrationsLocked`. Change the wait with `Migrator::with_lock_timeout`. SQLite has no named lock, so its lock covers one process only; see [docs/laravel-differences.md](docs/laravel-differences.md).

## Schema builder

`orbit_orm::schema` describes a table once and renders it for SQLite, MySQL and MariaDB, and PostgreSQL, after Laravel's `Schema::create` and `Blueprint`:

```rust
use orbit_orm::schema::{self, Action};

let posts = schema::create("posts", |t| {
    t.id();
    t.string("title");
    t.text("body").nullable();
    t.big_integer("user_id");
    t.foreign("user_id").references("users", "id").on_delete(Action::Restrict);
    t.unique(["title"]);
    t.timestamps();
});

for sql in posts.to_sql(db.driver()) {
    orbit_orm::raw::execute(&db, &sql, &[]).await?;
}
```

`to_sql` returns one SQL statement per element, to run in order: PostgreSQL and SQLite create a plain `index` with a separate `CREATE INDEX`. Columns are `NOT NULL` unless `nullable()`. `id()` is `BIGINT AUTO_INCREMENT PRIMARY KEY` on MySQL, `BIGSERIAL PRIMARY KEY` on PostgreSQL and `INTEGER PRIMARY KEY AUTOINCREMENT` on SQLite. `string` is `VARCHAR(255)` (`TEXT` on SQLite; change the length with `.length(n)`). Also: `text`, `integer`, `big_integer`, `boolean`, `date_time`, `timestamp`, `binary`, `timestamps`, `index`, `primary`, `schema::table`, `schema::create_if_not_exists`, `schema::drop`, `schema::drop_if_exists` and `schema::statement`. Names follow the query builder's rule, so `try_to_sql` returns `Error::InvalidIdentifier` for a name that is empty or has a quote character; `to_sql` panics in that case. An operation the database cannot do is `Error::Unsupported`.

### Changing a table

`schema::table` changes an existing table, as Laravel's `Schema::table`. A column described in the closure is added. A column that ends with `.change()` is modified: it takes the type, nullability and default described. `rename_column` and `drop_column` rename and drop columns:

```rust
use orbit_orm::schema;

let posts = schema::table("posts", |t| {
    t.string("subtitle").nullable();
    t.string("title").length(100).change();
    t.rename_column("body", "content");
    t.drop_column("legacy_slug");
});
```

Each operation is its own statement, run in the order it was called, as in Laravel 11 and later. So `t.drop_column("slug"); t.string("slug");` drops the column and adds it again, and `t.rename_column("name", "full_name"); t.string("name");` renames a column and adds a new one under the old name. SQLite cannot change a column in place, so `.change()` is `Error::Unsupported` there. On SQLite, a column added to a table that has rows cannot take `use_current` ("Cannot add a column with non-constant default"), and a `NOT NULL` column needs a default; Laravel has the same limit. `rename_column` needs MySQL 8.0.3 or MariaDB 10.5.2 or later, or SQLite 3.25 or later, and `drop_column` needs SQLite 3.35 or later.

### Indexes and keys

`index`, `unique` and `foreign` take `.name()` to choose the name. Without it the name is Laravel's: `{table}_{columns}_index`, `{table}_{columns}_unique` or `{table}_{column}_foreign`. `drop_index` and `drop_foreign` take the name:

```rust
use orbit_orm::schema::{self, Action};

let up = schema::table("posts", |t| {
    t.index(["user_id"]).name("posts_author_index");
    t.foreign("user_id").references("users", "id").on_delete(Action::Cascade);
});

let down = schema::table("posts", |t| {
    t.drop_foreign("posts_user_id_foreign");
    t.drop_index("posts_author_index");
});
```

`primary(["team_id", "user_id"])` adds a primary key over several columns. For one column, `.primary()` on the column does the same, for a key that is not an `id()`, such as `t.string("token").length(64).primary()`.

SQLite cannot add or drop a constraint on an existing table. There, a `foreign` in `schema::table` and `drop_foreign` render nothing, and `primary` in `schema::table` is `Error::Unsupported`. `drop_index` drops a unique index on MySQL, and on SQLite when `schema::table` added it. It does not drop a unique on PostgreSQL, and there is no `drop_unique`; see [docs/laravel-differences.md](docs/laravel-differences.md).

### Column types

`binary` is a column of raw bytes. `timestamp` is a `TIMESTAMP`, after Laravel's `timestamp`. `.precision(p)` sets the fractional seconds of a `date_time` or `timestamp`, from 0 to 6, and `.use_current()` makes the current time its default:

```rust
use orbit_orm::schema;

let sessions = schema::create("sessions", |t| {
    t.string("id").length(128).primary();
    t.binary("payload");
    t.timestamp("last_seen_at").precision(6).use_current();
    t.date_time("expires_at").nullable();
});
```

`binary` is `BLOB`, or `BYTEA` on PostgreSQL. `timestamp` is `TIMESTAMP` on MySQL, `TIMESTAMP(0) WITHOUT TIME ZONE` on PostgreSQL and `DATETIME` on SQLite. SQLite keeps `DATETIME` whatever the precision. `use_current` is `DEFAULT CURRENT_TIMESTAMP`, or `CURRENT_TIMESTAMP(p)` on MySQL when the precision is above 0.

### Creating a table if it does not exist

`schema::create_if_not_exists` renders `CREATE TABLE IF NOT EXISTS`. An existing table is left as it is, even when its columns differ:

```rust
use orbit_orm::schema;

let users = schema::create_if_not_exists("users", |t| {
    t.id();
    t.string("email");
    t.string("name");
    t.unique(["email"]);
    t.index(["name"]);
});
```

Laravel has no such method; a Laravel migration checks `Schema::hasTable` first. On PostgreSQL and SQLite, a plain `index` takes `CREATE INDEX IF NOT EXISTS`, so it is still created when the table already exists.

### Raw statements

`schema::statement` runs SQL as written, as Laravel's `DB::statement` in a migration. `.only(Dialect)` limits a statement to one database family; on the others it renders nothing:

```rust
use orbit_orm::schema::{self, Dialect};

let seed = schema::statement("INSERT INTO roles (name) VALUES ('admin')");
let avatar = schema::statement("ALTER TABLE users MODIFY avatar MEDIUMBLOB").only(Dialect::MySql);
```

The SQL is database-specific. Orbit does not check, quote or translate it, and runs the same string on every database unless `only` limits it. It takes no bound values, so it must not contain user input. `only` works on every statement, including `create`, `table` and `drop`.

## Tests

The database tests run against the database that `ORBIT_DATABASE_URL` names. Run them once per database, with the matching feature:

```sh
ORBIT_DATABASE_URL='mysql://user:password@127.0.0.1:3306/orbit_test' cargo test
ORBIT_DATABASE_URL='postgres://user:password@127.0.0.1:5432/orbit_test' cargo test --no-default-features --features postgres
ORBIT_DATABASE_URL='sqlite:///tmp/orbit_test.db?mode=rwc' cargo test --no-default-features --features sqlite
```

Without `ORBIT_DATABASE_URL`, the database tests are skipped. The tests create and drop their own tables.

## Differences from Laravel

See [docs/laravel-differences.md](docs/laravel-differences.md).

## License

MIT. Copyright (c) 2026 Jenny Martinez.
