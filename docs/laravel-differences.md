# Differences from Laravel

Orbit follows Laravel's Eloquent and query builder, with these exceptions. Each one is listed here so it is not a surprise to someone who knows Laravel.

- **`first()` has no default order.** As in Laravel, `first()` takes the first row the database returns. Call `order_by` when the row matters. Laravel's `sole`, `chunk` and `lazy` enforce an order; Orbit does not add one.
- **Columns outside `FILLABLE` are an error.** `insert` and `update` return `Error::NotFillable` for a column that is not in the model's `FILLABLE` list. Laravel's `Model::create` drops such an attribute without a message, so a misspelt column is ignored. Orbit stops instead, so the mistake shows up at once. Laravel's query builder does not check `$fillable` at all; Orbit checks it on every write.
- **`insert` returns the id.** It returns the auto-increment id as `u64`. Laravel's query builder `insert` returns a boolean, and `insertGetId` returns the id.
- **`where_eq(column, None)` is `IS NULL`**, and `Operator::Ne` with `None` is `IS NOT NULL`, as in Laravel. Other operators with `None` match nothing, where Laravel throws.
- **Terminal methods take the database.** `get(&db)` and the other terminal methods take the `Database` explicitly. Laravel uses a default connection.
