mod clientconfig;
mod config;
mod mcp;
mod trello;
mod util;
mod version;

fn main() {
    println!("{}", version::CURRENT);
}
