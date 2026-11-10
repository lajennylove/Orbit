/// Any error from Orbit. Database errors come from the driver unchanged.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The database rejected a connection, a query, or a statement.
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}
