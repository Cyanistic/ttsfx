//! Global tracing subscriber (`init_tracing`, `TTSFX_LOG`, optional span-tree formatting).

use crate::log_fmt::SpanTreeFormat;
use crate::span_transform::SpanTransformLayer;
use crate::{err, Result};
use tracing_subscriber::{filter::LevelFilter, prelude::*, EnvFilter};

/// Install the global tracing subscriber.
///
/// Filter precedence:
/// - Default ceiling: **WARN** for everything
/// - **`RUST_LOG`**: standard `tracing_subscriber` directives (merged via `from_env_lossy`)
/// - **`TTSFX_LOG`**: level for **`ttsfx` only** (default **`debug`**). `RUST_LOG=trace` does **not**
///   raise `ttsfx` unless you also set `TTSFX_LOG=trace` (or `RUST_LOG=ttsfx=trace`).
/// - Extra `directives` from the binary (e.g. `["hyper=warn"]`)
///
/// Formatting:
/// - **`TTSFX_LOG_TREE`** (default **true**): message-first lines with dimmed `request > handle_speech` span trail (`log_fmt::SpanTreeFormat`)
/// - **`TTSFX_LOG_TREE=0`**: plain `fmt` with file and line (`TTSFX_LOG_FILE=0` disables file/line there too)
pub fn init_tracing(directives: &[&str]) -> Result<()> {
    let ttsfx_level = std::env::var("TTSFX_LOG").unwrap_or_else(|_| "debug".to_string());

    let internal_crates = ["ttsfx"];
    let external_overrides = [
        "hyper=warn",
        "tower=warn",
        "h2=warn",
        "reqwest=warn",
        "notify=warn",
    ];

    let mut filter = EnvFilter::builder()
        .with_default_directive(LevelFilter::WARN.into())
        .from_env_lossy();

    for directive in internal_crates
        .iter()
        .map(|c| format!("{c}={ttsfx_level}"))
        .chain(external_overrides.iter().map(|s| s.to_string()))
        .chain(directives.iter().map(|s| s.to_string()))
    {
        filter = filter.add_directive(directive.parse().map_err(
            |e| err!(Configuration, "invalid tracing directive `{directive}`", @external: e),
        )?);
    }

    let use_span_tree_fmt = env_is_truthy("TTSFX_LOG_TREE", true);
    let transform = SpanTransformLayer::new();

    let fmt_layer = if use_span_tree_fmt {
        tracing_subscriber::fmt::layer()
            .event_format(SpanTreeFormat)
            .boxed()
    } else {
        let show_file = std::env::var("TTSFX_LOG_FILE")
            .map(|v| !matches!(v.as_str(), "0" | "false" | "no"))
            .unwrap_or(true);
        tracing_subscriber::fmt::layer()
            .with_line_number(show_file)
            .with_file(show_file)
            .boxed()
    };

    tracing_subscriber::registry()
        .with(tracing_error::ErrorLayer::default())
        .with(filter)
        .with(transform)
        .with(fmt_layer)
        .try_init()
        .map_err(|e| err!(Internal, "tracing subscriber already initialized", @external: e))?;

    Ok(())
}

fn env_is_truthy(var: &str, default: bool) -> bool {
    std::env::var(var)
        .map(|v| !v.is_empty() && v != "0" && v.to_lowercase() != "false")
        .unwrap_or(default)
}
