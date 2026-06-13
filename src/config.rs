use crate::{err, Result};
use config::{Config as ConfigBuilder, Environment, File, FileFormat};
use fancy_regex::Regex;
use serde::Deserialize;
use std::{borrow::Cow, path::PathBuf};

/// Default config template embedded from disk.
const DEFAULT_CONFIG: &str = include_str!("../config.toml");

/// Compute linear gain from dB target: 10^(db/20).
pub fn volume_gain(db: f64) -> f32 {
    (10.0_f64.powf(db / 20.0)) as f32
}

/// A compiled pattern filter rule for detecting onomatopoeia in text.
#[derive(Debug, Clone)]
pub struct PatternFilter {
    /// Unique identifier for this rule (e.g. "boom").
    pub id: String,
    /// Higher = checked first (descending sort at startup).
    pub priority: u32,
    /// Compiled fancy-regex pattern (e.g. r"\bboom\b").
    pub regex: Regex,
    /// Per-pattern dB override. None = use global default from Config.
    pub volume_override_db: Option<f64>,
}

impl PatternFilter {
    /// Returns (start_offset, end_offset) of the first match. None if no match.
    pub fn matches(&self, text: &str) -> Option<(usize, usize)> {
        self.regex
            .find(text)
            .ok()
            .flatten()
            .map(|m| (m.start(), m.end()))
    }

    /// Returns the matched substring (consumed segment). None if no match.
    pub fn consume<'text>(&self, text: &'text str) -> Option<&'text str> {
        self.regex.find(text).ok().flatten().map(|m| m.as_str())
    }

    /// Returns the rest of text after consuming a match. Empty if no match was found.
    pub fn remaining<'text>(&self, text: &'text str) -> Cow<'text, str> {
        if let Some((start, end)) = self.matches(text) {
            format!("{}{}", &text[..start], &text[end..]).into()
        } else {
            text.into()
        }
    }
}

/// Raw TOML representation of a pattern (regex is a string, compiled later).
#[derive(Debug, Deserialize)]
struct RawPattern {
    id: String,

    #[serde(default = "default_priority")]
    priority: u32,

    regex: String,

    #[serde(default)]
    volume_override_db: Option<f64>,
}

/// Raw TOML config (deserializes from file/env before compilation).
#[derive(Debug, Default, Deserialize)]
struct RawConfig {
    #[serde(default = "default_tts_base_url")]
    tts_base_url: String,

    #[serde(default)]
    sfx_api_key: Option<String>,

    #[serde(default = "default_volume_target_db")]
    volume_target_db: f64,

    #[serde(default = "default_sample_rate")]
    sample_rate: u32,

    #[serde(default)]
    cache_dir: Option<PathBuf>,

    patterns: Option<Vec<RawPattern>>,
}

/// Application configuration. Loaded from defaults + TOML file via config crate builder pattern.
#[derive(Debug)]
pub struct Config {
    /// TTS backend URL (e.g. "https://api.openai.com/v1").
    pub tts_base_url: String,

    /// ElevenLabs API key (from env var or config file).
    pub sfx_api_key: String,

    /// dB target for all segments before concat (default -16.0).
    pub volume_target_db: f64,

    /// Output sample rate (default 48000).
    pub sample_rate: u32,

    /// Audio files + .json sidecar metadata (default "sounds/").
    pub cache_dir: PathBuf,

    /// Compiled filters from defaults + user overrides.
    pub patterns: Vec<PatternFilter>,
}

impl Default for Config {
    fn default() -> Self {
        // Can't call from_raw here (it returns Result), so we use a bare-bones default.
        Self {
            tts_base_url: "https://api.openai.com/v1".to_string(),
            sfx_api_key: String::new(),
            volume_target_db: -16.0,
            sample_rate: 48_000,
            cache_dir: PathBuf::from("sounds"),
            patterns: Vec::new(), // no-op; callers should use Config::load() for real defaults
        }
    }
}

impl Config {
    /// Load configuration using layered defaults + TOML file.
    #[track_caller]
    pub fn load() -> Result<Self> {
        // Write default config on first run if no config.toml exists.
        let path = std::path::Path::new("config.toml");
        if !path.exists() {
            std::fs::write(path, DEFAULT_CONFIG)
                .map_err(|e| err!(Io, "failed to write default config: {}", e))?;
        }

        let settings = ConfigBuilder::builder()
            // Layer 1: environment variables with TTSFX_ prefix (e.g. TTSFX_SFX_API_KEY)
            .add_source(Environment::with_prefix("TTSFX"))
            // Layer 2: config file if it exists (missing is OK, defaults apply)
            .add_source(
                File::with_name("config")
                    .format(FileFormat::Toml)
                    .required(false),
            )
            .build()
            .map_err(|e| err!(Configuration, "failed to build config: {}", e))?;

        let raw: RawConfig = settings
            .try_deserialize()
            .map_err(|e| err!(Configuration, "failed to deserialize config: {}", e))?;

        Self::from_raw(raw)
    }

    /// Returns the effective volume target for a given pattern.
    pub fn get_volume_target(&self, filter: &PatternFilter) -> f64 {
        // Per-pattern override takes priority over global default.
        filter.volume_override_db.unwrap_or(self.volume_target_db)
    }

    #[track_caller]
    fn from_raw(raw: RawConfig) -> Result<Self> {
        // Compile patterns with graceful failure — invalid regexes are skipped, not fatal.
        let filters = raw
            .patterns
            .unwrap_or_default()
            .into_iter()
            .filter_map(|raw| match Regex::new(&raw.regex) {
                Ok(re) => Some(PatternFilter {
                    id: raw.id,
                    priority: raw.priority,
                    regex: re,
                    volume_override_db: raw.volume_override_db,
                }),
                Err(e) => {
                    tracing::warn!("skipping invalid regex for pattern '{}': {}", raw.id, e);
                    None
                }
            })
            .collect::<Vec<_>>();

        // Sort descending by priority, then ascending by id for stable order.
        let mut sorted = filters;
        sorted.sort_by(|a, b| b.priority.cmp(&a.priority).then_with(|| a.id.cmp(&b.id)));

        Ok(Self {
            tts_base_url: raw.tts_base_url,
            sfx_api_key: raw.sfx_api_key.unwrap_or_default(),
            volume_target_db: raw.volume_target_db,
            sample_rate: raw.sample_rate,
            cache_dir: raw.cache_dir.unwrap_or_else(|| PathBuf::from("sounds")),
            patterns: sorted,
        })
    }
}

// --- Default value helpers for serde ---
fn default_tts_base_url() -> String {
    "https://api.openai.com/v1".to_string()
}

fn default_priority() -> u32 {
    50
}

fn default_volume_target_db() -> f64 {
    -16.0
}

fn default_sample_rate() -> u32 {
    48_000
}
