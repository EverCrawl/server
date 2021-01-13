mod db;
mod net;
mod server;
mod util;

use anyhow::Result;

fn main() -> Result<()> {
    dotenv::dotenv().unwrap();
    pretty_env_logger::init();
    util::Control::init()?;

    let addr = std::env::var("SERVER_ADDRESS").unwrap_or_else(|_| "127.0.0.1:9002".into());
    server::Server::new(&addr).start()?;

    Ok(())
}
