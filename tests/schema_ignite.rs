//! Acceptance test: Ignite's nine panel migrations, written with the schema builder.
//!
//! Ignite ships its migrations as MySQL `up.sql` and `down.sql` files. Here each one is written
//! once with `orbit_orm::schema`, under the same folder name, and must serve SQLite, MySQL and
//! MariaDB, and PostgreSQL. The render tests pin the exact SQL per driver; the database test runs
//! the migrations on `ORBIT_DATABASE_URL` (it skips with a notice when that is not set), checks the
//! data and keys they leave behind, and rolls everything back.

mod common;

use orbit_orm::schema::{self, Action, Dialect, Statement};
use orbit_orm::{Database, Driver, Migration, Migrator, State, Value, raw};

/// One Ignite migration: its folder name and the statements that apply and revert it.
struct Step {
    name: &'static str,
    up: Vec<Statement>,
    down: Vec<Statement>,
}

impl Step {
    fn migration(self) -> Migration {
        Migration::schema(self.name, self.up, self.down)
    }
}

/// The tables the migrations create, children before the tables they reference.
const TABLES: [&str; 9] = [
    "role_user",
    "role_permission",
    "permissions",
    "roles",
    "user_avatars",
    "password_reset_tokens",
    "sessions",
    "login_throttles",
    "users",
];

/// The nine migrations, in order.
fn steps() -> Vec<Step> {
    vec![
        Step {
            name: "20261108000000_create_users",
            up: vec![schema::create_if_not_exists("users", |t| {
                t.id();
                t.string("name");
                t.string("email");
                t.string("password_hash");
                t.date_time("created_at").use_current();
                t.unique(["email"]);
            })],
            down: vec![schema::drop_if_exists("users")],
        },
        Step {
            name: "20261108010000_create_sessions",
            up: vec![schema::create_if_not_exists("sessions", |t| {
                t.string("id").length(64).primary();
                t.big_integer("user_id");
                t.date_time("expires_at");
                t.date_time("created_at").use_current();
                t.index(["user_id"]).name("sessions_user_index");
            })],
            down: vec![schema::drop_if_exists("sessions")],
        },
        Step {
            name: "20261108020000_create_login_throttles",
            up: vec![schema::create_if_not_exists("login_throttles", |t| {
                t.string("throttle_key").primary();
                t.integer("hits");
                t.date_time("window_start");
            })],
            down: vec![schema::drop_if_exists("login_throttles")],
        },
        Step {
            name: "20261110000000_add_user_theme",
            up: vec![schema::table("users", |t| {
                t.string("theme").length(8).default("light");
            })],
            down: vec![schema::table("users", |t| {
                t.drop_column("theme");
            })],
        },
        Step {
            name: "20261111000000_profile",
            up: vec![
                schema::table("users", |t| {
                    t.string("avatar_mime").length(32).nullable();
                    t.text("profile").nullable();
                }),
                schema::create_if_not_exists("user_avatars", |t| {
                    t.big_integer("user_id").primary();
                    t.binary("data");
                    t.foreign("user_id")
                        .references("users", "id")
                        .on_delete(Action::Cascade)
                        .name("user_avatars_user_fk");
                }),
                // A picture can be up to 1 MB, more than a MySQL BLOB holds (Ruling R5).
                schema::statement("ALTER TABLE user_avatars MODIFY data MEDIUMBLOB NOT NULL")
                    .only(Dialect::MySql),
            ],
            down: vec![
                schema::drop_if_exists("user_avatars"),
                schema::table("users", |t| {
                    t.drop_column("avatar_mime");
                    t.drop_column("profile");
                }),
            ],
        },
        Step {
            name: "20261112000000_login_throttles_window_index",
            up: vec![schema::table("login_throttles", |t| {
                t.index(["window_start"]);
            })],
            down: vec![schema::table("login_throttles", |t| {
                t.drop_index("login_throttles_window_start_index");
            })],
        },
        Step {
            name: "20261113000000_roles_permissions",
            up: vec![
                schema::create_if_not_exists("roles", |t| {
                    t.id();
                    t.string("name");
                    t.unique(["name"]);
                }),
                schema::create_if_not_exists("permissions", |t| {
                    t.id();
                    t.string("name");
                    t.unique(["name"]);
                }),
                schema::create_if_not_exists("role_permission", |t| {
                    t.big_integer("role_id");
                    t.big_integer("permission_id");
                    t.primary(["role_id", "permission_id"]);
                    t.foreign("role_id")
                        .references("roles", "id")
                        .on_delete(Action::Cascade)
                        .name("role_permission_role_fk");
                    t.foreign("permission_id")
                        .references("permissions", "id")
                        .on_delete(Action::Cascade)
                        .name("role_permission_permission_fk");
                }),
                schema::create_if_not_exists("role_user", |t| {
                    t.big_integer("user_id");
                    t.big_integer("role_id");
                    t.primary(["user_id", "role_id"]);
                    t.foreign("user_id")
                        .references("users", "id")
                        .on_delete(Action::Cascade)
                        .name("role_user_user_fk");
                    t.foreign("role_id")
                        .references("roles", "id")
                        .on_delete(Action::Cascade)
                        .name("role_user_role_fk");
                }),
                // The users who exist when roles arrive become super admins, so nobody loses
                // access on upgrade.
                schema::statement(SEED_ROLE),
                schema::statement(GRANT_ROLE),
            ],
            down: vec![
                schema::drop_if_exists("role_user"),
                schema::drop_if_exists("role_permission"),
                schema::drop_if_exists("permissions"),
                schema::drop_if_exists("roles"),
            ],
        },
        Step {
            name: "20261114000000_account_lifecycle",
            up: vec![
                schema::table("users", |t| {
                    t.date_time("disabled_at").nullable();
                }),
                // Sessions whose user is already gone would break the new key.
                schema::statement(DELETE_ORPHAN_SESSIONS),
                schema::table("sessions", |t| {
                    t.foreign("user_id")
                        .references("users", "id")
                        .on_delete(Action::Cascade)
                        .name("sessions_user_fk");
                }),
            ],
            down: vec![
                schema::table("sessions", |t| {
                    t.drop_foreign("sessions_user_fk");
                }),
                schema::table("users", |t| {
                    t.drop_column("disabled_at");
                }),
            ],
        },
        Step {
            name: "20261115000000_password_reset_tokens",
            up: vec![schema::create_if_not_exists("password_reset_tokens", |t| {
                t.string("email").primary();
                t.string("token");
                t.date_time("created_at").precision(6);
            })],
            down: vec![schema::drop_if_exists("password_reset_tokens")],
        },
    ]
}

const SEED_ROLE: &str = "INSERT INTO roles (name) VALUES ('super-admin')";
const GRANT_ROLE: &str = "INSERT INTO role_user (user_id, role_id) \
     SELECT users.id, roles.id FROM users CROSS JOIN roles WHERE roles.name = 'super-admin'";
const DELETE_ORPHAN_SESSIONS: &str =
    "DELETE FROM sessions WHERE user_id NOT IN (SELECT id FROM users)";

/// Every statement of a list rendered for `driver`, in order, as the migrator runs them.
fn render(statements: &[Statement], driver: Driver) -> Vec<String> {
    statements.iter().flat_map(|s| s.to_sql(driver)).collect()
}

/// Checks the rendered up and down SQL of every migration against `expected`, which lists, per
/// migration in order, its name, its up SQL and its down SQL.
fn assert_renders(driver: Driver, expected: &[(&str, &[&str], &[&str])]) {
    let steps = steps();
    assert_eq!(steps.len(), expected.len());
    for (step, (name, up, down)) in steps.iter().zip(expected) {
        assert_eq!(step.name, *name);
        assert_eq!(render(&step.up, driver), *up, "up of {name}");
        assert_eq!(render(&step.down, driver), *down, "down of {name}");
    }
}

#[cfg(feature = "mysql")]
#[test]
fn ignite_migrations_render_on_mysql() {
    assert_renders(
        Driver::MySql,
        &[
            (
                "20261108000000_create_users",
                &[
                    "CREATE TABLE IF NOT EXISTS `users` (`id` BIGINT AUTO_INCREMENT PRIMARY KEY, \
                   `name` VARCHAR(255) NOT NULL, `email` VARCHAR(255) NOT NULL, \
                   `password_hash` VARCHAR(255) NOT NULL, \
                   `created_at` DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP, \
                   CONSTRAINT `users_email_unique` UNIQUE (`email`))",
                ],
                &["DROP TABLE IF EXISTS `users`"],
            ),
            (
                "20261108010000_create_sessions",
                &["CREATE TABLE IF NOT EXISTS `sessions` (\
                   `id` VARCHAR(64) NOT NULL PRIMARY KEY, `user_id` BIGINT NOT NULL, \
                   `expires_at` DATETIME NOT NULL, \
                   `created_at` DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP, \
                   INDEX `sessions_user_index` (`user_id`))"],
                &["DROP TABLE IF EXISTS `sessions`"],
            ),
            (
                "20261108020000_create_login_throttles",
                &["CREATE TABLE IF NOT EXISTS `login_throttles` (\
                   `throttle_key` VARCHAR(255) NOT NULL PRIMARY KEY, `hits` INTEGER NOT NULL, \
                   `window_start` DATETIME NOT NULL)"],
                &["DROP TABLE IF EXISTS `login_throttles`"],
            ),
            (
                "20261110000000_add_user_theme",
                &["ALTER TABLE `users` ADD COLUMN `theme` VARCHAR(8) NOT NULL DEFAULT 'light'"],
                &["ALTER TABLE `users` DROP COLUMN `theme`"],
            ),
            (
                "20261111000000_profile",
                &[
                    "ALTER TABLE `users` ADD COLUMN `avatar_mime` VARCHAR(32) NULL",
                    "ALTER TABLE `users` ADD COLUMN `profile` TEXT NULL",
                    "CREATE TABLE IF NOT EXISTS `user_avatars` (\
                     `user_id` BIGINT NOT NULL PRIMARY KEY, `data` BLOB NOT NULL, \
                     CONSTRAINT `user_avatars_user_fk` FOREIGN KEY (`user_id`) \
                     REFERENCES `users` (`id`) ON DELETE CASCADE)",
                    "ALTER TABLE user_avatars MODIFY data MEDIUMBLOB NOT NULL",
                ],
                &[
                    "DROP TABLE IF EXISTS `user_avatars`",
                    "ALTER TABLE `users` DROP COLUMN `avatar_mime`",
                    "ALTER TABLE `users` DROP COLUMN `profile`",
                ],
            ),
            (
                "20261112000000_login_throttles_window_index",
                &["ALTER TABLE `login_throttles` ADD INDEX \
                   `login_throttles_window_start_index` (`window_start`)"],
                &["ALTER TABLE `login_throttles` DROP INDEX `login_throttles_window_start_index`"],
            ),
            (
                "20261113000000_roles_permissions",
                &[
                    "CREATE TABLE IF NOT EXISTS `roles` (`id` BIGINT AUTO_INCREMENT PRIMARY KEY, \
                     `name` VARCHAR(255) NOT NULL, CONSTRAINT `roles_name_unique` UNIQUE (`name`))",
                    "CREATE TABLE IF NOT EXISTS `permissions` (\
                     `id` BIGINT AUTO_INCREMENT PRIMARY KEY, `name` VARCHAR(255) NOT NULL, \
                     CONSTRAINT `permissions_name_unique` UNIQUE (`name`))",
                    "CREATE TABLE IF NOT EXISTS `role_permission` (`role_id` BIGINT NOT NULL, \
                     `permission_id` BIGINT NOT NULL, PRIMARY KEY (`role_id`, `permission_id`), \
                     CONSTRAINT `role_permission_role_fk` FOREIGN KEY (`role_id`) \
                     REFERENCES `roles` (`id`) ON DELETE CASCADE, \
                     CONSTRAINT `role_permission_permission_fk` FOREIGN KEY (`permission_id`) \
                     REFERENCES `permissions` (`id`) ON DELETE CASCADE)",
                    "CREATE TABLE IF NOT EXISTS `role_user` (`user_id` BIGINT NOT NULL, \
                     `role_id` BIGINT NOT NULL, PRIMARY KEY (`user_id`, `role_id`), \
                     CONSTRAINT `role_user_user_fk` FOREIGN KEY (`user_id`) \
                     REFERENCES `users` (`id`) ON DELETE CASCADE, \
                     CONSTRAINT `role_user_role_fk` FOREIGN KEY (`role_id`) \
                     REFERENCES `roles` (`id`) ON DELETE CASCADE)",
                    SEED_ROLE,
                    GRANT_ROLE,
                ],
                &[
                    "DROP TABLE IF EXISTS `role_user`",
                    "DROP TABLE IF EXISTS `role_permission`",
                    "DROP TABLE IF EXISTS `permissions`",
                    "DROP TABLE IF EXISTS `roles`",
                ],
            ),
            (
                "20261114000000_account_lifecycle",
                &[
                    "ALTER TABLE `users` ADD COLUMN `disabled_at` DATETIME NULL",
                    DELETE_ORPHAN_SESSIONS,
                    "ALTER TABLE `sessions` ADD CONSTRAINT `sessions_user_fk` \
                     FOREIGN KEY (`user_id`) REFERENCES `users` (`id`) ON DELETE CASCADE",
                ],
                &[
                    "ALTER TABLE `sessions` DROP FOREIGN KEY `sessions_user_fk`",
                    "ALTER TABLE `users` DROP COLUMN `disabled_at`",
                ],
            ),
            (
                "20261115000000_password_reset_tokens",
                &["CREATE TABLE IF NOT EXISTS `password_reset_tokens` (\
                   `email` VARCHAR(255) NOT NULL PRIMARY KEY, `token` VARCHAR(255) NOT NULL, \
                   `created_at` DATETIME(6) NOT NULL)"],
                &["DROP TABLE IF EXISTS `password_reset_tokens`"],
            ),
        ],
    );
}

#[cfg(feature = "postgres")]
#[test]
fn ignite_migrations_render_on_postgres() {
    assert_renders(
        Driver::Postgres,
        &[
            (
                "20261108000000_create_users",
                &[
                    "CREATE TABLE IF NOT EXISTS \"users\" (\"id\" BIGSERIAL PRIMARY KEY, \
                   \"name\" VARCHAR(255) NOT NULL, \"email\" VARCHAR(255) NOT NULL, \
                   \"password_hash\" VARCHAR(255) NOT NULL, \
                   \"created_at\" TIMESTAMP(0) WITHOUT TIME ZONE NOT NULL DEFAULT CURRENT_TIMESTAMP, \
                   CONSTRAINT \"users_email_unique\" UNIQUE (\"email\"))",
                ],
                &["DROP TABLE IF EXISTS \"users\""],
            ),
            (
                "20261108010000_create_sessions",
                &[
                    "CREATE TABLE IF NOT EXISTS \"sessions\" (\
                     \"id\" VARCHAR(64) NOT NULL PRIMARY KEY, \"user_id\" BIGINT NOT NULL, \
                     \"expires_at\" TIMESTAMP(0) WITHOUT TIME ZONE NOT NULL, \
                     \"created_at\" TIMESTAMP(0) WITHOUT TIME ZONE NOT NULL \
                     DEFAULT CURRENT_TIMESTAMP)",
                    "CREATE INDEX IF NOT EXISTS \"sessions_user_index\" ON \"sessions\" (\"user_id\")",
                ],
                &["DROP TABLE IF EXISTS \"sessions\""],
            ),
            (
                "20261108020000_create_login_throttles",
                &["CREATE TABLE IF NOT EXISTS \"login_throttles\" (\
                   \"throttle_key\" VARCHAR(255) NOT NULL PRIMARY KEY, \"hits\" INTEGER NOT NULL, \
                   \"window_start\" TIMESTAMP(0) WITHOUT TIME ZONE NOT NULL)"],
                &["DROP TABLE IF EXISTS \"login_throttles\""],
            ),
            (
                "20261110000000_add_user_theme",
                &["ALTER TABLE \"users\" ADD COLUMN \"theme\" VARCHAR(8) NOT NULL DEFAULT 'light'"],
                &["ALTER TABLE \"users\" DROP COLUMN \"theme\""],
            ),
            (
                "20261111000000_profile",
                &[
                    "ALTER TABLE \"users\" ADD COLUMN \"avatar_mime\" VARCHAR(32) NULL",
                    "ALTER TABLE \"users\" ADD COLUMN \"profile\" TEXT NULL",
                    "CREATE TABLE IF NOT EXISTS \"user_avatars\" (\
                     \"user_id\" BIGINT NOT NULL PRIMARY KEY, \"data\" BYTEA NOT NULL, \
                     CONSTRAINT \"user_avatars_user_fk\" FOREIGN KEY (\"user_id\") \
                     REFERENCES \"users\" (\"id\") ON DELETE CASCADE)",
                ],
                &[
                    "DROP TABLE IF EXISTS \"user_avatars\"",
                    "ALTER TABLE \"users\" DROP COLUMN \"avatar_mime\"",
                    "ALTER TABLE \"users\" DROP COLUMN \"profile\"",
                ],
            ),
            (
                "20261112000000_login_throttles_window_index",
                &["CREATE INDEX \"login_throttles_window_start_index\" \
                   ON \"login_throttles\" (\"window_start\")"],
                &["DROP INDEX \"login_throttles_window_start_index\""],
            ),
            (
                "20261113000000_roles_permissions",
                &[
                    "CREATE TABLE IF NOT EXISTS \"roles\" (\"id\" BIGSERIAL PRIMARY KEY, \
                     \"name\" VARCHAR(255) NOT NULL, \
                     CONSTRAINT \"roles_name_unique\" UNIQUE (\"name\"))",
                    "CREATE TABLE IF NOT EXISTS \"permissions\" (\"id\" BIGSERIAL PRIMARY KEY, \
                     \"name\" VARCHAR(255) NOT NULL, \
                     CONSTRAINT \"permissions_name_unique\" UNIQUE (\"name\"))",
                    "CREATE TABLE IF NOT EXISTS \"role_permission\" (\"role_id\" BIGINT NOT NULL, \
                     \"permission_id\" BIGINT NOT NULL, \
                     PRIMARY KEY (\"role_id\", \"permission_id\"), \
                     CONSTRAINT \"role_permission_role_fk\" FOREIGN KEY (\"role_id\") \
                     REFERENCES \"roles\" (\"id\") ON DELETE CASCADE, \
                     CONSTRAINT \"role_permission_permission_fk\" FOREIGN KEY (\"permission_id\") \
                     REFERENCES \"permissions\" (\"id\") ON DELETE CASCADE)",
                    "CREATE TABLE IF NOT EXISTS \"role_user\" (\"user_id\" BIGINT NOT NULL, \
                     \"role_id\" BIGINT NOT NULL, PRIMARY KEY (\"user_id\", \"role_id\"), \
                     CONSTRAINT \"role_user_user_fk\" FOREIGN KEY (\"user_id\") \
                     REFERENCES \"users\" (\"id\") ON DELETE CASCADE, \
                     CONSTRAINT \"role_user_role_fk\" FOREIGN KEY (\"role_id\") \
                     REFERENCES \"roles\" (\"id\") ON DELETE CASCADE)",
                    SEED_ROLE,
                    GRANT_ROLE,
                ],
                &[
                    "DROP TABLE IF EXISTS \"role_user\"",
                    "DROP TABLE IF EXISTS \"role_permission\"",
                    "DROP TABLE IF EXISTS \"permissions\"",
                    "DROP TABLE IF EXISTS \"roles\"",
                ],
            ),
            (
                "20261114000000_account_lifecycle",
                &[
                    "ALTER TABLE \"users\" ADD COLUMN \"disabled_at\" \
                     TIMESTAMP(0) WITHOUT TIME ZONE NULL",
                    DELETE_ORPHAN_SESSIONS,
                    "ALTER TABLE \"sessions\" ADD CONSTRAINT \"sessions_user_fk\" \
                     FOREIGN KEY (\"user_id\") REFERENCES \"users\" (\"id\") ON DELETE CASCADE",
                ],
                &[
                    "ALTER TABLE \"sessions\" DROP CONSTRAINT \"sessions_user_fk\"",
                    "ALTER TABLE \"users\" DROP COLUMN \"disabled_at\"",
                ],
            ),
            (
                "20261115000000_password_reset_tokens",
                &["CREATE TABLE IF NOT EXISTS \"password_reset_tokens\" (\
                   \"email\" VARCHAR(255) NOT NULL PRIMARY KEY, \"token\" VARCHAR(255) NOT NULL, \
                   \"created_at\" TIMESTAMP(6) WITHOUT TIME ZONE NOT NULL)"],
                &["DROP TABLE IF EXISTS \"password_reset_tokens\""],
            ),
        ],
    );
}

#[cfg(feature = "sqlite")]
#[test]
fn ignite_migrations_render_on_sqlite() {
    assert_renders(
        Driver::Sqlite,
        &[
            (
                "20261108000000_create_users",
                &["CREATE TABLE IF NOT EXISTS \"users\" (\
                   \"id\" INTEGER PRIMARY KEY AUTOINCREMENT, \
                   \"name\" TEXT NOT NULL, \"email\" TEXT NOT NULL, \
                   \"password_hash\" TEXT NOT NULL, \
                   \"created_at\" DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP, \
                   CONSTRAINT \"users_email_unique\" UNIQUE (\"email\"))"],
                &["DROP TABLE IF EXISTS \"users\""],
            ),
            (
                "20261108010000_create_sessions",
                &[
                    "CREATE TABLE IF NOT EXISTS \"sessions\" (\
                     \"id\" TEXT NOT NULL PRIMARY KEY, \"user_id\" INTEGER NOT NULL, \
                     \"expires_at\" DATETIME NOT NULL, \
                     \"created_at\" DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP)",
                    "CREATE INDEX IF NOT EXISTS \"sessions_user_index\" ON \"sessions\" (\"user_id\")",
                ],
                &["DROP TABLE IF EXISTS \"sessions\""],
            ),
            (
                "20261108020000_create_login_throttles",
                &["CREATE TABLE IF NOT EXISTS \"login_throttles\" (\
                   \"throttle_key\" TEXT NOT NULL PRIMARY KEY, \"hits\" INTEGER NOT NULL, \
                   \"window_start\" DATETIME NOT NULL)"],
                &["DROP TABLE IF EXISTS \"login_throttles\""],
            ),
            (
                "20261110000000_add_user_theme",
                &["ALTER TABLE \"users\" ADD COLUMN \"theme\" TEXT NOT NULL DEFAULT 'light'"],
                &["ALTER TABLE \"users\" DROP COLUMN \"theme\""],
            ),
            (
                "20261111000000_profile",
                &[
                    "ALTER TABLE \"users\" ADD COLUMN \"avatar_mime\" TEXT NULL",
                    "ALTER TABLE \"users\" ADD COLUMN \"profile\" TEXT NULL",
                    "CREATE TABLE IF NOT EXISTS \"user_avatars\" (\
                     \"user_id\" INTEGER NOT NULL PRIMARY KEY, \"data\" BLOB NOT NULL, \
                     CONSTRAINT \"user_avatars_user_fk\" FOREIGN KEY (\"user_id\") \
                     REFERENCES \"users\" (\"id\") ON DELETE CASCADE)",
                ],
                &[
                    "DROP TABLE IF EXISTS \"user_avatars\"",
                    "ALTER TABLE \"users\" DROP COLUMN \"avatar_mime\"",
                    "ALTER TABLE \"users\" DROP COLUMN \"profile\"",
                ],
            ),
            (
                "20261112000000_login_throttles_window_index",
                &["CREATE INDEX \"login_throttles_window_start_index\" \
                   ON \"login_throttles\" (\"window_start\")"],
                &["DROP INDEX \"login_throttles_window_start_index\""],
            ),
            (
                "20261113000000_roles_permissions",
                &[
                    "CREATE TABLE IF NOT EXISTS \"roles\" (\
                     \"id\" INTEGER PRIMARY KEY AUTOINCREMENT, \"name\" TEXT NOT NULL, \
                     CONSTRAINT \"roles_name_unique\" UNIQUE (\"name\"))",
                    "CREATE TABLE IF NOT EXISTS \"permissions\" (\
                     \"id\" INTEGER PRIMARY KEY AUTOINCREMENT, \"name\" TEXT NOT NULL, \
                     CONSTRAINT \"permissions_name_unique\" UNIQUE (\"name\"))",
                    "CREATE TABLE IF NOT EXISTS \"role_permission\" (\
                     \"role_id\" INTEGER NOT NULL, \"permission_id\" INTEGER NOT NULL, \
                     PRIMARY KEY (\"role_id\", \"permission_id\"), \
                     CONSTRAINT \"role_permission_role_fk\" FOREIGN KEY (\"role_id\") \
                     REFERENCES \"roles\" (\"id\") ON DELETE CASCADE, \
                     CONSTRAINT \"role_permission_permission_fk\" FOREIGN KEY (\"permission_id\") \
                     REFERENCES \"permissions\" (\"id\") ON DELETE CASCADE)",
                    "CREATE TABLE IF NOT EXISTS \"role_user\" (\
                     \"user_id\" INTEGER NOT NULL, \"role_id\" INTEGER NOT NULL, \
                     PRIMARY KEY (\"user_id\", \"role_id\"), \
                     CONSTRAINT \"role_user_user_fk\" FOREIGN KEY (\"user_id\") \
                     REFERENCES \"users\" (\"id\") ON DELETE CASCADE, \
                     CONSTRAINT \"role_user_role_fk\" FOREIGN KEY (\"role_id\") \
                     REFERENCES \"roles\" (\"id\") ON DELETE CASCADE)",
                    SEED_ROLE,
                    GRANT_ROLE,
                ],
                &[
                    "DROP TABLE IF EXISTS \"role_user\"",
                    "DROP TABLE IF EXISTS \"role_permission\"",
                    "DROP TABLE IF EXISTS \"permissions\"",
                    "DROP TABLE IF EXISTS \"roles\"",
                ],
            ),
            (
                // SQLite cannot add a constraint to an existing table: the session key is
                // skipped, and so is its drop (Ruling R4).
                "20261114000000_account_lifecycle",
                &[
                    "ALTER TABLE \"users\" ADD COLUMN \"disabled_at\" DATETIME NULL",
                    DELETE_ORPHAN_SESSIONS,
                ],
                &["ALTER TABLE \"users\" DROP COLUMN \"disabled_at\""],
            ),
            (
                "20261115000000_password_reset_tokens",
                &["CREATE TABLE IF NOT EXISTS \"password_reset_tokens\" (\
                   \"email\" TEXT NOT NULL PRIMARY KEY, \"token\" TEXT NOT NULL, \
                   \"created_at\" DATETIME NOT NULL)"],
                &["DROP TABLE IF EXISTS \"password_reset_tokens\""],
            ),
        ],
    );
}

// ---------------------------------------------------------------------------------------------
// On a real database
// ---------------------------------------------------------------------------------------------

/// Drops every table the migrations create, and the migrations table, so a run left over from
/// another test or a failed run cannot get in the way.
async fn clean(db: &Database) {
    let mut tables = TABLES.to_vec();
    tables.push("__diesel_schema_migrations");
    common::drop_tables(db, &tables).await;
}

async fn table_exists(db: &Database, table: &str) -> bool {
    let count: i64 = raw::fetch_scalar(
        db,
        common::table_count_sql(db.driver()),
        &[Value::from(table)],
    )
    .await
    .expect("reads the catalog");
    count > 0
}

async fn count(db: &Database, sql: &str, binds: &[Value]) -> i64 {
    raw::fetch_scalar(db, sql, binds)
        .await
        .unwrap_or_else(|e| panic!("`{sql}` failed: {e}"))
}

async fn columns(db: &Database, table: &str) -> Vec<String> {
    let rows: Vec<(String,)> =
        raw::fetch_all(db, &common::columns_sql(db.driver()), &[Value::from(table)])
            .await
            .expect("reads the columns");
    rows.into_iter().map(|(c,)| c).collect()
}

async fn add_user(db: &Database, name: &str, email: &str) -> i64 {
    raw::execute(
        db,
        "INSERT INTO users (name, email, password_hash) VALUES (?, ?, ?)",
        &[name.into(), email.into(), "hash".into()],
    )
    .await
    .expect("inserts a user");
    count(db, "SELECT id FROM users WHERE email = ?", &[email.into()]).await
}

async fn add_session(db: &Database, id: &str, user_id: i64) {
    raw::execute(
        db,
        "INSERT INTO sessions (id, user_id, expires_at) VALUES (?, ?, '2026-12-01 00:00:00')",
        &[id.into(), user_id.into()],
    )
    .await
    .expect("inserts a session");
}

async fn sessions_of(db: &Database, user_id: i64) -> i64 {
    count(
        db,
        "SELECT COUNT(*) FROM sessions WHERE user_id = ?",
        &[user_id.into()],
    )
    .await
}

#[tokio::test]
async fn ignite_migrations_run_and_roll_back() {
    let Some(url) = common::url() else {
        eprintln!("skipped: set ORBIT_DATABASE_URL to run the Ignite migrations test");
        return;
    };
    let db = Database::connect(&url).await.expect("connects");
    let driver = db.driver();
    clean(&db).await;

    // Stage one: the migrations before roles, as an install that predates them.
    let first: Vec<Migration> = steps().into_iter().take(6).map(Step::migration).collect();
    let applied = Migrator::new(first)
        .expect("valid")
        .run(&db)
        .await
        .expect("runs the first six");
    assert_eq!(applied.len(), 6);

    let ann = add_user(&db, "Ann", "ann@example.com").await;
    add_session(&db, "session-ann", ann).await;
    // A session whose user was deleted before the key existed.
    add_session(&db, "session-gone", ann + 1000).await;
    assert_eq!(
        count(&db, "SELECT COUNT(*) FROM users WHERE theme = 'light'", &[]).await,
        1,
        "a new user starts on the light theme"
    );

    // Stage two: the rest, as an upgrade.
    let migrator =
        Migrator::new(steps().into_iter().map(Step::migration).collect()).expect("valid");
    let applied = migrator.run(&db).await.expect("runs the rest");
    assert_eq!(
        applied,
        vec![
            "20261113000000_roles_permissions",
            "20261114000000_account_lifecycle",
            "20261115000000_password_reset_tokens",
        ]
    );
    let status = migrator.status(&db).await.expect("status");
    for (i, s) in status.iter().enumerate() {
        let batch = if i < 6 { 1 } else { 2 };
        assert_eq!(s.state, State::Applied { batch }, "{}", s.name);
    }
    for table in TABLES {
        assert!(table_exists(&db, table).await, "{table} exists");
    }

    // The role is seeded, and the user who was there before roles holds it.
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM roles WHERE name = 'super-admin'",
            &[]
        )
        .await,
        1
    );
    let super_admins = "SELECT COUNT(*) FROM role_user JOIN roles ON roles.id = role_user.role_id \
         WHERE roles.name = 'super-admin' AND role_user.user_id = ?";
    assert_eq!(count(&db, super_admins, &[ann.into()]).await, 1);
    // A user created after the upgrade gets no role.
    let bob = add_user(&db, "Bob", "bob@example.com").await;
    assert_eq!(count(&db, super_admins, &[bob.into()]).await, 0);

    // The email is unique.
    let duplicate = raw::execute(
        &db,
        "INSERT INTO users (name, email, password_hash) VALUES ('Ann 2', 'ann@example.com', 'x')",
        &[],
    )
    .await;
    assert!(duplicate.is_err(), "a second user with the same email");

    // The orphaned session went before the key was added; Ann's stayed.
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM sessions WHERE id = 'session-gone'",
            &[]
        )
        .await,
        0
    );
    assert_eq!(sessions_of(&db, ann).await, 1);
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM users WHERE disabled_at IS NULL",
            &[]
        )
        .await,
        2
    );

    // Avatar bytes come back unchanged. 100 000 bytes is more than a MySQL BLOB (64 KB) holds,
    // so on MySQL this also proves the MEDIUMBLOB statement ran.
    if common::on_mysql(driver) {
        let ty: String = raw::fetch_scalar(
            &db,
            "SELECT CAST(data_type AS CHAR) FROM information_schema.columns \
             WHERE table_schema = DATABASE() AND table_name = 'user_avatars' \
             AND column_name = 'data'",
            &[],
        )
        .await
        .expect("reads the column type");
        assert_eq!(ty.to_lowercase(), "mediumblob");
    }
    let avatar: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
    raw::execute(
        &db,
        "INSERT INTO user_avatars (user_id, data) VALUES (?, ?)",
        &[ann.into(), Value::Bytes(avatar.clone())],
    )
    .await
    .expect("stores an avatar");
    let stored: Vec<u8> = raw::fetch_scalar(
        &db,
        "SELECT data FROM user_avatars WHERE user_id = ?",
        &[ann.into()],
    )
    .await
    .expect("reads the avatar");
    assert_eq!(stored, avatar);

    // A reset token keeps microseconds.
    raw::execute(
        &db,
        "INSERT INTO password_reset_tokens (email, token, created_at) \
         VALUES ('ann@example.com', 'hashed', '2026-11-14 10:20:30.123456')",
        &[],
    )
    .await
    .expect("stores a reset token");
    let created: String = raw::fetch_scalar(
        &db,
        &format!(
            "SELECT CAST(created_at AS {}) FROM password_reset_tokens",
            common::text_type(driver)
        ),
        &[],
    )
    .await
    .expect("reads the token time");
    assert_eq!(created, "2026-11-14 10:20:30.123456");

    // Deleting Ann removes her avatar and roles on every database. Her sessions go with her on
    // MySQL and PostgreSQL; SQLite has no session key (Ruling R4), so they stay there.
    raw::execute(&db, "DELETE FROM users WHERE id = ?", &[ann.into()])
        .await
        .expect("deletes Ann");
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM user_avatars WHERE user_id = ?",
            &[ann.into()]
        )
        .await,
        0
    );
    assert_eq!(
        count(
            &db,
            "SELECT COUNT(*) FROM role_user WHERE user_id = ?",
            &[ann.into()]
        )
        .await,
        0
    );
    let left = if common::on_sqlite(driver) { 1 } else { 0 };
    assert_eq!(sessions_of(&db, ann).await, left);

    // Rolling back the upgrade leaves the install as it was after stage one.
    let reverted = migrator.rollback(&db).await.expect("rolls back batch 2");
    assert_eq!(
        reverted,
        vec![
            "20261115000000_password_reset_tokens",
            "20261114000000_account_lifecycle",
            "20261113000000_roles_permissions",
        ]
    );
    for table in [
        "password_reset_tokens",
        "roles",
        "permissions",
        "role_permission",
        "role_user",
    ] {
        assert!(!table_exists(&db, table).await, "{table} is gone");
    }
    assert_eq!(
        columns(&db, "users").await,
        vec![
            "id",
            "name",
            "email",
            "password_hash",
            "created_at",
            "theme",
            "avatar_mime",
            "profile"
        ]
    );

    let reverted = migrator.rollback(&db).await.expect("rolls back batch 1");
    assert_eq!(reverted.len(), 6);
    assert_eq!(reverted[0], "20261112000000_login_throttles_window_index");
    assert_eq!(reverted[5], "20261108000000_create_users");
    for table in TABLES {
        assert!(!table_exists(&db, table).await, "{table} is gone");
    }
    let status = migrator.status(&db).await.expect("status");
    assert!(status.iter().all(|s| s.state == State::Pending));

    clean(&db).await;
}
