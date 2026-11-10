use orbit_macros::OrbitModel;

#[derive(OrbitModel)]
#[orbit(table = "posts")]
struct Post(i64, String);

fn main() {}
