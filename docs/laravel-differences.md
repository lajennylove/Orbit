# Differences from Laravel

Orbit follows Laravel's Eloquent and query builder, with these exceptions. Each one is listed here so it is not a surprise to someone who knows Laravel.

- **`first()` has no default order.** As in Laravel, `first()` takes the first row the database returns. Call `order_by` when the row matters. Laravel's `sole`, `chunk` and `lazy` enforce an order; Orbit does not add one.
- **Columns outside `FILLABLE` are an error.** `insert` and `update` return `Error::NotFillable` for a column that is not in the model's `FILLABLE` list. Laravel's `Model::create` drops such an attribute without a message, so a misspelt column is ignored. Orbit stops instead, so the mistake shows up at once. Laravel's query builder does not check `$fillable` at all; Orbit checks it on every write.
- **`insert` returns the id.** It returns the auto-increment id as `u64`. Laravel's query builder `insert` returns a boolean, and `insertGetId` returns the id.
- **`where_eq(column, None)` is `IS NULL`**, and `Operator::Ne` with `None` is `IS NOT NULL`, as in Laravel. Other operators with `None` match nothing, where Laravel throws.
- **Terminal methods take the database.** `get(&db)` and the other terminal methods take the `Database` explicitly. Laravel uses a default connection.
- **Transactions take an async closure that receives the transaction.** `Database::transaction` passes the transaction as `tx`, and queries run on it when given `&mut *tx`. Laravel's `DB::transaction` takes no argument and uses the default connection. The closure's error type must convert from `Error`.
- **Transactions do not retry.** Laravel's `DB::transaction($callback, $attempts)` retries on a deadlock. Orbit returns `Error::Deadlock` and leaves the retry to the caller.
- **`for_update` needs a transaction.** Outside a transaction, `for_update` returns `Error::LockOutsideTransaction`. Laravel's `lockForUpdate()` runs without an error, but the lock lasts only for that statement in autocommit mode.
- **Grouped conditions are `where_group` and `or_where_group`.** They take a closure that receives an empty query, as Laravel's `where(fn)` does. Only the conditions are kept from the group; `order_by`, `limit` and the other settings inside it are ignored.
- **Errors are typed.** Unique and foreign key violations, deadlocks and lock wait timeouts are variants of `Error`. Laravel's `QueryException` carries the SQLSTATE code instead. A unique violation's index name comes from the server's message, because the MySQL driver does not return it as a separate field.
- **`replace_into` is not implemented.** MySQL's `REPLACE INTO` has no Laravel equivalent, so Orbit does not add it.
