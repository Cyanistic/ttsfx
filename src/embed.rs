use crate::config::PatternConfig;
use crate::{Result, err};
use reqwest::Client;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct EmbeddingsResponse {
    data: Vec<EmbeddingData>,
}

#[derive(Debug, Deserialize)]
struct EmbeddingData {
    embedding: Vec<f32>,
}

pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let na: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let nb: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    dot / (na * nb)
}

pub async fn embed_text(client: &Client, config: &PatternConfig, text: &str) -> Result<Vec<f32>> {
    let base = config
        .embed_base_url
        .trim_end_matches('/')
        .to_string();
    if base.is_empty() {
        return Err(err!(
            Configuration,
            "embed_base_url is required for embeddings match mode"
        ));
    }
    let url = format!("{base}/embeddings");
    let api_key = config.embed_api_key.resolve().unwrap_or_default();

    let mut req = client.post(&url).json(&serde_json::json!({
        "model": config.embed_model,
        "input": text,
    }));
    if !api_key.is_empty() {
        req = req.bearer_auth(api_key);
    }

    let resp = req
        .send()
        .await
        .map_err(|e| err!(Network, "embeddings request failed: {}", url, @source: e))?
        .error_for_status()
        .map_err(|e| err!(Network, "embeddings backend error", @source: e))?;

    let body: EmbeddingsResponse = resp
        .json()
        .await
        .map_err(|e| err!(Serialization, "invalid embeddings response", @external: e))?;

    body.data
        .into_iter()
        .next()
        .map(|d| d.embedding)
        .ok_or_else(|| err!(Serialization, "embeddings response missing data"))
}
