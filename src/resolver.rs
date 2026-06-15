use crate::cache::CacheIndex;
use crate::config::{Config, MatchMode, PatternConfig};
use crate::embed::embed_text;
use crate::pattern::PatternMatch;
use crate::recipe::{RecipeContext, render_recipe};
use crate::{Result, err};
use reqwest::Client;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};

/// Resolution of a pattern match: either a cached file or a generation request.
#[derive(Debug)]
pub enum Resolution {
    Cache(CacheHit),
    Generate(Box<GenerateData>),
}

/// Data for a cache hit — the audio file to load and its per-pattern volume.
#[derive(Debug)]
pub struct CacheHit {
    pub path: PathBuf,
    pub volume_db: f64,
    pub crossfade_ms: u32,
}

/// Data needed to generate a new sound effect.
#[derive(Debug)]
pub struct GenerateData {
    pub name: Option<String>,
    /// Matched substring (cache identity for Levenshtein).
    pub text: String,
    pub recipe: String,
    pub cache_tag: String,
    pub pattern_config: PatternConfig,
    /// Query vector when resolved via embeddings mode (stored on cache write).
    pub query_embedding: Option<Vec<f32>>,
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
    pub async fn resolve_sfx(
        &self,
        m: &PatternMatch,
        full_input: &str,
        matched_text: &str,
    ) -> Result<Resolution> {
        let cpf = &self.config.patterns[m.pattern_index];
        let pattern_config = cpf
            .overrides
            .apply_some(&self.config.overridable)
            .unwrap_or_else(|e| {
                warn!(error = %e, "failed to merge pattern overrides, using global defaults");
                self.config.overridable.clone()
            });
        let volume_db = pattern_config.volume_target_db;
        let crossfade_ms = pattern_config.crossfade_ms;

        let recipe = render_recipe(
            pattern_config.sfx_prompt_template.as_deref(),
            &RecipeContext {
                context_mode: &pattern_config.context,
                full_input,
                start: m.start,
                end: m.end,
                text: matched_text,
                filter: &cpf.filter,
            },
        )
        .unwrap_or_else(|e| {
            warn!(error = %e, "recipe render failed, using matched text");
            matched_text.to_owned()
        });

        let mut query_embedding = None;

        match &pattern_config.match_mode {
            MatchMode::Levenshtein(m) => {
                if let Some(entry) =
                    self.cache
                        .find_match_levenshtein(&cpf.filter, m.threshold, matched_text)
                {
                    info!(path = %entry.filepath.display(), "cache hit");
                    return Ok(Resolution::Cache(CacheHit {
                        path: entry.filepath,
                        volume_db,
                        crossfade_ms,
                    }));
                }
            }
            MatchMode::Embeddings(m) => {
                let vector = embed_text(&self.client, &pattern_config, &recipe).await?;
                query_embedding = Some(vector.clone());
                if let Some(entry) =
                    self.cache
                        .find_match_embedding(&cpf.filter, m.threshold, &vector)
                {
                    info!(path = %entry.filepath.display(), "embedding cache hit");
                    return Ok(Resolution::Cache(CacheHit {
                        path: entry.filepath,
                        volume_db,
                        crossfade_ms,
                    }));
                }
            }
        }

        info!(text = %matched_text, recipe = %recipe, "cache miss, generating");
        Ok(Resolution::Generate(
            GenerateData {
                name: cpf.filter.name.clone(),
                text: matched_text.to_owned(),
                recipe,
                cache_tag: cpf.filter.cache_tag.clone(),
                pattern_config,
                query_embedding,
            }
            .into(),
        ))
    }

    /// Generate a sound effect via ElevenLabs API.
    #[tracing::instrument(
        level = "debug",
        skip(self),
        fields(text = %request.text, name = ?request.name),
        err(level = "warn")
    )]
    pub async fn generate(&self, request: &GenerateData) -> Result<Vec<u8>> {
        let api_key = request.pattern_config.sfx_api_key.resolve()?;

        let base = request.pattern_config.sfx_base_url.trim_end_matches('/');
        let format = &request.pattern_config.sfx_output_format;
        let url = format!("{base}/v1/sound-generation?output_format={format}");
        let mut req = self.client.post(&url);
        if !api_key.is_empty() {
            req = req.header("xi-api-key", api_key);
        }

        let mut body = serde_json::json!({
            "text": request.recipe,
            "prompt_influence": request.pattern_config.sfx_prompt_influence,
        });
        if let Some(secs) = request.pattern_config.sfx_duration_seconds {
            body["duration_seconds"] = serde_json::json!(secs);
        }

        let resp = req
            .json(&body)
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

