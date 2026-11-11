use crate::Query;
use crate::driver::rows::DriverRows;

/// A table and the Rust type that its rows map to. Derive it with `#[derive(OrbitModel)]`, or write it by hand.
///
/// A model decodes from the row type of every database that the build enables, so `DriverRows`
/// holds for it. With only one database enabled, that is the one `FromRow` for that database.
pub trait Model: DriverRows + Send + Unpin + Sized {
    /// The table name.
    const TABLE: &'static str;

    /// The primary key, used by `find`. `first` has no default order, as in Laravel.
    const PRIMARY_KEY: &'static str = "id";

    /// The columns that `insert` and `update` may write. Empty by default, as in Laravel's `$fillable`.
    const FILLABLE: &'static [&'static str] = &[];

    /// Starts a query on the table.
    fn query() -> Query<Self> {
        Query::table()
    }
}
