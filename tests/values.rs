//! Runs against a real MySQL or MariaDB server. Set `ORBIT_DATABASE_URL` to a test database,
//! such as `mysql://user:password@127.0.0.1:3306/orbit_test`. The tests are skipped with a
//! notice when it is not set. Each test uses its own table and drops it at the end.

use chrono::{NaiveDate, NaiveDateTime};
use orbit_orm::{Database, Model, OrbitModel, Query, Value, raw};
use sqlx::AssertSqlSafe;

async fn connect() -> Option<Database> {
    let Ok(url) = std::env::var("ORBIT_DATABASE_URL") else {
        eprintln!("skipped: set ORBIT_DATABASE_URL to run the database tests");
        return None;
    };
    Some(Database::connect(&url).await.expect("connects"))
}

#[derive(Debug, PartialEq, OrbitModel)]
#[orbit(table = "orbit_values_events")]
struct Event {
    id: i64,
    #[orbit(fillable)]
    happened_at: NaiveDateTime,
    #[orbit(fillable)]
    day: NaiveDate,
    #[orbit(fillable)]
    payload: Vec<u8>,
}

async fn run_sql(db: &Database, sql: &str) {
    sqlx::query(AssertSqlSafe(sql.to_owned()))
        .execute(db.pool())
        .await
        .expect("runs sql");
}

async fn setup(db: &Database) {
    run_sql(db, "DROP TABLE IF EXISTS orbit_values_events").await;
    run_sql(
        db,
        "CREATE TABLE orbit_values_events (
            id BIGINT AUTO_INCREMENT PRIMARY KEY,
            happened_at DATETIME(6) NOT NULL,
            day DATE NOT NULL,
            payload MEDIUMBLOB NOT NULL
        )",
    )
    .await;
}

fn datetime() -> NaiveDateTime {
    NaiveDate::from_ymd_opt(2026, 11, 10)
        .unwrap()
        .and_hms_micro_opt(18, 35, 7, 123_456)
        .unwrap()
}

fn day() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 11, 10).unwrap()
}

/// A zero byte, bytes that are not UTF-8, and a trailing zero.
fn payload() -> Vec<u8> {
    vec![0x00, 0xff, 0xfe, b'o', b'k', 0x80, 0x00]
}

#[tokio::test]
async fn datetime_date_and_bytes_round_trip_through_the_builder_and_update() {
    let Some(db) = connect().await else { return };
    setup(&db).await;

    let id = Query::<Event>::table()
        .insert(
            &db,
            &[
                ("happened_at", Value::from(datetime())),
                ("day", Value::from(day())),
                ("payload", Value::from(payload())),
            ],
        )
        .await
        .expect("inserts");

    let saved = Event::query()
        .find_or_fail(&db, id as i64)
        .await
        .expect("finds");
    assert_eq!(
        saved,
        Event {
            id: id as i64,
            happened_at: datetime(),
            day: day(),
            payload: payload(),
        }
    );
    assert_eq!(
        saved.happened_at.and_utc().timestamp_subsec_micros(),
        123_456
    );

    // A tuple reads the same columns through FromRow, without the derive.
    let tuples: Vec<(NaiveDateTime, NaiveDate, Vec<u8>)> = raw::fetch_all(
        &db,
        "SELECT happened_at, day, payload FROM orbit_values_events",
        &[],
    )
    .await
    .expect("reads");
    assert_eq!(tuples, vec![(datetime(), day(), payload())]);

    // The bound values match the stored ones, so a where clause finds the row.
    let found = Event::query()
        .where_eq("happened_at", datetime())
        .where_eq("day", day())
        .where_eq("payload", payload())
        .count(&db)
        .await
        .expect("counts");
    assert_eq!(found, 1);

    // An update writes the same types, and the row reads back with them.
    let next_day = NaiveDate::from_ymd_opt(2026, 11, 11).unwrap();
    let changed = Event::query()
        .where_eq("id", id)
        .update(
            &db,
            &[
                ("day", Value::from(next_day)),
                ("payload", Value::from(vec![0x00u8, 0x01])),
            ],
        )
        .await
        .expect("updates");
    assert_eq!(changed, 1);
    let updated = Event::query()
        .find_or_fail(&db, id as i64)
        .await
        .expect("finds");
    assert_eq!(updated.day, next_day);
    assert_eq!(updated.payload, vec![0x00, 0x01]);
    assert_eq!(updated.happened_at, datetime());

    run_sql(&db, "DROP TABLE orbit_values_events").await;
    db.close().await;
}
