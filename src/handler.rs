use std::path::PathBuf;
use std::sync::Arc;

use crate::audio::{self, AudioSegment};
use crate::config::volume_gain;
use crate::cache::{CacheMeta, write_metadata};

use crate::pattern::{PatternMatch, match_patterns};
use crate::resolver::{GenerateData, Resolution};
use crate::state::{AppState, Fragment, FragmentKind, ResolvedAudio};
use crate::{Result, ensure, err};
use axum::Json;
use axum::extract::State;
use axum::http::header::{
    CONNECTION, CONTENT_LENGTH, CONTENT_TYPE, HOST, PROXY_AUTHENTICATE, PROXY_AUTHORIZATION, TE,
    TRAILER, TRANSFER_ENCODING, UPGRADE,
};
use axum::http::{HeaderMap, HeaderName, StatusCode};
use axum::response::{IntoResponse, Response};
use futures_util::future::join_all;
use tracing::{debug, info};

use serde::{Deserialize, Serialize};

/// OpenAI-style `response_format` for `/v1/audio/speech` (subset we encode).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SpeechResponseFormat {
    #[default]
    Wav,
    #[serde(alias = "mpeg")]
    Mp3,
}

impl SpeechResponseFormat {
    pub fn content_type(self) -> &'static str {
        match self {
            Self::Wav => "audio/wav",
            Self::Mp3 => "audio/mpeg",
        }
    }
}

/// Upstream TTS parameters shared across every text chunk in one client request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TtsSettings {
    pub model: String,
    pub voice: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct ProxyRequest {
    #[serde(flatten)]
    pub settings: TtsSettings,
    pub input: String,
    #[serde(default)]
    pub response_format: SpeechResponseFormat,
}

#[tracing::instrument(
    level = "info",
    skip(state, headers, body),
    fields(
        model = %body.settings.model,
        voice = %body.settings.voice,
        response_format = ?body.response_format,
        input_len = body.input.len(),
    ),
    err(level = "warn")
)]
pub async fn handle_speech(
    State(state): State<AppState>,
    mut headers: HeaderMap,
    Json(body): Json<ProxyRequest>,
) -> Result<Response> {
    ensure!(!body.input.is_empty(), Validation, "input text is empty");

    let patterns: Vec<_> = state.config.patterns.iter().map(|p| &p.filter).collect();
    let mut matches = match_patterns(patterns.into_iter(), &body.input);
    matches.sort_by_key(|m| m.start);

    let fragments = build_fragments(&body.input, &matches);
    ensure!(
        !fragments.is_empty(),
        Validation,
        "no audio fragments to synthesize"
    );
    debug!(sfx_matches = matches.len(), fragments = fragments.len(), "speech request split");

    let sample_rate = state.config.sample_rate;
    let settings = body.settings.clone();
    forward_client_headers(&mut headers);
    let forward_headers = Arc::new(headers);

    let futures: Vec<_> = fragments
        .into_iter()
        .map(|frag| {
            let state = state.clone();
            let settings = settings.clone();
            let forward_headers = Arc::clone(&forward_headers);
            async move { resolve_fragment(&state, frag, &settings, forward_headers.as_ref()).await }
        })
        .collect();

    let resolved: Vec<FragmentOutcome> = join_all(futures)
        .await
        .into_iter()
        .collect::<Result<Vec<_>>>()?;

    let mut segments: Vec<AudioSegment> = Vec::new();
    let mut crossfade_ms: Vec<u32> = Vec::new();
    let mut ref_rms: Option<f32> = None;

    for item in resolved {
        crossfade_ms.push(item.crossfade_ms);
        let mut seg = match item.audio {
            ResolvedAudio::Bytes(b) => audio::decode(&b, sample_rate).await?,
            ResolvedAudio::File(p) => audio::decode_file(&p, sample_rate).await?,
        };
        if item.kind == FragmentKind::Tts {
            if ref_rms.is_none() {
                ref_rms = Some(audio::rms_level(&seg.samples));
            }
        } else if let Some(target) = ref_rms {
            audio::match_loudness(&mut seg, target);
            let gain = volume_gain(item.volume_db);
            for s in &mut seg.samples {
                *s = (*s * gain).clamp(-1.0, 1.0);
            }
        }
        segments.push(seg);
    }

    let join_crossfade_ms: Vec<u32> = (0..segments.len().saturating_sub(1))
        .map(|i| crossfade_for_join(crossfade_ms[i], crossfade_ms[i + 1]))
        .collect();
    let join_crossfade_samples: Vec<usize> = join_crossfade_ms
        .iter()
        .map(|ms| ms_to_samples(*ms, sample_rate))
        .collect();

    let merged = audio::concat_segments_crossfaded_variable(segments, &join_crossfade_samples)
        .map_err(|_| err!(Internal, "audio segments sample rate or channel mismatch"))?;

    let format = body.response_format;
    let bytes = match format {
        SpeechResponseFormat::Mp3 => audio::encode_mp3(&merged.samples, merged.sample_rate).await?,
        SpeechResponseFormat::Wav => audio::encode_wav(&merged.samples, merged.sample_rate).await?,
    };

    info!(
        bytes = bytes.len(),
        duration_ms = (merged.samples.len() as u64 * 1000 / merged.sample_rate as u64),
        "speech response ready"
    );

    Ok((
        StatusCode::OK,
        [(CONTENT_TYPE, format.content_type())],
        bytes,
    )
        .into_response())
}

fn ms_to_samples(ms: u32, sample_rate: u32) -> usize {
    (ms as u64 * sample_rate as u64 / 1000) as usize
}

/// Crossfade when either side of the join is SFX (uses that pattern's `crossfade_ms`).
fn crossfade_for_join(left_ms: u32, right_ms: u32) -> u32 {
    left_ms.max(right_ms)
}

fn build_fragments(input: &str, matches: &[PatternMatch]) -> Vec<Fragment> {
    let mut out = Vec::new();
    let mut cursor = 0;
    for m in matches {
        if m.start > cursor {
            let text = input[cursor..m.start].to_string();
            if !text.is_empty() {
                out.push(Fragment::Tts { text });
            }
        }
        out.push(Fragment::Sfx {
            pattern_index: m.pattern_index,
            text: input[m.start..m.end].to_string(),
        });
        cursor = m.end;
    }
    if cursor < input.len() {
        let text = input[cursor..].to_string();
        if !text.is_empty() {
            out.push(Fragment::Tts { text });
        }
    }
    out
}

struct FragmentOutcome {
    kind: FragmentKind,
    audio: ResolvedAudio,
    volume_db: f64,
    crossfade_ms: u32,
}

async fn resolve_fragment(
    state: &AppState,
    frag: Fragment,
    settings: &TtsSettings,
    forward_headers: &HeaderMap,
) -> Result<FragmentOutcome> {
    match frag {
        Fragment::Tts { text } => {
            let bytes = forward_tts(state, settings, &text, forward_headers).await?;
            Ok(FragmentOutcome {
                kind: FragmentKind::Tts,
                audio: ResolvedAudio::Bytes(bytes),
                volume_db: state.config.overridable.volume_target_db,
                crossfade_ms: 0,
            })
        }
        Fragment::Sfx {
            pattern_index,
            text,
        } => {
            let m = PatternMatch {
                start: 0,
                end: text.len(),
                pattern_index,
            };
            match state.resolver.resolve(&m, &text) {
                Resolution::Cache(hit) => Ok(FragmentOutcome {
                    kind: FragmentKind::Sfx,
                    audio: ResolvedAudio::File(hit.path),
                    volume_db: hit.volume_db,
                    crossfade_ms: hit.crossfade_ms,
                }),
                Resolution::Generate(generate_data) => {
                    let (bytes, _path) = generate_and_cache(state, &generate_data).await?;
                    Ok(FragmentOutcome {
                        kind: FragmentKind::Sfx,
                        audio: ResolvedAudio::Bytes(bytes),
                        volume_db: generate_data.pattern_config.volume_target_db,
                        crossfade_ms: generate_data.pattern_config.crossfade_ms,
                    })
                }
            }
        }
    }
}

#[tracing::instrument(
    level = "debug",
    skip(state, settings, forward_headers),
    fields(input_len = input.len(), model = %settings.model, voice = %settings.voice),
    err(level = "warn")
)]
async fn forward_tts(
    state: &AppState,
    settings: &TtsSettings,
    input: &str,
    forward_headers: &HeaderMap,
) -> Result<Vec<u8>> {
    let url = format!(
        "{}/audio/speech",
        state.config.overridable.tts_base_url.trim_end_matches('/')
    );
    let client = reqwest::Client::new();
    let body = ProxyRequest {
        settings: settings.clone(),
        input: input.to_string(),
        response_format: SpeechResponseFormat::Wav,
    };
    let mut req = client.post(&url).json(&body);
    for (name, value) in forward_headers.iter() {
        req = req.header(name, value);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| err!(Network, "TTS request failed: {}", url, @source: e))?
        .error_for_status()
        .map_err(|e| err!(Network, "TTS backend error", @source: e))?;
    resp.bytes()
        .await
        .map(|b| b.to_vec())
        .map_err(|e| err!(Network, "failed to read TTS body", @source: e))
}

#[tracing::instrument(
    level = "debug",
    skip(state),
    fields(text = %generate_data.text, name = ?generate_data.name),
    err(level = "warn")
)]
async fn generate_and_cache(
    state: &AppState,
    generate_data: &GenerateData,
) -> Result<(Vec<u8>, PathBuf)> {
    let audio_bytes = state.resolver.generate(generate_data).await?;
    let ext = sfx_cache_extension(&generate_data.pattern_config.sfx_output_format);
    let hash = blake3::hash(&audio_bytes).to_hex().to_string();
    let filename = format!("{hash}.{ext}");
    let filepath = state.config.overridable.cache_dir.join(&filename);
    tokio::fs::write(&filepath, &audio_bytes)
        .await
        .map_err(|e| err!(Io, "failed to write cache file: {}", filepath.display(), @source: e))?;
    let meta = CacheMeta {
        matched_text: generate_data.text.clone(),
        context: None,
        version: String::new(),
    };
    write_metadata(&filepath, &meta).await?;
    info!(path = %filepath.display(), bytes = audio_bytes.len(), "sfx cached");
    Ok((audio_bytes, filepath))
}

/// Strip hop-by-hop / body headers unsuitable for upstream TTS sub-requests (`json()` sets body headers).
fn forward_client_headers(headers: &mut HeaderMap) {
    const SKIP: &[HeaderName] = &[
        HOST,
        CONNECTION,
        CONTENT_LENGTH,
        CONTENT_TYPE,
        TRANSFER_ENCODING,
        TE,
        TRAILER,
        UPGRADE,
        PROXY_AUTHORIZATION,
        PROXY_AUTHENTICATE,
    ];

    for name in SKIP {
        headers.remove(name);
    }
}

fn sfx_cache_extension(output_format: &str) -> &'static str {
    let f = output_format.to_ascii_lowercase();
    if f.starts_with("wav") { "wav" } else { "mp3" }
}
