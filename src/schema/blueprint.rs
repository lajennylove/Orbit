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

/// A foreign key under construction. Chain `references`, `on_delete` and `on_update`.
#[derive(Debug, Clone)]
pub struct ForeignKey {
    column: String,
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
}

#[derive(Debug, Clone)]
struct Index {
    columns: Vec<String>,
    unique: bool,
}

/// The columns, indexes and foreign keys of a table, in the order they were added.
#[derive(Debug, Clone, Default)]
pub struct Blueprint {
    columns: Vec<Column>,
    indexes: Vec<Index>,
    foreigns: Vec<ForeignKey>,
}

impl Blueprint {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    fn add(&mut self, name: &str, ty: ColumnType) -> &mut Column {
        self.columns.push(Column::new(name, ty));
        self.columns.last_mut().expect("just pushed")
    }

    /// An auto-incrementing 64-bit primary key named `id`.
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

    /// A unique constraint over `columns`, named `{table}_{columns}_unique`.
    pub fn unique<I, S>(&mut self, columns: I)
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.indexes.push(Index {
            columns: collect(columns),
            unique: true,
        });
    }

    /// A plain index over `columns`, named `{table}_{columns}_index`.
    pub fn index<I, S>(&mut self, columns: I)
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.indexes.push(Index {
            columns: collect(columns),
            unique: false,
        });
    }

    /// A foreign key on `column`, named `{table}_{column}_foreign`.
    pub fn foreign(&mut self, column: &str) -> &mut ForeignKey {
        self.foreigns.push(ForeignKey {
            column: column.to_owned(),
            target: None,
            on_delete: None,
            on_update: None,
        });
        self.foreigns.last_mut().expect("just pushed")
    }

    pub(crate) fn render(
        &self,
        driver: Driver,
        dialect: Dialect,
        table: &str,
    ) -> Result<Vec<String>, Error> {
        validate(table)?;
        let q = |name: &str| driver.quote(name);
        let list = |columns: &[String]| -> Result<String, Error> {
            for c in columns {
                validate(c)?;
            }
            Ok(columns.iter().map(|c| q(c)).collect::<Vec<_>>().join(", "))
        };
        let name_of =
            |columns: &[String], suffix: &str| format!("{table}_{}_{suffix}", columns.join("_"));

        let mut parts = Vec::new();
        for column in &self.columns {
            parts.push(column.render(driver, dialect)?);
        }
        let mut after = Vec::new();
        for index in &self.indexes {
            let columns = list(&index.columns)?;
            if index.unique {
                let name = name_of(&index.columns, "unique");
                parts.push(format!("CONSTRAINT {} UNIQUE ({columns})", q(&name)));
            } else {
                let name = name_of(&index.columns, "index");
                match dialect {
                    Dialect::MySql => parts.push(format!("INDEX {} ({columns})", q(&name))),
                    _ => after.push(format!(
                        "CREATE INDEX {} ON {} ({columns})",
                        q(&name),
                        q(table)
                    )),
                }
            }
        }
        for fk in &self.foreigns {
            validate(&fk.column)?;
            let Some((ref_table, ref_column)) = &fk.target else {
                return Err(Error::Unsupported {
                    operation: "a foreign key without references()",
                    driver: driver.name(),
                });
            };
            validate(ref_table)?;
            validate(ref_column)?;
            let name = name_of(std::slice::from_ref(&fk.column), "foreign");
            let mut sql = format!(
                "CONSTRAINT {} FOREIGN KEY ({}) REFERENCES {} ({})",
                q(&name),
                q(&fk.column),
                q(ref_table),
                q(ref_column)
            );
            if let Some(a) = fk.on_delete {
                sql.push_str(&format!(" ON DELETE {}", a.sql()));
            }
            if let Some(a) = fk.on_update {
                sql.push_str(&format!(" ON UPDATE {}", a.sql()));
            }
            parts.push(sql);
        }
        let mut out = vec![format!("CREATE TABLE {} ({})", q(table), parts.join(", "))];
        out.extend(after);
        Ok(out)
    }
}

fn collect<I, S>(columns: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    columns.into_iter().map(|c| c.as_ref().to_owned()).collect()
}
