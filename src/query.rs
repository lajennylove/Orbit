use std::marker::PhantomData;

use sqlx::AssertSqlSafe;

use crate::value::bind;
use crate::{Database, Error, Model, Value};

/// A comparison for `where_op`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operator {
    Lt,
    Le,
    Gt,
    Ge,
    Ne,
    Like,
}

impl Operator {
    fn sql(self) -> &'static str {
        match self {
            Self::Lt => "<",
            Self::Le => "<=",
            Self::Gt => ">",
            Self::Ge => ">=",
            Self::Ne => "!=",
            Self::Like => "LIKE",
        }
    }
}

/// The sort direction for `order_by`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Asc,
    Desc,
}

#[derive(Debug, Clone)]
enum Condition {
    Compare {
        column: String,
        op: &'static str,
        value: Value,
    },
    In {
        column: String,
        values: Vec<Value>,
    },
    Null {
        column: String,
        negated: bool,
    },
}

/// A query on one model's table. Start it with `Model::query()` or `Query::table()`, then
/// finish it with a terminal method such as `get`, `first` or `insert`.
pub struct Query<M: Model> {
    table: String,
    conditions: Vec<Condition>,
    orders: Vec<(String, Direction)>,
    columns: Option<String>,
    limit: Option<u64>,
    offset: Option<u64>,
    error: Option<Error>,
    model: PhantomData<M>,
}

/// Quotes a table or column name with backticks.
fn checked(name: &str) -> Result<String, Error> {
    if name.is_empty() || name.contains('`') {
        return Err(Error::InvalidIdentifier(name.to_owned()));
    }
    Ok(format!("`{name}`"))
}

/// Quotes a name, or records the error and returns an empty name for the terminal to report.
fn quote(name: &str, error: &mut Option<Error>) -> String {
    checked(name).unwrap_or_else(|e| {
        error.get_or_insert(e);
        String::new()
    })
}

/// Quotes a column that `insert` or `update` writes, and checks it against `FILLABLE`.
fn fillable<M: Model>(column: &str) -> Result<String, Error> {
    if !M::FILLABLE.contains(&column) {
        return Err(Error::NotFillable {
            table: M::TABLE,
            column: column.to_owned(),
        });
    }
    checked(column)
}

impl<M: Model> Query<M> {
    /// Starts a query on the model's table.
    pub fn table() -> Self {
        let mut error = None;
        let table = quote(M::TABLE, &mut error);
        Self {
            table,
            conditions: Vec::new(),
            orders: Vec::new(),
            columns: None,
            limit: None,
            offset: None,
            error,
            model: PhantomData,
        }
    }

    /// Adds `column = value`. A `NULL` value becomes `IS NULL`, as in Laravel.
    pub fn where_eq(self, column: &str, value: impl Into<Value>) -> Self {
        self.compare(column, "=", value.into())
    }

    /// Adds `column <op> value`. `!=` with a `NULL` value becomes `IS NOT NULL`.
    pub fn where_op(self, column: &str, op: Operator, value: impl Into<Value>) -> Self {
        self.compare(column, op.sql(), value.into())
    }

    fn compare(mut self, column: &str, op: &'static str, value: Value) -> Self {
        let column = quote(column, &mut self.error);
        let condition = match (value, op) {
            (Value::Null, "=") => Condition::Null {
                column,
                negated: false,
            },
            (Value::Null, "!=") => Condition::Null {
                column,
                negated: true,
            },
            (value, op) => Condition::Compare { column, op, value },
        };
        self.conditions.push(condition);
        self
    }

    /// Adds `column IN (values)`. An empty list matches no rows, as in Laravel.
    pub fn where_in<V: Into<Value>>(
        mut self,
        column: &str,
        values: impl IntoIterator<Item = V>,
    ) -> Self {
        let column = quote(column, &mut self.error);
        let values = values.into_iter().map(Into::into).collect();
        self.conditions.push(Condition::In { column, values });
        self
    }

    /// Adds `column IS NULL`.
    pub fn where_null(mut self, column: &str) -> Self {
        let column = quote(column, &mut self.error);
        self.conditions.push(Condition::Null {
            column,
            negated: false,
        });
        self
    }

    /// Adds `column IS NOT NULL`.
    pub fn where_not_null(mut self, column: &str) -> Self {
        let column = quote(column, &mut self.error);
        self.conditions.push(Condition::Null {
            column,
            negated: true,
        });
        self
    }

    /// Adds a sort. Later calls sort within the earlier ones.
    pub fn order_by(mut self, column: &str, direction: Direction) -> Self {
        let column = quote(column, &mut self.error);
        self.orders.push((column, direction));
        self
    }

    /// Chooses the columns that `get` and `first` select. Without it, all columns are selected.
    pub fn select(mut self, columns: &[&str]) -> Self {
        if columns.is_empty() {
            return self;
        }
        let quoted: Vec<String> = columns
            .iter()
            .map(|column| quote(column, &mut self.error))
            .collect();
        self.columns = Some(quoted.join(", "));
        self
    }

    /// Returns at most `limit` rows.
    pub fn limit(mut self, limit: u64) -> Self {
        self.limit = Some(limit);
        self
    }

    /// Skips the first `offset` rows.
    pub fn offset(mut self, offset: u64) -> Self {
        self.offset = Some(offset);
        self
    }

    /// Returns every row that matches.
    pub async fn get(mut self, db: &Database) -> Result<Vec<M>, Error> {
        self.take_error()?;
        self.rows(db, &self.orders, self.limit, self.offset).await
    }

    /// Returns the first row in the order given by `order_by`. Without `order_by` there is no
    /// order, as with Laravel's `first`, so call `order_by` when the row matters.
    pub async fn first(mut self, db: &Database) -> Result<Option<M>, Error> {
        self.take_error()?;
        let rows = self.rows(db, &self.orders, Some(1), self.offset).await?;
        Ok(rows.into_iter().next())
    }

    /// Returns the first row, or an error when none matches.
    pub async fn first_or_fail(self, db: &Database) -> Result<M, Error> {
        self.first(db)
            .await?
            .ok_or(Error::NotFound { table: M::TABLE })
    }

    /// Returns the row with the given primary key, or `None` when there is none.
    pub async fn find(self, db: &Database, id: impl Into<Value>) -> Result<Option<M>, Error> {
        self.where_eq(M::PRIMARY_KEY, id).first(db).await
    }

    /// Returns the row with the given primary key, or an error when there is none.
    pub async fn find_or_fail(self, db: &Database, id: impl Into<Value>) -> Result<M, Error> {
        self.find(db, id)
            .await?
            .ok_or(Error::NotFound { table: M::TABLE })
    }

    /// Counts the rows that match.
    pub async fn count(mut self, db: &Database) -> Result<i64, Error> {
        self.take_error()?;
        let mut values = Vec::new();
        let sql = format!(
            "SELECT COUNT(*) FROM {}{}",
            self.table,
            self.where_sql(&mut values)
        );
        let args = bind(values)?;
        Ok(
            sqlx::query_scalar_with::<_, i64, _>(AssertSqlSafe(sql), args)
                .fetch_one(db.pool())
                .await?,
        )
    }

    /// Returns whether any row matches.
    pub async fn exists(mut self, db: &Database) -> Result<bool, Error> {
        self.take_error()?;
        let mut values = Vec::new();
        let sql = format!(
            "SELECT EXISTS (SELECT 1 FROM {}{})",
            self.table,
            self.where_sql(&mut values)
        );
        let args = bind(values)?;
        let found: i64 = sqlx::query_scalar_with(AssertSqlSafe(sql), args)
            .fetch_one(db.pool())
            .await?;
        Ok(found != 0)
    }

    /// Inserts one row and returns its auto-increment id. The columns must be in `FILLABLE`.
    pub async fn insert(mut self, db: &Database, values: &[(&str, Value)]) -> Result<u64, Error> {
        self.take_error()?;
        let mut columns = Vec::new();
        let mut params = Vec::new();
        for (column, value) in values {
            columns.push(fillable::<M>(column)?);
            params.push(value.clone());
        }
        let sql = if columns.is_empty() {
            format!("INSERT INTO {} () VALUES ()", self.table)
        } else {
            format!(
                "INSERT INTO {} ({}) VALUES ({})",
                self.table,
                columns.join(", "),
                vec!["?"; columns.len()].join(", ")
            )
        };
        let result = sqlx::query_with(AssertSqlSafe(sql), bind(params)?)
            .execute(db.pool())
            .await?;
        Ok(result.last_insert_id())
    }

    /// Sets the columns on every matching row and returns how many rows changed. The columns
    /// must be in `FILLABLE`. Order and limit are ignored.
    pub async fn update(mut self, db: &Database, values: &[(&str, Value)]) -> Result<u64, Error> {
        self.take_error()?;
        if values.is_empty() {
            return Ok(0);
        }
        let mut sets = Vec::new();
        let mut params = Vec::new();
        for (column, value) in values {
            sets.push(format!("{} = ?", fillable::<M>(column)?));
            params.push(value.clone());
        }
        let mut sql = format!("UPDATE {} SET {}", self.table, sets.join(", "));
        sql.push_str(&self.where_sql(&mut params));
        let result = sqlx::query_with(AssertSqlSafe(sql), bind(params)?)
            .execute(db.pool())
            .await?;
        Ok(result.rows_affected())
    }

    /// Deletes every matching row and returns how many rows were deleted. Order and limit are
    /// ignored.
    pub async fn delete(mut self, db: &Database) -> Result<u64, Error> {
        self.take_error()?;
        let mut values = Vec::new();
        let sql = format!("DELETE FROM {}{}", self.table, self.where_sql(&mut values));
        let result = sqlx::query_with(AssertSqlSafe(sql), bind(values)?)
            .execute(db.pool())
            .await?;
        Ok(result.rows_affected())
    }

    /// Returns the first error recorded while building the query.
    fn take_error(&mut self) -> Result<(), Error> {
        match self.error.take() {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    async fn rows(
        &self,
        db: &Database,
        order: &[(String, Direction)],
        limit: Option<u64>,
        offset: Option<u64>,
    ) -> Result<Vec<M>, Error> {
        let columns = self.columns.as_deref().unwrap_or("*");
        let mut values = Vec::new();
        let sql = self.select_sql(columns, order, limit, offset, &mut values);
        let args = bind(values)?;
        Ok(sqlx::query_as_with::<_, M, _>(AssertSqlSafe(sql), args)
            .fetch_all(db.pool())
            .await?)
    }

    /// Builds `SELECT` with the where clauses, order, limit and offset. Each bound value is
    /// pushed to `values` in placeholder order.
    fn select_sql(
        &self,
        columns: &str,
        order: &[(String, Direction)],
        limit: Option<u64>,
        offset: Option<u64>,
        values: &mut Vec<Value>,
    ) -> String {
        let mut sql = format!("SELECT {columns} FROM {}", self.table);
        sql.push_str(&self.where_sql(values));
        if !order.is_empty() {
            let terms: Vec<String> = order
                .iter()
                .map(|(column, direction)| match direction {
                    Direction::Asc => format!("{column} ASC"),
                    Direction::Desc => format!("{column} DESC"),
                })
                .collect();
            sql.push_str(&format!(" ORDER BY {}", terms.join(", ")));
        }
        if let Some(limit) = limit {
            sql.push_str(" LIMIT ?");
            values.push(Value::UInt(limit));
        } else if offset.is_some() {
            // MySQL needs a LIMIT before OFFSET, so use its largest value.
            sql.push_str(" LIMIT 18446744073709551615");
        }
        if let Some(offset) = offset {
            sql.push_str(" OFFSET ?");
            values.push(Value::UInt(offset));
        }
        sql
    }

    /// Builds ` WHERE ...` from the conditions. Each bound value is pushed to `values`.
    fn where_sql(&self, values: &mut Vec<Value>) -> String {
        if self.conditions.is_empty() {
            return String::new();
        }
        let parts: Vec<String> = self
            .conditions
            .iter()
            .map(|condition| match condition {
                Condition::Compare { column, op, value } => {
                    values.push(value.clone());
                    format!("{column} {op} ?")
                }
                Condition::In { values: list, .. } if list.is_empty() => "0 = 1".to_owned(),
                Condition::In {
                    column,
                    values: list,
                } => {
                    values.extend(list.iter().cloned());
                    format!("{column} IN ({})", vec!["?"; list.len()].join(", "))
                }
                Condition::Null { column, negated } => {
                    let not = if *negated { "NOT " } else { "" };
                    format!("{column} IS {not}NULL")
                }
            })
            .collect();
        format!(" WHERE {}", parts.join(" AND "))
    }
}
