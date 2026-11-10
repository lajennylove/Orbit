use orbit_macros::OrbitModel;

#[derive(OrbitModel)]
#[orbit(table = "posts")]
struct Post<T> {
    id: i64,
    value: T,
}

fn main() {}
