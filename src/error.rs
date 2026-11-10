/// Any error from Orbit. Database errors come from the driver unchanged.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The database rejected a connection, a query, or a statement.
    #[error(transparent)]
    Database(#[from] sqlx::Error),

    /// A table or column name is empty or contains a backtick.
    #[error("invalid identifier `{0}`: names must not be empty or contain a backtick")]
    InvalidIdentifier(String),

    /// A column is not in the model's `FILLABLE` list.
    #[error("column `{column}` is not fillable on table `{table}`")]
    NotFillable { table: &'static str, column: String },

    /// A query that must find a row found none.
    #[error("no query results for table `{table}`")]
    NotFound { table: &'static str },
}
