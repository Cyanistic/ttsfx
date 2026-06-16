use crate::config::Config;
use crate::fs_watch::{event_touches_path, spawn_debounced_notify};
use crate::{err, Result};
use notify::RecursiveMode;
use std::env;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::watch;
use tracing::{Instrument, debug, info, info_span, warn};

/// Live config: readers take `snapshot()`; TOML reloads on save (see [`crate::fs_watch`]).
#[derive(Clone)]
pub struct ConfigHandle {
    rx: watch::Receiver<Arc<Config>>,
}

impl ConfigHandle {
    pub async fn load_and_watch(path: PathBuf) -> Result<Self> {
        let cwd = env::current_dir()
            .map_err(|e| err!(Io, "failed to get current directory for config watch", @source: e))?;
        let config_abs = if path.is_absolute() {
            path.canonicalize()
                .map_err(|e| err!(Io, "failed to resolve config path {}", path.display(), @source: e))?
        } else {
            cwd.join(&path).canonicalize().map_err(|e| {
                err!(
                    Io,
                    "failed to resolve config path {}",
                    cwd.join(&path).display(),
                    @source: e
                )
            })?
        };

        let initial = Arc::new(Config::load_from_path(&config_abs)?);
        let (tx, rx) = watch::channel(initial);

        let watch_dir = config_abs
            .parent()
            .map(PathBuf::from)
            .unwrap_or(cwd);

        debug!(
            path = %config_abs.display(),
            watch_dir = %watch_dir.display(),
            "config filesystem watch started"
        );

        spawn_debounced_notify(watch_dir, RecursiveMode::NonRecursive, move |events| {
            let config_path = config_abs.clone();
            let path_for_span = config_path.display().to_string();
            let tx = tx.clone();
            async move {
                if !events.iter().any(|e| event_touches_path(e, &config_path)) {
                    return;
                }
                match Config::load_from_path(&config_path) {
                    Ok(cfg) => {
                        info!(
                            patterns = cfg.patterns.len(),
                            path = %config_path.display(),
                            "config reloaded"
                        );
                        if tx.send(Arc::new(cfg)).is_err() {
                            debug!("config watch receiver dropped");
                        }
                    }
                    Err(e) => {
                        warn!(
                            error = %e,
                            path = %config_path.display(),
                            "config reload failed; keeping previous config"
                        );
                    }
                }
            }
            .instrument(info_span!("config_reload", path = %path_for_span))
        });

        Ok(Self { rx })
    }

    pub fn snapshot(&self) -> Arc<Config> {
        self.rx.borrow().clone()
    }
}