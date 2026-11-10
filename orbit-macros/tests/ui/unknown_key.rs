use orbit_macros::OrbitModel;

#[derive(OrbitModel)]
#[orbit(table = "posts", tabel = "posts")]
struct Post {
    id: i64,
}

fn main() {}
