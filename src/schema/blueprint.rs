use super::Dialect;
use super::column::{Column, ColumnType};
use crate::query::validate;
use crate::{Driver, Error};

/// What a foreign key does to this table's rows when the referenced row is deleted or updated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Cascade,
    Restrict,
    SetNull,
    SetDefault,
    NoAction,
}

impl Action {
    fn sql(self) -> &'static str {
        match self {
            Self::Cascade => "CASCADE",
            Self::Restrict => "RESTRICT",
            Self::SetNull => "SET NULL",
            Self::SetDefault => "SET DEFAULT",
            Self::NoAction => "NO ACTION",
        }
    }
}

/// A foreign key under construction. Chain `references`, `on_delete`, `on_update` and `name`.
#[derive(Debug, Clone)]
pub struct ForeignKey {
    column: String,
    name: Option<String>,
    target: Option<(String, String)>,
    on_delete: Option<Action>,
    on_update: Option<Action>,
}

impl ForeignKey {
    /// The table and column that this column points at.
    pub fn references(&mut self, table: &str, column: &str) -> &mut Self {
        self.target = Some((table.to_owned(), column.to_owned()));
        self
    }

    /// What happens when the referenced row is deleted.
    pub fn on_delete(&mut self, action: Action) -> &mut Self {
        self.on_delete = Some(action);
        self
    }

    /// What happens when the referenced key is updated.
    pub fn on_update(&mut self, action: Action) -> &mut Self {
        self.on_update = Some(action);
        self
    }

    /// Names the constraint, as the second argument of Laravel's `$table->foreign('col', 'name')`.
    /// Without it the name is `{table}_{column}_foreign`.
    pub fn name(&mut self, name: &str) -> &mut Self {
        self.name = Some(name.to_owned());
        self
    }
}

/// A plain or unique index under construction. Chain `name` to choose its name.
#[derive(Debug, Clone)]
pub struct Index {
    columns: Vec<String>,
    unique: bool,
    name: Option<String>,
}

impl Index {
    /// Names the index, as the second argument of Laravel's `$table->index(['a'], 'name')`.
    /// Without it the name is `{table}_{columns}_index`, or `{table}_{columns}_unique` for a
    /// unique index.
    pub fn name(&mut self, name: &str) -> &mut Self {
        self.name = Some(name.to_owned());
        self
    }

    /// The index's name, checked: the custom one, or the default for `table`.
    fn name_for(&self, table: &str) -> Result<String, Error> {
        match &self.name {
            Some(name) => {
                validate(name)?;
                Ok(name.clone())
            }
            None => Ok(name_of(
                table,
                &self.columns,
                if self.unique { "unique" } else { "index" },
            )),
        }
    }
}

/// Everything in a blueprint that is not a column, in call order. `schema::create` renders the
/// indexes, primary keys and foreign keys inside `CREATE TABLE`; `schema::table` renders each
/// command as its own statement after the added and changed columns, as Laravel's Blueprint
/// orders them.
#[derive(Debug, Clone)]
enum Command {
    Index(Index),
    Primary { columns: Vec<String> },
    Foreign(ForeignKey),
    RenameColumn { from: String, to: String },
    DropColumn { name: String },
    DropIndex { name: String },
    DropForeign { name: String },
}

/// The columns and commands (indexes, keys, renames, drops) of a table, in the order they were
/// added.
#[derive(Debug, Clone, Default)]
pub struct Blueprint {
    columns: Vec<Column>,
    commands: Vec<Command>,
}

impl Blueprint {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    fn add(&mut self, name: &str, ty: ColumnType) -> &mut Column {
        self.columns.push(Column::new(name, ty));
        self.columns.last_mut().expect("just pushed")
    }

    /// An auto-incrementing 64-bit primary key named `id`. In [`schema::table`](super::table)
    /// SQLite cannot add it, as SQLite cannot add a primary key column to an existing table.
    pub fn id(&mut self) -> &mut Column {
        self.add("id", ColumnType::Id)
    }

    /// A `VARCHAR(255)`; `TEXT` on SQLite. Change the length with `.length(n)`.
    pub fn string(&mut self, name: &str) -> &mut Column {
        self.add(name, ColumnType::String(255))
    }

    /// A `TEXT` column.
    pub fn text(&mut self, name: &str) -> &mut Column {
        self.add(name, ColumnType::Text)
    }

    /// An `INTEGER` column.
    pub fn integer(&mut self, name: &str) -> &mut Column {
        self.add(name, ColumnType::Integer)
    }

    /// A `BIGINT` column; `INTEGER` on SQLite, whose integers are 64-bit already.
    pub fn big_integer(&mut self, name: &str) -> &mut Column {
        self.add(name, ColumnType::BigInteger)
    }

    /// A boolean: `BOOLEAN` on PostgreSQL, `TINYINT(1)` elsewhere.
    pub fn boolean(&mut self, name: &str) -> &mut Column {
        self.add(name, ColumnType::Boolean)
    }

    /// A date and time without a time zone: `DATETIME`, or `TIMESTAMP(0)` on PostgreSQL.
    pub fn date_time(&mut self, name: &str) -> &mut Column {
        self.add(name, ColumnType::DateTime)
    }

    /// Nullable `created_at` and `updated_at` columns.
    pub fn timestamps(&mut self) {
        self.date_time("created_at").nullable();
        self.date_time("updated_at").nullable();
    }

    /// A unique constraint over `columns`, named `{table}_{columns}_unique` unless `.name()`
    /// gives another name.
    pub fn unique<I, S>(&mut self, columns: I) -> &mut Index
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.add_index(columns, true)
    }

    /// A plain index over `columns`, named `{table}_{columns}_index` unless `.name()` gives
    /// another name.
    pub fn index<I, S>(&mut self, columns: I) -> &mut Index
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.add_index(columns, false)
    }

    fn add_index<I, S>(&mut self, columns: I, unique: bool) -> &mut Index
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.commands.push(Command::Index(Index {
            columns: collect(columns),
            unique,
            name: None,
        }));
        match self.commands.last_mut() {
            Some(Command::Index(index)) => index,
            _ => unreachable!("just pushed an index"),
        }
    }

    /// A primary key over `columns`, as Laravel's `$table->primary(['a', 'b'])`: the table
    /// constraint `PRIMARY KEY (a, b)`. For a key of one column, `.primary()` on the column does
    /// the same. In [`schema::table`](super::table) SQLite cannot add it: it is
    /// [`Error::Unsupported`] there.
    pub fn primary<I, S>(&mut self, columns: I)
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.commands.push(Command::Primary {
            columns: collect(columns),
        });
    }

    /// A foreign key on `column`, named `{table}_{column}_foreign` unless `.name()` gives another
    /// name. In [`schema::table`](super::table) SQLite renders nothing, as SQLite cannot add a
    /// constraint to an existing table.
    pub fn foreign(&mut self, column: &str) -> &mut ForeignKey {
        self.commands.push(Command::Foreign(ForeignKey {
            column: column.to_owned(),
            name: None,
            target: None,
            on_delete: None,
            on_update: None,
        }));
        match self.commands.last_mut() {
            Some(Command::Foreign(fk)) => fk,
            _ => unreachable!("just pushed a foreign key"),
        }
    }

    /// Renames the column `from` to `to`: `ALTER TABLE ... RENAME COLUMN`. Only in
    /// [`schema::table`](super::table).
    pub fn rename_column(&mut self, from: &str, to: &str) {
        self.commands.push(Command::RenameColumn {
            from: from.to_owned(),
            to: to.to_owned(),
        });
    }

    /// Drops the column `name`: `ALTER TABLE ... DROP COLUMN`. Only in
    /// [`schema::table`](super::table). SQLite needs version 3.35 or later.
    pub fn drop_column(&mut self, name: &str) {
        self.commands.push(Command::DropColumn {
            name: name.to_owned(),
        });
    }

    /// Drops the index `name`, as Laravel's `dropIndex('name')`; on MySQL and SQLite it drops a
    /// unique index too. Only in [`schema::table`](super::table).
    pub fn drop_index(&mut self, name: &str) {
        self.commands.push(Command::DropIndex {
            name: name.to_owned(),
        });
    }

    /// Drops the foreign key `name`, as Laravel's `dropForeign('name')`. Only in
    /// [`schema::table`](super::table). SQLite renders nothing, as it cannot drop a constraint
    /// from an existing table and `foreign` added none there.
    pub fn drop_foreign(&mut self, name: &str) {
        self.commands.push(Command::DropForeign {
            name: name.to_owned(),
        });
    }

    /// Renders `CREATE TABLE`, and the separate `CREATE INDEX` statements of PostgreSQL and
    /// SQLite. With `if_not_exists`, both take `IF NOT EXISTS`.
    pub(crate) fn render_create(
        &self,
        driver: Driver,
        dialect: Dialect,
        table: &str,
        if_not_exists: bool,
    ) -> Result<Vec<String>, Error> {
        validate(table)?;
        let unsupported = |operation| Error::Unsupported {
            operation,
            driver: driver.name(),
        };
        let q = |name: &str| driver.quote(name);
        let exists = if if_not_exists { "IF NOT EXISTS " } else { "" };

        let mut parts = Vec::new();
        for column in &self.columns {
            if column.change {
                return Err(unsupported("changing a column in create"));
            }
            parts.push(column.render(driver, dialect)?);
        }
        let mut after = Vec::new();
        for command in &self.commands {
            match command {
                Command::Index(index) => {
                    let columns = column_list(driver, &index.columns)?;
                    let name = index.name_for(table)?;
                    if index.unique {
                        parts.push(format!("CONSTRAINT {} UNIQUE ({columns})", q(&name)));
                    } else {
                        match dialect {
                            Dialect::MySql => parts.push(format!("INDEX {} ({columns})", q(&name))),
                            _ => after.push(format!(
                                "CREATE INDEX {exists}{} ON {} ({columns})",
                                q(&name),
                                q(table)
                            )),
                        }
                    }
                }
                Command::Primary { columns } => {
                    parts.push(format!("PRIMARY KEY ({})", column_list(driver, columns)?));
                }
                Command::Foreign(_) => {}
                Command::RenameColumn { .. } => {
                    return Err(unsupported("renaming a column in create"));
                }
                Command::DropColumn { .. } => {
                    return Err(unsupported("dropping a column in create"));
                }
                Command::DropIndex { .. } => {
                    return Err(unsupported("dropping an index in create"));
                }
                Command::DropForeign { .. } => {
                    return Err(unsupported("dropping a foreign key in create"));
                }
            }
        }
        // Foreign keys follow the indexes inside CREATE TABLE.
        for command in &self.commands {
            if let Command::Foreign(fk) = command {
                parts.push(fk.render(driver, table)?);
            }
        }
        let mut out = vec![format!(
            "CREATE TABLE {exists}{} ({})",
            q(table),
            parts.join(", ")
        )];
        out.extend(after);
        Ok(out)
    }

    /// Renders the changes to an existing table, one statement per operation: the added columns,
    /// then the changed columns, then the other commands in call order. A foreign key and
    /// `drop_foreign` render nothing on SQLite.
    pub(crate) fn render_alter(
        &self,
        driver: Driver,
        dialect: Dialect,
        table: &str,
    ) -> Result<Vec<String>, Error> {
        validate(table)?;
        let unsupported = |operation| Error::Unsupported {
            operation,
            driver: driver.name(),
        };
        let q = |name: &str| driver.quote(name);
        let alter = format!("ALTER TABLE {}", q(table));

        let mut out = Vec::new();
        for column in self.columns.iter().filter(|c| !c.change) {
            if (column.ty == ColumnType::Id || column.primary) && dialect == Dialect::Sqlite {
                return Err(unsupported("adding a primary key column"));
            }
            out.push(format!(
                "{alter} ADD COLUMN {}",
                column.render(driver, dialect)?
            ));
        }
        for column in self.columns.iter().filter(|c| c.change) {
            if dialect == Dialect::Sqlite {
                return Err(unsupported("changing a column"));
            }
            if column.ty == ColumnType::Id {
                return Err(unsupported("changing an id column"));
            }
            if column.primary {
                return Err(unsupported("changing a column into a primary key"));
            }
            out.push(match dialect {
                Dialect::Postgres => format!("{alter} {}", column.render_change_postgres(driver)?),
                _ => format!("{alter} MODIFY COLUMN {}", column.render(driver, dialect)?),
            });
        }
        for command in &self.commands {
            match command {
                Command::Index(index) => {
                    let columns = column_list(driver, &index.columns)?;
                    let name = q(&index.name_for(table)?);
                    out.push(match (dialect, index.unique) {
                        (Dialect::MySql, false) => format!("{alter} ADD INDEX {name} ({columns})"),
                        (Dialect::MySql, true) => format!("{alter} ADD UNIQUE {name} ({columns})"),
                        (Dialect::Postgres, true) => {
                            format!("{alter} ADD CONSTRAINT {name} UNIQUE ({columns})")
                        }
                        (Dialect::Sqlite, true) => {
                            format!("CREATE UNIQUE INDEX {name} ON {} ({columns})", q(table))
                        }
                        (_, false) => format!("CREATE INDEX {name} ON {} ({columns})", q(table)),
                    });
                }
                Command::Primary { columns } => {
                    let columns = column_list(driver, columns)?;
                    if dialect == Dialect::Sqlite {
                        return Err(unsupported("adding a primary key"));
                    }
                    out.push(format!("{alter} ADD PRIMARY KEY ({columns})"));
                }
                Command::Foreign(fk) => {
                    // Rendered on SQLite too, so a missing target or a bad name is still an error.
                    let constraint = fk.render(driver, table)?;
                    if dialect != Dialect::Sqlite {
                        out.push(format!("{alter} ADD {constraint}"));
                    }
                }
                Command::RenameColumn { from, to } => {
                    validate(from)?;
                    validate(to)?;
                    out.push(format!("{alter} RENAME COLUMN {} TO {}", q(from), q(to)));
                }
                Command::DropColumn { name } => {
                    validate(name)?;
                    out.push(format!("{alter} DROP COLUMN {}", q(name)));
                }
                Command::DropIndex { name } => {
                    validate(name)?;
                    out.push(match dialect {
                        Dialect::MySql => format!("{alter} DROP INDEX {}", q(name)),
                        _ => format!("DROP INDEX {}", q(name)),
                    });
                }
                Command::DropForeign { name } => {
                    validate(name)?;
                    match dialect {
                        Dialect::MySql => out.push(format!("{alter} DROP FOREIGN KEY {}", q(name))),
                        Dialect::Postgres => {
                            out.push(format!("{alter} DROP CONSTRAINT {}", q(name)))
                        }
                        Dialect::Sqlite => {}
                    }
                }
            }
        }
        Ok(out)
    }
}

impl ForeignKey {
    /// The `CONSTRAINT ... FOREIGN KEY ... REFERENCES ...` clause, with its custom name or the
    /// default one for `table`.
    fn render(&self, driver: Driver, table: &str) -> Result<String, Error> {
        let q = |name: &str| driver.quote(name);
        validate(&self.column)?;
        let Some((ref_table, ref_column)) = &self.target else {
            return Err(Error::Unsupported {
                operation: "a foreign key without references()",
                driver: driver.name(),
            });
        };
        validate(ref_table)?;
        validate(ref_column)?;
        let name = match &self.name {
            Some(name) => {
                validate(name)?;
                name.clone()
            }
            None => name_of(table, std::slice::from_ref(&self.column), "foreign"),
        };
        let mut sql = format!(
            "CONSTRAINT {} FOREIGN KEY ({}) REFERENCES {} ({})",
            q(&name),
            q(&self.column),
            q(ref_table),
            q(ref_column)
        );
        if let Some(a) = self.on_delete {
            sql.push_str(&format!(" ON DELETE {}", a.sql()));
        }
        if let Some(a) = self.on_update {
            sql.push_str(&format!(" ON UPDATE {}", a.sql()));
        }
        Ok(sql)
    }
}

/// The default name of an index or constraint: `{table}_{columns}_{suffix}`, as Laravel names it.
fn name_of(table: &str, columns: &[String], suffix: &str) -> String {
    format!("{table}_{}_{suffix}", columns.join("_"))
}

/// The quoted, comma-separated column list of an index, each name checked.
fn column_list(driver: Driver, columns: &[String]) -> Result<String, Error> {
    for c in columns {
        validate(c)?;
    }
    Ok(columns
        .iter()
        .map(|c| driver.quote(c))
        .collect::<Vec<_>>()
        .join(", "))
}

fn collect<I, S>(columns: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    columns.into_iter().map(|c| c.as_ref().to_owned()).collect()
}
