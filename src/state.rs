use crate::cache::CacheIndex;
use crate::config::Config;
use crate::resolver::SoundResolver;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use strum::EnumDiscriminants;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub cache: CacheIndex,
    pub resolver: Arc<SoundResolver>,
    /// Shared upstream HTTP client (TTS + timeouts).
    pub http: reqwest::Client,
}

impl AppState {
    pub fn new(config: Arc<Config>, cache: CacheIndex, resolver: Arc<SoundResolver>) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .expect("failed to build HTTP client");
        Self {
            config,
            cache,
            resolver,
            http,
        }
    }
}

/// One span of the input after pattern detection, in narrative order.
#[derive(Debug, Clone, EnumDiscriminants)]
#[strum_discriminants(name(FragmentKind))]
pub enum Fragment {
    Tts {
        text: String,
    },
    Sfx {
        pattern_index: usize,
        text: String,
        start: usize,
        end: usize,
    },
}

/// Resolved audio for one fragment, ready to decode/mix.
#[derive(Debug)]
pub enum ResolvedAudio {
    Bytes(Vec<u8>),
    File(PathBuf),
}