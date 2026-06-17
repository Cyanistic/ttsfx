use std::net::SocketAddr;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use clap::{Parser, Subcommand};

const AFTER_LONG_HELP: &str = r#"Examples:
  ttsfx
  ttsfx -L 127.0.0.1:8787 --config config.toml
  curl -X POST http://127.0.0.1:8787/v1/audio/speech -H 'Content-Type: application/json' \
    -d '{"model":"kokoro","voice":"af_heart","input":"BOOM","response_format":"wav"}'
  ttsfx reindex --dry-run
  ttsfx reindex --force

You'll need ffmpeg, a filled-in config.toml, and ELEVENLABS_API_KEY when cache misses should generate new SFX.

Config and logging: config.toml plus TTSFX_OVERRIDABLE__* env vars; TTSFX_LOG / RUST_LOG / TTSFX_LOG_TREE — README.md.
"#;

/// Speech proxy that turns onomatopoeia in TTS input into real sound effects.
#[derive(Debug, Parser)]
#[command(
    name = "ttsfx",
    version,
    about = "Speech proxy: comic words in your text become sound effects in the WAV",
    long_about = "Handles POST /v1/audio/speech like your normal TTS endpoint. Words that match patterns in config.toml get replaced with cached or ElevenLabs-generated SFX; everything else is synthesized through your configured TTS backend. Needs ffmpeg on PATH and a valid config.toml.",
    after_long_help = AFTER_LONG_HELP
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,

    /// Socket address to bind when running the HTTP server.
    #[arg(
        long,
        short = 'L',
        default_value = "0.0.0.0:8787",
        env = "TTSFX_LISTEN",
        global = true
    )]
    pub listen: SocketAddr,

    /// TOML config path (default `config.toml`).
    #[arg(
        long,
        global = true,
        env = "TTSFX_CONFIG",
        default_value = "config.toml"
    )]
    pub config: PathBuf,
}

#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Run the HTTP server (this is the default).
    Run,
    /// Refresh embedding vectors for cached SFX recipes (after you change embed_* settings).
    #[command(name = "reindex")]
    ReindexEmbeddings(ReindexEmbeddingsArgs),
}

/// Arguments for `ttsfx reindex` (parsed by clap).
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
