# The blocking API

`orbit_orm::blocking` (feature `blocking`) is a synchronous API over the same queries as the async one. It works on every database that the build enables, and each database behaves as in the async API: see [laravel-differences.md](laravel-differences.md#differences-between-databases). Laravel has no blocking or async split, since PHP is synchronous, so there is no Laravel behaviour to match here.

## How it runs

`blocking::Database::connect` builds a current-thread Tokio runtime and connects the pool on it. The `Database` owns that runtime. Each blocking call runs the async query on the runtime with `block_on` and returns its output. The calling thread is blocked until the query finishes.

Call it from a thread that may block, such as `tokio::task::spawn_blocking`. Ignite does this.

## BlockingInRuntime

A blocking call on a thread that already runs async code returns `Error::BlockingInRuntime`. Tokio refuses to start a runtime there and panics before the query is polled. Orbit catches that panic and turns it into the error, so the thread is not stalled. Tokio still prints its own panic message to stderr before the error is returned.

`Database::connect` returns the same error on an async thread. Dropping a `Database` on such a thread is safe: the runtime is shut down in the background.

## Transactions

`Database::transaction` takes a closure that receives a `&mut Transaction`. Pass the transaction to blocking queries as `tx`, and queries given `&db` run on the pool, outside the transaction.

- **Commit.** The transaction commits when the closure returns `Ok`.
- **Rollback on Err.** When the closure returns `Err`, the transaction rolls back and the error is returned to the caller. A failed rollback does not replace that error.
- **Rollback on panic.** The closure runs inside `catch_unwind`. If it panics, the transaction rolls back on the database's runtime, and then the panic is resumed to the caller. The rollback happens before the unwind, because dropping the connection during the unwind needs a Tokio context. Without that step, sqlx returns the connection to the pool with a panic inside a destructor, which aborts the process.

Nothing is committed on a panic. Writes made before the panic are not visible afterwards.

## Tested cases

The cases are in `tests/blocking.rs`. They run against the database that `ORBIT_DATABASE_URL` names, on MySQL and MariaDB, PostgreSQL and SQLite, and are skipped with a notice when it is not set.

- `transaction_commits_when_the_closure_returns_ok`: a committed row is visible afterwards.
- `transaction_rolls_back_when_the_closure_returns_err`: the `Err` is returned, and the row is gone.
- `transaction_rolls_back_when_the_closure_panics`: the panic reaches the caller with its message, and the row written before it is gone.
- `queries_in_the_transaction_see_their_own_writes`: a query on `tx` sees the uncommitted insert, a query on the pool does not, and the row is visible after commit.
- `a_panic_on_a_blocking_thread_rolls_back_and_the_pool_still_works`: a panic inside `transaction` on a `spawn_blocking` thread is observed as a join error, the row is gone, and the pool still inserts and counts afterwards.
- `a_blocking_call_on_an_async_thread_is_an_error`: `BlockingInRuntime` is returned for queries, `transaction` and `connect` on an async thread.
- `spawn_blocking_runs_blocking_calls`: insert and find run on `spawn_blocking` threads.
