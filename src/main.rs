extern crate futures_util;
extern crate tokio;
#[macro_use]
extern crate log;
extern crate anyhow;
extern crate ctrlc;
extern crate deadqueue;
extern crate dotenv;
extern crate pretty_env_logger;
extern crate tokio_tungstenite;
extern crate tungstenite;

mod net;
mod server;
mod util;

use anyhow::Result;

fn main() -> Result<()> {
    dotenv::dotenv().unwrap();
    pretty_env_logger::init();
    util::Control::init()?;

    server::Server::new("127.0.0.1:9002").start()?;

    Ok(())
}
