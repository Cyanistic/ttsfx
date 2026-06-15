use std::net::SocketAddr;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use clap::{Parser, Subcommand};

/// OpenAI-compatible speech proxy with onomatopoeia → SFX splice.
#[derive(Debug, Parser)]
#[command(name = "ttsfx", version, about)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,

    /// Socket address to bind when running the HTTP server.
    #[arg(
        long,
        short = 'L',
        default_value = "0.0.0.0:3000",
        env = "TTSFX_LISTEN",
        global = true
    )]
    pub listen: SocketAddr,

    /// TOML config path (default `config.toml`).
    #[arg(long, global = true, env = "TTSFX_CONFIG", default_value = "config.toml")]
    pub config: PathBuf,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Run the HTTP speech proxy (default when no subcommand is given).
    Run,
    /// Re-embed cached SFX metadata from stored `recipe` strings.
    ReindexEmbeddings(ReindexEmbeddingsArgs),
}

/// Arguments for `ttsfx reindex-embeddings` (parsed by clap).
#[derive(Debug, Clone, Parser)]
pub struct ReindexEmbeddingsArgs {
    /// Only reindex entries with `embedded_at` strictly before this RFC3339 or YYYY-MM-DD time.
    #[arg(long)]
    pub before: Option<DateTime<Utc>>,
    /// Only reindex entries with `embedded_at` at or after this RFC3339 or YYYY-MM-DD time.
    #[arg(long)]
    pub after: Option<DateTime<Utc>>,
    /// Reindex every cache file with a recipe (or matched_text when recipe empty).
    #[arg(long)]
    pub force: bool,
    /// List candidates without calling the embeddings API.
    #[arg(long)]
    pub dry_run: bool,
}

impl Cli {
    pub fn parse_args() -> Self {
        Self::parse()
    }
}
