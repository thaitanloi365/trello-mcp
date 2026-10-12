mod config;
mod trello;
mod util;
mod version;

fn main() {
    println!("{}", version::CURRENT);
}
