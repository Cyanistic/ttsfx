use crate::utils::{PartialOrDefault, WarnOnError};
use crate::{err, utils::Partial, Result};
use config::{Config as ConfigBuilder, Environment, File, FileFormat};
use fancy_regex::Regex;
use serde::{Deserialize, Serialize};
use serde_with::{serde_as, DisplayFromStr, VecSkipError};
use std::path::PathBuf;

/// Default config template embedded from disk.
const DEFAULT_CONFIG: &str = include_str!("../config.toml");

/// Compute linear gain from dB target: 10^(db/20).
pub fn volume_gain(db: f64) -> f32 {
    10.0_f64.powf(db / 20.0) as f32
}

/// A compiled pattern filter rule for detecting onomatopoeia in text.
#[serde_as]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatternFilter {
    /// Optional human-readable name for logging (e.g. "explosion").
    pub name: Option<String>,
    /// Higher = checked first (descending sort at startup).
    #[serde(default)]
    pub priority: u32,
    /// Compiled fancy-regex pattern (e.g. r"\bboom\b").
    #[serde_as(as = "DisplayFromStr")]
    pub regex: Regex,
}

/// A compiled pattern filter rule for detecting onomatopoeia in text.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigPatternFilter {
    #[serde(flatten)]
    pub filter: PatternFilter,
    /// Per-pattern dB override. None = use global default from Config.
    #[serde(default)]
    pub overrides: Partial<PatternConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatternConfig {
    /// TTS backend URL (e.g. "https://api.openai.com/v1").
    pub tts_base_url: String,

    /// ElevenLabs SFX API base URL (e.g. "https://api.elevenlabs.io").
    pub sfx_base_url: String,

    /// ElevenLabs API key (from env var or config file).
    pub sfx_api_key: String,

    /// dB target for all segments before concat (default -16.0).
    pub volume_target_db: f64,

    /// Audio files + .json sidecar metadata (default "sounds/").
    pub cache_dir: PathBuf,

    pub match_mode: MatchMode,

    /// ElevenLabs prompt influence (0.0-1.0, default 0.3).
    /// Lower values give the model more creative freedom; higher values stick closer to the text prompt.
    pub sfx_prompt_influence: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "mode")]
pub enum MatchMode {
    Levenshtein(LevenshteinMode),
    Embeddings(EmbeddingMode),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LevenshteinMode {
    pub threshold: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingMode {
    pub threshold: f32,
}

impl Default for PatternConfig {
    fn default() -> Self {
        Self {
            tts_base_url: "https://api.openai.com/v1".into(),
            sfx_base_url: "https://api.elevenlabs.io".into(),
            sfx_api_key: String::new(),
            volume_target_db: -16.0,
            cache_dir: PathBuf::from("sounds"),
            match_mode: MatchMode::Levenshtein(LevenshteinMode { threshold: 1 }),
            sfx_prompt_influence: 0.3,
        }
    }
}

/// Application configuration. Loaded from defaults + TOML file via config crate builder pattern.
#[serde_as]
#[derive(Debug, Serialize, Deserialize)]
pub struct Config {
    #[serde_as(deserialize_as = "PartialOrDefault<_>")]
    pub overridable: PatternConfig,

    /// Output sample rate (default 48000).
    pub sample_rate: u32,

    /// Compiled filters from defaults + user overrides.
    #[serde_as(as = "VecSkipError<_, WarnOnError>")]
    pub patterns: Vec<ConfigPatternFilter>,
}

impl Config {
    /// Load configuration using layered defaults + TOML file.
    pub fn load() -> Result<Self> {
        // Write default config on first run if no config.toml exists.
        let path = std::path::Path::new("config.toml");
        if !path.exists() {
            std::fs::write(path, DEFAULT_CONFIG)
                .map_err(|e| err!(Io, "failed to write default config: {}", e))?;
        }

        let settings = ConfigBuilder::builder()
            .add_source(Environment::with_prefix("TTSFX"))
            .add_source(
                File::with_name("config")
                    .format(FileFormat::Toml)
                    .required(false),
            )
            .build()
            .map_err(|e| err!(Configuration, "failed to build config: {}", e))?;

        let raw: Config = settings
            .try_deserialize()
            .map_err(|e| err!(Configuration, "failed to deserialize config: {}", e))?;

        Self::from_raw(raw)
    }

    fn from_raw(mut raw: Config) -> Result<Self> {
        raw.patterns
            .sort_by(|a, b| b.filter.priority.cmp(&a.filter.priority));
        Ok(raw)
    }
}
