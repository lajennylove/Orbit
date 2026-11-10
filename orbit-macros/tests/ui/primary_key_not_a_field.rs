use orbit_macros::OrbitModel;

#[derive(OrbitModel)]
#[orbit(table = "posts", primary_key = "uuid")]
struct Post {
    id: i64,
}

fn main() {}
