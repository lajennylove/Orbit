use orbit_macros::OrbitModel;

#[derive(OrbitModel)]
#[orbit(table = "posts")]
struct Post {
    id: i64,
    #[orbit(colum = "title")]
    title: String,
}

fn main() {}
