use crate::cache::CacheIndex;
use crate::config::Config;
use crate::resolver::SoundResolver;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub cache: CacheIndex,
    pub resolver: Arc<SoundResolver>,
}

/// One span of the input after pattern detection, in narrative order.
#[derive(Debug, Clone)]
pub enum Fragment {
    Tts { text: String },
    Sfx { pattern_index: usize, text: String },
}

/// Resolved audio for one fragment, ready to decode/mix.
#[derive(Debug)]
pub enum ResolvedAudio {
    Bytes(Vec<u8>),
    File(PathBuf),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FragmentKind {
    Tts,
    Sfx,
}
