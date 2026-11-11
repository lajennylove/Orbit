//! Runs the `OrbitModel` derive against a real database: MySQL or MariaDB, PostgreSQL or SQLite.
//! Set `ORBIT_DATABASE_URL` to a test database, such as `mysql://user@127.0.0.1:3306/orbit_test`.
//! The tests are skipped with a notice when it is not set. Each test uses its own table and drops
//! it at the end.

mod common;

use orbit_orm::{Database, Direction, Error, Model, OrbitModel, Query, Value};

async fn connect() -> Option<Database> {
    let url = common::url()?;
    Some(Database::connect(&url).await.expect("connects"))
}

#[derive(Debug, PartialEq, OrbitModel)]
#[orbit(table = "orbit_derive_posts")]
struct Post {
    id: i64,
    #[orbit(fillable)]
    title: String,
    #[orbit(column = "view_count", fillable)]
    views: Option<i64>,
    #[orbit(skip)]
    cache: String,
}

#[derive(Debug, PartialEq, OrbitModel)]
#[orbit(table = "orbit_derive_tags", primary_key = "slug")]
struct Tag {
    #[orbit(fillable)]
    slug: String,
    #[orbit(fillable)]
    label: String,
}

async fn run_sql(db: &Database, sql: &str) {
    common::exec(db, sql).await;
}

async fn setup_posts(db: &Database) {
    run_sql(db, "DROP TABLE IF EXISTS orbit_derive_posts").await;
    run_sql(
        db,
        "CREATE TABLE orbit_derive_posts (
            id BIGINT AUTO_INCREMENT PRIMARY KEY,
            title VARCHAR(255) NOT NULL,
            view_count BIGINT NULL
        )",
    )
    .await;
}

async fn setup_tags(db: &Database) {
    run_sql(db, "DROP TABLE IF EXISTS orbit_derive_tags").await;
    run_sql(
        db,
        "CREATE TABLE orbit_derive_tags (
            slug VARCHAR(64) PRIMARY KEY,
            label VARCHAR(255) NOT NULL
        )",
    )
    .await;
}

#[test]
fn the_derive_fills_in_the_model_consts() {
    assert_eq!(Post::TABLE, "orbit_derive_posts");
    assert_eq!(Post::PRIMARY_KEY, "id");
    assert_eq!(Post::FILLABLE, &["title", "view_count"]);
    assert_eq!(Tag::PRIMARY_KEY, "slug");
    assert_eq!(Tag::FILLABLE, &["slug", "label"]);
}

#[tokio::test]
async fn find_uses_the_declared_primary_key() {
    let Some(db) = connect().await else { return };
    setup_tags(&db).await;
    run_sql(
        &db,
        "INSERT INTO orbit_derive_tags (slug, label) VALUES ('rust', 'Rust')",
    )
    .await;

    let tag = Tag::query().find(&db, "rust").await.expect("finds");
    assert_eq!(
        tag,
        Some(Tag {
            slug: "rust".into(),
            label: "Rust".into(),
        })
    );
    assert_eq!(Tag::query().find(&db, "go").await.expect("finds"), None);

    run_sql(&db, "DROP TABLE orbit_derive_tags").await;
    db.close().await;
}

#[tokio::test]
async fn posts_map_find_insert_and_update() {
    let Some(db) = connect().await else { return };
    setup_posts(&db).await;

    // find maps renamed and nullable columns
    run_sql(
            &db,
            "INSERT INTO orbit_derive_posts (id, title, view_count) VALUES (1, 'first', 10), (2, 'second', NULL)",
        )
        .await;

    let found = Post::query().find(&db, 1).await.expect("finds");
    assert_eq!(
        found,
        Some(Post {
            id: 1,
            title: "first".into(),
            views: Some(10),
            cache: String::new(),
        })
    );

    let nullable = Post::query().find_or_fail(&db, 2).await.expect("finds");
    assert_eq!(nullable.views, None);
    assert_eq!(Post::query().find(&db, 99).await.expect("finds"), None);

    setup_posts(&db).await;

    // insert and update write only fillable columns
    let id = Query::<Post>::table()
        .insert(
            &db,
            &[("title", "hello".into()), ("view_count", Value::from(3))],
        )
        .await
        .expect("inserts");
    assert_eq!(id, 1);
    let saved = Post::query().find_or_fail(&db, 1).await.expect("finds");
    assert_eq!(saved.views, Some(3));

    let changed = Post::query()
        .where_eq("id", 1)
        .update(
            &db,
            &[("title", "renamed".into()), ("view_count", Value::from(8))],
        )
        .await
        .expect("updates");
    assert_eq!(changed, 1);
    let updated = Post::query().find_or_fail(&db, 1).await.expect("finds");
    assert_eq!(updated.title, "renamed");
    assert_eq!(updated.views, Some(8));

    let by_order = Post::query()
        .order_by("id", Direction::Desc)
        .first_or_fail(&db)
        .await
        .expect("finds");
    assert_eq!(by_order.id, 1);

    setup_posts(&db).await;

    // a skipped field is not fillable
    let err = Post::query()
        .insert(&db, &[("cache", "nope".into())])
        .await
        .expect_err("cache is skipped");
    assert!(matches!(
        err,
        Error::NotFillable { table: "orbit_derive_posts", ref column } if column == "cache"
    ));

    let err = Post::query()
        .where_eq("id", 1)
        .update(&db, &[("id", Value::from(5))])
        .await
        .expect_err("id is not fillable");
    assert!(matches!(err, Error::NotFillable { .. }));

    run_sql(&db, "DROP TABLE orbit_derive_posts").await;
    db.close().await;
}
