use crate::config::{MatchMode, PatternConfig, PatternFilter};
use crate::error::ResultExt;
use crate::{Result, err, bail};
use notify::{Event, EventKind, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::process::Command;
use tokio::sync::{mpsc, watch};
use tracing::{debug, info, warn};

const AUDIO_EXTENSIONS: &[&str] = &["wav", "mp3", "ogg", "flac"];

/// Metadata embedded into audio files via ffmpeg tags.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheMeta {
    pub matched_text: String,
    /// Surrounding text context for future embeddings-based matching.
    pub context: Option<String>,
    #[serde(default)]
    pub version: String,
}

/// A cached audio entry: metadata + the audio file path.
#[derive(Debug, Clone)]
pub struct CacheEntry {
    pub meta: CacheMeta,
    pub filepath: PathBuf,
}

/// In-memory cache index backed by a watch channel.
/// One writer (notify watcher task), many readers (handler tasks).
#[derive(Clone)]
pub struct CacheIndex {
    rx: watch::Receiver<Vec<CacheEntry>>,
}

impl CacheIndex {
    /// Create a new CacheIndex. Loads initial entries and starts the filesystem watcher.
    pub async fn new(cache_dir: PathBuf) -> Self {
        let (tx, rx) = watch::channel(Vec::new());

        let initial = Self::load_from_dir(&cache_dir).await;
        info!(count = initial.len(), "loaded cache entries");
        let _ = tx.send(initial);

        tokio::spawn(Self::watch_loop(cache_dir, tx));

        Self { rx }
    }

    /// Find the best matching cache entry for a pattern + text.
    /// Returns the closest match within the distance threshold.
    pub fn find_match(
        &self,
        // Reserved for future per-pattern dispatch (e.g. embeddings vector search,
        // pattern-specific similarity strategies). Currently unused by Levenshtein mode.
        _filter: &PatternFilter,
        base: &PatternConfig,
        text: &str,
    ) -> Option<CacheEntry> {
        let threshold = match &base.match_mode {
            MatchMode::Levenshtein(m) => m.threshold,
            MatchMode::Embeddings(_) => unreachable!("validated at config load"),
        };

        let entries = self.rx.borrow();
        let text_lower = text.to_lowercase();
        entries
            .iter()
            .map(|e| {
                let dist = strsim::levenshtein(&text_lower, &e.meta.matched_text.to_lowercase());
                (dist, e)
            })
            .filter(|(dist, _)| *dist <= threshold)
            .min_by_key(|(dist, _)| *dist)
            .map(|(_, e)| e.clone())
    }

    /// Number of cached entries.
    pub fn count(&self) -> usize {
        self.rx.borrow().len()
    }

    /// Load a single cache entry by reading metadata from the audio file via ffmpeg.
    async fn load_entry(audio_path: &Path) -> Result<CacheEntry> {
        let meta = read_metadata(audio_path).await?;
        Ok(CacheEntry {
            meta,
            filepath: audio_path.to_path_buf(),
        })
    }

    /// Load all cache entries from audio files in the directory.
    async fn load_from_dir(dir: &Path) -> Vec<CacheEntry> {
        let paths: Vec<PathBuf> = match std::fs::read_dir(dir) {
            Ok(d) => d
                .flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.extension()
                        .and_then(|e| e.to_str())
                        .is_some_and(|ext| AUDIO_EXTENSIONS.contains(&ext))
                })
                .collect(),
            Err(_) => return Vec::new(),
        };

        let mut entries = Vec::new();
        for p in paths {
            if let Ok(entry) = Self::load_entry(&p).await.warn() {
                entries.push(entry);
            }
        }
        entries
    }

    /// Async watch loop: receives filesystem events, debounces, applies targeted updates.
    async fn watch_loop(cache_dir: PathBuf, tx: watch::Sender<Vec<CacheEntry>>) {
        let (event_tx, mut event_rx) = mpsc::channel::<Event>(64);

        let mut watcher = match notify::recommended_watcher(move |res: notify::Result<Event>| {
            if let Ok(event) = res.warn() {
                event_tx.blocking_send(event).warn().ok();
            }
        }) {
            Ok(w) => w,
            Err(e) => {
                warn!(error = %e, "failed to create cache watcher");
                return;
            }
        };

        if let Err(e) = watcher.watch(&cache_dir, RecursiveMode::Recursive) {
            warn!(error = %e, "failed to watch cache directory");
            return;
        }

        while let Some(first) = event_rx.recv().await {
            tokio::time::sleep(Duration::from_millis(250)).await;
            let mut events = vec![first];
            while let Ok(event) = event_rx.try_recv() {
                events.push(event);
            }

            let current: HashMap<PathBuf, CacheEntry> = tx
                .borrow()
                .iter()
                .cloned()
                .map(|e| (e.filepath.clone(), e))
                .collect();
            let mut map = current;

            for event in &events {
                match &event.kind {
                    EventKind::Create(_) => {
                        for path in &event.paths {
                            let is_audio = path
                                .extension()
                                .and_then(|e| e.to_str())
                                .is_some_and(|ext| AUDIO_EXTENSIONS.contains(&ext));
                            if !is_audio {
                                continue;
                            }
                            if let Ok(entry) = Self::load_entry(path).await.warn() {
                                map.insert(path.clone(), entry);
                            }
                        }
                    }
                    EventKind::Modify(_) => {
                        for path in &event.paths {
                            let is_audio = path
                                .extension()
                                .and_then(|e| e.to_str())
                                .is_some_and(|ext| AUDIO_EXTENSIONS.contains(&ext));
                            if !is_audio {
                                continue;
                            }
                            // On modify, failure is likely transient (e.g. file locked
                            // by ffmpeg during metadata write). Skip the update — keep
                            // the old valid entry instead of evicting it.
                            if let Ok(entry) = Self::load_entry(path).await.warn() {
                                map.insert(path.clone(), entry);
                            }
                        }
                    }
                    EventKind::Remove(_) => {
                        for path in &event.paths {
                            map.remove(path);
                        }
                    }
                    _ => {
                        continue;
                    }
                }
            }

            let entries: Vec<CacheEntry> = map.into_values().collect();
            info!(count = entries.len(), "cache index updated");
            if tx.send(entries).is_err() {
                debug!("cache index receiver dropped");
            }
        }

        drop(watcher);
    }
}

/// Stamp metadata into an audio file via ffmpeg. Writes to a temp file, then renames.
pub async fn write_metadata(file_path: &Path, meta: &CacheMeta) -> Result<()> {
    let json_payload = serde_json::to_string(meta)
        .map_err(|e| err!(Serialization, "failed to serialize cache metadata", @external: e))?;
    let mut temp_file_name = OsString::new();
    temp_file_name.push(file_path.file_stem().unwrap_or_default());
    temp_file_name.push(".tmp.");
    temp_file_name.push(file_path.extension().unwrap_or_default());
    let temp_path = file_path.with_file_name(temp_file_name);

    let metadata_arg = format!("ttsfx_metadata={json_payload}");

    let status = Command::new("ffmpeg")
        .arg("-y")
        .arg("-i")
        .arg(file_path)
        .arg("-metadata")
        .arg(&metadata_arg)
        .arg("-c:a")
        .arg("copy")
        .arg(&temp_path)
        .status()
        .await
        .map_err(|e| err!(Internal, "failed to spawn ffmpeg for metadata write", @external: e))?;

    if status.success() {
        std::fs::rename(&temp_path, file_path).map_err(|e| {
            std::fs::remove_file(&temp_path).ok();
            err!(
                Io,
                "failed to rename temp file: {}",
                temp_path.display(),
                @external: e
            )
        })?;
        Ok(())
    } else {
        let _ = std::fs::remove_file(&temp_path);
        Err(err!(Internal, "ffmpeg metadata injection failed"))
    }
}

/// Read metadata stamped into an audio file via ffprobe.
async fn read_metadata(file_path: &Path) -> Result<CacheMeta> {
    let output = Command::new("ffprobe")
        .arg("-v")
        .arg("quiet")
        .arg("-print_format")
        .arg("json")
        .arg("-show_format")
        .arg(file_path)
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            Io,
            "ffprobe failed for '{}': {}",
            file_path.to_string_lossy(),
            stderr
        );
    }

    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let raw_json = parsed
        .get("format")
        .and_then(|f| f.get("tags"))
        .and_then(|f| f.get("ttsfx_metadata"))
        .and_then(|t| t.as_str())
        .ok_or_else(|| {
            err!(
                Validation,
                "could not find ttsfx metadata in tags for '{}'",
                file_path.to_string_lossy()
            )
        })?;
    Ok(serde_json::from_str(raw_json)?)
}
