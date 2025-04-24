mod types;
mod wallet;
mod transactions;
mod server;

use std::fs;
use std::io;

fn main() -> io::Result<()> {
    server::init_tracing();

    // Load configuration
    let config = match fs::read_to_string("ark.config.toml") {
        Ok(content) => match toml::from_str::<types::Config>(&content) {
            Ok(config) => config,
            Err(e) => {
                eprintln!("Failed to parse config: {}", e);
                return Err(io::Error::new(io::ErrorKind::Other, "Config parse error"));
            }
        },
        Err(e) => {
            eprintln!("Failed to read config file: {}", e);
            return Err(io::Error::new(io::ErrorKind::Other, "Config read error"));
        }
    };

    // Start the server using tokio runtime
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            server::start_server(config).await
        })
}
