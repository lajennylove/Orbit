use std::marker::PhantomData;

use crate::backend;
use crate::driver::rows::DriverRows;
use crate::transaction::Executor;
use crate::{Driver, Error, Model, Value};

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

/// A condition, with its names as given. They are quoted when the SQL is built, for the driver
/// that runs it.
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
    columns: Option<Vec<String>>,
    limit: Option<u64>,
    offset: Option<u64>,
    lock: bool,
    error: Option<Error>,
    model: PhantomData<M>,
}

/// The values bound to one statement, in placeholder order. Each call to `bind` adds one and
/// returns the placeholder for it, in the driver's syntax.
pub(crate) struct Params {
    driver: Driver,
    pub(crate) values: Vec<Value>,
}

impl Params {
    pub(crate) fn new(driver: Driver) -> Self {
        Self {
            driver,
            values: Vec::new(),
        }
    }

    /// Adds `value` and returns its placeholder. A `NULL` is written as `NULL` on drivers that
    /// inline it, and is not bound.
    pub(crate) fn bind(&mut self, value: Value) -> String {
        if matches!(value, Value::Null) && self.driver.inlines_null() {
            return "NULL".to_owned();
        }
        self.values.push(value);
        self.driver.placeholder(self.values.len())
    }
}

/// Checks a table or column name. It must not be empty, and must not contain a backtick or a
/// double quote, which are the quote characters of the supported databases.
fn validate(name: &str) -> Result<(), Error> {
    if name.is_empty() || name.contains(['`', '"']) {
        return Err(Error::InvalidIdentifier(name.to_owned()));
    }
    Ok(())
}

/// Checks a name, or records the error and returns the name for the terminal to report.
fn checked(name: &str, error: &mut Option<Error>) -> String {
    if let Err(e) = validate(name) {
        error.get_or_insert(e);
    }
    name.to_owned()
}

/// Checks a column that `insert` or `update` writes against `FILLABLE`.
fn fillable<M: Model>(column: &str) -> Result<(), Error> {
    if !M::FILLABLE.contains(&column) {
        return Err(Error::NotFillable {
            table: M::TABLE,
            column: column.to_owned(),
        });
    }
    validate(column)
}

impl<M: Model> Query<M> {
    /// Starts a query on the model's table.
    pub fn table() -> Self {
        let mut error = None;
        let table = checked(M::TABLE, &mut error);
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
        let column = checked(column, &mut this.error);
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
        let column = checked(column, &mut self.error);
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
        let column = checked(column, &mut self.error);
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
        let column = checked(column, &mut self.error);
        self.orders.push((column, direction));
        self
    }

    /// Chooses the columns that `get` and `first` select. Without it, all columns are selected.
    pub fn select(mut self, columns: &[&str]) -> Self {
        if columns.is_empty() {
            return self;
        }
        let names = columns
            .iter()
            .map(|column| checked(column, &mut self.error))
            .collect();
        self.columns = Some(names);
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
    /// only for the statement, in autocommit, as Laravel's `lockForUpdate()` does. SQLite has no
    /// row locks, so the clause is left out there, as Laravel's SQLite grammar does.
    pub fn for_update(mut self) -> Self {
        self.lock = true;
        self
    }

    /// Returns every row that matches.
    pub async fn get<'a>(mut self, db: impl Executor<'a>) -> Result<Vec<M>, Error>
    where
        M: DriverRows,
    {
        self.take_error()?;
        self.rows(db, self.limit).await
    }

    /// Returns every row that matches, decoded as `T` instead of the model. Use it with `select`
    /// to read some columns into a tuple or a struct that implements `FromRow`. The clauses are
    /// the same as `get`.
    ///
    /// ```ignore
    /// let titles = Post::query()
    ///     .select(&["id", "title"])
    ///     .get_as::<(i64, String)>(&db)
    ///     .await?;
    /// ```
    pub async fn get_as<'a, T>(mut self, db: impl Executor<'a>) -> Result<Vec<T>, Error>
    where
        T: DriverRows + Send + Unpin,
    {
        self.take_error()?;
        self.rows::<T>(db, self.limit).await
    }

    /// Returns the first row decoded as `T`, in the order given by `order_by`. See `get_as`.
    pub async fn first_as<'a, T>(mut self, db: impl Executor<'a>) -> Result<Option<T>, Error>
    where
        T: DriverRows + Send + Unpin,
    {
        self.take_error()?;
        let rows = self.rows::<T>(db, Some(1)).await?;
        Ok(rows.into_iter().next())
    }

    /// Returns the first row in the order given by `order_by`. Without `order_by` there is no
    /// order, as with Laravel's `first`, so call `order_by` when the row matters.
    pub async fn first<'a>(mut self, db: impl Executor<'a>) -> Result<Option<M>, Error>
    where
        M: DriverRows,
    {
        self.take_error()?;
        let rows = self.rows::<M>(db, Some(1)).await?;
        Ok(rows.into_iter().next())
    }

    /// Returns the first row, or an error when none matches.
    pub async fn first_or_fail<'a>(self, db: impl Executor<'a>) -> Result<M, Error>
    where
        M: DriverRows,
    {
        self.first(db)
            .await?
            .ok_or(Error::NotFound { table: M::TABLE })
    }

    /// Returns the row with the given primary key, or `None` when there is none.
    pub async fn find<'a>(
        self,
        db: impl Executor<'a>,
        id: impl Into<Value>,
    ) -> Result<Option<M>, Error>
    where
        M: DriverRows,
    {
        self.where_eq(M::PRIMARY_KEY, id).first(db).await
    }

    /// Returns the row with the given primary key, or an error when there is none.
    pub async fn find_or_fail<'a>(
        self,
        db: impl Executor<'a>,
        id: impl Into<Value>,
    ) -> Result<M, Error>
    where
        M: DriverRows,
    {
        self.find(db, id)
            .await?
            .ok_or(Error::NotFound { table: M::TABLE })
    }

    /// Counts the rows that match.
    pub async fn count<'a>(mut self, db: impl Executor<'a>) -> Result<i64, Error> {
        self.take_error()?;
        let target = db.target();
        let driver = target.driver();
        let mut params = Params::new(driver);
        let sql = format!(
            "SELECT COUNT(*) FROM {}{}{}",
            self.table_sql(driver),
            self.where_sql(driver, &mut params),
            self.lock_sql(driver)
        );
        Ok(backend::scalar::<i64>(target, &sql, &params.values).await?)
    }

    /// Returns whether any row matches.
    pub async fn exists<'a>(mut self, db: impl Executor<'a>) -> Result<bool, Error> {
        self.take_error()?;
        let target = db.target();
        let driver = target.driver();
        let mut params = Params::new(driver);
        // The flag is cast to an integer so that every driver returns the same type.
        let sql = format!(
            "SELECT CAST(CASE WHEN EXISTS (SELECT 1 FROM {}{}{}) THEN 1 ELSE 0 END AS {})",
            self.table_sql(driver),
            self.where_sql(driver, &mut params),
            self.lock_sql(driver),
            driver.bigint()
        );
        let found = backend::scalar::<i64>(target, &sql, &params.values).await?;
        Ok(found != 0)
    }

    /// Inserts one row and returns its auto-increment id. The columns must be in `FILLABLE`.
    /// On PostgreSQL the id comes from `RETURNING` on the primary key, so the primary key must be
    /// an integer column.
    pub async fn insert<'a>(
        mut self,
        db: impl Executor<'a>,
        values: &[(&str, Value)],
    ) -> Result<u64, Error> {
        self.take_error()?;
        let target = db.target();
        let driver = target.driver();
        let mut params = Params::new(driver);
        let returning = driver.insert_returns_id().then(|| {
            format!(
                " RETURNING {}::{}",
                driver.quote(M::PRIMARY_KEY),
                driver.bigint()
            )
        });
        let sql = self.insert_sql(driver, "INSERT", &mut params, values, returning.as_deref())?;
        if returning.is_some() {
            let id: i64 = backend::scalar(target, &sql, &params.values).await?;
            return u64::try_from(id).map_err(|_| Error::Database(sqlx::Error::RowNotFound));
        }
        let outcome = backend::execute(target, &sql, &params.values).await?;
        outcome.insert_id.ok_or(Error::Unsupported {
            operation: "reading the insert id",
            driver: driver.name(),
        })
    }

    /// Replaces the row that has the same unique key or primary key as `values`, or inserts the
    /// row when none has. Returns the rows affected, as the database reports them: MySQL gives 2
    /// when a row was replaced, since it deletes the old row and inserts the new one, and SQLite
    /// gives 1.
    ///
    /// This is Diesel's `replace_into`, which issues `REPLACE INTO` on MySQL and SQLite. It is not
    /// Laravel's `upsert`, which is `INSERT ... ON DUPLICATE KEY UPDATE` or `ON CONFLICT`. The old
    /// row is deleted, so its auto-increment id changes and columns not in `values` take their
    /// defaults. The columns must be in `FILLABLE`.
    ///
    /// PostgreSQL has no `REPLACE`, and `INSERT ... ON CONFLICT` would keep the old row. So on
    /// PostgreSQL this returns `Error::Unsupported`.
    pub async fn replace<'a>(
        mut self,
        db: impl Executor<'a>,
        values: &[(&str, Value)],
    ) -> Result<u64, Error> {
        self.take_error()?;
        let target = db.target();
        let driver = target.driver();
        if !driver.supports_replace() {
            return Err(Error::Unsupported {
                operation: "replace",
                driver: driver.name(),
            });
        }
        let mut params = Params::new(driver);
        let sql = self.insert_sql(driver, "REPLACE", &mut params, values, None)?;
        let outcome = backend::execute(target, &sql, &params.values).await?;
        Ok(outcome.rows_affected)
    }

    /// Builds `INSERT INTO table (columns) VALUES (...)`, with `returning` appended. The columns
    /// must be in `FILLABLE`.
    fn insert_sql(
        &self,
        driver: Driver,
        keyword: &str,
        params: &mut Params,
        values: &[(&str, Value)],
        returning: Option<&str>,
    ) -> Result<String, Error> {
        let mut columns = Vec::with_capacity(values.len());
        let mut placeholders = Vec::with_capacity(values.len());
        for (column, value) in values {
            fillable::<M>(column)?;
            columns.push(driver.quote(column));
            placeholders.push(params.bind(value.clone()));
        }
        let table = self.table_sql(driver);
        let returning = returning.unwrap_or("");
        Ok(if columns.is_empty() {
            format!(
                "{keyword} INTO {table} {}{returning}",
                driver.empty_insert()
            )
        } else {
            format!(
                "{keyword} INTO {table} ({}) VALUES ({}){returning}",
                columns.join(", "),
                placeholders.join(", ")
            )
        })
    }

    /// Sets the columns on every matching row and returns how many rows matched. The columns
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
        let target = db.target();
        let driver = target.driver();
        let mut params = Params::new(driver);
        let mut sets = Vec::with_capacity(values.len());
        for (column, value) in values {
            fillable::<M>(column)?;
            let placeholder = params.bind(value.clone());
            sets.push(format!("{} = {placeholder}", driver.quote(column)));
        }
        let mut sql = format!("UPDATE {} SET {}", self.table_sql(driver), sets.join(", "));
        sql.push_str(&self.where_sql(driver, &mut params));
        let outcome = backend::execute(target, &sql, &params.values).await?;
        Ok(outcome.rows_affected)
    }

    /// Deletes every matching row and returns how many rows were deleted. Order and limit are
    /// ignored.
    pub async fn delete<'a>(mut self, db: impl Executor<'a>) -> Result<u64, Error> {
        self.take_error()?;
        let target = db.target();
        let driver = target.driver();
        let mut params = Params::new(driver);
        let sql = format!(
            "DELETE FROM {}{}",
            self.table_sql(driver),
            self.where_sql(driver, &mut params)
        );
        let outcome = backend::execute(target, &sql, &params.values).await?;
        Ok(outcome.rows_affected)
    }

    /// Returns the first error recorded while building the query.
    fn take_error(&mut self) -> Result<(), Error> {
        match self.error.take() {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// The lock clause when `for_update` was asked for, or nothing.
    fn lock_sql(&self, driver: Driver) -> &'static str {
        if self.lock { driver.lock_clause() } else { "" }
    }

    /// The quoted table name.
    fn table_sql(&self, driver: Driver) -> String {
        driver.quote(&self.table)
    }

    async fn rows<'a, T>(&self, db: impl Executor<'a>, limit: Option<u64>) -> Result<Vec<T>, Error>
    where
        T: DriverRows + Send + Unpin,
    {
        let target = db.target();
        let driver = target.driver();
        let columns = match &self.columns {
            Some(names) => names
                .iter()
                .map(|name| driver.quote(name))
                .collect::<Vec<_>>()
                .join(", "),
            None => "*".to_owned(),
        };
        let mut params = Params::new(driver);
        let sql = self.select_sql(driver, &columns, limit, &mut params);
        Ok(backend::fetch_all::<T>(target, &sql, &params.values).await?)
    }

    /// Builds `SELECT` with the where clauses, order, limit, offset and lock.
    fn select_sql(
        &self,
        driver: Driver,
        columns: &str,
        limit: Option<u64>,
        params: &mut Params,
    ) -> String {
        let mut sql = format!("SELECT {columns} FROM {}", self.table_sql(driver));
        sql.push_str(&self.where_sql(driver, params));
        if !self.orders.is_empty() {
            let terms: Vec<String> = self
                .orders
                .iter()
                .map(|(column, direction)| match direction {
                    Direction::Asc => format!("{} ASC", driver.quote(column)),
                    Direction::Desc => format!("{} DESC", driver.quote(column)),
                })
                .collect();
            sql.push_str(&format!(" ORDER BY {}", terms.join(", ")));
        }
        if let Some(limit) = limit {
            sql.push_str(&format!(" LIMIT {}", params.bind(Value::UInt(limit))));
        } else if let (true, Some(all)) = (self.offset.is_some(), driver.offset_only_limit()) {
            // MySQL and SQLite need a LIMIT before OFFSET, so they get their largest value.
            sql.push_str(&format!(" LIMIT {all}"));
        }
        if let Some(offset) = self.offset {
            sql.push_str(&format!(" OFFSET {}", params.bind(Value::UInt(offset))));
        }
        sql.push_str(self.lock_sql(driver));
        sql
    }

    /// Builds ` WHERE ...` from the conditions, or nothing.
    fn where_sql(&self, driver: Driver, params: &mut Params) -> String {
        if self.conditions.is_empty() {
            return String::new();
        }
        format!(" WHERE {}", render(driver, &self.conditions, params))
    }
}

/// Joins the clauses with their `AND` or `OR`. Values are bound in the order they appear.
fn render(driver: Driver, clauses: &[Clause], params: &mut Params) -> String {
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
                let placeholder = params.bind(value.clone());
                sql.push_str(&format!("{} {op} {placeholder}", driver.quote(column)));
            }
            Condition::In { values, .. } if values.is_empty() => sql.push_str("0 = 1"),
            Condition::In { column, values } => {
                let placeholders: Vec<String> =
                    values.iter().map(|v| params.bind(v.clone())).collect();
                sql.push_str(&format!(
                    "{} IN ({})",
                    driver.quote(column),
                    placeholders.join(", ")
                ));
            }
            Condition::Null { column, negated } => {
                let not = if *negated { "NOT " } else { "" };
                sql.push_str(&format!("{} IS {not}NULL", driver.quote(column)));
            }
            Condition::Group(inner) => {
                sql.push_str(&format!("({})", render(driver, inner, params)));
            }
        }
    }
    sql
}
