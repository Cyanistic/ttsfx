#![allow(clippy::needless_question_mark)]

pub mod audio;
pub mod cache;
pub mod config;
pub mod error;
pub mod handler;
pub mod log_fmt;
pub mod middleware;
pub mod pattern;
pub mod span_transform;
pub mod resolver;
pub mod state;
pub mod tracing_init;
pub mod utils;

use std::sync::Arc;

use axum::middleware::from_fn;
use axum::{Router, routing::post};
use axum_reverse_proxy::ReverseProxy;
use tokio::net::TcpListener;
use tower::ServiceBuilder;
use tower_http::trace::{DefaultOnFailure, DefaultOnRequest, TraceLayer};
use tracing::{Level, info};

pub use config::volume_gain;
pub use error::Result;
pub use tracing_init::init_tracing;

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

    let trace = ServiceBuilder::new().layer(
        TraceLayer::new_for_http()
            .make_span_with(middleware::RequestSpan)
            .on_request(DefaultOnRequest::new().level(Level::DEBUG))
            .on_response(middleware::StatusLevelOnResponse)
            .on_failure(DefaultOnFailure::new().level(Level::WARN)),
    );

    let app = Router::new()
        .route("/v1/audio/speech", post(handler::handle_speech))
        .fallback_service(proxy)
        .layer(trace)
        .layer(from_fn(middleware::request_id_middleware))
        .with_state(state);

    let listener = TcpListener::bind("0.0.0.0:3000").await?;
    info!(addr = %listener.local_addr()?, "listening");
    axum::serve(listener, app).await?;

    Ok(())
}
