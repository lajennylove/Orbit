use std::marker::PhantomData;

use sqlx::AssertSqlSafe;
use sqlx::mysql::{MySqlArguments, MySqlQueryResult};

use crate::transaction::{Executor, Target};
use crate::value::bind;
use crate::{Error, Model, Value};

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

/// How a condition joins the one before it. The first condition has no joiner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Joiner {
    And,
    Or,
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
    /// A parenthesised group, built by `where_group`.
    Group(Vec<Clause>),
}

#[derive(Debug, Clone)]
struct Clause {
    joiner: Joiner,
    condition: Condition,
}

/// A query on one model's table. Start it with `Model::query()` or `Query::table()`, then
/// finish it with a terminal method such as `get`, `first` or `insert`.
pub struct Query<M: Model> {
    table: String,
    conditions: Vec<Clause>,
    orders: Vec<(String, Direction)>,
    columns: Option<String>,
    limit: Option<u64>,
    offset: Option<u64>,
    lock: bool,
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
            error,
            ..Self::blank()
        }
    }

    /// A query with no table, for the conditions of a group.
    fn blank() -> Self {
        Self {
            table: String::new(),
            conditions: Vec::new(),
            orders: Vec::new(),
            columns: None,
            limit: None,
            offset: None,
            lock: false,
            error: None,
            model: PhantomData,
        }
    }

    /// Adds `column = value`. A `NULL` value becomes `IS NULL`, as in Laravel.
    pub fn where_eq(self, column: &str, value: impl Into<Value>) -> Self {
        self.compare(Joiner::And, column, "=", value.into())
    }

    /// Adds `OR column = value`.
    pub fn or_where_eq(self, column: &str, value: impl Into<Value>) -> Self {
        self.compare(Joiner::Or, column, "=", value.into())
    }

    /// Adds `column <op> value`. `!=` with a `NULL` value becomes `IS NOT NULL`.
    pub fn where_op(self, column: &str, op: Operator, value: impl Into<Value>) -> Self {
        self.compare(Joiner::And, column, op.sql(), value.into())
    }

    /// Adds `OR column <op> value`. `!=` with a `NULL` value becomes `IS NOT NULL`.
    pub fn or_where_op(self, column: &str, op: Operator, value: impl Into<Value>) -> Self {
        self.compare(Joiner::Or, column, op.sql(), value.into())
    }

    fn compare(self, joiner: Joiner, column: &str, op: &'static str, value: Value) -> Self {
        let mut this = self;
        let column = quote(column, &mut this.error);
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
        this.push(joiner, condition)
    }

    /// Adds `column IN (values)`. An empty list matches no rows, as in Laravel.
    pub fn where_in<V: Into<Value>>(
        self,
        column: &str,
        values: impl IntoIterator<Item = V>,
    ) -> Self {
        self.in_list(Joiner::And, column, values)
    }

    /// Adds `OR column IN (values)`. An empty list matches no rows.
    pub fn or_where_in<V: Into<Value>>(
        self,
        column: &str,
        values: impl IntoIterator<Item = V>,
    ) -> Self {
        self.in_list(Joiner::Or, column, values)
    }

    fn in_list<V: Into<Value>>(
        mut self,
        joiner: Joiner,
        column: &str,
        values: impl IntoIterator<Item = V>,
    ) -> Self {
        let column = quote(column, &mut self.error);
        let values = values.into_iter().map(Into::into).collect();
        self.push(joiner, Condition::In { column, values })
    }

    /// Adds `column IS NULL`.
    pub fn where_null(self, column: &str) -> Self {
        self.null(Joiner::And, column, false)
    }

    /// Adds `OR column IS NULL`.
    pub fn or_where_null(self, column: &str) -> Self {
        self.null(Joiner::Or, column, false)
    }

    /// Adds `column IS NOT NULL`.
    pub fn where_not_null(self, column: &str) -> Self {
        self.null(Joiner::And, column, true)
    }

    /// Adds `OR column IS NOT NULL`.
    pub fn or_where_not_null(self, column: &str) -> Self {
        self.null(Joiner::Or, column, true)
    }

    fn null(mut self, joiner: Joiner, column: &str, negated: bool) -> Self {
        let column = quote(column, &mut self.error);
        self.push(joiner, Condition::Null { column, negated })
    }

    /// Adds a group of conditions joined by `AND`, in parentheses. The closure gets an empty
    /// query and adds its conditions to it, as Laravel's `where(fn)` does:
    ///
    /// ```ignore
    /// .where_group(|q| q.where_eq("title", "a").or_where_eq("title", "b"))
    /// ```
    ///
    /// Only where conditions are kept from the group; `order_by`, `limit` and the other
    /// settings are ignored. An empty group adds nothing.
    pub fn where_group(self, f: impl FnOnce(Self) -> Self) -> Self {
        self.group(Joiner::And, f)
    }

    /// Adds a group of conditions joined by `OR`, in parentheses. See `where_group`.
    pub fn or_where_group(self, f: impl FnOnce(Self) -> Self) -> Self {
        self.group(Joiner::Or, f)
    }

    fn group(mut self, joiner: Joiner, f: impl FnOnce(Self) -> Self) -> Self {
        let inner = f(Self::blank());
        if let Some(error) = inner.error {
            self.error.get_or_insert(error);
        }
        if inner.conditions.is_empty() {
            return self;
        }
        self.push(joiner, Condition::Group(inner.conditions))
    }

    fn push(mut self, joiner: Joiner, condition: Condition) -> Self {
        self.conditions.push(Clause { joiner, condition });
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

    /// Locks the matching rows (`SELECT ... FOR UPDATE`). It applies to `get`, `first`, `find`,
    /// `count` and `exists`. In a transaction the lock lasts until it ends. On the pool it lasts
    /// only for the statement, in autocommit, as Laravel's `lockForUpdate()` does.
    pub fn for_update(mut self) -> Self {
        self.lock = true;
        self
    }

    /// Returns every row that matches.
    pub async fn get<'a>(mut self, db: impl Executor<'a>) -> Result<Vec<M>, Error> {
        self.take_error()?;
        self.rows(db, self.limit).await
    }

    /// Returns the first row in the order given by `order_by`. Without `order_by` there is no
    /// order, as with Laravel's `first`, so call `order_by` when the row matters.
    pub async fn first<'a>(mut self, db: impl Executor<'a>) -> Result<Option<M>, Error> {
        self.take_error()?;
        let rows = self.rows(db, Some(1)).await?;
        Ok(rows.into_iter().next())
    }

    /// Returns the first row, or an error when none matches.
    pub async fn first_or_fail<'a>(self, db: impl Executor<'a>) -> Result<M, Error> {
        self.first(db)
            .await?
            .ok_or(Error::NotFound { table: M::TABLE })
    }

    /// Returns the row with the given primary key, or `None` when there is none.
    pub async fn find<'a>(
        self,
        db: impl Executor<'a>,
        id: impl Into<Value>,
    ) -> Result<Option<M>, Error> {
        self.where_eq(M::PRIMARY_KEY, id).first(db).await
    }

    /// Returns the row with the given primary key, or an error when there is none.
    pub async fn find_or_fail<'a>(
        self,
        db: impl Executor<'a>,
        id: impl Into<Value>,
    ) -> Result<M, Error> {
        self.find(db, id)
            .await?
            .ok_or(Error::NotFound { table: M::TABLE })
    }

    /// Counts the rows that match.
    pub async fn count<'a>(mut self, db: impl Executor<'a>) -> Result<i64, Error> {
        self.take_error()?;
        let target = db.target();
        let lock = self.lock_clause();
        let mut values = Vec::new();
        let sql = format!(
            "SELECT COUNT(*) FROM {}{}{lock}",
            self.table,
            self.where_sql(&mut values)
        );
        scalar(target, sql, bind(values)?).await
    }

    /// Returns whether any row matches.
    pub async fn exists<'a>(mut self, db: impl Executor<'a>) -> Result<bool, Error> {
        self.take_error()?;
        let target = db.target();
        let lock = self.lock_clause();
        let mut values = Vec::new();
        let sql = format!(
            "SELECT EXISTS (SELECT 1 FROM {}{}{lock})",
            self.table,
            self.where_sql(&mut values)
        );
        let found = scalar(target, sql, bind(values)?).await?;
        Ok(found != 0)
    }

    /// Inserts one row and returns its auto-increment id. The columns must be in `FILLABLE`.
    pub async fn insert<'a>(
        mut self,
        db: impl Executor<'a>,
        values: &[(&str, Value)],
    ) -> Result<u64, Error> {
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
        let result = execute(db.target(), sql, bind(params)?).await?;
        Ok(result.last_insert_id())
    }

    /// Sets the columns on every matching row and returns how many rows changed. The columns
    /// must be in `FILLABLE`. Order and limit are ignored.
    pub async fn update<'a>(
        mut self,
        db: impl Executor<'a>,
        values: &[(&str, Value)],
    ) -> Result<u64, Error> {
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
        let result = execute(db.target(), sql, bind(params)?).await?;
        Ok(result.rows_affected())
    }

    /// Deletes every matching row and returns how many rows were deleted. Order and limit are
    /// ignored.
    pub async fn delete<'a>(mut self, db: impl Executor<'a>) -> Result<u64, Error> {
        self.take_error()?;
        let mut values = Vec::new();
        let sql = format!("DELETE FROM {}{}", self.table, self.where_sql(&mut values));
        let result = execute(db.target(), sql, bind(values)?).await?;
        Ok(result.rows_affected())
    }

    /// Returns the first error recorded while building the query.
    fn take_error(&mut self) -> Result<(), Error> {
        match self.error.take() {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// The ` FOR UPDATE` clause, or nothing.
    fn lock_clause(&self) -> &'static str {
        if self.lock { " FOR UPDATE" } else { "" }
    }

    async fn rows<'a>(&self, db: impl Executor<'a>, limit: Option<u64>) -> Result<Vec<M>, Error> {
        let target = db.target();
        let lock = self.lock_clause();
        let columns = self.columns.as_deref().unwrap_or("*");
        let mut values = Vec::new();
        let sql = self.select_sql(columns, limit, lock, &mut values);
        fetch_rows(target, sql, bind(values)?).await
    }

    /// Builds `SELECT` with the where clauses, order, limit, offset and lock. Each bound value
    /// is pushed to `values` in placeholder order.
    fn select_sql(
        &self,
        columns: &str,
        limit: Option<u64>,
        lock: &str,
        values: &mut Vec<Value>,
    ) -> String {
        let mut sql = format!("SELECT {columns} FROM {}", self.table);
        sql.push_str(&self.where_sql(values));
        if !self.orders.is_empty() {
            let terms: Vec<String> = self
                .orders
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
        } else if self.offset.is_some() {
            // MySQL needs a LIMIT before OFFSET, so use its largest value.
            sql.push_str(" LIMIT 18446744073709551615");
        }
        if let Some(offset) = self.offset {
            sql.push_str(" OFFSET ?");
            values.push(Value::UInt(offset));
        }
        sql.push_str(lock);
        sql
    }

    /// Builds ` WHERE ...` from the conditions. Each bound value is pushed to `values`.
    fn where_sql(&self, values: &mut Vec<Value>) -> String {
        if self.conditions.is_empty() {
            return String::new();
        }
        format!(" WHERE {}", render(&self.conditions, values))
    }
}

/// Joins the clauses with their `AND` or `OR`. Each bound value is pushed to `values` in order.
fn render(clauses: &[Clause], values: &mut Vec<Value>) -> String {
    let mut sql = String::new();
    for (index, clause) in clauses.iter().enumerate() {
        if index > 0 {
            sql.push_str(match clause.joiner {
                Joiner::And => " AND ",
                Joiner::Or => " OR ",
            });
        }
        match &clause.condition {
            Condition::Compare { column, op, value } => {
                values.push(value.clone());
                sql.push_str(&format!("{column} {op} ?"));
            }
            Condition::In { values: list, .. } if list.is_empty() => sql.push_str("0 = 1"),
            Condition::In {
                column,
                values: list,
            } => {
                values.extend(list.iter().cloned());
                sql.push_str(&format!(
                    "{column} IN ({})",
                    vec!["?"; list.len()].join(", ")
                ));
            }
            Condition::Null { column, negated } => {
                let not = if *negated { "NOT " } else { "" };
                sql.push_str(&format!("{column} IS {not}NULL"));
            }
            Condition::Group(inner) => {
                sql.push_str(&format!("({})", render(inner, values)));
            }
        }
    }
    sql
}

/// Runs a statement that changes rows, on the pool or on the transaction's connection.
async fn execute(
    target: Target<'_>,
    sql: String,
    args: MySqlArguments,
) -> Result<MySqlQueryResult, Error> {
    let query = sqlx::query_with(AssertSqlSafe(sql), args);
    Ok(match target {
        Target::Pool(pool) => query.execute(pool).await?,
        Target::Transaction(conn) => query.execute(conn).await?,
    })
}

/// Runs a query that returns one integer, on the pool or on the transaction's connection.
async fn scalar(target: Target<'_>, sql: String, args: MySqlArguments) -> Result<i64, Error> {
    let query = sqlx::query_scalar_with::<_, i64, _>(AssertSqlSafe(sql), args);
    Ok(match target {
        Target::Pool(pool) => query.fetch_one(pool).await?,
        Target::Transaction(conn) => query.fetch_one(conn).await?,
    })
}

/// Runs a `SELECT` and maps its rows to the model, on the pool or on the transaction's connection.
async fn fetch_rows<M: Model>(
    target: Target<'_>,
    sql: String,
    args: MySqlArguments,
) -> Result<Vec<M>, Error> {
    let query = sqlx::query_as_with::<_, M, _>(AssertSqlSafe(sql), args);
    Ok(match target {
        Target::Pool(pool) => query.fetch_all(pool).await?,
        Target::Transaction(conn) => query.fetch_all(conn).await?,
    })
}
