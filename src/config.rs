use crate::utils::{PartialOrDefault, WarnOnError};
use crate::{err, utils::Partial, Result};
use config::{Config as ConfigBuilder, Environment, File, FileFormat};
use fancy_regex::Regex;
use serde::{Deserialize, Serialize};
use serde_with::{serde_as, DisplayFromStr, VecSkipError};
use std::path::{Path, PathBuf};

/// Default config template embedded from disk.
const DEFAULT_CONFIG: &str = include_str!("../config.toml");

/// Secret resolved at config load: inline string or `env = "VAR_NAME"`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum SecretSource {
    Literal(String),
    FromEnv { env: String },
}

impl SecretSource {
    pub fn resolve(&self) -> Result<String> {
        match self {
            SecretSource::Literal(value) => Ok(value.clone()),
            SecretSource::FromEnv { env } => std::env::var(env).map_err(|e| {
                err!(
                    Configuration,
                    "environment variable `{}` is not set or not valid UTF-8",
                    env,
                    @external: e
                )
            }),
        }
    }
}

/// Compute linear gain from dB target: 10^(db/20).
pub fn volume_gain(db: f64) -> f32 {
    10.0_f64.powf(db / 20.0) as f32
}

pub fn default_cache_tag() -> String {
    "default".into()
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
    /// Cache lookup scope; entries only match within the same tag.
    #[serde(default = "default_cache_tag")]
    pub cache_tag: String,
    /// Cosmetic labels for recipe templates only; pattern-level only (not in `[overridable]` or `overrides`).
    #[serde(default)]
    pub tags: Vec<String>,
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
#[serde(deny_unknown_fields)]
pub struct PatternConfig {
    /// TTS backend URL (e.g. "https://api.openai.com/v1").
    pub tts_base_url: String,

    /// ElevenLabs SFX API base URL (e.g. "https://api.elevenlabs.io").
    pub sfx_base_url: String,

    /// ElevenLabs API key: `sfx_api_key = "…"` or `sfx_api_key = { env = "VAR" }`.
    pub sfx_api_key: SecretSource,

    /// dB target for all segments before concat (default -16.0).
    pub volume_target_db: f64,

    /// Audio files + .json sidecar metadata (default "sounds/").
    pub cache_dir: PathBuf,

    pub match_mode: MatchMode,

    /// ElevenLabs prompt influence (0.0-1.0, default 0.3).
    /// Lower values give the model more creative freedom; higher values stick closer to the text prompt.
    pub sfx_prompt_influence: f32,

    /// SFX API `output_format` query value (e.g. `mp3`, `wav_48000`). Response must be a container ffmpeg can probe.
    pub sfx_output_format: String,

    /// Linear crossfade duration (ms) at joins involving this pattern's SFX. `0` = hard join at that boundary.
    #[serde(default = "default_crossfade_ms")]
    pub crossfade_ms: u32,

    /// How to slice surrounding text for recipe templates.
    pub context: Option<ContextExtraction>,

    /// Minijinja template for ElevenLabs + embeddings; default `{{ text }}` when unset.
    pub sfx_prompt_template: Option<String>,

    /// Optional ElevenLabs `duration_seconds` on sound-generation.
    pub sfx_duration_seconds: Option<f32>,

    /// OpenAI-compatible embeddings API base (e.g. `http://127.0.0.1:8080/v1`).
    #[serde(default)]
    pub embed_base_url: String,

    #[serde(default = "default_embed_model")]
    pub embed_model: String,

    #[serde(default = "default_embed_api_key")]
    pub embed_api_key: SecretSource,
}

fn default_crossfade_ms() -> u32 {
    12
}

fn default_embed_model() -> String {
    "text-embedding-3-small".into()
}

fn default_embed_api_key() -> SecretSource {
    SecretSource::Literal(String::new())
}

/// How many units (chars or sentences) to include on each side of the match.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ContextWindow {
    pub before: usize,
    pub after: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum ContextExtraction {
    Chars(ContextWindow),
    Sentences(ContextWindow),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
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
            sfx_api_key: SecretSource::Literal(String::new()),
            volume_target_db: 0.0,
            cache_dir: PathBuf::from("sound_cache"),
            match_mode: MatchMode::Levenshtein(LevenshteinMode { threshold: 1 }),
            sfx_prompt_influence: 0.8,
            sfx_output_format: "mp3_44100_128".into(),
            crossfade_ms: default_crossfade_ms(),
            context: None,
            sfx_prompt_template: None,
            sfx_duration_seconds: None,
            embed_base_url: String::new(),
            embed_model: default_embed_model(),
            embed_api_key: SecretSource::Literal(String::new()),
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
    #[serde(default = "default_sample_rate")]
    pub sample_rate: u32,

    /// Compiled filters from defaults + user overrides.
    #[serde_as(as = "VecSkipError<_, WarnOnError>")]
    pub patterns: Vec<ConfigPatternFilter>,
}

pub fn default_sample_rate() -> u32 {
    48_000
}

impl Config {
    /// Load configuration using layered defaults + TOML file (`config.toml` by default).
    pub fn load() -> Result<Self> {
        Self::load_from_path(Path::new("config.toml"))
    }

    /// Load from a specific TOML path. Only writes the embedded default when the path is `config.toml` and missing.
    pub fn load_from_path(path: &Path) -> Result<Self> {
        if path == Path::new("config.toml") && !path.exists() {
            std::fs::write(path, DEFAULT_CONFIG)
                .map_err(|e| err!(Io, "failed to write default config: {}", e))?;
        }

        let settings = ConfigBuilder::builder()
            .add_source(Environment::with_prefix("TTSFX"))
            .add_source(
                File::from(path)
                    .format(FileFormat::Toml)
                    .required(true),
            )
            .build()
            .map_err(|e| err!(Configuration, "failed to build config from {}", path.display(), @external: e))?;

        let raw: Config = settings
            .try_deserialize()
            .map_err(|e| err!(Configuration, "failed to deserialize config: {}", e))?;

        Self::from_raw(raw)
    }

    fn from_raw(mut raw: Config) -> Result<Self> {
        if matches!(raw.overridable.match_mode, MatchMode::Embeddings(_))
            && raw.overridable.embed_base_url.trim().is_empty()
        {
            return Err(err!(
                Configuration,
                "embed_base_url is required when match_mode is Embeddings"
            ));
        }
        raw.patterns
            .sort_by(|a, b| b.filter.priority.cmp(&a.filter.priority));
        Ok(raw)
    }
}
