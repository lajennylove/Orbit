//! Helpers shared by the database tests. Each test file includes this with `mod common;`.
//!
//! The tests run once per database. `ORBIT_DATABASE_URL` names the database: `mysql://...`,
//! `postgres://...` or `sqlite:...`. Tests skip with a notice when it is not set. The SQL in the
//! tests is written in MySQL syntax, and `sql` turns it into the active database's syntax.

#![allow(dead_code)]

use orbit_orm::{Database, Driver};

/// The URL of the database under test, if one is set.
pub fn url() -> Option<String> {
    std::env::var("ORBIT_DATABASE_URL").ok()
}

/// Whether `driver` is MySQL or MariaDB. False when the mysql feature is not built.
pub fn on_mysql(driver: Driver) -> bool {
    #[cfg(feature = "mysql")]
    {
        driver == Driver::MySql
    }
    #[cfg(not(feature = "mysql"))]
    {
        let _ = driver;
        false
    }
}

/// Whether `driver` is PostgreSQL. False when the postgres feature is not built.
pub fn on_postgres(driver: Driver) -> bool {
    #[cfg(feature = "postgres")]
    {
        driver == Driver::Postgres
    }
    #[cfg(not(feature = "postgres"))]
    {
        let _ = driver;
        false
    }
}

/// Whether `driver` is SQLite. False when the sqlite feature is not built.
pub fn on_sqlite(driver: Driver) -> bool {
    #[cfg(feature = "sqlite")]
    {
        driver == Driver::Sqlite
    }
    #[cfg(not(feature = "sqlite"))]
    {
        let _ = driver;
        false
    }
}

/// The type name for a text cast (`CAST(x AS ...)`), so a value reads back as a string.
pub fn text_type(driver: Driver) -> &'static str {
    match driver {
        #[cfg(feature = "mysql")]
        Driver::MySql => "CHAR",
        #[cfg(any(feature = "postgres", feature = "sqlite"))]
        _ => "TEXT",
    }
}

/// The type name for a 64-bit integer cast.
pub fn bigint_type(driver: Driver) -> &'static str {
    match driver {
        #[cfg(feature = "mysql")]
        Driver::MySql => "SIGNED",
        #[cfg(feature = "postgres")]
        Driver::Postgres => "BIGINT",
        #[cfg(feature = "sqlite")]
        Driver::Sqlite => "INTEGER",
    }
}

/// Rewrites MySQL `UNIQUE KEY `name` (cols)` as `CONSTRAINT "name" UNIQUE (cols)`.
fn unique_keys(input: &str) -> String {
    let mut out = String::new();
    let mut rest = input;
    while let Some(at) = rest.find("UNIQUE KEY `") {
        out.push_str(&rest[..at]);
        let after = &rest[at + "UNIQUE KEY `".len()..];
        let end = after
            .find('`')
            .expect("a closing backtick after UNIQUE KEY");
        let name = &after[..end];
        out.push_str(&format!("CONSTRAINT \"{name}\" UNIQUE"));
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

/// Turns MySQL-syntax SQL from a test into the syntax of `driver`. MySQL is returned unchanged.
pub fn sql(driver: Driver, mysql: &str) -> String {
    let unique = unique_keys(mysql);
    match driver {
        #[cfg(feature = "mysql")]
        Driver::MySql => {
            let _ = &unique;
            mysql.to_owned()
        }
        #[cfg(feature = "postgres")]
        Driver::Postgres => unique
            .replace('`', "\"")
            .replace(
                "BIGINT NOT NULL AUTO_INCREMENT PRIMARY KEY",
                "BIGSERIAL PRIMARY KEY",
            )
            .replace("BIGINT AUTO_INCREMENT PRIMARY KEY", "BIGSERIAL PRIMARY KEY")
            .replace("DATETIME(6)", "TIMESTAMP(6)")
            .replace("TINYINT(1)", "BOOLEAN")
            .replace("MEDIUMBLOB", "BYTEA")
            .replace(" ENGINE=InnoDB", ""),
        #[cfg(feature = "sqlite")]
        Driver::Sqlite => unique
            .replace('`', "\"")
            .replace(
                "BIGINT NOT NULL AUTO_INCREMENT PRIMARY KEY",
                "INTEGER PRIMARY KEY AUTOINCREMENT",
            )
            .replace(
                "BIGINT AUTO_INCREMENT PRIMARY KEY",
                "INTEGER PRIMARY KEY AUTOINCREMENT",
            )
            .replace("DATETIME(6)", "TEXT")
            .replace("TINYINT(1)", "INTEGER")
            .replace("MEDIUMBLOB", "BLOB")
            .replace(" ENGINE=InnoDB", ""),
    }
}

/// Splits a script into statements at each `;` outside a quoted string.
pub fn statements(script: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    for c in script.chars() {
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                }
                current.push(c);
            }
            None => match c {
                '\'' | '"' => {
                    quote = Some(c);
                    current.push(c);
                }
                ';' => {
                    let s = current.trim().to_owned();
                    if !s.is_empty() {
                        out.push(s);
                    }
                    current.clear();
                }
                _ => current.push(c),
            },
        }
    }
    let s = current.trim().to_owned();
    if !s.is_empty() {
        out.push(s);
    }
    out
}

/// Runs one or more statements written in MySQL syntax, each translated with `sql`. Statements are
/// separated by `;`, and no bound values are used.
pub async fn exec(db: &Database, mysql: &str) {
    for statement in statements(mysql) {
        let statement = sql(db.driver(), &statement);
        orbit_orm::raw::execute(db, &statement, &[])
            .await
            .unwrap_or_else(|e| panic!("`{statement}` failed: {e}"));
    }
}

/// Drops each table in `tables`, in the order given. Put referencing tables first.
pub async fn drop_tables(db: &Database, tables: &[&str]) {
    for table in tables {
        exec(db, &format!("DROP TABLE IF EXISTS `{table}`")).await;
    }
}

/// The SQL that lists the columns of a table, in table order. It takes the table name as `?`.
pub fn columns_sql(driver: Driver) -> String {
    match driver {
        #[cfg(feature = "mysql")]
        Driver::MySql => "SELECT CAST(column_name AS CHAR) FROM information_schema.columns \
             WHERE table_schema = DATABASE() AND table_name = ? ORDER BY ordinal_position"
            .to_owned(),
        #[cfg(feature = "postgres")]
        Driver::Postgres => "SELECT CAST(column_name AS TEXT) FROM information_schema.columns \
             WHERE table_schema = current_schema() AND table_name = ? ORDER BY ordinal_position"
            .to_owned(),
        #[cfg(feature = "sqlite")]
        Driver::Sqlite => "SELECT name FROM pragma_table_info(?) ORDER BY cid".to_owned(),
    }
}

/// The SQL that counts the tables with a given name, as `COUNT(*)`. It takes the name as `?`.
pub fn table_count_sql(driver: Driver) -> &'static str {
    match driver {
        #[cfg(feature = "mysql")]
        Driver::MySql => {
            "SELECT COUNT(*) FROM information_schema.tables \
             WHERE table_schema = DATABASE() AND table_name = ?"
        }
        #[cfg(feature = "postgres")]
        Driver::Postgres => {
            "SELECT COUNT(*) FROM information_schema.tables \
             WHERE table_schema = current_schema() AND table_name = ?"
        }
        #[cfg(feature = "sqlite")]
        Driver::Sqlite => "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?",
    }
}
