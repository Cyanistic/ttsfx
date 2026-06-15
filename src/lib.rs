#![allow(clippy::needless_question_mark)]

pub mod audio;
pub mod cache;
pub mod config;
pub mod error;
pub mod handler;
pub mod pattern;
pub mod resolver;
pub mod state;
pub mod utils;

use std::sync::Arc;

use axum::{Router, routing::post};
use axum_reverse_proxy::ReverseProxy;
use tokio::net::TcpListener;
use tracing::info;

pub use config::volume_gain;
pub use error::Result;

/// Load config, build the router, and serve until shutdown.
pub async fn run() -> Result<()> {
    audio::check_ffmpeg().await?;

    let config = Arc::new(config::Config::load()?);
    info!(patterns = config.patterns.len(), "config loaded");

    let cache = cache::CacheIndex::new(config.overridable.cache_dir.clone()).await;
    let resolver = Arc::new(resolver::SoundResolver::new(config.clone(), cache.clone()));

    let state = state::AppState {
        config: config.clone(),
        cache,
        resolver,
    };

    let upstream = config
        .overridable
        .tts_base_url
        .trim_end_matches('/')
        .to_string();
    let proxy = ReverseProxy::new("/", upstream.as_str());

    let app = Router::new()
        .route("/v1/audio/speech", post(handler::handle_speech))
        .fallback_service(proxy)
        .with_state(state);

    let listener = TcpListener::bind("0.0.0.0:3000").await?;
    info!(addr = %listener.local_addr()?, "listening");
    axum::serve(listener, app).await?;

    Ok(())
}
