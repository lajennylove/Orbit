use orbit_macros::OrbitModel;

#[derive(OrbitModel)]
#[orbit(table = "posts")]
enum Post {
    Draft,
    Published,
}

fn main() {}
