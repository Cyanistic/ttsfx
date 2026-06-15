use std::net::SocketAddr;

use clap::Parser;

/// OpenAI-compatible speech proxy with onomatopoeia → SFX splice.
#[derive(Debug, Parser)]
#[command(name = "ttsfx", version, about)]
pub struct Cli {
    /// Socket address to bind (e.g. `0.0.0.0:3000`, `127.0.0.1:3000`).
    #[arg(long, short = 'L', default_value = "0.0.0.0:3000", env = "TTSFX_LISTEN")]
    pub listen: SocketAddr,
}

impl Cli {
    pub fn parse_args() -> Self {
        Self::parse()
    }
}