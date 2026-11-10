//! The default table name that `OrbitModel` derives when `table` is not given. These tests need
//! no database. The models are only read for their consts, so their fields are never read.
#![allow(dead_code)]

use orbit_orm::{Model, OrbitModel};

#[derive(OrbitModel)]
struct Flight {
    id: i64,
}

#[derive(OrbitModel)]
struct UserProfile {
    id: i64,
}

#[derive(OrbitModel)]
struct Category {
    id: i64,
}

#[derive(OrbitModel)]
struct Address {
    id: i64,
}

#[derive(OrbitModel)]
struct Person {
    id: i64,
}

#[derive(OrbitModel)]
struct Child {
    id: i64,
}

#[derive(OrbitModel)]
struct Box {
    id: i64,
}

#[derive(OrbitModel)]
struct Status {
    id: i64,
}

#[derive(OrbitModel)]
#[orbit(table = "legacy_widgets")]
struct Widget {
    id: i64,
}

#[derive(OrbitModel)]
#[orbit(table = "orbit_already_plural")]
struct Gadget {
    id: i64,
}

#[test]
fn the_default_table_is_the_plural_of_the_struct_name() {
    assert_eq!(Flight::TABLE, "flights");
    assert_eq!(UserProfile::TABLE, "user_profiles");
    assert_eq!(Category::TABLE, "categories");
    assert_eq!(Address::TABLE, "addresses");
    assert_eq!(Person::TABLE, "people");
    assert_eq!(Child::TABLE, "children");
    assert_eq!(Box::TABLE, "boxes");
    assert_eq!(Status::TABLE, "statuses");
}

#[test]
fn an_explicit_table_overrides_the_default() {
    assert_eq!(Widget::TABLE, "legacy_widgets");
    assert_eq!(Gadget::TABLE, "orbit_already_plural");
}
