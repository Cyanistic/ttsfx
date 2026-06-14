use crate::cache::CacheIndex;
use crate::config::{Config, PatternConfig};
use crate::pattern::PatternMatch;
use crate::{Result, bail, err};
use reqwest::Client;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};

/// Resolution of a pattern match: either a cached file or a generation request.
#[derive(Debug)]
pub enum Resolution {
    Cache(CacheHit),
    Generate(GenerateData),
}

/// Data for a cache hit — the audio file to load and its per-pattern volume.
#[derive(Debug)]
pub struct CacheHit {
    pub path: PathBuf,
    pub volume_db: f64,
}

/// Data needed to generate a new sound effect.
#[derive(Debug)]
pub struct GenerateData {
    pub name: Option<String>,
    pub text: String,
    pub pattern_config: PatternConfig,
}

/// Resolves pattern matches to audio sources (cache or generation).
pub struct SoundResolver {
    cache: CacheIndex,
    client: Client,
    config: Arc<Config>,
}

impl SoundResolver {
    pub fn new(config: Arc<Config>, cache: CacheIndex) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("failed to build HTTP client");

        Self {
            cache,
            client,
            config,
        }
    }

    /// Resolve a pattern match to a cache hit or generation request.
    pub fn resolve(&self, m: &PatternMatch, matched_text: &str) -> Resolution {
        let cpf = &self.config.patterns[m.pattern_index];
        let pattern_config = cpf
            .overrides
            .apply_some(&self.config.overridable)
            .unwrap_or_else(|e| {
                warn!(error = %e, "failed to merge pattern overrides, using global defaults");
                self.config.overridable.clone()
            });
        let volume_db = pattern_config.volume_target_db;

        // Try cache lookup
        if let Some(entry) = self
            .cache
            .find_match(&cpf.filter, &pattern_config, matched_text)
        {
            info!(path = %entry.filepath.display(), "cache hit");
            return Resolution::Cache(CacheHit {
                path: entry.filepath,
                volume_db,
            });
        }

        // Cache miss — need to generate
        info!(text = %matched_text, "cache miss, generating");
        Resolution::Generate(GenerateData {
            name: cpf.filter.name.clone(),
            text: matched_text.to_owned(),
            pattern_config,
        })
    }

    /// Generate a sound effect via ElevenLabs API.
    pub async fn generate(&self, request: &GenerateData) -> Result<Vec<u8>> {
        let api_key = &request.pattern_config.sfx_api_key;
        if api_key.is_empty() {
            bail!(Configuration, "sfx_api_key is not configured");
        }

        let url = format!(
            "{}/v1/sound-generation?output_format=pcm_24000",
            request.pattern_config.tts_base_url
        );
        let resp = self
            .client
            .post(&url)
            .header("xi-api-key", api_key)
            .json(&serde_json::json!({
                "text": request.text,
                "prompt_influence": request.pattern_config.sfx_prompt_influence,
            }))
            .send()
            .await
            .map_err(|e| err!(Network, "SFX generation request failed: {}", url, @source: e))?
            .error_for_status()
            .map_err(|e| err!(Network, "SFX backend returned error", @source: e))?;

        resp.bytes()
            .await
            .map(|b| b.to_vec())
            .map_err(|e| err!(Network, "failed to read SFX response body", @source: e))
    }
}
