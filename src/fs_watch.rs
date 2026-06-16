//! Shared notify + debounce loop. Domain code only handles batched [`Event`]s.

use crate::error::ResultExt;
use notify::{Event, EventKind, RecursiveMode, Watcher};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{Instrument, info_span, warn};

const DEBOUNCE_MS: u64 = 250;

/// True if this event is create/modify/remove and lists `path` (canonical compare when possible).
pub fn event_touches_path(event: &Event, path: &Path) -> bool {
    match &event.kind {
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) => {}
        _ => return false,
    }
    let path_canon = path.canonicalize().ok();
    event.paths.iter().any(|p| {
        if p.as_path() == path {
            return true;
        }
        match (&path_canon, p.canonicalize().ok()) {
            (Some(a), Some(b)) => a == &b,
            _ => false,
        }
    })
}

/// Watch `watch_dir`, debounce bursts, then call `on_batch` for each batch.
pub fn spawn_debounced_notify<F, Fut>(watch_dir: PathBuf, recursive: RecursiveMode, on_batch: F)
where
    F: Fn(Vec<Event>) -> Fut + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    let dir_for_span = watch_dir.display().to_string();
    tokio::spawn(
        async move {
            let (event_tx, mut event_rx) = mpsc::channel::<Event>(64);

            let mut watcher = match notify::recommended_watcher(move |res: notify::Result<Event>| {
                if let Ok(event) = res.warn() {
                    event_tx.blocking_send(event).warn().ok();
                }
            }) {
                Ok(w) => w,
                Err(e) => {
                    warn!(error = %e, "failed to create filesystem watcher");
                    return;
                }
            };

            if let Err(e) = watcher.watch(&watch_dir, recursive) {
                warn!(error = %e, dir = %watch_dir.display(), "failed to watch directory");
                return;
            }

            while let Some(first) = event_rx.recv().await {
                tokio::time::sleep(Duration::from_millis(DEBOUNCE_MS)).await;
                let mut events = vec![first];
                while let Ok(event) = event_rx.try_recv() {
                    events.push(event);
                }
                on_batch(events).await;
            }

            drop(watcher);
        }
        .instrument(info_span!("debounced_fs_watch", dir = %dir_for_span)),
    );
}